use std::collections::HashSet;

use crate::detect::types::{DetectionContext, PatternKind, PatternMatch};

pub fn detect_behavioral(ctx: &DetectionContext) -> Vec<PatternMatch> {
    let mut results = Vec::new();

    detect_strategy_or_command_or_state(ctx, &mut results);
    detect_observer(ctx, &mut results);
    detect_mediator(ctx, &mut results);
    detect_dotnet_mediator(ctx, &mut results);
    detect_chain_of_responsibility(ctx, &mut results);
    detect_handler(ctx, &mut results);
    detect_template_method(ctx, &mut results);
    detect_visitor(ctx, &mut results);
    detect_iterator(ctx, &mut results);
    detect_memento(ctx, &mut results);
    detect_interpreter(ctx, &mut results);

    results
}

fn return_type_of(m: &crate::resolve::types::MethodDescriptor) -> &str {
    m.signature.split_whitespace().next().unwrap_or("void")
}

/// Strategy/Command/State are structurally identical: single-method interface
/// with >=2 implementations. Differentiate via PDG call-graph:
///
/// State:
///   - An implementor calls the method on itself (self-call)
///   - The implementor participates in a state machine (transitions between states)
///
/// Command:
///   - An invoker class creates implementors via `new` or receives them externally
///   - The invoker calls the method (Execute/Run/Handle semantic)
///
/// Strategy:
///   - A context class receives the interface as a parameter and calls the method
///   - No creation of implementors, no self-referencing
fn detect_strategy_or_command_or_state(ctx: &DetectionContext, results: &mut Vec<PatternMatch>) {
    for (iface_name, iface) in &ctx.type_graph.interfaces {
        let non_accessor: Vec<_> = iface.methods.iter()
            .filter(|m| !m.method.starts_with("get_") && !m.method.starts_with("set_"))
            .collect();

        if non_accessor.len() != 1 { continue; }

        let implementors = ctx.type_graph.concrete_subclasses(iface_name);
        if implementors.len() < 2 { continue; }

        let sig = &non_accessor[0];
        let method_name = &sig.method;

        let (pattern, confidence, evidence) = if let Some(cg) = ctx.callgraph {
            // 1. State signal: an implementor calls the method on itself (self-call)
            let has_state_self_call = implementors.iter().any(|impl_class| {
                cg.calls.iter().any(|c| {
                    c.caller_class == impl_class.name
                        && c.callee == *method_name
                        && c.is_self_call
                })
            });

            // 2. State signal: the implementor's own method body calls the interface method
            let has_state_internal_call = implementors.iter().any(|impl_class| {
                cg.class_callees.get(&impl_class.name)
                    .map(|callees| callees.contains(method_name))
                    .unwrap_or(false)
            });

            // 3. Command signal: an external (non-implementor) class creates implementors via `new`
            let has_external_creator = implementors.iter().any(|impl_class| {
                cg.class_creations.iter().any(|(creator_class, created_types)| {
                    creator_class != &impl_class.name  // not self-creation
                        && created_types.contains(&impl_class.name)
                })
            });

            // 4. Command signal: an external class both creates implementors AND calls the method
            let has_invoker = implementors.iter().any(|impl_class| {
                cg.class_creations.iter().any(|(creator_class, created_types)| {
                    if creator_class != &impl_class.name
                        && created_types.contains(&impl_class.name)
                    {
                        cg.class_callees.get(creator_class)
                            .map(|callees| callees.contains(method_name))
                            .unwrap_or(false)
                    } else {
                        false
                    }
                })
            });

            // 5. Strategy signal: context class has method that takes iface as parameter
            let has_param_context = ctx.type_graph.classes.values().any(|class| {
                if implementors.iter().any(|i| i.name == class.name) { return false; }
                class.methods.iter().any(|m| {
                    let sig = &m.signature;
                    if let Some(start) = sig.find('(') {
                        if let Some(end) = sig.find(')') {
                            return sig[start+1..end].contains(iface_name);
                        }
                    }
                    false
                })
            });

            // 6. Strategy signal: context calls the method but doesn't create implementors
            let has_context_caller = implementors.iter().any(|impl_class| {
                cg.class_callees.iter().any(|(caller, callees)| {
                    caller != &impl_class.name
                        && callees.contains(method_name)
                })
            });

            // 7. Command signal: method name suggests command
            let is_command_name = method_name == "Execute" || method_name == "Run"
                || method_name == "Handle";

            // 8. Combined: param context + method caller = usage pattern
            let is_used_by_context = has_param_context && has_context_caller;

            // Decide
            if has_state_self_call || has_state_internal_call {
                let mut ev = vec![format!("self-call by implementor")];
                if has_state_self_call { ev.push("self-call".into()); }
                (PatternKind::State, 0.75, ev)
            } else if has_invoker {
                let mut ev = vec![format!("invoker creates implementors")];
                if has_invoker { ev.push("creates+calls".into()); }
                (PatternKind::Command, 0.8, ev)
            } else if is_command_name && has_context_caller {
                // Invoker calls Execute/Run/Handle on the command → Command
                (PatternKind::Command, 0.75, vec!["command name + caller".into()])
            } else if is_command_name {
                (PatternKind::Command, 0.55, vec!["command name".into()])
            } else if is_used_by_context && !has_external_creator {
                (PatternKind::Strategy, 0.85, vec!["context param + call".into()])
            } else if has_param_context {
                (PatternKind::Strategy, 0.7, vec!["context param".into()])
            } else if has_context_caller && !has_external_creator {
                (PatternKind::Strategy, 0.75, vec!["context caller".into()])
            } else if has_external_creator {
                (PatternKind::Command, 0.6, vec!["external creation".into()])
            } else {
                (PatternKind::Strategy, 0.9, vec!["default".into()])
            }
        } else {
            // No callgraph: use signature analysis
            // Check if any class has a method that accepts the iface as param → Strategy
            let has_param_context = ctx.type_graph.classes.values().any(|class| {
                if implementors.iter().any(|i| i.name == class.name) { return false; }
                class.methods.iter().any(|m| {
                    let sig = &m.signature;
                    if let Some(start) = sig.find('(') {
                        if let Some(end) = sig.find(')') {
                            return sig[start+1..end].contains(iface_name);
                        }
                    }
                    false
                })
            });

            let is_command_name = method_name == "Execute" || method_name == "Run"
                || method_name == "Handle";

            if is_command_name {
                (PatternKind::Command, 0.55, vec!["command name".into()])
            } else if has_param_context {
                (PatternKind::Strategy, 0.7, vec!["context param".into()])
            } else {
                (PatternKind::Strategy, 0.9, vec!["no callgraph".into()])
            }
        };

        results.push(PatternMatch {
            pattern,
            class: iface_name.clone(),
            description: format!(
                "Interface '{}' has single method '{}' with {} implementations",
                iface_name, sig.method, implementors.len()
            ),
            confidence,
            participants: {
                let mut p = vec![iface_name.clone()];
                p.extend(implementors.iter().map(|c| c.name.clone()));
                p
            },
            evidence,
        });
    }
}

