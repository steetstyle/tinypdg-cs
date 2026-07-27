//! Reflection call resolution.
//!
//! Kategori 6: Reflection
//! - Static string/nameof: `typeof(Foo).GetMethod("Bar")` [70-80%]
//! - Dynamic string: `GetMethod(variableName)` [<10%]
//! - `Activator.CreateInstance(typeof(Foo))` [70%]

use crate::resolve::types::{
    CallSite, CallTarget, Confidence, MethodDescriptor, ReflectionPattern, TypeGraph,
};

/// Scan AST for reflection patterns
pub fn scan_reflection(
    _source: &str,
    _type_graph: &TypeGraph,
) -> Vec<ReflectionPattern> {
    // Stub: real implementation traverses AST for typeof+GetMethod patterns
    Vec::new()
}

/// Resolve a reflection-based call
pub fn resolve_reflection(
    target: &CallTarget,
    caller: &str,
    _type_graph: &TypeGraph,
    _patterns: &[ReflectionPattern],
) -> Vec<CallSite> {
    let (type_name, method_name) = match target {
        CallTarget::Static { class, method } => {
            // Heuristic: if method has "GetMethod", "Invoke", "CreateInstance"
            match method.as_str() {
                "GetMethod" | "GetProperty" | "GetField" => (class.as_str(), None),
                "CreateInstance" => (class.as_str(), None),
                "Invoke" => return Vec::new(), // needs deeper analysis
                _ => return Vec::new(),
            }
        }
        _ => return Vec::new(),
    };

    let confidence = if is_compile_time_string(method_name) {
        Confidence::Reflection
    } else {
        Confidence::DynamicString
    };

    let resolved = if !type_name.is_empty() {
        vec![MethodDescriptor {
            class: type_name.to_string(),
            method: method_name.unwrap_or_default().to_string(),
            ..Default::default()
        }]
    } else {
        vec![]
    };

    vec![CallSite {
        caller: caller.to_string(),
        target: target.clone(),
        confidence,
        resolved,
    }]
}

/// Check if a method name is a compile-time constant (string literal or nameof)
fn is_compile_time_string(name: Option<&str>) -> bool {
    match name {
        Some(n) if n.starts_with('"') && n.ends_with('"') => true,
        Some("nameof") => true,
        _ => false,
    }
}

/// Classify an AST node as a reflection call
pub fn classify_reflection(node: tree_sitter::Node, source: &str) -> Option<CallTarget> {
    if node.kind() != "invocation_expression" {
        return None;
    }
    let func = node.child_by_field_name("function")?;

    // Detect `typeof(Foo).GetMethod("Bar")`
    if let Some(member) = check_typeof_chain(func, source) {
        return Some(member);
    }

    // Detect `Activator.CreateInstance(typeof(Foo))`
    if func.kind() == "member_access_expression" {
        let expr = func.child_by_field_name("expression")
            .and_then(|n| n.utf8_text(source.as_bytes()).ok());
        let name = func.child_by_field_name("name")
            .and_then(|n| n.utf8_text(source.as_bytes()).ok());
        if let (Some("Activator"), Some("CreateInstance")) = (expr, name) {
            // Extract type from first argument (typeof(Foo) or "TypeName")
            if let Some(arg) = node.child_by_field_name("arguments")
                .and_then(|a| a.child(0))
            {
                if arg.kind() == "typeof_expression" {
                    let type_node = arg.child_by_field_name("type")?;
                    let type_name = type_node.utf8_text(source.as_bytes()).ok()?;
                    return Some(CallTarget::Static {
                        class: type_name.to_string(),
                        method: "CreateInstance".into(),
                    });
                }
            }
        }
    }

    None
}

fn check_typeof_chain(node: tree_sitter::Node, source: &str) -> Option<CallTarget> {
    if node.kind() != "member_access_expression" {
        return None;
    }
    let name = node.child_by_field_name("name")
        .and_then(|n| n.utf8_text(source.as_bytes()).ok())?;

    if name != "GetMethod" && name != "GetProperty" {
        return None;
    }

    let expr = node.child_by_field_name("expression")?;
    if expr.kind() != "typeof_expression" {
        return None;
    }

    let type_node = expr.child_by_field_name("type")?;
    let type_name = type_node.utf8_text(source.as_bytes()).ok()?;

    Some(CallTarget::Static {
        class: type_name.to_string(),
        method: name.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reflection_empty_patterns() {
        let tg = TypeGraph::new();
        let patterns = scan_reflection("", &tg);
        assert!(patterns.is_empty());
    }

    #[test]
    fn test_reflection_getmethod_resolves() {
        let tg = TypeGraph::new();
        let patterns = vec![];
        let target = CallTarget::Static {
            class: "Foo".into(),
            method: "GetMethod".into(),
        };
        let sites = resolve_reflection(&target, "Test", &tg, &patterns);
        assert_eq!(sites.len(), 1);
        // Without pattern info, we default to DynamicString
        assert_eq!(sites[0].confidence, Confidence::DynamicString);
    }

    #[test]
    fn test_reflection_non_reflection_target() {
        let tg = TypeGraph::new();
        let patterns = vec![];
        let target = CallTarget::Instance {
            method: "NormalMethod".into(),
        };
        let sites = resolve_reflection(&target, "Test", &tg, &patterns);
        assert!(sites.is_empty());
    }

    #[test]
    fn test_classify_typeof_getmethod() {
        let src = "class C { void M() { typeof(Foo).GetMethod(\"Bar\"); } }";
        let mut parser = tree_sitter::Parser::new();
        parser.set_language(&tree_sitter_c_sharp::LANGUAGE.into()).unwrap();
        let tree = parser.parse(src, None).unwrap();
        #[allow(deprecated)]
        let root = tree.root_node();
        let mut cursor = root.walk();
        let mut finished = false;
        while !finished {
            let node = cursor.node();
            if node.kind() == "invocation_expression" {
                let target = classify_reflection(node, src);
                if let Some(CallTarget::Static { class, method }) = target {
                    assert_eq!(class, "Foo");
                    assert_eq!(method, "GetMethod");
                    return;
                }
            }
            if !cursor.goto_first_child() {
                loop {
                    if cursor.goto_next_sibling() { break; }
                    if !cursor.goto_parent() { finished = true; break; }
                }
            }
        }
        panic!("Expected to find typeof(Foo).GetMethod(\"Bar\") pattern");
    }
}
