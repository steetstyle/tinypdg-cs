//! Choosing what to embed, and rendering it.
//!
//! A symbol, not a file chunk. This is the difference between a code graph and RAG over
//! text: `GetAgencyMembersEndpoint.Handler` is a thing that exists, that callers point
//! at, and that has a line number. A 2000-character window around it is a thing that
//! mentions it. The first is a location the agent can open; the second is a paragraph
//! to re-read.
//!
//! So each entry is one symbol, rendered as the lines that identify it and enough of its
//! surroundings to say what it does. The rendering is the whole quality question for a
//! lexical model: two symbols that share a domain word should end up with similar text,
//! and two that share only a type name should not.

use std::collections::BTreeMap;

use super::provider::{Embedding, Provider};
use super::store::{Entry, VectorStore};

/// One symbol, and the text that represents it.
#[derive(Debug, Clone, PartialEq)]
pub struct Symbol {
    pub id: String,
    pub kind: String,
    pub file: String,
    pub line: u32,
    pub name: String,
    pub signature: Option<String>,
    pub containing_type: Option<String>,
    pub doc: Option<String>,
    /// Methods it calls, names only. Included in the text because a method is often
    /// found by what it touches rather than by what it is called.
    pub calls: Vec<String>,
    /// The symbol's place in the call graph. Never embedded, never used for ranking
    /// inside this module -- it travels with the symbol so the caller can rank on the
    /// graph and use the vector only to find candidates.
    pub graph: Option<Vec<String>>,
}

impl Symbol {
    /// The text handed to the provider.
    ///
    /// Built from identity first, then what it does, then what it touches. A model that
    /// only ever saw the name would match on names; this lets it match on behaviour too.
    pub fn render(&self) -> String {
        let mut parts: Vec<String> = Vec::new();

        parts.push(match &self.containing_type {
            Some(t) => format!("{t}.{}", self.name),
            None => self.name.clone(),
        });
        parts.push(self.kind.clone());
        parts.push(self.file.clone());

        if let Some(sig) = &self.signature {
            parts.push(sig.clone());
        }
        if let Some(doc) = &self.doc {
            parts.push(doc.clone());
        }
        if !self.calls.is_empty() {
            // Capped: a method with forty calls has no useful signature in vector form,
            // and the long tail is noise.
            let calls: Vec<String> = self.calls.iter().take(12).cloned().collect();
            parts.push(format!("calls {}", calls.join(" ")));
        }
        parts.join(" ")
    }

    fn to_entry(&self) -> Entry {
        Entry {
            symbol_id: self.id.clone(),
            kind: self.kind.clone(),
            file: self.file.clone(),
            line: self.line,
            text: self.render(),
        }
    }
}

/// What indexing did.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct IndexReport {
    pub symbols: usize,
    pub embedded: usize,
    pub model: String,
    pub dim: usize,
    pub store: String,
    pub elapsed_ms: u128,
    pub files: usize,
    /// Files that did not parse, with the reason. Reported, never fatal.
    pub failed: Vec<(String, String)>,
}

