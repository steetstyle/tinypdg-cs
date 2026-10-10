//! MCP tool implementations.
//!
//! Every tool returns structured JSON rather than DOT. An agent has to parse
//! DOT to use it, which wastes tokens and invites mistakes; the DOT renderers
//! in `cli::commands` stay for humans.

use std::collections::BTreeMap;
use std::path::PathBuf;

use petgraph::graph::NodeIndex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::cache;

// ───────────────────────── find_callers ─────────────────────────

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FindCallersArgs {
    /// Directory or file to analyse.
    pub path: String,
    pub class: String,
    pub method: String,
    /// Drop callers further than this many hops away (1 = direct callers only).
    #[serde(default)]
    pub max_distance: Option<usize>,
    /// Exclude callers whose project looks like a test project (`*.Tests`).
    #[serde(default)]
    pub exclude_test_projects: Option<bool>,
    /// Cap how many callers are listed.
    #[serde(default)]
    pub limit: Option<usize>,
    /// How many callers to skip, for paging through a long caller list.
    #[serde(default)]
    pub offset: Option<usize>,
}

/// One caller of the target, with its reachability depth.
#[derive(Debug, Serialize)]
pub struct CallerInfo {
    /// `Class.Method`
    pub method: String,
    /// True when this calls the target directly (depth 1).
    pub direct: bool,
    /// Shortest hop count from the target. 1 = direct caller.
    pub distance: usize,
    /// Source lines where this method calls the target. Empty when the
    /// relationship came from route registration rather than a call site.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub evidence: Vec<usize>,
}

/// Resolve a source argument to a local directory, fetching a GitHub specifier.
///
/// Every tool takes the same string, so "gh:owner/repo" means the same thing here as in
/// the CLI, and the cache means a second call with the same specifier does not re-clone.
fn source_dir(spec: &str, tool: &str) -> Result<PathBuf, String> {
    crate::source::resolve(spec)
        .map(|checkout| checkout.dir)
        .map_err(|e| format!("{tool} could not resolve '{spec}': {e:#}"))
}

pub fn find_callers(args: FindCallersArgs) -> Result<Value, String> {
    // Resolved once and used for both, because a `gh:` specifier is not a path the route
    // extractor could read: the checkout has to happen before either of them.
    let dir = source_dir(&args.path, "find_callers")?;
    let project = cache::get_or_build(&dir).map_err(|e| e.to_string())?;
    let routes =
        crate::route::extractor::extract(&dir.to_string_lossy()).map_err(|e| e.to_string())?;

    // Build from the cached graph rather than the path-based wrapper: the wrapper
    // re-parses the whole project on every call, which is exactly what the cache
    // exists to avoid.
    let graph = crate::analysis::impact::build_impact_from(
        &project.type_graph,
        &project.call_graph,
        &routes,
        &args.class,
        &args.method,
    )
    .map_err(|e| e.to_string())?;

    // Nodes other than the target are its callers; edge direction is
    // caller → callee, so an edge INTO the target names a direct caller.
    let target = graph.target.clone();
    // "Calls the target directly" means an edge into the target — nothing else.
    // The node's own caller count is not that: a node reached through a helper
    // has callers of its own, which made every transitive caller look direct.
    let direct: std::collections::HashSet<&String> = graph
        .edges
        .iter()
        .filter(|(_, callee)| callee == &target)
        .map(|(caller, _)| caller)
        .collect();

    // A minimal-API handler's caller is its registration site, which has no edge
    // in the call graph; the impact graph seeds it as a node instead.
    let registrars: std::collections::HashSet<&String> = graph
        .routes
        .iter()
        .filter_map(|r| r.registrar.as_ref())
        .collect();

    // Line numbers of each call site, so a caller can be shown where the call
    // actually is. This is the "evidence" the traversal engine is not available
    // to provide: stateless and exact.
    let mut call_sites: std::collections::HashMap<(String, String), Vec<usize>> =
        std::collections::HashMap::new();
    for c in &project.call_graph.calls {
        let (callee_class, _) = crate::analysis::diffimpact::resolve_callee_class(c);
        if callee_class.is_empty() {
            continue;
        }
        let caller = format!("{}.{}", c.caller_class, c.caller_method);
        let callee = format!("{}.{}", callee_class, c.callee);
        call_sites.entry((caller, callee)).or_default().push(c.line);
    }

    let mut callers: Vec<CallerInfo> = graph
        .nodes
        .iter()
        .filter(|(node, _)| *node != &target)
        .map(|(node, (_direct_count, _))| {
            let is_direct = direct.contains(node) || registrars.contains(node);
            let mut lines = call_sites
                .get(&(node.clone(), target.clone()))
                .cloned()
                .unwrap_or_default();
            lines.sort_unstable();
            lines.dedup();
            CallerInfo {
                method: node.clone(),
                direct: is_direct,
                // The impact graph records counts, not per-node depth; direct
                // callers get 1 and everything else is reported as transitive.
                distance: if is_direct { 1 } else { 2 },
                evidence: lines,
            }
        })
        .collect();
    callers.sort_by(|a, b| {
        a.distance
            .cmp(&b.distance)
            .then_with(|| a.method.cmp(&b.method))
    });

    // Filters. Applied after the graph is built so counts can still report what
    // was filtered out — a caller that silently gets an empty list cannot tell
    // "no callers" from "my filter was too narrow".
    let total_before_filter = callers.len();
    if let Some(max) = args.max_distance {
        callers.retain(|c| c.distance <= max);
    }
    if args.exclude_test_projects.unwrap_or(false) {
        callers.retain(|c| !looks_like_test_project(&c.method));
    }
    let after_filter = callers.len();

    let offset = args.offset.unwrap_or(0);
    let limit = args.limit.unwrap_or(usize::MAX).max(1);
    let window: Vec<CallerInfo> = callers.into_iter().skip(offset).take(limit).collect();

    Ok(json!({
        "target": target,
        "project": project.root.display().to_string(),
        "caller_count": after_filter,
        "callers": window,
        "total_before_filter": total_before_filter,
        "filtered_out": total_before_filter.saturating_sub(after_filter),
        // HTTP routes reaching this method. For a minimal-API handler this is
        // the only caller information that exists: the handler is wired up by a
        // method group reference, which is not a call site in the call graph.
        "routes": graph.routes,
        // Edge list kept so a caller can reason about the call structure,
        // e.g. a shared intermediate helper.
        "edges": graph.edges,
        "parsed_files": project.file_count,
    }))
}

/// Does this method belong to a project that looks like a test project?
///
/// Matched on the type graph's file paths rather than on namespaces, because a
/// test project's namespace usually mirrors production while its path does not.
fn looks_like_test_project(method: &str) -> bool {
    let class = method.split_once('.').map(|x| x.0).unwrap_or(method);
    let lower = class.to_lowercase();
    lower.ends_with("tests")
        || lower.contains(".tests.")
        || lower.starts_with("tests.")
        || lower.contains("test")
}

// ───────────────────────── method_pdg ─────────────────────────

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct MethodPdgArgs {
    pub path: String,
    pub class: String,
    pub method: String,
    /// Restrict to one file, for graphs with several same-named methods.
    pub file: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PdgBlock {
    pub id: usize,
    pub kind: String,
    pub start_line: usize,
    pub end_line: usize,
}

#[derive(Debug, Serialize)]
pub struct PdgLink {
    pub from: usize,
    pub to: usize,
    /// `control`, `data`, or `cfg:<kind>`.
    pub kind: String,
}

/// Arguments for the hammock-block view of a method.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct MethodHammocksArgs {
    pub path: String,
    pub class: String,
    pub method: String,
    /// Restrict to one file, for methods declared in more than one place.
    #[serde(default)]
    pub file: Option<String>,
    /// Report only regions that contain at least this many blocks.
    ///
    /// 2 is the definition's own floor. Raising it is how you get the handful of
    /// structured regions rather than every adjacent pair of statements.
    #[serde(default)]
    pub min_blocks: Option<usize>,
}

/// A region as the forest is built: its line span, and the widest body found for it
/// together with the blocks that bound it.
type Region = ((usize, usize), (usize, NodeIndex, NodeIndex));

