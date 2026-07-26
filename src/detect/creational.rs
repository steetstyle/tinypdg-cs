use crate::detect::types::{DetectionContext, PatternKind, PatternMatch};

pub fn detect_creational(ctx: &DetectionContext) -> Vec<PatternMatch> {
    let mut results = Vec::new();
    detect_singleton(ctx, &mut results);
    detect_factory_methods(ctx, &mut results);
    detect_abstract_factory(ctx, &mut results);
    detect_builder(ctx, &mut results);
    detect_prototype(ctx, &mut results);
    results
}

fn return_type_of(m: &crate::resolve::types::MethodDescriptor) -> &str {
    m.signature.split_whitespace().next().unwrap_or("void")
}

fn detect_singleton(ctx: &DetectionContext, results: &mut Vec<PatternMatch>) {
    for class in ctx.type_graph.classes.values() {
        let self_returning_statics: Vec<_> = class.methods.iter()
            .filter(|m| m.is_static && return_type_of(m) == class.name)
            .collect();

        if !self_returning_statics.is_empty() {
            results.push(PatternMatch {
                pattern: PatternKind::Singleton,
                class: class.name.clone(),
                description: format!(
                    "'{}' has static method returning itself — single instance",
                    class.name
                ),
                confidence: 0.9,
                participants: vec![class.name.clone()],
                evidence: self_returning_statics.iter().map(|m| m.method.clone()).collect(),
            });
        }
    }
}

fn detect_factory_methods(ctx: &DetectionContext, results: &mut Vec<PatternMatch>) {
    for class in ctx.type_graph.classes.values() {
        for m in &class.methods {
            let rt = return_type_of(m);
            if rt == "void" || rt == class.name { continue; }

            let is_iface = ctx.type_graph.interfaces.contains_key(rt);
            let is_abstract = ctx.type_graph.classes.get(rt)
                .map(|c| c.is_abstract).unwrap_or(false);

            if is_iface || is_abstract {
                results.push(PatternMatch {
                    pattern: PatternKind::FactoryMethod,
                    class: class.name.clone(),
                    description: format!("'{}' returns '{}' (interface/abstract)", m.method, rt),
                    confidence: 0.85,
                    participants: vec![class.name.clone(), rt.to_string()],
                    evidence: vec![format!("{} → {}", m.method, rt)],
                });
            }
        }
    }
}

fn detect_abstract_factory(ctx: &DetectionContext, results: &mut Vec<PatternMatch>) {
    for (iface_name, iface) in &ctx.type_graph.interfaces {
        let returns_interfaces: Vec<_> = iface.methods.iter()
            .filter(|m| {
                let rt = return_type_of(m);
                rt != "void" && ctx.type_graph.interfaces.contains_key(rt)
            })
            .collect();

        if returns_interfaces.len() >= 2 {
            results.push(PatternMatch {
                pattern: PatternKind::AbstractFactory,
                class: iface_name.clone(),
                description: format!(
                    "Interface '{}' has {} methods returning interfaces",
                    iface_name, returns_interfaces.len()
                ),
                confidence: 0.9,
                participants: vec![iface_name.clone()],
                evidence: returns_interfaces.iter().map(|m| m.method.clone()).collect(),
            });
        }
    }
}

fn detect_builder(ctx: &DetectionContext, results: &mut Vec<PatternMatch>) {
    for class in ctx.type_graph.classes.values() {
        if class.is_abstract { continue; }

        let self_returning: Vec<_> = class.methods.iter()
            .filter(|m| !m.is_static && return_type_of(m) == class.name)
            .collect();

        let returns_other: Vec<_> = class.methods.iter()
            .filter(|m| {
                let rt = return_type_of(m);
                rt != "void" && rt != class.name
            })
            .collect();

        if self_returning.len() >= 2 && !returns_other.is_empty() {
            results.push(PatternMatch {
                pattern: PatternKind::Builder,
                class: class.name.clone(),
                description: format!(
                    "'{}' has {} fluent methods + {} build methods — builder pattern",
                    class.name, self_returning.len(), returns_other.len()
                ),
                confidence: 0.85,
                participants: vec![class.name.clone()],
                evidence: {
                    let mut e: Vec<_> = self_returning.iter().map(|m| m.method.clone()).collect();
                    e.extend(returns_other.iter().map(|m| m.method.clone()));
                    e
                },
            });
        }
    }
}

