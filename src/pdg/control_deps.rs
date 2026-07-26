use petgraph::graph::{DiGraph, NodeIndex};
use std::collections::HashSet;

use crate::cfg::builder::{BasicBlock, BlockEdge};

/// Post-dominator tree'den control dependence edges çıkarır.
pub fn compute_control_deps(
    cfg: &DiGraph<BasicBlock, BlockEdge>,
    entry: NodeIndex,
    exit: NodeIndex,
) -> Vec<(NodeIndex, NodeIndex)> {
    let pdom = PostDominators::compute(cfg, exit);
    let mut result = Vec::new();

    for u in cfg.node_indices() {
        if u == exit {
            continue;
        }
        // Control dependence sadece birden çok successor'u olan node'lar için
        let successors: Vec<_> = cfg.neighbors(u).collect();
        if successors.len() < 2 {
            continue;
        }
        for &v in &successors {
            if v == u || v == exit {
                continue;
            }
            let mut w = v;
            loop {
                if w == entry || w == exit {
                    break;
                }
                if pdom.dominates(w, u) {
                    break;
                }
                result.push((u, w));
                match pdom.idom(w) {
                    Some(next) if next != w => w = next,
                    _ => break,
                }
            }
        }
    }

    result.sort();
    result.dedup();
    result
}

struct PostDominators {
    idoms: Vec<Option<NodeIndex>>,
}

impl PostDominators {
    /// Reverse graph üzerinde dominator hesapla (post-dominator).
    fn compute(cfg: &DiGraph<BasicBlock, BlockEdge>, exit: NodeIndex) -> Self {
        // Reverse graph oluştur
        let n = cfg.node_count();
        let mut rev = DiGraph::<(), ()>::with_capacity(n, cfg.edge_count());
        for _ in cfg.node_indices() {
            rev.add_node(());
        }
        for e in cfg.raw_edges() {
            rev.add_edge(e.target(), e.source(), ());
        }

        // Exit'ten ulaşılamayan node'ları bul
        let reachable_from_exit = reachable_set(&rev, exit);

        // Her node için post-dominator adaylarını bul
        let mut idoms = vec![None; n];

        for v in cfg.node_indices() {
            if v == exit {
                idoms[v.index()] = Some(exit);
                continue;
            }
            if !reachable_from_exit.contains(&v) {
                continue;
            }

            // Exit'e giden tüm path'ler
            let paths = all_paths_to_exit(cfg, v, exit, 100);
            if paths.is_empty() {
                continue;
            }

            // Tüm path'lerde ortak olan node'lar → post-dominator adayları
            let mut common: HashSet<NodeIndex> = paths[0].iter().copied().collect();
            for p in &paths[1..] {
                let pset: HashSet<_> = p.iter().copied().collect();
                common.retain(|n| pset.contains(n));
            }
            common.remove(&v);

            // En yakın post-dominator (idom)
            idoms[v.index()] = common.into_iter().min_by_key(|&c| {
                cfg.neighbors(v).filter(|&n| n == c).count()
            });
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

fn reachable_set(graph: &DiGraph<(), ()>, root: NodeIndex) -> HashSet<NodeIndex> {
    let mut set = HashSet::new();
    let mut stack = vec![root];
    set.insert(root);
    while let Some(node) = stack.pop() {
        for next in graph.neighbors(node) {
            if set.insert(next) {
                stack.push(next);
            }
        }
    }
    set
}

fn all_paths_to_exit(
    cfg: &DiGraph<BasicBlock, BlockEdge>,
    from: NodeIndex,
    exit: NodeIndex,
    max: usize,
) -> Vec<Vec<NodeIndex>> {
    let mut result = Vec::new();
    let mut stack = vec![(from, vec![from], HashSet::new())];
    while let Some((cur, path, mut visited)) = stack.pop() {
        if cur == exit {
            result.push(path);
            if result.len() >= max {
                return result;
            }
            continue;
        }
        visited.insert(cur);
        for next in cfg.neighbors(cur) {
            if !visited.contains(&next) {
                let mut new_path = path.clone();
                new_path.push(next);
                let new_visited = visited.clone();
                stack.push((next, new_path, new_visited));
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cfg::builder::{build_cfg, BlockKind};

    #[test]
    fn test_sequential_no_control_deps() {
        let cfg = build_cfg("class C { void M() { int a = 1; int b = 2; } }").unwrap();
        let entry = cfg.node_indices().find(|i| cfg[*i].kind == BlockKind::Entry).unwrap();
        let exit = cfg.node_indices().find(|i| cfg[*i].kind == BlockKind::Exit).unwrap();
        let deps = compute_control_deps(&cfg, entry, exit);
        assert_eq!(deps.len(), 0);
    }

    #[test]
    fn test_if_has_control_deps() {
        let cfg = build_cfg("class C { void M() { if (true) { foo(); } else { bar(); } } }").unwrap();
        let entry = cfg.node_indices().find(|i| cfg[*i].kind == BlockKind::Entry).unwrap();
        let exit = cfg.node_indices().find(|i| cfg[*i].kind == BlockKind::Exit).unwrap();
        let deps = compute_control_deps(&cfg, entry, exit);
        assert!(deps.len() > 0);
    }
}