/// One hammock block, with the parent that makes a traversal walk possible.
#[derive(Debug, Serialize)]
pub struct HammockRegion {
    /// Stable within one response; quote it back to traverse.
    pub id: String,
    /// The enclosing region's id, or null at the outermost level.
    pub parent_id: Option<String>,
    /// How many blocks deep from the outermost region of this method.
    pub depth: usize,
    pub kind: String,
    pub start_line: usize,
    pub end_line: usize,
    pub blocks: usize,
    pub header_kind: String,
    pub footer_kind: String,
}

/// The hammock blocks of one method, as a containment forest.
///
/// Hammock blocks (Johnson '94) restructure a method into single-entry-single-exit
/// regions, and nest: a loop body inside a conditional inside a method. That nesting is
/// the point. It is what lets an agent move between module, class, function and
/// statement granularity in one pass, choosing the level that is most informative for
/// the symptom in front of it, instead of reasoning over one flattened graph.
///
/// Which is why this is a forest with named nodes and parents rather than a list: a
/// traversal that can only go "down" has no way to zoom out after it has gone too deep.
pub fn method_hammocks(args: MethodHammocksArgs) -> Result<Value, String> {
    let project =
        cache::get_or_build(&source_dir(&args.path, "analyse")?).map_err(|e| e.to_string())?;

    let method = project
        .type_graph
        .classes
        .get(&args.class)
        .and_then(|c| c.methods.iter().find(|m| m.method == args.method))
        .ok_or_else(|| {
            format!(
                "method {}.{} not found; classes include {:?}",
                args.class,
                args.method,
                project
                    .type_graph
                    .classes
                    .keys()
                    .take(5)
                    .collect::<Vec<_>>()
            )
        })?;

    if let Some(ref want) = args.file {
        if !method.file.ends_with(want.as_str()) {
            return Err(format!(
                "{} found in {}, but --file {} was requested",
                args.method, method.file, want
            ));
        }
    }

    let source = std::fs::read_to_string(&method.file)
        .map_err(|e| format!("read {}: {}", method.file, e))?;
    let cfg = crate::cfg::builder::build_cfg(&source).map_err(|e| e.to_string())?;

    // Per file, then narrowed to this method by line span. Regions are computed per
    // method inside find_hammocks_in_file; a method's own Entry and Exit bound it.
    let all = crate::hammock::builder::find_hammocks_in_file(&cfg);
    let (start, end) = (method.line_start, method.line_end);

    let min_blocks = args.min_blocks.unwrap_or(2);

    // One region per line span, keeping the widest body found for it.
    //
    // Two blocks often share a span -- the Entry block and the first statement both start
    // on the method's line -- so distinct hammock headers can map to the same span and
    // report the same region several times. Keying on the span is what makes the result a
    // forest: with the node identity kept, several entries would each be "the parent" of
    // the same child and the hierarchy would be ambiguous.
    let mut widest: BTreeMap<(usize, usize), (usize, NodeIndex, NodeIndex)> = BTreeMap::new();
    for hammock in &all {
        let header = &cfg[hammock.header];
        let footer = &cfg[hammock.footer];
        if header.start_line < start || footer.end_line > end {
            continue;
        }
        if hammock.body.len() < min_blocks {
            continue;
        }
        let span = (header.start_line, footer.end_line);
        let entry = widest
            .entry(span)
            .or_insert((0, hammock.header, hammock.footer));
        if hammock.body.len() > entry.0 {
            *entry = (hammock.body.len(), hammock.header, hammock.footer);
        }
    }

    // Outermost first, so a parent always appears before its children.
    //
    // The `Reverse` is what does it. The map iterates by `(start, end)` ascending, and
    // two regions sharing a start line are nested with the *wider* one outside, so
    // ascending `end` would emit the child first — and a reader attaching depth on one
    // pass, or a traversal walking down without backtracking, would meet a parent id
    // that had not been seen yet.
    let mut regions: Vec<Region> = widest.into_iter().collect();
    regions.sort_by_key(|((s, e), _)| (*s, std::cmp::Reverse(*e)));

    let count = regions.len();
    let out: Vec<HammockRegion> = regions
        .iter()
        .enumerate()
        .map(|(i, ((s, e), (blocks, header, footer)))| {
            // Regions that strictly contain this one. Strict means a different span, so
            // two regions sharing a span overlap rather than nest and neither becomes
            // the other's parent.
            let enclosing: Vec<usize> = regions
                .iter()
                .enumerate()
                .filter(|(j, ((ps, pe), _))| *j != i && ps <= s && pe >= e && (ps, pe) != (s, e))
                .map(|(j, _)| j)
                .collect();

            // The parent is the *tightest* enclosing region: the narrowest span, which is
            // what "immediate" means here.
            let parent_index = enclosing.iter().copied().min_by_key(|j| {
                let ((ps, pe), _) = regions[*j];
                (pe - ps, *j)
            });

            // Depth from the parent chain, not from a direct count of enclosing spans:
            // counting every enclosing span would say a region whose ancestor has an equal
            // span is deeper than its own parent reports.
            let parents: Vec<Option<usize>> = regions
                .iter()
                .enumerate()
                .map(|(i, ((s, e), _))| {
                    regions
                        .iter()
                        .enumerate()
                        .filter(|(j, ((ps, pe), _))| {
                            *j != i && ps <= s && pe >= e && (ps, pe) != (s, e)
                        })
                        .map(|(j, ((ps, pe), _))| (pe - ps, j))
                        .min()
                        .map(|(_, j)| j)
                })
                .collect();

            let mut depth = 0;
            let mut cursor = parents[i];
            // Parents are strictly wider, so the chain ends; the bound is a guard against
            // a future change that breaks that, not a normal exit.
            while let Some(p) = cursor {
                depth += 1;
                cursor = parents[p];
                if depth > regions.len() {
                    break;
                }
            }

            HammockRegion {
                id: format!("h{i}"),
                parent_id: parent_index.map(|j| format!("h{j}")),
                depth,
                kind: format!("{:?}", cfg[*header].kind),
                start_line: *s,
                end_line: *e,
                blocks: *blocks,
                header_kind: format!("{:?}", cfg[*header].kind),
                footer_kind: format!("{:?}", cfg[*footer].kind),
            }
        })
        .collect();

    Ok(serde_json::json!({
        "class": args.class,
        "method": args.method,
        "file": method.file,
        "granularity": "hammock blocks (single-entry, single-exit regions), nested",
        "region_count": count,
        "min_blocks": min_blocks,
        "regions": out,
        "how_to_use": "Each region names its parent, so a traversal can move up to a wider \
                       region with Expand and out to an adjacent one with Relate. Regions \
                       are ordered outermost first, and a parent always appears before its \
                       children.",
    }))
}

pub fn method_pdg(args: MethodPdgArgs) -> Result<Value, String> {
    let project =
        cache::get_or_build(&source_dir(&args.path, "analyse")?).map_err(|e| e.to_string())?;

    let method = project
        .type_graph
        .classes
        .get(&args.class)
        .and_then(|c| c.methods.iter().find(|m| m.method == args.method))
        .ok_or_else(|| {
            format!(
                "method {}.{} not found; classes include {:?}",
                args.class,
                args.method,
                project
                    .type_graph
                    .classes
                    .keys()
                    .take(5)
                    .collect::<Vec<_>>()
            )
        })?;

    if let Some(ref want) = args.file {
        if !method.file.ends_with(want.as_str()) {
            return Err(format!(
                "{} found in {}, but --file {} was requested",
                args.method, method.file, want
            ));
        }
    }

    let source = std::fs::read_to_string(&method.file)
        .map_err(|e| format!("read {}: {}", method.file, e))?;
    let cfg = crate::cfg::builder::build_cfg(&source).map_err(|e| e.to_string())?;
    let pdg = crate::pdg::pdg_builder::build_pdg(&cfg).map_err(|e| e.to_string())?;

    // build_cfg works over the whole file, so narrow to the target method by
    // line range before reporting.
    let (start, end) = (method.line_start, method.line_end);
    let blocks: Vec<PdgBlock> = pdg
        .node_indices()
        .map(|idx| {
            let b = &pdg[idx];
            PdgBlock {
                id: b.id,
                kind: format!("{:?}", b.kind),
                start_line: b.start_line,
                end_line: b.end_line,
            }
        })
        .filter(|b| b.end_line >= start && b.start_line <= end)
        .collect();

    let in_range = |line: usize| line >= start && line <= end;
    use petgraph::visit::EdgeRef;
    let links: Vec<PdgLink> = pdg
        .edge_references()
        .filter_map(|e| {
            let from = &pdg[e.source()];
            let to = &pdg[e.target()];
            if !in_range(from.start_line) && !in_range(to.start_line) {
                return None;
            }
            Some(PdgLink {
                from: from.id,
                to: to.id,
                kind: match e.weight() {
                    crate::pdg::pdg_builder::PdgEdge::Control => "control".into(),
                    crate::pdg::pdg_builder::PdgEdge::Data => "data".into(),
                    crate::pdg::pdg_builder::PdgEdge::Cfg(k) => format!("cfg:{:?}", k),
                },
            })
        })
        .collect();

    Ok(json!({
        "class": args.class,
        "method": args.method,
        "file": method.file,
        "line_start": start,
        "line_end": end,
        "signature": method.signature,
        "blocks": blocks,
        "edges": links,
    }))
}

