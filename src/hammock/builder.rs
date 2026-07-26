use std::collections::HashSet;

use petgraph::graph::{DiGraph, NodeIndex};

use crate::cfg::builder::{BasicBlock, BlockEdge};

/// A hammock region: single-entry, single-exit subgraph
#[derive(Debug, Clone)]
pub struct Hammock {
    pub header: NodeIndex,
    pub footer: NodeIndex,
    pub body: Vec<NodeIndex>,
}

/// Find all hammock regions in a CFG using the Johnson '94 algorithm.
///
/// A hammock (h, t) satisfies:
/// 1. h dominates t
/// 2. t post-dominates h
/// 3. All edges entering the region go to h (single entry)
/// 4. All edges leaving the region come from t (single exit)
pub fn find_hammocks(
    cfg: &DiGraph<BasicBlock, BlockEdge>,
    entry: NodeIndex,
    exit: NodeIndex,
) -> Vec<Hammock> {
    let dom = Dominators::compute(cfg, entry);
    let pdom = Dominators::compute_reverse(cfg, exit);

    let mut hammocks = Vec::new();

    for h in cfg.node_indices() {
        if h == entry || h == exit {
            continue;
        }
        for t in cfg.node_indices() {
            if t == entry || t == exit || h == t {
                continue;
            }

            if !dom.dominates(h, t) {
                continue;
            }
            if !pdom.dominates(t, h) {
                continue;
            }

            let body = compute_body(cfg, h, t);
            if body.len() <= 1 {
                // Skip trivial hammocks (single node body)
                continue;
            }

            if !check_single_entry(cfg, h, t, &body) {
                continue;
            }
            if !check_single_exit(cfg, h, t, &body) {
                continue;
            }

            hammocks.push(Hammock {
                header: h,
                footer: t,
                body,
            });
        }
    }

    // Sort by body size ascending (smallest hammocks first)
    hammocks.sort_by_key(|h| h.body.len());
    hammocks
}

/// Compute the body of a candidate hammock (h, t):
/// nodes reachable from h without going through t,
/// plus h and t themselves.
fn compute_body(
    cfg: &DiGraph<BasicBlock, BlockEdge>,
    h: NodeIndex,
    t: NodeIndex,
) -> Vec<NodeIndex> {
    let mut body = Vec::new();
    let mut visited = HashSet::new();
    let mut stack = vec![h];
    visited.insert(h);

    while let Some(node) = stack.pop() {
        if node == t {
            // Don't include footer in body traversal
            continue;
        }
        body.push(node);
        for next in cfg.neighbors(node) {
            if visited.insert(next) {
                stack.push(next);
            }
        }
    }

    body
}

/// Check that every edge from outside the region targets only the header
fn check_single_entry(
    cfg: &DiGraph<BasicBlock, BlockEdge>,
    h: NodeIndex,
    _t: NodeIndex,
    body: &[NodeIndex],
) -> bool {
    let body_set: HashSet<_> = body.iter().copied().collect();
    for &node in &body_set {
        if node == h {
            continue;
        }
        for pred in cfg.neighbors_directed(node, petgraph::Direction::Incoming) {
            if !body_set.contains(&pred) {
                // Edge from outside into non-header node → violates single entry
                return false;
            }
        }
    }
    true
}

/// Check that every edge from inside the region targets only the footer
fn check_single_exit(
    cfg: &DiGraph<BasicBlock, BlockEdge>,
    _h: NodeIndex,
    t: NodeIndex,
    body: &[NodeIndex],
) -> bool {
    let body_set: HashSet<_> = body.iter().copied().collect();
    for &node in &body_set {
        for next in cfg.neighbors(node) {
            if node == t || next == t {
                continue;
            }
            if !body_set.contains(&next) {
                // Edge from body to outside that's not from footer → violates single exit
                return false;
            }
        }
    }
    true
}

/// Generic dominator computation using iterative dataflow
struct Dominators {
    idoms: Vec<Option<NodeIndex>>,
}

