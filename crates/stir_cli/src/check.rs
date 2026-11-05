//! `sphp check`: type-check the project and render diagnostics.

use std::collections::HashMap;
use std::io::{IsTerminal as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

use clap::{Args, ValueEnum};
use futures::{StreamExt as _, TryStreamExt as _};
use stir_db::{Diagnostic, Severity, SourceFile};
use stir_project::Project;
use stir_syntax::TextRange;

use stir_project::workspace::{LoadOptions, Loaded, load_workspace};

#[derive(Debug, Args)]
pub struct CheckArgs {
    /// Only report diagnostics for files under these paths.
    pub paths: Vec<PathBuf>,
    /// Output format.
    #[arg(long, value_enum, default_value_t = Format::Human)]
    pub format: Format,
    /// Do not load vendor files from Composer autoload rules.
    #[arg(long)]
    pub no_vendor: bool,
    /// The rule level, 0 to 10, over the one in the PHPStan config.
    #[arg(short, long, value_parser = clap::value_parser!(u8).range(0..=10))]
    pub level: Option<u8>,
    /// Show at most N diagnostics (all are still counted).
    #[arg(long, value_name = "N")]
    pub max_errors: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    Human,
    Json,
}

pub async fn run(args: CheckArgs) -> anyhow::Result<ExitCode> {
    let cwd = std::env::current_dir()?;
    let project = Project::load_or_default(&cwd).await?;

    let filters = canonical_filters(&cwd, &args.paths).await?;
    let loaded = load_workspace(
        &project,
        LoadOptions {
            vendor: !args.no_vendor,
            level: args.level,
        },
    )
    .await?;

    tracing::debug!(
        sources = loaded.sources.len(),
        stubs = loaded.stub_count,
        vendor = loaded.vendor_count,
        "loaded files"
    );
    let (diagnostics, checked) = collect_diagnostics(&loaded, &filters).await?;
    print_notices(&loaded, &filters);
    let texts = source_texts(&loaded);
    let shown = &diagnostics[..args.max_errors.unwrap_or(usize::MAX).min(diagnostics.len())];

    let summary = Summary::new(&diagnostics, checked);
    let mut stdout = std::io::stdout().lock();
    match args.format {
        Format::Human => {
            print_human(&mut stdout, &diagnostics, &texts, &cwd, args.max_errors)?;
            writeln!(stdout, "{summary}")?;
        }
        Format::Json => {
            let json = render_json(shown, &texts, &cwd);
            serde_json::to_writer_pretty(&mut stdout, &json)?;
            writeln!(stdout)?;
            eprintln!("{summary}");
        }
    }
    stdout.flush()?;

    Ok(if summary.errors > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

/// What the user should know about the project, on stderr so that it stays out of the JSON.
fn print_notices(loaded: &Loaded, filters: &[PathBuf]) {
    let held_back: usize = loaded
        .sources
        .iter()
        .filter(|f| filters.is_empty() || filters.iter().any(|p| f.path(&loaded.db).starts_with(p)))
        .map(|&f| stir_checker::held_back_count(&loaded.db, loaded.workspace, f))
        .sum();
    let install = loaded
        .composer
        .as_ref()
        .and_then(stir_project::Composer::install_notice);
    for notice in &loaded.notices {
        if held_back > 0 && install.as_ref() == Some(notice) {
            eprintln!("notice: {notice} Held back so far: {held_back}.");
        } else {
            eprintln!("notice: {notice}");
        }
    }
}

/// Type-checks the project's sources, except `.php` ones with a `// @sphp-nocheck`
/// comment, limited to files under `filters` when non-empty. Returns the sorted diagnostics and the
/// number of files checked.
async fn collect_diagnostics(
    loaded: &Loaded,
    filters: &[PathBuf],
) -> anyhow::Result<(Vec<Diagnostic>, usize)> {
    let targets: Vec<SourceFile> = loaded
        .sources
        .iter()
        .copied()
        .filter(|f| !stir_checker::levels::is_nocheck(f.text(&loaded.db)))
        .filter(|f| filters.is_empty() || filters.iter().any(|p| f.path(&loaded.db).starts_with(p)))
        .collect();

    let start = Instant::now();
    let mut diagnostics = check_files(loaded, &targets).await?;
    tracing::debug!(files = targets.len(), elapsed = ?start.elapsed(), "checked");
    diagnostics.sort_by(|a, b| {
        (&a.file, a.range.start(), a.range.end(), a.severity, a.code).cmp(&(
            &b.file,
            b.range.start(),
            b.range.end(),
            b.severity,
            b.code,
        ))
    });
    Ok((diagnostics, targets.len()))
}

/// Path → text of every project source, for rendering diagnostics.
fn source_texts(loaded: &Loaded) -> HashMap<&Path, &str> {
    loaded
        .sources
        .iter()
        .map(|f| (f.path(&loaded.db).as_path(), f.text(&loaded.db).as_str()))
        .collect()
}

/// Renders diagnostics for humans (coloured when stdout is a terminal), showing at
/// most `max_errors` of them.
fn print_human(
    out: &mut impl std::io::Write,
    diagnostics: &[Diagnostic],
    texts: &HashMap<&Path, &str>,
    cwd: &Path,
    max_errors: Option<usize>,
) -> std::io::Result<()> {
    let shown = &diagnostics[..max_errors.unwrap_or(usize::MAX).min(diagnostics.len())];
    let color = use_color(std::io::stdout().is_terminal());
    out.write_all(&render_human(shown, texts, cwd, color))?;
    if shown.len() < diagnostics.len() {
        writeln!(
            out,
            "... {} more not shown (--max-errors {})",
            diagnostics.len() - shown.len(),
            shown.len()
        )?;
    }
    Ok(())
}

async fn canonical_filters(cwd: &Path, paths: &[PathBuf]) -> anyhow::Result<Vec<PathBuf>> {
    let mut out = Vec::with_capacity(paths.len());
    for path in paths {
        let full = cwd.join(path);
        match tokio::fs::canonicalize(&full).await {
            Ok(p) => out.push(p),
            Err(e) => anyhow::bail!("cannot check `{}`: {e}", path.display()),
        }
    }
    Ok(out)
}

/// Runs [`stir_checker::check_file`] for every target in parallel, each on its own
/// database clone inside `spawn_blocking`.
async fn check_files(loaded: &Loaded, targets: &[SourceFile]) -> anyhow::Result<Vec<Diagnostic>> {
    let parallelism = std::thread::available_parallelism().map_or(4, |n| n.get());
    let ws = loaded.workspace;
    let results: Vec<Arc<Vec<Diagnostic>>> = futures::stream::iter(targets.iter().map(|&file| {
        let db = loaded.db.clone();
        tokio::task::spawn_blocking(move || stir_checker::check_file(&db, ws, file).clone())
    }))
    .buffer_unordered(parallelism)
    .try_collect()
    .await?;
    Ok(results.iter().flat_map(|d| d.iter().cloned()).collect())
}

fn use_color(is_tty: bool) -> bool {
    is_tty && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
}

struct Summary {
    errors: usize,
    warnings: usize,
    files_with_problems: usize,
    checked: usize,
}

impl Summary {
    fn new(diagnostics: &[Diagnostic], checked: usize) -> Self {
        let count = |s| diagnostics.iter().filter(|d| d.severity == s).count();
        let mut files: Vec<&Path> = diagnostics.iter().map(|d| d.file.as_path()).collect();
        files.dedup();
        Self {
            errors: count(Severity::Error),
            warnings: count(Severity::Warning),
            files_with_problems: files.len(),
            checked,
        }
    }
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("{n} {word}")
    } else {
        format!("{n} {word}s")
    }
}

impl std::fmt::Display for Summary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.errors == 0 && self.warnings == 0 {
            write!(f, "No problems found in {}", plural(self.checked, "file"))
        } else {
            write!(
                f,
                "{}, {} in {} (checked {})",
                plural(self.errors, "error"),
                plural(self.warnings, "warning"),
                plural(self.files_with_problems, "file"),
                plural(self.checked, "file"),
            )
        }
    }
}

