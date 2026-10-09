use std::collections::HashSet;

use petgraph::graph::{DiGraph, NodeIndex};

use crate::cfg::builder::{BasicBlock, BlockEdge, BlockKind};

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
    // Inside a method only. A file's CFG is one Entry/Exit pair per method laid end to
    // end, so a dominator search rooted at the first Entry reasons about nodes belonging
    // to other methods: those nodes are not dominated by that Entry and not
    // post-dominated by that Exit, and the resulting regions span unrelated code. The
    // per-file caller below is what keeps this correct in practice.
    find_hammocks_in_method(cfg, entry, exit)
}

/// Find hammock regions for every method in a file.
///
/// Each method gets its own dominator and post-dominator computation, because that is
/// the unit in which a hammock block means anything.
pub fn find_hammocks_in_file(cfg: &DiGraph<BasicBlock, BlockEdge>) -> Vec<Hammock> {
    let mut all = Vec::new();
    for entry in cfg
        .node_indices()
        .filter(|i| cfg[*i].kind == BlockKind::Entry)
    {
        let Some(exit) = reachable_exit(cfg, entry) else {
            continue;
        };
        all.extend(find_hammocks_in_method(cfg, entry, exit));
    }
    all
}

/// The first Exit reachable from `entry`.
///
/// `build_cfg` appends a method's body between its Entry and its Exit and never adds an
/// edge out of a method, so the first reachable Exit is that method's own. Matching on
/// reachability rather than on index arithmetic means a method added to the middle of
/// the file later does not silently shift the pairing.
fn reachable_exit(cfg: &DiGraph<BasicBlock, BlockEdge>, entry: NodeIndex) -> Option<NodeIndex> {
    let mut seen = HashSet::new();
    let mut stack = vec![entry];
    while let Some(node) = stack.pop() {
        if node != entry && cfg[node].kind == BlockKind::Exit {
            return Some(node);
        }
        for next in cfg.neighbors_directed(node, petgraph::Direction::Outgoing) {
            if seen.insert(next) {
                stack.push(next);
            }
        }
    }
    None
}

