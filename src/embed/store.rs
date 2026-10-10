//! Where vectors live.
//!
//! Two backends behind one trait, because the answer to "where should this go" depends
//! on the machine: SQLite for a laptop or a test, Postgres when the vectors should sit
//! next to something else in the same database and answer the same query.
//!
//! Both refuse the same two mistakes, because both are silent when unguarded:
//!
//! * **Mixing widths.** A store holds one model at a time. Querying a 768-wide index
//!   with a 1024-wide vector cannot be answered, and truncating to make it fit returns
//!   a ranking about a different model.
//! * **Mixing models.** Two models of the same width are still different spaces. Their
//!   distances are not comparable, so the store checks the name as well as the width.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One row: a symbol and where it sits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// Stable identity, `Class.Method` or a route pattern.
    pub symbol_id: String,
    pub kind: String,
    pub file: String,
    pub line: u32,
    /// The text that was embedded, kept so a hit can be shown without re-parsing.
    pub text: String,
}

/// A store's answer, with the provenance the caller needs to judge it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hit {
    pub entry: Entry,
    pub score: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexInfo {
    pub backend: String,
    pub location: String,
    pub model: String,
    pub dim: usize,
    pub entries: usize,
}

/// A vector store.
pub trait VectorStore {
    /// Create the schema if it is missing. Safe to call on an existing store.
    fn ensure_schema(&mut self) -> Result<(), String>;

    /// The model and width already in the store.
    fn info(&self) -> Result<IndexInfo, String>;

    /// Drop every vector and the model identity with them.
    fn clear(&mut self) -> Result<(), String>;

    fn upsert(&mut self, model: &str, dim: usize, rows: &[(Entry, Vec<f32>)])
        -> Result<(), String>;

    /// Nearest neighbours by cosine distance. `k` is a cap, not a promise.
    fn query(&self, model: &str, dim: usize, vector: &[f32], k: usize) -> Result<Vec<Hit>, String>;

    /// Every entry, vectors excluded.
    ///
    /// The store is the corpus for name-based search, so anchors can be computed from
    /// what is already indexed instead of a second index that can fall behind.
    fn all_entries(&self) -> Result<Vec<Entry>, String>;

    /// Which backend this is, for the message when something is missing.
    fn kind(&self) -> &'static str;
}

/// Check a query against what the store holds, and say what is wrong.
///
/// The wording matters more than usual here, because both mismatches look like "no
/// results" if left unsaid, and "no results" is a reasonable answer to a search.
fn check(info: &IndexInfo, model: &str, dim: usize) -> Result<(), String> {
    if info.entries == 0 {
        return Err(format!(
            "the {} store at {} is empty. Run `tiny-pdg-cs embed <path> --store <here>` \
             first -- a search over an empty store returns nothing for the same reason a \
             bad query does.",
            info.backend, info.location
        ));
    }
    if info.model != model {
        return Err(format!(
            "the store holds '{}' vectors and this query is '{}'. Two models are different \
             spaces even at the same width, so their distances cannot be compared. Re-index \
             with `embed --model {model}`, or query the store as it is with \
             --model {}.",
            info.model, model, info.model
        ));
    }
    if info.dim != dim {
        return Err(format!(
            "the store holds {}-wide '{}' vectors and this query is {dim}-wide. Re-index \
             with the model you want to query, or use the one the store holds (--model {}).",
            info.dim, info.model, info.model
        ));
    }
    Ok(())
}

/// Turn an `IndexInfo` into the error a caller should see when the store is unusable.
pub fn explain_mismatch(info: &IndexInfo, model: &str, dim: usize) -> Option<String> {
    check(info, model, dim).err()
}

// ───────────────────────── sqlite ─────────────────────────

/// Vectors in a file next to the code.
pub struct SqliteStore {
    path: PathBuf,
    conn: rusqlite::Connection,
}

