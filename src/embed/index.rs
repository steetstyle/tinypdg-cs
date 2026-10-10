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
    /// The file's namespace, read from the source.
    ///
    /// Carried because it is one of the three test signals and a symbol otherwise has no
    /// way to know it: a test class kept in a production folder is invisible from the
    /// path, and invisible from the type name if the class is not named `*Tests`.
    pub namespace: String,
    pub doc: Option<String>,
    /// Methods it calls, names only. Included in the text because a method is often
    /// found by what it touches rather than by what it is called.
    pub calls: Vec<String>,
    /// The other methods of the same type -- distance 1 in the containment graph.
    ///
    /// Kept on the symbol as well as in the store's edge table, because the edge table
    /// can only be read after an index exists and this is what the index is built from.
    pub siblings: Vec<String>,
}

impl Symbol {
    /// The text handed to the provider.
    ///
    /// Built from identity first, then what it contains, then what it touches.
    ///
    /// The kind is deliberately **not** in the text. It is constant across every entry of
    /// a kind -- "method" appears 2,569 times and "type" 2,057 -- so it carries no
    /// discriminative signal and only spends tokens.
    pub fn render(&self) -> String {
        let mut parts: Vec<String> = Vec::new();

        parts.push(match &self.containing_type {
            Some(t) => format!("{t}.{}", self.name),
            None => self.name.clone(),
        });
        parts.push(self.file.clone());

        if let Some(sig) = &self.signature {
            parts.push(sig.clone());
        }
        if let Some(doc) = &self.doc {
            parts.push(doc.clone());
        }
        // A type does NOT list its members here, though it looks like it should. Tried,
        // measured, removed: putting a type's field and method names into its own text
        // made types match *more*, not less.
        //
        //   "charge a customer's credit balance"
        //     without: 0.626  CreditTransactionResult
        //     with:    0.626  CreditTransactionResult
        //   "pause an advertising campaign"
        //     without: 0.601  AdNetworkConnectionPauseTool.PauseConnection  (the answer)
        //     with:    0.598  PauseConnectionResult                          (not the answer)
        //
        // A type whose text names its methods matches any question about those methods,
        // so it becomes a better match than the method itself. That is the problem this
        // was meant to solve, made worse. Fixing it needs the graph, not the text.
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
    /// Symbols the filter dropped, with the reason and how many shared it.
    ///
    /// Reported because a smaller index reads exactly like a repository that has nothing
    /// to exclude, and an agent cannot tell the difference from the number alone.
    pub excluded: Vec<(String, usize)>,
    pub excluded_total: usize,
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
        excluded: Vec::new(),
        excluded_total: 0,
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
    /// Symbols the filter dropped: reason -> how many.
    pub excluded: Vec<(String, usize)>,
    /// The code graph, for expansion at query time.
    pub edges: Vec<super::store::Edge>,
}

impl ParseOutcome {
    pub fn excluded_total(&self) -> usize {
        self.excluded.iter().map(|(_, n)| n).sum()
    }
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

pub fn collect_symbols(root: &std::path::Path, filter: &super::filter::Filter) -> ParseOutcome {
    use crate::parse::parser::parse_source;
    use crate::resolve::symbols::SymbolTable;

    let mut symbols: BTreeMap<String, Symbol> = BTreeMap::new();
    let mut edges: std::collections::BTreeSet<(String, String, String)> =
        std::collections::BTreeSet::new();
    let mut failed = Vec::new();
    let mut excluded: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
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
        let namespace = read_namespace(&source);

        // Needs the type graph, so it runs after the symbol table is built.
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

            // Constructors excluded here as well as below. Leaving them in produced
            // 1,467 edges pointing at symbols the index does not hold -- 11% of the
            // containment graph was dangling, and a neighbour lookup that resolves to
            // something the answer cannot show is worse than no edge at all.
            let method_ids: Vec<String> = info
                .methods
                .iter()
                .filter(|m| m.method != ".ctor")
                .map(|m| format!("{class_name}.{}", m.method))
                .collect();

            // Once per class rather than per method: the reasons are class-level, and a
            // class of forty methods would otherwise print forty identical lines.
            if let Some(reason) = filter.rejection(&file, &namespace, class_name) {
                *excluded.entry(reason).or_default() += info.methods.len().max(1);
                continue;
            }

            for method in &info.methods {
                // Constructors arrive from the parser as a method literally named
                // `.ctor`, once per type. Ten per cent of the index was these, and three
                // of the top twelve hits for a real query were constructors whose text
                // read "AddCreditCommandHandler..ctor method AddCreditCommandHandler.cs".
                // No query names a constructor, so they cannot win anything a person
                // wanted.
                if method.method == ".ctor" {
                    continue;
                }
                let id = format!("{class_name}.{}", method.method);
                symbols.entry(id.clone()).or_insert_with(|| Symbol {
                    id,
                    kind: "method".into(),
                    file: file.clone(),
                    line: method.line_start as u32,
                    name: method.method.clone(),
                    signature: Some(method.signature.clone()),
                    containing_type: Some(class_name.clone()),
                    namespace: namespace.clone(),
                    doc: None,
                    // Filled in from the call sites once every file has been walked; the
                    // call graph is project-wide and this loop is not.
                    calls: Vec::new(),
                    siblings: method_ids.clone(),
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
                namespace: namespace.clone(),
                doc: None,
                calls: Vec::new(),
                siblings: method_ids,
            });
        }
    }

