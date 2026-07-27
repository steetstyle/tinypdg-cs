use std::collections::HashMap;

use crate::resolve::types::TypeGraph;
use crate::analysis::callgraph::{CallGraph, CallSite};

#[derive(Debug, Clone)]
pub struct NodeRef {
    pub class: String,
    pub method: String,
}

#[derive(Debug, Clone)]
pub enum EdgeKind {
    Direct,
    Interface { interface: String, implementations: Vec<(String, f64)> },
    Virtual { base_class: String, overrides: Vec<(String, f64)> },
    External,
}

#[derive(Debug, Clone)]
pub struct NavEntry {
    pub idx: usize,
    pub callee: String,
    pub via: String,
    pub target: NodeRef,
    pub kind: EdgeKind,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Judgment {
    Primary,
    Symptom,
    Unrelated,
}

#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub node: NodeRef,
    pub action: String,
    pub insight: String,
}

#[derive(Debug, Clone)]
pub struct TraversalState {
    pub current: NodeRef,
    pub queue: Vec<NodeRef>,
    pub history: Vec<HistoryEntry>,
    pub judgments: HashMap<String, Judgment>,
    pub evidence: HashMap<String, String>,
    pub incident_context: Option<String>,
    pub down: Vec<NavEntry>,
    pub up: Vec<NavEntry>,
}

impl TraversalState {
    pub fn init(tg: &TypeGraph, cg: &CallGraph, class: &str, context: Option<String>) -> Result<Self, String> {
        let lower = class.to_lowercase();
        let class_name = tg.classes.keys()
            .find(|k| k.eq_ignore_ascii_case(class))
            .or_else(|| {
                let matches: Vec<&String> = tg.classes.keys()
                    .filter(|k| k.to_lowercase().contains(&lower))
                    .collect();
                if matches.len() == 1 { Some(matches[0]) } else { None }
            })
            .ok_or_else(|| {
                let matches: Vec<&String> = tg.classes.keys()
                    .filter(|k| k.to_lowercase().contains(&lower))
                    .collect();
                if matches.is_empty() {
                    format!("Class '{}' not found", class)
                } else {
                    let names: Vec<_> = matches.iter().map(|k| k.as_str()).collect();
                    format!("'{}' matches multiple classes:\n  {}\nUse an exact class name.", class, names.join("\n  "))
                }
            })?;
        let class_info = tg.classes.get(class_name)
            .ok_or_else(|| format!("Class '{}' not found", class_name))?;

        let methods: Vec<String> = class_info.methods.iter()
            .filter(|m| {
                let n = m.method.as_str();
                n != ".ctor" && n != class_name.as_str()
                    && !n.starts_with("get_") && !n.starts_with("set_")
                    && !n.starts_with("add_") && !n.starts_with("remove_")
            })
            .map(|m| m.method.clone())
            .collect();

        if methods.is_empty() {
            return Err(format!("Class '{}' has no methods to traverse", class_name));
        }

        let first = &methods[0];
        let remaining: Vec<NodeRef> = methods[1..].iter().map(|m| mk_node(tg, class_name, m)).collect();

        let current = mk_node(tg, class_name, first);
        let down = resolve_down(cg, tg, &current);
        let up = resolve_up(cg, tg, &current);

        Ok(TraversalState {
            current,
            queue: remaining,
            history: Vec::new(),
            judgments: HashMap::new(),
            evidence: HashMap::new(),
            incident_context: context,
            down,
            up,
        })
    }

    pub fn navigate_down(&mut self, idx: usize, tg: &TypeGraph, cg: &CallGraph) {
        let entry = self.down.iter().find(|e| e.idx == idx);
        let target = match entry {
            Some(e) => e.target.clone(),
            None => return,
        };
        self.history.push(HistoryEntry {
            node: self.current.clone(),
            action: format!("↓{}", idx),
            insight: String::new(),
        });
        self.current = target;
        self.down = resolve_down(cg, tg, &self.current);
        self.up = resolve_up(cg, tg, &self.current);
    }

    pub fn navigate_up(&mut self, idx: usize, tg: &TypeGraph, cg: &CallGraph) {
        let entry = self.up.iter().find(|e| e.idx == idx);
        let target = match entry {
            Some(e) => e.target.clone(),
            None => return,
        };
        self.history.push(HistoryEntry {
            node: self.current.clone(),
            action: format!("↑{}", idx),
            insight: String::new(),
        });
        self.current = target;
        self.down = resolve_down(cg, tg, &self.current);
        self.up = resolve_up(cg, tg, &self.current);
    }

