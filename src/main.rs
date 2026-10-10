use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "tiny-pdg-cs",
    about = "C# Program Dependence Graph (PDG) builder"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Parse C# source and output type graph as JSON
    Parse {
        #[arg(help = "Path to .cs file")]
        file: String,
    },
    /// Search GitHub for repositories or code
    Search {
        #[arg(help = "What to look for: a repository name, a topic, or code to find")]
        query: String,
        #[arg(
            long,
            help = "repositories (default) or code. Code search needs GITHUB_TOKEN"
        )]
        kind: Option<String>,
        /// Limit code search to one repository, as `owner/name`
        #[arg(long, value_name = "OWNER/NAME")]
        repo: Option<String>,
        #[arg(long, default_value = "10", help = "How many results (1-100)")]
        limit: usize,
        #[arg(long, help = "JSON output instead of a table")]
        json: bool,
    },
    /// Build Control Flow Graph from C# source
    Cfg {
        #[arg(help = "Path to .cs file")]
        file: String,
        #[arg(long, help = "Output format: dot, json")]
        format: Option<String>,
    },
    /// Build Program Dependence Graph
    Pdg {
        #[arg(help = "Path to .cs file")]
        file: String,
        #[arg(long, help = "Output format: dot, json")]
        format: Option<String>,
    },
    /// Identify hammock blocks in CFG
    Hammock {
        #[arg(help = "Path to .cs file")]
        file: String,
        #[arg(long, help = "Granularity level: module, class, function, statement")]
        level: Option<String>,
    },
    /// Resolve calls through DI/Reflection/CHA
    Resolve {
        #[arg(help = "Path to .cs file or project directory")]
        path: String,
        #[arg(long, help = "Resolution kind: di, factory, reflection, cha, all")]
        kind: Option<String>,
    },
    /// Show focused call graph for a class or full solution
    Callgraph {
        #[arg(help = "Path to .cs file or project directory")]
        path: String,
        #[arg(long, help = "Focus on a specific class name")]
        class: Option<String>,
        #[arg(long, default_value = "3", help = "Max trace depth")]
        depth: usize,
        #[arg(long, help = "Show only outbound calls from class")]
        outbound: bool,
        #[arg(long, help = "Show only inbound calls to class")]
        inbound: bool,
        #[arg(
            long,
            help = "Trace dispatch: show possible implementations and their call graphs"
        )]
        trace: bool,
    },
    /// Detect design patterns in C# source code
    Detect {
        #[arg(help = "Path to .cs file or project directory")]
        path: String,
    },
    /// Print where a source reference resolves to, and nothing else
    Where {
        #[arg(help = "A directory, or gh:owner/repo@ref[:subpath]")]
        spec: String,
    },
    /// Serve static code analysis over MCP for AI agents
    Serve {
        /// Serve over Streamable HTTP instead of stdio
        #[arg(long)]
        http: bool,
        /// Bind address for --http (loopback by default; no authentication)
        #[arg(long, default_value = "127.0.0.1")]
        bind: std::net::IpAddr,
        #[arg(long, default_value = "8080")]
        port: u16,
        /// Endpoint path for --http
        #[arg(long, default_value = "mcp")]
        path: String,
    },
    /// Extract HTTP routes from C# files (Controllers + Minimal APIs)
    Route {
        #[arg(help = "Path to .cs file or project directory")]
        path: String,
        #[arg(long, help = "Output as JSON")]
        json: bool,
    },
    /// Interactive PRAXIS-style code traversal
    Traverse {
        #[arg(help = "Path to .cs file or project directory")]
        path: String,
        #[arg(long, help = "Class name to start traversal from")]
        class: String,
        #[arg(long, help = "Incident context description")]
        context: Option<String>,
    },
    /// Impact analysis: shows a DOT graph of all places affected by changing a method
    Impact {
        #[arg(help = "Path to .cs file or project directory")]
        path: String,
        #[arg(long, help = "Target class name")]
        class: String,
        #[arg(long, help = "Target method name")]
        method: String,
    },
    /// Diff-impact: compare two versions of the same project and show changes + affected places
    Diffimpact {
        #[arg(help = "Path to version 1 (old)")]
        v1: String,
        #[arg(help = "Path to version 2 (new)")]
        v2: String,
        #[arg(long, help = "Target class name for impact graph")]
        class: String,
        #[arg(long, help = "Target method name for impact graph")]
        method: String,
    },
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    // Wrapped in a closure so a source specifier can be resolved with `?`: a fetch
    // failure has to abort before the command runs, not be turned into a confusing
    // error from inside whichever handler happened to be handed a bad path.
    //
    // The closure's value is the match's value. Ending it with `Ok(())` instead would
    // discard every handler's Result -- which compiles with a warning and turns every
    // failure into a silent exit code 0.
    let result: anyhow::Result<()> = (|| match cli.command {
        Commands::Search {
            query,
            kind,
            repo,
            limit,
            json: as_json,
        } => tiny_pdg_cs::cli::commands::handle_search(
            &query,
            kind.as_deref(),
            repo.as_deref(),
            limit,
            as_json,
        ),
        Commands::Parse { file } => {
            tiny_pdg_cs::cli::commands::handle_parse(&source_arg(&file, false)?)
        }
        Commands::Cfg { file, format } => {
            tiny_pdg_cs::cli::commands::handle_cfg(&source_arg(&file, false)?, format.as_deref())
        }
        Commands::Pdg { file, format } => {
            tiny_pdg_cs::cli::commands::handle_pdg(&source_arg(&file, false)?, format.as_deref())
        }
        Commands::Hammock { file, level: _ } => {
            tiny_pdg_cs::cli::commands::handle_hammock(&source_arg(&file, false)?, None)
        }
        Commands::Resolve { path, kind } => {
            tiny_pdg_cs::cli::commands::handle_resolve(&path, kind.as_deref())
        }
        Commands::Detect { path } => {
            tiny_pdg_cs::cli::commands::handle_detect(&source_arg(&path, true)?)
        }
        Commands::Where { spec } => {
            // announce() first, so a reference nobody has fetched still says it fetched.
            // It says nothing on a cache hit on purpose -- printing on every invocation
            // is how a useful message becomes noise -- which is why this command exists:
            // without it there is no way to ask where a reference resolved.
            tiny_pdg_cs::source::announce(&spec);
            println!("{}", tiny_pdg_cs::source::resolve(&spec)?.dir.display());
            Ok(())
        }
        Commands::Callgraph {
            path,
            class,
            depth,
            outbound,
            inbound,
            trace,
        } => tiny_pdg_cs::cli::commands::handle_callgraph(
            &source_arg(&path, true)?,
            class.as_deref(),
            depth,
            outbound,
            inbound,
            trace,
        ),
        Commands::Serve {
            http,
            bind,
            port,
            path,
        } => serve(http, bind, port, path),
        Commands::Route { path, json } => {
            tiny_pdg_cs::cli::commands::handle_route(&source_arg(&path, true)?, json)
        }
        Commands::Traverse {
            path,
            class,
            context,
        } => tiny_pdg_cs::cli::commands::handle_traverse(
            &source_arg(&path, true)?,
            &class,
            context.as_deref(),
        ),
        Commands::Impact {
            path,
            class,
            method,
        } => tiny_pdg_cs::cli::commands::handle_impact(&source_arg(&path, true)?, &class, &method),
        Commands::Diffimpact {
            v1,
            v2,
            class,
            method,
        } => tiny_pdg_cs::cli::commands::handle_diffimpact(
            &source_arg(&v1, true)?,
            &source_arg(&v2, true)?,
            &class,
            &method,
        ),
    })();

    if let Err(e) = result {
        // `{:#}`, not `{}`: anyhow prints only the outermost context with `{}`, and the
        // context is the *summary*. A git failure reported as "running git to fetch X"
        // with the underlying errno missing sends the reader to the wrong place.
        eprintln!("Error: {e:#}");
        std::process::exit(1);
    }
}