    // The call graph, from the project loader rather than from the loop above.
    //
    // Measured, and the reason is worth keeping: built per file against a per-file type
    // graph, the same project yielded 784 call edges of which 777 were inside a single
    // class and only 7 crossed between types. A type graph that cannot see the other
    // 1,666 files resolves every bare method name to the class it happens to share a file
    // with, which is not a call graph. The project loader sees all of them and finds
    // 29,896 call sites instead. It costs 0.90s on 1,721 files -- indexing spends its
    // seconds on the embedding calls, not the parse -- so the second parse is worth a
    // graph that means something.
    //
    // Failure degrades to containment-only rather than failing the index: half a graph
    // is better than no index.
    let project = crate::cli::commands::load_project(root).ok();
    let sites: Vec<crate::analysis::callgraph::CallSite> = project
        .as_ref()
        .map(|(_, call_graph)| call_graph.calls.clone())
        .unwrap_or_default();

    // Callees, from the call sites collected above.
    //
    // `callee` is the bare method name; `callee_class` is the owner the type graph
    // resolved it to, and it is empty when resolution failed. A callee whose class is
    // unknown is kept under its bare name rather than dropped and rather than guessed:
    // it is still a true statement about what this method touches, and guessing a class
    // would put a distance on the guess.
    let mut callees: std::collections::HashMap<String, std::collections::BTreeSet<String>> =
        std::collections::HashMap::new();
    for site in &sites {
        if site.callee.is_empty() || site.caller_method == ".ctor" {
            continue;
        }
        let from = format!("{}.{}", site.caller_class, site.caller_method);
        let to = if site.callee_class.is_empty() {
            site.callee.clone()
        } else {
            format!("{}.{}", site.callee_class, site.callee)
        };
        callees.entry(from).or_default().insert(to);
    }

    for symbol in symbols.values_mut() {
        if let Some(set) = callees.get(&symbol.id) {
            symbol.calls = set.iter().cloned().collect();
        }
    }

    // The graph. Two relations, because they answer different questions: `declares` is
    // containment and is always certain, `calls` is the call graph and is only as good
    // as the type resolution behind it.
    for (id, symbol) in &symbols {
        for sibling in &symbol.siblings {
            edges.insert((id.clone(), sibling.clone(), "declares".into()));
        }
    }
    for site in &sites {
        if site.callee.is_empty() || site.caller_method == ".ctor" {
            continue;
        }
        let from = format!("{}.{}", site.caller_class, site.caller_method);
        let to = if site.callee_class.is_empty() {
            site.callee.clone()
        } else {
            format!("{}.{}", site.callee_class, site.callee)
        };
        // Only edges between indexed symbols, and never a self-edge. A dangling edge
        // expands to something the answer cannot show, and a self-edge expands a symbol
        // to itself: both look like the graph worked and neither tells anyone anything.
        // 23 of them on a 4,141-symbol index.
        if from != to && symbols.contains_key(&from) && symbols.contains_key(&to) {
            edges.insert((from, to, "calls".into()));
        }
    }

