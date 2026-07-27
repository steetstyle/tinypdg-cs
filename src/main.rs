use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "tiny-pdg-cs", about = "C# Program Dependence Graph (PDG) builder")]
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
        #[arg(long, help = "Trace dispatch: show possible implementations and their call graphs")]
        trace: bool,
    },
    /// Detect design patterns in C# source code
    Detect {
        #[arg(help = "Path to .cs file or project directory")]
        path: String,
    },
    /// Start HTTP server for PRAXIS integration
    Serve {
        #[arg(long, default_value = "8080")]
        port: u16,
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
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    let result = match cli.command {
        Commands::Parse { file } => {
            tiny_pdg_cs::cli::commands::handle_parse(&file)
        }
        Commands::Cfg { file, format } => {
            tiny_pdg_cs::cli::commands::handle_cfg(&file, format.as_deref())
        }
        Commands::Pdg { file, format } => {
            tiny_pdg_cs::cli::commands::handle_pdg(&file, format.as_deref())
        }
        Commands::Hammock { file, level: _ } => {
            tiny_pdg_cs::cli::commands::handle_hammock(&file, None)
        }
        Commands::Resolve { path, kind } => {
            tiny_pdg_cs::cli::commands::handle_resolve(&path, kind.as_deref())
        }
        Commands::Detect { path } => {
            tiny_pdg_cs::cli::commands::handle_detect(&path)
        }
        Commands::Callgraph { path, class, depth, outbound, inbound, trace } => {
            tiny_pdg_cs::cli::commands::handle_callgraph(&path, class.as_deref(), depth, outbound, inbound, trace)
        }
        Commands::Serve { port } => {
            println!("serve on {}", port);
            Ok(())
        }
        Commands::Route { path, json } => {
            tiny_pdg_cs::cli::commands::handle_route(&path, json)
        }
        Commands::Traverse { path, class, context } => {
            tiny_pdg_cs::cli::commands::handle_traverse(&path, &class, context.as_deref())
        }
    };

    if let Err(e) = result {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}
