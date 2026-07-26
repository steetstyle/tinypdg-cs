use std::collections::HashSet;

use crate::detect::types::{DetectionContext, PatternKind, PatternMatch};

pub fn detect_structural(ctx: &DetectionContext) -> Vec<PatternMatch> {
    let mut results = Vec::new();

    detect_composite(ctx, &mut results);
    detect_adapter_decorator_proxy(ctx, &mut results);
    detect_decorator(ctx, &mut results);
    detect_bridge(ctx, &mut results);
    detect_flyweight(ctx, &mut results);
    detect_facade(ctx, &mut results);

    results
}

fn detect_composite(ctx: &DetectionContext, results: &mut Vec<PatternMatch>) {
    for (iface_name, _iface) in &ctx.type_graph.interfaces {
        detect_composite_for_type(ctx, results, iface_name, true);
    }
    for (class_name, class) in &ctx.type_graph.classes {
        if !class.is_abstract { continue; }
        detect_composite_for_type(ctx, results, class_name, false);
    }
}

fn return_type_of(m: &crate::resolve::types::MethodDescriptor) -> &str {
    m.signature.split_whitespace().next().unwrap_or("void")
}

fn detect_composite_for_type(ctx: &DetectionContext, results: &mut Vec<PatternMatch>,
    type_name: &str, is_interface: bool)
{
    let methods = if is_interface {
        if let Some(iface) = ctx.type_graph.interfaces.get(type_name) {
            &iface.methods
        } else { return }
    } else {
        if let Some(class) = ctx.type_graph.classes.get(type_name) {
            &class.methods
        } else { return }
    };

    let non_accessor: Vec<_> = methods.iter()
        .filter(|m| !m.method.starts_with("get_") && !m.method.starts_with("set_"))
        .collect();

    let implementors = ctx.type_graph.concrete_subclasses(type_name);
    if implementors.len() < 2 { return; }

    // Exclude intermediate abstract classes (Decorator signal)
    if !is_interface {
        if let Some(class_info) = ctx.type_graph.classes.get(type_name) {
            // Skip if this abstract class has an abstract base class
            if class_info.base_class.as_ref()
                .and_then(|base| ctx.type_graph.classes.get(base))
                .map(|base| base.is_abstract)
                .unwrap_or(false)
            {
                return;
            }
        }
        // Skip if this class has an abstract subclass (Composite should have concrete subs only)
        let has_abstract_sub = ctx.type_graph.classes.values().any(|c| {
            c.is_abstract && c.base_class.as_deref() == Some(type_name)
        });
        if has_abstract_sub { return; }
    }

    if non_accessor.len() < 2 { return; }
    let returns_self = non_accessor.iter().any(|m| return_type_of(m) == type_name);

    let confidence = if returns_self {
        0.85
    } else {
        if non_accessor.len() >= 3 {
            let has_void = non_accessor.iter().any(|m| return_type_of(m) == "void");
            let has_non_void = non_accessor.iter().any(|m| return_type_of(m) != "void");
            if has_void && has_non_void { 0.65 } else { return; }
        } else {
            return;
        }
    };

    results.push(PatternMatch {
        pattern: PatternKind::Composite,
        class: type_name.to_string(),
        description: format!(
            "{} '{}' has mgmt+ops with {} implementations — Composite",
            if is_interface { "Interface" } else { "Abstract class" },
            type_name, implementors.len()
        ),
        confidence,
        participants: vec![type_name.to_string()],
        evidence: vec![
            format!("returns_self: {}", returns_self),
            format!("implementations: {}", implementors.len()),
        ],
    });
}

