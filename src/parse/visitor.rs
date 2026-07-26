//! AST traversal helpers.
//!
//! tree-sitter CST üzerinde gezinmek, statement/expression tiplerini
//! tanımak ve anlamlı düğümleri toplamak için yardımcı fonksiyonlar.

use tree_sitter::{Node, Tree};

/// Maksimum AST/CST derinliği (stack overflow koruması)
const MAX_AST_DEPTH: usize = 500;

/// C# statement tipleri
#[derive(Debug, Clone, PartialEq)]
pub enum StmtKind {
    If,
    Else,
    For,
    ForEach,
    While,
    DoWhile,
    Switch,
    Try,
    Catch,
    Finally,
    Using,
    Lock,
    Return,
    Throw,
    Break,
    Continue,
    Expression,
    Declaration,
    Block,
    Unsafe,
    Fixed,
    Checked,
    Unchecked,
    YieldReturn,
    YieldBreak,
    Goto,
    Labeled,
    LocalFunction,
    Unknown(String),
}

impl StmtKind {
    pub fn from_node(node: &Node) -> Self {
        match node.kind() {
            "if_statement" => StmtKind::If,
            "else_clause" => StmtKind::Else,
            "for_statement" => StmtKind::For,
            "for_each_statement" => StmtKind::ForEach,
            "while_statement" => StmtKind::While,
            "do_statement" => StmtKind::DoWhile,
            "switch_statement" => StmtKind::Switch,
            "try_statement" => StmtKind::Try,
            "catch_clause" => StmtKind::Catch,
            "finally_clause" => StmtKind::Finally,
            "using_statement" => StmtKind::Using,
            "lock_statement" => StmtKind::Lock,
            "return_statement" => StmtKind::Return,
            "throw_statement" => StmtKind::Throw,
            "break_statement" => StmtKind::Break,
            "continue_statement" => StmtKind::Continue,
            "expression_statement" => StmtKind::Expression,
            "local_declaration_statement" => StmtKind::Declaration,
            "block" => StmtKind::Block,
            other => StmtKind::Unknown(other.to_string()),
        }
    }
}

/// AST'deki tüm fonksiyon/method bildirimlerini toplar.
/// Iterative traversal, MAX_AST_DEPTH ile stack overflow korumalı.
pub fn find_methods(tree: &Tree) -> Vec<Node<'_>> {
    let mut methods = Vec::new();
    let mut cursor = tree.walk();
    let mut depth: usize = 0;

    loop {
        let node = cursor.node();
        if matches!(
            node.kind(),
            "method_declaration" | "constructor_declaration" | "local_function_statement"
        ) {
            methods.push(node);
        }

        if depth < MAX_AST_DEPTH && cursor.goto_first_child() {
            depth += 1;
            continue;
        }

        loop {
            if cursor.goto_next_sibling() {
                break;
            }
            if !cursor.goto_parent() {
                return methods;
            }
            depth -= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parser::parse_source;

    #[test]
    fn test_find_methods() {
        let source = "class Foo { void A() { } int B() => 1; }";
        let tree = parse_source(source).unwrap();
        let methods = find_methods(&tree);
        assert_eq!(methods.len(), 2);
    }
}