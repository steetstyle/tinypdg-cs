//! Symbol table: builds type hierarchy from AST

use std::collections::HashMap;

use anyhow::Result;
use tree_sitter::Node;

use crate::resolve::types::{ClassInfo, FieldDescriptor, InterfaceInfo, MethodDescriptor, TypeGraph};

/// Extract type information from AST and build a TypeGraph
pub struct SymbolTable {
    pub type_graph: TypeGraph,
    base_types: HashMap<String, Vec<String>>,
}

impl SymbolTable {
    pub fn new() -> Self {
        SymbolTable {
            type_graph: TypeGraph::new(),
            base_types: HashMap::new(),
        }
    }

    /// Walk the AST root and register all types
    pub fn from_ast(root: Node, source: &str) -> Result<Self> {
        let mut st = SymbolTable::new();
        st.visit_node(root, source);
        st.resolve_inheritance();
        Ok(st)
    }

    fn visit_node(&mut self, node: Node, source: &str) {
        match node.kind() {
            "class_declaration" => self.register_class(node, source, false),
            "interface_declaration" => self.register_interface(node, source),
            "struct_declaration" => self.register_class(node, source, false),
            "record_declaration" => self.register_class(node, source, false),
            _ => {}
        }

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.visit_node(child, source);
        }
    }

    fn register_class(&mut self, node: Node, source: &str, _is_struct: bool) {
        let name = node.child_by_field_name("name")
            .and_then(|n| n.utf8_text(source.as_bytes()).ok())
            .map(|s| s.to_string());

        let name = match name {
            Some(n) => n,
            None => return,
        };

        let mut base_types = Vec::new();
        for i in 0..node.child_count() {
            let child = node.child(i).unwrap();
            if child.kind() == "base_list" {
                let mut cursor = child.walk();
                for grandchild in child.children(&mut cursor) {
                    if grandchild.kind() == "identifier" {
                        if let Ok(text) = grandchild.utf8_text(source.as_bytes()) {
                            base_types.push(text.to_string());
                        }
                    } else if grandchild.kind() == "generic_name" {
                        // Extract the base type name (before generic args)
                        for j in 0..grandchild.child_count() {
                            let inner = grandchild.child(j).unwrap();
                            if inner.kind() == "identifier" {
                                if let Ok(text) = inner.utf8_text(source.as_bytes()) {
                                    base_types.push(text.to_string());
                                }
                                break;
                            }
                        }
                    }
                }
                break;
            }
        }

        let mut methods = Vec::new();
        self.collect_methods(node, source, &mut methods);

        // C# 12 primary constructor: class Foo(int x, string y) { ... }
        // tree-sitter produces a `parameter_list` child on the class_declaration node
        if has_parameter_list(node) {
            methods.push(MethodDescriptor {
                class: String::new(),
                method: ".ctor".into(),
                signature: format!(".ctor ({})", extract_parameter_types(node, source)),
                is_static: false,
                is_virtual: false,
                is_abstract: false,
                file: String::new(),
                line_start: node.start_position().row + 1,
                line_end: node.start_position().row + 1,
            });
        }

        let mut fields = Vec::new();
        self.collect_fields(node, source, &mut fields);

        let is_abstract = has_class_modifier(node, source, "abstract");
        let is_sealed = has_class_modifier(node, source, "sealed");
        let is_static = has_class_modifier(node, source, "static");

        let ci = ClassInfo {
            name: name.clone(),
            base_class: None,
            interfaces: Vec::new(),
            methods,
            fields,
            is_abstract,
            is_sealed,
            is_static,
        };

        // Merge if class already exists (same name from another file/namespace)
        if let Some(existing) = self.type_graph.classes.get_mut(&name) {
            existing.methods.extend(ci.methods);
            existing.fields.extend(ci.fields);
            if ci.base_class.is_some() { existing.base_class = ci.base_class.clone(); }
            existing.interfaces.extend(ci.interfaces.clone());
            existing.is_abstract = existing.is_abstract || ci.is_abstract;
            existing.is_sealed = existing.is_sealed || ci.is_sealed;
            existing.is_static = existing.is_static || ci.is_static;
        } else {
            self.type_graph.classes.insert(name.clone(), ci);
        }
        self.base_types.insert(name, base_types);
    }

    fn collect_methods(&self, node: Node, source: &str, methods: &mut Vec<MethodDescriptor>) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "method_declaration" => {
                    if let Some(md) = extract_method(child, source) {
                        methods.push(md);
                    }
                }
                "constructor_declaration" => {
                    if let Some(md) = extract_method(child, source) {
                        methods.push(md);
                    }
                }
                _ => {
                    self.collect_methods(child, source, methods);
                }
            }
        }
    }

    fn collect_fields(&self, node: Node, source: &str, fields: &mut Vec<FieldDescriptor>) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "field_declaration" {
                let mut is_static = false;
                let mut is_readonly = false;
                let mut fcursor = child.walk();
                for modifier in child.children(&mut fcursor) {
                    let text = modifier.utf8_text(source.as_bytes()).unwrap_or("");
                    match text {
                        "static" => is_static = true,
                        "readonly" => is_readonly = true,
                        _ => {}
                    }
                }
                let field_type = child.child_by_field_name("type")
                    .and_then(|t| t.utf8_text(source.as_bytes()).ok())
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                let decl = child.child_by_field_name("declarator")
                    .or_else(|| {
                        // variable_declaration may have declarator list
                        child.child_by_field_name("declarator_list")
                            .and_then(|dl| dl.child(0))
                    });
                let field_name = decl
                    .and_then(|d| d.utf8_text(source.as_bytes()).ok())
                    .map(|s| s.to_string())
                    .unwrap_or_default();
                if !field_name.is_empty() {
                    fields.push(FieldDescriptor {
                        name: field_name,
                        field_type: field_type.clone(),
                        is_static,
                        is_readonly,
                    });
                }
            }
        }
    }

    fn register_interface(&mut self, node: Node, source: &str) {
        let name = node.child_by_field_name("name")
            .and_then(|n| n.utf8_text(source.as_bytes()).ok())
            .map(|s| s.to_string());

        let name = match name {
            Some(n) => n,
            None => return,
        };

        let mut methods = Vec::new();
        self.collect_methods(node, source, &mut methods);

        let ii = InterfaceInfo { name: name.clone(), methods };
        self.type_graph.interfaces.insert(name, ii);
    }

    fn resolve_inheritance(&mut self) {
        let names: Vec<String> = self.base_types.keys().cloned().collect();
        for class_name in names {
            let bt: Vec<String> = self.base_types.remove(&class_name).unwrap_or_default();
            let mut new_base: Option<String> = None;
            let mut new_ifaces: Vec<String> = Vec::new();
            for type_name in &bt {
                if type_name == &class_name { continue; }
                if self.type_graph.interfaces.contains_key(type_name) {
                    new_ifaces.push(type_name.clone());
                } else if self.type_graph.classes.contains_key(type_name) {
                    new_base = Some(type_name.clone());
                } else {
                    // Unknown base type — likely an external interface
                    new_ifaces.push(type_name.clone());
                }
            }
            if let Some(ci) = self.type_graph.classes.get_mut(&class_name) {
                ci.base_class = new_base;
                ci.interfaces = new_ifaces;
            }
        }
    }
}