fn detect_decorator(ctx: &DetectionContext, results: &mut Vec<PatternMatch>) {
    // Decorator via abstract class hierarchy: base abstract class (Component)
    // has an intermediate abstract subclass (Decorator) that extends it,
    // which in turn has 2+ concrete subclasses (ConcreteDecorators).
    // The base class also has a direct concrete subclass (ConcreteComponent).
    for class in ctx.type_graph.classes.values() {
        if !class.is_abstract { continue; }

        // Check if this abstract class has a base that is also abstract
        let base_name = match &class.base_class {
            Some(b) => b.clone(),
            None => continue,
        };
        let base_class = match ctx.type_graph.classes.get(&base_name) {
            Some(c) if c.is_abstract => c,
            _ => continue,
        };

        // The intermediate class must have 1+ methods beyond the base
        if class.methods.len() <= base_class.methods.len() { continue; }

        // Must have 2+ concrete subclasses
        let concrete_subs: Vec<_> = ctx.type_graph.classes.values()
            .filter(|c| !c.is_abstract && c.base_class.as_deref() == Some(&class.name))
            .collect();
        if concrete_subs.len() < 2 { continue; }

        results.push(PatternMatch {
            pattern: PatternKind::Decorator,
            class: class.name.clone(),
            description: format!(
                "'{}' wraps '{}' with {} decorators — Decorator",
                class.name, base_name, concrete_subs.len()
            ),
            confidence: 0.7,
            participants: vec![class.name.clone(), base_name.clone()],
            evidence: concrete_subs.iter().map(|c| c.name.clone()).collect(),
        });
    }
}

fn detect_adapter_decorator_proxy(ctx: &DetectionContext, results: &mut Vec<PatternMatch>) {
    let interface_names: HashSet<&str> = ctx.type_graph.interfaces.keys()
        .map(|s| s.as_str()).collect();

    for class in ctx.type_graph.classes.values() {
        if class.interfaces.is_empty() { continue; }

        for iface in &class.interfaces {
            if !interface_names.contains(iface.as_str()) { continue; }

            let target_iface = match ctx.type_graph.interfaces.get(iface.as_str()) {
                Some(i) => i,
                None => continue,
            };

            if target_iface.methods.is_empty() { continue; }

            let impl_methods: HashSet<&str> = class.methods.iter()
                .map(|m| m.method.as_str()).collect();

            let missing: Vec<&str> = target_iface.methods.iter()
                .filter(|im| !impl_methods.contains(im.method.as_str()))
                .map(|im| im.method.as_str())
                .collect();

            if !missing.is_empty() { continue; }

            // An adapter wraps a different type: check if any constructor
            // takes a parameter whose type is NOT the target interface
            // and is a known class
            let wraps_other_type = class.methods.iter().any(|m| {
                m.method == class.name // constructor
                    && {
                        let sig = &m.signature;
                        // Extract parameter types from signature like "void Foo(ParamType)"
                        if let Some(params_start) = sig.find('(') {
                            if let Some(params_end) = sig.find(')') {
                                let params = &sig[params_start+1..params_end];
                                params.split(',').any(|p| {
                                    let p = p.trim();
                                    !p.is_empty()
                                        && p != iface
                                        && ctx.type_graph.classes.contains_key(p)
                                })
                            } else { false }
                        } else { false }
                    }
            });

            // Also allow extra non-interface methods (wrapper behavior)
            let iface_method_set: HashSet<&str> = target_iface.methods.iter()
                .map(|m| m.method.as_str()).collect();
            let extra_methods: Vec<&str> = class.methods.iter()
                .filter(|m| !m.method.starts_with("get_") && !m.method.starts_with("set_"))
                .filter(|m| m.method != class.name)
                .map(|m| m.method.as_str())
                .filter(|m| !iface_method_set.contains(m))
                .collect();
            let has_extra = !extra_methods.is_empty();

            if !wraps_other_type && !has_extra { continue; }

            let returns_interface = target_iface.methods.iter().any(|m| {
                let rt = return_type_of(m);
                ctx.type_graph.interfaces.contains_key(rt) || ctx.type_graph.classes.get(rt).map(|c| c.is_abstract).unwrap_or(false)
            });

            if returns_interface {
                results.push(PatternMatch {
                    pattern: PatternKind::Decorator,
                    class: class.name.clone(),
                    description: format!("'{}' wraps '{}' — decorator pattern", class.name, iface),
                    confidence: 0.5,
                    participants: vec![class.name.clone(), iface.clone()],
                    evidence: target_iface.methods.iter().map(|m| m.method.clone()).collect(),
                });
            } else {
                results.push(PatternMatch {
                    pattern: PatternKind::Adapter,
                    class: class.name.clone(),
                    description: format!("'{}' implements '{}' — adapter", class.name, iface),
                    confidence: 0.5,
                    participants: vec![class.name.clone(), iface.clone()],
                    evidence: target_iface.methods.iter().map(|m| m.method.clone()).collect(),
                });
            }
        }
    }
}