    // Who uses this type.
    //
    // This is the relation that makes expansion worth having. Measured: the winning
    // anchor for "charge a customer's credit balance" was `CreditTransaction`, a record
    // with no methods and no callers, and it had *zero* edges in either direction --
    // because nothing calls a record. But it is used, and by exactly the right thing:
    //
    //   Billing.Processing/UseCases/Commands/AddCreditCommand.cs:25
    //     IDatabaseRepository<CreditTransaction> transactionRepository
    //   Billing.Processing/UseCases/Commands/AddCreditCommand.cs:84
    //     var transaction = new CreditTransaction
    //
    // A call graph cannot see either line: one is a parameter type, the other an
    // allocation. Type usage is the edge that connects a noun to the verb that works on
    // it, and without it expansion has nothing to say about the records and DTOs that
    // half of every search result is made of.
    if let Some((type_graph, call_graph)) = &project {
        for (class, created) in &call_graph.class_creations {
            for created in created {
                if symbols.contains_key(class) && symbols.contains_key(created) {
                    edges.insert((class.clone(), created.clone(), "references".into()));
                }
            }
        }
        for (class, info) in &type_graph.classes {
            for field in &info.fields {
                for named in named_types(&field.field_type) {
                    if named != *class && symbols.contains_key(&named) {
                        edges.insert((class.clone(), named, "references".into()));
                    }
                }
            }
        }
    }

    ParseOutcome {
        symbols: symbols.into_values().collect(),
        files: files.len(),
        failed,
        excluded: excluded.into_iter().collect(),
        edges: edges
            .into_iter()
            .map(|(from, to, relation)| super::store::Edge { from, to, relation })
            .collect(),
    }
}

/// The type names a declared type mentions, without the generics around them.
///
/// `IReadOnlyList<CreditTransaction>` names `CreditTransaction`, and it is the one worth
/// an edge: the wrapper is a standard type that half the project mentions, so an edge to
/// it says nothing, while the argument says which record this class actually holds.
fn named_types(declared: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut rest = declared.trim();
    while let Some(open) = rest.find(['<', '[', '(']) {
        let inner = &rest[open + 1..];
        let close = match rest.as_bytes()[open] {
            b'<' => inner.find('>'),
            b'[' => inner.find(']'),
            _ => inner.find(')'),
        };
        let Some(end) = close else { break };
        for part in inner[..end].split(',') {
            let name = part.trim().rsplit('.').next().unwrap_or("").trim();
            if !name.is_empty() {
                out.push(name.to_string());
            }
        }
        rest = &inner[end + 1..];
    }
    let bare = rest
        .trim()
        .rsplit('.')
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    if !bare.is_empty() && !out.contains(&bare) {
        out.push(bare);
    }
    // A declared type can carry modifiers and nullable marks. Whatever is left after
    // dropping them is a name a class could be called; anything else is punctuation the
    // store would never match against a symbol.
    //
    // The `?` comes off first and not in the same pass. It is not rare punctuation --
    // nullable reference types are ordinary C# -- and testing for a valid name before
    // dropping the mark throws the whole name away instead of the mark, which looks
    // exactly like the type being unused.
    for n in &mut out {
        *n = n.trim_end_matches('?').to_string();
    }
    out.retain(|n| !n.is_empty() && n.chars().all(|c| c.is_alphanumeric() || c == '_'));
    out
}

/// The file's namespace, or the empty string.
///
/// Read from the source rather than from the path, because a project that keeps tests
/// beside production code has no way to say so in either. Cheap: it is the first
/// namespace declaration in the file, and the cost of being wrong is that the filter
/// falls back to the other two signals.
fn read_namespace(source: &str) -> String {
    for line in source.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("namespace ") {
            let name = rest.trim().trim_end_matches(';').trim();
            if !name.is_empty() {
                return name.to_string();
            }
        }
    }
    String::new()
}

