use petgraph::dot::Dot;
use petgraph::visit::EdgeRef;

use crate::cfg::builder::{BasicBlock, BlockEdge, BlockKind};
use crate::pdg::pdg_builder::PdgEdge;

pub fn cfg_to_dot(graph: &petgraph::graph::DiGraph<BasicBlock, BlockEdge>) -> String {
    format!("{}", Dot::new(graph))
}

pub fn pdg_to_dot(
    pdg: &petgraph::graph::DiGraph<BasicBlock, PdgEdge>,
    title: &str,
) -> String {
    let mut dot = String::from("digraph PDG {\n  rankdir=TB;\n  node [shape=box style=rounded];\n\n");
    if !title.is_empty() {
        dot.push_str(&format!("  label=\"{}\";\n  labelloc=t;\n\n", title));
    }
    dot.push_str(&pdg_nodes_edges_to_dot(pdg, ""));
    dot.push_str("}\n");
    dot
}

/// Return only the nodes and edges statements (no digraph wrapper).
pub fn pdg_nodes_edges_to_dot(
    pdg: &petgraph::graph::DiGraph<BasicBlock, PdgEdge>,
    indent: &str,
) -> String {
    let mut dot = String::new();
    pdg_write_nodes_edges(pdg, &mut dot, indent);
    dot
}

/// Write PDG nodes and edges into an existing DOT string, with an optional
/// per-line indent prefix (e.g. `"  "` when embedding inside a subgraph).
pub fn pdg_write_nodes_edges(
    pdg: &petgraph::graph::DiGraph<BasicBlock, PdgEdge>,
    dot: &mut String,
    indent: &str,
) {
    for idx in pdg.node_indices() {
        let block = &pdg[idx];
        let label = format!("L{}-{}:{}", block.start_line, block.end_line, block.kind)
            .replace('"', "'");
        let color = match block.kind {
            BlockKind::Entry => "green",
            BlockKind::Exit => "red",
            _ => "lightblue",
        };
        dot.push_str(&format!(
            "{indent}n{} [label=\"{label}\" style=filled fillcolor={color}];\n",
            idx.index(),
        ));
    }

    dot.push('\n');
    for e in pdg.edge_references() {
        let (style, color) = match e.weight() {
            PdgEdge::Control => ("dashed", "red"),
            PdgEdge::Data => ("dotted", "blue"),
            PdgEdge::Cfg(_) => ("solid", "black"),
        };
        dot.push_str(&format!(
            "{indent}n{} -> n{} [style={style} color={color}];\n",
            e.source().index(),
            e.target().index(),
        ));
    }
}
