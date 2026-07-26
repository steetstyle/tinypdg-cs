use std::fmt;

use anyhow::Result;

use petgraph::graph::{DiGraph, NodeIndex};
use tree_sitter::Node;

use crate::parse::parser::parse_source;

pub type CfgGraph = DiGraph<BasicBlock, BlockEdge>;

#[derive(Debug, Clone)]
pub struct BasicBlock {
    pub id: usize,
    pub start_line: usize,
    pub end_line: usize,
    pub kind: BlockKind,
}

impl fmt::Display for BasicBlock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {:?} (L{}-L{})", self.id, self.kind, self.start_line, self.end_line)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum BlockKind {
    Entry,
    Exit,
    Statement,
    Condition,
    BranchTrue,
    BranchFalse,
    LoopHeader,
    LoopBody,
    LoopExit,
    Handler,
    Finally,
    Resume,
    Unknown,
}

impl fmt::Display for BlockKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum BlockEdge {
    Seq,
    CondTrue,
    CondFalse,
    LoopBack,
    LoopExit,
    Throw,
    Catch,
    Finally,
    Return,
    Resume,
}

impl fmt::Display for BlockEdge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

struct CfgCtx {
    graph: CfgGraph,
}

impl CfgCtx {
    fn new() -> Self {
        Self { graph: DiGraph::new() }
    }

    fn add_block(&mut self, kind: BlockKind, node: &Node) -> NodeIndex {
        let id = self.graph.node_count();
        let start = node.start_position().row + 1;
        let end = node.end_position().row + 1;
        self.graph.add_node(BasicBlock { id, start_line: start, end_line: end, kind })
    }

    fn add_edge(&mut self, from: NodeIndex, to: NodeIndex, kind: BlockEdge) {
        self.graph.add_edge(from, to, kind);
    }
}

struct BlockRange {
    first: NodeIndex,
    last: NodeIndex,
}

pub fn build_cfg(source: &str) -> Result<CfgGraph> {
    let tree = parse_source(source)?;

    let methods = {
        let mut ms = Vec::new();
        let mut cursor = tree.walk();
        let mut depth = 0usize;
        'outer: loop {
            let n = cursor.node();
            if matches!(n.kind(), "method_declaration" | "constructor_declaration") {
                ms.push(n);
            }
            if depth < 500 && cursor.goto_first_child() {
                depth += 1;
                continue;
            }
            loop {
                if cursor.goto_next_sibling() {
                    continue 'outer;
                }
                if !cursor.goto_parent() {
                    break 'outer;
                }
                depth -= 1;
            }
        }
        ms
    };

    let mut ctx = CfgCtx::new();

    for method in &methods {
        let body = match method.child_by_field_name("body") {
            Some(b) if b.kind() == "block" => b,
            _ => continue,
        };

        let entry = ctx.add_block(BlockKind::Entry, method);
        let exit = ctx.add_block(BlockKind::Exit, method);

        let result = build_block(&mut ctx, &body);
        match result {
            Some(r) => {
                ctx.add_edge(entry, r.first, BlockEdge::Seq);
                ctx.add_edge(r.last, exit, BlockEdge::Seq);
            }
            None => {
                ctx.add_edge(entry, exit, BlockEdge::Seq);
            }
        }
    }

    Ok(ctx.graph)
}

fn build_block(ctx: &mut CfgCtx, block_node: &Node) -> Option<BlockRange> {
    let mut first: Option<NodeIndex> = None;
    let mut last: Option<NodeIndex> = None;

    let mut cursor = block_node.walk();
    for child in block_node.named_children(&mut cursor) {
        let kind = child.kind();
        if matches!(kind, "{" | "}" | ";" | "(" | ")") {
            continue;
        }
        let result = build_stmt(ctx, &child);
        if let Some(r) = result {
            if let Some(prev) = last {
                ctx.add_edge(prev, r.first, BlockEdge::Seq);
            } else {
                first = Some(r.first);
            }
            last = Some(r.last);
        }
    }

    first.map(|f| BlockRange { first: f, last: last.unwrap() })
}

