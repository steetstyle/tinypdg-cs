//! Factory method resolution.
//!
//! - Factory delegates: `Func<T>` → conditional branching [50%]
//! - Factory method patterns: `Create()`, `Build()` → concrete type [80%]
//! - Conditional factory: lambda with if/switch branching

use crate::resolve::types::{
    CallSite, CallTarget, Confidence, FactoryDescriptor, TypeGraph,
};

/// Detect factory patterns in the AST:
/// - Methods named `Create`, `Build`, `Make`, `Resolve`
/// - Methods returning an interface/abstract type
/// - Lambda expressions that construct objects
pub fn detect_factories(
    _type_graph: &TypeGraph,
) -> Vec<FactoryDescriptor> {
    // Stub: real implementation scans methods for factory patterns
    Vec::new()
}

/// Resolve a factory-based call
pub fn resolve_factory(
    target: &CallTarget,
    caller: &str,
    _type_graph: &TypeGraph,
    _factories: &[FactoryDescriptor],
) -> Vec<CallSite> {
    let method = match target {
        CallTarget::Static { method, .. } => method.as_str(),
        CallTarget::Instance { method } => method.as_str(),
        _ => return Vec::new(),
    };

    // Heuristic: methods named Create/Build/Resolve are factory methods
    let factory_names = ["Create", "Build", "Make", "Resolve", "GetInstance"];
    if !factory_names.contains(&method) {
        return Vec::new();
    }

    // Return a low-confidence factory resolution
    vec![CallSite {
        caller: caller.to_string(),
        target: target.clone(),
        confidence: Confidence::RTA,
        resolved: vec![],
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_factory_detect_empty() {
        let tg = TypeGraph::new();
        let factories = detect_factories(&tg);
        assert!(factories.is_empty());
    }

    #[test]
    fn test_factory_resolve_known_names() {
        let tg = TypeGraph::new();
        let factories = vec![];
        let target = CallTarget::Static {
            class: "Factory".into(),
            method: "Create".into(),
        };
        let sites = resolve_factory(&target, "Test", &tg, &factories);
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].confidence, Confidence::RTA);
    }

    #[test]
    fn test_factory_resolve_ignores_non_factory() {
        let tg = TypeGraph::new();
        let factories = vec![];
        let target = CallTarget::Instance {
            method: "ToString".into(),
        };
        let sites = resolve_factory(&target, "Test", &tg, &factories);
        assert!(sites.is_empty());
    }
}
