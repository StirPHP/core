mod check;
mod fmt;

use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use stir_project::Project;
use stir_project::init::Outcome;
use tracing_subscriber::EnvFilter;

/// The version shown by `--version`: release builds set `STIRPHP_BUILD_VERSION`
/// (e.g. `0.1.0-nightly.20261101+3f02c05` for nightlies).
const VERSION: &str = match option_env!("STIRPHP_BUILD_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

#[cfg(target_env = "musl")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// StirPHP: a type checker and language server for PHP.
#[derive(Debug, Parser)]
#[command(name = "stirphp", version = VERSION, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Type-check the project.
    Check(check::CheckArgs),
    /// Format PHP files, in the style of PER-CS.
    Fmt(fmt::FmtArgs),
    /// Write a configuration, or add to the PHPStan one the project has.
    Init(InitArgs),
    /// Run the language server (over stdin/stdout).
    Lsp,
}

#[derive(Debug, Args)]
struct InitArgs {
    /// Keep the settings in PHPStan's configuration, creating that if there's none.
    #[arg(long)]
    phpstan: bool,
}

/// `stirphp init`.
async fn init(args: InitArgs) -> anyhow::Result<ExitCode> {
    let project = Project::load_or_default(std::env::current_dir()?).await?;
    match stir_project::init::init(&project.root, args.phpstan)? {
        Outcome::Created(path) => println!("Created `{}`.", path.display()),
        Outcome::Updated(path) => println!("Added `parameters.stirphp` to `{}`.", path.display()),
        Outcome::AlreadySetUp(path) => println!("`{}` is set up already.", path.display()),
    }
    Ok(ExitCode::SUCCESS)
}

#[tokio::main]
async fn main() -> anyhow::Result<ExitCode> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::builder()
                .with_default_directive(tracing_subscriber::filter::LevelFilter::WARN.into())
                .with_env_var("STIRPHP_LOG")
                .from_env_lossy(),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    match cli.command {
        Command::Check(args) => check::run(args).await,
        Command::Fmt(args) => fmt::run(args).await,
        Command::Init(args) => init(args).await,
        Command::Lsp => {
            stir_lsp::run_stdio().await;
            Ok(ExitCode::SUCCESS)
        }
    }
}