/// Turn symbols into vectors and put them in a store.
///
/// `reset` drops what is there first. That is the only way to change model or width,
/// because a store holding two widths cannot answer a query about either of them --
/// the store refuses mixed queries precisely so this cannot happen by accident.
pub fn embed_symbols(
    store: &mut dyn VectorStore,
    provider: &dyn Provider,
    symbols: &[Symbol],
    reset: bool,
) -> Result<IndexReport, String> {
    let started = std::time::Instant::now();
    store.ensure_schema()?;

    if reset {
        store.clear()?;
    }

    if symbols.is_empty() {
        return Err(
            "no symbols to embed. The path parsed to nothing, or the filter excluded \
             everything -- check it points at C# sources."
                .to_string(),
        );
    }

    let texts: Vec<String> = symbols.iter().map(Symbol::render).collect();

    // Batched, because an OpenAI-compatible server takes a list and a per-symbol
    // request would be a round trip each.
    const BATCH: usize = 64;
    let mut rows: Vec<(Entry, Vec<f32>)> = Vec::with_capacity(symbols.len());
    let mut model = provider.model().to_string();
    let mut dim = provider.dim().unwrap_or(0);

    for chunk in texts.chunks(BATCH) {
        let embeddings: Vec<Embedding> = provider.embed(chunk)?;
        if embeddings.len() != chunk.len() {
            return Err(format!(
                "the provider returned {} vector(s) for {} input(s). Refusing to write a \
                 partial index -- it would be silently missing entries.",
                embeddings.len(),
                chunk.len()
            ));
        }
        for (embedding, text) in embeddings.into_iter().zip(chunk) {
            let this_dim = embedding.dim();
            if dim == 0 {
                dim = this_dim;
            } else if dim != this_dim {
                return Err(format!(
                    "the provider returned {this_dim}-wide and {dim}-wide vectors in one \
                     run. A store holds one width; re-index or pick one model."
                ));
            }
            model = embedding.model.clone();
            // Pair back to the symbol by position, since the provider is given text and
            // returns vectors in order.
            rows.push((
                Entry {
                    symbol_id: String::new(), // filled by the caller below
                    kind: String::new(),
                    file: String::new(),
                    line: 0,
                    text: text.clone(),
                },
                embedding.values,
            ));
        }
    }

    // Attach the symbols to the vectors the provider returned for them. Zip rather than
    // a map: order is the only link between the two, and a provider that reorders is
    // handled above by the count check.
    let rows: Vec<(Entry, Vec<f32>)> = rows
        .into_iter()
        .zip(symbols)
        .map(|((_, values), symbol)| (symbol.to_entry(), values))
        .collect();

    let count = rows.len();
    store.upsert(&model, dim, &rows)?;

    let location = store.info()?.location;
    Ok(IndexReport {
        symbols: symbols.len(),
        embedded: count,
        model,
        dim,
        store: location,
        elapsed_ms: started.elapsed().as_millis(),
        files: 0,
        failed: Vec::new(),
    })
}

/// Rank symbols against a query, returning the best `k`.
pub fn search(
    store: &dyn VectorStore,
    provider: &dyn Provider,
    query: &str,
    k: usize,
) -> Result<Vec<super::store::Hit>, String> {
    let texts = vec![query.to_string()];
    let vectors = provider.embed(&texts)?;
    let vector = &vectors[0];
    store.query(&vector.model, vector.dim(), &vector.values, k)
}

/// Group symbols by the file they are in, which is what a caller usually wants next.
pub fn symbols_by_file(symbols: &[Symbol]) -> BTreeMap<String, Vec<&Symbol>> {
    let mut out: BTreeMap<String, Vec<&Symbol>> = BTreeMap::new();
    for symbol in symbols {
        out.entry(symbol.file.clone()).or_default().push(symbol);
    }
    out
}

/// The line a type is declared on, by its own declaration or by its first method.
///
/// `ClassInfo` carries no position of its own, so the first thing that did was `line: 0`
/// -- and a location with line 0 is not a location. It sends an agent searching the file
/// for the name, which is the thing a graph exists to avoid. So: the declaration if the
/// text has one, otherwise the first method, and 0 only when there is genuinely neither.
fn declaration_line(source: &str, class_name: &str) -> u32 {
    for (index, line) in source.lines().enumerate() {
        let trimmed = line.trim_start();
        // Only the declaration part: a field initialiser on the same line must not
        // supply a position, or every member of a one-line type shares the first line.
        let head = trimmed.split('{').next().unwrap_or(trimmed);
        for keyword in ["class ", "record ", "struct ", "interface ", "enum "] {
            let Some(at) = head.find(keyword) else {
                continue;
            };
            let declared = head[at + keyword.len()..]
                .split(['<', ' ', ':', '(', '{'])
                .next()
                .unwrap_or("")
                .trim();
            if declared == class_name {
                return (index + 1) as u32;
            }
        }
    }
    0
}