impl Dominators {
    /// Forward dominators: standard iterative algorithm
    /// dom(entry) = {entry}
    /// dom(n) = {n} ∪ (∩ dom(p) for all predecessors p of n)
    fn compute(cfg: &DiGraph<BasicBlock, BlockEdge>, entry: NodeIndex) -> Self {
        let n = cfg.node_count();
        let mut dom_sets: Vec<Option<HashSet<NodeIndex>>> = vec![None; n];

        // Initialize: entry = {entry}, others = all nodes
        let all_nodes: HashSet<NodeIndex> = cfg.node_indices().collect();
        for v in cfg.node_indices() {
            dom_sets[v.index()] = if v == entry {
                Some(HashSet::from([entry]))
            } else {
                Some(all_nodes.clone())
            };
        }

        // Iterate until stable
        let mut changed = true;
        while changed {
            changed = false;
            for v in cfg.node_indices() {
                if v == entry {
                    continue;
                }

                // Compute intersection of predecessors' dom sets
                let preds: Vec<NodeIndex> = cfg
                    .neighbors_directed(v, petgraph::Direction::Incoming)
                    .collect();

                if preds.is_empty() {
                    continue;
                }

                let mut new_dom = dom_sets[preds[0].index()]
                    .as_ref()
                    .cloned()
                    .unwrap_or_default();

                for p in &preds[1..] {
                    let pset = dom_sets[p.index()].as_ref().cloned().unwrap_or_default();
                    new_dom = new_dom.intersection(&pset).copied().collect();
                }
                new_dom.insert(v); // {v} ∪ intersection

                if dom_sets[v.index()].as_ref() != Some(&new_dom) {
                    dom_sets[v.index()] = Some(new_dom);
                    changed = true;
                }
            }
        }

        // Compute immediate dominator from dominator sets
        let mut idoms = vec![None; n];
        idoms[entry.index()] = Some(entry);
        for v in cfg.node_indices() {
            if v == entry {
                continue;
            }
            let dom_v = match &dom_sets[v.index()] {
                Some(s) => s.clone(),
                None => continue,
            };

            // idom(v) = the unique d ∈ dom(v) \ {v} that is dominated by all
            // other members of dom(v) \ {v}
            let mut candidates: Vec<NodeIndex> = dom_v.iter().filter(|&&d| d != v).copied().collect();
            // Sort by dom set size descending — the immediate dominator has the largest dom set
            candidates.sort_by_key(|&c| dom_sets[c.index()].as_ref().map(|s| s.len()).unwrap_or(0));
            idoms[v.index()] = candidates.last().copied();
        }

        Self { idoms }
    }

    /// Reverse dominators (post-dominators): same algorithm on reversed graph
    fn compute_reverse(cfg: &DiGraph<BasicBlock, BlockEdge>, exit: NodeIndex) -> Self {
        let n = cfg.node_count();

        // Build reverse graph
        let mut rev = DiGraph::<(), ()>::with_capacity(n, cfg.edge_count());
        for _ in cfg.node_indices() {
            rev.add_node(());
        }
        for e in cfg.raw_edges() {
            rev.add_edge(e.target(), e.source(), ());
        }

        // Run standard dominator algorithm on reverse graph
        let all_nodes: HashSet<NodeIndex> = cfg.node_indices().collect();
        let mut dom_sets: Vec<Option<HashSet<NodeIndex>>> = vec![None; n];

        for v in cfg.node_indices() {
            dom_sets[v.index()] = if v == exit {
                Some(HashSet::from([exit]))
            } else {
                Some(all_nodes.clone())
            };
        }

        let mut changed = true;
        while changed {
            changed = false;
            for v in cfg.node_indices() {
                if v == exit {
                    continue;
                }

                let preds: Vec<NodeIndex> = rev
                    .neighbors_directed(v, petgraph::Direction::Incoming)
                    .collect();

                if preds.is_empty() {
                    continue;
                }

                let mut new_dom = dom_sets[preds[0].index()]
                    .as_ref()
                    .cloned()
                    .unwrap_or_default();
                for p in &preds[1..] {
                    let pset = dom_sets[p.index()].as_ref().cloned().unwrap_or_default();
                    new_dom = new_dom.intersection(&pset).copied().collect();
                }
                new_dom.insert(v);

                if dom_sets[v.index()].as_ref() != Some(&new_dom) {
                    dom_sets[v.index()] = Some(new_dom);
                    changed = true;
                }
            }
        }

        let mut idoms = vec![None; n];
        idoms[exit.index()] = Some(exit);
        for v in cfg.node_indices() {
            if v == exit {
                continue;
            }
            let dom_v = match &dom_sets[v.index()] {
                Some(s) => s.clone(),
                None => continue,
            };
            let mut candidates: Vec<NodeIndex> = dom_v.iter().filter(|&&d| d != v).copied().collect();
            candidates.sort_by_key(|&c| dom_sets[c.index()].as_ref().map(|s| s.len()).unwrap_or(0));
            idoms[v.index()] = candidates.last().copied();
        }

        Self { idoms }
    }

