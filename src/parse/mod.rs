//! # parse
//!
//! tree-sitter C# grammar ile kaynak kodu AST'ye çevirir.
//! AST'den anlamlı bir iç temsil (IR) oluşturur.
//!
//! ## Aşamalar
//! 1. `parser::parse()` → tree-sitter CST
//! 2. `visitor::walk()` → AST traversal, sembol toplama
//!
//! ## Kullanım
//! ```rust,no_run
//! use tiny_pdg_cs::parse::parser::parse_file;
//! let ast = parse_file("tests/fixtures/control_flow/if_else.cs").expect("parse failed");
//! ```

pub mod parser;
pub mod visitor;