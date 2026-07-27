use std::collections::{HashMap, HashSet};

use tree_sitter::Node;

use crate::resolve::types::{ClassInfo, TypeGraph};

/// A method call site: who calls whom, and what type it's called on
#[derive(Debug, Clone)]
pub struct CallSite {
    /// Which class this call belongs to
    pub caller_class: String,
    /// Which method contains this call
    pub caller_method: String,
    /// Method being called
    pub callee: String,
    /// Class that owns the callee method (resolved by type_graph)
    pub callee_class: String,
    /// The expression target (e.g., "this", "SomeClass", variable name)
    pub target_expr: String,
    /// Is this a `this.Something()` or just `Something()` call?
    pub is_self_call: bool,
    /// Is this a `new Type(args)` call?
    pub is_creation: bool,
    /// The type being constructed (if is_creation)
    pub created_type: Option<String>,
    /// Source line number (1-indexed)
    pub line: usize,
    /// Delegate method names passed as arguments (e.g. MapPost("/path", CreateItem))
    pub delegates: Vec<String>,
}

/// Call graph edges for a compilation unit
#[derive(Debug, Clone)]
pub struct CallGraph {
    /// All call sites found
    pub calls: Vec<CallSite>,
    /// Per-class: set of methods called
    pub class_callees: HashMap<String, HashSet<String>>,
    /// Per-class: set of types constructed via `new`
    pub class_creations: HashMap<String, HashSet<String>>,
    /// Per-method: set of method names called within
    pub method_calls: HashMap<String, Vec<String>>,
}

impl CallGraph {
    pub fn new() -> Self {
        CallGraph {
            calls: Vec::new(),
            class_callees: HashMap::new(),
            class_creations: HashMap::new(),
            method_calls: HashMap::new(),
        }
    }

    /// Does class `name` call any method named `method`?
    pub fn class_calls_method(&self, name: &str, method: &str) -> bool {
        self.class_callees.get(name)
            .map(|s| s.iter().any(|m| m == method))
            .unwrap_or(false)
    }

    /// Does class `name` construct type `ty` via `new`?
    pub fn class_creates_type(&self, name: &str, ty: &str) -> bool {
        self.class_creations.get(name)
            .map(|s| s.contains(ty))
            .unwrap_or(false)
    }

    /// Get the number of `new` expressions in a class that return a given type
    pub fn creation_count(&self, class: &str, ty: &str) -> usize {
        self.calls.iter()
            .filter(|c| c.caller_class == class && c.is_creation
                && c.created_type.as_deref() == Some(ty))
            .count()
    }

    /// Get all types created via `new` in a class
    pub fn created_types(&self, class: &str) -> Vec<&str> {
        self.class_creations.get(class)
            .map(|s| s.iter().map(|s| s.as_str()).collect())
            .unwrap_or_default()
    }
}

/// Build CallGraph from AST
pub struct CallGraphBuilder;

impl CallGraphBuilder {
    pub fn build(root: Node, source: &str, type_graph: &TypeGraph) -> CallGraph {
        let mut cg = CallGraph::new();
        let mut cursor = root.walk();
        let mut depth = 0;
        let mut current_class = String::new();
        let mut current_method = String::new();

        loop {
            let node = cursor.node();
            match node.kind() {
                "class_declaration" => {
                    if let Some(name) = node.child_by_field_name("name")
                        .and_then(|n| n.utf8_text(source.as_bytes()).ok())
                    {
                        current_class = name.to_string();
                    }
                }
                "method_declaration" | "constructor_declaration" => {
                    if let Some(name) = node.child_by_field_name("name")
                        .and_then(|n| n.utf8_text(source.as_bytes()).ok())
                    {
                        current_method = name.to_string();
                    }
                }
                "invocation_expression" => {
                    if !current_class.is_empty() && !current_method.is_empty() {
                        if let Some(site) = Self::extract_call(node, source, &current_class, &current_method, type_graph) {
                            cg.calls.push(site.clone());
                            cg.class_callees.entry(current_class.clone())
                                .or_default()
                                .insert(site.callee.clone());
                            cg.method_calls.entry(format!("{}.{}", current_class, current_method))
                                .or_default()
                                .push(site.callee.clone());
                        }
                    }
                }


                "object_creation_expression" => {
                    if !current_class.is_empty() {
                        for i in 0..node.child_count() {
                            let child = node.child(i).unwrap();
                            if child.kind() == "identifier" {
                                if let Ok(ty) = child.utf8_text(source.as_bytes()) {
                                    cg.class_creations.entry(current_class.clone())
                                        .or_default()
                                        .insert(ty.to_string());
                                }
                            }
                        }
                    }
                }
                _ => {}
            }

            if depth < 500 && cursor.goto_first_child() {
                depth += 1;
                continue;
            }

            loop {
                if cursor.goto_next_sibling() {
                    break;
                }
                if !cursor.goto_parent() {
                    return cg;
                }
                depth -= 1;
                if depth == 0 {
                    current_class.clear();
                }
                let parent = cursor.node();
                if parent.kind() == "class_declaration" {
                    current_class.clear();
                } else if parent.kind() == "method_declaration" || parent.kind() == "constructor_declaration" {
                    current_method.clear();
                }
            }
        }
    }

