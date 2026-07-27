use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use crate::analysis::callgraph::CallGraph;
use crate::analysis::impact::{build_impact_graph, ImpactGraph};
use crate::cli::commands::load_project;

/// A change record between two versions.
#[derive(Debug)]
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

/// Compare two versions of a project and find changes + affected places.
pub fn build_diff_impact(v1_path: &Path, v2_path: &Path, target_class: &str, target_method: &str) -> anyhow::Result<DiffImpactResult> {
    let (_tg1, cg1) = load_project(v1_path)?;
    let (_tg2, cg2) = load_project(v2_path)?;

    // Build method signatures for comparison: "Class.Method"
    fn method_keys(cg: &CallGraph) -> HashSet<(String, String)> {
        let mut keys = HashSet::new();
        for c in &cg.calls {
            keys.insert((c.callee_class.clone(), c.callee.clone()));
            keys.insert((c.caller_class.clone(), c.caller_method.clone()));
        }
        keys
    }

    let keys1 = method_keys(&cg1);
    let keys2 = method_keys(&cg2);

    // Find what's different
    let mut changes: BTreeMap<String, ChangeKind> = BTreeMap::new();

    // Methods only in v1 (removed in v2)
    for (cls, mtd) in &keys1 {
        let key = format!("{}.{}", cls, mtd);
        if !keys2.contains(&(cls.clone(), mtd.clone())) {
            changes.insert(key, ChangeKind::Removed);
        }
    }

    // Methods only in v2 (added)
    for (cls, mtd) in &keys2 {
        let key = format!("{}.{}", cls, mtd);
        if !keys1.contains(&(cls.clone(), mtd.clone())) {
            changes.entry(key).or_insert(ChangeKind::Added);
        }
    }

    // Methods with different callers
    let caller_map = |cg: &CallGraph| -> HashMap<String, Vec<String>> {
        let mut m: HashMap<String, Vec<String>> = HashMap::new();
        for c in &cg.calls {
            let key = format!("{}.{}", c.callee_class, c.callee);
            m.entry(key).or_default().push(format!("{}.{}", c.caller_class, c.caller_method));
        }
        m
    };

    let callers1 = caller_map(&cg1);
    let callers2 = caller_map(&cg2);

    for (key, v2_callers) in &callers2 {
        if !changes.contains_key(key) {
            let v1_callers = callers1.get(key).cloned().unwrap_or_default();
            let v1_set: HashSet<&str> = v1_callers.iter().map(|s| s.as_str()).collect();
            let v2_set: HashSet<&str> = v2_callers.iter().map(|s| s.as_str()).collect();
            let removed_callers: Vec<String> = v1_set.difference(&v2_set).map(|s| s.to_string()).collect();
            let added_callers: Vec<String> = v2_set.difference(&v1_set).map(|s| s.to_string()).collect();
            if !removed_callers.is_empty() || !added_callers.is_empty() {
                changes.insert(key.clone(), ChangeKind::CallersChanged { removed_callers, added_callers });
            }
        }
    }

    // Build combined impact: for each changed method (focus on v2), trace callers in v2
    let mut all_changed_v2: Vec<(String, String)> = Vec::new();
    let v2_method_set: HashSet<String> = keys2.iter().map(|(c, m)| format!("{}.{}", c, m)).collect();

    for key in changes.keys() {
        if v2_method_set.contains(key) {
            if let Some((cls, mtd)) = key.split_once('.') {
                all_changed_v2.push((cls.to_string(), mtd.to_string()));
            }
        }
    }

    // If a specific target is given, also build impact for that
    let target_key = format!("{}.{}", target_class, target_method);
    let has_target = changes.contains_key(&target_key) || v2_method_set.contains(&target_key);
    let impact = if has_target {
        // Build impact for the target method in v2
        build_impact_graph(v2_path, target_class, target_method)?.0
    } else {
        // Fallback: build impact from the first changed method in v2
        if let Some((cls, mtd)) = all_changed_v2.first() {
            build_impact_graph(v2_path, cls, mtd)?.0
        } else {
            ImpactGraph {
                nodes: BTreeMap::new(),
                edges: Vec::new(),
                target: String::new(),
            }
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
    dot.push_str(&format!("digraph DiffImpact {{\n  rankdir=BT;\n  node [shape=box style=rounded];\n\n"));
    dot.push_str(&format!("  label=\"{}\";\n  labelloc=t;\n  fontsize=14;\n\n", title));

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
            dot.push_str(&format!("  \"{}\" [label=\"{}\\n{}\" style=filled fillcolor={fill}];\n", key, key, kind_label));
        }
    }

    dot.push_str("}\n");
    dot
}