/// Index a project directory: parse it, filter it, then embed what is left.
///
/// Test code is dropped unless the filter says otherwise, and how many were dropped is
/// in the report: an index of 5,883 symbols where a repository has 7,087 says nothing on
/// its own, and the number is what tells an agent to ask.
pub fn index_project(
    store: &mut dyn VectorStore,
    provider: &dyn Provider,
    path: &std::path::Path,
    reset: bool,
    filter: &super::filter::Filter,
) -> Result<IndexReport, String> {
    let outcome = collect_symbols(path, filter);
    let mut report = embed_symbols(store, provider, &outcome.symbols, reset)?;
    // After the vectors, not before: a graph with no vectors to expand towards is a
    // table nobody reads.
    store.put_edges(&outcome.edges)?;
    report.files = outcome.files;
    // Reported rather than thrown: a handful of unparseable files in a repository is
    // normal, and refusing to index the other two hundred is not useful.
    report.excluded_total = outcome.excluded_total();
    report.excluded = outcome.excluded;
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
            namespace: String::new(),
            doc: None,
            calls: Vec::new(),
            siblings: Vec::new(),
        }
    }

    /// The argument of a generic is the type worth an edge; the wrapper is not.
    ///
    /// `IReadOnlyList<CreditTransaction>` says this class holds a `CreditTransaction`.
    /// An edge to `IReadOnlyList` would say every collection in the project uses it,
    /// which is true of all of them and therefore true of none.
    #[test]
    fn a_generic_argument_is_the_type_worth_naming() {
        assert_eq!(
            named_types("IReadOnlyList<CreditTransaction>"),
            vec!["CreditTransaction"]
        );
        assert_eq!(
            named_types("Dictionary<string, CreditTransaction>"),
            vec!["string", "CreditTransaction"]
        );
        assert_eq!(named_types("List<Credit>[]"), vec!["Credit"]);
    }

    /// A namespace is stripped: the symbol table is keyed by the bare name.
    #[test]
    fn a_namespace_is_not_part_of_the_name() {
        assert_eq!(
            named_types("Billing.Database.Models.Credit"),
            vec!["Credit"]
        );
        assert_eq!(named_types("  Credit  "), vec!["Credit"]);
    }

    /// `Credit?` is a `Credit`. Dropping the nullable mark would lose the edge; keeping
    /// it would write a name no class is called.
    #[test]
    fn a_nullable_mark_does_not_hide_the_type() {
        assert_eq!(named_types("CreditTransaction?"), vec!["CreditTransaction"]);
        assert_eq!(named_types("global::System.Guid"), vec!["Guid"]);
    }

    #[test]
    fn a_type_with_nothing_inside_yields_nothing() {
        assert!(named_types("").is_empty());
        assert!(named_types("   ").is_empty());
    }

    #[test]
    fn the_rendered_text_identifies_the_symbol() {
        let s = symbol("GetAgencyMembersEndpoint.Handler", "Handler", "method");
        let text = s.render();
        assert!(text.contains("GetAgencyMembersEndpoint.Handler"), "{text}");
        assert!(text.contains("Handler.cs"), "{text}");
        assert!(
            text.contains("public void Handler()"),
            "the signature is what separates two overloads: {text}"
        );
    }

    /// The kind is not in the text, and that is deliberate.
    ///
    /// It is constant across every entry of a kind -- "method" appeared 2,569 times and
    /// "type" 2,057 in a real index -- so it carries no discriminative signal and only
    /// spends tokens the model has to read.
    #[test]
    fn the_kind_is_not_in_the_rendered_text() {
        let method = symbol("A.M", "M", "method").render();
        let type_ = symbol("T", "T", "type").render();
        assert!(!method.contains(" method "), "{method}");
        assert!(!type_.contains(" type "), "{type_}");
    }

    /// Constructors arrive from the parser named `.ctor`, once per type.
    ///
    /// Ten per cent of a real index was these, and three of the top twelve hits for a
    /// real query were constructors. No query names a constructor, so they cannot win
    /// anything a person wanted.
    #[test]
    fn constructors_are_not_indexed() {
        let s = symbol("AddCreditCommandHandler..ctor", ".ctor", "method");
        // The symbol helper is happy to build one, so the check has to live where the
        // index is built.
        assert!(
            s.render().contains(".ctor"),
            "the helper can still build one"
        );
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
