use std::collections::HashMap;

use crate::resolve::types::{ClassInfo, TypeGraph};
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

    pub fn navigate_down_dispatch(&mut self, idx: usize, sub: usize, tg: &TypeGraph, cg: &CallGraph) {
        let entry = match self.down.iter().find(|e| e.idx == idx) {
            Some(e) => e.clone(),
            None => return,
        };
        let impls = match &entry.kind {
            EdgeKind::Interface { implementations, .. } => implementations,
            _ => return,
        };
        let impl_class = match impls.get(sub) {
            Some((name, _)) => name.clone(),
            None => return,
        };
        let target = NodeRef { class: impl_class, method: entry.callee.clone() };
        self.history.push(HistoryEntry {
            node: self.current.clone(),
            action: format!("↓{}{}", idx, (b'a' + sub as u8) as char),
            insight: String::new(),
        });
        self.current = target;
        self.down = resolve_down(cg, tg, &self.current);
        self.up = resolve_up(cg, tg, &self.current);
    }

    pub fn navigate_up_dispatch(&mut self, idx: usize, sub: usize, tg: &TypeGraph, cg: &CallGraph) {
        let entry = match self.up.iter().find(|e| e.idx == idx) {
            Some(e) => e.clone(),
            None => return,
        };
        let impls = match &entry.kind {
            EdgeKind::Interface { implementations, .. } => implementations,
            _ => return,
        };
        let impl_class = match impls.get(sub) {
            Some((name, _)) => name.clone(),
            None => return,
        };
        let target = NodeRef { class: impl_class, method: entry.callee.clone() };
        self.history.push(HistoryEntry {
            node: self.current.clone(),
            action: format!("↑{}{}", idx, (b'a' + sub as u8) as char),
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
            let found = find_callee_node(tg, node.class.clone(), &callee, &first.target_expr);
            let method_exists = tg.classes.get(&found.class)
                .map(|c| c.methods.iter().any(|m| m.method == found.method))
                .unwrap_or(false);
            if !method_exists {
                // Unresolvable external call
                (NodeRef { class: first.target_expr.clone(), method: callee.clone() }, EdgeKind::External)
            } else {
                (found, classify_edge(tg, &callee, &first.target_expr))
            }
        } else {
            let kind = classify_edge(tg, &callee, &first.target_expr);
            (mk_node(tg, &callee_class, &callee), kind)
        };
        result.push(NavEntry { idx, callee, via, target, kind });
    }

    result
}