// ───────────────────────── list_routes ─────────────────────────

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListRoutesArgs {
    pub path: String,
    /// Case-insensitive substring filter over the pattern and handler class.
    pub filter: Option<String>,
    /// Restrict to one HTTP verb (`GET`, `POST`, ...).
    #[serde(default)]
    pub http_method: Option<String>,
    /// Restrict to routes whose pattern starts with this prefix.
    #[serde(default)]
    pub path_prefix: Option<String>,
    /// Cap how many routes are listed.
    #[serde(default)]
    pub limit: Option<usize>,
    /// How many routes to skip, for paging through a large route table.
    #[serde(default)]
    pub offset: Option<usize>,
}

pub fn list_routes(args: ListRoutesArgs) -> Result<Value, String> {
    let (table, stats) = crate::route::extractor::extract_with_stats(
        &source_dir(&args.path, "list_routes")?.to_string_lossy(),
    )
    .map_err(|e| e.to_string())?;

    let needle = args.filter.as_ref().map(|f| f.to_lowercase());
    let verb = args.http_method.as_ref().map(|v| v.to_uppercase());
    let prefix = args.path_prefix.as_ref().map(|p| p.to_lowercase());

    let matched: Vec<&crate::route::extractor::RouteEntry> = table
        .routes
        .iter()
        .filter(|r| match needle.as_ref() {
            None => true,
            Some(n) => {
                r.path.to_lowercase().contains(n)
                    || r.class.to_lowercase().contains(n)
                    || r.handler.to_lowercase().contains(n)
            }
        })
        .filter(|r| match verb.as_ref() {
            None => true,
            Some(v) => r.http_method.eq_ignore_ascii_case(v),
        })
        .filter(|r| match prefix.as_ref() {
            None => true,
            Some(p) => r.path.to_lowercase().starts_with(p.as_str()),
        })
        .collect();

    let total_matched = matched.len();
    let offset = args.offset.unwrap_or(0);
    let limit = args.limit.unwrap_or(usize::MAX).max(1);
    let window = &matched[offset.min(total_matched)..];

    let routes: Vec<Value> = window
        .iter()
        .take(limit)
        .map(|r| {
            json!({
                "method": r.http_method,
                "pattern": r.path,
                "class": r.class,
                "handler": r.handler,
                "style": r.source,
            })
        })
        .collect();

    Ok(json!({
        "routes": routes,
        "count": routes.len(),
        // Report the pre-window total so a caller can tell a complete answer
        // from a page, and knows whether to ask for the next one.
        "total_matched": total_matched,
        "offset": offset,
        "has_more": offset + routes.len() < total_matched,
        "inline_lambda_routes": stats.inline_lambda_routes,
        "files_parsed": stats.files_parsed,
        "files_failed": stats.files_failed,
    }))
}

// ───────────────────────── find_patterns ─────────────────────────

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FindPatternsArgs {
    pub path: String,
    /// Minimum confidence to include.
    pub min_confidence: Option<f64>,
    /// Cap how many detections are listed.
    #[serde(default)]
    pub limit: Option<usize>,
    /// How many detections to skip, for paging.
    #[serde(default)]
    pub offset: Option<usize>,
}

pub fn find_patterns(args: FindPatternsArgs) -> Result<Value, String> {
    let project =
        cache::get_or_build(&source_dir(&args.path, "analyse")?).map_err(|e| e.to_string())?;

    let mut tg = project.type_graph.clone();
    // Detection reads method source for HTTP attributes, so per-file sources
    // must be annotated for the lookup to hit.
    let mut per_file: std::collections::HashMap<String, String> = Default::default();
    let mut files: Vec<String> = tg
        .classes
        .values()
        .flat_map(|c| c.methods.iter())
        .map(|m| m.file.clone())
        .filter(|f| !f.is_empty())
        .collect();
    files.sort();
    files.dedup();
    for file in files {
        if let Ok(src) = std::fs::read_to_string(&file) {
            per_file.insert(file, src);
        }
    }
    let mut all_sources = String::new();
    for src in per_file.values() {
        all_sources.push_str(src);
        all_sources.push('\n');
    }
    tg.annotate_method_files("");

    let ctx = crate::detect::types::DetectionContext::with_sources(
        &tg,
        &project.call_graph,
        &all_sources,
        &per_file,
    );

    let mut detections = Vec::new();
    detections.extend(crate::detect::creational::detect_creational(&ctx));
    detections.extend(crate::detect::structural::detect_structural(&ctx));
    detections.extend(crate::detect::behavioral::detect_behavioral(&ctx));
    detections.extend(crate::detect::dotnet::detect_dotnet(&ctx));

    let threshold = args.min_confidence.unwrap_or(0.0);
    let qualified: Vec<crate::detect::types::PatternMatch> = detections
        .into_iter()
        .filter(|d| d.confidence >= threshold)
        .collect();
    let total_matched = qualified.len();
    let offset = args.offset.unwrap_or(0);
    let limit = args.limit.unwrap_or(usize::MAX).max(1);
    let matches: Vec<Value> = qualified
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|d| {
            json!({
                "pattern": format!("{:?}", d.pattern),
                "class": d.class,
                "confidence": d.confidence,
                "description": d.description,
                "participants": d.participants,
                "evidence": d.evidence,
            })
        })
        .collect();

    Ok(json!({
        "patterns": matches,
        "count": matches.len(),
        "total_matched": total_matched,
        "offset": offset,
        "has_more": offset + matches.len() < total_matched,
        "parsed_files": project.file_count,
    }))
}

// ───────────────────────── diff_impact ─────────────────────────

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DiffImpactArgs {
    /// Baseline: the version before the change.
    pub path_v1: String,
    /// The version under suspicion.
    pub path_v2: String,
    /// Restrict the impact graph to this class. Omit to trace every changed method.
    #[serde(default)]
    pub class: Option<String>,
    /// Restrict the impact graph to this method.
    #[serde(default)]
    pub method: Option<String>,
    /// Report only methods that lost a caller.
    ///
    /// This is the regression signal: something that used to call a method no
    /// longer does, which is what a "worked yesterday, 500 today" incident
    /// usually comes down to. Defaults to true — the added-caller and
    /// added-method noise is rarely what you are looking for.
    #[serde(default)]
    pub only_lost_callers: Option<bool>,
    /// Cap how many changes are listed.
    #[serde(default)]
    pub limit: Option<usize>,
}

