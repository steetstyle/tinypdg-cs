use petgraph::graph::{DiGraph, NodeIndex};

use crate::cfg::builder::{BasicBlock, BlockEdge};

/// Data dependence edges: tanım (def) → kullanım (use) arası
///
/// Her basic block'ta tanımlanan değişkenlerin hangi block'larda
/// kullanıldığını bulur. Reaching definitions analizi ile.
///
/// Şu an için stub: AST'den değişken isimlerini çıkaracak
/// altyapı hazır değil. Faz 3a'da gerçek implementasyon.
pub fn compute_data_deps(
    _cfg: &DiGraph<BasicBlock, BlockEdge>,
) -> Vec<(NodeIndex, NodeIndex)> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cfg::builder::build_cfg;

    #[test]
    fn test_data_deps_empty() {
        let cfg = build_cfg("class C { void M() { } }").unwrap();
        let deps = compute_data_deps(&cfg);
        assert_eq!(deps.len(), 0);
    }
}