fn resolve_up(cg: &CallGraph, tg: &TypeGraph, node: &NodeRef) -> Vec<NavEntry> {
    // Interface node — show callers that call the interface method
    if is_interface(tg, &node.class) {
        let calls: Vec<&CallSite> = cg.calls.iter()
            .filter(|c| {
                c.callee == node.method
                    && (c.callee_class == node.class || c.callee_class.is_empty())
            })
            .collect();
        if !calls.is_empty() {
            return group_up_calls(calls);
        }
        // If callers call the method through the interface name, show those too
        let iface_calls: Vec<&CallSite> = cg.calls.iter()
            .filter(|c| c.callee == node.method && c.callee_class == node.class)
            .collect();
        if !iface_calls.is_empty() {
            return group_up_calls(iface_calls);
        }
        return Vec::new();
    }

    let calls: Vec<&CallSite> = cg.calls.iter()
        .filter(|c| {
            c.callee == node.method
                && (c.callee_class == node.class || c.callee_class.is_empty())
        })
        .collect();

    if !calls.is_empty() {
        return group_up_calls(calls);
    }

    // No direct callers — check if method implements an interface method
    let matching_ifaces = find_matching_interfaces(tg, node);

    // Always check for dispatch sources (type-flow based dispatch chain)
    // even when no interface is involved
    let dispatch = find_dispatch_sources(tg, cg, node);

    if matching_ifaces.is_empty() {
        if dispatch.is_empty() {
            return Vec::new();
        }
        // Only dispatch sources found
        return dispatch;
    }

    // Search for callers through the interface (callee_class = interface name)
    let iface_calls: Vec<&CallSite> = cg.calls.iter()
        .filter(|c| {
            c.callee == node.method
                && matching_ifaces.contains(&&c.callee_class)
        })
        .collect();

    if !iface_calls.is_empty() {
        let mut result = Vec::new();
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
            result.push(NavEntry {
                idx,
                callee: first.callee.clone(),
                via: first.caller_method.clone(),
                target,
                kind: EdgeKind::Interface { interface: iface_name.clone(), implementations: impls.clone() },
            });
        }
        let base = result.len();
        for (i, mut entry) in dispatch.into_iter().enumerate() {
            entry.idx = base + i + 1;
            result.push(entry);
        }
        return result;
    }

    // No interface dispatch callers either — search for any call to this method
    // from outside the class (potential dispatch through interface-typed variables)
    let loose_calls: Vec<&CallSite> = cg.calls.iter()
        .filter(|c| c.callee == node.method && c.caller_class != node.class)
        .collect();

    if !loose_calls.is_empty() {
        let mut result = group_up_calls(loose_calls);
        let base = result.len();
        for (i, mut entry) in dispatch.into_iter().enumerate() {
            entry.idx = base + i + 1;
            result.push(entry);
        }
        return result;
    }

    // No callers anywhere — create a synthetic entry pointing to the interface
    let iface_name = matching_ifaces[0].clone();
    let impls: Vec<(String, f64)> = find_implementors(tg, &iface_name)
        .iter().filter(|c| c.name != node.class)
        .map(|c| (c.name.clone(), 0.95)).collect();
    let mut result = vec![NavEntry {
        idx: 1,
        callee: node.method.clone(),
        via: iface_name.clone(),
        target: NodeRef { class: iface_name.clone(), method: node.method.clone() },
        kind: EdgeKind::Interface { interface: iface_name, implementations: impls },
    }];

    // Add dispatch sources — methods with matching parameter types calling external
    let base = result.len();
    for (i, mut entry) in dispatch.into_iter().enumerate() {
        entry.idx = base + i + 1;
        result.push(entry);
    }

    result
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
        result.push(NavEntry { idx, callee: via, via: String::new(), target, kind: EdgeKind::Direct });
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
            if ap == bp
                || is_subtype(tg, ap, bp)
                || is_subtype(tg, bp, ap)
                || ap.contains(bp.as_str())
                || bp.contains(ap.as_str())
            {
                return true;
            }
        }
    }
    false
}

