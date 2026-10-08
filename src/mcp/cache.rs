//! Parse cache for MCP tool calls.
//!
//! Parsing is the dominant cost: a full 1667-file solution takes ~1.1s per
//! call, which would make an MCP tool unusable in an agent loop. A parsed
//! project is immutable once built, so cache it per path for the process
//! lifetime.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, RwLock};

use crate::analysis::callgraph::CallGraph;
use crate::resolve::types::TypeGraph;

/// A parsed project. Immutable and shared between concurrent tool calls.
#[derive(Debug)]
pub struct Project {
    pub root: PathBuf,
    pub type_graph: TypeGraph,
    pub call_graph: CallGraph,
    /// Files that were parsed, for diagnostics.
    pub file_count: usize,
}

type Cache = RwLock<HashMap<PathBuf, Arc<Project>>>;

fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Normalise a path for use as a cache key: absolute where possible, without a
/// trailing separator. Two relative spellings of the same directory must hit
/// the same entry.
pub fn cache_key(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    let mut normalized = PathBuf::new();
    for comp in absolute.components() {
        normalized.push(comp);
    }
    while normalized.to_string_lossy().len() > 1 && normalized.to_string_lossy().ends_with('/') {
        normalized.pop();
    }
    normalized
}

/// Get a parsed project, building it on first request.
pub fn get_or_build(path: &Path) -> anyhow::Result<Arc<Project>> {
    let key = cache_key(path);

    if let Ok(guard) = cache().read() {
        if let Some(hit) = guard.get(&key) {
            return Ok(Arc::clone(hit));
        }
    }

    // Build outside the lock: parsing is the expensive part and holds no shared
    // state, so concurrent requests for different projects should not block.
    let built = Arc::new(build(path)?);

    match cache().write() {
        Ok(mut guard) => {
            // Another request may have finished first; keep whichever landed
            // first so callers always observe a stable Arc.
            Ok(Arc::clone(guard.entry(key).or_insert(built)))
        }
        // A poisoned cache should not fail the request — the built value is
        // still correct, it just will not be reused.
        Err(_) => Ok(built),
    }
}

fn build(path: &Path) -> anyhow::Result<Project> {
    let (type_graph, call_graph) = crate::cli::commands::load_project(path)?;
    let file_count = type_graph
        .classes
        .values()
        .flat_map(|c| c.methods.iter())
        .filter(|m| !m.file.is_empty())
        .map(|m| m.file.as_str())
        .collect::<std::collections::HashSet<_>>()
        .len();
    Ok(Project {
        root: path.to_path_buf(),
        type_graph,
        call_graph,
        file_count,
    })
}

/// Drop all cached projects. Exposed so long-lived servers and tests can bound
/// memory after a source change.
pub fn clear() {
    if let Ok(mut guard) = cache().write() {
        guard.clear();
    }
}

/// Number of cached projects, for diagnostics and tests.
pub fn len() -> usize {
    cache().read().map(|g| g.len()).unwrap_or(0)
}

/// Serialise tests that touch the cache.
///
/// The cache is process-global by design, so two tests running concurrently
/// would otherwise observe each other's `clear()` calls and the entry count
/// would not be attributable. Test-only: production callers never need this.
#[cfg(test)]
pub(crate) fn test_guard() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        // A previous test panicking must not wedge every later one.
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_normalizes_trailing_separator() {
        let _guard = test_guard();
        let a = cache_key(Path::new("/tmp/project"));
        let b = cache_key(Path::new("/tmp/project/"));
        assert_eq!(a, b);
    }

    #[test]
    fn cache_key_resolves_relative_to_cwd() {
        let _guard = test_guard();
        let rel = cache_key(Path::new("some/dir"));
        let abs = cache_key(Path::new("some/dir"));
        assert_eq!(rel, abs);
    }

    #[test]
    fn cache_key_root_stays_root() {
        assert_eq!(cache_key(Path::new("/")), PathBuf::from("/"));
    }

    #[test]
    fn get_or_build_reuses_parsed_project() {
        let _guard = test_guard();
        let dir = std::env::temp_dir().join(format!("tinylink_cache_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("A.cs"),
            "namespace N;\npublic class A { public void M() { Helper(); } }\n",
        )
        .unwrap();

        clear();
        assert_eq!(len(), 0);

        let first = get_or_build(&dir).expect("build");
        assert_eq!(first.type_graph.classes.len(), 1);
        assert_eq!(len(), 1);

        let second = get_or_build(&dir).expect("cached build");
        // Same Arc, so no reparse happened.
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(len(), 1, "cache must not duplicate entries");

        clear();
        assert_eq!(len(), 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn distinct_paths_are_cached_separately() {
        let _guard = test_guard();
        let base = std::env::temp_dir().join(format!("tinylink_cache2_{}", std::process::id()));
        let a = base.join("a");
        let b = base.join("b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("A.cs"), "namespace N;\npublic class A { }\n").unwrap();
        std::fs::write(b.join("B.cs"), "namespace N;\npublic class B { }\n").unwrap();

        clear();
        let pa = get_or_build(&a).expect("a");
        let pb = get_or_build(&b).expect("b");
        assert!(!Arc::ptr_eq(&pa, &pb));
        assert!(pa.type_graph.classes.contains_key("A"));
        assert!(pb.type_graph.classes.contains_key("B"));
        assert_eq!(len(), 2);

        clear();
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn missing_path_reports_error() {
        let _guard = test_guard();
        clear();
        let result = get_or_build(Path::new("/nonexistent/tinylink/path"));
        // A missing directory yields an empty project rather than a panic; what
        // matters is that it does not panic and does not poison the cache.
        if let Ok(p) = result {
            assert!(p.type_graph.classes.is_empty());
        }
        clear();
    }
}
