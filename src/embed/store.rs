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
use std::collections::BTreeMap;
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

/// One directed edge in the code graph: `from` stands in `relation` `to`.
///
/// Separate from the vectors because it answers a different question. A vector answers
/// "does this mean the same thing"; an edge answers "is this connected to that". Only
/// the second one knows that a record and the method that charges a balance are related
/// at all, and that is the relation a name search can never find -- measured, the query
/// "charge a customer\'s credit balance" returned `CreditTransactionResult`, a noun,
/// while `AddCreditCommandHandler.HandleAsync` sat sixth.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    pub from: String,
    pub to: String,
    /// `declares` for a type and its methods, `calls` for a call site.
    pub relation: String,
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

    /// Replace the graph with these edges.
    ///
    /// Replace, not merge: the graph describes the same project the vectors describe,
    /// and a re-index that left the old edges behind would answer about a codebase that
    /// no longer exists.
    fn put_edges(&mut self, edges: &[Edge]) -> Result<(), String>;

    /// The outgoing edges of each id, in one round trip.
    ///
    /// One, because expansion asks this about every anchor at once and Postgres charges
    /// a network trip per call.
    fn neighbours_of(&self, ids: &[&str]) -> Result<BTreeMap<String, Vec<Edge>>, String>;

    /// How many edges the store holds. Zero on a store indexed before the graph existed.
    fn edge_count(&self) -> Result<usize, String>;
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
                 CREATE INDEX IF NOT EXISTS idx_vectors_dim ON vectors(dim);
                 -- `from_id` rather than `from`: FROM is a keyword, and quoting it in
                 -- every query is a mistake waiting for the one place that forgets.
                 CREATE TABLE IF NOT EXISTS graph (
                     from_id   TEXT NOT NULL,
                     to_id     TEXT NOT NULL,
                     relation  TEXT NOT NULL,
                     PRIMARY KEY (from_id, to_id, relation)
                 );
                 CREATE INDEX IF NOT EXISTS idx_graph_from ON graph(from_id);",
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
            .execute_batch("DELETE FROM vectors; DELETE FROM graph; DELETE FROM meta;")
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

    fn put_edges(&mut self, edges: &[Edge]) -> Result<(), String> {
        if edges.is_empty() {
            return Ok(());
        }
        self.ensure_schema()?;
        let tx = self.conn.transaction().map_err(|e| e.to_string())?;
        {
            let mut stmt = tx
                .prepare(
                    "INSERT INTO graph(from_id,to_id,relation) VALUES(?1,?2,?3)
                     ON CONFLICT(from_id,to_id,relation) DO NOTHING",
                )
                .map_err(|e| e.to_string())?;
            for edge in edges {
                stmt.execute(rusqlite::params![&edge.from, &edge.to, &edge.relation])
                    .map_err(|e| {
                        format!("cannot write the edge {} -> {}: {e}", edge.from, edge.to)
                    })?;
            }
        }
        tx.commit().map_err(|e| e.to_string())
    }

    fn neighbours_of(&self, ids: &[&str]) -> Result<BTreeMap<String, Vec<Edge>>, String> {
        let mut out: BTreeMap<String, Vec<Edge>> = BTreeMap::new();
        if ids.is_empty() {
            return Ok(out);
        }
        // Chunked rather than one statement with a parameter per id: SQLite's limit on
        // bound variables is fixed and low enough that a long anchor list would fail,
        // and failing on a long list is how a feature looks broken instead of bounded.
        for chunk in ids.chunks(200) {
            let placeholders = vec!["?"; chunk.len()].join(",");
            // Both directions, normalised so `to` is always the neighbour and `from` is
            // always the symbol asked about.
            //
            // One direction is not enough, and it fails in exactly the case expansion is
            // for. A record like `Credit` has no outgoing calls -- nothing calls a record
            // -- so following edges forwards from it finds nothing, while "which methods
            // use this" is the question worth asking. Measured: the top three anchors for
            // "charge a customer's credit balance" had zero edges in either direction
            // before this, because only one direction was ever looked up.
            // The direction comes back from the query rather than being re-derived from
            // the two ids. Deriving it means comparing them, and when both ends are asked
            // about that comparison cannot say which is which: the first version did
            // exactly that and reversed every relation it was handed.
            let sql = format!(
                "SELECT from_id, to_id, relation, 0 FROM graph WHERE from_id IN ({placeholders}) \
                 UNION ALL \
                 SELECT to_id, from_id, relation, 1 FROM graph WHERE to_id IN ({placeholders})"
            );
            let mut stmt = self.conn.prepare(&sql).map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map(
                    rusqlite::params_from_iter(chunk.iter().chain(chunk.iter())),
                    |r| {
                        let from: String = r.get(0)?;
                        let to: String = r.get(1)?;
                        let relation: String = r.get(2)?;
                        let reversed: i64 = r.get(3)?;
                        Ok(Edge {
                            // Only the relation carries the direction, so the caller never has
                            // to re-derive it from the edge.
                            relation: if reversed == 1 {
                                reverse_relation(&relation)
                            } else {
                                relation
                            },
                            from,
                            to,
                        })
                    },
                )
                .map_err(|e| e.to_string())?;
            for edge in rows.flatten() {
                if edge.from == edge.to {
                    continue;
                }
                out.entry(edge.from.clone()).or_default().push(edge);
            }
        }
        for edges in out.values_mut() {
            // Sorted so the same query returns the same expansion twice.
            edges.sort_by(|a, b| a.to.cmp(&b.to).then_with(|| a.relation.cmp(&b.relation)));
        }
        Ok(out)
    }

    fn edge_count(&self) -> Result<usize, String> {
        Ok(self
            .conn
            .query_row("SELECT count(*) FROM graph", [], |r| r.get::<_, i64>(0))
            .unwrap_or(0)
            .max(0) as usize)
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

    /// The graph lives beside the vectors rather than inside them: one project's
    /// vectors and edges have the same lifetime, so a suffix on the same base name
    /// keeps them together without a second table name to configure.
    fn qualified_graph(&self) -> String {
        format!("public.{}_graph", self.table)
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

        let graph = self.qualified_graph();
        self.lock()?
            .batch_execute(&format!(
                "CREATE TABLE IF NOT EXISTS {graph} (
                     from_id   TEXT NOT NULL,
                     to_id     TEXT NOT NULL,
                     relation  TEXT NOT NULL,
                     PRIMARY KEY (from_id, to_id, relation)
                 )"
            ))
            .map_err(|e| {
                format!(
                    "cannot create {graph}: {}",
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
        let graph = self.qualified_graph();
        self.lock()?
            .batch_execute(&format!("TRUNCATE {table}; TRUNCATE {graph}"))
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

    fn put_edges(&mut self, edges: &[Edge]) -> Result<(), String> {
        if edges.is_empty() {
            return Ok(());
        }
        self.ensure_schema()?;
        let graph = self.qualified_graph();

        let mut guard = self.lock()?;
        let mut client = guard.transaction().map_err(|e| e.to_string())?;
        for edge in edges {
            client
                .execute(
                    &format!(
                        "INSERT INTO {graph} (from_id,to_id,relation) VALUES ($1,$2,$3)
                         ON CONFLICT DO NOTHING"
                    ),
                    &[&edge.from, &edge.to, &edge.relation],
                )
                .map_err(|e| format!("cannot write the edge {} -> {}: {e}", edge.from, edge.to))?;
        }
        client.commit().map_err(|e| e.to_string())
    }

    fn neighbours_of(&self, ids: &[&str]) -> Result<BTreeMap<String, Vec<Edge>>, String> {
        let mut out: BTreeMap<String, Vec<Edge>> = BTreeMap::new();
        if ids.is_empty() {
            return Ok(out);
        }
        let graph = self.qualified_graph();

        // Chunked for the same reason SQLite is: a long anchor list must not depend on
        // the parameter cap of whichever server happens to be there.
        for chunk in ids.chunks(200) {
            let mut params: Vec<&(dyn postgres::types::ToSql + Sync)> = Vec::new();
            for id in chunk {
                params.push(id);
            }
            let placeholders: Vec<String> = (1..=chunk.len()).map(|n| format!("${n}")).collect();
            let sql = format!(
                "SELECT from_id, to_id, relation, 0 FROM {graph} WHERE from_id IN ({fwd}) \
                 UNION ALL \
                 SELECT to_id, from_id, relation, 1 FROM {graph} WHERE to_id IN ({rev})",
                fwd = placeholders.join(","),
                rev = placeholders.join(",")
            );
            let both: Vec<&(dyn postgres::types::ToSql + Sync)> = params
                .iter()
                .copied()
                .chain(params.iter().copied())
                .collect();
            let rows = self
                .lock()?
                .query(&sql, &both)
                .map_err(|e| format!("query failed: {e}"))?;
            for row in rows {
                let from: String = row.get(0);
                let to: String = row.get(1);
                let relation: String = row.get(2);
                let reversed: i32 = row.get(3);
                if from == to {
                    continue;
                }
                let edge = Edge {
                    relation: if reversed == 1 {
                        reverse_relation(&relation)
                    } else {
                        relation
                    },
                    from,
                    to,
                };
                out.entry(edge.from.clone()).or_default().push(edge);
            }
        }
        for edges in out.values_mut() {
            edges.sort_by(|a, b| a.to.cmp(&b.to).then_with(|| a.relation.cmp(&b.relation)));
        }
        Ok(out)
    }

    fn edge_count(&self) -> Result<usize, String> {
        let graph = self.qualified_graph();
        Ok(self
            .lock()?
            .query_one(&format!("SELECT count(*) FROM {graph}"), &[])
            .map_err(|e| format!("query failed: {e}"))?
            .get::<_, i64>(0)
            .max(0) as usize)
    }
}

/// The same edge read the other way round, in words.
///
/// A relation is stored in one direction and looked up in two, so the answer has to say
/// which way it is pointing. `caller --calls--> callee` becomes `callee --called by-->
/// caller`: the store does not store a second edge, because one call site is one fact
/// and storing it twice makes "how many call sites are there" wrong.
fn reverse_relation(relation: &str) -> String {
    match relation {
        "declares" => "declared by",
        "calls" => "called by",
        "references" => "referenced by",
        other => return format!("{other} (reverse)"),
    }
    .to_string()
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

    fn with_edges(path: &Path) -> SqliteStore {
        let mut store = SqliteStore::open(path).expect("open");
        store.ensure_schema().expect("schema");
        store
            .put_edges(&[
                Edge {
                    from: "Handler".into(),
                    to: "Record".into(),
                    relation: "references".into(),
                },
                Edge {
                    from: "Type".into(),
                    to: "Handler".into(),
                    relation: "declares".into(),
                },
            ])
            .expect("edges");
        store
    }

    /// Neighbours come back in both directions, and the relation says which way.
    ///
    /// One direction is not enough, and it fails on exactly the case expansion exists
    /// for: a record has no outgoing calls, because nothing calls a record, while "which
    /// methods use this" is the question worth asking.
    #[test]
    fn neighbours_come_back_from_both_ends() {
        let path = temp_path("bothways");
        let store = with_edges(&path);

        let forward = store.neighbours_of(&["Handler"]).expect("query");
        let mut got: Vec<(String, String)> = forward["Handler"]
            .iter()
            .map(|e| (e.to.clone(), e.relation.clone()))
            .collect();
        got.sort();
        assert_eq!(
            got,
            vec![
                // Stored `Handler references Record`, read forwards.
                ("Record".to_string(), "references".to_string()),
                // Stored `Type declares Handler`, read backwards, so it reads as what
                // is true from here: the neighbour is the thing that declared this one.
                ("Type".to_string(), "declared by".to_string()),
            ],
            "one direction was lost, or reversed, or both"
        );
        std::fs::remove_file(&path).ok();
    }

    /// A symbol nobody is connected to gets an empty answer, not an error.
    #[test]
    fn a_symbol_with_no_edges_has_no_neighbours() {
        let path = temp_path("noedges");
        let store = with_edges(&path);
        let got = store.neighbours_of(&["NothingHere"]).expect("query");
        assert!(got.is_empty(), "{got:?}");
        std::fs::remove_file(&path).ok();
    }

    /// Asking about nothing is not a query.
    #[test]
    fn asking_about_nothing_costs_nothing() {
        let path = temp_path("noids");
        let store = with_edges(&path);
        assert!(store.neighbours_of(&[]).expect("query").is_empty());
        std::fs::remove_file(&path).ok();
    }

    /// Writing the same edge twice is still one edge.
    ///
    /// A call site is one fact; storing it twice makes "how many edges are there" wrong,
    /// which is the number the answer reports.
    #[test]
    fn the_same_edge_twice_is_still_one_edge() {
        let path = temp_path("dupedge");
        let mut store = with_edges(&path);
        assert_eq!(store.edge_count().expect("count"), 2);
        store
            .put_edges(&[Edge {
                from: "Handler".into(),
                to: "Record".into(),
                relation: "references".into(),
            }])
            .expect("edges");
        assert_eq!(store.edge_count().expect("count"), 2);
        std::fs::remove_file(&path).ok();
    }

    /// Emptying the store empties the graph too. A re-index that left the edges behind
    /// would answer about a codebase that no longer exists.
    #[test]
    fn clearing_removes_the_graph_as_well() {
        let path = temp_path("cleargraph");
        let mut store = with_edges(&path);
        store.clear().expect("clear");
        assert_eq!(store.edge_count().expect("count"), 0);
        std::fs::remove_file(&path).ok();
    }

    /// Reversing a relation produces words that read correctly in the answer.
    #[test]
    fn a_reversed_relation_reads_correctly() {
        assert_eq!(reverse_relation("calls"), "called by");
        assert_eq!(reverse_relation("declares"), "declared by");
        assert_eq!(reverse_relation("references"), "referenced by");
        assert_eq!(
            reverse_relation("something else"),
            "something else (reverse)"
        );
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
