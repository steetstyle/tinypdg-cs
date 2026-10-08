//! Serving the MCP server over Streamable HTTP.
//!
//! # Why this transport
//!
//! The MCP spec marks the standalone SSE transport as deprecated and replaced it
//! with Streamable HTTP; the Rust SDK reflects that and ships no standalone SSE
//! server. So the supported pair is:
//!
//! - **stdio** — an MCP client launches the binary. Best for a local agent or a
//!   desktop app: no port, no auth, no listening socket.
//! - **Streamable HTTP** — a remote client POSTs to one endpoint. Needed for a
//!   shared server, a container, or several clients at once.
//!
//! Both speak the same protocol; only the framing differs, so the tool surface
//! is identical and nothing downstream has to care.
//!
//! # Security note
//!
//! This binds loopback by default and has no authentication. MCP's authorization
//! spec expects an OAuth flow in front of a remote endpoint, and that is not
//! implemented here — so do not bind it to a public interface as it stands.

use std::net::{IpAddr, SocketAddr};

use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::tower::StreamableHttpService;

use crate::mcp::AnalysisServer;

/// Default mount path. `/mcp` is what most clients expect and it leaves the rest
/// of the port free for a health endpoint.
pub const DEFAULT_PATH: &str = "/mcp";

/// How to serve over HTTP.
#[derive(Debug, Clone, clap::Parser)]
pub struct HttpArgs {
    /// Address to bind. Loopback only by default: this server has no auth.
    #[arg(long, default_value = "127.0.0.1")]
    pub bind: IpAddr,
    #[arg(long, default_value = "8080")]
    pub port: u16,
    /// Path to mount the MCP endpoint at
    #[arg(long, default_value = DEFAULT_PATH)]
    pub path: String,
}

/// Serve until the process is stopped.
///
/// Returns once the listener stops. Intended to be the whole body of a
/// subcommand, hence the blocking call.
pub fn serve_http(args: HttpArgs) -> anyhow::Result<()> {
    let addr = SocketAddr::new(args.bind, args.port);

    if !args.bind.is_loopback() {
        // Not fatal — an operator may be fronting this with their own proxy —
        // but it must not be silent, since there is no authorization here.
        tracing::warn!(
            "binding {addr} on a non-loopback address without authentication; \
             put an authenticating proxy in front before exposing it"
        );
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(run(args, addr))
}

async fn run(args: HttpArgs, addr: SocketAddr) -> anyhow::Result<()> {
    let path = if args.path.starts_with('/') {
        args.path.clone()
    } else {
        format!("/{}", args.path)
    };

    let service: StreamableHttpService<AnalysisServer, LocalSessionManager> =
        StreamableHttpService::new(
            || Ok(AnalysisServer::new()),
            Default::default(),
            Default::default(),
        );

    let router = axum::Router::new().nest_service(&path, service);

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| anyhow::anyhow!("cannot bind {addr}: {e}"))?;
    let local = listener.local_addr().unwrap_or(addr);
    // Logs go to stderr; stdout belongs to the stdio transport.
    eprintln!("tiny-pdg-cs MCP listening on http://{local}{path} (streamable HTTP)");

    axum::serve(listener, router)
        .await
        .map_err(|e| anyhow::anyhow!("server stopped: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser as _;

    #[test]
    fn http_args_default_to_loopback_without_auth() {
        let args = HttpArgs::parse_from(["tiny-pdg-cs"]);
        assert!(
            args.bind.is_loopback(),
            "must not listen publicly by default"
        );
        assert_eq!(args.port, 8080);
        assert_eq!(args.path, DEFAULT_PATH);
    }

    #[test]
    fn http_args_parse_overrides() {
        let args =
            HttpArgs::parse_from(["x", "--bind", "0.0.0.0", "--port", "9000", "--path", "rpc"]);
        assert_eq!(args.bind.to_string(), "0.0.0.0");
        assert_eq!(args.port, 9000);
        assert_eq!(args.path, "rpc");
    }

    #[test]
    fn path_is_normalised_to_leading_slash() {
        let args = HttpArgs::parse_from(["x", "--path", "rpc"]);
        let path = if args.path.starts_with('/') {
            args.path.clone()
        } else {
            format!("/{}", args.path)
        };
        assert_eq!(path, "/rpc");
    }
}