/// What changed between two versions, and what the change reaches.
///
/// The incident-time question is almost always "what did we deploy", and the
/// signal that carries it is a method losing a caller.
pub fn diff_impact(args: DiffImpactArgs) -> Result<Value, String> {
    // Resolved first, then checked. The order matters in both directions: a `gh:`
    // specifier is not a path, so checking the raw argument rejects every repository
    // reference with "does not exist", which is true and useless; and dropping the check
    // entirely lets a typo'd directory through as "0 methods changed".
    let dir_v1 = source_dir(&args.path_v1, "diff_impact")?;
    let dir_v2 = source_dir(&args.path_v2, "diff_impact")?;

    for (dir, given) in [(&dir_v1, &args.path_v1), (&dir_v2, &args.path_v2)] {
        if !dir.is_dir() {
            return Err(format!("path does not exist: {given}"));
        }
    }

    use crate::analysis::diffimpact::{compute_changes, merge_impact_graphs};
    use crate::analysis::impact::build_impact_from;

    // Parse each version exactly once. build_diff_impact would parse v2 a second
    // time inside build_impact_graph, and tracing N changed methods through it
    // would parse N+1 times -- on a 1667-file solution that is seconds per call.
    // v1's type graph is not needed: the comparison only uses call edges.
    let (_, cg1) = crate::cli::commands::load_project(&dir_v1)
        .map_err(|e| format!("failed to load {}: {e:#}", args.path_v1))?;
    let (tg2, cg2) = crate::cli::commands::load_project(&dir_v2)
        .map_err(|e| format!("failed to load {}: {e:#}", args.path_v2))?;
    let routes2 = crate::route::extractor::extract(&dir_v2.to_string_lossy())
        .map_err(|e| format!("failed to extract routes from {}: {e:#}", args.path_v2))?;

    let changes = compute_changes(&cg1, &cg2);

    // Split the change set: losing a caller is the actionable part, and
    // separating it here means the caller can filter without parsing JSON.
    let mut lost: Vec<Value> = Vec::new();
    let mut rest: Vec<Value> = Vec::new();
    for (method, kind) in &changes {
        let entry = serde_json::to_value(kind).unwrap_or(Value::Null);
        let mut item = json!({ "method": method, "change": entry });
        if let crate::analysis::diffimpact::ChangeKind::CallersChanged {
            removed_callers,
            added_callers,
        } = kind
        {
            item["lost_callers"] = json!(removed_callers.len());
            item["gained_callers"] = json!(added_callers.len());
            if removed_callers.is_empty() {
                rest.push(item);
            } else {
                lost.push(item);
            }
        } else {
            rest.push(item);
        }
    }
    // Most callers lost first: that is the biggest behavioural change.
    lost.sort_by_key(|v| std::cmp::Reverse(v["lost_callers"].as_u64().unwrap_or(0)));

    let only_lost = args.only_lost_callers.unwrap_or(true);
    let mut reported = if only_lost {
        lost.clone()
    } else {
        let mut all = lost.clone();
        all.extend(rest.clone());
        all
    };
    let limit = args.limit.unwrap_or(50).max(1);
    let truncated = reported.len() > limit;
    reported.truncate(limit);

    // Impact: the named target, or every changed method that still exists in v2
    // (a removed method has no callers left to find).
    let (impact, impact_targets) = match (args.class.as_ref(), args.method.as_ref()) {
        (Some(class), Some(method)) => {
            let g = build_impact_from(&tg2, &cg2, &routes2, class, method)
                .map_err(|e| e.to_string())?;
            (Some(g), 1usize)
        }
        _ => {
            let v2_methods: std::collections::HashSet<String> = tg2
                .classes
                .iter()
                .flat_map(|(name, info)| {
                    info.methods
                        .iter()
                        .map(move |m| format!("{}.{}", name, m.method))
                })
                .collect();
            let targets: Vec<(String, String)> = changes
                .keys()
                .filter(|k| v2_methods.contains(*k))
                .filter_map(|k| k.split_once('.'))
                .map(|(c, m)| (c.to_string(), m.to_string()))
                .take(25)
                .collect();
            let graphs: Vec<_> = targets
                .iter()
                .filter_map(|(c, m)| build_impact_from(&tg2, &cg2, &routes2, c, m).ok())
                .collect();
            let n = graphs.len();
            if graphs.is_empty() {
                (None, 0)
            } else {
                (Some(merge_impact_graphs(graphs)), n)
            }
        }
    };

    // The commits between the two references, with their messages.
    //
    // Attached to the same response because the question an investigation is actually
    // asking is "what changed between the deploy that was fine and the one that was
    // not", and a diff of call edges does not say which of those commits a given change
    // belongs to. Answering it needs a second call otherwise, and the caller has to
    // know to make it.
    let commits = match crate::source::commit_range(&args.path_v1, &args.path_v2, 100) {
        None => json!({
            "available": false,
            "reason": "these are local directories, which have no history to read. \
                       Use gh:owner/repo@from and gh:owner/repo@to to get the commit \
                       list with messages.",
        }),
        Some(Err(e)) => json!({"available": false, "reason": e}),
        Some(Ok(range)) => json!({
            "available": true,
            "from": range.from,
            "to": range.to,
            "ahead_by": range.ahead_by,
            "behind_by": range.behind_by,
            "total": range.total,
            "truncated": range.truncated,
            "commits": range.commits,
        }),
    };

    Ok(json!({
        "changes": reported,
        "total_changes": changes.len(),
        "lost_caller_changes": lost.len(),
        "other_changes": rest.len(),
        "only_lost_callers": only_lost,
        "truncated": truncated,
        "commits": commits,
        "impact": impact.map(|g| json!({
            "targets": g.target,
            "affected_methods": g.nodes.len(),
            "edges": g.edges.len(),
        })),
        "impact_targets_traced": impact_targets,
        // 25 is the cap on traced targets; say so rather than letting a caller
        // read a partial graph as complete.
        "impact_capped": impact_targets >= 25,
    }))
}

// ───────────────────────── method_callees ─────────────────────────

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct MethodCalleesArgs {
    pub path: String,
    pub class: String,
    pub method: String,
    /// How many hops to follow (1 = direct callees only).
    #[serde(default = "default_depth")]
    pub depth: usize,
    /// Exclude callees that are not defined in this project (framework calls
    /// such as `ToString` or EF Core internals).
    #[serde(default)]
    pub internal_only: Option<bool>,
    /// Cap how many callees are listed.
    #[serde(default)]
    pub limit: Option<usize>,
}

fn default_depth() -> usize {
    1
}

#[derive(Debug, Serialize)]
pub struct CalleeInfo {
    pub method: String,
    /// True when this project defines the callee.
    pub internal: bool,
    pub depth: usize,
    /// Source lines where the call happens.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub evidence: Vec<usize>,
}

/// What a method calls, breadth-first.
///
/// The complement to `find_callers`: knowing who reaches a method says nothing
/// about what it then does, which is the other half of "why did this fail".
pub fn method_callees(args: MethodCalleesArgs) -> Result<Value, String> {
    let path = source_dir(&args.path, "analyse")?;
    let project = cache::get_or_build(&path).map_err(|e| e.to_string())?;

    let start = format!("{}.{}", args.class, args.method);
    if project
        .type_graph
        .classes
        .get(&args.class)
        .map(|c| !c.methods.iter().any(|m| m.method == args.method))
        .unwrap_or(true)
    {
        return Err(format!(
            "method {start} not found; use project_summary or list_routes to find real names"
        ));
    }

    let depth = args.depth.clamp(1, 10);
    let internal_only = args.internal_only.unwrap_or(false);

    let mut out: Vec<CalleeInfo> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    seen.insert(start.clone());

    let mut frontier = vec![(args.class.clone(), args.method.clone(), 1usize)];
    while let Some((cls, mtd, level)) = frontier.pop() {
        if level > depth {
            continue;
        }
        let current = format!("{}.{}", cls, mtd);
        for call in &project.call_graph.calls {
            if call.caller_class != cls || call.caller_method != mtd {
                continue;
            }
            let (callee_class, _) = crate::analysis::diffimpact::resolve_callee_class(call);
            let callee_id = format!("{}.{}", callee_class, call.callee);
            let internal =
                !callee_class.is_empty() && project.type_graph.classes.contains_key(&callee_class);
            if internal_only && !internal {
                continue;
            }
            if !seen.insert(callee_id.clone()) {
                continue;
            }
            out.push(CalleeInfo {
                method: callee_id.clone(),
                internal,
                depth: level,
                evidence: vec![call.line],
            });
            if level < depth {
                if let Some((c_cls, c_mtd)) = callee_id.split_once('.') {
                    if internal {
                        frontier.push((c_cls.to_string(), c_mtd.to_string(), level + 1));
                    }
                }
            }
        }
        let _ = current;
    }

    out.sort_by(|a, b| {
        a.depth
            .cmp(&b.depth)
            .then_with(|| a.internal.cmp(&b.internal).reverse())
            .then_with(|| a.method.cmp(&b.method))
    });

    let total = out.len();
    let limit = args.limit.unwrap_or(usize::MAX).max(1);
    let window: Vec<CalleeInfo> = out.into_iter().take(limit).collect();

    Ok(json!({
        "method": start,
        "callees": window,
        "total_callees": total,
        "depth": depth,
        "internal_only": internal_only,
        "parsed_files": project.file_count,
    }))
}

