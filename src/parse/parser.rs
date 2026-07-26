//! tree-sitter C# parser wrapper.
//!
//! Kaynak kodu tree-sitter ile parse eder, CST (Concrete Syntax Tree)
//! üzerinde gezinmek için yardımcı fonksiyonlar sağlar.

use anyhow::Result;
use tree_sitter::{Parser, Tree};

/// Bir `.cs` dosyasını tree-sitter ile parse eder.
pub fn parse_file(path: &str) -> Result<Tree> {
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_c_sharp::LANGUAGE.into())
        .map_err(|e| anyhow::anyhow!("Failed to set C# language: {}", e))?;
    let source = std::fs::read_to_string(path)?;
    let tree = parser.parse(&source, None)
        .ok_or_else(|| anyhow::anyhow!("Failed to parse {}", path))?;
    Ok(tree)
}

/// Bir string'i tree-sitter ile parse eder (testler için).
pub fn parse_source(source: &str) -> Result<Tree> {
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_c_sharp::LANGUAGE.into())
        .map_err(|e| anyhow::anyhow!("Failed to set C# language: {}", e))?;
    let tree = parser.parse(source, None)
        .ok_or_else(|| anyhow::anyhow!("Failed to parse source"))?;
    Ok(tree)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_empty_class() {
        let source = "class Foo { }";
        let tree = parse_source(source).unwrap();
        assert!(tree.root_node().child_count() > 0);
    }

    #[test]
    fn test_parse_method() {
        let source = "class Foo { void Bar() { int x = 1; } }";
        let tree = parse_source(source).unwrap();
        let root = tree.root_node();
        let class = root.child(0).unwrap();
        assert_eq!(class.kind(), "class_declaration");
    }
}