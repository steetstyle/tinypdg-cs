//! MCP server exposing static code analysis to an AI agent.
//!
//! Separate from any trace/telemetry tooling on purpose: these tools answer
//! questions about *code* ("who calls this method", "what does its PDG look
//! like"), which need no runtime data and work on any checkout.
//!
//! Start with `tiny-pdg-cs serve`, then connect an MCP client over stdio.

pub mod cache;
#[cfg(feature = "mcp-http")]
pub mod http;
pub mod tools;

use rmcp::{
    handler::server::router::tool::ToolRouter,
    handler::server::wrapper::Parameters,
    model::{ServerCapabilities, ServerConfig},
    tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler,
};
use serde_json::Value;

use tools::{
    DiffImpactArgs, FindCallersArgs, FindPatternsArgs, ListRoutesArgs, MethodCalleesArgs,
    MethodHammocksArgs, MethodPdgArgs, ProjectSummaryArgs,
};

pub struct AnalysisServer {
    tool_router: ToolRouter<AnalysisServer>,
}

impl Default for AnalysisServer {
    fn default() -> Self {
        Self::new()
    }
}

/// The tool names this server serves, in listing order.
///
/// Declared here rather than written out in each test because a test that asserts a
/// number it also has to remember to update stops being a check: `tools_are_callable_
/// over_http` was asserting 8 while the router served 9, because `search_github` was
/// added and the number was not. The point of the assertion is that the transport does
/// not lose tools, and a hand-kept count cannot tell lost from added.
pub const LISTED_TOOLS: [&str; 9] = [
    "project_summary",
    "list_routes",
    "find_callers",
    "method_pdg",
    "method_callees",
    "find_patterns",
    "diff_impact",
    "method_hammocks",
    "search_github",
];