fn detect_prototype(ctx: &DetectionContext, results: &mut Vec<PatternMatch>) {
    for class in ctx.type_graph.classes.values() {
        let impl_icloneable = class.interfaces.iter().any(|i| i == "ICloneable");

        if impl_icloneable {
            results.push(PatternMatch {
                pattern: PatternKind::Prototype,
                class: class.name.clone(),
                description: format!("'{}' implements ICloneable — prototype", class.name),
                confidence: 0.95,
                participants: vec![class.name.clone()],
                evidence: vec!["ICloneable".into()],
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::types::*;

    #[test]
    fn test_singleton_instance_returns_self() {
        let mut tg = TypeGraph::new();
        tg.classes.insert("Config".into(), ClassInfo {
            name: "Config".into(),
            base_class: None, interfaces: vec![],
            methods: vec![MethodDescriptor {
                class: "Config".into(), method: "get_Instance".into(),
                signature: "Config get_Instance".into(),
                is_static: true, is_virtual: false, is_abstract: false,
            }],
            fields: vec![],
            is_abstract: false, is_sealed: false, is_static: false,
        });
        let ctx = DetectionContext::new(&tg, "");
        let r = detect_creational(&ctx);
        assert!(r.iter().any(|m| m.pattern == PatternKind::Singleton));
    }

    #[test]
    fn test_factory_method_returns_interface() {
        let mut tg = TypeGraph::new();
        tg.interfaces.insert("IService".into(), InterfaceInfo {
            name: "IService".into(), methods: vec![],
        });
        tg.classes.insert("Factory".into(), ClassInfo {
            name: "Factory".into(), base_class: None, interfaces: vec![],
            methods: vec![MethodDescriptor {
                class: "Factory".into(), method: "Make".into(),
                signature: "IService Make".into(),
                is_static: false, is_virtual: false, is_abstract: false,
            }],
            fields: vec![],
            is_abstract: false, is_sealed: false, is_static: false,
        });
        let ctx = DetectionContext::new(&tg, "");
        let r = detect_creational(&ctx);
        assert!(r.iter().any(|m| m.pattern == PatternKind::FactoryMethod));
    }

    #[test]
    fn test_abstract_factory_interface() {
        let mut tg = TypeGraph::new();
        tg.interfaces.insert("IWidgetFactory".into(), InterfaceInfo {
            name: "IWidgetFactory".into(),
            methods: vec![
                MethodDescriptor {
                    class: "IWidgetFactory".into(), method: "MakeButton".into(),
                    signature: "IButton MakeButton".into(),
                    is_static: false, is_virtual: false, is_abstract: false,
                },
                MethodDescriptor {
                    class: "IWidgetFactory".into(), method: "MakeDialog".into(),
                    signature: "IDialog MakeDialog".into(),
                    is_static: false, is_virtual: false, is_abstract: false,
                },
            ],
        });
        tg.interfaces.insert("IButton".into(), InterfaceInfo {
            name: "IButton".into(), methods: vec![],
        });
        tg.interfaces.insert("IDialog".into(), InterfaceInfo {
            name: "IDialog".into(), methods: vec![],
        });
        let ctx = DetectionContext::new(&tg, "");
        let r = detect_creational(&ctx);
        assert!(r.iter().any(|m| m.pattern == PatternKind::AbstractFactory));
    }

    #[test]
    fn test_builder_fluent_methods() {
        let mut tg = TypeGraph::new();
        tg.classes.insert("HtmlBuilder".into(), ClassInfo {
            name: "HtmlBuilder".into(), base_class: None, interfaces: vec![],
            methods: vec![
                MethodDescriptor { class: "HtmlBuilder".into(), method: "SetTitle".into(),
                    signature: "HtmlBuilder SetTitle".into(),
                    is_static: false, is_virtual: false, is_abstract: false },
                MethodDescriptor { class: "HtmlBuilder".into(), method: "SetBody".into(),
                    signature: "HtmlBuilder SetBody".into(),
                    is_static: false, is_virtual: false, is_abstract: false },
                MethodDescriptor { class: "HtmlBuilder".into(), method: "Build".into(),
                    signature: "string Build".into(),
                    is_static: false, is_virtual: false, is_abstract: false },
            ],
            fields: vec![],
            is_abstract: false, is_sealed: false, is_static: false,
        });
        let ctx = DetectionContext::new(&tg, "");
        let r = detect_creational(&ctx);
        assert!(r.iter().any(|m| m.pattern == PatternKind::Builder));
    }

    #[test]
    fn test_prototype_icloneable() {
        let mut tg = TypeGraph::new();
        tg.classes.insert("Entity".into(), ClassInfo {
            name: "Entity".into(), base_class: None,
            interfaces: vec!["ICloneable".into()],
            fields: vec![],
            methods: vec![], is_abstract: false, is_sealed: false, is_static: false,
        });
        let ctx = DetectionContext::new(&tg, "");
        let r = detect_creational(&ctx);
        assert!(r.iter().any(|m| m.pattern == PatternKind::Prototype));
    }
}
