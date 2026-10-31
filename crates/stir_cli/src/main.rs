use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

/// StirPHP: a typed superset of PHP.
#[derive(Debug, Parser)]
#[command(name = "sphp", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Create a new StirPHP project in the current directory.
    Init,
    /// Type-check the project.
    Check,
    /// Transpile the project to plain PHP.
    Build,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_env("SPHP_LOG"))
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    match cli.command {
        Command::Init | Command::Check | Command::Build => {
            anyhow::bail!("`{:?}` is not implemented yet", cli.command)
        }
    }
}