/// 1-based line and column lookup; columns count UTF-8 characters.
struct LineMap<'a> {
    text: &'a str,
    starts: Vec<usize>,
}

impl<'a> LineMap<'a> {
    fn new(text: &'a str) -> Self {
        let starts = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        Self { text, starts }
    }

    fn line_col(&self, offset: usize) -> (usize, usize) {
        let mut offset = offset.min(self.text.len());
        while !self.text.is_char_boundary(offset) {
            offset -= 1;
        }
        let line = self.starts.partition_point(|&s| s <= offset) - 1;
        let col = self.text[self.starts[line]..offset].chars().count();
        (line + 1, col + 1)
    }
}

fn display_path(path: &Path, cwd: &Path) -> String {
    path.strip_prefix(cwd).unwrap_or(path).display().to_string()
}

/// Byte range for ariadne; empty ranges are widened to cover one character so the
/// label is visible.
fn span(range: TextRange, text: &str) -> std::ops::Range<usize> {
    let start = usize::from(range.start()).min(text.len());
    let end = usize::from(range.end()).clamp(start, text.len());
    if start < end {
        return start..end;
    }
    match text[start..].chars().next() {
        Some(c) if c != '\n' && c != '\r' => start..start + c.len_utf8(),
        _ => start..start,
    }
}

