//! `sphp build`: type-check the project, then transpile it into `outDir`.
//!
//! The output directory mirrors `srcDir` (`src/App/User.sphp` → `build/App/User.php`);
//! plain `.php` sources are copied unchanged. Every file the build writes is recorded
//! in `outDir/.sphp-build.json`, and only files listed there by a previous build are
//! ever deleted as stale. Files whose content did not change are not rewritten, so
//! their mtimes stay stable (opcache, rsync, make-style tooling).

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write as _;
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use anyhow::Context as _;
use clap::Args;
use futures::{StreamExt as _, TryStreamExt as _};
use stir_db::{SourceFile, Workspace};
use stir_emit::{EmitOptions, Emitted};
use stir_project::Project;
use stir_syntax::Dialect;

use crate::check::{
    Summary, collect_diagnostics, display_path, load_project, print_human, source_texts,
};
use crate::workspace::{LoadOptions, Loaded, load_workspace};

/// Name of the build manifest inside `outDir`.
pub const MANIFEST_FILE: &str = ".sphp-build.json";
/// Name of the runtime (stdlib) file inside `outDir`.
pub const RUNTIME_FILE: &str = "stir-runtime.php";

/// Maximum number of files written concurrently.
const WRITE_CONCURRENCY: usize = 64;

#[derive(Debug, Clone, Args)]
pub struct BuildArgs {
    /// Output directory (overrides `outDir` from sphp.toml; relative to the current directory).
    #[arg(long, value_name = "DIR")]
    pub out_dir: Option<PathBuf>,
    /// Inline extension function calls (also enabled by `inlineExtensions` in sphp.toml).
    #[arg(long)]
    pub inline_extensions: bool,
    /// Do not write `.php.map` source maps.
    #[arg(long)]
    pub no_source_maps: bool,
    /// Rebuild whenever sources or sphp.toml change.
    #[arg(long, short)]
    pub watch: bool,
    /// Write output even if type-checking reports errors.
    #[arg(long, short)]
    pub force: bool,
}

/// Settings of one build, resolved from the CLI flags and the project config.
#[derive(Debug, Clone)]
pub struct BuildOptions {
    pub inline_extensions: bool,
    pub source_maps: bool,
    pub force: bool,
}

/// Result of [`build`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Output written; no errors.
    Built,
    /// Output written although there were errors (`--force`).
    BuiltWithErrors,
    /// Errors were reported and nothing was written.
    Blocked,
}

pub async fn run(args: BuildArgs) -> anyhow::Result<ExitCode> {
    let cwd = std::env::current_dir()?;
    let mut project = load_project(&cwd).await?;
    apply_overrides(&mut project, &args, &cwd);
    if args.watch {
        return crate::watch::run(project, args, cwd).await;
    }
    let loaded = load_workspace(&project, LoadOptions::default()).await?;
    let outcome = build(&project, &loaded, &options(&project, &args), &cwd).await?;
    Ok(match outcome {
        Outcome::Built => ExitCode::SUCCESS,
        Outcome::BuiltWithErrors | Outcome::Blocked => ExitCode::FAILURE,
    })
}

/// Applies CLI overrides to the loaded project (currently `--out-dir`).
///
/// The override is stored as an absolute path, so [`Project::out_dir`] returns it and
/// discovery skips it like a configured `outDir`.
pub fn apply_overrides(project: &mut Project, args: &BuildArgs, cwd: &Path) {
    if let Some(dir) = &args.out_dir {
        project.config.out_dir = cwd.join(dir);
    }
}

pub fn options(project: &Project, args: &BuildArgs) -> BuildOptions {
    BuildOptions {
        inline_extensions: args.inline_extensions || project.config.inline_extensions,
        source_maps: !args.no_source_maps,
        force: args.force,
    }
}

/// What to produce for one project source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JobKind {
    Transpile,
    Copy,
}

#[derive(Debug, Clone)]
struct Job {
    file: SourceFile,
    kind: JobKind,
    /// Output path relative to `outDir`.
    out_rel: PathBuf,
}

/// One file to write, relative to `outDir`.
struct Output {
    rel: PathBuf,
    bytes: Vec<u8>,
}

