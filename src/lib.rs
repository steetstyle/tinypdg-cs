pub mod analysis;
pub mod cfg;
pub mod cli;
pub mod detect;
pub mod github;
pub mod graph;
pub mod hammock;
pub mod parse;
pub mod pdg;
pub mod resolve;
pub mod route;
pub mod source;
pub mod traverse;

#[cfg(feature = "mcp")]
pub mod mcp;

pub use cfg::builder as cfg_builder;
pub use graph::dot;
pub use parse::parser;
pub use pdg::pdg_builder;
