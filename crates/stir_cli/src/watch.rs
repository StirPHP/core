//! `sphp build --watch`: rebuild whenever sources or `sphp.toml` change.
//!
//! File events come from `notify` (debounced by `notify-debouncer-full`) and are
//! bridged into a tokio channel. Between rebuilds the salsa database is kept: when
//! only the contents of known sources changed, their `SourceFile` inputs are updated
//! with `set_text`, so unchanged files are not re-parsed or re-checked. Added or
//! removed files and config changes trigger a full reload.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use notify_debouncer_full::notify::{EventKind, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, new_debouncer};
use salsa::Setter as _;
use stir_project::Project;

use crate::build::{BuildArgs, apply_overrides, build, options};
use crate::check::display_path;
use crate::workspace::{LoadOptions, Loaded, load_workspace};

const DEBOUNCE: Duration = Duration::from_millis(100);

/// What a batch of file events requires.
#[derive(Debug, Default)]
struct Changes {
    config: bool,
    sources: BTreeSet<PathBuf>,
}

impl Changes {
    fn is_empty(&self) -> bool {
        !self.config && self.sources.is_empty()
    }
}

pub async fn run(mut project: Project, args: BuildArgs, cwd: PathBuf) -> anyhow::Result<ExitCode> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<DebounceEventResult>();
    let mut debouncer = new_debouncer(DEBOUNCE, None, move |result| {
        let _ = tx.send(result);
    })?;
    // The project root non-recursively (for sphp.toml, which editors may replace
    // atomically) and the source directory recursively.
    debouncer.watch(&project.root, RecursiveMode::NonRecursive)?;
    let mut watched_src = watch_src(&mut debouncer, &project, None);

    let mut loaded = load_and_build(&project, &args, &cwd).await;
    print_watching(&project, &cwd);

    loop {
        let first = tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            event = rx.recv() => match event {
                Some(event) => event,
                None => break,
            },
        };
        let mut changes = Changes::default();
        classify(&project, first, &mut changes);
        while let Ok(more) = rx.try_recv() {
            classify(&project, more, &mut changes);
        }
        if changes.is_empty() {
            continue;
        }

        if changes.config {
            println!("\nsphp.toml changed, reloading project...");
            match Project::load_exact(&project.root).await {
                Ok(mut reloaded) => {
                    apply_overrides(&mut reloaded, &args, &cwd);
                    project = reloaded;
                    watched_src = watch_src(&mut debouncer, &project, watched_src);
                }
                Err(error) => {
                    eprintln!("error: {error}");
                    print_watching(&project, &cwd);
                    continue;
                }
            }
            loaded = None;
        } else {
            println!("\nChange detected, rebuilding...");
        }

        loaded = match loaded {
            Some(mut current) if !changes.config => {
                match update(&project, &mut current, &changes.sources).await {
                    Ok(true) => {
                        rebuild(&project, &current, &args, &cwd).await;
                        Some(current)
                    }
                    Ok(false) => load_and_build(&project, &args, &cwd).await,
                    Err(error) => {
                        eprintln!("error: {error:#}");
                        Some(current)
                    }
                }
            }
            _ => load_and_build(&project, &args, &cwd).await,
        };
        print_watching(&project, &cwd);
    }
    println!("Stopped watching.");
    Ok(ExitCode::SUCCESS)
}

fn print_watching(project: &Project, cwd: &Path) {
    let mut src = display_path(&project.src_dir(), cwd);
    if !src.ends_with('/') {
        src.push('/');
    }
    println!("Watching {src} for changes (press Ctrl-C to stop)");
}

/// Starts watching the project's source directory (recursively) and stops watching
/// `previous` if it differs. Returns the watched directory.
fn watch_src(
    debouncer: &mut notify_debouncer_full::Debouncer<
        notify_debouncer_full::notify::RecommendedWatcher,
        notify_debouncer_full::RecommendedCache,
    >,
    project: &Project,
    previous: Option<PathBuf>,
) -> Option<PathBuf> {
    let src = project.src_dir();
    if previous.as_ref() == Some(&src) {
        return previous;
    }
    if let Some(previous) = previous {
        let _ = debouncer.unwatch(&previous);
        if previous == project.root {
            let _ = debouncer.watch(&project.root, RecursiveMode::NonRecursive);
        }
    }
    if src == project.root {
        // Watch the whole tree; the non-recursive root watch is superseded.
        let _ = debouncer.unwatch(&project.root);
    }
    match debouncer.watch(&src, RecursiveMode::Recursive) {
        Ok(()) => Some(src),
        Err(error) => {
            eprintln!("warning: cannot watch `{}`: {error}", src.display());
            None
        }
    }
}

/// Sorts debounced events into config and source changes, ignoring reads and
/// anything inside the output directory.
fn classify(project: &Project, result: DebounceEventResult, changes: &mut Changes) {
    let events = match result {
        Ok(events) => events,
        Err(errors) => {
            for error in errors {
                tracing::warn!(%error, "file watcher error");
            }
            return;
        }
    };
    let config = project.config_path();
    let src = project.src_dir();
    let out = project.out_dir();
    for event in events {
        if matches!(event.kind, EventKind::Access(_)) {
            continue;
        }
        for path in &event.paths {
            if *path == config {
                changes.config = true;
            } else if path.starts_with(&src) && !path.starts_with(&out) {
                changes.sources.insert(path.clone());
            }
        }
    }
}

async fn load_and_build(project: &Project, args: &BuildArgs, cwd: &Path) -> Option<Loaded> {
    match load_workspace(project, LoadOptions::default()).await {
        Ok(loaded) => {
            rebuild(project, &loaded, args, cwd).await;
            Some(loaded)
        }
        Err(error) => {
            eprintln!("error: {error:#}");
            None
        }
    }
}

async fn rebuild(project: &Project, loaded: &Loaded, args: &BuildArgs, cwd: &Path) {
    if let Err(error) = build(project, loaded, &options(project, args), cwd).await {
        eprintln!("error: {error:#}");
    }
}

/// Updates the database in place for content changes. Returns `false` when the set
/// of source files changed and a full reload is needed.
async fn update(
    project: &Project,
    loaded: &mut Loaded,
    changed: &BTreeSet<PathBuf>,
) -> anyhow::Result<bool> {
    let discovered: BTreeSet<PathBuf> = stir_project::discover(project)
        .await?
        .into_iter()
        .map(|f| f.path)
        .collect();
    let current: BTreeSet<PathBuf> = loaded
        .sources
        .iter()
        .map(|f| f.path(&loaded.db).clone())
        .collect();
    if discovered != current {
        return Ok(false);
    }
    for file in loaded.sources.clone() {
        let path = file.path(&loaded.db).clone();
        if !changed.contains(&path) {
            continue;
        }
        let text = match tokio::fs::read_to_string(&path).await {
            Ok(text) => text,
            // Deleted between the event and discovery, or mid-write: reload fully.
            Err(_) => return Ok(false),
        };
        if *file.text(&loaded.db) != text {
            tracing::debug!(path = %path.display(), "source changed");
            file.set_text(&mut loaded.db).to(text);
        }
    }
    Ok(true)
}