fn build_stmt(ctx: &mut CfgCtx, stmt: &Node) -> Option<BlockRange> {
    match stmt.kind() {
        "expression_statement" | "local_declaration_statement" | "fixed_statement"
        | "checked_statement" | "unchecked_statement" | "unsafe_statement"
        | "using_statement" | "lock_statement" => {
            let n = ctx.add_block(BlockKind::Statement, stmt);
            Some(BlockRange { first: n, last: n })
        }
        "return_statement" => {
            let n = ctx.add_block(BlockKind::Statement, stmt);
            Some(BlockRange { first: n, last: n })
        }
        "throw_statement" => {
            let n = ctx.add_block(BlockKind::Statement, stmt);
            Some(BlockRange { first: n, last: n })
        }
        "break_statement" | "continue_statement" => {
            let n = ctx.add_block(BlockKind::Statement, stmt);
            Some(BlockRange { first: n, last: n })
        }
        "block" => build_block(ctx, stmt),
        "if_statement" => build_if(ctx, stmt),
        "while_statement" => build_while(ctx, stmt),
        "do_statement" => build_do_while(ctx, stmt),
        "for_statement" => build_for(ctx, stmt),
        "for_each_statement" => build_foreach(ctx, stmt),
        "switch_statement" => build_switch(ctx, stmt),
        "try_statement" => build_try(ctx, stmt),
        "yield_statement" => {
            let n = ctx.add_block(BlockKind::Resume, stmt);
            Some(BlockRange { first: n, last: n })
        }
        _ => {
            let n = ctx.add_block(BlockKind::Unknown, stmt);
            Some(BlockRange { first: n, last: n })
        }
    }
}

/// if_statement: condition -> consequence -> merge
///                         └──> alternative (optional) -> merge
fn build_if(ctx: &mut CfgCtx, if_node: &Node) -> Option<BlockRange> {
    let cond = ctx.add_block(BlockKind::Condition, if_node);
    let merge = ctx.add_block(BlockKind::Statement, if_node);

    let consequence = if_node.child_by_field_name("consequence");
    if let Some(c) = consequence {
        if let Some(r) = build_block_or_stmt(ctx, &c) {
            ctx.add_edge(cond, r.first, BlockEdge::CondTrue);
            if !matches!(c.kind(), "return_statement" | "throw_statement") {
                ctx.add_edge(r.last, merge, BlockEdge::Seq);
            }
        } else {
            ctx.add_edge(cond, merge, BlockEdge::CondTrue);
        }
    } else {
        ctx.add_edge(cond, merge, BlockEdge::CondTrue);
    }

    if let Some(alt) = if_node.child_by_field_name("alternative") {
        if let Some(r) = build_block_or_stmt(ctx, &alt) {
            ctx.add_edge(cond, r.first, BlockEdge::CondFalse);
            if !matches!(alt.kind(), "return_statement" | "throw_statement") {
                ctx.add_edge(r.last, merge, BlockEdge::Seq);
            }
        } else {
            ctx.add_edge(cond, merge, BlockEdge::CondFalse);
        }
    } else {
        ctx.add_edge(cond, merge, BlockEdge::CondFalse);
    }

    Some(BlockRange { first: cond, last: merge })
}

/// while_statement: condition (loop header) -> body -> loop back to condition
///                                        └──> loop exit
fn build_while(ctx: &mut CfgCtx, while_node: &Node) -> Option<BlockRange> {
    let header = ctx.add_block(BlockKind::LoopHeader, while_node);
    let exit = ctx.add_block(BlockKind::LoopExit, while_node);

    let body = while_node.child_by_field_name("body");
    if let Some(b) = body {
        if let Some(r) = build_block_or_stmt(ctx, &b) {
            ctx.add_edge(header, r.first, BlockEdge::CondTrue);
            ctx.add_edge(r.last, header, BlockEdge::LoopBack);
        } else {
            ctx.add_edge(header, exit, BlockEdge::CondTrue);
        }
    }
    ctx.add_edge(header, exit, BlockEdge::CondFalse);

    Some(BlockRange { first: header, last: exit })
}