fn detect_observer(ctx: &DetectionContext, results: &mut Vec<PatternMatch>) {
    // Observer: interface/class with both subscription and notification.
    // Subscription: method that takes a callback/interface parameter
    // Notification: method that calls the callback/interface method
    for (iface_name, iface) in &ctx.type_graph.interfaces {
        let methods: Vec<_> = iface.methods.iter()
            .filter(|m| !m.method.starts_with("get_") && !m.method.starts_with("set_"))
            .collect();

        if methods.len() < 2 { continue; }

        let has_param_matching_iface = methods.iter().any(|m| {
            m.signature.contains(iface_name)
        });

        if has_param_matching_iface {
            let implementors = ctx.type_graph.concrete_subclasses(iface_name);
            results.push(PatternMatch {
                pattern: PatternKind::Observer,
                class: iface_name.clone(),
                description: format!(
                    "Interface '{}' has subscription pattern with {} implementations",
                    iface_name, implementors.len()
                ),
                confidence: 0.75,
                participants: vec![iface_name.clone()],
                evidence: vec![format!("methods: {}", methods.len())],
            });
        }
    }

    // Also detect via class implementing IObservable/IObserver
    for class in ctx.type_graph.classes.values() {
        let matches = class.interfaces.iter()
            .any(|i| i == "IObservable" || i == "IObserver");
        if matches {
            results.push(PatternMatch {
                pattern: PatternKind::Observer,
                class: class.name.clone(),
                description: format!("'{}' implements IObservable/IObserver", class.name),
                confidence: 0.9,
                participants: vec![class.name.clone()],
                evidence: class.interfaces.clone(),
            });
        }
    }
}