/// Turn a `gh:owner/repo@ref` argument into a local path before a command runs.
///
/// Every command that takes a source goes through here, so GitHub support is one
/// insertion point rather than one per handler. The `file` commands are told apart from
/// the `path` ones because the subpath of a specifier may name either, and only the
/// directory form is checked for being a directory.
fn source_arg(spec: &str, want_directory: bool) -> anyhow::Result<String> {
    tiny_pdg_cs::source::announce(spec);
    let resolved = if want_directory {
        tiny_pdg_cs::source::resolve(spec)?.dir
    } else {
        tiny_pdg_cs::source::resolve_path(spec)?
    };
    Ok(resolved.to_string_lossy().into_owned())
}

/// Run the MCP server.
///
/// Stdio by default, which is what MCP clients launch. `--http` serves Streamable
/// HTTP instead, for shared or remote deployments.
#[cfg(feature = "mcp")]
fn serve(http: bool, bind: std::net::IpAddr, port: u16, path: String) -> anyhow::Result<()> {
    if http {
        return serve_http(bind, port, path);
    }
    serve_stdio()
}

#[cfg(all(feature = "mcp", not(feature = "mcp-http")))]
fn serve_http(_bind: std::net::IpAddr, _port: u16, _path: String) -> anyhow::Result<()> {
    anyhow::bail!(
        "`serve --http` requires the `mcp-http` feature: \
         cargo build --features mcp-http"
    )
}

#[cfg(feature = "mcp-http")]
fn serve_http(bind: std::net::IpAddr, port: u16, path: String) -> anyhow::Result<()> {
    tiny_pdg_cs::mcp::http::serve_http(tiny_pdg_cs::mcp::http::HttpArgs { bind, port, path })
}

#[cfg(feature = "mcp")]
fn serve_stdio() -> anyhow::Result<()> {
    use rmcp::transport::stdio;
    use rmcp::ServiceExt;

    let server = tiny_pdg_cs::mcp::AnalysisServer::new();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(async move {
        let service = server.serve(stdio()).await?;
        service.waiting().await?;
        Ok::<(), anyhow::Error>(())
    })
}

/// The `mcp` feature is required for `serve`; keep the other commands usable
/// without it.
#[cfg(not(feature = "mcp"))]
fn serve(_http: bool, _bind: std::net::IpAddr, _port: u16, _path: String) -> anyhow::Result<()> {
    anyhow::bail!("`serve` requires the `mcp` feature: cargo build --features mcp")
}
