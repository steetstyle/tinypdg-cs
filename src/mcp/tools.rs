//! MCP tool implementations.
//!
//! Every tool returns structured JSON rather than DOT. An agent has to parse
//! DOT to use it, which wastes tokens and invites mistakes; the DOT renderers
//! in `cli::commands` stay for humans.

use std::path::Path;

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
}

pub fn find_callers(args: FindCallersArgs) -> Result<Value, String> {
    let path = Path::new(&args.path);
    let project = cache::get_or_build(path).map_err(|e| e.to_string())?;

    let impact = crate::analysis::impact::build_impact_graph(path, &args.class, &args.method)
        .map_err(|e| e.to_string())?;

    let (graph, _, _) = impact;

    // Nodes other than the target are its callers; edge direction is
    // caller → callee, so an edge INTO the target names a direct caller.
    let target = graph.target.clone();
    let direct: std::collections::HashSet<&String> = graph
        .edges
        .iter()
        .filter(|(_, callee)| callee == &target)
        .map(|(caller, _)| caller)
        .collect();

    let mut callers: Vec<CallerInfo> = graph
        .nodes
        .iter()
        .filter(|(node, _)| *node != &target)
        .map(|(node, (direct_count, _))| CallerInfo {
            method: node.clone(),
            direct: direct.contains(node) || *direct_count > 0,
            // The impact graph records counts, not per-node depth; direct
            // callers get 1 and everything else is reported as transitive.
            distance: if direct.contains(node) || *direct_count > 0 {
                1
            } else {
                2
            },
        })
        .collect();
    callers.sort_by(|a, b| {
        a.distance
            .cmp(&b.distance)
            .then_with(|| a.method.cmp(&b.method))
    });

    Ok(json!({
        "target": target,
        "project": project.root.display().to_string(),
        "caller_count": callers.len(),
        "callers": callers,
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

pub fn method_pdg(args: MethodPdgArgs) -> Result<Value, String> {
    let project = cache::get_or_build(Path::new(&args.path)).map_err(|e| e.to_string())?;

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
}

pub fn list_routes(args: ListRoutesArgs) -> Result<Value, String> {
    let (table, stats) =
        crate::route::extractor::extract_with_stats(&args.path).map_err(|e| e.to_string())?;

    let needle = args.filter.as_ref().map(|f| f.to_lowercase());
    let routes: Vec<Value> = table
        .routes
        .iter()
        .filter(|r| match needle {
            None => true,
            Some(ref n) => {
                r.path.to_lowercase().contains(n)
                    || r.class.to_lowercase().contains(n)
                    || r.handler.to_lowercase().contains(n)
            }
        })
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
        // Routes registered with an inline lambda have no named handler, so
        // they cannot be mapped to a method. Report them: a low mapping rate
        // should be explainable.
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
}

pub fn find_patterns(args: FindPatternsArgs) -> Result<Value, String> {
    let project = cache::get_or_build(Path::new(&args.path)).map_err(|e| e.to_string())?;

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
    let matches: Vec<Value> = detections
        .into_iter()
        .filter(|d| d.confidence >= threshold)
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
    let project = cache::get_or_build(Path::new(&args.path)).map_err(|e| e.to_string())?;

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
    use std::path::PathBuf;

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
        })
        .unwrap();
        let strict = find_patterns(FindPatternsArgs {
            path: dir.display().to_string(),
            min_confidence: Some(0.95),
        })
        .unwrap();
        assert!(
            strict["count"].as_u64().unwrap() <= all["count"].as_u64().unwrap(),
            "higher threshold must not return more patterns"
        );
        cache::clear();
        std::fs::remove_dir_all(&dir).ok();
    }
}
