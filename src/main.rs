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
    /// Embed a project's symbols into a vector store
    Embed {
        #[arg(help = "Project directory with C# sources")]
        path: String,
        /// Where the vectors live: a path, sqlite:<path>, or postgres:<url>
        #[arg(long, value_name = "STORE")]
        store: String,
        /// `hashing` (no key, offline) or `openai` (any /v1/embeddings server)
        #[arg(long)]
        provider: Option<String>,
        /// The model, for the openai provider. For hashing, `hashing-<width>` picks the width
        #[arg(long)]
        model: Option<String>,
        /// Empty the store first. Required to change model or width
        #[arg(long)]
        reset: bool,
    },
    /// Find symbols by meaning rather than by name
    Semantic {
        /// What to look for, in words rather than in identifiers
        query: String,
        /// Where the vectors live: a path, sqlite:<path>, or postgres:<url>
        #[arg(long, value_name = "STORE")]
        store: String,
        /// `hashing` or `openai`, matching how the store was built
        #[arg(long)]
        provider: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long, default_value = "10", help = "How many results (1-100)")]
        limit: usize,
        #[arg(long, help = "JSON output instead of a table")]
        json: bool,
    },
    /// Find symbols by name and meaning together, and report which source found what
    Context {
        /// What to look for, in words rather than in identifiers
        query: String,
        /// Where the vectors live
        #[arg(long, value_name = "STORE")]
        store: String,
        /// `hashing` or `openai`, matching how the store was built
        #[arg(long)]
        provider: Option<String>,
        #[arg(long)]
        model: Option<String>,
        #[arg(long, default_value = "10", help = "How many anchors (1-100)")]
        limit: usize,
        #[arg(long, help = "JSON output instead of a table")]
        json: bool,
    },
    /// What a vector store holds
    Embedinfo {
        /// Where the vectors live
        #[arg(long, value_name = "STORE")]
        store: String,
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
        Commands::Embed {
            path,
            store,
            provider,
            model,
            reset,
        } => handle_embed(&path, &store, provider.as_deref(), model.as_deref(), reset),
        Commands::Semantic {
            query,
            store,
            provider,
            model,
            limit,
            json,
        } => handle_semantic(
            &query,
            &store,
            provider.as_deref(),
            model.as_deref(),
            limit,
            json,
        ),
        Commands::Context {
            query,
            store,
            provider,
            model,
            limit,
            json,
        } => handle_context(
            &query,
            &store,
            provider.as_deref(),
            model.as_deref(),
            limit,
            json,
        ),
        Commands::Embedinfo { store } => handle_embedinfo(&store),
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

/// `embed` — index a project's symbols into a vector store.
fn handle_embed(
    path: &str,
    store_spec: &str,
    provider: Option<&str>,
    model: Option<&str>,
    reset: bool,
) -> anyhow::Result<()> {
    use tiny_pdg_cs::embed::{index_project, open_store, ProviderSpec};

    let spec = ProviderSpec::resolve(provider, model).map_err(anyhow::Error::msg)?;
    let provider = spec.build().map_err(anyhow::Error::msg)?;
    let mut store = open_store(store_spec).map_err(anyhow::Error::msg)?;

    let report = index_project(
        store.as_mut(),
        provider.as_ref(),
        std::path::Path::new(path),
        reset,
    )
    .map_err(anyhow::Error::msg)?;

    println!(
        "Embedded {} symbol(s) from {} file(s) into {}",
        report.embedded, report.files, report.store
    );
    println!("  model: {} ({} dim)", report.model, report.dim);
    println!("  took:  {} ms", report.elapsed_ms);
    if !report.failed.is_empty() {
        // Counted and reported, not fatal: one file mid-edit should not cost the other
        // two hundred.
        println!("  skipped {} file(s):", report.failed.len());
        for (file, reason) in report.failed.iter().take(10) {
            println!("    {file}: {reason}");
        }
        if report.failed.len() > 10 {
            println!("    ... and {} more", report.failed.len() - 10);
        }
    }
    Ok(())
}

/// `semantic` — nearest symbols by meaning.
fn handle_semantic(
    query: &str,
    store_spec: &str,
    provider: Option<&str>,
    model: Option<&str>,
    limit: usize,
    json: bool,
) -> anyhow::Result<()> {
    use tiny_pdg_cs::embed::{open_store, search, ProviderSpec};

    let spec = ProviderSpec::resolve(provider, model).map_err(anyhow::Error::msg)?;
    let provider = spec.build().map_err(anyhow::Error::msg)?;
    let store = open_store(store_spec).map_err(anyhow::Error::msg)?;

    let hits =
        search(store.as_ref(), provider.as_ref(), query, limit).map_err(anyhow::Error::msg)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&hits)?);
        return Ok(());
    }

    println!("{} result(s) for {query:?}:", hits.len());
    for (i, hit) in hits.iter().enumerate() {
        println!("  {}. {}  [{:.3}]", i + 1, hit.entry.symbol_id, hit.score);
        println!("     {}:{}", hit.entry.file, hit.entry.line);
    }
    Ok(())
}

/// `context` — name-based and meaning-based search together, with the sources named.
fn handle_context(
    query: &str,
    store_spec: &str,
    provider: Option<&str>,
    model: Option<&str>,
    limit: usize,
    json: bool,
) -> anyhow::Result<()> {
    use tiny_pdg_cs::embed::{find_context, open_store, ProviderSpec};

    let spec = ProviderSpec::resolve(provider, model).map_err(anyhow::Error::msg)?;
    let provider = spec.build().map_err(anyhow::Error::msg)?;
    let store = open_store(store_spec).map_err(anyhow::Error::msg)?;

    let ctx = find_context(store.as_ref(), provider.as_ref(), query, limit)
        .map_err(anyhow::Error::msg)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&ctx)?);
        return Ok(());
    }

    println!(
        "{} anchor(s) for {query:?}  [{}: {}, {} entries searched]",
        ctx.anchors.len(),
        ctx.model,
        ctx.store.rsplit('/').next().unwrap_or(&ctx.store),
        ctx.searched
    );
    for (i, anchor) in ctx.anchors.iter().enumerate() {
        let sources = anchor.sources.join("+");
        println!("  {}. {}  [{}]", i + 1, anchor.symbol_id, sources);
        println!(
            "     {}:{}{}",
            anchor.file,
            anchor.line,
            anchor
                .similarity
                .map(|s| format!("  {s:.3}"))
                .unwrap_or_default()
        );
    }
    println!(
        "  confidence: {} -- {}",
        ctx.coverage.confidence, ctx.coverage.note
    );
    Ok(())
}

/// `embedinfo` — what a store holds.
fn handle_embedinfo(store_spec: &str) -> anyhow::Result<()> {
    use tiny_pdg_cs::embed::open_store;

    let store = open_store(store_spec).map_err(anyhow::Error::msg)?;
    let info = store.info().map_err(anyhow::Error::msg)?;
    if info.entries == 0 {
        println!("{} store at {} is empty", info.backend, info.location);
        println!("  index it with: tiny-pdg-cs embed <path> --store {store_spec}");
        return Ok(());
    }
    println!(
        "{} store at {} holds {} vector(s)",
        info.backend, info.location, info.entries
    );
    println!("  model: {}", info.model);
    println!("  width: {}", info.dim);
    Ok(())
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