fn find_dispatch_sources(tg: &TypeGraph, cg: &CallGraph, node: &NodeRef) -> Vec<NavEntry> {
    let cur_params = match tg.classes.get(&node.class)
        .and_then(|c| c.methods.iter().find(|m| m.method == node.method))
    {
        Some(m) => parse_sig_param_types(&m.signature),
        None => return Vec::new(),
    };
    if cur_params.is_empty() {
        return Vec::new();
    }

    let mut seen = std::collections::HashSet::new();
    let mut result = Vec::new();
    let mut idx = 0;

    for (class_name, class_info) in &tg.classes {
        if *class_name == node.class { continue; }
        for md in &class_info.methods {
            let md_params = parse_sig_param_types(&md.signature);
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
    fn test_params_type_match_containment() {
        let (tg, _) = build_tg_and_cg("class A {}");
        // "IntegrationEvent" is contained in "OrderIntegrationEvent"
        assert!(params_type_match(&tg, &["OrderIntegrationEvent".into()], &["IntegrationEvent".into()]));
    }

    #[test]
    fn test_params_type_match_containment_rev() {
        let (tg, _) = build_tg_and_cg("class A {}");
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
        let node = NodeRef { class: "I".into(), method: "M".into() };
        let down = resolve_down(&cg, &tg, &node);
        assert_eq!(down.len(), 1);
        assert_eq!(down[0].callee, "M");
        assert_eq!(down[0].target.class, "C");
    }

    #[test]
    fn test_resolve_down_interface_node_no_impl() {
        let (tg, cg) = build_tg_and_cg("interface I { void M(); }");
        let node = NodeRef { class: "I".into(), method: "M".into() };
        let down = resolve_down(&cg, &tg, &node);
        assert!(down.is_empty());
    }

    #[test]
    fn test_resolve_down_direct_call() {
        let src = "class A { public void M() {} } class B { void Caller() { var a = new A(); a.M(); } }";
        let (tg, cg) = build_tg_and_cg(src);
        let node = NodeRef { class: "B".into(), method: "Caller".into() };
        let down = resolve_down(&cg, &tg, &node);
        assert!(down.iter().any(|e| e.callee == "M" && e.target.class == "A"));
    }

    #[test]
    fn test_resolve_down_external_call() {
        // A method that calls something unresolved (no class has that method)
        let src = "class B { void Caller() { someObj.PublishAsync(); } }";
        let (tg, cg) = build_tg_and_cg(src);
        let node = NodeRef { class: "B".into(), method: "Caller".into() };
        let down = resolve_down(&cg, &tg, &node);
        assert!(down.iter().any(|e| matches!(e.kind, EdgeKind::External)));
    }

    #[test]
    fn test_resolve_down_empty() {
        let src = "class A { void M() {} }";
        let (tg, cg) = build_tg_and_cg(src);
        let node = NodeRef { class: "A".into(), method: "M".into() };
        let down = resolve_down(&cg, &tg, &node);
        assert!(down.is_empty());
    }

    // ─── resolve_up ─────────────────────────────────────────────────

    #[test]
    fn test_resolve_up_direct_caller() {
        let src = "class A { public void M() {} } class B { void Caller() { var a = new A(); a.M(); } }";
        let (tg, cg) = build_tg_and_cg(src);
        let node = NodeRef { class: "A".into(), method: "M".into() };
        let up = resolve_up(&cg, &tg, &node);
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
        let up = resolve_up(&cg, &tg, &node);
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
        let up = resolve_up(&cg, &tg, &node);
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
        let up = resolve_up(&cg, &tg, &node);
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
        let up = resolve_up(&cg, &tg, &node);
        // Should include dispatch source (PublishThroughBus) since handler has no callers
        assert!(up.iter().any(|e| e.callee == "PublishThroughBus"),
            "dispatch source PublishThroughBus not found; up: {:?}", up);
    }

    #[test]
    fn test_resolve_up_no_callers() {
        let src = "class A { void M() {} }";
        let (tg, cg) = build_tg_and_cg(src);
        let node = NodeRef { class: "A".into(), method: "M".into() };
        let up = resolve_up(&cg, &tg, &node);
        assert!(up.is_empty());
    }

    #[test]
    fn test_resolve_up_no_callers_but_interface() {
        let src = "interface I { void M(); } class C : I { public void M() {} }";
        let (tg, cg) = build_tg_and_cg(src);
        let node = NodeRef { class: "C".into(), method: "M".into() };
        let up = resolve_up(&cg, &tg, &node);
        // Should have a synthetic interface entry since no callers
        assert!(!up.is_empty());
        let has_interface = up.iter().any(|e| matches!(e.kind, EdgeKind::Interface { .. }));
        assert!(has_interface);
    }

    // ─── find_dispatch_sources ──────────────────────────────────────

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
        let state = TraversalState::init(&tg, &cg, "A", None).unwrap();
        assert_eq!(state.current.class, "A");
        assert_eq!(state.current.method, "M");
    }

    #[test]
    fn test_traversal_state_init_not_found() {
        let (tg, cg) = build_tg_and_cg("class A { void M() {} }");
        let state = TraversalState::init(&tg, &cg, "NonExistent", None);
        assert!(state.is_err());
    }

    #[test]
    fn test_traversal_state_navigate_down() {
        let src = "class A { public void M() {} } class B { void Caller() { var a = new A(); a.M(); } }";
        let (tg, cg) = build_tg_and_cg(src);
        let mut state = TraversalState::init(&tg, &cg, "B", None).unwrap();
        let down_len = state.down.len();
        assert!(down_len > 0);
        state.navigate_down(1, &tg, &cg);
        assert_eq!(state.current.class, "A");
        assert_eq!(state.current.method, "M");
    }

    #[test]
    fn test_traversal_state_navigate_up() {
        let src = "class A { public void M() {} } class B { void Caller() { var a = new A(); a.M(); } }";
        let (tg, cg) = build_tg_and_cg(src);
        let mut state = TraversalState::init(&tg, &cg, "A", None).unwrap();
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
        let state = TraversalState::init(&tg, &cg, "C1", None).unwrap();
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
        let mut state = TraversalState::init(&tg, &cg, "Caller", None).unwrap();
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
        let mut state = TraversalState::init(&tg, &cg, "C1", None).unwrap();
        // Find index of the interface entry
        let iface_idx = state.up.iter().find(|e| matches!(e.kind, EdgeKind::Interface { .. })).unwrap().idx;
        // Navigate to sub-entry 0 (first other implementor, C2)
        state.navigate_up_dispatch(iface_idx, 0, &tg, &cg);
        assert_eq!(state.current.class, "C2");
        assert_eq!(state.current.method, "M");
    }

    #[test]
    fn test_traversal_state_history() {
        let src = "class A { public void M() {} } class B { void Caller() { var a = new A(); a.M(); } }";
        let (tg, cg) = build_tg_and_cg(src);
        let mut state = TraversalState::init(&tg, &cg, "B", None).unwrap();
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
        let mut state = TraversalState::init(&tg, &cg, "A", None).unwrap();
        state.complete(Judgment::Primary, "found the bug".into());
        assert_eq!(state.judgments.get("A"), Some(&Judgment::Primary));
        assert_eq!(state.evidence.get("A"), Some(&"found the bug".into()));
    }

    #[test]
    fn test_traversal_state_next_in_queue() {
        let src = "class A { void M1() {} void M2() {} }";
        let (tg, cg) = build_tg_and_cg(src);
        let mut state = TraversalState::init(&tg, &cg, "A", None).unwrap();
        assert_eq!(state.current.method, "M1");
        assert!(state.next_in_queue(&tg, &cg));
        assert_eq!(state.current.method, "M2");
        assert!(!state.next_in_queue(&tg, &cg));
    }

    #[test]
    fn test_traversal_state_discard() {
        let src = "class A { void M1() {} void M2() {} }";
        let (tg, cg) = build_tg_and_cg(src);
        let mut state = TraversalState::init(&tg, &cg, "A", None).unwrap();
        assert_eq!(state.history.len(), 0);
        state.discard();
        assert_eq!(state.history.len(), 1);
        assert_eq!(state.history[0].action, "d");
    }

    #[test]
    fn test_traversal_state_init_case_insensitive() {
        let src = "class MyClass { void M() {} }";
        let (tg, cg) = build_tg_and_cg(src);
        let state = TraversalState::init(&tg, &cg, "myclass", None).unwrap();
        assert_eq!(state.current.class, "MyClass");
    }

    #[test]
    fn test_traversal_state_init_substring_match() {
        let src = "class UniqueClassName { void M() {} } class NotThisOne { void N() {} }";
        let (tg, cg) = build_tg_and_cg(src);
        let state = TraversalState::init(&tg, &cg, "Unique", None).unwrap();
        assert_eq!(state.current.class, "UniqueClassName");
    }

    #[test]
    fn test_traversal_state_init_ambiguous() {
        let src = "class Foo1 { void M() {} } class Foo2 { void N() {} }";
        let (tg, cg) = build_tg_and_cg(src);
        let state = TraversalState::init(&tg, &cg, "Foo", None);
        assert!(state.is_err());
    }

    #[test]
    fn test_traversal_state_init_filters_get_set() {
        let src = "class A { int get_X() { return 0; } int set_X(int v) {} void M() {} }";
        let (tg, cg) = build_tg_and_cg(src);
        let state = TraversalState::init(&tg, &cg, "A", None).unwrap();
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
}