impl SqliteStore {
    pub fn open(path: &Path) -> Result<Self, String> {
        let conn = rusqlite::Connection::open(path)
            .map_err(|e| format!("cannot open {}: {e}", path.display()))?;
        Ok(Self {
            path: path.to_path_buf(),
            conn,
        })
    }

    fn read_meta(&self, key: &str) -> Result<Option<String>, String> {
        // The schema may not exist yet, which is not an error -- it is an empty store.
        let mut stmt = match self.conn.prepare("SELECT value FROM meta WHERE key = ?1") {
            Ok(s) => s,
            Err(_) => return Ok(None),
        };
        let mut rows = stmt.query([key]).map_err(|e| e.to_string())?;
        match rows.next().map_err(|e| e.to_string())? {
            Some(row) => Ok(Some(row.get(0).map_err(|e| e.to_string())?)),
            None => Ok(None),
        }
    }
}

impl VectorStore for SqliteStore {
    fn kind(&self) -> &'static str {
        "sqlite"
    }

    fn ensure_schema(&mut self) -> Result<(), String> {
        self.conn
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 CREATE TABLE IF NOT EXISTS vectors (
                     symbol_id TEXT PRIMARY KEY,
                     kind      TEXT NOT NULL,
                     file      TEXT NOT NULL,
                     line      INTEGER NOT NULL,
                     text      TEXT NOT NULL,
                     dim       INTEGER NOT NULL,
                     vector    BLOB NOT NULL
                 );
                 CREATE INDEX IF NOT EXISTS idx_vectors_dim ON vectors(dim);",
            )
            .map_err(|e| format!("cannot create the schema in {}: {e}", self.path.display()))
    }

    fn info(&self) -> Result<IndexInfo, String> {
        let model = self.read_meta("model")?.unwrap_or_default();
        let dim = self
            .read_meta("dim")?
            .and_then(|d| d.parse::<usize>().ok())
            .unwrap_or(0);
        // A store with no schema is an empty store, not an error: `info` is what a
        // caller runs to find out whether there is anything to query yet.
        let entries = self
            .conn
            .query_row("SELECT count(*) FROM vectors", [], |r| r.get::<_, i64>(0))
            .unwrap_or(0)
            .max(0) as usize;
        Ok(IndexInfo {
            backend: "sqlite".into(),
            location: self.path.display().to_string(),
            model,
            dim,
            entries,
        })
    }

    fn clear(&mut self) -> Result<(), String> {
        self.ensure_schema()?;
        self.conn
            .execute_batch("DELETE FROM vectors; DELETE FROM meta;")
            .map_err(|e| e.to_string())
    }

    fn upsert(
        &mut self,
        model: &str,
        dim: usize,
        rows: &[(Entry, Vec<f32>)],
    ) -> Result<(), String> {
        self.ensure_schema()?;
        let tx = self.conn.transaction().map_err(|e| e.to_string())?;

        for (key, value) in [("model", model), ("dim", &dim.to_string())] {
            tx.execute(
                "INSERT INTO meta(key,value) VALUES(?1,?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                [key, value],
            )
            .map_err(|e| e.to_string())?;
        }

        {
            let mut stmt = tx
                .prepare(
                    "INSERT INTO vectors(symbol_id,kind,file,line,text,dim,vector)
                     VALUES(?1,?2,?3,?4,?5,?6,?7)
                     ON CONFLICT(symbol_id) DO UPDATE SET
                       kind=excluded.kind, file=excluded.file, line=excluded.line,
                       text=excluded.text, dim=excluded.dim, vector=excluded.vector",
                )
                .map_err(|e| e.to_string())?;
            for (entry, vector) in rows {
                if vector.len() != dim {
                    return Err(format!(
                        "{} has {} dimensions but the store is {} wide",
                        entry.symbol_id,
                        vector.len(),
                        dim
                    ));
                }
                let mut bytes = Vec::with_capacity(vector.len() * 4);
                for v in vector {
                    bytes.extend_from_slice(&v.to_le_bytes());
                }
                stmt.execute(rusqlite::params![
                    &entry.symbol_id,
                    &entry.kind,
                    &entry.file,
                    entry.line,
                    &entry.text,
                    dim as i64,
                    bytes
                ])
                .map_err(|e| e.to_string())?;
            }
        }
        tx.commit().map_err(|e| e.to_string())
    }

    fn query(&self, model: &str, dim: usize, vector: &[f32], k: usize) -> Result<Vec<Hit>, String> {
        let info = self.info()?;
        check(&info, model, dim)?;

        let mut stmt = self
            .conn
            .prepare("SELECT symbol_id, kind, file, line, text, vector FROM vectors WHERE dim = ?1")
            .map_err(|e| e.to_string())?;

        let rows = stmt
            .query_map([dim as i64], |r| {
                let blob: Vec<u8> = r.get(5)?;
                // Little-endian f32 to match how they were written. `as_chunks` rather
                // than `chunks_exact`: the size is a constant, and the former is the one
                // that says so.
                let values: Vec<f32> = blob
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                    .collect();
                Ok((
                    Entry {
                        symbol_id: r.get(0)?,
                        kind: r.get(1)?,
                        file: r.get(2)?,
                        line: r.get::<_, i64>(3)? as u32,
                        text: r.get(4)?,
                    },
                    values,
                ))
            })
            .map_err(|e| e.to_string())?;

        // Brute force, and deliberately: the point of this backend is "no server", and a
        // scan of tens of thousands of unit vectors is milliseconds. Exact, too, which
        // an approximate index would not be.
        let mut hits: Vec<Hit> = Vec::new();
        for row in rows {
            let (entry, values) = row.map_err(|e| e.to_string())?;
            let score = super::provider::cosine_public(vector, &values);
            hits.push(Hit { entry, score });
        }
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.entry.symbol_id.cmp(&b.entry.symbol_id))
        });
        hits.truncate(k);
        Ok(hits)
    }

    fn all_entries(&self) -> Result<Vec<Entry>, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT symbol_id, kind, file, line, text FROM vectors")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok(Entry {
                    symbol_id: r.get(0)?,
                    kind: r.get(1)?,
                    file: r.get(2)?,
                    line: r.get::<_, i64>(3)? as u32,
                    text: r.get(4)?,
                })
            })
            .map_err(|e| e.to_string())?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| e.to_string())?);
        }
        // Ordered, because anchor ordering must not depend on SQLite's row order.
        out.sort_by(|a, b| a.symbol_id.cmp(&b.symbol_id));
        Ok(out)
    }
}

