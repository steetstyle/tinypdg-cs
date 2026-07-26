//! DI container resolution.
//!
//! Kategori 5: Dependency Injection
//! - Static registration: `AddScoped<TIf, TImpl>()` [95%]
//! - Conditional/factory: lambda CFG branching [50%]
//! - Keyed/named: `GetKeyedService<IFoo>("key")` [80-90%]
//! - Assembly scanning: `RegisterAssemblyTypes()` [30-60%]

use std::collections::HashMap;

use crate::resolve::types::{
    CallSite, CallTarget, Confidence, DiRegistration, MethodDescriptor, TypeGraph,
};

/// Container of DI registrations, built from scanning startup code
#[derive(Debug, Clone)]
pub struct DiContainer {
    /// Interface → { implementation, lifetime, confidence }
    pub registrations: HashMap<String, Vec<DiRegistration>>,
}

impl DiContainer {
    pub fn new() -> Self {
        DiContainer {
            registrations: HashMap::new(),
        }
    }

    /// Register a DI mapping
    pub fn register(&mut self, reg: DiRegistration) {
        self.registrations
            .entry(reg.interface_type.clone())
            .or_default()
            .push(reg);
    }

    /// Find all implementations for an interface
    pub fn resolve(&self, interface: &str) -> Vec<&DiRegistration> {
        self.registrations.get(interface).map(|v| v.iter().collect()).unwrap_or_default()
    }
}

/// Build DI container from AST traversal
/// Detects patterns like:
/// - `services.AddScoped<IFoo, Foo>();`
/// - `services.AddSingleton<IFoo>(sp => new Foo());`
/// - `services.AddTransient<IFoo, Foo>();`
pub fn scan_di_registrations(
    _source: &str,
    _type_graph: &TypeGraph,
) -> DiContainer {
    // Stub: real implementation scans AST for AddScoped/AddSingleton calls
    DiContainer::new()
}

/// Resolve a call via DI container
pub fn resolve_di(
    target: &CallTarget,
    caller: &str,
    container: &DiContainer,
    _type_graph: &TypeGraph,
) -> Vec<CallSite> {
    let interface = match target {
        CallTarget::DiResolved { interface, .. } => interface,
        CallTarget::Abstract { interface, .. } => {
            // Check if this interface has DI registrations
            if container.registrations.contains_key(interface) {
                interface
            } else {
                return Vec::new();
            }
        }
        _ => return Vec::new(),
    };

    let registrations = container.resolve(interface);
    if registrations.is_empty() {
        return Vec::new();
    }

    let resolved: Vec<MethodDescriptor> = registrations
        .iter()
        .filter_map(|reg| {
            _type_graph.classes.get(&reg.implementation_type).map(|cls| {
                // Return a method descriptor for the implementation class
                // (without specific method - caller must resolve further)
                MethodDescriptor {
                    class: cls.name.clone(),
                    method: String::new(),
                    signature: String::new(),
                    is_static: false,
                    is_virtual: false,
                    is_abstract: false,
                }
            })
        })
        .collect();

    if resolved.is_empty() {
        return Vec::new();
    }

    // Pick the best confidence from all registrations
    let confidence = registrations
        .iter()
        .map(|r| r.confidence.clone())
        .max_by_key(|c| c.score())
        .unwrap_or(Confidence::DiRegistration);

    vec![CallSite {
        caller: caller.to_string(),
        target: target.clone(),
        confidence,
        resolved,
    }]
}

/// Classify an invocation as a DI container call
pub fn classify_di_call(node: tree_sitter::Node, source: &str) -> Option<CallTarget> {
    if node.kind() != "invocation_expression" {
        return None;
    }
    let func = node.child_by_field_name("function")?;
    let method_name = match func.kind() {
        "member_access_expression" => {
            func.child_by_field_name("name")
                .and_then(|n| n.utf8_text(source.as_bytes()).ok())
                .map(|s| s.to_string())
        }
        _ => None,
    }?;

    // Detect DI registration calls
    match method_name.as_str() {
        "AddScoped" | "AddSingleton" | "AddTransient" => {
            // Extract type argument if available
            let type_args = node.child_by_field_name("type_arguments")
                .and_then(|n| n.utf8_text(source.as_bytes()).ok());
            if let Some(args) = type_args {
                // args looks like "<IFoo, Foo>" or "<IFoo>"
                let clean = args.trim_start_matches('<').trim_end_matches('>').to_string();
                let parts: Vec<&str> = clean.split(',').map(|s| s.trim()).collect();
                if let Some(iface) = parts.first() {
                    return Some(CallTarget::Static {
                        class: iface.to_string(),
                        method: method_name.clone(),
                    });
                }
            }
            None
        }
        "GetService" | "GetRequiredService" | "GetKeyedService" => {
            let type_args = node.child_by_field_name("type_arguments")
                .and_then(|n| n.utf8_text(source.as_bytes()).ok());
            if let Some(args) = type_args {
                let iface = args.trim_start_matches('<').trim_end_matches('>').trim();
                Some(CallTarget::DiResolved {
                    interface: iface.to_string(),
                    method: method_name.clone(),
                })
            } else {
                None
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::types::DiLifetime;

    #[test]
    fn test_di_container_empty() {
        let container = DiContainer::new();
        assert!(container.resolve("IFoo").is_empty());
    }

    #[test]
    fn test_di_register_and_resolve() {
        let mut container = DiContainer::new();
        container.register(DiRegistration {
            interface_type: "IFoo".into(),
            implementation_type: "Foo".into(),
            lifetime: DiLifetime::Scoped,
            confidence: Confidence::DiRegistration,
        });
        let regs = container.resolve("IFoo");
        assert_eq!(regs.len(), 1);
        assert_eq!(regs[0].implementation_type, "Foo");
    }

    #[test]
    fn test_di_resolve_call() {
        let mut container = DiContainer::new();
        container.register(DiRegistration {
            interface_type: "IFoo".into(),
            implementation_type: "Foo".into(),
            lifetime: DiLifetime::Scoped,
            confidence: Confidence::DiRegistration,
        });
        let mut tg = TypeGraph::new();
        tg.classes.insert("Foo".into(), crate::resolve::types::ClassInfo {
            name: "Foo".into(),
            base_class: None,
            interfaces: vec!["IFoo".into()],
            methods: vec![],
            fields: vec![],
            is_abstract: false,
            is_sealed: false,
            is_static: false,
        });
        let target = CallTarget::DiResolved {
            interface: "IFoo".into(),
            method: "GetService".into(),
        };
        let sites = resolve_di(&target, "Test", &container, &tg);
        assert_eq!(sites.len(), 1);
    }
}
