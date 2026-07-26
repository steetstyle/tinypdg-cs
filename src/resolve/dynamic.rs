//! C# dynamic / DLR resolution.
//!
//! Kategori 6b: C# `dynamic` keyword
//! - No static resolution possible [0% static, 100% runtime]
//! - We mark call sites as dynamic for tracking purposes

use crate::resolve::types::{
    CallSite, CallTarget, Confidence,
};

/// Detect if a call target uses C# `dynamic`
pub fn is_dynamic_call(node: tree_sitter::Node, source: &str) -> bool {
    if node.kind() != "invocation_expression" {
        return false;
    }
    let func = node.child_by_field_name("function");
    if let Some(f) = func {
        // Check if any identifier in the expression is `dynamic`
        let mut cursor = f.walk();
        loop {
            let n = cursor.node();
            if n.kind() == "identifier" {
                if let Ok(text) = n.utf8_text(source.as_bytes()) {
                    // Lowercase starts often indicate local variables
                    // but we can't know statically if they're `dynamic`
                    if text == "dynamic" {
                        return true;
                    }
                }
            }
            if !cursor.goto_first_child() {
                loop {
                    if cursor.goto_next_sibling() { break; }
                    if !cursor.goto_parent() { break; }
                }
            }
        }
    }
    false
}

/// Resolve a dynamic call (always marked as Unknown)
pub fn resolve_dynamic(
    target: &CallTarget,
    caller: &str,
) -> Vec<CallSite> {
    vec![CallSite {
        caller: caller.to_string(),
        target: target.clone(),
        confidence: Confidence::Unknown,
        resolved: vec![],
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dynamic_call_returns_unknown() {
        let target = CallTarget::Instance {
            method: "Foo".into(),
        };
        let sites = resolve_dynamic(&target, "Test");
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].confidence, Confidence::Unknown);
    }
}