#[tool_router(router = tool_router)]
impl AnalysisServer {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    /// Orientation: what classes, methods and call sites are in this project.
    /// Start here before asking anything else, so later calls can name real
    /// classes and methods instead of guessing.
    #[tool(
        name = "project_summary",
        description = "List classes, method counts and call-site totals for a C# project or file. Largest classes come first, since HTTP entry points usually live there. Use this first to learn the real class and method names in a codebase.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub async fn project_summary(
        &self,
        args: Parameters<ProjectSummaryArgs>,
    ) -> Result<String, McpError> {
        wrap(tools::project_summary(args.0))
    }

    /// HTTP routes declared in the project.
    #[tool(
        name = "list_routes",
        description = "List HTTP routes with their handler class and method. Supports MVC controllers and minimal APIs including the MapGet(\"/path\", Type.Handler) idiom. Routes registered with inline lambdas are reported separately because they have no named handler to map to.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub async fn list_routes(&self, args: Parameters<ListRoutesArgs>) -> Result<String, McpError> {
        wrap(tools::list_routes(args.0))
    }

    /// Who calls a method, directly and transitively.
    #[tool(
        name = "find_callers",
        description = "Find direct and transitive callers of a method, with the source lines of each call site as evidence. Use it for \"what breaks if I change this\" and \"what could have called this\". For minimal-API handlers the caller is the route registration site. Narrow with max_distance, exclude_test_projects, limit and offset.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub async fn find_callers(
        &self,
        args: Parameters<FindCallersArgs>,
    ) -> Result<String, McpError> {
        wrap(tools::find_callers(args.0))
    }

    /// Program dependence graph for one method.
    #[tool(
        name = "method_pdg",
        description = "Build the program dependence graph (CFG plus control and data dependences) for a single method. Returns basic blocks with line ranges and control/data edges. Use it to reason about which statements a branch depends on.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub async fn method_pdg(&self, args: Parameters<MethodPdgArgs>) -> Result<String, McpError> {
        wrap(tools::method_pdg(args.0))
    }

    /// Nested single-entry, single-exit regions of one method.
    #[tool(
        name = "method_hammocks",
        description = "Restructure a method into hammock blocks: single-entry, single-exit regions (Johnson '94) that nest, so a loop body sits inside a conditional inside the method. Returns a containment forest where each region names its parent and its depth, giving module, class, function and statement granularity in one pass. Prefer this to method_pdg when the question is which structured region a symptom falls in: it returns far fewer, larger regions, and the parent link is what lets a traversal zoom back out.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub async fn method_hammocks(
        &self,
        args: Parameters<MethodHammocksArgs>,
    ) -> Result<String, McpError> {
        wrap(tools::method_hammocks(args.0))
    }

    /// Design-pattern detections.
    #[tool(
        name = "find_patterns",
        description = "Detect design patterns (GoF, DI container, controller/API, MediatR and others) in a project with a confidence score per detection. Useful for orienting in unfamiliar code.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub async fn find_patterns(
        &self,
        args: Parameters<FindPatternsArgs>,
    ) -> Result<String, McpError> {
        wrap(tools::find_patterns(args.0))
    }

    /// What changed between two versions, and what the change reaches.
    #[tool(
        name = "diff_impact",
        description = "Compare two versions of a C# project and report what changed plus the blast radius of that change. Use it for the post-incident question \"what did we deploy that broke this\": the signal that matters is a method that LOST a caller, which is what only_lost_callers filters for (it defaults to true).",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub async fn diff_impact(&self, args: Parameters<DiffImpactArgs>) -> Result<String, McpError> {
        wrap(tools::diff_impact(args.0))
    }

    /// What a method calls.
    #[tool(
        name = "method_callees",
        description = "List what a method calls, following the call graph outwards to a given depth. The complement to find_callers: knowing who reaches a method says nothing about what it then does. Set internal_only to hide framework and third-party callees.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub async fn method_callees(
        &self,
        args: Parameters<MethodCalleesArgs>,
    ) -> Result<String, McpError> {
        wrap(tools::method_callees(args.0))
    }

    /// Search GitHub for repositories or code.
    #[tool(
        name = "search_github",
        description = "Find code on GitHub and get back results you can analyse immediately. `kind` is `repositories` to find projects, or `code` to find files inside them -- code search needs GITHUB_TOKEN and covers the private repositories the token can see as well as the public ones; repository search works without a token. Pass `repository` as owner/name to keep a code search inside one project.\n\nEvery result carries a `specifier`, and a specifier is a `path` argument: pass it to method_pdg, method_hammocks, find_callers, find_patterns, list_routes or diff_impact unchanged. Nothing is needed between finding something and analysing it.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    pub async fn search_github(
        &self,
        args: Parameters<tools::SearchArgs>,
    ) -> Result<String, McpError> {
        wrap(tools::search_github(args.0))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for AnalysisServer {
    fn get_info(&self) -> ServerConfig {
        // ServerInfo is #[non_exhaustive], so build and mutate rather than
        // using struct update syntax.
        let mut info = ServerConfig::default();
        info.instructions = Some(
            "Static analysis of C# code: classes, HTTP routes, call graphs, program \
             dependence graphs and design-pattern detection. All tools are read-only \
             and need no runtime data. Start with project_summary to learn the actual \
             class and method names, then use list_routes or find_callers to navigate.\n\
             \
             A `path` may be a directory or a repository reference: gh:owner/repo, \
             gh:owner/repo@branch, gh:owner/repo@40-hex-commit, or gh:owner/repo@ref:sub/dir \
             for a subdirectory. search_github returns results already in that form, so \
             finding code and analysing it takes no step in between. Repositories are \
             fetched once and cached."
                .into(),
        );
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info
    }
}

/// Tools return JSON strings so the result stays machine-readable and an agent
/// can read it without guessing a format. A wrong path or method must not kill
/// the server, so failures become MCP errors.
fn wrap(result: Result<Value, String>) -> Result<String, McpError> {
    result
        .map(|value| value.to_string())
        .map_err(|message| McpError::invalid_params(message, None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn router_registers_the_analysis_tools() {
        let router = AnalysisServer::tool_router();
        let names: Vec<String> = router
            .list_all()
            .iter()
            .map(|t| t.name.to_string())
            .collect();
        for expected in LISTED_TOOLS {
            assert!(
                names.contains(&expected.to_string()),
                "missing {expected}: {names:?}"
            );
        }
        assert_eq!(
            names.len(),
            LISTED_TOOLS.len(),
            "unexpected tool set: {names:?}"
        );
    }

    /// The listing and the router must agree, and this is the assertion that notices.
    ///
    /// Everything else in this file checks the router against the list, which cannot
    /// catch the list falling behind -- it is checked against the same list. This one
    /// goes the other way and is what catches a tool added to one and not the other.
    #[test]
    fn the_listing_and_the_router_agree() {
        let router = AnalysisServer::tool_router();
        let from_router: std::collections::BTreeSet<String> = router
            .list_all()
            .iter()
            .map(|t| t.name.to_string())
            .collect();
        let from_const: std::collections::BTreeSet<String> =
            LISTED_TOOLS.iter().map(|t| t.to_string()).collect();

        assert_eq!(
            from_router, from_const,
            "LISTED_TOOLS and the router disagree; a tool was added to one and not the other"
        );
    }

    #[test]
    fn every_tool_is_marked_read_only() {
        let router = AnalysisServer::tool_router();
        for tool in router.list_all() {
            let annotations = tool
                .annotations
                .as_ref()
                .unwrap_or_else(|| panic!("{} has no annotations", tool.name));
            assert_eq!(
                annotations.read_only_hint,
                Some(true),
                "{} must be advertised as read-only",
                tool.name
            );
            assert!(
                tool.description.is_some(),
                "{} needs a description for the agent to know when to call it",
                tool.name
            );
        }
    }

    #[test]
    fn server_info_declares_tools_and_instructions() {
        let server = AnalysisServer::new();
        let info = server.get_info();
        assert!(
            info.capabilities.tools.is_some(),
            "tools capability must be advertised"
        );
        let instructions = info.instructions.expect("instructions");
        assert!(
            instructions.contains("project_summary"),
            "instructions should point at the orientation tool"
        );
    }

    #[test]
    fn tool_errors_surface_as_mcp_errors() {
        let err = wrap(Err("method X.Y not found".to_string()));
        assert!(err.is_err(), "a tool failure must be an Err, not a panic");
        assert!(wrap(Ok(Value::Null)).is_ok());
    }
}