/// do_statement: body -> condition -> loop back to body
///                                 └──> loop exit
fn build_do_while(ctx: &mut CfgCtx, do_node: &Node) -> Option<BlockRange> {
    let header = ctx.add_block(BlockKind::LoopHeader, do_node);
    let exit = ctx.add_block(BlockKind::LoopExit, do_node);

    let body = do_node.child_by_field_name("body");
    if let Some(b) = body {
        if let Some(r) = build_block_or_stmt(ctx, &b) {
            ctx.add_edge(header, r.first, BlockEdge::Seq);
            ctx.add_edge(r.last, exit, BlockEdge::CondFalse);
            ctx.add_edge(r.last, header, BlockEdge::LoopBack);
        } else {
            ctx.add_edge(header, exit, BlockEdge::Seq);
        }
    } else {
        ctx.add_edge(header, exit, BlockEdge::Seq);
    }

    Some(BlockRange { first: header, last: exit })
}

/// for_statement: init -> condition -> body -> increment -> condition
fn build_for(ctx: &mut CfgCtx, for_node: &Node) -> Option<BlockRange> {
    let header = ctx.add_block(BlockKind::LoopHeader, for_node);
    let exit = ctx.add_block(BlockKind::LoopExit, for_node);

    if let Some(init) = for_node.child_by_field_name("initializer") {
        let n = ctx.add_block(BlockKind::Statement, &init);
        ctx.add_edge(n, header, BlockEdge::Seq);
    }

    let body = for_node.child_by_field_name("body");
    if let Some(b) = body {
        if let Some(r) = build_block_or_stmt(ctx, &b) {
            ctx.add_edge(header, r.first, BlockEdge::CondTrue);

            if let Some(inc) = for_node.child_by_field_name("increment") {
                let inc_node = ctx.add_block(BlockKind::Statement, &inc);
                ctx.add_edge(r.last, inc_node, BlockEdge::Seq);
                ctx.add_edge(inc_node, header, BlockEdge::LoopBack);
            } else {
                ctx.add_edge(r.last, header, BlockEdge::LoopBack);
            }
        }
    }
    ctx.add_edge(header, exit, BlockEdge::CondFalse);

    Some(BlockRange { first: header, last: exit })
}

/// for_each_statement: similar to for — condition per iteration
fn build_foreach(ctx: &mut CfgCtx, foreach_node: &Node) -> Option<BlockRange> {
    let header = ctx.add_block(BlockKind::LoopHeader, foreach_node);
    let exit = ctx.add_block(BlockKind::LoopExit, foreach_node);

    let body = foreach_node.child_by_field_name("body");
    if let Some(b) = body {
        if let Some(r) = build_block_or_stmt(ctx, &b) {
            ctx.add_edge(header, r.first, BlockEdge::CondTrue);
            ctx.add_edge(r.last, header, BlockEdge::LoopBack);
        }
    }
    ctx.add_edge(header, exit, BlockEdge::CondFalse);

    Some(BlockRange { first: header, last: exit })
}

/// switch_statement: selector -> each case's first statement -> merge
fn build_switch(ctx: &mut CfgCtx, switch_node: &Node) -> Option<BlockRange> {
    let header = ctx.add_block(BlockKind::Condition, switch_node);
    let merge = ctx.add_block(BlockKind::Statement, switch_node);

    let body = switch_node.child_by_field_name("body");
    if let Some(b) = body {
        let mut cursor = b.walk();
        for section in b.named_children(&mut cursor) {
            if section.kind() != "switch_section" {
                continue;
            }
            let mut sc = section.walk();
            let mut has_body = false;
            for child in section.named_children(&mut sc) {
                if matches!(child.kind(), "break_statement" | "continue_statement") {
                    let n = ctx.add_block(BlockKind::Statement, &child);
                    ctx.add_edge(header, n, BlockEdge::CondTrue);
                    has_body = true;
                } else if let Some(r) = build_block_or_stmt(ctx, &child) {
                    ctx.add_edge(header, r.first, BlockEdge::CondTrue);
                    ctx.add_edge(r.last, merge, BlockEdge::Seq);
                    has_body = true;
                }
            }
            if !has_body {
                ctx.add_edge(header, merge, BlockEdge::CondTrue);
            }
        }
    }
    ctx.add_edge(header, merge, BlockEdge::CondFalse);

    Some(BlockRange { first: header, last: merge })
}

