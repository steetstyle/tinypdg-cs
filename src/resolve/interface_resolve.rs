//! Interface dispatch resolution.
//!
//! Kategori 4: Interface dispatch
//! - Single impl → devirtualization [95%]
//! - Multi impl → CHA set [60%]
//! - Explicit interface impl [90%]
//! - Default Interface Methods (DIM) [85%]

use crate::resolve::types::{
    CallSite, CallTarget, Confidence, TypeGraph,
};

/// Resolve an interface method call
pub fn resolve_interface(
    target: &CallTarget,
    caller: &str,
    type_graph: &TypeGraph,
) -> Vec<CallSite> {
    let (iface, method) = match target {
        CallTarget::Abstract { interface, method } => (interface, method),
        CallTarget::Virtual { class: Some(c), method } => {
            // If the receiver type is an interface (not a class), treat as abstract
            if type_graph.interfaces.contains_key(c) {
                (c, method)
            } else {
                return Vec::new();
            }
        }
        _ => return Vec::new(),
    };

    let implementors = type_graph.implementors_of(iface);
    if implementors.is_empty() {
        return Vec::new();
    }

    let mut resolved = Vec::new();
    for cls in &implementors {
        let method_text = method.as_str();
        if let Some(md) = cls.methods.iter().find(|m| m.method == method_text) {
            resolved.push(md.clone());
        }
    }

    if resolved.is_empty() {
        return Vec::new();
    }

    let confidence = if resolved.len() == 1 {
        Confidence::ExplicitImpl
    } else {
        Confidence::MultiImpl
    };

    vec![CallSite {
        caller: caller.to_string(),
        target: target.clone(),
        confidence,
        resolved,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::types::{ClassInfo, InterfaceInfo, MethodDescriptor, TypeGraph};

    fn make_interface_graph() -> TypeGraph {
        let mut tg = TypeGraph::new();
        tg.interfaces.insert("IFoo".into(), InterfaceInfo {
            name: "IFoo".into(),
            methods: vec![MethodDescriptor {
                class: "IFoo".into(),
                method: "Bar".into(),
                signature: "void Bar".into(),
                is_static: false,
                is_virtual: false,
                is_abstract: false,
            }],
        });
        tg.classes.insert("FooImpl".into(), ClassInfo {
            name: "FooImpl".into(),
            base_class: None,
            interfaces: vec!["IFoo".into()],
            methods: vec![MethodDescriptor {
                class: "FooImpl".into(),
                method: "Bar".into(),
                signature: "void Bar".into(),
                is_static: false,
                is_virtual: false,
                is_abstract: false,
            }],
            fields: vec![],
            is_abstract: false,
            is_sealed: false,
            is_static: false,
        });
        tg
    }

    #[test]
    fn test_single_impl_returns_explicit() {
        let tg = make_interface_graph();
        let target = CallTarget::Abstract {
            interface: "IFoo".into(),
            method: "Bar".into(),
        };
        let sites = resolve_interface(&target, "Test", &tg);
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].confidence, Confidence::ExplicitImpl);
        assert_eq!(sites[0].resolved.len(), 1);
        assert_eq!(sites[0].resolved[0].class, "FooImpl");
    }

    #[test]
    fn test_multi_impl_returns_multi_impl_confidence() {
        let mut tg = make_interface_graph();
        tg.classes.insert("AnotherImpl".into(), ClassInfo {
            name: "AnotherImpl".into(),
            base_class: None,
            interfaces: vec!["IFoo".into()],
            methods: vec![MethodDescriptor {
                class: "AnotherImpl".into(),
                method: "Bar".into(),
                signature: "void Bar".into(),
                is_static: false,
                is_virtual: false,
                is_abstract: false,
            }],
            fields: vec![],
            is_abstract: false,
            is_sealed: false,
            is_static: false,
        });
        let target = CallTarget::Abstract {
            interface: "IFoo".into(),
            method: "Bar".into(),
        };
        let sites = resolve_interface(&target, "Test", &tg);
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].confidence, Confidence::MultiImpl);
        assert_eq!(sites[0].resolved.len(), 2);
    }

    #[test]
    fn test_no_implementors_returns_empty() {
        let tg = TypeGraph::new();
        let target = CallTarget::Abstract {
            interface: "IMissing".into(),
            method: "Bar".into(),
        };
        let sites = resolve_interface(&target, "Test", &tg);
        assert!(sites.is_empty());
    }
}
