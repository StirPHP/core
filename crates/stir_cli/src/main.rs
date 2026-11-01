mod check;
mod workspace;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use stir_project::{InitOptions, InitReport};
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
    Init(InitArgs),
    /// Type-check the project.
    Check(check::CheckArgs),
    /// Transpile the project to plain PHP.
    Build,
}

#[derive(Debug, Args)]
struct InitArgs {
    /// Accept defaults (and any flags given) without prompting.
    #[arg(short, long)]
    yes: bool,
    /// Overwrite an existing sphp.toml.
    #[arg(short, long)]
    force: bool,
    /// Project name (defaults to the directory name).
    #[arg(long)]
    name: Option<String>,
    /// Source directory.
    #[arg(long)]
    src_dir: Option<PathBuf>,
    /// Output directory for transpiled PHP.
    #[arg(long)]
    out_dir: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<ExitCode> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_env("SPHP_LOG"))
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    match cli.command {
        Command::Init(args) => run_init(args).await.map(|()| ExitCode::SUCCESS),
        Command::Check(args) => check::run(args).await,
        Command::Build => anyhow::bail!("`build` is not implemented yet"),
    }
}

async fn run_init(args: InitArgs) -> anyhow::Result<()> {
    let dir = std::env::current_dir()?;
    let defaults = InitOptions::default();
    let mut options = InitOptions {
        name: args.name.or_else(|| stir_project::default_name(&dir)),
        src_dir: args.src_dir.unwrap_or(defaults.src_dir),
        out_dir: args.out_dir.unwrap_or(defaults.out_dir),
        allow_php: defaults.allow_php,
        force: args.force,
    };
    if !args.yes {
        options = tokio::task::spawn_blocking(move || prompt(options)).await??;
    }
    let report = stir_project::init(&dir, options).await?;
    print_summary(&dir, &report);
    Ok(())
}

fn prompt(mut options: InitOptions) -> anyhow::Result<InitOptions> {
    use inquire::{Confirm, Text};

    let name = options.name.clone().unwrap_or_default();
    let name = Text::new("Project name:").with_default(&name).prompt()?;
    options.name = (!name.trim().is_empty()).then(|| name.trim().to_owned());

    let src = options.src_dir.to_string_lossy().into_owned();
    options.src_dir = Text::new("Source directory:")
        .with_default(&src)
        .prompt()?
        .into();

    let out = options.out_dir.to_string_lossy().into_owned();
    options.out_dir = Text::new("Output directory:")
        .with_default(&out)
        .prompt()?
        .into();

    options.allow_php = Confirm::new("Allow plain .php files in the project?")
        .with_default(options.allow_php)
        .prompt()?;
    Ok(options)
}

fn print_summary(dir: &Path, report: &InitReport) {
    let show = |p: &Path| p.strip_prefix(dir).unwrap_or(p).display().to_string();
    println!("Initialized StirPHP project in {}", dir.display());
    for path in &report.created {
        println!("  created  {}", show(path));
    }
    if report.gitignore_updated && !report.created.iter().any(|p| p.ends_with(".gitignore")) {
        println!("  updated  .gitignore");
    }
    for path in &report.skipped {
        println!("  kept     {} (already exists)", show(path));
    }
    if let Some(composer) = &report.composer {
        println!("  detected {}", composer.display());
    }
    if let Some(phpstan) = &report.phpstan {
        println!("  detected {}", phpstan.display());
    }
}
