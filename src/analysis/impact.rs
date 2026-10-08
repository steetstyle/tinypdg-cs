use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use crate::analysis::callgraph::{CallGraph, CallSite};
use crate::resolve::types::TypeGraph;

/// Result of an impact analysis: nodes (method → direct caller count + transitive count) and edges (caller → callee).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ImpactGraph {
    /// Map from node_id (Class.Method) → (direct_caller_count, transitive_caller_count)
    pub nodes: BTreeMap<String, (usize, usize)>,
    /// Edges from caller → callee (deduplicated)
    pub edges: Vec<(String, String)>,
    /// The target method that was analyzed
    pub target: String,
    /// HTTP routes whose handler is the target method.
    ///
    /// A minimal-API handler is reached by a method group reference
    /// (`MapGet("/x", Type.Handler)`), which the call graph does not record, so
    /// without this an endpoint handler reports zero callers and zero reachable
    /// URLs. `registrar` names the code that wires the route up, when known.
    #[serde(default)]
    pub routes: Vec<RouteBinding>,
}

/// A route that reaches a method.
#[derive(Debug, Clone, serde::Serialize)]
pub struct RouteBinding {
    pub http_method: String,
    pub pattern: String,
    /// `Class.Method` performing the registration, when resolvable.
    pub registrar: Option<String>,
}

impl ImpactGraph {
    pub fn target_callers(&self) -> usize {
        self.nodes.get(&self.target).map(|(_, t)| *t).unwrap_or(0)
    }
}

/// Build an ImpactGraph for a given class.method by tracing all transitive callers.
pub fn build_impact_graph(
    path: &Path,
    class: &str,
    method: &str,
) -> anyhow::Result<(ImpactGraph, TypeGraph, CallGraph)> {
    let (tg, cg) = crate::cli::commands::load_project(path)?;

    // Verify the target method exists
    let method_exists = tg
        .classes
        .get(class)
        .map(|c| c.methods.iter().any(|m| m.method == method))
        .unwrap_or(false);

    if !method_exists {
        let candidates: Vec<String> = tg
            .classes
            .iter()
            .filter(|(cn, ci)| {
                cn.to_lowercase() == class.to_lowercase()
                    && ci
                        .methods
                        .iter()
                        .any(|m| m.method.to_lowercase() == method.to_lowercase())
            })
            .map(|(cn, _)| cn.clone())
            .collect();
        if candidates.len() == 1 {
            let actual_class = &candidates[0];
            let actual_method = tg.classes[actual_class]
                .methods
                .iter()
                .find(|m| m.method.to_lowercase() == method.to_lowercase())
                .map(|m| m.method.clone())
                .unwrap();
            let msg = format!(
                "Method '{}' not found in '{}'. Did you mean '{}' in '{}'?",
                method, class, actual_method, actual_class
            );
            anyhow::bail!("{}", msg);
        }
        if !candidates.is_empty() {
            let msg = format!(
                "Method '{}' not found in '{}'. Candidates: {}",
                method,
                class,
                candidates.join(", ")
            );
            anyhow::bail!("{}", msg);
        }
        anyhow::bail!("Method '{}' not found in class '{}'", method, class);
    }

    // BFS through reverse call graph
    let mut nodes: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut edges: Vec<(String, String)> = Vec::new();
    let mut visited: HashSet<(String, String)> = HashSet::new();
    let mut queue: Vec<(String, String)> = Vec::new();

    visited.insert((class.to_string(), method.to_string()));
    queue.push((class.to_string(), method.to_string()));

    // Routes whose handler is the target. Collected up front so the BFS can
    // treat a registration site as a direct caller of the endpoint handler.
    let (routes, route_callers) = routes_for_target(path, class, method);

    while let Some((cls, mtd)) = queue.pop() {
        let direct_callers: Vec<&CallSite> = cg
            .calls
            .iter()
            .filter(|c| c.callee == mtd && c.callee_class == cls)
            .collect();
        let direct_count = direct_callers.len();
        // Clone the route-derived callers so the borrow of `cg` stays immutable.
        let mut route_caller_ids: Vec<String> = Vec::new();
        if cls == class && mtd == method {
            for caller in &route_callers {
                if !direct_callers
                    .iter()
                    .any(|c| format!("{}.{}", c.caller_class, c.caller_method) == *caller)
                {
                    route_caller_ids.push(caller.clone());
                }
            }
        }

        let call_count = direct_count + route_caller_ids.len();
        let node_id = format!("{}.{}", cls, mtd);
        nodes
            .entry(node_id.clone())
            .and_modify(|(_, c)| {
                if call_count > *c {
                    *c = call_count;
                }
            })
            .or_insert((call_count, 0));

        for caller_id in route_caller_ids {
            edges.push((caller_id.clone(), node_id.clone()));
            if let Some((c_cls, c_mtd)) = caller_id.split_once('.') {
                let caller_key = (c_cls.to_string(), c_mtd.to_string());
                if visited.insert(caller_key.clone()) {
                    queue.push(caller_key);
                }
            }
        }

        for call in &direct_callers {
            let caller_id = format!("{}.{}", call.caller_class, call.caller_method);
            edges.push((caller_id, node_id.clone()));

            let caller_key = (call.caller_class.clone(), call.caller_method.clone());
            if visited.insert(caller_key.clone()) {
                queue.push(caller_key);
            }
        }
    }

    // Compute transitive caller counts
    let mut incoming: HashMap<String, Vec<String>> = HashMap::new();
    for (caller, callee) in &edges {
        incoming
            .entry(callee.clone())
            .or_default()
            .push(caller.clone());
    }

    fn compute_transitive(
        node: &str,
        incoming: &HashMap<String, Vec<String>>,
        cache: &mut HashMap<String, usize>,
    ) -> usize {
        if let Some(&cached) = cache.get(node) {
            return cached;
        }
        let mut total = 0;
        if let Some(callers) = incoming.get(node) {
            for caller in callers {
                total += 1;
                total += compute_transitive(caller, incoming, cache);
            }
        }
        cache.insert(node.to_string(), total);
        total
    }

    let target_key = format!("{}.{}", class, method);
    let mut cache = HashMap::new();
    let node_ids: Vec<String> = nodes.keys().cloned().collect();
    for node_id in &node_ids {
        let t = compute_transitive(node_id, &incoming, &mut cache);
        if let Some(entry) = nodes.get_mut(node_id) {
            entry.1 = t;
        }
    }

    // Deduplicate edges
    let mut edge_set: HashSet<(String, String)> = HashSet::new();
    edges.retain(|e| edge_set.insert(e.clone()));

    let graph = ImpactGraph {
        nodes,
        edges,
        target: target_key,
        routes,
    };

    Ok((graph, tg, cg))
}