fn detect_mediator(ctx: &DetectionContext, results: &mut Vec<PatternMatch>) {
    // Mediator: interface with >=2 methods, exactly 1 concrete implementor,
    // and other non-implementing classes with action methods (components).
    for (iface_name, iface) in &ctx.type_graph.interfaces {
        let methods: Vec<_> = iface.methods.iter()
            .filter(|m| !m.method.starts_with("get_") && !m.method.starts_with("set_"))
            .collect();

        if methods.len() < 2 { continue; }

        // Must have exactly 1 concrete implementor
        let implementors: Vec<_> = ctx.type_graph.classes.values()
            .filter(|c| !c.is_abstract && c.interfaces.iter().any(|i| i == iface_name))
            .collect();
        if implementors.len() != 1 { continue; }

        // There must be other classes that don't implement the interface
        // but have "component" behavior (non-accessor methods)
        let component_count = ctx.type_graph.classes.values()
            .filter(|c| !c.interfaces.iter().any(|i| i == iface_name))
            .filter(|c| {
                c.methods.iter().any(|m| {
                    !m.method.starts_with("get_") && !m.method.starts_with("set_")
                        && m.method != c.name // not a constructor
                })
            })
            .count();
        // At least 2 component-like classes besides the mediator itself and the mediator
        let non_mediator_count = ctx.type_graph.classes.len().saturating_sub(implementors.len());
        if non_mediator_count < 2 && component_count < 2 { continue; }

        results.push(PatternMatch {
            pattern: PatternKind::Mediator,
            class: iface_name.clone(),
            description: format!(
                "Interface '{}' has {} implementor with {} component classes — Mediator",
                iface_name, implementors.len(), component_count
            ),
            confidence: 0.6,
            participants: vec![iface_name.clone()],
            evidence: methods.iter().map(|m| m.method.clone()).collect(),
        });
    }
}

fn detect_dotnet_mediator(ctx: &DetectionContext, results: &mut Vec<PatternMatch>) {
    // .NET MediatR: IMediator-like interface (>=2 methods, 1 implementor)
    // paired with separate handler interfaces (1 method, >=2 implementors each).
    // The mediator implementor does NOT implement handler interfaces.
    for (iface_name, iface) in &ctx.type_graph.interfaces {
        let methods: Vec<_> = iface.methods.iter()
            .filter(|m| !m.method.starts_with("get_") && !m.method.starts_with("set_"))
            .collect();
        if methods.len() < 2 { continue; }

        let mediator_impls: Vec<_> = ctx.type_graph.classes.values()
            .filter(|c| !c.is_abstract && c.interfaces.iter().any(|i| i == iface_name))
            .collect();
        if mediator_impls.len() != 1 { continue; }
        let mediator_class = mediator_impls[0];

        // Find handler interfaces: single method, >=2 implementors,
        // and mediator implementor does NOT implement them
        let handler_ifaces: Vec<_> = ctx.type_graph.interfaces.iter()
            .filter(|(h_name, h_iface)| {
                if *h_name == iface_name { return false; }
                let h_methods: Vec<_> = h_iface.methods.iter()
                    .filter(|m| !m.method.starts_with("get_") && !m.method.starts_with("set_"))
                    .collect();
                if h_methods.len() != 1 { return false; }

                let h_impls: Vec<_> = ctx.type_graph.classes.values()
                    .filter(|c| !c.is_abstract && c.interfaces.iter().any(|i| i == *h_name))
                    .collect();
                if h_impls.len() < 2 { return false; }

                // Mediator class must NOT implement this handler interface
                if mediator_class.interfaces.iter().any(|i| i == *h_name) {
                    return false;
                }

                true
            })
            .collect();

        if handler_ifaces.len() < 2 { continue; }

        results.push(PatternMatch {
            pattern: PatternKind::DotnetMediator,
            class: iface_name.clone(),
            description: format!(
                "'{}' dispatches to {} handler interfaces — .NET MediatR",
                iface_name, handler_ifaces.len()
            ),
            confidence: 0.85,
            participants: {
                let mut p = vec![iface_name.clone()];
                p.extend(handler_ifaces.iter().map(|(n, _)| (*n).clone()));
                p
            },
            evidence: handler_ifaces.iter().map(|(n, _)| (*n).clone()).collect(),
        });
    }
}