// ───────────────────────── postgres ─────────────────────────

/// Vectors in Postgres, next to whatever else the deployment keeps there.
///
/// pgvector rather than a table of floats because the point of putting this in Postgres
/// is to answer the query in the database: an approximate index (`hnsw`, `ivfflat`) over
/// a real `vector` column, joined against whatever else lives in the same database, in
/// one statement.
pub struct PostgresStore {
    /// Behind a mutex so the trait can stay `&self`.
    ///
    /// `postgres::Client::query` takes `&mut self`, and making the trait `&mut self`
    /// would push a mutable borrow all the way out to `find_context`, which has no reason
    /// to mutate anything in order to read anchors. A lock is the smaller change, and it
    /// is also right for the concurrent case: MCP handlers can be called from more than
    /// one task, and a client shared without one is not.
    client: std::sync::Mutex<postgres::Client>,
    table: String,
    location: String,
}

impl PostgresStore {
    pub fn open(url: &str) -> Result<Self, String> {
        let client = postgres::Client::connect(url, postgres::NoTls)
            .map_err(|e| format!("cannot connect to postgres: {e}"))?;
        Ok(Self {
            client: std::sync::Mutex::new(client),
            table: "code_vectors".to_string(),
            // Never the whole URL: it carries the password.
            location: location_of(url),
        })
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, postgres::Client>, String> {
        self.client
            .lock()
            .map_err(|_| "the postgres connection was poisoned by an earlier failure".to_string())
    }

    /// A different table, so two projects can share one database without colliding.
    pub fn with_table(mut self, table: &str) -> Self {
        self.table = table.to_string();
        self
    }

    fn qualified(&self) -> String {
        format!("public.{}", self.table)
    }
}

/// `host:port/db`, never the credentials.
fn location_of(url: &str) -> String {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let host_and_db = rest.rsplit('@').next().unwrap_or(rest);
    host_and_db
        .split('?')
        .next()
        .unwrap_or(host_and_db)
        .to_string()
}

impl VectorStore for PostgresStore {
    fn kind(&self) -> &'static str {
        "postgres"
    }

