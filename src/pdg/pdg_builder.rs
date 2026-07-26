use std::fmt;

use anyhow::Result;
use petgraph::graph::DiGraph;

use crate::cfg::builder::{BasicBlock, BlockEdge, BlockKind};
use crate::pdg::control_deps::compute_control_deps;
use crate::pdg::data_deps::compute_data_deps;

#[derive(Debug, Clone, PartialEq)]
pub enum PdgEdge {
    Control,
    Data,
    Cfg(BlockEdge),
}

impl fmt::Display for PdgEdge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PdgEdge::Control => write!(f, "control"),
            PdgEdge::Data => write!(f, "data"),
            PdgEdge::Cfg(e) => write!(f, "cfg({:?})", e),
        }
    }
}

pub type PdgGraph = DiGraph<BasicBlock, PdgEdge>;

/// CFG + control dependence + data dependence → PDG
pub fn build_pdg(cfg: &DiGraph<BasicBlock, BlockEdge>) -> Result<PdgGraph> {
    let entry = cfg.node_indices()
        .find(|i| cfg[*i].kind == BlockKind::Entry)
        .expect("CFG must have an Entry node");
    let exit = cfg.node_indices()
        .find(|i| cfg[*i].kind == BlockKind::Exit)
        .expect("CFG must have an Exit node");

    // CFG düğümlerini PDG'ye kopyala
    let mut pdg = DiGraph::with_capacity(cfg.node_count(), cfg.edge_count());
    let mut node_map = Vec::new();
    for n in cfg.node_weights() {
        let idx = pdg.add_node(n.clone());
        node_map.push(idx);
    }

    // CFG edge'lerini kopyala
    for e in cfg.raw_edges() {
        let src = node_map[e.source().index()];
        let tgt = node_map[e.target().index()];
        pdg.add_edge(src, tgt, PdgEdge::Cfg(e.weight.clone()));
    }

    // Control dependence edges ekle
    let cdeps = compute_control_deps(cfg, entry, exit);
    for (from, to) in &cdeps {
        let src = node_map[from.index()];
        let tgt = node_map[to.index()];
        pdg.add_edge(src, tgt, PdgEdge::Control);
    }

    // Data dependence edges ekle
    let ddeps = compute_data_deps(cfg);
    for (from, to) in &ddeps {
        let src = node_map[from.index()];
        let tgt = node_map[to.index()];
        pdg.add_edge(src, tgt, PdgEdge::Data);
    }

    Ok(pdg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cfg::builder::build_cfg;

    #[test]
    fn test_empty_method_pdg() {
        let cfg = build_cfg("class C { void M() { } }").unwrap();
        let pdg = build_pdg(&cfg).unwrap();
        assert_eq!(pdg.node_count(), cfg.node_count());
    }

    #[test]
    fn test_if_pdg() {
        let cfg = build_cfg("class C { void M() { if (true) { foo(); } else { bar(); } } }").unwrap();
        let pdg = build_pdg(&cfg).unwrap();
        assert!(pdg.edge_count() >= cfg.edge_count());
    }

    #[test]
    fn test_pdg_has_control_edges() {
        let cfg = build_cfg("class C { void M() { if (true) { foo(); } else { bar(); } } }").unwrap();
        let pdg = build_pdg(&cfg).unwrap();
        let control_edges = pdg.raw_edges().iter().filter(|e| e.weight == PdgEdge::Control).count();
        assert!(control_edges > 0);
    }
}