fn detect_chain_of_responsibility(ctx: &DetectionContext, results: &mut Vec<PatternMatch>) {
    for (iface_name, iface) in &ctx.type_graph.interfaces {
        let non_accessor: Vec<_> = iface.methods.iter()
            .filter(|m| !m.method.starts_with("get_") && !m.method.starts_with("set_"))
            .collect();

        if non_accessor.len() < 2 { continue; }

        let implementors = ctx.type_graph.concrete_subclasses(iface_name);
        if implementors.len() < 2 { continue; }

        results.push(PatternMatch {
            pattern: PatternKind::ChainOfResponsibility,
            class: iface_name.clone(),
            description: format!(
                "Interface '{}' has {} methods and {} implementations — chain",
                iface_name, non_accessor.len(), implementors.len()
            ),
            confidence: 0.5,
            participants: vec![iface_name.clone()],
            evidence: vec![
                format!("methods: {}", non_accessor.len()),
                format!("implementations: {}", implementors.len()),
            ],
        });
    }
}

fn detect_handler(ctx: &DetectionContext, results: &mut Vec<PatternMatch>) {
    // Handler (CQRS): detects when there are both command and query
    // handler interfaces, each with >=2 implementations.
    let handler_ifaces: Vec<_> = ctx.type_graph.interfaces.iter()
        .filter(|(name, iface)| {
            let non_accessor: Vec<_> = iface.methods.iter()
                .filter(|m| !m.method.starts_with("get_") && !m.method.starts_with("set_"))
                .collect();
            non_accessor.len() == 1
                && ctx.type_graph.concrete_subclasses(name).len() >= 2
        })
        .collect();

    // Only detect Handler if there are 2+ such interfaces (command + query)
    if handler_ifaces.len() >= 2 {
        for (name, _) in &handler_ifaces {
            results.push(PatternMatch {
                pattern: PatternKind::Handler,
                class: name.to_string(),
                description: format!("'{}' is a handler in a CQRS-style architecture", name),
                confidence: 0.7,
                participants: handler_ifaces.iter().map(|(n, _)| (*n).clone()).collect(),
                evidence: vec![],
            });
        }
    }
}

fn detect_template_method(ctx: &DetectionContext, results: &mut Vec<PatternMatch>) {
    for class in ctx.type_graph.classes.values() {
        if !class.is_abstract { continue; }

        let abstract_methods: Vec<_> = class.methods.iter()
            .filter(|m| m.is_abstract)
            .collect();

        let concrete_methods: Vec<_> = class.methods.iter()
            .filter(|m| !m.is_abstract && !m.is_virtual
                && !m.method.starts_with("get_") && !m.method.starts_with("set_"))
            .collect();

        if abstract_methods.is_empty() || concrete_methods.is_empty() { continue; }

        results.push(PatternMatch {
            pattern: PatternKind::TemplateMethod,
            class: class.name.clone(),
            description: format!(
                "Abstract '{}' has {} abstract + {} concrete methods — Template Method",
                class.name, abstract_methods.len(), concrete_methods.len()
            ),
            confidence: 0.75,
            participants: vec![class.name.clone()],
            evidence: vec![
                format!("abstract: {}", abstract_methods.iter().map(|m| m.method.clone()).collect::<Vec<_>>().join(", ")),
                format!("concrete: {}", concrete_methods.iter().map(|m| m.method.clone()).collect::<Vec<_>>().join(", ")),
            ],
        });
    }
}

fn detect_visitor(ctx: &DetectionContext, results: &mut Vec<PatternMatch>) {
    // Visitor: pair of interfaces where one (Element) has exactly 1 void method
    // and the other (Visitor) has >=2 methods, with >=2 Element implementations.
    let iface_names: Vec<String> = ctx.type_graph.interfaces.keys().cloned().collect();

    for comp_name in &iface_names {
        let comp_iface = match ctx.type_graph.interfaces.get(comp_name) {
            Some(i) => i,
            None => continue,
        };

        let comp_non_accessor: Vec<_> = comp_iface.methods.iter()
            .filter(|m| !m.method.starts_with("get_") && !m.method.starts_with("set_"))
            .collect();

        if comp_non_accessor.len() != 1 { continue; }
        if return_type_of(&comp_non_accessor[0]) != "void" { continue; }

        let comp_implementors = ctx.type_graph.concrete_subclasses(comp_name);
        if comp_implementors.len() < 2 { continue; }

        for vis_name in &iface_names {
            if vis_name == comp_name { continue; }

            let vis_iface = match ctx.type_graph.interfaces.get(vis_name) {
                Some(i) => i,
                None => continue,
            };

            let vis_non_accessor: Vec<_> = vis_iface.methods.iter()
                .filter(|m| !m.method.starts_with("get_") && !m.method.starts_with("set_"))
                .collect();

            if vis_non_accessor.len() < 2 { continue; }

            results.push(PatternMatch {
                pattern: PatternKind::Visitor,
                class: vis_name.clone(),
                description: format!(
                    "'{}' has {} methods over {} component types — Visitor",
                    vis_name, vis_non_accessor.len(), comp_implementors.len()
                ),
                confidence: 0.7,
                participants: vec![vis_name.clone(), comp_name.clone()],
                evidence: vis_non_accessor.iter().map(|m| m.method.clone()).collect(),
            });
            break;
        }
    }
}

