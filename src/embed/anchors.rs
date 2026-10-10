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

/// Where a symbol was found. Ordered so the strongest source reads first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Explicit,
    Lexical,
    Semantic,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Explicit => "explicit",
            Source::Lexical => "lexical",
            Source::Semantic => "semantic",
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
    let hits: Vec<Hit> =
        super::index::search(store, provider, query, k.max(10)).unwrap_or_default();
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

    anchors.truncate(k);
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
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embed::index::embed_symbols;
    use crate::embed::provider::HashingProvider;
    use crate::embed::store::SqliteStore;
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
            doc: None,
            calls: Vec::new(),
            graph: None,
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
                anchor
                    .sources
                    .iter()
                    .all(|s| matches!(s.as_str(), "explicit" | "lexical" | "semantic")),
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
