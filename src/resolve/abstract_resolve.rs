//! Abstract class resolution.
//!
//! Given an abstract class with abstract methods, find all
//! concrete implementations via CHA.

use crate::resolve::types::{
    CallSite, CallTarget, Confidence, MethodDescriptor, TypeGraph,
};

/// Resolve an abstract method call to concrete implementations
pub fn resolve_abstract(
    target: &CallTarget,
    caller: &str,
    type_graph: &TypeGraph,
) -> Vec<CallSite> {
    let (class_name, method) = match target {
        CallTarget::Abstract { interface, method } => (interface.as_str(), method.as_str()),
        CallTarget::Static { class, method } | CallTarget::Virtual { class: Some(class), method } => {
            // Also handle abstract base class calls
            match type_graph.classes.get(class) {
                Some(c) if c.is_abstract => (class.as_str(), method.as_str()),
                _ => return Vec::new(),
            }
        }
        _ => return Vec::new(),
    };

    let concretes = type_graph.concrete_subclasses(class_name);
    if concretes.is_empty() {
        return Vec::new();
    }

    let mut resolved = Vec::new();
    for cls in &concretes {
        let method_text = method;
        if let Some(md) = cls.methods.iter().find(|m| m.method == method_text) {
            resolved.push(md.clone());
        } else {
            // Inherited method: look up the hierarchy
            resolved.push(MethodDescriptor {
                class: cls.name.clone(),
                method: method_text.to_string(),
                signature: String::new(),
                is_static: false,
                is_virtual: false,
                is_abstract: false,
            });
        }
    }

    if resolved.is_empty() {
        return Vec::new();
    }

    let confidence = if resolved.len() == 1 {
        Confidence::ExplicitImpl
    } else {
        Confidence::CHA
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
    use crate::resolve::types::{ClassInfo, TypeGraph};

    fn make_abstract_graph() -> TypeGraph {
        let mut tg = TypeGraph::new();
        tg.classes.insert("AbstractBase".into(), ClassInfo {
            name: "AbstractBase".into(),
            base_class: None,
            interfaces: vec![],
            methods: vec![MethodDescriptor {
                class: "AbstractBase".into(),
                method: "DoWork".into(),
                signature: "void DoWork".into(),
                is_static: false,
                is_virtual: false,
                is_abstract: true,
            }],
            fields: vec![],
            is_abstract: true,
            is_sealed: false,
            is_static: false,
        });
        tg.classes.insert("Concrete".into(), ClassInfo {
            name: "Concrete".into(),
            base_class: Some("AbstractBase".into()),
            interfaces: vec![],
            methods: vec![MethodDescriptor {
                class: "Concrete".into(),
                method: "DoWork".into(),
                signature: "void DoWork".into(),
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
    fn test_abstract_resolve_finds_concrete() {
        let tg = make_abstract_graph();
        let target = CallTarget::Abstract {
            interface: "AbstractBase".into(),
            method: "DoWork".into(),
        };
        let sites = resolve_abstract(&target, "Test", &tg);
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].resolved.len(), 1);
        assert_eq!(sites[0].resolved[0].class, "Concrete");
    }

    #[test]
    fn test_abstract_with_no_impl_returns_empty() {
        let tg = TypeGraph::new();
        let target = CallTarget::Abstract {
            interface: "IMissing".into(),
            method: "Foo".into(),
        };
        let sites = resolve_abstract(&target, "Test", &tg);
        assert!(sites.is_empty());
    }
}