fn has_parameter_list(node: tree_sitter::Node) -> bool {
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            if child.kind() == "parameter_list" {
                return true;
            }
        }
    }
    false
}

fn extract_parameter_types(node: tree_sitter::Node, source: &str) -> String {
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            if child.kind() == "parameter_list" {
                if let Ok(text) = child.utf8_text(source.as_bytes()) {
                    return text.to_string();
                }
            }
        }
    }
    String::new()
}

fn has_class_modifier(node: Node, source: &str, modifier: &str) -> bool {
    for i in 0..node.child_count() {
        let child = node.child(i).unwrap();
        if child.kind() == "modifier" {
            if let Ok(text) = child.utf8_text(source.as_bytes()) {
                if text == modifier {
                    return true;
                }
            }
        }
    }
    false
}

fn extract_method(node: Node, source: &str) -> Option<MethodDescriptor> {
    let name = node.child_by_field_name("name")
        .and_then(|n| n.utf8_text(source.as_bytes()).ok())
        .map(|s| s.to_string())?;

    let return_type = node.child_by_field_name("returns")
        .or_else(|| node.child_by_field_name("return_type"))
        .and_then(|n| n.utf8_text(source.as_bytes()).ok())
        .unwrap_or("void")
        .to_string();

    let mut is_static = false;
    let mut is_virtual = false;
    let mut is_abstract = false;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let text = child.utf8_text(source.as_bytes()).unwrap_or("");
        match text {
            "static" => is_static = true,
            "virtual" => is_virtual = true,
            "abstract" => is_abstract = true,
            _ => {}
        }
    }

    // Extract parameter types from the parameter list
    let param_types = node.child_by_field_name("parameters")
        .map(|params_node| {
            let mut ptypes = Vec::new();
            let mut pcursor = params_node.walk();
            for param in params_node.children(&mut pcursor) {
                if param.kind() == "parameter" {
                    if let Some(ptype) = param.child_by_field_name("type") {
                        if let Ok(text) = ptype.utf8_text(source.as_bytes()) {
                            ptypes.push(text.to_string());
                        }
                    }
                }
            }
            ptypes
        })
        .unwrap_or_default();

    let param_str = if param_types.is_empty() {
        String::new()
    } else {
        format!("({})", param_types.join(", "))
    };
    let signature = format!("{} {}{}", return_type, name, param_str);
    let start_pos = node.start_position();
    let end_pos = node.end_position();

    Some(MethodDescriptor {
        class: String::new(),
        method: name,
        signature,
        is_static,
        is_virtual,
        is_abstract,
        file: String::new(),
        line_start: start_pos.row + 1,
        line_end: end_pos.row + 1,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parser::parse_source;

    #[test]
    fn test_symbol_table_empty() {
        let st = SymbolTable::new();
        assert_eq!(st.type_graph.classes.len(), 0);
    }

    #[test]
    fn test_symbol_table_from_simple_class() {
        let src = "class Foo { void Bar() { } }";
        let tree = parse_source(src).unwrap();
        let root = tree.root_node();
        let st = SymbolTable::from_ast(root, src).unwrap();
        assert!(st.type_graph.classes.contains_key("Foo"));
        if let Some(cls) = st.type_graph.classes.get("Foo") {
            assert_eq!(cls.methods.len(), 1);
            assert_eq!(cls.methods[0].method, "Bar");
        }
    }

    #[test]
    fn test_symbol_table_detects_virtual() {
        let src = "class Base { virtual void Foo() { } }";
        let tree = parse_source(src).unwrap();
        let root = tree.root_node();
        let st = SymbolTable::from_ast(root, src).unwrap();
        if let Some(cls) = st.type_graph.classes.get("Base") {
            assert!(cls.methods[0].is_virtual);
        }
    }

    #[test]
    fn test_symbol_table_interface_implementation() {
        let src = "interface IFoo { void Bar(); } class Foo : IFoo { public void Bar() { } }";
        let tree = parse_source(src).unwrap();
        let root = tree.root_node();
        let st = SymbolTable::from_ast(root, src).unwrap();
        assert!(st.type_graph.interfaces.contains_key("IFoo"));
        if let Some(cls) = st.type_graph.classes.get("Foo") {
            assert_eq!(cls.interfaces, vec!["IFoo"]);
        }
    }

    #[test]
    fn test_symbol_table_multiple_interfaces() {
        let src = "interface IA { void A(); } interface IB { void B(); } class Foo : IA, IB { public void A() { } public void B() { } }";
        let tree = parse_source(src).unwrap();
        let root = tree.root_node();
        let st = SymbolTable::from_ast(root, src).unwrap();
        if let Some(cls) = st.type_graph.classes.get("Foo") {
            assert!(cls.interfaces.contains(&"IA".to_string()));
            assert!(cls.interfaces.contains(&"IB".to_string()));
        }
    }

    #[test]
    fn test_symbol_table_base_class_and_interfaces() {
        let src = "interface IFoo { void Bar(); } class Base { } class Derived : Base, IFoo { public void Bar() { } }";
        let tree = parse_source(src).unwrap();
        let root = tree.root_node();
        let st = SymbolTable::from_ast(root, src).unwrap();
        if let Some(cls) = st.type_graph.classes.get("Derived") {
            assert_eq!(cls.base_class, Some("Base".to_string()));
            assert_eq!(cls.interfaces, vec!["IFoo"]);
        }
    }
}