mod check;

use std::process::ExitCode;

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

/// The version shown by `--version`: release builds set `SPHP_BUILD_VERSION`
/// (e.g. `0.1.0-nightly.20261101+3f02c05` for nightlies).
const VERSION: &str = match option_env!("SPHP_BUILD_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

#[cfg(target_env = "musl")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// StirPHP: a type checker and language server for PHP.
#[derive(Debug, Parser)]
#[command(name = "sphp", version = VERSION, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Type-check the project.
    Check(check::CheckArgs),
    /// Run the language server (over stdin/stdout).
    Lsp,
}

#[tokio::main]
async fn main() -> anyhow::Result<ExitCode> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(tracing_subscriber::filter::LevelFilter::WARN.into())
                .with_env_var("SPHP_LOG")
                .from_env_lossy(),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    match cli.command {
        Command::Check(args) => check::run(args).await,
        Command::Lsp => {
            stir_lsp::run_stdio().await;
            Ok(ExitCode::SUCCESS)
        }
    }
}