fn render_human(
    diagnostics: &[Diagnostic],
    texts: &HashMap<&Path, &str>,
    cwd: &Path,
    color: bool,
) -> Vec<u8> {
    use ariadne::{Color, Config, IndexType, Label, Report, ReportKind};

    let mut out = Vec::new();
    let mut cache = ariadne::sources(
        diagnostics
            .iter()
            .map(|d| d.file.as_path())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .map(|p| (display_path(p, cwd), texts.get(p).copied().unwrap_or(""))),
    );
    let config = Config::default()
        .with_color(color)
        .with_index_type(IndexType::Byte);

    for d in diagnostics {
        let id = display_path(&d.file, cwd);
        let text = texts.get(d.file.as_path()).copied().unwrap_or("");
        let (name, kind_color, label_color) = match d.severity {
            Severity::Error => ("error", Color::Red, Color::Red),
            Severity::Warning => ("warning", Color::Yellow, Color::Yellow),
            Severity::Hint => ("hint", Color::Fixed(147), Color::Fixed(147)),
        };
        let kind_name = format!("{name}[{}]", d.code);
        let primary = span(d.range, text);
        let mut report = Report::build(
            ReportKind::Custom(&kind_name, kind_color),
            (id.clone(), primary.clone()),
        )
        .with_config(config)
        .with_message(&d.message)
        .with_label(
            Label::new((id.clone(), primary))
                .with_message(&d.message)
                .with_color(label_color)
                .with_priority(1),
        );
        for (range, message) in &d.labels {
            report.add_label(
                Label::new((id.clone(), span(*range, text)))
                    .with_message(message)
                    .with_color(Color::Blue),
            );
        }
        for note in &d.notes {
            report.add_note(note);
        }
        if let Err(error) = report.finish().write(&mut cache, &mut out) {
            tracing::warn!(%error, "failed to render diagnostic");
            let _ = writeln!(out, "{id}: {kind_name}: {}", d.message);
        }
    }
    if color {
        out
    } else {
        // ariadne paints custom report kinds even when colours are disabled.
        strip_ansi(&out)
    }
}

/// Removes ANSI SGR sequences (`ESC [ ... m`).
fn strip_ansi(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1b && bytes.get(i + 1) == Some(&b'[') {
            let mut j = i + 2;
            while j < bytes.len() && (bytes[j].is_ascii_digit() || bytes[j] == b';') {
                j += 1;
            }
            if bytes.get(j) == Some(&b'm') {
                i = j + 1;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

fn render_json(
    diagnostics: &[Diagnostic],
    texts: &HashMap<&Path, &str>,
    cwd: &Path,
) -> serde_json::Value {
    let mut maps: HashMap<&Path, LineMap<'_>> = HashMap::new();
    let items = diagnostics
        .iter()
        .map(|d| {
            let map = maps.entry(d.file.as_path()).or_insert_with(|| {
                LineMap::new(texts.get(d.file.as_path()).copied().unwrap_or(""))
            });
            let position = |range: TextRange| {
                let (line, column) = map.line_col(range.start().into());
                let (end_line, end_column) = map.line_col(range.end().into());
                (line, column, end_line, end_column)
            };
            let (line, column, end_line, end_column) = position(d.range);
            let labels: Vec<_> = d
                .labels
                .iter()
                .map(|(range, message)| {
                    let (line, column, end_line, end_column) = position(*range);
                    serde_json::json!({
                        "line": line,
                        "column": column,
                        "endLine": end_line,
                        "endColumn": end_column,
                        "message": message,
                    })
                })
                .collect();
            serde_json::json!({
                "file": display_path(&d.file, cwd),
                "line": line,
                "column": column,
                "endLine": end_line,
                "endColumn": end_column,
                "severity": match d.severity {
                    Severity::Error => "error",
                    Severity::Warning => "warning",
                    Severity::Hint => "hint",
                },
                "code": d.code,
                "message": d.message,
                "labels": labels,
                "notes": d.notes,
            })
        })
        .collect();
    serde_json::Value::Array(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_col_counts_chars() {
        let map = LineMap::new("ab\nçd\n");
        assert_eq!(map.line_col(0), (1, 1));
        assert_eq!(map.line_col(2), (1, 3));
        assert_eq!(map.line_col(3), (2, 1));
        // `ç` is two bytes but one column.
        assert_eq!(map.line_col(5), (2, 2));
        assert_eq!(map.line_col(4), (2, 1));
        assert_eq!(map.line_col(100), (3, 1));
    }

    #[test]
    fn strips_ansi() {
        assert_eq!(strip_ansi(b"\x1b[31merror\x1b[0m: x\x1b[1;2m"), b"error: x");
        assert_eq!(strip_ansi(b"\x1b[x"), b"\x1b[x");
    }

    #[test]
    fn summary_text() {
        assert_eq!(
            Summary {
                errors: 0,
                warnings: 0,
                files_with_problems: 0,
                checked: 1
            }
            .to_string(),
            "No problems found in 1 file"
        );
        assert_eq!(
            Summary {
                errors: 2,
                warnings: 1,
                files_with_problems: 1,
                checked: 3
            }
            .to_string(),
            "2 errors, 1 warning in 1 file (checked 3 files)"
        );
    }
}