/// Routes bound to `class.method`, plus the distinct registration sites that
/// reach it.
///
/// The class comparison is case-insensitive to match the lenient lookup used for
/// the target method itself.
fn routes_for_target(path: &Path, class: &str, method: &str) -> (Vec<RouteBinding>, Vec<String>) {
    let table = match crate::route::extractor::extract(&path.to_string_lossy()) {
        Ok(t) => t,
        // Route extraction is best-effort here: a project without routes still
        // gets call-graph impact analysis.
        Err(_) => return (Vec::new(), Vec::new()),
    };

    let class_lower = class.to_lowercase();
    let mut bindings = Vec::new();
    let mut registrars: Vec<String> = Vec::new();
    for entry in table.routes {
        if entry.class.to_lowercase() != class_lower || entry.handler != method {
            continue;
        }
        if let Some(ref registrar) = entry.registrar {
            if !registrars.contains(registrar) {
                registrars.push(registrar.clone());
            }
        }
        bindings.push(RouteBinding {
            http_method: entry.http_method,
            pattern: entry.path,
            registrar: entry.registrar,
        });
    }
    bindings.sort_by(|a, b| {
        a.http_method
            .cmp(&b.http_method)
            .then_with(|| a.pattern.cmp(&b.pattern))
    });
    (bindings, registrars)
}

/// Render an ImpactGraph as DOT string.
pub fn impact_to_dot(ig: &ImpactGraph, title: &str) -> String {
    let mut dot =
        String::from("digraph Impact {\n  rankdir=BT;\n  node [shape=box style=rounded];\n\n");
    dot.push_str(&format!(
        "  label=\"{}\";\n  labelloc=t;\n  fontsize=14;\n\n",
        title
    ));

    for (node_id, (_direct, transitive)) in &ig.nodes {
        let label = node_id.replace('"', "'");
        let is_target = node_id == &ig.target;
        let (fill, style) = if is_target {
            ("lightcoral", "filled")
        } else {
            ("lightblue", "filled")
        };
        let extra = if is_target { " penwidth=2" } else { "" };
        let total_label = format!("{} (affects {} sites)", label, transitive);
        dot.push_str(&format!(
            "  \"{}\" [label=\"{}\" style={style} fillcolor={fill}{extra}];\n",
            node_id, total_label
        ));
    }

    dot.push('\n');
    let mut edge_set: HashSet<(String, String)> = HashSet::new();
    for (caller, callee) in &ig.edges {
        if edge_set.insert((caller.clone(), callee.clone())) {
            dot.push_str(&format!("  \"{}\" -> \"{}\";\n", caller, callee));
        }
    }

    dot.push_str("}\n");
    dot
}
