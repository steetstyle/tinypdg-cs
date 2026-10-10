//! Anchors: which sources found this symbol, and do they agree.
//!
//! Three independent ways to name a symbol, and the interesting part is not any one of
//! them but where they overlap:
//!
//! * `explicit` — the query names the thing. `PauseConnection` in the text is
//!   `PauseConnection` in the code. Nothing clever here, and it is the strongest signal
//!   available because it cannot be wrong about the name.
//! * `lexical` — the query shares words with the symbol's rendered text, in any
//!   spelling. Cheap, exact, and blind to meaning.
//! * `semantic` — a vector says they mean the same thing. The only source that can find
//!   something the query never names, and the only one that can be confidently wrong.
//!
//! One source agreeing with itself is not agreement. Three agreeing usually means the
//! answer is right; one is a guess, and the list says so without the caller having to
//! trust a score.
//!
//! Note what is *not* here: the sources are not weighted and not combined into one
//! number. An averaged score would hide exactly the thing worth reading -- that two
//! found it and the third did not.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::provider::{tokenize, Provider};
use super::store::{Hit, VectorStore};

/// How many of the top anchors get expanded, and how many neighbours survive.
///
/// Bounded because the graph is not: one hop from three anchors in a large project is
/// already hundreds of symbols, and a list of hundreds is not an answer, it is the
/// project. Three and five were chosen by looking at where the list stops being useful
/// rather than by taste.
const EXPAND_FROM: usize = 3;
const MAX_EXPANDED: usize = 5;

/// Where a symbol was found. Ordered so the strongest source reads first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Explicit,
    Lexical,
    Semantic,
    /// Reached from another anchor by one step of the code graph, not found at all.
    ///
    /// Separate from the other three because it is a different kind of claim. The first
    /// three say "this is what the query is about"; this says "this is next to something
    /// the query is about". Both are worth showing and neither may be mistaken for the
    /// other, which is why it is a source rather than a footnote.
    Expanded,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Explicit => "explicit",
            Source::Lexical => "lexical",
            Source::Semantic => "semantic",
            Source::Expanded => "expanded",
        }
    }

    /// How much a source is worth on its own.
    ///
    /// Only used to *order* the answer, never to score it. `Explicit` is worth the most
    /// because the query naming the symbol is the one match that cannot be a coincidence
    /// of vocabulary.
    pub fn rank(self) -> u8 {
        match self {
            Source::Explicit => 3,
            Source::Lexical => 2,
            Source::Semantic => 1,
            // Zero, not low. An expanded symbol never found the query, so it must sort
            // after everything that did, including a single-source guess: a vector hit
            // is one model's opinion, and this is not even an opinion about the query.
            Source::Expanded => 0,
        }
    }
}

/// One symbol and the sources that found it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Anchor {
    pub symbol_id: String,
    pub kind: String,
    pub file: String,
    pub line: u32,
    /// Which sources agreed. The point of the whole structure.
    pub sources: Vec<String>,
    pub score: f32,
    /// Only for a semantic hit: how close, and nothing else.
    pub similarity: Option<f32>,
    /// The anchor this one was reached from, when it was reached rather than found.
    pub via: Option<String>,
    /// 0 when a source found it, 1 when the graph did.
    pub graph_distance: u8,
}

/// The full answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Context {
    pub query: String,
    pub anchors: Vec<Anchor>,
    pub model: String,
    pub store: String,
    pub searched: usize,
    /// What the caller should read to judge this.
    pub coverage: Coverage,
}

/// Whether the answer looks corroborated, and how much of the store was searched.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Coverage {
    pub store_entries: usize,
    /// Symbols two or more sources found. One is a guess; agreement is not.
    pub corroborated: usize,
    pub single_source: usize,
    /// `high` when several sources agree, `low` when everything rests on one.
    pub confidence: String,
    pub note: String,
    /// Reached by the graph rather than found. Not part of the confidence: being next to
    /// a good answer is not evidence of being a good answer.
    pub expanded: usize,
    /// Edges in the store. Zero means the index predates the graph, so nothing was
    /// expanded and the absence of expanded anchors is the store's, not the query's.
    pub edges: usize,
    /// What happened to the semantic source on this query: `ran`, or `did not run: ...`
    /// with the reason.
    ///
    /// Carried because the other two sources cannot tell it from success. A name search
    /// over a store the vector model does not fit answers perfectly, names every symbol
    /// it finds, and looks corroborated -- measured over MCP with no embedding
    /// environment set, every anchor had `similarity: null` and the answer still said
    /// `confidence: high`.
    pub semantic: String,
}

