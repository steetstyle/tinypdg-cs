use petgraph::dot::Dot;

use crate::cfg::builder::{BasicBlock, BlockEdge};

pub fn cfg_to_dot(graph: &petgraph::graph::DiGraph<BasicBlock, BlockEdge>) -> String {
    format!("{}", Dot::new(graph))
}