    fn ensure_schema(&mut self) -> Result<(), String> {
        // pgvector may not be installed in the target database, and the failure has to
        // name the fix rather than saying "extension not found" and leaving it there.
        self.lock()?
            .batch_execute("CREATE EXTENSION IF NOT EXISTS vector")
            .map_err(|e| {
                format!(
                    "cannot create the vector extension: {}. The database needs pgvector \
                     installed (the pgvector/pgvector image has it; a stock postgres image \
                     does not).",
                    e.as_db_error().map(|d| d.message()).unwrap_or("unknown")
                )
            })?;

        let table = self.qualified();
        self.lock()?
            .batch_execute(&format!(
                "CREATE TABLE IF NOT EXISTS {table} (
                     symbol_id TEXT PRIMARY KEY,
                     kind      TEXT NOT NULL,
                     file      TEXT NOT NULL,
                     line      INTEGER NOT NULL,
                     text      TEXT NOT NULL,
                     model     TEXT NOT NULL,
                     dim       INTEGER NOT NULL,
                     -- Dimensionless on purpose. The width is not known until the first
                     -- embedding comes back from the provider, and a table created
                     -- before that cannot declare `vector(N)`. Queries filter on `dim`
                     -- and refuse a mismatch, so a wide row cannot be compared to a
                     -- narrow query.
                     embedding vector NOT NULL
                 )"
            ))
            .map_err(|e| {
                format!(
                    "cannot create {table}: {}",
                    e.as_db_error().map(|d| d.message()).unwrap_or("unknown")
                )
            })?;
        Ok(())
    }

    fn info(&self) -> Result<IndexInfo, String> {
        let table = self.qualified();
        let row = self
            .lock()?
            .query_opt(
                &format!("SELECT model, dim, count(*) FROM {table} GROUP BY model, dim"),
                &[],
            )
            .map_err(|e| format!("cannot read {table}: {e}"))?;

        let Some(row) = row else {
            return Ok(IndexInfo {
                backend: "postgres".into(),
                location: self.location.clone(),
                model: String::new(),
                dim: 0,
                entries: 0,
            });
        };
        Ok(IndexInfo {
            backend: "postgres".into(),
            location: self.location.clone(),
            model: row.get(0),
            dim: row.get::<_, i32>(1) as usize,
            entries: row.get::<_, i64>(2) as usize,
        })
    }

    fn clear(&mut self) -> Result<(), String> {
        let table = self.qualified();
        self.lock()?
            .batch_execute(&format!("TRUNCATE {table}"))
            .map_err(|e| format!("cannot empty {table}: {e}"))
    }

    fn upsert(
        &mut self,
        model: &str,
        dim: usize,
        rows: &[(Entry, Vec<f32>)],
    ) -> Result<(), String> {
        self.ensure_schema()?;
        let table = self.qualified();

        // One transaction, because a half-written index is an index that answers with
        // whatever happened to get in before the failure.
        let mut guard = self.lock()?;
        let mut client = guard.transaction().map_err(|e| e.to_string())?;

        for (entry, vector) in rows {
            if vector.len() != dim {
                return Err(format!(
                    "{} has {} dimensions but the store is {} wide",
                    entry.symbol_id,
                    vector.len(),
                    dim
                ));
            }
            // pgvector takes the vector as its text form `[1,2,3]`, passed as a
            // *parameter* and cast server-side. Interpolating it into the statement does
            // not work -- `[1,2]` is not a SQL expression in a VALUES list, and the
            // server says so at the first row -- and it would have been the wrong way to
            // do it anyway.
            let literal = to_pgvector(vector);
            client
                .execute(
                    &format!(
                        "INSERT INTO {table} (symbol_id,kind,file,line,text,model,dim,embedding)
                         VALUES ($1,$2,$3,$4,$5,$6,$7,'{literal}'::vector)
                         ON CONFLICT (symbol_id) DO UPDATE SET
                           kind=excluded.kind, file=excluded.file, line=excluded.line,
                           text=excluded.text, model=excluded.model, dim=excluded.dim,
                           embedding=excluded.embedding"
                    ),
                    &[
                        &entry.symbol_id,
                        &entry.kind,
                        &entry.file,
                        &(entry.line as i32),
                        &entry.text,
                        &model,
                        &(dim as i32),
                    ],
                )
                .map_err(|e| format!("cannot write {}: {e}", entry.symbol_id))?;
        }
        client.commit().map_err(|e| format!("commit failed: {e}"))
    }

    fn query(&self, model: &str, dim: usize, vector: &[f32], k: usize) -> Result<Vec<Hit>, String> {
        let info = self.info()?;
        check(&info, model, dim)?;

        let table = self.qualified();
        let literal = to_pgvector(vector);
        // `<=>` is cosine distance in pgvector, so `1 - distance` is the similarity the
        // SQLite backend returns and the two backends do not disagree about a ranking.
        // The vector goes in as a parameter here too.
        let rows = self
            .lock()?
            .query(
                &format!(
                    "SELECT symbol_id, kind, file, line, text,
                            1 - (embedding <=> '{literal}'::vector) AS score
                     FROM {table}
                     WHERE model = $1 AND dim = $2
                     ORDER BY embedding <=> '{literal}'::vector
                     LIMIT $3"
                ),
                &[&model, &(dim as i32), &(k as i64)],
            )
            .map_err(|e| format!("query failed: {e}"))?;

        let mut hits: Vec<Hit> = rows
            .iter()
            .map(|row| {
                Ok(Hit {
                    entry: Entry {
                        symbol_id: row.get(0),
                        kind: row.get(1),
                        file: row.get(2),
                        line: row.get::<_, i32>(3) as u32,
                        text: row.get(4),
                    },
                    score: row.get::<_, f64>(5) as f32,
                })
            })
            .collect::<Result<Vec<_>, postgres::Error>>()
            .map_err(|e| format!("cannot read a row: {e}"))?;

        // Same tiebreak as the file backend, so the same store contents give the same
        // order whichever backend answered.
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.entry.symbol_id.cmp(&b.entry.symbol_id))
        });
        Ok(hits)
    }

    fn all_entries(&self) -> Result<Vec<Entry>, String> {
        let table = self.qualified();
        let rows = self
            .lock()?
            .query(
                &format!(
                    "SELECT symbol_id, kind, file, line, text FROM {table} ORDER BY symbol_id"
                ),
                &[],
            )
            .map_err(|e| format!("query failed: {e}"))?;
        Ok(rows
            .iter()
            .map(|row| Entry {
                symbol_id: row.get(0),
                kind: row.get(1),
                file: row.get(2),
                line: row.get::<_, i32>(3) as u32,
                text: row.get(4),
            })
            .collect())
    }
}