fn detect_iterator(ctx: &DetectionContext, results: &mut Vec<PatternMatch>) {
    let iter_interfaces: HashSet<&str> =
        ["IEnumerable", "IEnumerator"].iter().copied().collect();

    for class in ctx.type_graph.classes.values() {
        let impls = class.interfaces.iter().any(|i| iter_interfaces.contains(i.as_str()));
        if impls {
            results.push(PatternMatch {
                pattern: PatternKind::Iterator,
                class: class.name.clone(),
                description: format!("'{}' implements IEnumerable/IEnumerator", class.name),
                confidence: 0.95,
                participants: vec![class.name.clone()],
                evidence: class.interfaces.clone(),
            });
        }
    }

    for (iface_name, iface) in &ctx.type_graph.interfaces {
        if iter_interfaces.contains(iface_name.as_str()) {
            let implementors = ctx.type_graph.concrete_subclasses(iface_name);
            results.push(PatternMatch {
                pattern: PatternKind::Iterator,
                class: iface_name.clone(),
                description: format!("Interface '{}' with {} implementations", iface_name, implementors.len()),
                confidence: 0.9,
                participants: vec![iface_name.clone()],
                evidence: iface.methods.iter().map(|m| m.method.clone()).collect(),
            });
        }
    }
}

fn detect_memento(ctx: &DetectionContext, results: &mut Vec<PatternMatch>) {
    for class in ctx.type_graph.classes.values() {
        if class.is_abstract || class.is_static { continue; }

        let total = class.methods.len();
        let accessors = class.methods.iter()
            .filter(|m| m.method.starts_with("get_") || m.method.starts_with("set_"))
            .count();

        if total > 0 && accessors == total && total <= 6 {
            results.push(PatternMatch {
                pattern: PatternKind::Memento,
                class: class.name.clone(),
                description: format!("'{}' has only property accessors — state snapshot", class.name),
                confidence: 0.4,
                participants: vec![class.name.clone()],
                evidence: class.methods.iter().map(|m| m.method.clone()).collect(),
            });
        }
    }
}

