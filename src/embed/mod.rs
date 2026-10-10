//! Semantic code search, as anchors rather than as a ranking.
//!
//! # What this is for
//!
//! A task text like *"why does the agency member list time out"* names three things: a
//! domain word, an operation, and a failure. The graph already knows two of them —
//! `GetAgencyMembersEndpoint.Handler` is in the call graph and `Handler` is on a
//! route. Only the failure is new, and only the failure is what an embedding is good at
//! finding. That is the entire job: **naming entry points the graph does not already
//! know**.
//!
//! # What this is not for
//!
//! It does not rank the answer. A vector distance never enters the ordering. Position
//! in a result comes from the graph — graph distance, reference kind, centrality —
//! because those are answers with exact values, and a cosine similarity is not.
//!
//! The reason is not squeamishness about embeddings. It is that the two kinds of answer
//! fail differently. A graph distance is either right or it is not, and you can check
//! it. A similarity is a number that moves when the model does, so an answer built on
//! one is not reproducible: the same question tomorrow returns a different order, and
//! nobody can tell whether the code changed or the model did.
//!
//! So a hit carries which sources agreed, and agreement is the thing to read:
//!
//! ```text
//! anchors {
//!   "GetAgencyMembersEndpoint.Handler"  [explicit, lexical]
//!   "AgencyRepository.Resolve"          [semantic]
//! }
//! ```
//!
//! Two independent sources agreeing is usually right. One is a guess, and the list
//! says so without having to be trusted.

pub mod anchors;
pub mod filter;
pub mod index;
pub mod provider;
pub mod store;

pub use anchors::{find_context, Anchor, Context, Coverage, Source};
pub use filter::Filter;
pub use index::{index_project, search, Symbol};
pub use provider::{tokenize, Embedding, HashingProvider, OpenAiProvider, Provider, ProviderSpec};
pub use store::{Entry, Hit, IndexInfo, PostgresStore, SqliteStore, VectorStore};

use std::path::Path;

/// Which backend a store specifier names.
///
/// Split out from `open_store` so the routing can be checked without a database, which
/// is the only way to test it honestly: opening really connects, so a test with a made-up
/// URL would be testing a connection failure and calling it a routing test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Sqlite,
    Postgres,
}

impl Backend {
    pub fn as_str(self) -> &'static str {
        match self {
            Backend::Sqlite => "sqlite",
            Backend::Postgres => "postgres",
        }
    }
}

/// Read a store specifier.
///
/// `sqlite:<path>` and a bare path both mean the file backend; `postgres:<url>` and
/// `pg:<url>` mean pgvector. A URL is caught by its scheme, so it is never mistaken for a
/// filename -- writing a file named `postgres://u:p@h:5432/db` would be silent nonsense.
pub fn backend_for(spec: &str) -> Result<(Backend, String), String> {
    let trimmed = spec.trim();

    if let Some(url) = trimmed
        .strip_prefix("postgres:")
        .or_else(|| trimmed.strip_prefix("pg:"))
    {
        if url.is_empty() {
            return Err("postgres: needs a connection url, as postgres://user@host/db".into());
        }
        return Ok((Backend::Postgres, url.to_string()));
    }
    // A bare scheme is a URL somebody pasted without the prefix.
    if trimmed.starts_with("postgres://") || trimmed.starts_with("postgresql://") {
        return Ok((Backend::Postgres, trimmed.to_string()));
    }

    let path = trimmed
        .strip_prefix("sqlite:")
        .unwrap_or(trimmed)
        .trim_start_matches("file:")
        .to_string();

    if path.is_empty() {
        return Err(
            "a store needs a location: --store <path.db>, or postgres:<url> for pgvector"
                .to_string(),
        );
    }
    Ok((Backend::Sqlite, path))
}

/// Open a store by specifier.
pub fn open_store(spec: &str) -> Result<Box<dyn VectorStore>, String> {
    match backend_for(spec)? {
        (Backend::Postgres, url) => Ok(Box::new(PostgresStore::open(&url)?)),
        (Backend::Sqlite, path) => {
            if let Some(parent) = Path::new(&path).parent() {
                if !parent.as_os_str().is_empty() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
                }
            }
            Ok(Box::new(SqliteStore::open(Path::new(&path))?))
        }
    }
}

/// Describe a store without touching it, for `embed --dry-run` and `info`.
pub fn store_info(store: &dyn VectorStore) -> Result<IndexInfo, String> {
    store.info()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_path_is_sqlite() {
        let (backend, path) = backend_for("/tmp/x.db").expect("route");
        assert_eq!(backend, Backend::Sqlite);
        assert_eq!(path, "/tmp/x.db");
    }

    /// The same file named three ways is the same store.
    #[test]
    fn the_sqlite_prefixes_all_mean_the_same_file() {
        for spec in ["/tmp/x.db", "sqlite:/tmp/x.db", "file:/tmp/x.db"] {
            let (backend, path) = backend_for(spec).expect("route");
            assert_eq!(backend, Backend::Sqlite, "{spec}");
            assert_eq!(path, "/tmp/x.db", "{spec}");
        }
    }

    /// A URL is a URL, however it was spelled. Writing a file named after one would be
    /// silent nonsense.
    #[test]
    fn a_postgres_url_is_never_mistaken_for_a_path() {
        for spec in [
            "postgres://u:p@h:5432/db",
            "postgresql://u:p@h:5432/db",
            "pg:postgres://u:p@h:5432/db",
            "postgres:postgres://u:p@h:5432/db",
        ] {
            let (backend, _) = backend_for(spec).expect("route");
            assert_eq!(backend, Backend::Postgres, "{spec}");
        }
    }

    #[test]
    fn an_empty_store_location_is_refused() {
        assert!(backend_for("   ").is_err());
        assert!(backend_for("sqlite:").is_err());
        assert!(backend_for("postgres:").is_err());
    }

    /// Opening really connects, so the routing test above is the one that can run
    /// without a database -- and this one checks that opening actually happens.
    #[test]
    fn opening_a_sqlite_store_creates_it() {
        let p = std::env::temp_dir().join("tiny_pdg_store_open_test.db");
        let _ = std::fs::remove_file(&p);
        let store = open_store(p.to_str().unwrap()).expect("open");
        assert_eq!(store.kind(), "sqlite");
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn a_missing_directory_is_created() {
        let dir = std::env::temp_dir().join("tiny_pdg_store_mkdir/nested");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("x.db");
        let _ = open_store(path.to_str().unwrap()).expect("must create the directory");
        assert!(dir.exists(), "the parent directory should have been made");
        std::fs::remove_dir_all(&dir).ok();
    }
}
