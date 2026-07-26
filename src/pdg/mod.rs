//! # pdg
//!
//! Program Dependence Graph (PDG) inşası.
//! CFG üzerinde control dependence + data dependence.
//!
//! ## Modüller (Faz 2a)
//! - `control_deps.rs` — Post-dominator tree → control dependence edges
//! - `data_deps.rs` — Reaching definitions → data dependence edges
//! - `pdg_builder.rs` — CFG + deps → PDG

pub mod control_deps;
pub mod data_deps;
pub mod pdg_builder;
