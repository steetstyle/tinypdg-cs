//! Direct call resolution: static, non-virtual instance, private calls.
//!
//! Kategori 1 calls have 100% confidence — the target method is
//! deterministically known at compile time.

use tree_sitter::Node;

use crate::resolve::types::{CallSite, CallTarget, Confidence, MethodDescriptor};
use crate::resolve::symbols::SymbolTable;

/// Check if a call expression is a direct (non-virtual) invocation
pub fn resolve_direct(
    node: Node,
    source: &str,
    caller: &str,
    _symbols: &SymbolTable,
) -> Option<CallSite> {
    let target = classify_call(node, source)?;

    // Direct resolution is only for static and non-virtual instance calls
    let confidence = match &target {
        CallTarget::Static { .. } => Confidence::Direct,
        CallTarget::Instance { .. } => {
            // Instance calls without `virtual` target = direct
            // (we can verify via symbol table, but for now assume non-virtual)
            Confidence::Direct
        }
        _ => return None,
    };

    let resolved = vec![MethodDescriptor {
        class: match &target {
            CallTarget::Static { class, .. } => class.clone(),
            CallTarget::Instance { .. } => String::new(),
            _ => return None,
        },
        method: match &target {
            CallTarget::Static { method, .. } => method.clone(),
            CallTarget::Instance { method } => method.clone(),
            _ => return None,
        },
        is_static: matches!(&target, CallTarget::Static { .. }),
        ..Default::default()
    }];

    Some(CallSite {
        caller: caller.to_string(),
        target,
        confidence,
        resolved,
    })
}

/// Classify a tree-sitter invocation node into a CallTarget
pub fn classify_call(node: Node, source: &str) -> Option<CallTarget> {
    let kind = node.kind();
    match kind {
        "invocation_expression" => {
            let func = node.child_by_field_name("function")?;
            classify_function(func, source)
        }
        "object_creation_expression" => {
            let type_node = node.child_by_field_name("type")?;
            let class = type_node.utf8_text(source.as_bytes()).ok()?;
            Some(CallTarget::Static {
                class: class.to_string(),
                method: ".ctor".to_string(),
            })
        }
        _ => None,
    }
}

fn classify_function(node: Node, source: &str) -> Option<CallTarget> {
    match node.kind() {
        "identifier" => {
            let name = node.utf8_text(source.as_bytes()).ok()?;
            Some(CallTarget::Instance {
                method: name.to_string(),
            })
        }
        "member_access_expression" => {
            let member = node.child_by_field_name("name")?;
            let method = member.utf8_text(source.as_bytes()).ok()?.to_string();
            let expr = node.child_by_field_name("expression")?;

            // If expression is an identifier, it could be static
            if expr.kind() == "identifier" {
                let expr_text = expr.utf8_text(source.as_bytes()).ok()?;
                // Uppercase start heuristic for static classes
                if expr_text.chars().next().map_or(false, |c| c.is_uppercase()) {
                    return Some(CallTarget::Static {
                        class: expr_text.to_string(),
                        method,
                    });
                }
            }

            Some(CallTarget::Instance { method })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parser::parse_source;
    use crate::parse::visitor::find_methods;

    #[test]
    fn test_classify_static_call() {
        let src = "class C { void M() { Foo.Bar(); } }";
        let tree = parse_source(src).unwrap();
        let methods = find_methods(&tree);
        assert!(!methods.is_empty());
        // Walk the AST from root to find invocation
        let root = tree.root_node();
        let mut cursor = root.walk();
        loop {
            let node = cursor.node();
            if node.kind() == "invocation_expression" {
                let target = classify_call(node, src);
                assert!(target.is_some());
                assert!(matches!(target.unwrap(), CallTarget::Static { class, method }
                    if class == "Foo" && method == "Bar"
                ));
                return;
            }
            if !cursor.goto_first_child() {
                loop {
                    if cursor.goto_next_sibling() {
                        break;
                    }
                    if !cursor.goto_parent() {
                        panic!("No invocation found");
                    }
                }
            }
        }
    }

    #[test]
    fn test_classify_instance_call() {
        let src = "class C { void M() { this.Foo(); } }";
        let tree = parse_source(src).unwrap();
        let methods = find_methods(&tree);
        assert!(!methods.is_empty());
        let root = tree.root_node();
        let mut cursor = root.walk();
        loop {
            let node = cursor.node();
            if node.kind() == "invocation_expression" {
                let target = classify_call(node, src);
                assert!(target.is_some());
                assert!(matches!(target.unwrap(), CallTarget::Instance { method }
                    if method == "Foo"
                ));
                return;
            }
            if !cursor.goto_first_child() {
                loop {
                    if cursor.goto_next_sibling() {
                        break;
                    }
                    if !cursor.goto_parent() {
                        panic!("No invocation found");
                    }
                }
            }
        }
    }
}
