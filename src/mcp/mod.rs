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

use tools::{FindCallersArgs, FindPatternsArgs, ListRoutesArgs, MethodPdgArgs, ProjectSummaryArgs};

pub struct AnalysisServer {
    tool_router: ToolRouter<AnalysisServer>,
}

impl Default for AnalysisServer {
    fn default() -> Self {
        Self::new()
    }
}

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
        description = "Find direct and transitive callers of a method. Use this for \"what breaks if I change this\" and \"what could have called this\". For minimal-API handlers the caller is the route registration site.",
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
             class and method names, then use list_routes or find_callers to navigate."
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
    fn router_registers_exactly_the_five_tools() {
        let router = AnalysisServer::tool_router();
        let names: Vec<String> = router
            .list_all()
            .iter()
            .map(|t| t.name.to_string())
            .collect();
        for expected in [
            "project_summary",
            "list_routes",
            "find_callers",
            "method_pdg",
            "find_patterns",
        ] {
            assert!(
                names.contains(&expected.to_string()),
                "missing {expected}: {names:?}"
            );
        }
        assert_eq!(names.len(), 5, "unexpected tool set: {names:?}");
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