fn detect_bridge(ctx: &DetectionContext, results: &mut Vec<PatternMatch>) {
    for class in ctx.type_graph.classes.values() {
        if !class.is_abstract || class.interfaces.is_empty() { continue; }

        let iface = &class.interfaces[0];
        if !ctx.type_graph.interfaces.contains_key(iface.as_str()) { continue; }

        let implementors = ctx.type_graph.concrete_subclasses(&class.name);
        if implementors.len() >= 2 {
            results.push(PatternMatch {
                pattern: PatternKind::Bridge,
                class: class.name.clone(),
                description: format!(
                    "Abstract '{}' implements '{}' with {} subclasses — Bridge",
                    class.name, iface, implementors.len()
                ),
                confidence: 0.6,
                participants: vec![class.name.clone(), iface.clone()],
                evidence: vec![format!("subclasses: {}", implementors.len())],
            });
        }
    }
}

fn detect_flyweight(ctx: &DetectionContext, results: &mut Vec<PatternMatch>) {
    for class in ctx.type_graph.classes.values() {
        // Flyweight: has a static cache field (Dictionary, ConcurrentDictionary, etc.)
        // and static factory methods that return the class itself
        let has_cache_field = class.fields.iter().any(|f| {
            f.is_static && (f.field_type.contains("Dictionary")
                || f.field_type.contains("ConcurrentDictionary")
                || f.field_type.contains("Cache")
                || f.field_type.contains("Pool"))
        });

        let has_static_factory = class.methods.iter().any(|m| {
            let rt = m.signature.split_whitespace().next().unwrap_or("");
            m.is_static && rt == class.name
        });

        let returns_self = class.methods.iter()
            .filter(|m| {
                let rt = m.signature.split_whitespace().next().unwrap_or("");
                rt == class.name && !m.is_static
            })
            .count();

        let confidence = if has_cache_field && has_static_factory {
            0.85
        } else if has_cache_field {
            0.6
        } else if returns_self >= 2 {
            0.4
        } else {
            continue;
        };

        let mut evidence = Vec::new();
        if has_cache_field {
            evidence.push("static cache field".into());
        }
        if has_static_factory {
            evidence.push("static factory".into());
        }
        evidence.extend(class.methods.iter()
            .filter(|m| {
                let rt = m.signature.split_whitespace().next().unwrap_or("");
                rt == class.name
            })
            .map(|m| format!("{} -> {}", m.method, class.name)));

        results.push(PatternMatch {
            pattern: PatternKind::Flyweight,
            class: class.name.clone(),
            description: format!(
                "'{}' — Flyweight (cache: {}, factory: {})",
                class.name, has_cache_field, has_static_factory
            ),
            confidence,
            participants: vec![class.name.clone()],
            evidence,
        });
    }
}

