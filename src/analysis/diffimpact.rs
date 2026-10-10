use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use crate::analysis::callgraph::CallGraph;
use crate::analysis::impact::{build_impact_graph, ImpactGraph};
use crate::cli::commands::load_project;

/// A change record between two versions.
#[derive(Debug, Clone, serde::Serialize)]
pub enum ChangeKind {
    Added,
    Removed,
    CallersChanged {
        removed_callers: Vec<String>,
        added_callers: Vec<String>,
    },
}

/// Result of a diff-impact analysis.
pub struct DiffImpactResult {
    /// All changed methods with their change kinds.
    pub changes: BTreeMap<String, ChangeKind>,
    /// Combined impact graph for all changes (v2 callers of changed methods).
    pub impact: ImpactGraph,
    /// Full method set in v2 (for context).
    pub v2_methods: HashSet<String>,
}

/// Does this look like a type name rather than a variable?
fn is_type_like(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_uppercase() => {}
        _ => return false,
    }
    s.chars().all(|c| c.is_alphanumeric() || c == '_')
}

/// Best-effort owning class for a call site.
///
/// `CallSite::callee_class` is empty whenever the resolver could not type the
/// receiver, and the common case is a temporary instance: `new Shared().Run()`
/// leaves it blank. `target_expr` still names the type in that shape, so recover
/// it rather than emitting a nameless `.Run` key that no impact lookup can use.
///
/// Returns the class and whether it was inferred. An inferred name is a guess —
/// `target_expr` is also a variable name — so callers that need certainty should
/// treat it as such.
pub fn resolve_callee_class(c: &crate::analysis::callgraph::CallSite) -> (String, bool) {
    if !c.callee_class.is_empty() {
        return (c.callee_class.clone(), false);
    }
    let expr = c.target_expr.trim();

    // `new Shared()`, `new Outer.Inner()`
    if let Some(rest) = expr.strip_prefix("new ") {
        let type_name = rest.trim().trim_end_matches("()");
        let simple = type_name.rsplit('.').next().unwrap_or(type_name);
        if is_type_like(simple) {
            return (simple.to_string(), true);
        }
        return (String::new(), false);
    }

    // `Shared.Run()` — a bare, type-shaped name.
    if !expr.is_empty() && !expr.contains(['.', '(', ' ']) && is_type_like(expr) {
        return (expr.to_string(), true);
    }

    // Anything else is a variable or an unresolvable expression. Guessing here
    // would attribute a call to an arbitrary type.
    (String::new(), false)
}

/// Methods present in one call graph, as `Class.Method` keys.
fn method_keys(cg: &CallGraph) -> HashSet<(String, String)> {
    let mut keys = HashSet::new();
    for c in &cg.calls {
        let (callee_class, _) = resolve_callee_class(c);
        // Skip calls whose owner could not be determined: a `.Run` key collides
        // across every unresolved type and would merge unrelated methods.
        if !callee_class.is_empty() {
            keys.insert((callee_class, c.callee.clone()));
        }
        if !c.caller_class.is_empty() {
            keys.insert((c.caller_class.clone(), c.caller_method.clone()));
        }
    }
    keys
}

/// Compute the changes between two versions from their call graphs.
///
/// Pure: no parsing, so a caller that already holds both graphs pays for the
/// parse once. `build_diff_impact` uses this; the MCP tool uses it directly
/// instead of re-parsing v2 to build an impact graph afterwards.
pub fn compute_changes(cg1: &CallGraph, cg2: &CallGraph) -> BTreeMap<String, ChangeKind> {
    let keys1 = method_keys(cg1);
    let keys2 = method_keys(cg2);

    let mut changes: BTreeMap<String, ChangeKind> = BTreeMap::new();

    for key in keys2.difference(&keys1) {
        changes.insert(format!("{}.{}", key.0, key.1), ChangeKind::Added);
    }
    for key in keys1.difference(&keys2) {
        changes.insert(format!("{}.{}", key.0, key.1), ChangeKind::Removed);
    }

    // A method whose caller set changed is the interesting case for a regression:
    // something that used to call it no longer does.
    let mut callers1: HashMap<String, Vec<String>> = HashMap::new();
    let mut callers2: HashMap<String, Vec<String>> = HashMap::new();
    for c in &cg1.calls {
        let (callee_class, _) = resolve_callee_class(c);
        if callee_class.is_empty() || c.caller_class.is_empty() {
            continue;
        }
        callers1
            .entry(format!("{}.{}", callee_class, c.callee))
            .or_default()
            .push(format!("{}.{}", c.caller_class, c.caller_method));
    }
    for c in &cg2.calls {
        let (callee_class, _) = resolve_callee_class(c);
        if callee_class.is_empty() || c.caller_class.is_empty() {
            continue;
        }
        callers2
            .entry(format!("{}.{}", callee_class, c.callee))
            .or_default()
            .push(format!("{}.{}", c.caller_class, c.caller_method));
    }

    for key in keys2.intersection(&keys1) {
        let key_string = format!("{}.{}", key.0, key.1);
        let empty = Vec::new();
        let v1_callers = callers1.get(&key_string).unwrap_or(&empty);
        let v2_callers = callers2.get(&key_string).unwrap_or(&empty);
        let v1_set: HashSet<&str> = v1_callers.iter().map(|s| s.as_str()).collect();
        let v2_set: HashSet<&str> = v2_callers.iter().map(|s| s.as_str()).collect();
        let mut removed_callers: Vec<String> =
            v1_set.difference(&v2_set).map(|s| s.to_string()).collect();
        let mut added_callers: Vec<String> =
            v2_set.difference(&v1_set).map(|s| s.to_string()).collect();
        if !removed_callers.is_empty() || !added_callers.is_empty() {
            removed_callers.sort();
            added_callers.sort();
            changes.insert(
                key_string,
                ChangeKind::CallersChanged {
                    removed_callers,
                    added_callers,
                },
            );
        }
    }

    changes
}