/// Type-checks and builds the loaded workspace into `project.out_dir()`.
pub async fn build(
    project: &Project,
    loaded: &Loaded,
    opts: &BuildOptions,
    cwd: &Path,
) -> anyhow::Result<Outcome> {
    let start = Instant::now();
    let (diagnostics, checked) = collect_diagnostics(project, loaded, &[]).await?;
    let summary = Summary::new(&diagnostics, checked);
    {
        let mut stdout = std::io::stdout().lock();
        if !diagnostics.is_empty() {
            print_human(&mut stdout, &diagnostics, &source_texts(loaded), cwd, None)?;
            writeln!(stdout, "{summary}")?;
        }
        if summary.errors > 0 && !opts.force {
            writeln!(
                stdout,
                "Build failed: fix the errors above or pass --force; no files were written"
            )?;
            return Ok(Outcome::Blocked);
        }
    }

    let out_dir = project.out_dir();
    let jobs = plan(project, loaded)?;
    let (mut outputs, needs_runtime) = emit(loaded, &jobs, &out_dir, opts).await?;

    let db = &loaded.db;
    if needs_runtime {
        let emit_opts = emit_options(opts, None);
        let runtime = stir_emit::runtime(db, loaded.workspace, &emit_opts);
        outputs.push(Output {
            rel: PathBuf::from(RUNTIME_FILE),
            bytes: runtime.code.into_bytes(),
        });
    }
    check_unique(&outputs, &jobs, loaded)?;

    let written: BTreeSet<String> = outputs.iter().map(|o| slash_path(&o.rel)).collect();
    let changed = write_outputs(&out_dir, outputs).await?;
    let removed = remove_stale(&out_dir, &written).await?;
    write_manifest(&out_dir, &written).await?;

    let copied = jobs.iter().filter(|j| j.kind == JobKind::Copy).count();
    tracing::debug!(changed, removed, "outputs written");
    let mut out_display = display_path(&out_dir, cwd);
    if !out_display.ends_with('/') {
        out_display.push('/');
    }
    println!(
        "Built {} ({copied} copied) into {out_display} in {}ms",
        plural(jobs.len(), "file"),
        start.elapsed().as_millis()
    );
    Ok(if summary.errors > 0 {
        Outcome::BuiltWithErrors
    } else {
        Outcome::Built
    })
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("{n} {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// Maps each project source to its output path under `outDir`.
fn plan(project: &Project, loaded: &Loaded) -> anyhow::Result<Vec<Job>> {
    let db = &loaded.db;
    let src_dir = project.src_dir();
    let mut jobs = Vec::with_capacity(loaded.sources.len());
    for &file in &loaded.sources {
        let path = file.path(db);
        let Ok(rel) = path.strip_prefix(&src_dir) else {
            tracing::warn!(path = %path.display(), "source outside srcDir; not built");
            continue;
        };
        let job = match file.dialect(db) {
            Dialect::Stir => Job {
                file,
                kind: JobKind::Transpile,
                out_rel: rel.with_extension("php"),
            },
            Dialect::Php => Job {
                file,
                kind: JobKind::Copy,
                out_rel: rel.to_path_buf(),
            },
        };
        jobs.push(job);
    }
    Ok(jobs)
}

/// Fails when two outputs (e.g. `A.sphp` and `A.php`, or a source named like the
/// runtime file) would land on the same path.
fn check_unique(outputs: &[Output], jobs: &[Job], loaded: &Loaded) -> anyhow::Result<()> {
    let mut owners: BTreeMap<&Path, Option<&Path>> = BTreeMap::new();
    let source_of = |rel: &Path| {
        jobs.iter()
            .find(|j| j.out_rel == rel)
            .map(|j| j.file.path(&loaded.db).as_path())
    };
    for out in outputs {
        if let Some(previous) = owners.insert(&out.rel, source_of(&out.rel)) {
            let name = |p: Option<&Path>| {
                p.map_or_else(
                    || "the runtime".to_owned(),
                    |p| format!("`{}`", p.display()),
                )
            };
            anyhow::bail!(
                "{} and another source both compile to `{}`",
                name(previous),
                out.rel.display()
            );
        }
    }
    Ok(())
}

fn emit_options(opts: &BuildOptions, runtime_require: Option<String>) -> EmitOptions {
    EmitOptions {
        inline_extensions: opts.inline_extensions,
        runtime_require,
        ..EmitOptions::default()
    }
}

/// Transpiles (or copies) every job in parallel on database clones. Returns the
/// outputs and whether any file needs the runtime.
async fn emit(
    loaded: &Loaded,
    jobs: &[Job],
    out_dir: &Path,
    opts: &BuildOptions,
) -> anyhow::Result<(Vec<Output>, bool)> {
    let parallelism = std::thread::available_parallelism().map_or(4, |n| n.get());
    let ws = loaded.workspace;
    let results: Vec<(Vec<Output>, bool)> =
        futures::stream::iter(jobs.iter().cloned().map(|job| {
            let db = loaded.db.clone();
            let out_dir = out_dir.to_path_buf();
            let opts = opts.clone();
            async move {
                let path = job.file.path(&db).clone();
                tokio::task::spawn_blocking(move || emit_one(&db, ws, &job, &out_dir, &opts))
                    .await
                    .with_context(|| format!("internal error while building `{}`", path.display()))
            }
        }))
        .buffer_unordered(parallelism)
        .try_collect()
        .await?;
    let needs_runtime = results.iter().any(|(_, n)| *n);
    Ok((
        results.into_iter().flat_map(|(o, _)| o).collect(),
        needs_runtime,
    ))
}

fn emit_one(
    db: &stir_db::RootDatabase,
    ws: Workspace,
    job: &Job,
    out_dir: &Path,
    opts: &BuildOptions,
) -> (Vec<Output>, bool) {
    let text = job.file.text(db);
    if job.kind == JobKind::Copy {
        let out = Output {
            rel: job.out_rel.clone(),
            bytes: text.clone().into_bytes(),
        };
        return (vec![out], false);
    }

    let require = runtime_require(&job.out_rel);
    let emitted = stir_emit::transpile(db, ws, job.file, &emit_options(opts, Some(require)));
    let mut outputs = Vec::with_capacity(2);
    if opts.source_maps {
        let out_path = out_dir.join(&job.out_rel);
        let map = source_map(&emitted, &out_path, job.file.path(db), text);
        let mut map_rel = job.out_rel.clone().into_os_string();
        map_rel.push(".map");
        outputs.push(Output {
            rel: map_rel.into(),
            bytes: map,
        });
    }
    let needs_runtime = emitted.needs_runtime;
    outputs.push(Output {
        rel: job.out_rel.clone(),
        bytes: emitted.code.into_bytes(),
    });
    (outputs, needs_runtime)
}

/// The `require_once` expression for the runtime from an output file at `out_rel`
/// (relative to `outDir`), e.g. `__DIR__ . '/../stir-runtime.php'`.
fn runtime_require(out_rel: &Path) -> String {
    let depth = out_rel.parent().map_or(0, |p| p.components().count());
    let rel = format!("/{}{RUNTIME_FILE}", "../".repeat(depth));
    format!(
        "__DIR__ . '{}'",
        rel.replace('\\', "\\\\").replace('\'', "\\'")
    )
}

/// `path` with `/` separators.
fn slash_path(path: &Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Byte offset → 0-based (line, UTF-16 column), as source maps v3 count columns.
struct LineIndex<'a> {
    text: &'a str,
    starts: Vec<usize>,
}

impl<'a> LineIndex<'a> {
    fn new(text: &'a str) -> Self {
        let starts = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        Self { text, starts }
    }

    fn position(&self, offset: usize) -> (u32, u32) {
        let mut offset = offset.min(self.text.len());
        while !self.text.is_char_boundary(offset) {
            offset -= 1;
        }
        let line = self.starts.partition_point(|&s| s <= offset) - 1;
        let col = self.text[self.starts[line]..offset].encode_utf16().count();
        (
            u32::try_from(line).unwrap_or(u32::MAX),
            u32::try_from(col).unwrap_or(u32::MAX),
        )
    }
}

/// Source Map v3 JSON for one emitted file. Each mapping contributes a token at its
/// output start pointing at its source start; when several mappings start at the
/// same output position the innermost (last in pre-order) wins. Columns are UTF-16
/// code units, as the specification requires.
fn source_map(emitted: &Emitted, out_path: &Path, source_path: &Path, source: &str) -> Vec<u8> {
    let out_dir = out_path.parent().unwrap_or(Path::new(""));
    let source_rel = pathdiff::diff_paths(source_path, out_dir)
        .map_or_else(|| slash_path(source_path), |p| slash_path(&p));
    let file_name = out_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned());

    let mut builder = sourcemap::SourceMapBuilder::new(file_name.as_deref());
    let src_id = builder.add_source(&source_rel);
    builder.set_source_contents(src_id, Some(source));

    let out_index = LineIndex::new(&emitted.code);
    let src_index = LineIndex::new(source);
    let mut tokens: Vec<((u32, u32), (u32, u32))> = emitted
        .mappings
        .iter()
        .map(|m| {
            (
                out_index.position(m.start.offset as usize),
                src_index.position(m.origin.start as usize),
            )
        })
        .collect();
    // Stable sort keeps pre-order among equal starts; keep the last (innermost).
    tokens.sort_by_key(|(dst, _)| *dst);
    let mut deduped: Vec<((u32, u32), (u32, u32))> = Vec::with_capacity(tokens.len());
    for token in tokens {
        match deduped.last_mut() {
            Some(last) if last.0 == token.0 => *last = token,
            _ => deduped.push(token),
        }
    }
    for ((dst_line, dst_col), (src_line, src_col)) in deduped {
        builder.add_raw(
            dst_line,
            dst_col,
            src_line,
            src_col,
            Some(src_id),
            None,
            false,
        );
    }

    let mut out = Vec::new();
    builder
        .into_sourcemap()
        .to_writer(&mut out)
        .expect("writing a source map to memory cannot fail");
    out
}

/// Writes outputs whose content differs from what is on disk. Returns how many
/// files were (re)written.
async fn write_outputs(out_dir: &Path, outputs: Vec<Output>) -> anyhow::Result<usize> {
    let results: Vec<bool> = futures::stream::iter(
        outputs
            .into_iter()
            .map(|o| write_if_changed(out_dir.join(o.rel), o.bytes)),
    )
    .buffer_unordered(WRITE_CONCURRENCY)
    .try_collect()
    .await?;
    Ok(results.into_iter().filter(|&c| c).count())
}

async fn write_if_changed(path: PathBuf, bytes: Vec<u8>) -> anyhow::Result<bool> {
    if let Ok(existing) = tokio::fs::read(&path).await
        && existing == bytes
    {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create `{}`", parent.display()))?;
    }
    tokio::fs::write(&path, bytes)
        .await
        .with_context(|| format!("failed to write `{}`", path.display()))?;
    Ok(true)
}

/// Files recorded in the previous build's manifest (empty if there is none or it is
/// unreadable).
async fn read_manifest(out_dir: &Path) -> Vec<String> {
    let Ok(text) = tokio::fs::read_to_string(out_dir.join(MANIFEST_FILE)).await else {
        return Vec::new();
    };
    match serde_json::from_str::<serde_json::Value>(&text) {
        Ok(json) => json["files"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|f| f.as_str().map(str::to_owned))
            .collect(),
        Err(error) => {
            tracing::warn!(%error, "ignoring malformed build manifest");
            Vec::new()
        }
    }
}

async fn write_manifest(out_dir: &Path, files: &BTreeSet<String>) -> anyhow::Result<()> {
    let json = serde_json::json!({
        "version": 1,
        "generator": concat!("sphp ", env!("CARGO_PKG_VERSION")),
        "files": files,
    });
    let mut text = serde_json::to_string_pretty(&json)?;
    text.push('\n');
    write_if_changed(out_dir.join(MANIFEST_FILE), text.into_bytes()).await?;
    Ok(())
}

/// A manifest entry as a path below `outDir`, or `None` if it could escape it.
fn manifest_path(entry: &str) -> Option<PathBuf> {
    let path = Path::new(entry);
    let safe = !entry.is_empty() && path.components().all(|c| matches!(c, Component::Normal(_)));
    safe.then(|| path.to_path_buf())
}

/// Deletes files listed in the previous manifest that this build did not produce,
/// then any directories left empty by that. Returns the number of files removed.
async fn remove_stale(out_dir: &Path, written: &BTreeSet<String>) -> anyhow::Result<usize> {
    let mut removed = 0;
    for entry in read_manifest(out_dir).await {
        if written.contains(&entry) {
            continue;
        }
        let Some(rel) = manifest_path(&entry) else {
            tracing::warn!(entry, "ignoring unsafe manifest entry");
            continue;
        };
        let path = out_dir.join(&rel);
        match tokio::fs::remove_file(&path).await {
            Ok(()) => removed += 1,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(e).with_context(|| format!("failed to remove `{}`", path.display()));
            }
        }
        // Prune now-empty parent directories (never `outDir` itself).
        let mut dir = rel.parent();
        while let Some(d) = dir.filter(|d| !d.as_os_str().is_empty()) {
            if tokio::fs::remove_dir(out_dir.join(d)).await.is_err() {
                break;
            }
            dir = d.parent();
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_require_paths() {
        assert_eq!(
            runtime_require(Path::new("main.php")),
            "__DIR__ . '/stir-runtime.php'"
        );
        assert_eq!(
            runtime_require(Path::new("App/User.php")),
            "__DIR__ . '/../stir-runtime.php'"
        );
        assert_eq!(
            runtime_require(Path::new("App/Models/User.php")),
            "__DIR__ . '/../../stir-runtime.php'"
        );
    }

    #[test]
    fn line_index_utf16() {
        let index = LineIndex::new("a\n\u{e7}\u{1F600}x\n");
        assert_eq!(index.position(0), (0, 0));
        assert_eq!(index.position(2), (1, 0));
        // `ç` is 2 bytes / 1 UTF-16 unit, the emoji 4 bytes / 2 units.
        assert_eq!(index.position(4), (1, 1));
        assert_eq!(index.position(8), (1, 3));
        assert_eq!(index.position(100), (2, 0));
    }

    #[test]
    fn manifest_paths_must_stay_inside() {
        assert_eq!(manifest_path("App/User.php"), Some("App/User.php".into()));
        assert_eq!(manifest_path("../x.php"), None);
        assert_eq!(manifest_path("/etc/passwd"), None);
        assert_eq!(manifest_path("a/./b"), Some("a/b".into()));
        assert_eq!(manifest_path(""), None);
    }

    #[test]
    fn source_map_tokens() {
        use stir_emit::ast::Origin;
        use stir_emit::{Mapping, Position};
        let source = "val x = \"é\"\nval y = 2\n";
        let code = "<?php\n$x = 'é'; $y = 2;\n".to_owned();
        let pos = |offset: u32, line, col| Position { offset, line, col };
        let emitted = Emitted {
            mappings: vec![
                Mapping {
                    start: pos(6, 1, 0),
                    end: pos(16, 1, 10),
                    origin: Origin::new(0, 0, 11),
                },
                // Same start as the outer one: the inner mapping wins.
                Mapping {
                    start: pos(6, 1, 0),
                    end: pos(8, 1, 2),
                    origin: Origin::new(0, 4, 5),
                },
                Mapping {
                    start: pos(17, 1, 11),
                    end: pos(24, 1, 18),
                    origin: Origin::new(0, 13, 22),
                },
            ],
            code,
            needs_runtime: false,
        };
        let bytes = source_map(
            &emitted,
            Path::new("/p/build/App/X.php"),
            Path::new("/p/src/App/X.sphp"),
            source,
        );
        let map = sourcemap::SourceMap::from_slice(&bytes).unwrap();
        assert_eq!(map.get_file(), Some("X.php"));
        assert_eq!(map.get_source(0), Some("../../src/App/X.sphp"));
        assert_eq!(map.get_source_contents(0), Some(source));
        let tokens: Vec<_> = map.tokens().map(|t| (t.get_dst(), t.get_src())).collect();
        // `é` is two bytes in the output but one UTF-16 column.
        assert_eq!(tokens, vec![((1, 0), (0, 4)), ((1, 10), (1, 0))]);
    }
}
