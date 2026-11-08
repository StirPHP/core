//! `stirphp fmt`: format PHP files.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use clap::Args;
use stir_fmt::{Indent, Options};
use stir_neon::phpstan::FormatIndent;
use stir_project::Project;

#[derive(Debug, Args)]
pub struct FmtArgs {
    /// Files and folders to format, `-` for standard input; by default the project's sources.
    pub paths: Vec<PathBuf>,
    /// Change nothing: list the files that would change, and fail if there are any.
    #[arg(long)]
    pub check: bool,
    /// `tab`, or how many spaces to a level, over what the project's configuration says.
    #[arg(long, value_name = "INDENT")]
    pub indent: Option<String>,
}

/// `4` or `tab`.
fn parse_indent(text: &str) -> anyhow::Result<Indent> {
    if text == "tab" {
        return Ok(Indent::Tabs);
    }
    match text.parse::<u32>() {
        Ok(n @ 1..=16) => Ok(Indent::Spaces(n)),
        _ => anyhow::bail!("`--indent` takes `tab` or a number of spaces from 1 to 16"),
    }
}

/// What came of one file.
enum Outcome {
    Same,
    Changed,
    Failed(String),
}

fn format_file(path: &Path, options: &Options, check: bool) -> Outcome {
    if !stir_fmt::formats_path(path) {
        return Outcome::Same;
    }
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => return Outcome::Failed(error.to_string()),
    };
    let Ok(source) = String::from_utf8(bytes) else {
        return Outcome::Failed("it isn't UTF-8".to_owned());
    };
    let formatted = match stir_fmt::format(&source, options) {
        Ok(formatted) => formatted,
        Err(error) => return Outcome::Failed(error.to_string()),
    };
    if formatted == source {
        return Outcome::Same;
    }
    if !check && let Err(error) = std::fs::write(path, formatted) {
        return Outcome::Failed(error.to_string());
    }
    Outcome::Changed
}

pub async fn run(args: FmtArgs) -> anyhow::Result<ExitCode> {
    let cwd = std::env::current_dir()?;
    let project = Project::load_or_default(&cwd).await?;
    let indent = match &args.indent {
        Some(text) => parse_indent(text)?,
        None => match project.config.indent {
            Some(FormatIndent::Tab) => Indent::Tabs,
            Some(FormatIndent::Spaces(n)) => Indent::Spaces(u32::from(n)),
            None => Indent::default(),
        },
    };
    let options = Options { indent };

    if args.paths.iter().any(|p| p == Path::new("-")) {
        let mut source = String::new();
        std::io::stdin().read_to_string(&mut source)?;
        let formatted = stir_fmt::format(&source, &options)?;
        if args.check {
            return Ok(if formatted == source {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            });
        }
        std::io::stdout().write_all(formatted.as_bytes())?;
        return Ok(ExitCode::SUCCESS);
    }

    let mut files: Vec<PathBuf> = Vec::new();
    if args.paths.is_empty() {
        files.extend(
            stir_project::discover(&project)
                .await?
                .into_iter()
                .map(|f| f.path),
        );
    }
    for path in &args.paths {
        if path.is_dir() {
            files.extend(stir_project::php_files_under(path));
        } else if path.exists() {
            files.push(path.clone());
        } else {
            anyhow::bail!("`{}` doesn't exist", path.display());
        }
    }

    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<(usize, Outcome)>> = Mutex::new(Vec::new());
    let check = args.check;
    std::thread::scope(|scope| {
        let workers = std::thread::available_parallelism().map_or(4, |n| n.get());
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(path) = files.get(i) else { break };
                    let outcome = format_file(path, &options, check);
                    results.lock().unwrap().push((i, outcome));
                }
            });
        }
    });
    let mut results = results.into_inner().unwrap();
    results.sort_by_key(|(i, _)| *i);

    let (mut changed, mut failed) = (0usize, 0usize);
    for (i, outcome) in &results {
        let shown = files[*i].strip_prefix(&cwd).unwrap_or(&files[*i]).display();
        match outcome {
            Outcome::Same => {}
            Outcome::Changed => {
                changed += 1;
                println!("{shown}");
            }
            Outcome::Failed(why) => {
                failed += 1;
                eprintln!("{shown}: {why}");
            }
        }
    }
    let verb = if check { "would change" } else { "changed" };
    eprintln!(
        "{changed} of {} files {verb}{}",
        files.len(),
        if failed > 0 {
            format!(", {failed} couldn't be formatted")
        } else {
            String::new()
        }
    );
    Ok(if failed > 0 || (check && changed > 0) {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}