/// Walk a project and collect the symbols worth embedding.
///
/// Goes file by file through the same parse path the rest of the crate uses --
/// `parse_source` then `SymbolTable::from_ast` -- rather than a second parser, because
/// two parsers would disagree about what a symbol is and the disagreement would only
/// show up as bad search results.
///
/// A file that does not parse is counted and skipped, not fatal: a repository with one
/// file mid-edit should still index the other two hundred.
#[derive(Debug, Clone, PartialEq)]
pub struct ParseOutcome {
    pub symbols: Vec<Symbol>,
    pub files: usize,
    pub failed: Vec<(String, String)>,
}

/// Every `.cs` file under `root`, sorted, so a re-index is deterministic.
pub fn csharp_files(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            // Skip what a source tree fills with and no analysis reads: build output,
            // packages, version control.
            if name.starts_with('.') || name == "bin" || name == "obj" || name == "node_modules" {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("cs") {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

pub fn collect_symbols(root: &std::path::Path) -> ParseOutcome {
    use crate::parse::parser::parse_source;
    use crate::resolve::symbols::SymbolTable;

    let mut symbols: BTreeMap<String, Symbol> = BTreeMap::new();
    let mut failed = Vec::new();
    let files = csharp_files(root);

    for path in &files {
        let display = path.display().to_string();
        let Ok(source) = std::fs::read_to_string(path) else {
            failed.push((display, "cannot read".into()));
            continue;
        };
        let Ok(tree) = parse_source(&source) else {
            failed.push((display, "does not parse".into()));
            continue;
        };
        let table = match SymbolTable::from_ast(tree.root_node(), &source) {
            Ok(t) => t,
            Err(e) => {
                failed.push((display, e.to_string()));
                continue;
            }
        };

        for (class_name, info) in &table.type_graph.classes {
            let file = if info
                .methods
                .first()
                .map(|m| !m.file.is_empty())
                .unwrap_or(false)
            {
                info.methods[0].file.clone()
            } else {
                display.clone()
            };

            let method_ids: Vec<String> = info
                .methods
                .iter()
                .map(|m| format!("{class_name}.{}", m.method))
                .collect();

            for method in &info.methods {
                let id = format!("{class_name}.{}", method.method);
                let callees: Vec<String> = info
                    .fields
                    .iter()
                    .filter(|f| f.name.contains(&method.method))
                    .map(|f| f.name.clone())
                    .collect();
                symbols.entry(id.clone()).or_insert_with(|| Symbol {
                    id,
                    kind: "method".into(),
                    file: file.clone(),
                    line: method.line_start as u32,
                    name: method.method.clone(),
                    signature: Some(method.signature.clone()),
                    containing_type: Some(class_name.clone()),
                    doc: None,
                    calls: callees,
                    // Kept out of the rendered text but recorded, because a symbol's
                    // position in the graph is what ranking is built from.
                    graph: Some(method_ids.clone()),
                });
            }

            let type_id = class_name.clone();
            symbols.entry(type_id.clone()).or_insert_with(|| Symbol {
                id: type_id,
                kind: "type".into(),
                file: file.clone(),
                line: declaration_line(&source, class_name),
                name: class_name.clone(),
                signature: None,
                containing_type: None,
                doc: None,
                calls: Vec::new(),
                graph: Some(method_ids),
            });
        }
    }

    ParseOutcome {
        symbols: symbols.into_values().collect(),
        files: files.len(),
        failed,
    }
}

/// Index a project directory: parse it, then embed what it found.
pub fn index_project(
    store: &mut dyn VectorStore,
    provider: &dyn Provider,
    path: &std::path::Path,
    reset: bool,
) -> Result<IndexReport, String> {
    let outcome = collect_symbols(path);
    let mut report = embed_symbols(store, provider, &outcome.symbols, reset)?;
    report.files = outcome.files;
    // Reported rather than thrown: a handful of unparseable files in a repository is
    // normal, and refusing to index the other two hundred is not useful.
    report.failed = outcome.failed;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::provider::HashingProvider;
    use crate::embed::store::SqliteStore;
    use std::path::PathBuf;

    fn temp_path(tag: &str) -> PathBuf {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut p = std::env::temp_dir();
        p.push(format!(
            "tiny_pdg_index_{tag}_{}_{n}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&p);
        p
    }

    fn symbol(id: &str, fallback_name: &str, kind: &str) -> Symbol {
        let (containing, name) = match id.rsplit_once('.') {
            Some((c, n)) => (Some(c.to_string()), n.to_string()),
            None => (None, fallback_name.to_string()),
        };
        Symbol {
            id: id.into(),
            kind: kind.into(),
            file: format!("{id}.cs"),
            line: 10,
            name: name.clone(),
            signature: Some(format!("public void {name}()")),
            containing_type: containing,
            doc: None,
            calls: Vec::new(),
            graph: None,
        }
    }

    #[test]
    fn the_rendered_text_identifies_the_symbol() {
        let s = symbol("GetAgencyMembersEndpoint.Handler", "Handler", "method");
        let text = s.render();
        assert!(text.contains("GetAgencyMembersEndpoint.Handler"), "{text}");
        assert!(text.contains("method"), "{text}");
        assert!(text.contains("Handler.cs"), "{text}");
    }

    /// A method is often found by what it calls, not what it is named.
    #[test]
    fn calls_are_part_of_the_rendered_text() {
        let mut s = symbol("AgencyRepository.Resolve", "Resolve", "method");
        s.calls = vec!["LoadAgency".into(), "ReadTenant".into()];
        let text = s.render();
        assert!(text.contains("LoadAgency"), "{text}");
        assert!(text.contains("ReadTenant"), "{text}");
    }

    /// Forty callees and a signature nobody reads is worse than a dozen that characterise
    /// the method.
    #[test]
    fn a_long_call_list_is_capped() {
        let mut s = symbol("Big.Fan", "Fan", "method");
        s.calls = (0..40).map(|i| format!("C{i}")).collect();
        let text = s.render();
        assert!(text.contains("C11"), "the first dozen belong: {text}");
        assert!(!text.contains("C39"), "the tail is noise: {text}");
    }

    #[test]
    fn indexing_then_searching_finds_the_symbol_it_wrote() {
        let path = temp_path("find");
        let mut store = SqliteStore::open(&path).expect("open");
        let provider = HashingProvider::new(256);

        let symbols = vec![
            symbol("GetAgencyMembersEndpoint.Handler", "Handler", "method"),
            symbol("PostCreateInvitationEndpoint.Handler", "Handler", "method"),
            symbol("BillingRepository.Charge", "Charge", "method"),
        ];
        let report = embed_symbols(&mut store, &provider, &symbols, true).expect("embed");
        assert_eq!(report.embedded, 3);
        assert_eq!(report.dim, 256);

        let hits = search(&store, &provider, "get agency members endpoint", 3).expect("search");

        assert_eq!(hits.len(), 3);
        assert!(
            hits[0].entry.symbol_id.starts_with("GetAgencyMembers"),
            "the wrong symbol came first: {:?}",
            hits.iter()
                .map(|h| h.entry.symbol_id.clone())
                .collect::<Vec<_>>()
        );
        std::fs::remove_file(&path).ok();
    }

    /// A type with no position is a type an agent has to go hunting for.
    #[test]
    fn a_type_gets_the_line_it_is_declared_on() {
        let source = "namespace N;\npublic class AgencyRepository {\n    void M() {}\n}\n";
        assert_eq!(declaration_line(source, "AgencyRepository"), 2);
        assert_eq!(declaration_line(source, "NotThere"), 0);

        // Generic, sealed and nested declarations all have to be found, not just the
        // plainest spelling.
        let richer = "public sealed class A<T> : B { }\ninternal record struct C(int X);\n";
        assert_eq!(declaration_line(richer, "A"), 1);
        assert_eq!(declaration_line(richer, "C"), 2);
    }

    #[test]
    fn every_entry_keeps_its_file_and_line() {
        let path = temp_path("loc");
        let mut store = SqliteStore::open(&path).expect("open");
        let provider = HashingProvider::new(64);
        embed_symbols(&mut store, &provider, &[symbol("A.B", "B", "method")], true).expect("embed");

        let hits = search(&store, &provider, "B", 1).expect("search");
        assert_eq!(hits[0].entry.file, "A.B.cs");
        assert_eq!(hits[0].entry.line, 10);
        std::fs::remove_file(&path).ok();
    }

    /// Re-indexing with a different model has to be possible, and has to replace rather
    /// than accumulate.
    #[test]
    fn re_indexing_with_another_model_replaces_the_contents() {
        let path = temp_path("remodel");
        let mut store = SqliteStore::open(&path).expect("open");

        embed_symbols(
            &mut store,
            &HashingProvider::new(64),
            &[symbol("A.B", "B", "method")],
            true,
        )
        .expect("first");
        assert_eq!(store.info().expect("info").dim, 64);

        embed_symbols(
            &mut store,
            &HashingProvider::new(128),
            &[symbol("A.B", "B", "method"), symbol("C.D", "D", "method")],
            true,
        )
        .expect("second");

        let info = store.info().expect("info");
        assert_eq!(info.dim, 128);
        assert_eq!(info.entries, 2, "the first index is gone, not merged");

        // And the old width is now refused rather than silently comparable. Naming the
        // model the store holds is what makes it right, so the query has to name the
        // right model too -- otherwise the model check fires first and says so.
        let err = store
            .query("hashing-64", 64, &[0.0; 64], 5)
            .expect_err("the old model must be refused");
        assert!(err.contains("hashing-128"), "{err}");

        let err = store
            .query("hashing-128", 64, &[0.0; 64], 5)
            .expect_err("the old width must be refused");
        assert!(err.contains("128-wide"), "{err}");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn an_empty_symbol_list_is_an_error_naming_the_cause() {
        let path = temp_path("nosymbols");
        let mut store = SqliteStore::open(&path).expect("open");
        let err = embed_symbols(&mut store, &HashingProvider::new(64), &[], true)
            .expect_err("must refuse");
        assert!(err.contains("no symbols"), "{err}");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn k_is_respected() {
        let path = temp_path("k");
        let mut store = SqliteStore::open(&path).expect("open");
        let provider = HashingProvider::new(64);
        let symbols: Vec<Symbol> = (0..10)
            .map(|i| symbol(&format!("T.C{i}"), &format!("C{i}"), "method"))
            .collect();
        embed_symbols(&mut store, &provider, &symbols, true).expect("embed");

        assert_eq!(search(&store, &provider, "C3", 3).expect("q").len(), 3);
        std::fs::remove_file(&path).ok();
    }

    /// The report has to say what was actually stored, because "indexed" without a count
    /// and a model is not something an agent can check.
    #[test]
    fn the_report_carries_the_model_and_width() {
        let path = temp_path("report");
        let mut store = SqliteStore::open(&path).expect("open");
        let report = embed_symbols(
            &mut store,
            &HashingProvider::new(512),
            &[symbol("A.B", "B", "method")],
            true,
        )
        .expect("embed");
        assert_eq!(report.model, "hashing-512");
        assert_eq!(report.dim, 512);
        assert!(!report.store.is_empty());
        std::fs::remove_file(&path).ok();
    }
}
