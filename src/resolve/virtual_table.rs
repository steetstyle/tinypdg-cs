//! Virtual dispatch resolution: CHA (Class Hierarchy Analysis) + RTA (Rapid Type Analysis)

use std::collections::HashSet;

use crate::resolve::types::{CallSite, CallTarget, Confidence, MethodDescriptor, TypeGraph};

/// CHA — walk the type hierarchy to find all possible override targets
pub fn resolve_cha(
    target: &CallTarget,
    caller: &str,
    type_graph: &TypeGraph,
) -> Vec<CallSite> {
    match target {
        CallTarget::Virtual { class, method } => {
            let class_name = class.as_deref().unwrap_or("");
            resolve_virtual_impl(target, caller, type_graph, class_name, method)
        }
        CallTarget::Abstract { interface, method } => {
            resolve_virtual_impl(target, caller, type_graph, interface, method)
        }
        _ => Vec::new(),
    }
}

fn resolve_virtual_impl(
    target: &CallTarget,
    caller: &str,
    type_graph: &TypeGraph,
    class_name: &str,
    method: &str,
) -> Vec<CallSite> {
    let concretes = type_graph.concrete_subclasses(class_name);
    let mut resolved = Vec::new();
    let mut seen = HashSet::new();

    for cls in concretes {
        for m in &cls.methods {
            if m.method == *method && !seen.contains(&(cls.name.clone(), m.method.clone())) {
                seen.insert((cls.name.clone(), m.method.clone()));
                resolved.push((*m).clone());
            }
        }
    }

    if resolved.is_empty() {
        return Vec::new();
    }

    vec![CallSite {
        caller: caller.to_string(),
        target: target.clone(),
        confidence: Confidence::CHA,
        resolved,
    }]
}

/// RTA — filter CHA results by types that are actually instantiated (`new` expressions)
pub fn resolve_rta(
    target: &CallTarget,
    caller: &str,
    type_graph: &TypeGraph,
    _instantiated_types: &HashSet<String>,
) -> Vec<CallSite> {
    let cha_sites = resolve_cha(target, caller, type_graph);
    if cha_sites.is_empty() {
        return cha_sites;
    }

    // RTA filtering: only keep methods from types that are instantiated
    let site = &cha_sites[0];
    let filtered: Vec<MethodDescriptor> = site
        .resolved
        .iter()
        .filter(|m| _instantiated_types.contains(&m.class))
        .cloned()
        .collect();

    if filtered.is_empty() {
        return Vec::new();
    }

    let confidence = if filtered.len() == 1 {
        Confidence::RTA
    } else {
        Confidence::RTA
    };

    vec![CallSite {
        caller: caller.to_string(),
        target: target.clone(),
        confidence,
        resolved: filtered,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::types::{ClassInfo, TypeGraph};

    fn make_type_graph() -> TypeGraph {
        let mut tg = TypeGraph::new();
        tg.classes.insert("Base".into(), ClassInfo {
            name: "Base".into(),
            base_class: None,
            interfaces: vec![],
            methods: vec![MethodDescriptor {
                class: "Base".into(),
                method: "Foo".into(),
                signature: "void Foo".into(),
                is_static: false,
                is_virtual: true,
                is_abstract: false,
file: String::new(),
line_start: 0,
line_end: 0
}],
            fields: vec![],
            is_abstract: false,
            is_sealed: false,
            is_static: false,
        });
        tg.classes.insert("Derived".into(), ClassInfo {
            name: "Derived".into(),
            base_class: Some("Base".into()),
            interfaces: vec![],
            methods: vec![MethodDescriptor {
                class: "Derived".into(),
                method: "Foo".into(),
                signature: "void Foo".into(),
                is_static: false,
                is_virtual: false,
                is_abstract: false,
file: String::new(),
line_start: 0,
line_end: 0
}],
            fields: vec![],
            is_abstract: false,
            is_sealed: false,
            is_static: false,
        });
        tg
    }

    #[test]
    fn test_cha_finds_overrides() {
        let tg = make_type_graph();
        let target = CallTarget::Virtual {
            class: Some("Base".into()),
            method: "Foo".into(),
        };
        let sites = resolve_cha(&target, "Test", &tg);
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].resolved.len(), 2); // Base + Derived
    }

    #[test]
    fn test_rta_filters_by_instantiation() {
        let tg = make_type_graph();
        let target = CallTarget::Virtual {
            class: Some("Base".into()),
            method: "Foo".into(),
        };
        let mut instantiated = HashSet::new();
        instantiated.insert("Derived".into());
        let sites = resolve_rta(&target, "Test", &tg, &instantiated);
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].resolved.len(), 1);
        assert_eq!(sites[0].resolved[0].class, "Derived");
    }
}