    fn caller_class_info<'a>(type_graph: &'a TypeGraph, caller_class: &str) -> Option<&'a ClassInfo> {
        type_graph.classes.get(caller_class)
    }

    fn extract_call(node: Node, source: &str, caller_class: &str, caller_method: &str, type_graph: &TypeGraph) -> Option<CallSite> {
        let mut callee = String::new();
        let mut target_expr = String::new();
        let mut is_self_call = false;
        let mut is_creation = false;
        let mut created_type = None;

        for i in 0..node.child_count() {
            let child = node.child(i).unwrap();
            let fname = node.field_name_for_child(i as u32);
            match fname {
                Some("function") | Some("name") => {
                    if child.kind() == "member_access_expression" {
                        // Normal .Foo() call
                        if let Some(name_node) = child.child_by_field_name("name") {
                            if let Ok(text) = name_node.utf8_text(source.as_bytes()) {
                                callee = text.to_string();
                            }
                        }
                        if let Some(expr_node) = child.child_by_field_name("expression") {
                            if let Ok(text) = expr_node.utf8_text(source.as_bytes()) {
                                target_expr = text.to_string();
                            }
                            if expr_node.kind() == "this" || expr_node.kind() == "this_expression" {
                                is_self_call = true;
                            }
                        }
                    } else if child.kind() == "conditional_access_expression" {
                        // ?.Foo() call — first child is target, member_binding_expression has callee
                        let mut c_cursor = child.walk();
                        for inner in child.children(&mut c_cursor) {
                            if inner.kind() == "member_binding_expression" {
                                let mut m_cursor = inner.walk();
                                for gc in inner.children(&mut m_cursor) {
                                    if gc.kind() == "identifier" {
                                        if let Ok(text) = gc.utf8_text(source.as_bytes()) {
                                            callee = text.to_string();
                                        }
                                    }
                                }
                            } else if inner.kind() == "identifier" || inner.kind() == "this_expression" {
                                // target expression (first child before ?)
                                if let Ok(text) = inner.utf8_text(source.as_bytes()) {
                                    target_expr = text.to_string();
                                }
                                if inner.kind() == "this_expression" {
                                    is_self_call = true;
                                }
                            }
                        }
                    } else {
                        // Implicit call: Foo()
                        if let Ok(text) = child.utf8_text(source.as_bytes()) {
                            callee = text.to_string();
                        }
                    }
                }
                Some("expression") => {
                    if let Ok(text) = child.utf8_text(source.as_bytes()) {
                        target_expr = text.to_string();
                    }
                    if child.kind() == "this_expression" {
                        is_self_call = true;
                    }
                    if child.kind() == "object_creation_expression" {
                        is_creation = true;
                        for j in 0..child.child_count() {
                            let gc = child.child(j).unwrap();
                            if gc.kind() == "identifier" {
                                if let Ok(text) = gc.utf8_text(source.as_bytes()) {
                                    created_type = Some(text.to_string());
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        // Extract delegate methods from argument_list:
        // e.g. MapPost("/path", CreateItem) → "CreateItem" is a delegate
        let caller_class_info = Self::caller_class_info(type_graph, caller_class);
        let mut delegates = Vec::new();
        for i in 0..node.child_count() {
            let child = node.child(i).unwrap();
            if child.kind() == "argument_list" {
                let mut a_cursor = child.walk();
                for arg in child.children(&mut a_cursor) {
                    if arg.kind() == "argument" {
                        for j in 0..arg.child_count() {
                            if let Some(gc) = arg.child(j) {
                                if gc.kind() == "identifier" {
                                    if let Ok(text) = gc.utf8_text(source.as_bytes()) {
                                        if caller_class_info.map_or(false, |ci| ci.methods.iter().any(|m| m.method == text)) {
                                            delegates.push(text.to_string());
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        let callee_class = if is_self_call || target_expr.is_empty() {
            // this.Foo() or Foo() (implicit this) → caller_class
            if type_graph.classes.get(caller_class)
                .map(|c| c.methods.iter().any(|m| m.method == callee))
                .unwrap_or(false)
            {
                caller_class.to_string()
            } else {
                // implicit call but method not in caller — search all classes
                type_graph.classes.iter()
                    .find(|(_, ci)| ci.methods.iter().any(|m| m.method == callee))
                    .map(|(name, _)| name.clone())
                    .unwrap_or_default()
            }
        } else if type_graph.classes.contains_key(&target_expr) {
            // ClassName.Foo()
            target_expr.clone()
        } else {
            String::new()
        };

        if callee.is_empty() {
            None
        } else {
            Some(CallSite {
                caller_class: caller_class.to_string(),
                caller_method: caller_method.to_string(),
                callee,
                callee_class,
                target_expr,
                is_self_call,
                is_creation,
                created_type,
                line: node.start_position().row + 1,
                delegates,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parser::parse_source;
    use crate::resolve::symbols::SymbolTable;

    #[test]
    fn test_callgraph_empty() {
        let cg = CallGraph::new();
        assert!(cg.calls.is_empty());
    }

    #[test]
    fn test_callgraph_self_call() {
        let src = "class C { void A() { this.B(); } void B() { } }";
        let tree = parse_source(src).unwrap();
        let st = SymbolTable::from_ast(tree.root_node(), src).unwrap();
        let cg = CallGraphBuilder::build(tree.root_node(), src, &st.type_graph);
        assert!(cg.class_calls_method("C", "B"));
        assert_eq!(cg.calls.len(), 1);
        assert!(cg.calls[0].is_self_call);
    }

    #[test]
    fn test_callgraph_creation() {
        let src = "class C { void M() { var x = new Foo(); } }";
        let tree = parse_source(src).unwrap();
        let st = SymbolTable::from_ast(tree.root_node(), src).unwrap();
        let cg = CallGraphBuilder::build(tree.root_node(), src, &st.type_graph);
        assert!(cg.class_creates_type("C", "Foo"));
        assert_eq!(cg.created_types("C"), vec!["Foo"]);
    }

    #[test]
    fn test_callgraph_static_call() {
        let src = "class C { void M() { Singleton.GetInstance(); } }";
        let tree = parse_source(src).unwrap();
        let st = SymbolTable::from_ast(tree.root_node(), src).unwrap();
        let cg = CallGraphBuilder::build(tree.root_node(), src, &st.type_graph);
        assert!(cg.class_calls_method("C", "GetInstance"));
    }

    #[test]
    fn test_callgraph_method_calls_per_method() {
        let src = "class C { void A() { B(); C(); } void B() { } void C() { } }";
        let tree = parse_source(src).unwrap();
        let st = SymbolTable::from_ast(tree.root_node(), src).unwrap();
        let cg = CallGraphBuilder::build(tree.root_node(), src, &st.type_graph);
        let key = "C.A";
        let calls = cg.method_calls.get(key).cloned().unwrap_or_default();
        assert!(calls.contains(&"B".to_string()));
        assert!(calls.contains(&"C".to_string()));
    }

    #[test]
    fn test_null_conditional_call() {
        let src = "class C { void M() { x?.Foo(1); } }";
        let tree = parse_source(src).unwrap();
        let st = SymbolTable::from_ast(tree.root_node(), src).unwrap();
        let cg = CallGraphBuilder::build(tree.root_node(), src, &st.type_graph);
        assert!(cg.class_calls_method("C", "Foo"));
        let site = cg.calls.iter().find(|c| c.callee == "Foo").unwrap();
        assert_eq!(site.callee_class, "");
        assert_eq!(site.target_expr, "x");
    }

    #[test]
    fn test_self_call_callee_class() {
        let src = "class C { void A() { this.B(); } void B() { } }";
        let tree = parse_source(src).unwrap();
        let st = SymbolTable::from_ast(tree.root_node(), src).unwrap();
        let cg = CallGraphBuilder::build(tree.root_node(), src, &st.type_graph);
        let site = cg.calls.iter().find(|c| c.callee == "B").unwrap();
        assert_eq!(site.callee_class, "C");
    }

    #[test]
    fn test_static_call_callee_class() {
        let src = "class C { void M() { Singleton.GetInstance(); } } class Singleton { public static Singleton GetInstance() { return null; } }";
        let tree = parse_source(src).unwrap();
        let st = SymbolTable::from_ast(tree.root_node(), src).unwrap();
        let cg = CallGraphBuilder::build(tree.root_node(), src, &st.type_graph);
        let site = cg.calls.iter().find(|c| c.callee == "GetInstance").unwrap();
        assert_eq!(site.callee_class, "Singleton");
    }

    #[test]
    fn test_implicit_call_callee_class() {
        let src = "class C { void A() { B(); } void B() { } }";
        let tree = parse_source(src).unwrap();
        let st = SymbolTable::from_ast(tree.root_node(), src).unwrap();
        let cg = CallGraphBuilder::build(tree.root_node(), src, &st.type_graph);
        let site = cg.calls.iter().find(|c| c.callee == "B").unwrap();
        assert_eq!(site.callee_class, "C");
    }


}
