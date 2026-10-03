use campus_mcp::{backend::LiveBackend, server::CampusServer};
use clap::{Parser, Subcommand, ValueEnum};
use rmcp::ServiceExt;
use std::{path::PathBuf, sync::Arc};

#[derive(Parser)]
#[command(version, about = "Read-only PLMS and Outlook MCP server")]
struct Cli {
    /// Explicit local env file. Existing environment variables take precedence.
    #[arg(long, global = true)]
    env_file: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}
#[derive(Clone, Copy, ValueEnum, PartialEq)]
enum Module {
    Lms,
    Mail,
}
#[derive(Subcommand)]
enum Command {
    /// Serve MCP over stdin/stdout; stdout is reserved for JSON-RPC.
    Serve {
        #[arg(long, value_enum, value_delimiter = ',', default_value = "lms,mail")]
        modules: Vec<Module>,
        #[arg(long)]
        cache_dir: Option<PathBuf>,
    },
    /// Explicit interactive Outlook sign-in. Run in a terminal, not as an MCP tool.
    MailLogin,
}
#[tokio::main]
async fn main() {
    if let Err(message) = run().await {
        eprintln!("{message}");
        std::process::exit(1);
    }
}
async fn run() -> Result<(), String> {
    let cli = Cli::parse();
    if let Some(path) = cli.env_file {
        dotenvy::from_path(path)
            .map_err(|_| "Cannot load the explicitly configured env file".to_string())?;
    }
    match cli.command {
        Command::MailLogin => campus_mcp::backend::mail::login().await,
        Command::Serve { modules, cache_dir } => {
            let cache_dir = match cache_dir {
                Some(path) => path,
                None => std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .ok_or("HOME or --cache-dir is required")?
                    .join(".cache/campus-mcp"),
            };
            let server = CampusServer::new(
                Arc::new(LiveBackend::new(cache_dir)),
                modules.contains(&Module::Lms),
                modules.contains(&Module::Mail),
            );
            let service = server
                .serve(rmcp::transport::stdio())
                .await
                .map_err(|_| "MCP initialization failed".to_string())?;
            service
                .waiting()
                .await
                .map_err(|_| "MCP transport failed".to_string())?;
            Ok(())
        }
    }
}
