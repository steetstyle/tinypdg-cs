use std::collections::HashMap;
use std::sync::Arc;

use crate::resolve::types::{ClassInfo, TypeGraph};
use crate::analysis::callgraph::{CallGraph, CallSite};
use crate::analysis::pdg_context::PdgContext;

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
    Delegate { handlers: Vec<String> },
}

#[derive(Debug, Clone)]
pub struct NavEntry {
    pub idx: usize,
    pub callee: String,
    pub via: String,
    pub target: NodeRef,
    pub kind: EdgeKind,
    pub line: Option<usize>,
    pub context: Option<String>,
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

#[derive(Debug)]
pub struct TraversalState {
    pub current: NodeRef,
    pub queue: Vec<NodeRef>,
    pub history: Vec<HistoryEntry>,
    pub judgments: HashMap<String, Judgment>,
    pub evidence: HashMap<String, String>,
    pub incident_context: Option<String>,
    pub down: Vec<NavEntry>,
    pub up: Vec<NavEntry>,
    pub up_dispatch: Vec<NavEntry>,
    pub pdg: Arc<PdgContext>,
}

impl TraversalState {
    pub fn init(tg: &TypeGraph, cg: &CallGraph, class: &str, context: Option<String>, project_dir: Option<&str>) -> Result<Self, String> {
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
                    let mut names: Vec<&str> = tg.classes.keys().map(|k| k.as_str()).collect();
                    names.sort();
                    names.truncate(30);
                    format!("Class '{}' not found. Available classes (first 30):\n  {}\nUse --class <NAME> to specify.", class, names.join("\n  "))
                } else {
                    let names: Vec<_> = matches.iter().map(|k| k.as_str()).collect();
                    format!("'{}' matches multiple classes:\n  {}\nUse an exact class name.", class, names.join("\n  "))
                }
            })?;
        let class_info = tg.classes.get(class_name)
            .ok_or_else(|| format!("Class '{}' not found", class_name))?;

        let all_methods: Vec<String> = class_info.methods.iter().map(|m| m.method.clone()).collect();
        let (filtered, kept): (Vec<_>, Vec<_>) = class_info.methods.iter()
            .partition(|m| {
                let n = m.method.as_str();
                n == ".ctor" || n == class_name.as_str()
                    || n.starts_with("get_") || n.starts_with("set_")
                    || n.starts_with("add_") || n.starts_with("remove_")
            });
        let methods: Vec<String> = kept.iter().map(|m| m.method.clone()).collect();

        if methods.is_empty() {
            let total = all_methods.len();
            let filtered_names: Vec<&str> = filtered.iter().map(|m| m.method.as_str()).collect();
            if total == 0 {
                return Err(format!("Class '{}' has no methods at all (empty or contains only fields/properties)", class_name));
            }
            return Err(format!(
                "Class '{}' has no traversable methods ({} total, {} filtered: [{}])",
                class_name, total, filtered.len(), filtered_names.join(", ")
            ));
        }

        let first = &methods[0];
        let remaining: Vec<NodeRef> = methods[1..].iter().map(|m| mk_node(tg, class_name, m)).collect();

        let pdg = match project_dir {
            Some(dir) => Arc::new(PdgContext::build(std::path::Path::new(dir))),
            None => Arc::new(PdgContext::empty()),
        };

        let current = mk_node(tg, class_name, first);
        let down = resolve_down(cg, tg, &pdg, &current);
        let (up, up_dispatch) = resolve_up(cg, tg, &current);

        Ok(TraversalState {
            current,
            queue: remaining,
            history: Vec::new(),
            judgments: HashMap::new(),
            evidence: HashMap::new(),
            incident_context: context,
            down,
            up,
            up_dispatch,
            pdg,
        })
    }

    pub fn navigate_down(&mut self, idx: usize, tg: &TypeGraph, cg: &CallGraph) {
        let entry = match self.down.iter().find(|e| e.idx == idx) {
            Some(e) => e.clone(),
            None => return,
        };
        // External/lib calls are leaf nodes — skip navigation
        if matches!(entry.kind, EdgeKind::External) {
            println!("  ⚠ External call — cannot navigate further (leaf node)");
            return;
        }
        // Self-call or same target — skip to avoid infinite loop
        if entry.target.class == self.current.class && entry.target.method == self.current.method {
            println!("  ⚠ Self-call — already at {}.{}", self.current.class, self.current.method);
            return;
        }
        self.history.push(HistoryEntry {
            node: self.current.clone(),
            action: format!("↓{}", idx),
            insight: String::new(),
        });
        self.current = entry.target;
        self.down = resolve_down(cg, tg, &self.pdg, &self.current);
        let (up, up_dispatch) = resolve_up(cg, tg, &self.current);
        self.up = up;
        self.up_dispatch = up_dispatch;
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
        self.down = resolve_down(cg, tg, &self.pdg, &self.current);
        let (up, up_dispatch) = resolve_up(cg, tg, &self.current);
        self.up = up;
        self.up_dispatch = up_dispatch;
    }

    pub fn navigate_down_dispatch(&mut self, idx: usize, sub: usize, tg: &TypeGraph, cg: &CallGraph) {
        let entry = match self.down.iter().find(|e| e.idx == idx) {
            Some(e) => e.clone(),
            None => return,
        };
        let target = match &entry.kind {
            EdgeKind::Interface { implementations, .. } => {
                let impl_class = match implementations.get(sub) {
                    Some((name, _)) => name.clone(),
                    None => return,
                };
                NodeRef { class: impl_class, method: entry.callee.clone() }
            }
            EdgeKind::Delegate { handlers } => {
                let method = match handlers.get(sub) {
                    Some(m) => m.clone(),
                    None => return,
                };
                NodeRef { class: self.current.class.clone(), method }
            }
            _ => return,
        };
        self.history.push(HistoryEntry {
            node: self.current.clone(),
            action: format!("↓{}{}", idx, (b'a' + sub as u8) as char),
            insight: String::new(),
        });
        self.current = target;
        self.down = resolve_down(cg, tg, &self.pdg, &self.current);
        let (up, up_dispatch) = resolve_up(cg, tg, &self.current);
        self.up = up;
        self.up_dispatch = up_dispatch;
    }

    pub fn navigate_up_dispatch(&mut self, idx: usize, sub: usize, tg: &TypeGraph, cg: &CallGraph) {
        let entry = match self.up.iter().find(|e| e.idx == idx) {
            Some(e) => e.clone(),
            None => return,
        };
        let target = match &entry.kind {
            EdgeKind::Interface { implementations, .. } => {
                let impl_class = match implementations.get(sub) {
                    Some((name, _)) => name.clone(),
                    None => return,
                };
                NodeRef { class: impl_class, method: entry.callee.clone() }
            }
            EdgeKind::Delegate { handlers: _ } => {
                // Up delegates navigate to the caller's class where the delegation happened
                NodeRef { class: entry.target.class.clone(), method: entry.target.method.clone() }
            }
            _ => return,
        };
        self.history.push(HistoryEntry {
            node: self.current.clone(),
            action: format!("↑{}{}", idx, (b'a' + sub as u8) as char),
            insight: String::new(),
        });
        self.current = target;
        self.down = resolve_down(cg, tg, &self.pdg, &self.current);
        let (up, up_dispatch) = resolve_up(cg, tg, &self.current);
        self.up = up;
        self.up_dispatch = up_dispatch;
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
        self.down = resolve_down(cg, tg, &self.pdg, &self.current);
        let (up, up_dispatch) = resolve_up(cg, tg, &self.current);
        self.up = up;
        self.up_dispatch = up_dispatch;
        true
    }
}