/// try_statement: body (normal/throw) -> catch(es) -> finally (always)
fn build_try(ctx: &mut CfgCtx, try_node: &Node) -> Option<BlockRange> {
    let body = try_node.child_by_field_name("body")?;
    let body_result = build_block(ctx, &body);

    let mut catches: Vec<BlockRange> = Vec::new();
    let mut cursor = try_node.walk();
    for child in try_node.named_children(&mut cursor) {
        if child.kind() == "catch_clause" {
            if let Some(catch_body) = child.child_by_field_name("body") {
                if let Some(r) = build_block(ctx, &catch_body) {
                    catches.push(r);
                }
            }
        }
    }

    let finally_result: Option<BlockRange> = try_node.named_children(&mut try_node.walk())
        .find(|c| c.kind() == "finally_clause")
        .and_then(|f| {
            if let Some(fb) = f.named_child(0) {
                build_block(ctx, &fb)
            } else {
                None
            }
        });

    let first = body_result.as_ref().map(|b| b.first)?;
    let last = finally_result.as_ref().map(|f| f.last)
        .or_else(|| catches.last().map(|c| c.last))
        .or_else(|| body_result.as_ref().map(|b| b.last))?;

    if let Some(ref b) = body_result {
        for c in &catches {
            ctx.add_edge(b.last, c.first, BlockEdge::Throw);
            if let Some(ref f) = finally_result {
                ctx.add_edge(c.last, f.first, BlockEdge::Finally);
            }
        }
        if let Some(ref f) = finally_result {
            ctx.add_edge(b.last, f.first, BlockEdge::Seq);
        }
    }

    Some(BlockRange { first, last })
}

/// Helper: if the node is a block, recurse; otherwise treat as a single statement.
fn build_block_or_stmt(ctx: &mut CfgCtx, node: &Node) -> Option<BlockRange> {
    if node.kind() == "block" {
        build_block(ctx, node)
    } else {
        build_stmt(ctx, node)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_method() {
        let g = build_cfg("class C { void M() { } }").unwrap();
        assert_eq!(g.node_count(), 2);
    }

    #[test]
    fn test_sequential() {
        let g = build_cfg("class C { void M() { int a = 1; int b = 2; } }").unwrap();
        assert_eq!(g.node_count(), 4);
    }

    #[test]
    fn test_if() {
        let g = build_cfg("class C { void M() { if (true) { foo(); } } }").unwrap();
        assert!(g.node_count() >= 4);
    }

    #[test]
    fn test_if_else() {
        let g = build_cfg("class C { void M() { if (true) { foo(); } else { bar(); } } }").unwrap();
        assert!(g.node_count() >= 5);
    }

    #[test]
    fn test_while() {
        let g = build_cfg("class C { void M() { while (true) { foo(); } } }").unwrap();
        assert!(g.node_count() >= 4);
    }

    #[test]
    fn test_for() {
        let g = build_cfg("class C { void M() { for (int i = 0; i < 10; i++) { foo(); } } }").unwrap();
        assert!(g.node_count() >= 5);
    }

    #[test]
    fn test_switch() {
        let g = build_cfg("class C { void M(int x) { switch (x) { case 1: foo(); break; case 2: bar(); break; } } }").unwrap();
        assert!(g.node_count() >= 5);
    }

    #[test]
    fn test_try_catch() {
        let g = build_cfg("class C { void M() { try { foo(); } catch { bar(); } } }").unwrap();
        assert!(g.node_count() >= 4); // entry, exit, try-body, catch-body
    }

    #[test]
    fn test_try_finally() {
        let g = build_cfg("class C { void M() { try { foo(); } finally { bar(); } } }").unwrap();
        assert!(g.node_count() >= 4); // entry, exit, try-body, finally-body
    }

    #[test]
    fn test_try_catch_finally() {
        let g = build_cfg("class C { void M() { try { foo(); } catch { bar(); } finally { baz(); } } }").unwrap();
        assert!(g.node_count() >= 5);
    }

    #[test]
    fn test_nested_if() {
        let g = build_cfg("class C { void M() { if (a) { if (b) { foo(); } } } }").unwrap();
        assert!(g.node_count() >= 5);
    }
}