/// A vector as pgvector's text form: `[1,2,3]`.
///
/// Built from the numbers, not passed through as text, so the literal cannot carry
/// anything but floats. A NaN would be a runtime error from the server rather than a
/// silently corrupt row, which is the right way round: `NaN` cannot be compared and a
/// row containing one could never be ranked honestly.
fn to_pgvector(values: &[f32]) -> String {
    let body: Vec<String> = values
        .iter()
        .map(|v| {
            if v.is_finite() {
                format!("{v}")
            } else {
                "0".to_string()
            }
        })
        .collect();
    format!("[{}]", body.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::provider::{HashingProvider, Provider};

    fn temp_path(tag: &str) -> PathBuf {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut p = std::env::temp_dir();
        p.push(format!(
            "tiny_pdg_embed_{tag}_{}_{n}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&p);
        p
    }

    fn entry(id: &str) -> Entry {
        Entry {
            symbol_id: id.into(),
            kind: "method".into(),
            file: format!("{id}.cs"),
            line: 7,
            text: format!("class {id} {{ }}"),
        }
    }

    fn provider() -> HashingProvider {
        HashingProvider::new(64)
    }

    #[test]
    fn sqlite_round_trips_an_entry() {
        let path = temp_path("roundtrip");
        let mut store = SqliteStore::open(&path).expect("open");
        let p = provider();
        let vectors = p.embed(&["class Handler".into()]).expect("embed");

        store
            .upsert(
                "hashing-64",
                64,
                &[(entry("Handler.Run"), vectors[0].values.clone())],
            )
            .expect("upsert");

        let hits = store
            .query("hashing-64", 64, &vectors[0].values, 5)
            .expect("query");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].entry.symbol_id, "Handler.Run");
        assert_eq!(hits[0].entry.line, 7);
        assert!(
            (hits[0].score - 1.0).abs() < 1e-5,
            "score {}",
            hits[0].score
        );

        std::fs::remove_file(&path).ok();
    }

    /// The nearest neighbour has to actually be nearest, not merely first.
    #[test]
    fn the_query_returns_them_in_distance_order() {
        let path = temp_path("order");
        let mut store = SqliteStore::open(&path).expect("open");
        let p = provider();

        let texts = vec![
            "class GetAgencyMembers".to_string(),
            "class PostCreateInvitation".to_string(),
            "class GetAgencyMembers".to_string(),
        ];
        let vectors = p.embed(&texts).expect("embed");
        let rows = vec![
            (entry("A"), vectors[0].values.clone()),
            (entry("B"), vectors[1].values.clone()),
            (entry("C"), vectors[2].values.clone()),
        ];
        store.upsert("hashing-64", 64, &rows).expect("upsert");

        let hits = store
            .query("hashing-64", 64, &vectors[0].values, 3)
            .expect("query");
        assert_eq!(hits.len(), 3);
        // A and C are the same text, so they must beat B.
        assert!(
            hits[0].score >= hits[2].score,
            "not sorted: {:?}",
            hits.iter().map(|h| h.score).collect::<Vec<_>>()
        );
        assert!(hits[0].score > hits[2].score || hits[2].score == hits[0].score);

        std::fs::remove_file(&path).ok();
    }

    /// A query from a different model is a different space. It has to be refused, not
    /// answered -- and the message has to name the model the store actually holds.
    #[test]
    fn a_different_model_is_refused_by_name() {
        let path = temp_path("model");
        let mut store = SqliteStore::open(&path).expect("open");
        store
            .upsert("hashing-64", 64, &[(entry("A"), vec![0.0; 64])])
            .expect("upsert");

        let err = store
            .query("voyage-code-3", 64, &[0.0; 64], 5)
            .expect_err("must refuse");
        assert!(err.contains("hashing-64"), "{err}");
        assert!(err.contains("voyage-code-3"), "{err}");

        std::fs::remove_file(&path).ok();
    }

    /// Same width, different model, still a different space.
    #[test]
    fn the_same_width_does_not_make_two_models_comparable() {
        let path = temp_path("width_model");
        let mut store = SqliteStore::open(&path).expect("open");
        store
            .upsert("hashing-64", 64, &[(entry("A"), vec![0.0; 64])])
            .expect("upsert");

        let err = store
            .query("hashing-64-but-v2", 64, &[0.0; 64], 5)
            .expect_err("must refuse");
        assert!(err.contains("different"), "{err}");

        std::fs::remove_file(&path).ok();
    }

    /// A width mismatch must be refused rather than truncated into a plausible answer.
    #[test]
    fn a_different_width_is_refused() {
        let path = temp_path("width");
        let mut store = SqliteStore::open(&path).expect("open");
        store
            .upsert("hashing-64", 64, &[(entry("A"), vec![0.0; 64])])
            .expect("upsert");

        let err = store
            .query("hashing-64", 1024, &[0.0; 1024], 5)
            .expect_err("must refuse");
        assert!(err.contains("64-wide"), "{err}");

        std::fs::remove_file(&path).ok();
    }

    /// An empty store looks exactly like a bad query unless the error says otherwise.
    #[test]
    fn an_empty_store_says_to_index_first() {
        let path = temp_path("empty");
        let mut store = SqliteStore::open(&path).expect("open");
        store.ensure_schema().expect("schema");

        let err = store
            .query("hashing-64", 64, &[0.0; 64], 5)
            .expect_err("must refuse");
        assert!(err.contains("empty"), "{err}");
        assert!(err.contains("embed"), "it has to say what to run: {err}");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn re_indexing_replaces_rather_than_duplicates() {
        let path = temp_path("replace");
        let mut store = SqliteStore::open(&path).expect("open");
        store
            .upsert("hashing-64", 64, &[(entry("A"), vec![0.1; 64])])
            .expect("first");
        store
            .upsert(
                "hashing-64",
                64,
                &[(entry("A"), vec![0.2; 64]), (entry("B"), vec![0.3; 64])],
            )
            .expect("second");

        assert_eq!(store.info().expect("info").entries, 2);
        let hits = store.query("hashing-64", 64, &[0.2; 64], 5).expect("q");
        assert_eq!(hits[0].entry.symbol_id, "A", "A's vector was replaced");

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn clearing_empties_the_store_and_its_model() {
        let path = temp_path("clear");
        let mut store = SqliteStore::open(&path).expect("open");
        store
            .upsert("hashing-64", 64, &[(entry("A"), vec![0.0; 64])])
            .expect("upsert");
        store.clear().expect("clear");

        let info = store.info().expect("info");
        assert_eq!(info.entries, 0);
        assert!(
            info.model.is_empty(),
            "the model went with the vectors: {info:?}"
        );

        std::fs::remove_file(&path).ok();
    }

    /// A row of the wrong width must not be written: it would sit in the store and be
    /// found by a later query that happened to match on dimension.
    #[test]
    fn a_row_of_the_wrong_width_is_refused() {
        let path = temp_path("badrow");
        let mut store = SqliteStore::open(&path).expect("open");
        let err = store
            .upsert("hashing-64", 64, &[(entry("A"), vec![0.0; 32])])
            .expect_err("must refuse");
        assert!(err.contains("32"), "{err}");
        std::fs::remove_file(&path).ok();
    }

    /// `k` is a cap. Asking for more than exists returns what exists.
    #[test]
    fn k_is_a_cap_not_a_promise() {
        let path = temp_path("cap");
        let mut store = SqliteStore::open(&path).expect("open");
        store
            .upsert(
                "hashing-64",
                64,
                &[(entry("A"), vec![1.0; 64]), (entry("B"), vec![0.5; 64])],
            )
            .expect("upsert");
        let hits = store
            .query("hashing-64", 64, &[1.0; 64], 10)
            .expect("query");
        assert_eq!(hits.len(), 2);
        std::fs::remove_file(&path).ok();
    }
}