fn mk_node(_tg: &TypeGraph, class: &str, method: &str) -> NodeRef {
    NodeRef {
        class: class.to_string(),
        method: method.to_string(),
    }
}

pub fn resolve_down(cg: &CallGraph, tg: &TypeGraph, pdg: &PdgContext, node: &NodeRef) -> Vec<NavEntry> {
    // Interface node — show implementor methods
    if is_interface(tg, &node.class) {
        let mut result = Vec::new();
        let mut idx = 0;
        let implementors = find_implementors(tg, &node.class);
        for impl_info in implementors {
            if impl_info.methods.iter().any(|m| m.method == node.method) {
                idx += 1;
                result.push(NavEntry {
                    idx,
                    callee: node.method.clone(),
                    via: impl_info.name.clone(),
                    target: NodeRef { class: impl_info.name.clone(), method: node.method.clone() },
                    kind: EdgeKind::Direct,
                    line: None,
                    context: None,
                });
            }
        }
        return result;
    }

    let calls: Vec<&CallSite> = cg.calls.iter()
        .filter(|c| c.caller_class == node.class && c.caller_method == node.method)
        .collect();

    let mut seen: HashMap<(String, String), Vec<&CallSite>> = HashMap::new();
    for c in calls {
        seen.entry((c.callee.clone(), c.callee_class.clone())).or_default().push(c);
    }

    let mut result = Vec::new();
    let mut idx = 0;
    let mut sorted: Vec<_> = seen.into_iter().collect();
    sorted.sort_by(|a, b| (a.0).0.cmp(&(b.0).0));

    for ((callee, callee_class), sites) in sorted {
        idx += 1;
        let first = sites[0];
        let via = if first.target_expr.is_empty() { String::new() } else { first.target_expr.clone() };
        let (target, kind) = if callee_class.is_empty() {
            if !first.target_expr.is_empty() && !tg.classes.contains_key(&first.target_expr) {
                // target_expr is a field/property/chain/variable expression (not a known class)
                // — first check if it's an interface method (dispatch on interface type)
                let edge_kind = classify_edge(tg, &callee, &first.target_expr);
                match &edge_kind {
                    EdgeKind::Interface { interface, .. } => {
                        (mk_node(tg, interface, &callee), edge_kind)
                    }
                    _ => {
                        // Not interface — treat as external/delegate
                        let delegate_handlers: Vec<String> = sites.iter()
                            .flat_map(|s| s.delegates.iter().cloned())
                            .collect();
                        let kind = if delegate_handlers.is_empty() {
                            EdgeKind::External
                        } else {
                            EdgeKind::Delegate { handlers: delegate_handlers }
                        };
                        (NodeRef { class: first.target_expr.clone(), method: callee.clone() }, kind)
                    }
                }
            } else {
                // target_expr is empty (bare call) or a known class name
                let found = find_callee_node(tg, node.class.clone(), &callee, &first.target_expr);
                let method_exists = tg.classes.get(&found.class)
                    .map(|c| c.methods.iter().any(|m| m.method == found.method))
                    .unwrap_or(false);
                if !method_exists {
                    let delegate_handlers: Vec<String> = sites.iter()
                        .flat_map(|s| s.delegates.iter().cloned())
                        .collect();
                    let kind = if delegate_handlers.is_empty() {
                        EdgeKind::External
                    } else {
                        EdgeKind::Delegate { handlers: delegate_handlers }
                    };
                    (NodeRef { class: first.target_expr.clone(), method: callee.clone() }, kind)
                } else {
                    (found, classify_edge(tg, &callee, &first.target_expr))
                }
            }
        } else {
            let kind = classify_edge(tg, &callee, &first.target_expr);
            (mk_node(tg, &callee_class, &callee), kind)
        };
        let line = first.line;
        let context = get_context(tg, pdg, &first.target_expr, line, &callee, &target, &sites);
        result.push(NavEntry { idx, callee, via, target, kind, line: Some(line), context });
    }

    result
}

fn get_context(tg: &TypeGraph, pdg: &PdgContext, _target_expr: &str, line: usize, _callee: &str, target: &NodeRef, _sites: &[&CallSite]) -> Option<String> {
    let file = PdgContext::get_method_file(&target.class, &target.method, tg)?;
    let ctrl = pdg.get_control_context(&file, line);
    let data = pdg.get_data_context(&file, line);
    let mut parts = Vec::new();
    if let Some(ref c) = ctrl {
        parts.push(format!("ctrl: [{}]", c.join(", ")));
    }
    if let Some(ref d) = data {
        parts.push(format!("data: [{}]", d.join(", ")));
    }
    if parts.is_empty() { None } else { Some(parts.join("; ")) }
}