fn detect_facade(ctx: &DetectionContext, results: &mut Vec<PatternMatch>) {
    for class in ctx.type_graph.classes.values() {
        if class.is_abstract || class.is_static || !class.interfaces.is_empty() { continue; }

        if class.methods.len() >= 4 {
            results.push(PatternMatch {
                pattern: PatternKind::Facade,
                class: class.name.clone(),
                description: format!("'{}' has {} methods and no interfaces — Facade", class.name, class.methods.len()),
                confidence: 0.3,
                participants: vec![class.name.clone()],
                evidence: class.methods.iter().map(|m| m.method.clone()).collect(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::types::*;

    #[test]
    fn test_composite_with_add_and_implementors() {
        let methods = vec![
            MethodDescriptor { class: "IComponent".into(), method: "Add".into(),
                signature: "IComponent Add".into(), is_static: false,
                is_virtual: false, is_abstract: false },
            MethodDescriptor { class: "IComponent".into(), method: "Render".into(),
                signature: "void Render".into(), is_static: false,
                is_virtual: false, is_abstract: false },
        ];

        let mut tg = TypeGraph::new();
        tg.interfaces.insert("IComponent".into(), InterfaceInfo {
            name: "IComponent".into(),
            methods,
        });
        tg.classes.insert("Leaf".into(), ClassInfo {
            name: "Leaf".into(), base_class: None,
            interfaces: vec!["IComponent".into()],
            methods: vec![MethodDescriptor { class: "Leaf".into(), method: "Render".into(),
                signature: "void Render".into(), is_static: false,
                is_virtual: false, is_abstract: false }],
            fields: vec![],
            is_abstract: false, is_sealed: false, is_static: false,
        });
        tg.classes.insert("Composite".into(), ClassInfo {
            name: "Composite".into(), base_class: None,
            interfaces: vec!["IComponent".into()],
            methods: vec![
                MethodDescriptor { class: "Composite".into(), method: "Add".into(),
                    signature: "void Add".into(), is_static: false,
                    is_virtual: false, is_abstract: false },
                MethodDescriptor { class: "Composite".into(), method: "Render".into(),
                    signature: "void Render".into(), is_static: false,
                    is_virtual: false, is_abstract: false },
            ],
            fields: vec![],
            is_abstract: false, is_sealed: false, is_static: false,
        });
        let ctx = DetectionContext::new(&tg, "");
        let r = detect_structural(&ctx);
        assert!(r.iter().any(|m| m.pattern == PatternKind::Composite));
    }

    #[test]
    fn test_facade_many_methods_no_interfaces() {
        let mut tg = TypeGraph::new();
        tg.classes.insert("OrderService".into(), ClassInfo {
            name: "OrderService".into(), base_class: None, interfaces: vec![],
            methods: (0..5).map(|i| MethodDescriptor {
                class: "OrderService".into(),
                method: format!("Method{}", i),
                signature: format!("void Method{}", i),
                is_static: false, is_virtual: false, is_abstract: false,
            }).collect(),
            fields: vec![],
            is_abstract: false, is_sealed: false, is_static: false,
        });
        let ctx = DetectionContext::new(&tg, "");
        let r = detect_structural(&ctx);
        assert!(r.iter().any(|m| m.pattern == PatternKind::Facade));
    }

    #[test]
    fn test_bridge_abstract_with_subclasses() {
        let mut tg = TypeGraph::new();
        tg.interfaces.insert("IDraw".into(), InterfaceInfo {
            name: "IDraw".into(), methods: vec![],
        });
        tg.classes.insert("Shape".into(), ClassInfo {
            name: "Shape".into(), base_class: None,
            interfaces: vec!["IDraw".into()],
            methods: vec![MethodDescriptor { class: "Shape".into(), method: "Draw".into(),
                signature: "void Draw".into(), is_static: false,
                is_virtual: true, is_abstract: false }],
            fields: vec![],
            is_abstract: true, is_sealed: false, is_static: false,
        });
        tg.classes.insert("Circle".into(), ClassInfo {
            name: "Circle".into(), base_class: Some("Shape".into()),
            interfaces: vec![],
            methods: vec![MethodDescriptor { class: "Circle".into(), method: "Draw".into(),
                signature: "void Draw".into(), is_static: false,
                is_virtual: false, is_abstract: false }],
            fields: vec![],
            is_abstract: false, is_sealed: false, is_static: false,
        });
        tg.classes.insert("Square".into(), ClassInfo {
            name: "Square".into(), base_class: Some("Shape".into()),
            interfaces: vec![],
            methods: vec![MethodDescriptor { class: "Square".into(), method: "Draw".into(),
                signature: "void Draw".into(), is_static: false,
                is_virtual: false, is_abstract: false }],
            fields: vec![],
            is_abstract: false, is_sealed: false, is_static: false,
        });
        let ctx = DetectionContext::new(&tg, "");
        let r = detect_structural(&ctx);
        assert!(r.iter().any(|m| m.pattern == PatternKind::Bridge));
    }
}