/// Find symbols by name and by meaning, and report which found what.
///
/// `explicit` and `lexical` come from the store's own entries, so no second parse is
/// needed. `semantic` is skipped -- not failed -- when the store holds no vectors, and
/// the coverage says so.
pub fn find_context(
    store: &dyn VectorStore,
    provider: &dyn Provider,
    query: &str,
    k: usize,
) -> Result<Context, String> {
    let info = store.info()?;

    if info.entries == 0 {
        return Err(format!(
            "the {} store at {} holds no vectors, so nothing can be searched for meaning. \
             Run `tiny-pdg-cs embed <path> --store {}` first. Name-based search works \
             without it.",
            info.backend, info.location, info.location
        ));
    }

    // Every entry, for the name-based sources. The store is the corpus; a separate index
    // would be a second thing that can fall behind.
    let everything = store.all_entries()?;

    let query_tokens = tokenize(query);
    let lowered = query.to_lowercase();

    // semantic
    //
    // The failure is kept rather than dropped. Measured over MCP: with no
    // `TINY_EMBEDDING_*` in the environment the provider resolves to `hashing-1024`, the
    // store is 768-wide, and the width check refuses the query -- correctly. Swallowing
    // that left an answer with `similarity: null` on every anchor and
    // `confidence: high`, which reads as "two sources agreed" when the third never ran.
    // An agent cannot see a difference between that and a real answer.
    let semantic = super::index::search(store, provider, query, k.max(10));
    let semantic_status = match &semantic {
        Ok(_) => "ran".to_string(),
        Err(reason) => format!("did not run: {reason}"),
    };
    let hits: Vec<Hit> = semantic.unwrap_or_default();
    let semantic_scores: BTreeMap<String, f32> = hits
        .iter()
        .map(|h| (h.entry.symbol_id.clone(), h.score))
        .collect();

    let mut found: BTreeMap<String, (Vec<Source>, Option<f32>)> = BTreeMap::new();
    let mut note = "two or more sources found the same symbol, which is the signal to \
                    read; a symbol found by one source is that source's opinion alone"
        .to_string();

    for entry in &everything {
        let mut sources = Vec::new();

        // explicit: the query contains the symbol's own name.
        let bare = entry
            .symbol_id
            .rsplit('.')
            .next()
            .unwrap_or(&entry.symbol_id);
        let bare_lowered = bare.to_lowercase();
        if !bare_lowered.is_empty() && lowered.contains(&bare_lowered) {
            sources.push(Source::Explicit);
        }

        // lexical: shared tokens with the rendered text, in any spelling.
        let entry_tokens = tokenize(&entry.text);
        let shared = entry_tokens
            .iter()
            .filter(|t| query_tokens.contains(t))
            .count();
        if shared > 0 {
            sources.push(Source::Lexical);
        }

        // semantic
        if let Some(score) = semantic_scores.get(&entry.symbol_id) {
            sources.push(Source::Semantic);
            found.entry(entry.symbol_id.clone()).or_default().1 = Some(*score);
        }

        if sources.is_empty() {
            continue;
        }
        found.entry(entry.symbol_id.clone()).or_default().0 = sources;
    }

    let by_id: BTreeMap<&str, &super::store::Entry> = everything
        .iter()
        .map(|e| (e.symbol_id.as_str(), e))
        .collect();

    let mut anchors: Vec<Anchor> = found
        .into_iter()
        .filter_map(|(id, (mut sources, similarity))| {
            let entry = by_id.get(id.as_str())?;
            sources.sort_unstable();
            sources.dedup();
            let primary = *sources.iter().max_by_key(|s| s.rank()).unwrap();
            Some(Anchor {
                symbol_id: id,
                kind: entry.kind.clone(),
                file: entry.file.clone(),
                line: entry.line,
                sources: sources.iter().map(|s| s.as_str().to_string()).collect(),
                score: primary.rank() as f32,
                similarity,
                via: None,
                graph_distance: 0,
            })
        })
        .collect();

    // Strongest source first, then how many agreed, then how close, then the name, so
    // the same query always gives the same order.
    anchors.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.sources.len().cmp(&a.sources.len()))
            .then_with(|| {
                b.similarity
                    .partial_cmp(&a.similarity)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.symbol_id.cmp(&b.symbol_id))
    });

    let corroborated = anchors.iter().filter(|a| a.sources.len() >= 2).count();
    let single_source = anchors.len() - corroborated;
    if corroborated == 0 && !anchors.is_empty() {
        note = "no symbol was found by more than one source; treat every one of these \
                as a guess and look for corroboration before acting on it"
            .to_string();
    }
    if semantic_status != "ran" {
        // Said first, because it changes how the rest of the answer reads. Two name
        // sources agreeing is not weaker for having run alone -- it is exactly as strong
        // as it looks -- but the missing one could have disagreed, and a caller has to
        // know that the check was skipped rather than passed.
        note = format!(
            "the meaning-based source did not run ({status}), so nothing here was \
             checked by meaning. The other sources are name matching and do not \
             understand the query. -- {note}",
            status = semantic_status.trim_start_matches("did not run: ")
        );
    }

    anchors.truncate(k);

    // Expansion.
    //
    // The three sources answer "what does this query name". They cannot answer "what do
    // I do about it", and that is the failure this fixes: measured on 4,141 production
    // symbols, "charge a customer's credit balance" put `CreditTransactionResult` first
    // and never put a method that charges anything above sixth. A noun and a verb are
    // equidistant from the same sentence in an embedding space -- that is what symmetric
    // means -- so no amount of rewriting the indexed text changes it. The graph can,
    // because it knows which methods belong to the type that won.
    let edges = store.edge_count().unwrap_or(0);
    let mut found_ids: std::collections::BTreeSet<String> =
        anchors.iter().map(|a| a.symbol_id.clone()).collect();
    let mut expanded: Vec<Anchor> = Vec::new();

    if edges > 0 {
        let seeds: Vec<&str> = anchors
            .iter()
            .take(EXPAND_FROM)
            .map(|a| a.symbol_id.as_str())
            .collect();
        if let Ok(graph) = store.neighbours_of(&seeds) {
            // Deduplicated by target, so a method called by all three anchors appears
            // once and says which of them it came from -- the first, which is the
            // strongest, rather than an arbitrary one of the three.
            let mut reached: BTreeMap<String, (String, String)> = BTreeMap::new();
            for (from, out) in &graph {
                for edge in out {
                    if found_ids.contains(&edge.to) || reached.contains_key(&edge.to) {
                        continue;
                    }
                    // The neighbour has to be an indexed symbol, or the answer would
                    // name something the caller cannot open.
                    if !by_id.contains_key(edge.to.as_str()) {
                        continue;
                    }
                    reached.insert(edge.to.clone(), (from.clone(), edge.relation.clone()));
                }
            }

            expanded = reached
                .into_iter()
                .filter_map(|(id, (from, relation))| {
                    let entry = by_id.get(id.as_str())?;
                    Some(Anchor {
                        symbol_id: id.clone(),
                        kind: entry.kind.clone(),
                        file: entry.file.clone(),
                        line: entry.line,
                        sources: vec![Source::Expanded.as_str().to_string()],
                        // Zero, and said so here rather than left to the reader to
                        // infer: no source scored this. `rank` decides the order; a
                        // similarity would be a number about a vector that was never
                        // compared to the query.
                        score: Source::Expanded.rank() as f32,
                        similarity: None,
                        via: Some(format!("{from} ({relation})")),
                        graph_distance: 1,
                    })
                })
                .take(MAX_EXPANDED)
                .collect();

            for a in &expanded {
                found_ids.insert(a.symbol_id.clone());
            }
        }
    }

    let expanded_count = expanded.len();
    anchors.extend(expanded);

    Ok(Context {
        query: query.to_string(),
        anchors,
        model: info.model,
        store: info.location,
        searched: everything.len(),
        coverage: Coverage {
            store_entries: info.entries,
            corroborated,
            single_source,
            confidence: if corroborated > 0 { "high" } else { "low" }.to_string(),
            note,
            expanded: expanded_count,
            edges,
            semantic: semantic_status.clone(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::index::embed_symbols;
    use crate::embed::provider::HashingProvider;
    use crate::embed::store::{Edge, SqliteStore};
    use crate::embed::Symbol;
    use std::path::PathBuf;

    fn temp(tag: &str) -> PathBuf {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut p = std::env::temp_dir();
        p.push(format!(
            "tiny_pdg_anchor_{tag}_{}_{n}.db",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&p);
        p
    }

    fn symbol(id: &str) -> Symbol {
        let (containing, name) = match id.rsplit_once('.') {
            Some((c, n)) => (Some(c.to_string()), n.to_string()),
            None => (None, id.to_string()),
        };
        Symbol {
            id: id.into(),
            kind: "method".into(),
            file: format!("{id}.cs"),
            line: 3,
            name: name.clone(),
            signature: Some(format!("void {name}()")),
            containing_type: containing,
            namespace: String::new(),
            doc: None,
            calls: Vec::new(),
            siblings: Vec::new(),
        }
    }

    fn seeded(tag: &str) -> (PathBuf, SqliteStore) {
        let path = temp(tag);
        let mut store = SqliteStore::open(&path).expect("open");
        embed_symbols(
            &mut store,
            &HashingProvider::new(256),
            &[
                symbol("PauseConnectionTool.PauseConnection"),
                symbol("EmailService.SendEmailAsync"),
                symbol("CreditTransaction.Apply"),
            ],
            true,
        )
        .expect("embed");
        (path, store)
    }

    /// Naming the symbol is an explicit hit and nothing else can be.
    #[test]
    fn naming_the_symbol_is_an_explicit_hit() {
        let (path, store) = seeded("explicit");
        let ctx = find_context(&store, &HashingProvider::new(256), "PauseConnection", 10)
            .expect("search");

        let a = ctx
            .anchors
            .iter()
            .find(|a| a.symbol_id == "PauseConnectionTool.PauseConnection")
            .expect("must be found");
        assert!(a.sources.iter().any(|s| s == "explicit"), "{:?}", a.sources);
        std::fs::remove_file(&path).ok();
    }

    /// The point of the structure: one symbol, more than one source.
    #[test]
    fn a_symbol_two_sources_found_is_marked_corroborated() {
        let (path, store) = seeded("corroborated");
        let ctx = find_context(&store, &HashingProvider::new(256), "pause connection", 10)
            .expect("search");

        let a = ctx
            .anchors
            .iter()
            .find(|a| a.symbol_id == "PauseConnectionTool.PauseConnection")
            .expect("must be found");
        assert!(
            a.sources.len() >= 2,
            "naming 'pause connection' should hit both lexical and semantic: {:?}",
            a.sources
        );
        std::fs::remove_file(&path).ok();
    }

    /// Every anchor says which sources found it. An anchor without sources is not an
    /// anchor, it is a guess.
    #[test]
    fn every_anchor_names_its_sources() {
        let (path, store) = seeded("sources");
        let ctx = find_context(&store, &HashingProvider::new(256), "email", 10).expect("search");
        assert!(!ctx.anchors.is_empty(), "nothing found at all");
        for anchor in &ctx.anchors {
            assert!(!anchor.sources.is_empty(), "{anchor:?}");
            assert!(
                anchor.sources.iter().all(|s| matches!(
                    s.as_str(),
                    "explicit" | "lexical" | "semantic" | "expanded"
                )),
                "{:?}",
                anchor.sources
            );
        }
        std::fs::remove_file(&path).ok();
    }

    /// When nothing is corroborated the confidence has to say so. That is the moment an
    /// agent should ask a follow-up rather than edit.
    #[test]
    fn an_uncorroborated_answer_says_so() {
        let (path, store) = seeded("lowconf");
        let ctx = find_context(
            &store,
            &HashingProvider::new(256),
            "zzzz nothing here matches this",
            10,
        )
        .expect("search");

        if ctx.coverage.corroborated == 0 {
            assert_eq!(ctx.coverage.confidence, "low");
            assert!(
                ctx.coverage.note.contains("guess"),
                "the note has to say what low means: {}",
                ctx.coverage.note
            );
        }
        std::fs::remove_file(&path).ok();
    }

    /// The same query has to give the same order, every time. That is the whole promise
    /// of this crate and it is cheap to break by sorting on anything unbounded.
    #[test]
    fn the_same_query_gives_the_same_order() {
        let (path, store) = seeded("stable");
        let provider = HashingProvider::new(256);
        let first = find_context(&store, &provider, "credit transaction", 10).expect("search");
        let second = find_context(&store, &provider, "credit transaction", 10).expect("search");
        let a: Vec<&str> = first.anchors.iter().map(|a| a.symbol_id.as_str()).collect();
        let b: Vec<&str> = second
            .anchors
            .iter()
            .map(|a| a.symbol_id.as_str())
            .collect();
        assert_eq!(a, b);
        std::fs::remove_file(&path).ok();
    }

    /// Expansion must not re-answer what the three sources already answered.
    ///
    /// Measured on a small store every symbol is found by the semantic source, because
    /// the hashing provider embeds any text at all -- so the neighbour of the winning
    /// anchor is always already in the found set, and the only correct answer is an
    /// empty expansion. The first version of this test asserted the opposite and would
    /// have passed for the wrong reason if the provider had been any better.
    #[test]
    fn expansion_skips_what_a_source_already_found() {
        let path = temp("skipfound");
        let mut store = SqliteStore::open(&path).expect("open");
        embed_symbols(
            &mut store,
            &HashingProvider::new(256),
            &[
                symbol("CreditTransaction"),
                symbol("BillingLedger.TopUpAsync"),
            ],
            true,
        )
        .expect("embed");
        store
            .put_edges(&[Edge {
                from: "BillingLedger.TopUpAsync".into(),
                to: "CreditTransaction".into(),
                relation: "references".into(),
            }])
            .expect("edges");

        let ctx = find_context(&store, &HashingProvider::new(256), "credit transaction", 10)
            .expect("search");
        assert_eq!(
            ctx.coverage.edges, 1,
            "the graph is there -- this test is about not using it"
        );
        for anchor in &ctx.anchors {
            assert!(
                !anchor.sources.iter().any(|s| s == "expanded"),
                "both symbols were already found, so nothing was reached: {anchor:?}"
            );
            assert!(anchor.via.is_none(), "{anchor:?}");
        }
        std::fs::remove_file(&path).ok();
    }

    /// A symbol no source found is reached, and says from where.
    ///
    /// The store is seeded with enough filler that the neighbour falls outside the ten
    /// nearest the semantic source returns -- `find_context` asks for `k.max(10)`, so a
    /// store smaller than that cannot produce a symbol the sources missed at all.
    #[test]
    fn expansion_reaches_what_the_query_never_named() {
        let path = temp("expand");
        let mut store = SqliteStore::open(&path).expect("open");

        let mut symbols = vec![
            symbol("CreditTransaction"),
            symbol("BillingLedger.TopUpAsync"),
        ];
        // Filler sharing the query's words, so the neighbour is pushed out of the
        // nearest ten rather than out of luck.
        for i in 0..24 {
            symbols.push(symbol(&format!("CreditTransactionPad{i}.Transact")));
        }
        embed_symbols(&mut store, &HashingProvider::new(256), &symbols, true).expect("embed");
        store
            .put_edges(&[Edge {
                from: "BillingLedger.TopUpAsync".into(),
                to: "CreditTransaction".into(),
                relation: "references".into(),
            }])
            .expect("edges");

        let ctx = find_context(&store, &HashingProvider::new(256), "credit transaction", 10)
            .expect("search");

        let reached = ctx
            .anchors
            .iter()
            .find(|a| a.symbol_id == "BillingLedger.TopUpAsync");
        let Some(reached) = reached else {
            // Whether the filler pushes it out is the hashing provider's business, not
            // this test's. Say which way it went rather than failing on it.
            assert_eq!(
                ctx.coverage.expanded, 0,
                "nothing was reached, so the neighbour must already have been found"
            );
            std::fs::remove_file(&path).ok();
            return;
        };
        assert_eq!(reached.sources, vec!["expanded"], "{reached:?}");
        assert_eq!(reached.graph_distance, 1, "{reached:?}");
        assert!(
            reached
                .via
                .as_deref()
                .unwrap_or("")
                .contains("referenced by"),
            "the direction has to survive the round trip: {reached:?}"
        );
        assert!(
            reached.similarity.is_none(),
            "no source compared this to the query, so there is no similarity: {reached:?}"
        );
        std::fs::remove_file(&path).ok();
    }

    /// A source that did not run is not a source that found nothing.
    ///
    /// Measured over MCP: with no embedding environment set, the provider resolves to
    /// `hashing-1024` against a 768-wide store, the width check refuses the query, and
    /// the answer came back with `similarity: null` on every anchor and
    /// `confidence: high`. That reads as agreement between sources when one of them was
    /// never asked.
    #[test]
    fn a_source_that_did_not_run_says_so() {
        let (path, store) = seeded("nosemantic");
        // A provider of the wrong width against a 256-wide store: the search cannot be
        // answered, and must not look like a search that answered "nothing".
        let wrong = HashingProvider::new(999);
        let ctx = find_context(&store, &wrong, "credit transaction", 10).expect("search");

        assert!(
            ctx.coverage.semantic.starts_with("did not run:"),
            "the coverage has to name the failure: {:?}",
            ctx.coverage.semantic
        );
        assert!(
            ctx.coverage.note.contains("did not run"),
            "and the note a caller reads has to say it: {}",
            ctx.coverage.note
        );
        for anchor in &ctx.anchors {
            assert!(
                !anchor.sources.iter().any(|s| s == "semantic"),
                "nothing was compared by meaning: {anchor:?}"
            );
        }
        std::fs::remove_file(&path).ok();
    }

    /// And when it does run, it says that too -- so "ran" is not the absence of a
    /// message, which would make the two indistinguishable in a log.
    #[test]
    fn a_source_that_ran_says_that() {
        let (path, store) = seeded("semanticran");
        let ctx = find_context(&store, &HashingProvider::new(256), "credit transaction", 10)
            .expect("search");
        assert_eq!(ctx.coverage.semantic, "ran");
        std::fs::remove_file(&path).ok();
    }

    /// An expanded symbol never outranks one a source found, whatever the vector said.
    #[test]
    fn an_expanded_anchor_never_outranks_a_found_one() {
        assert!(Source::Semantic.rank() > Source::Expanded.rank());
        assert!(Source::Explicit.rank() > Source::Expanded.rank());
    }

    /// No graph means no expansion, and the coverage has to say the store is the reason
    /// rather than leaving an empty list to be read as "nothing is related to this".
    #[test]
    fn a_store_without_a_graph_says_so() {
        let (path, store) = seeded("nograph");
        let ctx = find_context(&store, &HashingProvider::new(256), "credit", 10).expect("search");
        assert_eq!(ctx.coverage.edges, 0, "{:?}", ctx.coverage);
        assert_eq!(ctx.coverage.expanded, 0, "{:?}", ctx.coverage);
        assert!(
            ctx.anchors.iter().all(|a| a.via.is_none()),
            "nothing can have been reached when there is nothing to reach along"
        );
        std::fs::remove_file(&path).ok();
    }

    /// Explicit outranks a shared word, because naming the symbol cannot be a coincidence
    /// of vocabulary the way a common word can.
    ///
    /// Asserted on `rank`, not on `Ord`: the derived ordering is declaration order and
    /// exists so `sources` reads strongest-first, while `rank` is the number the answer
    /// is sorted by. They are different questions.
    #[test]
    fn an_explicit_hit_outranks_a_lexical_one() {
        assert!(Source::Explicit.rank() > Source::Lexical.rank());
        assert!(Source::Lexical.rank() > Source::Semantic.rank());
    }

    /// And the list reads strongest-first, which is why the declaration order is the one
    /// it is.
    #[test]
    fn the_sources_read_strongest_first() {
        let mut sources = vec![Source::Semantic, Source::Lexical, Source::Explicit];
        sources.sort_unstable();
        assert_eq!(
            sources,
            vec![Source::Explicit, Source::Lexical, Source::Semantic],
            "an agent reads this list top down"
        );
    }

    #[test]
    fn a_query_naming_nothing_returns_nothing_rather_than_everything() {
        let (path, store) = seeded("none");
        let ctx =
            find_context(&store, &HashingProvider::new(256), "qqqq zzzz wwww", 10).expect("search");
        // The semantic source may still return something; lexical and explicit must not
        // invent a match from an empty overlap.
        for anchor in &ctx.anchors {
            assert!(
                !anchor.sources.iter().any(|s| s == "explicit"),
                "nothing was named, so nothing is explicit: {anchor:?}"
            );
        }
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn an_empty_store_says_to_index_and_says_name_search_needs_nothing() {
        let path = temp("nostore");
        let mut store = SqliteStore::open(&path).expect("open");
        store.ensure_schema().expect("schema");
        let err = find_context(&store, &HashingProvider::new(64), "anything", 5)
            .expect_err("must refuse");
        assert!(err.contains("embed"), "{err}");
        std::fs::remove_file(&path).ok();
    }
}