pub fn resolve_up(cg: &CallGraph, tg: &TypeGraph, node: &NodeRef) -> (Vec<NavEntry>, Vec<NavEntry>) {
    // Always compute dispatch sources (type-flow based dispatch chain)
    let dispatch = find_dispatch_sources(tg, cg, node);

    // Interface node — show callers that call the interface method
    if is_interface(tg, &node.class) {
        let calls: Vec<&CallSite> = cg.calls.iter()
            .filter(|c| {
                c.callee == node.method
                    && (c.callee_class == node.class
                        || (c.callee_class.is_empty() && !c.is_self_call
                            && c.caller_class != node.class
                            && is_method_unique(tg, &node.method)))
            })
            .collect();
        if !calls.is_empty() {
            return (group_up_calls(calls), dispatch);
        }
        return (Vec::new(), dispatch);
    }

    // Exact match: callee_class resolved to our class
    // Also include calls with empty callee_class through variables,
    // but only if the method name is unique across all classes
    // (avoids false positives from method name collisions)
    let calls: Vec<&CallSite> = cg.calls.iter()
        .filter(|c| {
            if c.callee != node.method { return false; }
            if c.callee_class == node.class { return true; }
            c.callee_class.is_empty()
                && !c.is_self_call
                && !tg.classes.contains_key(&c.target_expr)
                && is_method_unique(tg, &node.method)
        })
        .collect();

    if !calls.is_empty() {
        return (group_up_calls(calls), dispatch);
    }

    // Check for delegate callers: our method passed as argument to another method
    let delegate_calls: Vec<&CallSite> = cg.calls.iter()
        .filter(|c| c.delegates.iter().any(|d| d == &node.method) && c.caller_method != node.method)
        .collect();
    let delegate_up = if delegate_calls.is_empty() {
        Vec::new()
    } else {
        group_up_calls(delegate_calls)
    };

    // No direct callers — check if method implements an interface method
    let matching_ifaces = find_matching_interfaces(tg, node);

    if matching_ifaces.is_empty() {
        return (delegate_up, dispatch);
    }

    // Search for callers through the interface (callee_class = interface name)
    // Also include calls with empty callee_class — these are often calls through
    // interface-typed variables (e.g., `IInterface x = ...; x.Method()`)
    let iface_calls: Vec<&CallSite> = cg.calls.iter()
        .filter(|c| {
            c.callee == node.method
                && (matching_ifaces.contains(&&c.callee_class)
                    || (c.callee_class.is_empty() && c.caller_class != node.class))
        })
        .collect();

    if !iface_calls.is_empty() {
        let mut up = Vec::new();
        let mut idx = 0;
        let mut seen: HashMap<String, Vec<&CallSite>> = HashMap::new();
        for c in iface_calls {
            seen.entry(c.caller_class.clone()).or_default().push(c);
        }
        let mut sorted: Vec<_> = seen.into_iter().collect();
        sorted.sort_by(|a, b| a.0.cmp(&b.0));
        let iface_name = matching_ifaces[0].clone();
        let impls: Vec<(String, f64)> = find_implementors(tg, &iface_name)
            .iter().filter(|c| c.name != node.class)
            .map(|c| (c.name.clone(), 0.95)).collect();
        for (caller_class, sites) in sorted {
            idx += 1;
            let first = sites[0];
            let target = NodeRef { class: caller_class.clone(), method: first.caller_method.clone() };
            up.push(NavEntry {
                idx,
                callee: first.callee.clone(),
                via: first.caller_method.clone(),
                target,
                kind: EdgeKind::Interface { interface: iface_name.clone(), implementations: impls.clone() },
                line: None,
                context: None,
            });
        }
        let base = up.len();
        for (i, mut entry) in delegate_up.into_iter().enumerate() {
            entry.idx = base + i + 1;
            up.push(entry);
        }
        return (up, dispatch);
    }

    // No callers through interface — create a synthetic entry pointing to the interface
    let iface_name = matching_ifaces[0].clone();
    let impls: Vec<(String, f64)> = find_implementors(tg, &iface_name)
        .iter().filter(|c| c.name != node.class)
        .map(|c| (c.name.clone(), 0.95)).collect();
    let up = vec![NavEntry {
        idx: 1,
        callee: node.method.clone(),
        via: iface_name.clone(),
        target: NodeRef { class: iface_name.clone(), method: node.method.clone() },
        kind: EdgeKind::Interface { interface: iface_name, implementations: impls },
        line: None,
        context: None,
    }];
    let mut result = up;
    let base = result.len();
    for (i, mut entry) in delegate_up.into_iter().enumerate() {
        entry.idx = base + i + 1;
        result.push(entry);
    }

    (result, dispatch)
}

fn find_matching_interfaces<'a>(tg: &'a TypeGraph, node: &NodeRef) -> Vec<&'a String> {
    let class_info = match tg.classes.get(&node.class) {
        Some(c) => c,
        None => return Vec::new(),
    };
    class_info.interfaces.iter()
        .filter(|_iface_name| {
            // The method is defined in this class and the class declares this interface
            // → assume the method implements the interface method
            class_info.methods.iter().any(|m| m.method == node.method)
        })
        .collect()
}

/// Returns true if only ONE class in the type graph has a method with this name.
/// Used to disambiguate calls with empty callee_class.
fn is_method_unique(tg: &TypeGraph, method: &str) -> bool {
    let count = tg.classes.values()
        .filter(|ci| ci.methods.iter().any(|m| m.method == method))
        .count();
    count == 1
}

fn group_up_calls(calls: Vec<&CallSite>) -> Vec<NavEntry> {
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
        result.push(NavEntry { idx, callee: via, via: String::new(), target, kind: EdgeKind::Direct, line: None, context: None });
    }

    result
}

fn find_callee_node(tg: &TypeGraph, caller_class: String, callee: &str, target_expr: &str) -> NodeRef {
    if !target_expr.is_empty() && tg.classes.contains_key(target_expr) {
        return mk_node(tg, target_expr, callee);
    }
    for (class_name, class_info) in &tg.classes {
        // If target_expr is a field (not empty, not a known class), skip the caller class
        if !target_expr.is_empty() && *class_name == caller_class {
            continue;
        }
        if class_info.methods.iter().any(|m| m.method == callee) {
            return mk_node(tg, class_name, callee);
        }
    }
    NodeRef { class: caller_class, method: callee.to_string() }
}

fn is_interface(tg: &TypeGraph, name: &str) -> bool {
    tg.interfaces.contains_key(name)
        || tg.classes.values().any(|c| c.interfaces.iter().any(|i| i == name))
}

fn find_implementors<'a>(tg: &'a TypeGraph, iface_name: &str) -> Vec<&'a ClassInfo> {
    tg.classes.values()
        .filter(|c| !c.is_abstract && !c.is_static && c.interfaces.iter().any(|i| i == iface_name))
        .collect()
}

fn parse_sig_param_types(sig: &str) -> Vec<String> {
    let paren_start = match sig.find('(') {
        Some(p) => p,
        None => return Vec::new(),
    };
    let paren_end = match sig.find(')') {
        Some(p) => p,
        None => return Vec::new(),
    };
    if paren_end <= paren_start + 1 {
        return Vec::new();
    }
    sig[paren_start + 1..paren_end]
        .split(',')
        .map(|s| s.trim().to_string())
        .collect()
}

fn is_subtype(tg: &TypeGraph, derived: &str, base: &str) -> bool {
    if derived == base { return true; }
    if let Some(ci) = tg.classes.get(derived) {
        if let Some(parent) = &ci.base_class {
            return is_subtype(tg, parent, base);
        }
    }
    false
}

fn params_type_match(tg: &TypeGraph, a: &[String], b: &[String]) -> bool {
    for ap in a {
        for bp in b {
            if ap == bp || is_subtype(tg, ap, bp) || is_subtype(tg, bp, ap) {
                return true;
            }
        }
    }
    false
}

fn extract_type_name(param: &str) -> &str {
    let type_part = param.rsplit_once(' ').map_or(param, |(t, _)| t.trim());
    // Strip common modifiers
    type_part.trim_start_matches("ref ").trim_start_matches("out ").trim_start_matches("in ")
        .trim_start_matches("params ")
}

fn is_user_type(tg: &TypeGraph, param: &str) -> bool {
    let type_name = extract_type_name(param);
    tg.classes.contains_key(type_name) || tg.interfaces.contains_key(type_name)
}