/// Merge several impact graphs into one.
///
/// Node counts take the maximum rather than summing: a node reached from two
/// changed methods is still one caller site, and summing would overstate the
/// blast radius of a change set.
pub fn merge_impact_graphs(graphs: Vec<ImpactGraph>) -> ImpactGraph {
    let mut nodes: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut edges: Vec<(String, String)> = Vec::new();
    let mut seen_edges: HashSet<(String, String)> = HashSet::new();
    let mut targets: Vec<String> = Vec::new();

    for g in graphs {
        for (node, (direct, transitive)) in g.nodes {
            let entry = nodes.entry(node).or_insert((0, 0));
            entry.0 = entry.0.max(direct);
            entry.1 = entry.1.max(transitive);
        }
        for e in g.edges {
            if seen_edges.insert(e.clone()) {
                edges.push(e);
            }
        }
        if !g.target.is_empty() && !targets.contains(&g.target) {
            targets.push(g.target.clone());
        }
    }

    ImpactGraph {
        nodes,
        edges,
        // Several changed methods can contribute; list them rather than picking
        // one arbitrarily, so the caller can see the full change set.
        target: targets.join(", "),
        routes: Vec::new(),
    }
}

/// Compare two versions of a project and find changes + affected places.
///
/// Kept as the path-based entry point for the CLI. The MCP tool calls
/// [`compute_changes`] directly so it can build impact graphs from the graphs it
/// already parsed — going through here would parse v2 a second time.
pub fn build_diff_impact(
    v1_path: &Path,
    v2_path: &Path,
    target_class: &str,
    target_method: &str,
) -> anyhow::Result<DiffImpactResult> {
    let (_tg1, cg1) = load_project(v1_path)?;
    let (_tg2, cg2) = load_project(v2_path)?;

    let changes = compute_changes(&cg1, &cg2);

    let v2_method_set: HashSet<String> = method_keys(&cg2)
        .iter()
        .map(|(c, m)| format!("{}.{}", c, m))
        .collect();

    // Changed methods that still exist in v2 are the ones worth tracing: a
    // removed method has no callers left to find.
    let all_changed_v2: Vec<(String, String)> = changes
        .keys()
        .filter(|k| v2_method_set.contains(*k))
        .filter_map(|k| k.split_once('.'))
        .map(|(c, m)| (c.to_string(), m.to_string()))
        .collect();

    // If a specific target is given, build impact for that; otherwise fall back
    // to the first changed method that survived into v2.
    let target_key = format!("{}.{}", target_class, target_method);
    let has_target = changes.contains_key(&target_key) || v2_method_set.contains(&target_key);
    let impact = if has_target {
        build_impact_graph(v2_path, target_class, target_method)?.0
    } else if let Some((cls, mtd)) = all_changed_v2.first() {
        build_impact_graph(v2_path, cls, mtd)?.0
    } else {
        ImpactGraph {
            nodes: BTreeMap::new(),
            edges: Vec::new(),
            target: String::new(),
            routes: Vec::new(),
        }
    };

    Ok(DiffImpactResult {
        changes,
        impact,
        v2_methods: v2_method_set,
    })
}

/// Render a DiffImpactResult as DOT.
pub fn diff_impact_to_dot(result: &DiffImpactResult, title: &str) -> String {
    let mut dot = String::new();

    // Summary section
    dot.push_str("digraph DiffImpact {\n  rankdir=BT;\n  node [shape=box style=rounded];\n\n");
    dot.push_str(&format!(
        "  label=\"{}\";\n  labelloc=t;\n  fontsize=14;\n\n",
        title
    ));

    // Render impact graph first (with coloring for changed nodes)
    if !result.impact.target.is_empty() {
        // Reuse impact rendering, but overlay change colors
        for (node_id, (_direct, transitive)) in &result.impact.nodes {
            let label = node_id.replace('"', "'");
            let is_target = node_id == &result.impact.target;
            let change_info = result.changes.get(node_id);
            let (fill, extra) = if is_target {
                ("lightcoral", " penwidth=2")
            } else if change_info.is_some() {
                ("lightyellow", " penwidth=2")
            } else {
                ("lightblue", "")
            };
            let total_label = format!("{} (affects {} sites)", label, transitive);
            dot.push_str(&format!(
                "  \"{}\" [label=\"{}\" style=filled fillcolor={fill}{extra}];\n",
                node_id, total_label
            ));
        }
        dot.push('\n');
        let mut edge_set: HashSet<(String, String)> = HashSet::new();
        for (caller, callee) in &result.impact.edges {
            if edge_set.insert((caller.clone(), callee.clone())) {
                dot.push_str(&format!("  \"{}\" -> \"{}\";\n", caller, callee));
            }
        }
    } else {
        // No specific target — list all changed methods as isolated nodes
        dot.push_str("  // Changed methods (no specific target)\n");
        for (key, change) in &result.changes {
            let kind_label = match change {
                ChangeKind::Added => "ADDED",
                ChangeKind::Removed => "REMOVED",
                ChangeKind::CallersChanged { .. } => "CALLERS CHANGED",
            };
            let fill = match change {
                ChangeKind::Added => "lightgreen",
                ChangeKind::Removed => "lightcoral",
                ChangeKind::CallersChanged { .. } => "lightyellow",
            };
            dot.push_str(&format!(
                "  \"{}\" [label=\"{}\\n{}\" style=filled fillcolor={fill}];\n",
                key, key, kind_label
            ));
        }
    }

    dot.push_str("}\n");
    dot
}