fn find_hammocks_in_method(
    cfg: &DiGraph<BasicBlock, BlockEdge>,
    entry: NodeIndex,
    exit: NodeIndex,
) -> Vec<Hammock> {
    let dom = Dominators::compute(cfg, entry);
    let pdom = Dominators::compute_reverse(cfg, exit);

    let mut hammocks = Vec::new();

    // Only this method's nodes. `dominates` is now a set lookup, so a cross-method pair
    // no longer hangs — but it would still be nonsense: two blocks from different methods
    // are not control-dependent on each other in any sense worth reporting.
    let in_method: Vec<NodeIndex> = cfg
        .node_indices()
        .filter(|i| {
            *i != entry
                && *i != exit
                && cfg[*i].start_line >= cfg[entry].start_line
                && cfg[*i].end_line <= cfg[exit].end_line
        })
        .collect();

    for h in &in_method {
        let h = *h;
        for t in &in_method {
            let t = *t;
            if h == t {
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

/// Dominator sets, one per node.
///
/// The sets, not the immediate dominators, because `dominates` needs the relation and
/// not a parent pointer. The previous version walked an idom chain, and a chain can
/// cycle: the idom was picked by a heuristic (the candidate with the largest dominator
/// set) rather than computed, so on a graph with several entry points it could hand
/// back a parent that eventually pointed back at the start. `dominates` then looped
/// forever -- not slowly, forever -- on 36 of the 60 real endpoint files measured.
///
/// Membership is what was wanted and it is O(1), so there is no chain to walk and no
/// way to hang.
struct Dominators {
    /// `sets[v]` is every node that dominates `v`, including `v` itself.
    sets: Vec<HashSet<NodeIndex>>,
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

        // The sets are the answer. Deriving an idom chain from them buys nothing here
        // and is where the cycle came from.
        Dominators {
            sets: dom_sets
                .into_iter()
                .map(|s| s.unwrap_or_default())
                .collect(),
        }
    }

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

        Self {
            sets: dom_sets
                .into_iter()
                .map(|s| s.unwrap_or_default())
                .collect(),
        }
    }

    /// Whether `dom` dominates `node`.
    ///
    /// A set lookup, by construction of the dataflow: `sets[node]` *is* the set of nodes
    /// that dominate it. Walking an idom chain to answer this was both slower and, on a
    /// graph with more than one entry, non-terminating.
    fn dominates(&self, dom: NodeIndex, node: NodeIndex) -> bool {
        self.sets
            .get(node.index())
            .is_some_and(|s| s.contains(&dom))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cfg::builder::{build_cfg, BlockKind};

    #[test]
    fn test_dominators_empty_method() {
        let cfg = build_cfg("class C { void M() { } }").unwrap();
        let entry = cfg
            .node_indices()
            .find(|i| cfg[*i].kind == BlockKind::Entry)
            .unwrap();
        let exit = cfg
            .node_indices()
            .find(|i| cfg[*i].kind == BlockKind::Exit)
            .unwrap();
        let dom = Dominators::compute(&cfg, entry);
        assert!(dom.dominates(entry, exit));
    }

    #[test]
    fn test_hammocks_if_else() {
        let cfg =
            build_cfg("class C { void M() { if (true) { foo(); } else { bar(); } } }").unwrap();
        let entry = cfg
            .node_indices()
            .find(|i| cfg[*i].kind == BlockKind::Entry)
            .unwrap();
        let exit = cfg
            .node_indices()
            .find(|i| cfg[*i].kind == BlockKind::Exit)
            .unwrap();
        let hammocks = find_hammocks(&cfg, entry, exit);
        assert!(!hammocks.is_empty(), "Expected hammocks, found none");
    }

    #[test]
    fn test_hammocks_sequential_no_hammocks() {
        let cfg = build_cfg("class C { void M() { int a = 1; int b = 2; } }").unwrap();
        let entry = cfg
            .node_indices()
            .find(|i| cfg[*i].kind == BlockKind::Entry)
            .unwrap();
        let exit = cfg
            .node_indices()
            .find(|i| cfg[*i].kind == BlockKind::Exit)
            .unwrap();
        let hammocks = find_hammocks(&cfg, entry, exit);
        // Sequential code with no branching shouldn't have hammocks
        // (each statement is its own node, but there's no structured region)
        assert!(hammocks.is_empty());
    }

    #[test]
    fn test_hammocks_loop() {
        let cfg = build_cfg("class C { void M() { for (;;) { foo(); } } }").unwrap();
        let entry = cfg
            .node_indices()
            .find(|i| cfg[*i].kind == BlockKind::Entry)
            .unwrap();
        let exit = cfg
            .node_indices()
            .find(|i| cfg[*i].kind == BlockKind::Exit)
            .unwrap();
        let hammocks = find_hammocks(&cfg, entry, exit);
        // for loop should form a hammock (header=loop_cond, footer=loop_exit or post-loop)
        assert!(!hammocks.is_empty());
    }

    #[test]
    fn test_hammock_footer_post_dominates_header() {
        let cfg =
            build_cfg("class C { void M() { if (true) { foo(); } else { bar(); } } }").unwrap();
        let entry = cfg
            .node_indices()
            .find(|i| cfg[*i].kind == BlockKind::Entry)
            .unwrap();
        let exit = cfg
            .node_indices()
            .find(|i| cfg[*i].kind == BlockKind::Exit)
            .unwrap();
        let pdom = Dominators::compute_reverse(&cfg, exit);
        let hammocks = find_hammocks(&cfg, entry, exit);
        for h in &hammocks {
            assert!(
                pdom.dominates(h.footer, h.header),
                "footer must post-dominate header"
            );
        }
    }
}