fn detect_interpreter(ctx: &DetectionContext, results: &mut Vec<PatternMatch>) {
    for (iface_name, iface) in &ctx.type_graph.interfaces {
        let non_accessor: Vec<_> = iface.methods.iter()
            .filter(|m| !m.method.starts_with("get_") && !m.method.starts_with("set_"))
            .collect();

        if non_accessor.len() != 1 { continue; }

        let implementors = ctx.type_graph.concrete_subclasses(iface_name);
        if implementors.len() >= 2 {
            let rt = return_type_of(&non_accessor[0]);
            if rt != "void" {
                results.push(PatternMatch {
                    pattern: PatternKind::Interpreter,
                    class: iface_name.clone(),
                    description: format!(
                        "Interface '{}' with {} expression types — Interpreter",
                        iface_name, implementors.len()
                    ),
                    confidence: 0.8,
                    participants: vec![iface_name.clone()],
                    evidence: vec![
                        format!("method: {}", non_accessor[0].method),
                        format!("implementations: {}", implementors.len()),
                    ],
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::symbols::SymbolTable;
    use crate::resolve::types::*;

    #[test]
    fn test_command_single_method_interface() {
        let mut tg = TypeGraph::new();
        tg.interfaces.insert("ICommand".into(), InterfaceInfo {
            name: "ICommand".into(),
            methods: vec![MethodDescriptor {
                class: "ICommand".into(), method: "Run".into(),
                signature: "void Run".into(),
                is_static: false, is_virtual: false, is_abstract: false,
file: String::new(),
line_start: 0,
line_end: 0
}],
        });
        tg.classes.insert("SaveCmd".into(), ClassInfo {
            name: "SaveCmd".into(), base_class: None,
            interfaces: vec!["ICommand".into()],
            methods: vec![MethodDescriptor { class: "SaveCmd".into(), method: "Run".into(),
                signature: "void Run".into(), is_static: false,
                is_virtual: false, is_abstract: false, file: String::new(), line_start: 0, line_end: 0 }],
            fields: vec![],
            is_abstract: false, is_sealed: false, is_static: false,
        });
        tg.classes.insert("DeleteCmd".into(), ClassInfo {
            name: "DeleteCmd".into(), base_class: None,
            interfaces: vec!["ICommand".into()],
            methods: vec![MethodDescriptor { class: "DeleteCmd".into(), method: "Run".into(),
                signature: "void Run".into(), is_static: false,
                is_virtual: false, is_abstract: false, file: String::new(), line_start: 0, line_end: 0 }],
            fields: vec![],
            is_abstract: false, is_sealed: false, is_static: false,
        });
        let ctx = DetectionContext::new(&tg, "");
        let r = detect_behavioral(&ctx);
        assert!(r.iter().any(|m| m.pattern == PatternKind::Command));
    }

    #[test]
    fn test_state_self_call() {
        let mut tg = TypeGraph::new();
        tg.interfaces.insert("IState".into(), InterfaceInfo {
            name: "IState".into(),
            methods: vec![MethodDescriptor {
                class: "IState".into(), method: "Handle".into(),
                signature: "void Handle".into(),
                is_static: false, is_virtual: false, is_abstract: false,
file: String::new(),
line_start: 0,
line_end: 0
}],
        });
        tg.classes.insert("ConcreteStateA".into(), ClassInfo {
            name: "ConcreteStateA".into(), base_class: None,
            interfaces: vec!["IState".into()],
            methods: vec![MethodDescriptor { class: "ConcreteStateA".into(), method: "Handle".into(),
                signature: "void Handle".into(), is_static: false,
                is_virtual: false, is_abstract: false, file: String::new(), line_start: 0, line_end: 0 }],
            fields: vec![], is_abstract: false, is_sealed: false, is_static: false,
        });
        tg.classes.insert("ConcreteStateB".into(), ClassInfo {
            name: "ConcreteStateB".into(), base_class: None,
            interfaces: vec!["IState".into()],
            methods: vec![MethodDescriptor { class: "ConcreteStateB".into(), method: "Handle".into(),
                signature: "void Handle".into(), is_static: false,
                is_virtual: false, is_abstract: false, file: String::new(), line_start: 0, line_end: 0 }],
            fields: vec![], is_abstract: false, is_sealed: false, is_static: false,
        });
        // Build a callgraph where ConcreteStateA calls Handle on itself
        let src = "class ConcreteStateA : IState { void Handle() { this.Handle(); } }";
        let tree = crate::parse::parser::parse_source(src).unwrap();
        let st = SymbolTable::from_ast(tree.root_node(), src).unwrap();
        let cg = crate::analysis::callgraph::CallGraphBuilder::build(
            tree.root_node(), src, &st.type_graph);
        // Merge type_graphs
        for (name, _info) in &st.type_graph.classes {
            let n = name.clone();
            tg.classes.entry(n.clone()).or_insert_with(|| {
                ClassInfo {
                    name: n, base_class: None, interfaces: vec![],
                    methods: vec![], fields: vec![],
                    is_abstract: false, is_sealed: false, is_static: false,
                }
            });
        }
        let ctx = DetectionContext::with_callgraph(&tg, &cg, "");
        let r = detect_behavioral(&ctx);
        assert!(r.iter().any(|m| m.pattern == PatternKind::State));
    }

    #[test]
    fn test_template_method_abstract_with_concrete_methods() {
        let mut tg = TypeGraph::new();
        tg.classes.insert("DataProcessor".into(), ClassInfo {
            name: "DataProcessor".into(),
            base_class: None, interfaces: vec![],
            methods: vec![
                MethodDescriptor { class: "DataProcessor".into(), method: "Process".into(),
                    signature: "void Process".into(),
                    is_static: false, is_virtual: false, is_abstract: false, file: String::new(), line_start: 0, line_end: 0 },
                MethodDescriptor { class: "DataProcessor".into(), method: "ReadData".into(),
                    signature: "string ReadData".into(),
                    is_static: false, is_virtual: false, is_abstract: true, file: String::new(), line_start: 0, line_end: 0 },
                MethodDescriptor { class: "DataProcessor".into(), method: "WriteData".into(),
                    signature: "void WriteData".into(),
                    is_static: false, is_virtual: false, is_abstract: true, file: String::new(), line_start: 0, line_end: 0 },
            ],
            fields: vec![],
            is_abstract: true, is_sealed: false, is_static: false,
        });
        let ctx = DetectionContext::new(&tg, "");
        let r = detect_behavioral(&ctx);
        assert!(r.iter().any(|m| m.pattern == PatternKind::TemplateMethod));
    }

    #[test]
    fn test_visitor_accept_pattern() {
        let mut tg = TypeGraph::new();
        tg.interfaces.insert("IElement".into(), InterfaceInfo {
            name: "IElement".into(),
            methods: vec![MethodDescriptor {
                class: "IElement".into(), method: "Accept".into(),
                signature: "void Accept".into(),
                is_static: false, is_virtual: false, is_abstract: false,
file: String::new(),
line_start: 0,
line_end: 0
}],
        });
        tg.interfaces.insert("IVisitor".into(), InterfaceInfo {
            name: "IVisitor".into(),
            methods: vec![
                MethodDescriptor { class: "IVisitor".into(), method: "VisitA".into(),
                    signature: "void VisitA".into(),
                    is_static: false, is_virtual: false, is_abstract: false, file: String::new(), line_start: 0, line_end: 0 },
                MethodDescriptor { class: "IVisitor".into(), method: "VisitB".into(),
                    signature: "void VisitB".into(),
                    is_static: false, is_virtual: false, is_abstract: false, file: String::new(), line_start: 0, line_end: 0 },
            ],
        });
        tg.classes.insert("ElemA".into(), ClassInfo {
            name: "ElemA".into(), base_class: None,
            interfaces: vec!["IElement".into()],
            methods: vec![MethodDescriptor { class: "ElemA".into(), method: "Accept".into(),
                signature: "void Accept".into(), is_static: false,
                is_virtual: false, is_abstract: false, file: String::new(), line_start: 0, line_end: 0 }],
            fields: vec![],
            is_abstract: false, is_sealed: false, is_static: false,
        });
        tg.classes.insert("ElemB".into(), ClassInfo {
            name: "ElemB".into(), base_class: None,
            interfaces: vec!["IElement".into()],
            methods: vec![MethodDescriptor { class: "ElemB".into(), method: "Accept".into(),
                signature: "void Accept".into(), is_static: false,
                is_virtual: false, is_abstract: false, file: String::new(), line_start: 0, line_end: 0 }],
            fields: vec![],
            is_abstract: false, is_sealed: false, is_static: false,
        });
        let ctx = DetectionContext::new(&tg, "");
        let r = detect_behavioral(&ctx);
        assert!(r.iter().any(|m| m.pattern == PatternKind::Visitor));
    }

    #[test]
    fn test_iterator_enumerable() {
        let mut tg = TypeGraph::new();
        tg.classes.insert("MyList".into(), ClassInfo {
            name: "MyList".into(), base_class: None,
            interfaces: vec!["IEnumerable".into()],
            methods: vec![MethodDescriptor { class: "MyList".into(), method: "GetEnumerator".into(),
                signature: "IEnumerator GetEnumerator".into(),
                is_static: false, is_virtual: false, is_abstract: false, file: String::new(), line_start: 0, line_end: 0 }],
            fields: vec![],
            is_abstract: false, is_sealed: false, is_static: false,
        });
        let ctx = DetectionContext::new(&tg, "");
        let r = detect_behavioral(&ctx);
        assert!(r.iter().any(|m| m.pattern == PatternKind::Iterator));
    }

    #[test]
    fn test_observer_observable() {
        let mut tg = TypeGraph::new();
        tg.classes.insert("StockTicker".into(), ClassInfo {
            name: "StockTicker".into(), base_class: None,
            interfaces: vec!["IObservable".into()],
            methods: vec![MethodDescriptor { class: "StockTicker".into(),
                method: "Subscribe".into(), signature: "void Subscribe".into(),
                is_static: false, is_virtual: false, is_abstract: false, file: String::new(), line_start: 0, line_end: 0 }],
            fields: vec![],
            is_abstract: false, is_sealed: false, is_static: false,
        });
        let ctx = DetectionContext::new(&tg, "");
        let r = detect_behavioral(&ctx);
        assert!(r.iter().any(|m| m.pattern == PatternKind::Observer));
    }

    #[test]
    fn test_chain_of_responsibility() {
        let mut tg = TypeGraph::new();
        tg.interfaces.insert("IHandler".into(), InterfaceInfo {
            name: "IHandler".into(),
            methods: vec![
                MethodDescriptor { class: "IHandler".into(), method: "Process".into(),
                    signature: "void Process".into(),
                    is_static: false, is_virtual: false, is_abstract: false, file: String::new(), line_start: 0, line_end: 0 },
                MethodDescriptor { class: "IHandler".into(), method: "SetNext".into(),
                    signature: "void SetNext".into(),
                    is_static: false, is_virtual: false, is_abstract: false, file: String::new(), line_start: 0, line_end: 0 },
            ],
        });
        tg.classes.insert("AuthHandler".into(), ClassInfo {
            name: "AuthHandler".into(), base_class: None,
            interfaces: vec!["IHandler".into()],
            methods: vec![
                MethodDescriptor { class: "AuthHandler".into(), method: "Process".into(),
                    signature: "void Process".into(),
                    is_static: false, is_virtual: false, is_abstract: false, file: String::new(), line_start: 0, line_end: 0 },
                MethodDescriptor { class: "AuthHandler".into(), method: "SetNext".into(),
                    signature: "void SetNext".into(),
                    is_static: false, is_virtual: false, is_abstract: false, file: String::new(), line_start: 0, line_end: 0 },
            ],
            fields: vec![],
            is_abstract: false, is_sealed: false, is_static: false,
        });
        tg.classes.insert("LogHandler".into(), ClassInfo {
            name: "LogHandler".into(), base_class: None,
            interfaces: vec!["IHandler".into()],
            methods: vec![
                MethodDescriptor { class: "LogHandler".into(), method: "Process".into(),
                    signature: "void Process".into(),
                    is_static: false, is_virtual: false, is_abstract: false, file: String::new(), line_start: 0, line_end: 0 },
                MethodDescriptor { class: "LogHandler".into(), method: "SetNext".into(),
                    signature: "void SetNext".into(),
                    is_static: false, is_virtual: false, is_abstract: false, file: String::new(), line_start: 0, line_end: 0 },
            ],
            fields: vec![],
            is_abstract: false, is_sealed: false, is_static: false,
        });
        let ctx = DetectionContext::new(&tg, "");
        let r = detect_behavioral(&ctx);
        assert!(r.iter().any(|m| m.pattern == PatternKind::ChainOfResponsibility));
    }

    #[test]
    fn test_interpreter_expression_hierarchy() {
        let mut tg = TypeGraph::new();
        tg.interfaces.insert("IExpression".into(), InterfaceInfo {
            name: "IExpression".into(),
            methods: vec![MethodDescriptor {
                class: "IExpression".into(), method: "Eval".into(),
                signature: "int Eval".into(),
                is_static: false, is_virtual: false, is_abstract: false,
file: String::new(),
line_start: 0,
line_end: 0
}],
        });
        tg.classes.insert("NumberExpr".into(), ClassInfo {
            name: "NumberExpr".into(), base_class: None,
            interfaces: vec!["IExpression".into()],
            methods: vec![MethodDescriptor { class: "NumberExpr".into(),
                method: "Eval".into(), signature: "int Eval".into(),
                is_static: false, is_virtual: false, is_abstract: false, file: String::new(), line_start: 0, line_end: 0 }],
            fields: vec![],
            is_abstract: false, is_sealed: false, is_static: false,
        });
        tg.classes.insert("AddExpr".into(), ClassInfo {
            name: "AddExpr".into(), base_class: None,
            interfaces: vec!["IExpression".into()],
            methods: vec![MethodDescriptor { class: "AddExpr".into(), method: "Eval".into(),
                signature: "int Eval".into(),
                is_static: false, is_virtual: false, is_abstract: false, file: String::new(), line_start: 0, line_end: 0 }],
            fields: vec![],
            is_abstract: false, is_sealed: false, is_static: false,
        });
        let ctx = DetectionContext::new(&tg, "");
        let r = detect_behavioral(&ctx);
        assert!(r.iter().any(|m| m.pattern == PatternKind::Interpreter));
    }
}