fn find_dispatch_sources(tg: &TypeGraph, cg: &CallGraph, node: &NodeRef) -> Vec<NavEntry> {
    let all_params = match tg.classes.get(&node.class)
        .and_then(|c| c.methods.iter().find(|m| m.method == node.method))
    {
        Some(m) => parse_sig_param_types(&m.signature),
        None => return Vec::new(),
    };
    // Only match on user-defined types (ignore string, int, etc.)
    let cur_params: Vec<String> = all_params.into_iter()
        .filter(|p| is_user_type(tg, p))
        .collect();
    if cur_params.is_empty() {
        return Vec::new();
    }

    let mut seen = std::collections::HashSet::new();
    let mut result = Vec::new();
    let mut idx = 0;

    for (class_name, class_info) in &tg.classes {
        if *class_name == node.class { continue; }
        for md in &class_info.methods {
            let md_params: Vec<String> = parse_sig_param_types(&md.signature)
                .into_iter().filter(|p| is_user_type(tg, p)).collect();
            if !params_type_match(tg, &cur_params, &md_params) { continue; }

            // Check if this method calls an unresolvable (external) method
            let has_external = cg.calls.iter().any(|c| {
                c.caller_class == *class_name
                    && c.caller_method == md.method
                    && c.callee_class.is_empty()
                    && !tg.classes.values().any(|ci| ci.methods.iter().any(|m| m.method == c.callee))
            });
            if !has_external { continue; }

            let key = (class_name.clone(), md.method.clone());
            if !seen.contains(&key) {
                seen.insert(key);
                idx += 1;
                result.push(NavEntry {
                    idx,
                    callee: md.method.clone(),
                    via: String::new(),
                    target: NodeRef { class: class_name.clone(), method: md.method.clone() },
                    kind: EdgeKind::Direct,
                    line: None,
                    context: None,
                });
            }
        }
    }

    result
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parser::parse_source;
    use crate::resolve::symbols::SymbolTable;
    use crate::analysis::callgraph::CallGraphBuilder;

    // ─── helpers ────────────────────────────────────────────────────

    fn build_tg_and_cg(src: &str) -> (TypeGraph, CallGraph) {
        let tree = parse_source(src).unwrap();
        let st = SymbolTable::from_ast(tree.root_node(), src).unwrap();
        let cg = CallGraphBuilder::build(tree.root_node(), src, &st.type_graph);
        (st.type_graph, cg)
    }

    // ─── parse_sig_param_types ──────────────────────────────────────

    #[test]
    fn test_parse_sig_param_types_empty() {
        assert!(parse_sig_param_types("void M()").is_empty());
    }

    #[test]
    fn test_parse_sig_param_types_one() {
        let p = parse_sig_param_types("void M(int x)");
        assert_eq!(p, vec!["int x"]);
    }

    #[test]
    fn test_parse_sig_param_types_multi() {
        let p = parse_sig_param_types("Task Handle(OrderStatusChangedToPaidIntegrationEvent evt, string s)");
        assert_eq!(p, vec!["OrderStatusChangedToPaidIntegrationEvent evt", "string s"]);
    }

    #[test]
    fn test_parse_sig_param_types_no_parens() {
        assert!(parse_sig_param_types("void M").is_empty());
    }

    // ─── is_subtype ─────────────────────────────────────────────────

    #[test]
    fn test_is_subtype_same() {
        let (tg, _) = build_tg_and_cg("class A {}");
        assert!(is_subtype(&tg, "A", "A"));
    }

    #[test]
    fn test_is_subtype_direct() {
        let (tg, _) = build_tg_and_cg("class A {} class B : A {}");
        assert!(is_subtype(&tg, "B", "A"));
        assert!(!is_subtype(&tg, "A", "B"));
    }

    #[test]
    fn test_is_subtype_chain() {
        let (tg, _) = build_tg_and_cg("class A {} class B : A {} class C : B {}");
        assert!(is_subtype(&tg, "C", "A"));
        assert!(is_subtype(&tg, "C", "B"));
        assert!(!is_subtype(&tg, "A", "C"));
    }

    #[test]
    fn test_is_subtype_unrelated() {
        let (tg, _) = build_tg_and_cg("class A {} class B {}");
        assert!(!is_subtype(&tg, "A", "B"));
    }

    #[test]
    fn test_is_subtype_unknown() {
        let (tg, _) = build_tg_and_cg("class A {}");
        assert!(!is_subtype(&tg, "A", "NonExistent"));
        assert!(!is_subtype(&tg, "NonExistent", "A"));
    }

    // ─── params_type_match ──────────────────────────────────────────

    #[test]
    fn test_params_type_match_exact() {
        assert!(params_type_match(&TypeGraph::default(), &["A".into()], &["A".into()]));
    }

    #[test]
    fn test_params_type_match_subtype() {
        let (tg, _) = build_tg_and_cg("class EventBase {} class OrderEvent : EventBase {}");
        assert!(params_type_match(&tg, &["OrderEvent".into()], &["EventBase".into()]));
    }

    #[test]
    fn test_params_type_match_subtype_rev() {
        let (tg, _) = build_tg_and_cg("class IntegrationEvent {} class OrderIntegrationEvent : IntegrationEvent {}");
        assert!(params_type_match(&tg, &["IntegrationEvent".into()], &["OrderIntegrationEvent".into()]));
    }

    #[test]
    fn test_params_type_match_no_match() {
        let (tg, _) = build_tg_and_cg("class A {} class B {}");
        assert!(!params_type_match(&tg, &["A".into()], &["B".into()]));
    }

    // ─── find_matching_interfaces ──────────────────────────────────

    #[test]
    fn test_find_matching_interfaces_none() {
        let (tg, _) = build_tg_and_cg("class C { void M() {} }");
        let node = NodeRef { class: "C".into(), method: "M".into() };
        let matches = find_matching_interfaces(&tg, &node);
        assert!(matches.is_empty());
    }

    #[test]
    fn test_find_matching_interfaces_found() {
        let src = "interface I { void M(); } class C : I { public void M() {} }";
        let (tg, _) = build_tg_and_cg(src);
        let node = NodeRef { class: "C".into(), method: "M".into() };
        let matches = find_matching_interfaces(&tg, &node);
        assert_eq!(matches.len(), 1);
        assert_eq!(*matches[0], "I");
    }

    #[test]
    fn test_find_matching_interfaces_multi() {
        let src = "interface I1 { void M(); } interface I2 { void M(); } class C : I1, I2 { public void M() {} }";
        let (tg, _) = build_tg_and_cg(src);
        let node = NodeRef { class: "C".into(), method: "M".into() };
        let matches = find_matching_interfaces(&tg, &node);
        assert_eq!(matches.len(), 2);
        assert!(matches.iter().any(|m| **m == "I1"));
        assert!(matches.iter().any(|m| **m == "I2"));
    }

    // ─── find_implementors ──────────────────────────────────────────

    #[test]
    fn test_find_implementors_none() {
        let (tg, _) = build_tg_and_cg("interface I { void M(); }");
        let impls = find_implementors(&tg, "I");
        assert!(impls.is_empty());
    }

    #[test]
    fn test_find_implementors_one() {
        let src = "interface I { void M(); } class C : I { public void M() {} }";
        let (tg, _) = build_tg_and_cg(src);
        let impls = find_implementors(&tg, "I");
        assert_eq!(impls.len(), 1);
        assert_eq!(impls[0].name, "C");
    }

    #[test]
    fn test_find_implementors_skips_abstract() {
        // C directly implements I to avoid inheritance chain issues
        let src = "interface I { void M(); } abstract class A : I { public abstract void M(); } class C : I { public void M() {} }";
        let (tg, _) = build_tg_and_cg(src);
        let impls = find_implementors(&tg, "I");
        assert_eq!(impls.len(), 1, "expected 1 implementor, got {:?}", impls.iter().map(|c| &c.name).collect::<Vec<_>>());
        assert_eq!(impls[0].name, "C");
    }

    #[test]
    fn test_find_implementors_multiple() {
        let src = "interface I { void M(); } class C1 : I { public void M() {} } class C2 : I { public void M() {} } class C3 : I { public void M() {} }";
        let (tg, _) = build_tg_and_cg(src);
        let impls = find_implementors(&tg, "I");
        assert_eq!(impls.len(), 3);
    }

    // ─── is_interface ───────────────────────────────────────────────

    #[test]
    fn test_is_interface_true() {
        let (tg, _) = build_tg_and_cg("interface I { void M(); } class C : I { public void M() {} }");
        assert!(is_interface(&tg, "I"));
    }

    #[test]
    fn test_is_interface_not() {
        let (tg, _) = build_tg_and_cg("class C { void M() {} }");
        assert!(!is_interface(&tg, "C"));
    }

    #[test]
    fn test_is_interface_unregistered_interface() {
        // External interface: not in tg.interfaces but referenced in ClassInfo.interfaces
        let (tg, _) = build_tg_and_cg("class C : IExternal { public void M() {} }");
        assert!(is_interface(&tg, "IExternal"));
    }

    // ─── classify_edge ──────────────────────────────────────────────

    #[test]
    fn test_classify_edge_direct() {
        let (tg, _) = build_tg_and_cg("class C { void M() {} }");
        let kind = classify_edge(&tg, "M", "");
        assert!(matches!(kind, EdgeKind::Direct));
    }

    #[test]
    fn test_classify_edge_interface() {
        let src = "interface I { void M(); } class C : I { public void M() {} }";
        let (tg, _) = build_tg_and_cg(src);
        let kind = classify_edge(&tg, "M", "");
        assert!(matches!(kind, EdgeKind::Interface { .. }));
        if let EdgeKind::Interface { interface, implementations } = &kind {
            assert_eq!(interface, "I");
            assert_eq!(implementations.len(), 1);
            assert_eq!(implementations[0].0, "C");
        }
    }

    // ─── resolve_down ───────────────────────────────────────────────

    #[test]
    fn test_resolve_down_interface_node() {
        let src = "interface I { void M(); } class C : I { public void M() {} void Caller() { M(); } }";
        let (tg, cg) = build_tg_and_cg(src);
        let pdg = PdgContext::empty();
        let node = NodeRef { class: "I".into(), method: "M".into() };
        let down = resolve_down(&cg, &tg, &pdg, &node);
        assert_eq!(down.len(), 1);
        assert_eq!(down[0].callee, "M");
        assert_eq!(down[0].target.class, "C");
    }

    #[test]
    fn test_resolve_down_interface_node_no_impl() {
        let (tg, cg) = build_tg_and_cg("interface I { void M(); }");
        let pdg = PdgContext::empty();
        let node = NodeRef { class: "I".into(), method: "M".into() };
        let down = resolve_down(&cg, &tg, &pdg, &node);
        assert!(down.is_empty());
    }

    #[test]
    fn test_resolve_down_direct_call() {
        let src = "class A { public void M() {} } class B { void Caller() { var a = new A(); a.M(); } }";
        let (tg, cg) = build_tg_and_cg(src);
        let pdg = PdgContext::empty();
        let node = NodeRef { class: "B".into(), method: "Caller".into() };
        let down = resolve_down(&cg, &tg, &pdg, &node);
        // a.M() has target_expr="a" (not a class) → external (no type info to resolve to A)
        assert!(down.iter().any(|e| matches!(e.kind, EdgeKind::External) && e.callee == "M"));
    }

    #[test]
    fn test_resolve_down_external_call() {
        // A method that calls something unresolved (no class has that method)
        let src = "class B { void Caller() { someObj.PublishAsync(); } }";
        let (tg, cg) = build_tg_and_cg(src);
        let pdg = PdgContext::empty();
        let node = NodeRef { class: "B".into(), method: "Caller".into() };
        let down = resolve_down(&cg, &tg, &pdg, &node);
        assert!(down.iter().any(|e| matches!(e.kind, EdgeKind::External)));
    }

    #[test]
    fn test_resolve_down_delegate_call() {
        let src = "class Service { void Setup() { MapPost(\"/path\", CreateItem); } void CreateItem() {} }";
        let (tg, cg) = build_tg_and_cg(src);
        let pdg = PdgContext::empty();
        let node = NodeRef { class: "Service".into(), method: "Setup".into() };
        let down = resolve_down(&cg, &tg, &pdg, &node);
        let delegate_entry = down.iter().find(|e| matches!(e.kind, EdgeKind::Delegate { .. }));
        assert!(delegate_entry.is_some(), "expected a Delegate entry; down: {:?}", down);
        if let Some(entry) = delegate_entry {
            if let EdgeKind::Delegate { handlers } = &entry.kind {
                assert!(handlers.contains(&"CreateItem".to_string()));
            }
        }
    }

    #[test]
    fn test_resolve_down_delegate_no_handlers() {
        let src = "class Service { void Setup() { MapPost(\"/path\", UnknownThing); } void CreateItem() {} }";
        let (tg, cg) = build_tg_and_cg(src);
        let pdg = PdgContext::empty();
        let node = NodeRef { class: "Service".into(), method: "Setup".into() };
        let down = resolve_down(&cg, &tg, &pdg, &node);
        // UnknownThing is not a method of Service → external, not delegate
        let has_ext = down.iter().any(|e| matches!(e.kind, EdgeKind::External));
        let has_del = down.iter().any(|e| matches!(e.kind, EdgeKind::Delegate { .. }));
        assert!(has_ext, "unknown arg should fall to external");
        assert!(!has_del, "should not be delegate when arg is unknown method");
    }

    #[test]
    fn test_resolve_down_empty() {
        let src = "class A { void M() {} }";
        let (tg, cg) = build_tg_and_cg(src);
        let pdg = PdgContext::empty();
        let node = NodeRef { class: "A".into(), method: "M".into() };
        let down = resolve_down(&cg, &tg, &pdg, &node);
        assert!(down.is_empty());
    }

    // ─── resolve_up ─────────────────────────────────────────────────

    #[test]
    fn test_resolve_up_direct_caller() {
        let src = "class A { public void M() {} } class B { void Caller() { var a = new A(); a.M(); } }";
        let (tg, cg) = build_tg_and_cg(src);
        let node = NodeRef { class: "A".into(), method: "M".into() };
        let (up, _dispatch) = resolve_up(&cg, &tg, &node);
        assert_eq!(up.len(), 1);
        assert_eq!(up[0].target.class, "B");
        assert_eq!(up[0].target.method, "Caller");
    }

    #[test]
    fn test_resolve_up_interface_dispatch() {
        // Handler has no direct callers → synthetic interface entry created
        let src = "interface IEventHandler { void Handle(string e); }
class Handler : IEventHandler { public void Handle(string e) {} }
class Dispatcher { void Dispatch() { IEventHandler h = null; h.Handle(\"x\"); } }";
        let (tg, cg) = build_tg_and_cg(src);
        let node = NodeRef { class: "Handler".into(), method: "Handle".into() };
        let (up, _dispatch) = resolve_up(&cg, &tg, &node);
        // Handler.Handle IS called (via h.Handle), so direct caller is found
        // In that case resolve_up returns Direct, not Interface
        assert!(up.iter().any(|e| e.target.class == "Dispatcher"));
    }

    #[test]
    fn test_resolve_up_interface_synthetic_no_callers() {
        // Handler has NO callers → synthetic interface entry with implementations
        let src = "interface IEventHandler { void Handle(string e); }
class Handler1 : IEventHandler { public void Handle(string e) {} }
class Handler2 : IEventHandler { public void Handle(string e) {} }";
        let (tg, cg) = build_tg_and_cg(src);
        let node = NodeRef { class: "Handler1".into(), method: "Handle".into() };
        let (up, _dispatch) = resolve_up(&cg, &tg, &node);
        assert!(!up.is_empty(), "should have synthetic interface entry");
        let has_iface = up.iter().any(|e| matches!(e.kind, EdgeKind::Interface { .. }));
        assert!(has_iface);
        // Should list Handler2 as other implementor
        if let Some(entry) = up.iter().find(|e| matches!(e.kind, EdgeKind::Interface { .. })) {
            if let EdgeKind::Interface { implementations, .. } = &entry.kind {
                assert!(implementations.iter().any(|(name, _)| name == "Handler2"));
                assert!(implementations.iter().all(|(name, _)| name != "Handler1"), "should exclude current class");
            }
        }
    }

    #[test]
    fn test_resolve_up_interface_node() {
        let src = "interface IEventHandler { void Handle(string e); }
class Handler : IEventHandler { public void Handle(string e) {} }
class Dispatcher { void Dispatch() { IEventHandler h = null; h.Handle(\"x\"); } }";
        let (tg, cg) = build_tg_and_cg(src);
        let node = NodeRef { class: "IEventHandler".into(), method: "Handle".into() };
        let (up, _dispatch) = resolve_up(&cg, &tg, &node);
        assert!(up.iter().any(|e| e.target.class == "Dispatcher"));
    }

    #[test]
    fn test_resolve_up_dispatch_sources() {
        // Handler takes IntegrationEvent, service publishes it through external bus
        let src = "class OrderIntegrationEvent {}
class OrderHandler { public void Handle(OrderIntegrationEvent evt) {} }
class EventBusService { public void PublishThroughBus(OrderIntegrationEvent evt) { someBus.PublishAsync(evt); } }";
        let (tg, cg) = build_tg_and_cg(src);
        let node = NodeRef { class: "OrderHandler".into(), method: "Handle".into() };
        let (_up, dispatch) = resolve_up(&cg, &tg, &node);
        // Should include dispatch source (PublishThroughBus)
        assert!(dispatch.iter().any(|e| e.callee == "PublishThroughBus"),
            "dispatch source PublishThroughBus not found; dispatch: {:?}", dispatch);
    }

    #[test]
    fn test_resolve_up_no_callers() {
        let src = "class A { void M() {} }";
        let (tg, cg) = build_tg_and_cg(src);
        let node = NodeRef { class: "A".into(), method: "M".into() };
        let (up, dispatch) = resolve_up(&cg, &tg, &node);
        assert!(up.is_empty());
        assert!(dispatch.is_empty());
    }

    #[test]
    fn test_resolve_up_no_callers_but_interface() {
        let src = "interface I { void M(); } class C : I { public void M() {} }";
        let (tg, cg) = build_tg_and_cg(src);
        let node = NodeRef { class: "C".into(), method: "M".into() };
        let (up, _dispatch) = resolve_up(&cg, &tg, &node);
        // Should have a synthetic interface entry since no callers
        assert!(!up.is_empty());
        let has_interface = up.iter().any(|e| matches!(e.kind, EdgeKind::Interface { .. }));
        assert!(has_interface);
    }

    #[test]
    fn test_resolve_up_delegate_caller() {
        // CreateItem is passed as delegate argument to MapPost → resolve_up should find Setup as caller
        let src = "class Service { void Setup() { MapPost(\"/path\", CreateItem); } void CreateItem() {} }";
        let (tg, cg) = build_tg_and_cg(src);
        let node = NodeRef { class: "Service".into(), method: "CreateItem".into() };
        let (up, _dispatch) = resolve_up(&cg, &tg, &node);
        assert!(up.iter().any(|e| e.target.method == "Setup"),
            "expected Setup as delegate caller; up: {:?}", up);
    }

    #[test]
    fn test_resolve_up_delegate_caller_multiple_handlers() {
        let src = "class Service { void Setup() { MapPost(\"/a\", H1); MapPost(\"/b\", H2); } void H1() {} void H2() {} }";
        let (tg, cg) = build_tg_and_cg(src);
        let node = NodeRef { class: "Service".into(), method: "H1".into() };
        let (up, _dispatch) = resolve_up(&cg, &tg, &node);
        assert!(up.iter().any(|e| e.target.method == "Setup"),
            "H1 should find Setup as delegate caller; up: {:?}", up);
    }

    // ─── find_dispatch_sources ──────────────────────────────────────

    #[test]
    fn test_extract_type_name_simple() {
        assert_eq!(extract_type_name("int x"), "int");
        assert_eq!(extract_type_name("string key"), "string");
        assert_eq!(extract_type_name("CatalogServices services"), "CatalogServices");
    }

    #[test]
    fn test_extract_type_name_with_modifiers() {
        assert_eq!(extract_type_name("ref string value"), "string");
        assert_eq!(extract_type_name("out int result"), "int");
        assert_eq!(extract_type_name("params string[] args"), "string[]");
    }

    #[test]
    fn test_extract_type_name_generic() {
        assert_eq!(extract_type_name("List<CatalogItem> items"), "List<CatalogItem>");
        assert_eq!(extract_type_name("Task<Results<NoContent, NotFound>> id"), "Task<Results<NoContent, NotFound>>");
    }

    #[test]
    fn test_is_user_type_primitive() {
        let (tg, _) = build_tg_and_cg("class A {}");
        assert!(!is_user_type(&tg, "int x"));
        assert!(!is_user_type(&tg, "string key"));
        assert!(!is_user_type(&tg, "bool flag"));
    }

    #[test]
    fn test_is_user_type_class() {
        let (tg, _) = build_tg_and_cg("class CatalogServices {} class Handler { void M(CatalogServices s) {} }");
        assert!(is_user_type(&tg, "CatalogServices s"));
    }

    #[test]
    fn test_is_user_type_interface() {
        let (tg, _) = build_tg_and_cg("interface IEventHandler {} class Handler { void M(IEventHandler h) {} }");
        assert!(is_user_type(&tg, "IEventHandler h"));
    }

    #[test]
    fn test_find_dispatch_sources_none() {
        let src = "class Handler { public void Handle(int x) {} } class Service { public void DoWork(int x) {} }";
        let (tg, cg) = build_tg_and_cg(src);
        let node = NodeRef { class: "Handler".into(), method: "Handle".into() };
        let sources = find_dispatch_sources(&tg, &cg, &node);
        assert!(sources.is_empty());
    }

    #[test]
    fn test_find_dispatch_sources_with_external() {
        let src = "class IntegrationEvent {}
class Handler { public void Handle(IntegrationEvent evt) {} }
class BusService { public void Publish(IntegrationEvent evt) { bus.PublishAsync(evt); } }";
        let (_tg, cg) = build_tg_and_cg(src);
        // Build tg separately for dispatch source lookup
        let (tg2, _) = build_tg_and_cg(src);
        let node = NodeRef { class: "Handler".into(), method: "Handle".into() };
        let sources = find_dispatch_sources(&tg2, &cg, &node);
        assert!(!sources.is_empty());
        assert!(sources.iter().any(|s| s.callee == "Publish"));
    }

    #[test]
    fn test_find_dispatch_sources_exact_subtype() {
        let src = "class IntegrationEvent {} class OrderIntegrationEvent : IntegrationEvent {}
class Handler { public void Handle(OrderIntegrationEvent evt) {} }
class BusService { public void Publish(IntegrationEvent evt) { bus.PublishAsync(evt); } }";
        let (tg, cg) = build_tg_and_cg(src);
        let node = NodeRef { class: "Handler".into(), method: "Handle".into() };
        let sources = find_dispatch_sources(&tg, &cg, &node);
        assert!(!sources.is_empty());
        assert!(sources.iter().any(|s| s.callee == "Publish"));
    }

    #[test]
    fn test_find_dispatch_sources_same_class() {
        // Should NOT include methods from the handler's own class
        let src = "class IntegrationEvent {}
class Handler { public void Handle(IntegrationEvent evt) { anotherMethod(); } private void anotherMethod() {} }";
        let (tg, cg) = build_tg_and_cg(src);
        let node = NodeRef { class: "Handler".into(), method: "Handle".into() };
        let sources = find_dispatch_sources(&tg, &cg, &node);
        assert!(sources.is_empty());
    }

    #[test]
    fn test_find_dispatch_sources_no_params() {
        let src = "class Handler { public void Handle() {} } class BusService { public void Publish() { bus.PublishAsync(); } }";
        let (tg, cg) = build_tg_and_cg(src);
        let node = NodeRef { class: "Handler".into(), method: "Handle".into() };
        let sources = find_dispatch_sources(&tg, &cg, &node);
        assert!(sources.is_empty());
    }

    // ─── TraversalState ─────────────────────────────────────────────

    #[test]
    fn test_traversal_state_init() {
        let src = "class A { void M() {} }";
        let (tg, cg) = build_tg_and_cg(src);
        let state = TraversalState::init(&tg, &cg, "A", None, None).unwrap();
        assert_eq!(state.current.class, "A");
        assert_eq!(state.current.method, "M");
    }

    #[test]
    fn test_traversal_state_init_not_found() {
        let (tg, cg) = build_tg_and_cg("class A { void M() {} }");
        let state = TraversalState::init(&tg, &cg, "NonExistent", None, None);
        assert!(state.is_err());
    }

    #[test]
    fn test_traversal_state_navigate_down() {
        // Self-call: B.Caller calls M() (bare, no target_expr) → resolves to B.M
        let src = "class B { void Caller() { M(); } void M() {} }";
        let (tg, cg) = build_tg_and_cg(src);
        let mut state = TraversalState::init(&tg, &cg, "B", None, None).unwrap();
        let down_len = state.down.len();
        assert!(down_len > 0, "B.Caller should have down entries; down: {:?}", state.down);
        state.navigate_down(1, &tg, &cg);
        assert_eq!(state.current.class, "B");
        assert_eq!(state.current.method, "M");
    }

    #[test]
    fn test_traversal_state_navigate_up() {
        let src = "class A { public void M() {} } class B { void Caller() { var a = new A(); a.M(); } }";
        let (tg, cg) = build_tg_and_cg(src);
        let mut state = TraversalState::init(&tg, &cg, "A", None, None).unwrap();
        state.navigate_up(1, &tg, &cg);
        assert_eq!(state.current.class, "B");
        assert_eq!(state.current.method, "Caller");
    }

    #[test]
    fn test_traversal_state_navigate_up_dispatch() {
        // No callers exists → synthetic interface entry is created
        let src = "interface I { void M(); }
class C1 : I { public void M() {} }
class C2 : I { public void M() {} }";
        let (tg, cg) = build_tg_and_cg(src);
        let state = TraversalState::init(&tg, &cg, "C1", None, None).unwrap();
        // Up should include synthetic interface dispatch entry
        let has_iface = state.up.iter().any(|e| matches!(e.kind, EdgeKind::Interface { .. }));
        assert!(has_iface, "up should have interface entry: {:?}", state.up);
        // Find the interface entry and check sub-entries
        for e in &state.up {
            if let EdgeKind::Interface { implementations, .. } = &e.kind {
                if !implementations.is_empty() {
                    assert!(implementations.iter().any(|(name, _)| name == "C2"));
                }
            }
        }
    }

    #[test]
    fn test_traversal_state_navigate_down_dispatch() {
        // Build an explicit EdgeKind::Interface entry and verify it navigates
        let src = "interface I { void M(); }
class Impl1 : I { public void M() {} }
class Impl2 : I { public void M() {} }
class Caller { void call() { I x = null; x.M(); } }";
        let (tg, cg) = build_tg_and_cg(src);
        // Start from Caller — its call to x.M() generates a down entry with interface edge
        let mut state = TraversalState::init(&tg, &cg, "Caller", None, None).unwrap();
        assert!(!state.down.is_empty(), "Caller should have down entries");
        // Find the interface edge in down entries
        let iface_entry = state.down.iter()
            .find(|e| matches!(e.kind, EdgeKind::Interface { .. }))
            .expect("should have an interface down entry");
        let iface_idx = iface_entry.idx;
        if let EdgeKind::Interface { implementations, .. } = &iface_entry.kind {
            assert!(implementations.len() >= 2, "need at least 2 implementors");
            // Navigate to the last implementor (different from target's current class)
            state.navigate_down_dispatch(iface_idx, implementations.len() - 1, &tg, &cg);
            // History should show the dispatch action
            assert!(state.history.iter().any(|h| h.action.starts_with('↓')
                && h.action.len() > 2),
                "history should contain a down-dispatch action: {:?}", state.history);
        }
    }

    #[test]
    fn test_traversal_state_navigate_down_dispatch_sub() {
        // No callers → synthetic interface entry → can navigate to other implementor
        let src = "interface I { void M(); }
class C1 : I { public void M() {} }
class C2 : I { public void M() {} }";
        let (tg, cg) = build_tg_and_cg(src);
        let mut state = TraversalState::init(&tg, &cg, "C1", None, None).unwrap();
        // Find index of the interface entry
        let iface_idx = state.up.iter().find(|e| matches!(e.kind, EdgeKind::Interface { .. })).unwrap().idx;
        // Navigate to sub-entry 0 (first other implementor, C2)
        state.navigate_up_dispatch(iface_idx, 0, &tg, &cg);
        assert_eq!(state.current.class, "C2");
        assert_eq!(state.current.method, "M");
    }

    #[test]
    fn test_traversal_state_navigate_down_dispatch_delegate() {
        let src = "class Service { void Setup() { MapPost(\"/x\", CreateItem); } void CreateItem() {} void Other() {} }";
        let (tg, cg) = build_tg_and_cg(src);
        let mut state = TraversalState::init(&tg, &cg, "Service", None, None).unwrap();
        // Find delegate entry in down
        let del_entry = state.down.iter()
            .find(|e| matches!(e.kind, EdgeKind::Delegate { .. }))
            .expect("Setup should have a Delegate entry");
        let del_idx = del_entry.idx;
        // Navigate to the first delegate handler (CreateItem)
        state.navigate_down_dispatch(del_idx, 0, &tg, &cg);
        assert_eq!(state.current.class, "Service", "should stay in same class");
        assert_eq!(state.current.method, "CreateItem", "should navigate to handler method");
        // History should contain the dispatch action
        assert!(state.history.iter().any(|h| h.action.starts_with('↓') && h.action.len() > 2));
    }

    #[test]
    fn test_traversal_state_navigate_down_dispatch_delegate_multiple() {
        let src = "class Service {
            void Setup() {
                MapPost(\"/x\", H1);
                MapPost(\"/y\", H2);
            }
            void H1() {}
            void H2() {}
        }";
        let (tg, cg) = build_tg_and_cg(src);
        let mut state = TraversalState::init(&tg, &cg, "Service", None, None).unwrap();
        // Find delegate entries
        let del_entries: Vec<_> = state.down.iter()
            .filter(|e| matches!(e.kind, EdgeKind::Delegate { .. }))
            .collect();
        // There should be one delegate entry per external call with handlers
        // MapPost("/x", H1) and MapPost("/y", H2) each produce a Delegate entry
        // Both have H1/H2 as handlers respectively
        assert!(del_entries.len() >= 1, "should have at least one Delegate entry; down: {:?}", state.down);
        // Navigate into first delegate entry's first handler
        let first = del_entries[0];
        state.navigate_down_dispatch(first.idx, 0, &tg, &cg);
        assert_eq!(state.current.class, "Service");
        assert!(state.current.method == "H1" || state.current.method == "H2");
    }

    #[test]
    fn test_traversal_state_history() {
        // Use self-call so navigation works (a.M() would be external)
        let src = "class B { void Caller() { M(); } void M() {} }";
        let (tg, cg) = build_tg_and_cg(src);
        let mut state = TraversalState::init(&tg, &cg, "B", None, None).unwrap();
        assert_eq!(state.history.len(), 0);
        state.navigate_down(1, &tg, &cg);
        assert_eq!(state.history.len(), 1);
        assert_eq!(state.history[0].action, "↓1");
        state.navigate_up(1, &tg, &cg);
        assert_eq!(state.history.len(), 2);
        assert_eq!(state.history[1].action, "↑1");
    }

    #[test]
    fn test_traversal_state_complete() {
        let src = "class A { void M() {} }";
        let (tg, cg) = build_tg_and_cg(src);
        let mut state = TraversalState::init(&tg, &cg, "A", None, None).unwrap();
        state.complete(Judgment::Primary, "found the bug".into());
        assert_eq!(state.judgments.get("A"), Some(&Judgment::Primary));
        assert_eq!(state.evidence.get("A"), Some(&"found the bug".into()));
    }

    #[test]
    fn test_traversal_state_next_in_queue() {
        let src = "class A { void M1() {} void M2() {} }";
        let (tg, cg) = build_tg_and_cg(src);
        let mut state = TraversalState::init(&tg, &cg, "A", None, None).unwrap();
        assert_eq!(state.current.method, "M1");
        assert!(state.next_in_queue(&tg, &cg));
        assert_eq!(state.current.method, "M2");
        assert!(!state.next_in_queue(&tg, &cg));
    }

    #[test]
    fn test_traversal_state_discard() {
        let src = "class A { void M1() {} void M2() {} }";
        let (tg, cg) = build_tg_and_cg(src);
        let mut state = TraversalState::init(&tg, &cg, "A", None, None).unwrap();
        assert_eq!(state.history.len(), 0);
        state.discard();
        assert_eq!(state.history.len(), 1);
        assert_eq!(state.history[0].action, "d");
    }

    #[test]
    fn test_traversal_state_init_case_insensitive() {
        let src = "class MyClass { void M() {} }";
        let (tg, cg) = build_tg_and_cg(src);
        let state = TraversalState::init(&tg, &cg, "myclass", None, None).unwrap();
        assert_eq!(state.current.class, "MyClass");
    }

    #[test]
    fn test_traversal_state_init_substring_match() {
        let src = "class UniqueClassName { void M() {} } class NotThisOne { void N() {} }";
        let (tg, cg) = build_tg_and_cg(src);
        let state = TraversalState::init(&tg, &cg, "Unique", None, None).unwrap();
        assert_eq!(state.current.class, "UniqueClassName");
    }

    #[test]
    fn test_traversal_state_init_ambiguous() {
        let src = "class Foo1 { void M() {} } class Foo2 { void N() {} }";
        let (tg, cg) = build_tg_and_cg(src);
        let state = TraversalState::init(&tg, &cg, "Foo", None, None);
        assert!(state.is_err());
    }

    #[test]
    fn test_traversal_state_init_filters_get_set() {
        let src = "class A { int get_X() { return 0; } int set_X(int v) {} void M() {} }";
        let (tg, cg) = build_tg_and_cg(src);
        let state = TraversalState::init(&tg, &cg, "A", None, None).unwrap();
        assert_eq!(state.current.method, "M");
    }

    // ─── EdgeKind ───────────────────────────────────────────────────

    #[test]
    fn test_edge_kind_direct() {
        let kind = EdgeKind::Direct;
        assert!(matches!(kind, EdgeKind::Direct));
    }

    #[test]
    fn test_edge_kind_interface() {
        let kind = EdgeKind::Interface { interface: "I".into(), implementations: vec![("C".into(), 0.95)] };
        if let EdgeKind::Interface { interface, implementations } = &kind {
            assert_eq!(interface, "I");
            assert_eq!(implementations.len(), 1);
            assert_eq!(implementations[0].0, "C");
            assert!((implementations[0].1 - 0.95).abs() < 0.01);
        }
    }

    #[test]
    fn test_edge_kind_external() {
        let kind = EdgeKind::External;
        assert!(matches!(kind, EdgeKind::External));
    }

    #[test]
    fn test_edge_kind_delegate() {
        let kind = EdgeKind::Delegate { handlers: vec!["CreateItem".into(), "DeleteItem".into()] };
        if let EdgeKind::Delegate { handlers } = &kind {
            assert_eq!(handlers.len(), 2);
            assert_eq!(handlers[0], "CreateItem");
            assert_eq!(handlers[1], "DeleteItem");
        }
    }
}