// ───────────────────────── project_summary ─────────────────────────

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ProjectSummaryArgs {
    pub path: String,
}

/// Orientation tool: what is in this project before asking anything else.
pub fn project_summary(args: ProjectSummaryArgs) -> Result<Value, String> {
    let project =
        cache::get_or_build(&source_dir(&args.path, "analyse")?).map_err(|e| e.to_string())?;

    let methods: usize = project
        .type_graph
        .classes
        .values()
        .map(|c| c.methods.len())
        .sum();
    let mut classes: Vec<Value> = project
        .type_graph
        .classes
        .iter()
        .map(|(name, info)| {
            json!({
                "name": name,
                "methods": info.methods.len(),
                "is_abstract": info.is_abstract,
            })
        })
        .collect();
    classes
        .sort_by_key(|c| std::cmp::Reverse(c.get("methods").and_then(|m| m.as_u64()).unwrap_or(0)));

    Ok(json!({
        "path": project.root.display().to_string(),
        "files": project.file_count,
        "classes": project.type_graph.classes.len(),
        "interfaces": project.type_graph.interfaces.len(),
        "methods": methods,
        "call_sites": project.call_graph.calls.len(),
        // Largest classes first: that is where a request handler usually lives.
        "largest_classes": classes.into_iter().take(20).collect::<Vec<_>>(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::path::PathBuf;

    /// Write a source file into a fixture directory, creating subdirectories.
    fn write(dir: &Path, name: &str, content: &str) {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    fn fixture_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("tinylink_mcp_{}_{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn find_callers_reports_direct_caller() {
        let _guard = cache::test_guard();
        let dir = fixture_dir("callers");
        std::fs::write(
            dir.join("Svc.cs"),
            r#"
namespace N;
public class Helper { public void Shared() { } }
public class Svc {
    public void Entry() { Shared(); }
    public void Shared() { }
}
"#,
        )
        .unwrap();

        let out = find_callers(FindCallersArgs {
            path: dir.display().to_string(),
            class: "Svc".into(),
            method: "Shared".into(),
            max_distance: None,
            exclude_test_projects: None,
            limit: None,
            offset: None,
        })
        .expect("find_callers");
        assert_eq!(out["target"], "Svc.Shared");
        assert!(out["caller_count"].as_u64().unwrap() >= 1, "{out}");
        cache::clear();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn method_pdg_returns_blocks_within_method_only() {
        let _guard = cache::test_guard();
        let dir = fixture_dir("pdg");
        std::fs::write(
            dir.join("P.cs"),
            r#"
namespace N;
public class P {
    public int Branch(int n) {
        if (n < 0) { return -1; }
        var doubled = n * 2;
        return doubled;
    }
    public void Other() { }
}
"#,
        )
        .unwrap();

        let out = method_pdg(MethodPdgArgs {
            path: dir.display().to_string(),
            class: "P".into(),
            method: "Branch".into(),
            file: None,
        })
        .expect("method_pdg");
        assert_eq!(out["method"], "Branch");
        let blocks = out["blocks"].as_array().unwrap();
        assert!(!blocks.is_empty(), "expected blocks: {out}");
        // No block may start before the method declaration.
        let start = out["line_start"].as_u64().unwrap();
        for b in blocks {
            assert!(
                b["start_line"].as_u64().unwrap() >= start,
                "block outside method range: {b}"
            );
        }
        cache::clear();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn method_pdg_errors_on_unknown_method() {
        let _guard = cache::test_guard();
        let dir = fixture_dir("pdg_missing");
        std::fs::write(dir.join("Q.cs"), "namespace N;\npublic class Q { }\n").unwrap();
        let err = method_pdg(MethodPdgArgs {
            path: dir.display().to_string(),
            class: "Q".into(),
            method: "Nope".into(),
            file: None,
        })
        .expect_err("unknown method must error");
        assert!(err.contains("not found"), "unhelpful error: {err}");
        cache::clear();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_routes_finds_minimal_api_handlers() {
        let _guard = cache::test_guard();
        let dir = fixture_dir("routes");
        std::fs::write(
            dir.join("Endpoint.cs"),
            "namespace N;\npublic class ThingEndpoint { public static IResult Handler() => null; }\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("Ext.cs"),
            r#"
var api = app.MapGroup("");
api.MapGet("/api/thing/{id:guid}", ThingEndpoint.Handler);
"#,
        )
        .unwrap();

        let out = list_routes(ListRoutesArgs {
            path: dir.display().to_string(),
            filter: None,
            http_method: None,
            path_prefix: None,
            limit: None,
            offset: None,
        })
        .expect("list_routes");
        assert_eq!(out["count"], 1, "{out}");
        assert_eq!(out["routes"][0]["pattern"], "/api/thing/{id}");
        assert_eq!(out["routes"][0]["class"], "ThingEndpoint");
        assert_eq!(out["inline_lambda_routes"], 0);
        cache::clear();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_routes_filter_narrows_results() {
        let _guard = cache::test_guard();
        let dir = fixture_dir("routes_filter");
        std::fs::write(
            dir.join("A.cs"),
            "namespace N;\npublic class AlphaEndpoint { public static void Handler() {} }\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("B.cs"),
            "namespace N;\npublic class BetaEndpoint { public static void Handler() {} }\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("Ext.cs"),
            "app.MapGet(\"/api/alpha\", AlphaEndpoint.Handler);\napp.MapPost(\"/api/beta\", BetaEndpoint.Handler);\n",
        )
        .unwrap();

        let out = list_routes(ListRoutesArgs {
            path: dir.display().to_string(),
            filter: Some("alpha".into()),
            http_method: None,
            path_prefix: None,
            limit: None,
            offset: None,
        })
        .expect("list_routes");
        assert_eq!(out["count"], 1, "{out}");
        assert_eq!(out["routes"][0]["class"], "AlphaEndpoint");
        cache::clear();
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A minimal-API handler is reached by a method group reference, not a call
    /// site, so without the route table `find_callers` reports zero callers for
    /// every endpoint. The registration site must appear as a caller and the
    /// route must be reported.
    #[test]
    fn find_callers_attributes_minimal_api_endpoint_to_its_registrar() {
        let _guard = cache::test_guard();
        let dir = fixture_dir("route_caller");
        std::fs::write(
            dir.join("Endpoint.cs"),
            "namespace N;\npublic class GetItemsEndpoint { public static IResult Handler() => null; }\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("Ext.cs"),
            r#"
public static class EndpointExtension
{
    public static IEndpointRouteBuilder MapApi(this IEndpointRouteBuilder app)
    {
        var api = app.MapGroup("");
        api.MapGet("/api/items/{id}", GetItemsEndpoint.Handler);
        return app;
    }
}
"#,
        )
        .unwrap();

        let out = find_callers(FindCallersArgs {
            path: dir.display().to_string(),
            class: "GetItemsEndpoint".into(),
            method: "Handler".into(),
            max_distance: None,
            exclude_test_projects: None,
            limit: None,
            offset: None,
        })
        .expect("find_callers");

        let routes = out["routes"].as_array().expect("routes array");
        assert_eq!(
            routes.len(),
            1,
            "the endpoint's route must be reported: {out}"
        );
        assert_eq!(routes[0]["pattern"], "/api/items/{id}");
        assert_eq!(routes[0]["http_method"], "GET");
        assert_eq!(
            routes[0]["registrar"], "EndpointExtension.MapApi",
            "the registration site is the caller of a minimal-API handler"
        );

        let callers = out["callers"].as_array().expect("callers array");
        assert!(
            callers
                .iter()
                .any(|c| c["method"] == "EndpointExtension.MapApi"),
            "registrar must be counted as a direct caller: {out}"
        );
        let registrar_entry = callers
            .iter()
            .find(|c| c["method"] == "EndpointExtension.MapApi")
            .unwrap();
        assert_eq!(registrar_entry["direct"], true);
        assert_eq!(registrar_entry["distance"], 1);

        cache::clear();
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A plain library method with no route must report no routes.
    #[test]
    fn find_callers_reports_no_routes_for_library_method() {
        let _guard = cache::test_guard();
        let dir = fixture_dir("no_route");
        std::fs::write(
            dir.join("Lib.cs"),
            "namespace N;\npublic class Lib { public void Helper() {} public void Use() { Helper(); } }\n",
        )
        .unwrap();

        let out = find_callers(FindCallersArgs {
            path: dir.display().to_string(),
            class: "Lib".into(),
            method: "Helper".into(),
            max_distance: None,
            exclude_test_projects: None,
            limit: None,
            offset: None,
        })
        .expect("find_callers");
        assert_eq!(out["routes"].as_array().unwrap().len(), 0, "{out}");
        // The real call site is still found.
        assert!(
            out["callers"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["method"] == "Lib.Use"),
            "{out}"
        );
        cache::clear();
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The regression signal: a method that lost a caller between versions.
    ///
    /// This is what a "worked yesterday, 500 today" incident reduces to, and it
    /// is invisible to a plain textual diff because the method itself is
    /// unchanged — only its callers moved.
    #[test]
    fn diff_impact_reports_a_method_that_lost_a_caller() {
        let v1 = fixture_dir("diff_v1");
        let v2 = fixture_dir("diff_v2");

        // v1: both callers reach Shared.
        write(
            &v1,
            "Shared.cs",
            "namespace N;\npublic class Shared { public void Run() {} }\n",
        );
        write(
            &v1,
            "A.cs",
            "namespace N;\npublic class A { public void Go() { new Shared().Run(); } }\n",
        );
        write(
            &v1,
            "B.cs",
            "namespace N;\npublic class B { public void Go() { new Shared().Run(); } }\n",
        );

        // v2: B no longer calls it, A adds a new caller C.
        write(
            &v2,
            "Shared.cs",
            "namespace N;\npublic class Shared { public void Run() {} }\n",
        );
        write(
            &v2,
            "A.cs",
            "namespace N;\npublic class A { public void Go() { new Shared().Run(); } }\n",
        );
        write(
            &v2,
            "B.cs",
            "namespace N;\npublic class B { public void Go() { } }\n",
        );
        write(
            &v2,
            "C.cs",
            "namespace N;\npublic class C { public void Go() { new Shared().Run(); } }\n",
        );

        let out = diff_impact(DiffImpactArgs {
            path_v1: v1.display().to_string(),
            path_v2: v2.display().to_string(),
            class: None,
            method: None,
            only_lost_callers: None,
            limit: None,
        })
        .expect("diff_impact");

        let changes = out["changes"].as_array().expect("changes");
        let shared = changes
            .iter()
            .find(|c| c["method"] == "Shared.Run")
            .unwrap_or_else(|| panic!("Shared.Run must be reported: {out}"));
        assert_eq!(shared["lost_callers"], 1, "{shared}");
        assert_eq!(shared["gained_callers"], 1, "{shared}");

        // The kind must name the actual caller, not just a count.
        let removed = &shared["change"]["CallersChanged"]["removed_callers"];
        assert_eq!(removed[0], "B.Go", "{shared}");

        assert!(out["lost_caller_changes"].as_u64().unwrap() >= 1, "{out}");
        assert!(
            out["impact"]["affected_methods"].as_u64().unwrap() >= 1,
            "{out}"
        );

        std::fs::remove_dir_all(&v1).ok();
        std::fs::remove_dir_all(&v2).ok();
    }

    /// `only_lost_callers` defaults to true, so the added-method noise a caller
    /// does not care about is filtered out of the default response.
    #[test]
    fn diff_impact_only_lost_callers_filter_excludes_added_methods() {
        let v1 = fixture_dir("filter_v1");
        let v2 = fixture_dir("filter_v2");

        write(
            &v1,
            "Keep.cs",
            "namespace N;\npublic class Keep { public void M() {} }\npublic class K1 { public void G() { Keep.M(); } }\n",
        );
        write(
            &v2,
            "Keep.cs",
            "namespace N;\npublic class Keep { public void M() {} }\n",
        );
        write(
            &v2,
            "Brand.cs",
            "namespace N;\npublic class Brand { public void M() {} }\npublic class B1 { public void G() { Brand.M(); } }\n",
        );

        let defaults = diff_impact(DiffImpactArgs {
            path_v1: v1.display().to_string(),
            path_v2: v2.display().to_string(),
            class: None,
            method: None,
            only_lost_callers: None,
            limit: None,
        })
        .expect("diff_impact");
        assert_eq!(defaults["only_lost_callers"], true);
        assert_eq!(
            defaults["changes"].as_array().unwrap().len(),
            0,
            "{defaults}"
        );

        let everything = diff_impact(DiffImpactArgs {
            path_v1: v1.display().to_string(),
            path_v2: v2.display().to_string(),
            class: None,
            method: None,
            only_lost_callers: Some(false),
            limit: None,
        })
        .expect("diff_impact");
        assert!(
            !everything["changes"].as_array().unwrap().is_empty(),
            "opting out of the filter must show everything: {everything}"
        );

        std::fs::remove_dir_all(&v1).ok();
        std::fs::remove_dir_all(&v2).ok();
    }

    /// Identical versions must report no changes rather than claiming every
    /// method changed.
    #[test]
    fn diff_impact_on_identical_versions_reports_nothing() {
        let v1 = fixture_dir("same_v1");
        let v2 = fixture_dir("same_v2");
        let src = "namespace N;\npublic class S { public void A() {} public void B() { A(); } }\n";
        write(&v1, "S.cs", src);
        write(&v2, "S.cs", src);

        let out = diff_impact(DiffImpactArgs {
            path_v1: v1.display().to_string(),
            path_v2: v2.display().to_string(),
            class: None,
            method: None,
            only_lost_callers: None,
            limit: None,
        })
        .expect("diff_impact");
        assert_eq!(out["total_changes"], 0, "{out}");

        std::fs::remove_dir_all(&v1).ok();
        std::fs::remove_dir_all(&v2).ok();
    }

    #[test]
    fn diff_impact_rejects_a_missing_path_with_a_clear_message() {
        let err = diff_impact(DiffImpactArgs {
            path_v1: "/nonexistent/version1".into(),
            path_v2: "/nonexistent/version2".into(),
            class: None,
            method: None,
            only_lost_callers: None,
            limit: None,
        })
        .expect_err("missing path must error");
        assert!(err.contains("does not exist"), "unhelpful error: {err}");
    }

    #[test]
    fn diff_impact_limit_truncates_and_says_so() {
        let v1 = fixture_dir("limit_v1");
        let v2 = fixture_dir("limit_v2");
        write(
            &v1,
            "A.cs",
            "namespace N;\npublic class A { public void M() {} }\npublic class Caller { public void G() { A.M(); } }\n",
        );
        for i in 0..8 {
            write(
                &v2,
                &format!("N{i}.cs"),
                &format!(
                    "namespace N;\npublic class N{i} {{ public void M() {{}} }}\npublic class C{i} {{ public void G() {{ N{i}.M(); }} }}\n"
                ),
            );
        }

        let out = diff_impact(DiffImpactArgs {
            path_v1: v1.display().to_string(),
            path_v2: v2.display().to_string(),
            class: None,
            method: None,
            only_lost_callers: Some(false),
            limit: Some(3),
        })
        .expect("diff_impact");
        assert_eq!(out["changes"].as_array().unwrap().len(), 3, "{out}");
        assert_eq!(
            out["truncated"], true,
            "a truncated list must say so, or a caller reads it as complete: {out}"
        );
        assert!(out["total_changes"].as_u64().unwrap() > 3, "{out}");

        std::fs::remove_dir_all(&v1).ok();
        std::fs::remove_dir_all(&v2).ok();
    }

    /// The outbound half of the graph: find_callers says who reaches a method,
    /// this says what it then does.
    #[test]
    fn method_callees_lists_what_a_method_calls() {
        let dir = fixture_dir("callees");
        write(
            &dir,
            "Svc.cs",
            r#"
namespace N;
public class Repo { public static void Query() {} }
public class Svc {
    public void Handle() { Repo.Query(); }
}
"#,
        );

        let out = method_callees(MethodCalleesArgs {
            path: dir.display().to_string(),
            class: "Svc".into(),
            method: "Handle".into(),
            depth: 1,
            internal_only: None,
            limit: None,
        })
        .expect("method_callees");

        let callees = out["callees"].as_array().expect("callees");
        assert!(
            callees.iter().any(|c| c["method"] == "Repo.Query"),
            "the internal callee must be reported: {out}"
        );
        let repo = callees
            .iter()
            .find(|c| c["method"] == "Repo.Query")
            .unwrap();
        assert_eq!(repo["internal"], true, "{out}");
        assert_eq!(repo["depth"], 1, "{out}");
        // Evidence: the line inside Svc.Handle where the call happens.
        assert!(!repo["evidence"].as_array().unwrap().is_empty(), "{out}");

        cache::clear();
        std::fs::remove_dir_all(&dir).ok();
    }

    /// `internal_only` hides framework and third-party callees, which dominate
    /// a real method's list and bury the project's own code.
    #[test]
    fn method_callees_internal_only_hides_unresolved_callees() {
        let dir = fixture_dir("callees_internal");
        write(
            &dir,
            "Svc.cs",
            r#"
namespace N;
public class Repo { public static void Query() {} }
public class Svc {
    public void Handle() { Repo.Query(); var s = ToString(); }
}
"#,
        );

        let all = method_callees(MethodCalleesArgs {
            path: dir.display().to_string(),
            class: "Svc".into(),
            method: "Handle".into(),
            depth: 1,
            internal_only: None,
            limit: None,
        })
        .unwrap();
        let everything = all["callees"].as_array().unwrap();
        assert!(
            everything.len() > 1,
            "the fixture should also produce an unresolved callee: {all}"
        );

        let internal = method_callees(MethodCalleesArgs {
            path: dir.display().to_string(),
            class: "Svc".into(),
            method: "Handle".into(),
            depth: 1,
            internal_only: Some(true),
            limit: None,
        })
        .unwrap();
        let filtered = internal["callees"].as_array().unwrap();
        assert!(
            filtered.iter().all(|c| c["internal"] == true),
            "internal_only must drop unresolved callees: {internal}"
        );

        cache::clear();
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Documents a real limit of the resolver, so the behaviour is a decision
    /// rather than a surprise: a call through a local variable cannot be typed.
    ///
    /// `Repo.Query()` resolves; `var r = new Repo(); r.Query()` does not, because
    /// nothing tracks that `r` is a `Repo`. Such a callee is still reported —
    /// it is listed with `internal: false` — because a call that exists but
    /// cannot be attributed is information, and silently dropping it would make a
    /// method look simpler than it is.
    #[test]
    fn method_callees_reports_untypeable_calls_as_external() {
        let dir = fixture_dir("callees_untyped");
        write(
            &dir,
            "U.cs",
            r#"
namespace N;
public class Repo { public void Query() {} }
public class Svc {
    public void Handle() { var r = new Repo(); r.Query(); }
}
"#,
        );

        let out = method_callees(MethodCalleesArgs {
            path: dir.display().to_string(),
            class: "Svc".into(),
            method: "Handle".into(),
            depth: 1,
            internal_only: None,
            limit: None,
        })
        .expect("method_callees");

        let callees = out["callees"].as_array().expect("callees");
        assert!(
            !callees.is_empty(),
            "an unresolvable call must still be reported: {out}"
        );
        let untyped = callees.iter().find(|c| c["method"] == ".Query").unwrap();
        assert_eq!(
            untyped["internal"], false,
            "a call with no resolvable owner must not claim to be internal: {untyped}"
        );
        assert!(
            !untyped["evidence"].as_array().unwrap().is_empty(),
            "{untyped}"
        );

        cache::clear();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn method_callees_errors_on_an_unknown_method() {
        let dir = fixture_dir("callees_missing");
        write(
            &dir,
            "Q.cs",
            "namespace N;\npublic class Q { public void M() {} }\n",
        );

        let err = method_callees(MethodCalleesArgs {
            path: dir.display().to_string(),
            class: "Q".into(),
            method: "Nope".into(),
            depth: 1,
            internal_only: None,
            limit: None,
        })
        .expect_err("unknown method must error");
        // The error should point at a discovery tool rather than leave the
        // caller guessing at names.
        assert!(err.contains("project_summary"), "unhelpful error: {err}");

        cache::clear();
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Paging: without offset/limit a long list is silently cut off and reads as
    /// complete.
    #[test]
    fn list_routes_pages_with_offset_and_limit() {
        let dir = fixture_dir("routes_page");
        let mut registration = String::new();
        for i in 0..6 {
            write(
                &dir,
                &format!("E{i}.cs"),
                &format!("namespace N;\npublic class E{i}Endpoint {{ public static IResult Handler() => null; }}\n"),
            );
            registration.push_str(&format!(
                "app.MapGet(\"/api/item{i}\", E{i}Endpoint.Handler);\n"
            ));
        }
        write(&dir, "Ext.cs", &registration);

        let first = list_routes(ListRoutesArgs {
            path: dir.display().to_string(),
            filter: None,
            http_method: None,
            path_prefix: None,
            limit: Some(2),
            offset: Some(0),
        })
        .expect("list_routes");
        assert_eq!(first["count"], 2, "{first}");
        assert_eq!(first["total_matched"], 6, "{first}");
        assert_eq!(first["has_more"], true, "{first}");

        let last = list_routes(ListRoutesArgs {
            path: dir.display().to_string(),
            filter: None,
            http_method: None,
            path_prefix: None,
            limit: Some(2),
            offset: Some(4),
        })
        .expect("list_routes");
        assert_eq!(last["count"], 2, "{last}");
        assert_eq!(
            last["has_more"], false,
            "the last page must say there is nothing more: {last}"
        );

        cache::clear();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_routes_filters_by_http_method_and_path_prefix() {
        let dir = fixture_dir("routes_verb");
        write(
            &dir,
            "A.cs",
            "namespace N;\npublic class AEndpoint { public static IResult Handler() => null; }\n",
        );
        write(
            &dir,
            "B.cs",
            "namespace N;\npublic class BEndpoint { public static IResult Handler() => null; }\n",
        );
        write(
            &dir,
            "Ext.cs",
            "app.MapGet(\"/api/agency/member\", AEndpoint.Handler);\napp.MapPost(\"/api/agency/member\", BEndpoint.Handler);\napp.MapGet(\"/api/billing/invoice\", BEndpoint.Handler);\n",
        );

        let gets = list_routes(ListRoutesArgs {
            path: dir.display().to_string(),
            filter: None,
            http_method: Some("get".into()),
            path_prefix: None,
            limit: None,
            offset: None,
        })
        .expect("list_routes");
        assert_eq!(gets["count"], 2, "{gets}");
        // Case-insensitive on the verb.
        for r in gets["routes"].as_array().unwrap() {
            assert_eq!(r["method"], "GET", "{gets}");
        }

        let agency = list_routes(ListRoutesArgs {
            path: dir.display().to_string(),
            filter: None,
            http_method: Some("GET".into()),
            path_prefix: Some("/API/Agency".into()),
            limit: None,
            offset: None,
        })
        .expect("list_routes");
        assert_eq!(agency["count"], 1, "{agency}");
        assert_eq!(agency["routes"][0]["pattern"], "/api/agency/member");

        cache::clear();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn find_callers_max_distance_keeps_only_direct_callers() {
        let dir = fixture_dir("distance");
        write(
            &dir,
            "Chain.cs",
            r#"
namespace N;
public class Target { public static void M() {} }
public class Near { public void G() { Target.M(); } }
public class Mid { public void G() { Near.G(); } }
public class Far { public void G() { Mid.G(); } }
"#,
        );

        let all = find_callers(FindCallersArgs {
            path: dir.display().to_string(),
            class: "Target".into(),
            method: "M".into(),
            max_distance: None,
            exclude_test_projects: None,
            limit: None,
            offset: None,
        })
        .expect("find_callers");
        assert!(all["caller_count"].as_u64().unwrap() >= 3, "{all}");

        let direct = find_callers(FindCallersArgs {
            path: dir.display().to_string(),
            class: "Target".into(),
            method: "M".into(),
            max_distance: Some(1),
            exclude_test_projects: None,
            limit: None,
            offset: None,
        })
        .expect("find_callers");
        assert_eq!(direct["caller_count"], 1, "{direct}");
        assert_eq!(direct["callers"][0]["method"], "Near.G", "{direct}");
        // Filtering must not hide that it removed something.
        assert!(
            direct["filtered_out"].as_u64().unwrap() > 0,
            "a filtered response must report what it dropped: {direct}"
        );

        cache::clear();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn find_callers_reports_call_sites_as_evidence() {
        let dir = fixture_dir("evidence");
        write(
            &dir,
            "E.cs",
            r#"
namespace N;
public class Target { public void M() {} }
public class Caller {
    public void A() { Target.M(); }
    public void B() { Target.M(); }
}
"#,
        );

        let out = find_callers(FindCallersArgs {
            path: dir.display().to_string(),
            class: "Target".into(),
            method: "M".into(),
            max_distance: None,
            exclude_test_projects: None,
            limit: None,
            offset: None,
        })
        .expect("find_callers");

        let caller = out["callers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["method"] == "Caller.A" || c["method"] == "Caller.B")
            .expect("caller present");
        // Two distinct call sites in the same class, so the evidence has to name
        // a line — this is what lets an agent open the file at the right place.
        assert!(!caller["evidence"].as_array().unwrap().is_empty(), "{out}");

        cache::clear();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn project_summary_counts_classes_and_methods() {
        let _guard = cache::test_guard();
        let dir = fixture_dir("summary");
        std::fs::write(
            dir.join("S.cs"),
            "namespace N;\npublic class S { public void A() {} public void B() {} }\npublic class T { }\n",
        )
        .unwrap();

        let out = project_summary(ProjectSummaryArgs {
            path: dir.display().to_string(),
        })
        .expect("project_summary");
        assert_eq!(out["classes"], 2);
        assert_eq!(out["methods"], 2);
        // Largest class first.
        assert_eq!(out["largest_classes"][0]["name"], "S");
        cache::clear();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn find_patterns_runs_without_panicking() {
        let _guard = cache::test_guard();
        let dir = fixture_dir("patterns");
        std::fs::write(
            dir.join("F.cs"),
            r#"
namespace N;
public interface IShape { double Area(); }
public class Circle : IShape {
    private readonly double _r;
    public Circle(double r) { _r = r; }
    public double Area() => 3.14 * _r * _r;
}
"#,
        )
        .unwrap();

        let out = find_patterns(FindPatternsArgs {
            path: dir.display().to_string(),
            min_confidence: None,
            limit: None,
            offset: None,
        })
        .expect("find_patterns");
        assert!(out["patterns"].is_array(), "{out}");
        cache::clear();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn min_confidence_filters_patterns() {
        let _guard = cache::test_guard();
        let dir = fixture_dir("patterns_filter");
        std::fs::write(
            dir.join("F.cs"),
            "namespace N;\npublic class F { public void A() {} public void B() {} public void C() {} }\n",
        )
        .unwrap();

        let all = find_patterns(FindPatternsArgs {
            path: dir.display().to_string(),
            min_confidence: None,
            limit: None,
            offset: None,
        })
        .unwrap();
        let strict = find_patterns(FindPatternsArgs {
            path: dir.display().to_string(),
            min_confidence: Some(0.95),
            limit: None,
            offset: None,
        })
        .unwrap();
        assert!(
            strict["count"].as_u64().unwrap() <= all["count"].as_u64().unwrap(),
            "higher threshold must not return more patterns"
        );
        cache::clear();
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The forest has to be readable in one pass: a parent before its children.
    ///
    /// Two regions can start on the same line, and when they do the wider one contains
    /// the narrower. Ordering by `(start, end)` ascending therefore emits the child
    /// first, and a reader that attaches depth as it goes — or a traversal that walks
    /// down and later walks back up — meets a parent id it has not seen yet.
    #[test]
    fn a_parent_is_always_listed_before_its_children() {
        let _guard = cache::test_guard();
        let dir = std::env::temp_dir().join("tiny_pdg_hammock_order");
        std::fs::create_dir_all(&dir).unwrap();

        // Two nested regions sharing a start line: the guard is on line 20, and both the
        // `if` it belongs to and its body start there.
        std::fs::write(
            dir.join("Endpoint.cs"),
            "namespace N;
public class Endpoint
{
    public static void Handler(int id)
    {
        if (id == 0)
        {
            throw new ArgumentException();
        }
        while (id > 0)
        {
            id--;
        }
    }
}
",
        )
        .unwrap();

        let value = super::method_hammocks(MethodHammocksArgs {
            path: dir.display().to_string(),
            class: "Endpoint".into(),
            method: "Handler".into(),
            file: None,
            min_blocks: None,
        })
        .unwrap();

        let regions = value["regions"].as_array().expect("regions");
        assert!(regions.len() >= 2, "the fixture nests: {value}");

        let seen: std::collections::HashSet<&str> =
            regions.iter().map(|r| r["id"].as_str().unwrap()).collect();

        for region in regions {
            match region["parent_id"].as_str() {
                None => assert_eq!(region["depth"], 0, "{region}"),
                Some(parent) => {
                    assert!(
                        seen.contains(parent),
                        "{parent} is referenced but never listed"
                    );
                    // The whole point: the parent was already emitted.
                    let parent_index = regions
                        .iter()
                        .position(|r| r["id"].as_str() == Some(parent))
                        .expect("listed");
                    let own_index = regions
                        .iter()
                        .position(|r| r["id"] == region["id"])
                        .expect("listed");
                    assert!(
                        parent_index < own_index,
                        "{} at {own_index} is listed before its parent {} at {parent_index}",
                        region["id"],
                        parent
                    );
                    assert_eq!(
                        region["depth"].as_u64(),
                        Some(region["depth"].as_u64().unwrap().max(1))
                    );
                }
            }
        }

        cache::clear();
        std::fs::remove_dir_all(&dir).ok();
    }
}

// ───────────────────────── search_github ─────────────────────────

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SearchArgs {
    /// A repository name, a topic, or code to find.
    pub query: String,
    /// `repositories` (default) to find projects, or `code` to find files inside them.
    ///
    /// Code search needs a token; repository search does not.
    #[serde(default)]
    pub kind: Option<String>,
    /// Keep a code search inside one repository, as `owner/name`.
    #[serde(default)]
    pub repository: Option<String>,
    /// How many results to return, 1 to 100.
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Find code on GitHub. Results come back as `path` arguments the other tools accept.
pub fn search_github(args: SearchArgs) -> Result<Value, String> {
    use crate::github::{self, SearchKind};

    let kind = match args.kind.as_deref() {
        None | Some("") => SearchKind::Repositories,
        Some(raw) => SearchKind::parse_public(raw).map_err(|e| e.to_string())?,
    };

    let results = github::search(
        &args.query,
        kind,
        args.limit.unwrap_or(10),
        args.repository.as_deref(),
    )
    .map_err(|e| e.to_string())?;

    let mut value = serde_json::to_value(&results).map_err(|e| e.to_string())?;
    value["how_to_use"] = json!(github::HOW_TO_USE);
    Ok(value)
}