    pub fn complete(&mut self, judgment: Judgment, evidence: String) {
        self.judgments.insert(self.current.class.clone(), judgment);
        self.evidence.insert(self.current.class.clone(), evidence);
        self.history.push(HistoryEntry {
            node: self.current.clone(),
            action: format!("c {:?}", judgment),
            insight: String::new(),
        });
    }

    pub fn discard(&mut self) {
        self.history.push(HistoryEntry {
            node: self.current.clone(),
            action: "d".into(),
            insight: String::new(),
        });
    }

    pub fn next_in_queue(&mut self, tg: &TypeGraph, cg: &CallGraph) -> bool {
        if self.queue.is_empty() { return false; }
        let next = self.queue.remove(0);
        self.current = next;
        self.down = resolve_down(cg, tg, &self.current);
        self.up = resolve_up(cg, tg, &self.current);
        true
    }
}

fn mk_node(_tg: &TypeGraph, class: &str, method: &str) -> NodeRef {
    NodeRef {
        class: class.to_string(),
        method: method.to_string(),
    }
}

fn resolve_down(cg: &CallGraph, tg: &TypeGraph, node: &NodeRef) -> Vec<NavEntry> {
    let calls: Vec<&CallSite> = cg.calls.iter()
        .filter(|c| c.caller_class == node.class && c.caller_method == node.method)
        .collect();

    let mut seen: HashMap<String, Vec<&CallSite>> = HashMap::new();
    for c in calls {
        seen.entry(c.callee.clone()).or_default().push(c);
    }

    let mut result = Vec::new();
    let mut idx = 0;
    let mut sorted: Vec<_> = seen.into_iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));

    for (callee, sites) in sorted {
        idx += 1;
        let first = sites[0];
        let via = if first.target_expr.is_empty() { String::new() } else { first.target_expr.clone() };
        let target = find_callee_node(tg, node.class.clone(), &callee, &first.target_expr);
        let kind = classify_edge(tg, &callee, &first.target_expr);
        result.push(NavEntry { idx, callee, via, target, kind });
    }

    result
}

fn resolve_up(cg: &CallGraph, _tg: &TypeGraph, node: &NodeRef) -> Vec<NavEntry> {
    let calls: Vec<&CallSite> = cg.calls.iter()
        .filter(|c| c.callee == node.method)
        .collect();

    let mut seen: HashMap<String, Vec<&CallSite>> = HashMap::new();
    for c in calls {
        seen.entry(c.caller_class.clone()).or_default().push(c);
    }

    let mut result = Vec::new();
    let mut idx = 0;
    let mut sorted: Vec<_> = seen.into_iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));

    for (caller_class, sites) in sorted {
        idx += 1;
        let first = sites[0];
        let via = first.caller_method.clone();
        let target = NodeRef { class: caller_class.clone(), method: first.caller_method.clone() };
        let kind = EdgeKind::Direct;
        result.push(NavEntry { idx, callee: via, via: String::new(), target, kind });
    }

    result
}

fn find_callee_node(tg: &TypeGraph, caller_class: String, callee: &str, target_expr: &str) -> NodeRef {
    if !target_expr.is_empty() && tg.classes.contains_key(target_expr) {
        return mk_node(tg, target_expr, callee);
    }
    for (class_name, class_info) in &tg.classes {
        if class_info.methods.iter().any(|m| m.method == callee) {
            return mk_node(tg, class_name, callee);
        }
    }
    NodeRef { class: caller_class, method: callee.to_string() }
}

fn classify_edge(tg: &TypeGraph, callee: &str, _target_expr: &str) -> EdgeKind {
    let matching: Vec<&str> = tg.interfaces.iter()
        .filter(|(_, iface)| iface.methods.iter().any(|m| m.method == callee))
        .filter_map(|(name, _)| {
            let impls = tg.concrete_subclasses(name);
            if impls.is_empty() { None } else { Some(name.as_str()) }
        })
        .collect();

    if !matching.is_empty() {
        let iface_name = matching[0].to_string();
        let impls: Vec<(String, f64)> = tg.concrete_subclasses(&iface_name)
            .iter().map(|c| (c.name.clone(), 0.70)).collect();
        return EdgeKind::Interface { interface: iface_name, implementations: impls };
    }

    EdgeKind::Direct
}