    fn idom(&self, node: NodeIndex) -> Option<NodeIndex> {
        self.idoms.get(node.index()).copied().flatten()
    }

    fn dominates(&self, dom: NodeIndex, node: NodeIndex) -> bool {
        if dom == node {
            return true;
        }
        let mut cur = node;
        loop {
            match self.idom(cur) {
                Some(id) => {
                    if id == dom {
                        return true;
                    }
                    if id == cur {
                        return false;
                    }
                    cur = id;
                }
                None => return false,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cfg::builder::{build_cfg, BlockKind};

    #[test]
    fn test_dominators_empty_method() {
        let cfg = build_cfg("class C { void M() { } }").unwrap();
        let entry = cfg.node_indices().find(|i| cfg[*i].kind == BlockKind::Entry).unwrap();
        let exit = cfg.node_indices().find(|i| cfg[*i].kind == BlockKind::Exit).unwrap();
        let dom = Dominators::compute(&cfg, entry);
        assert!(dom.dominates(entry, exit));
    }

    #[test]
    fn test_hammocks_if_else() {
        let cfg = build_cfg("class C { void M() { if (true) { foo(); } else { bar(); } } }").unwrap();
        let entry = cfg.node_indices().find(|i| cfg[*i].kind == BlockKind::Entry).unwrap();
        let exit = cfg.node_indices().find(|i| cfg[*i].kind == BlockKind::Exit).unwrap();
        let hammocks = find_hammocks(&cfg, entry, exit);
        assert!(!hammocks.is_empty(), "Expected hammocks, found none");
    }

    #[test]
    fn test_hammocks_sequential_no_hammocks() {
        let cfg = build_cfg("class C { void M() { int a = 1; int b = 2; } }").unwrap();
        let entry = cfg.node_indices().find(|i| cfg[*i].kind == BlockKind::Entry).unwrap();
        let exit = cfg.node_indices().find(|i| cfg[*i].kind == BlockKind::Exit).unwrap();
        let hammocks = find_hammocks(&cfg, entry, exit);
        // Sequential code with no branching shouldn't have hammocks
        // (each statement is its own node, but there's no structured region)
        assert!(hammocks.is_empty());
    }

    #[test]
    fn test_hammocks_loop() {
        let cfg = build_cfg("class C { void M() { for (;;) { foo(); } } }").unwrap();
        let entry = cfg.node_indices().find(|i| cfg[*i].kind == BlockKind::Entry).unwrap();
        let exit = cfg.node_indices().find(|i| cfg[*i].kind == BlockKind::Exit).unwrap();
        let hammocks = find_hammocks(&cfg, entry, exit);
        // for loop should form a hammock (header=loop_cond, footer=loop_exit or post-loop)
        assert!(!hammocks.is_empty());
    }

    #[test]
    fn test_hammock_footer_post_dominates_header() {
        let cfg = build_cfg("class C { void M() { if (true) { foo(); } else { bar(); } } }").unwrap();
        let entry = cfg.node_indices().find(|i| cfg[*i].kind == BlockKind::Entry).unwrap();
        let exit = cfg.node_indices().find(|i| cfg[*i].kind == BlockKind::Exit).unwrap();
        let pdom = Dominators::compute_reverse(&cfg, exit);
        let hammocks = find_hammocks(&cfg, entry, exit);
        for h in &hammocks {
            assert!(pdom.dominates(h.footer, h.header),
                "footer must post-dominate header");
        }
    }
}
