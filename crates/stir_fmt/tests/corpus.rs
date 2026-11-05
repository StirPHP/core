//! The formatter over every PHP file in `corpus/`, in memory: every token survives, a second run
//! changes nothing, and nothing that parsed stops parsing. `cargo test --release -p stir_fmt --test
//! corpus -- --ignored --nocapture`

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use stir_fmt::{Indent, Options};

fn php_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if path.file_name().is_some_and(|n| n != ".git") {
                php_files(&path, out);
            }
        } else if kind.is_file() && stir_syntax::is_php_path(&path) {
            out.push(path);
        }
    }
}

/// Non-whitespace tokens, each as it is, comments without the whitespace at their lines' ends:
/// nothing here is shared with what the formatter checks itself against.
fn tokens(source: &str) -> Vec<(stir_syntax::SyntaxKind, String)> {
    let mut at = 0;
    let mut out = Vec::new();
    for t in stir_syntax::lex(source).tokens {
        let text = &source[at..at + t.len as usize];
        at += t.len as usize;
        match t.kind {
            stir_syntax::SyntaxKind::WHITESPACE => {}
            stir_syntax::SyntaxKind::COMMENT | stir_syntax::SyntaxKind::DOC_COMMENT => {
                let lines: Vec<&str> = text.lines().map(str::trim).collect();
                out.push((t.kind, lines.join("\n")));
            }
            _ => out.push((t.kind, text.to_owned())),
        }
    }
    out
}

/// Runs over `files` on every core; what comes back are the counts.
fn run(files: &[PathBuf], options: &Options) -> Report {
    let next = AtomicUsize::new(0);
    let report = Mutex::new(Report::default());
    std::thread::scope(|scope| {
        for _ in 0..std::thread::available_parallelism().map_or(4, |n| n.get()) {
            scope.spawn(|| {
                let mut local = Report::default();
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(path) = files.get(i) else { break };
                    let Ok(bytes) = std::fs::read(path) else {
                        continue;
                    };
                    let Ok(source) = String::from_utf8(bytes) else {
                        local.not_utf8 += 1;
                        continue;
                    };
                    local.visit(path, &source, options);
                }
                report.lock().unwrap().merge(local);
            });
        }
    });
    report.into_inner().unwrap()
}

#[derive(Default)]
struct Report {
    files: usize,
    bytes: usize,
    lines: usize,
    lines_changed: usize,
    unchanged_files: usize,
    not_utf8: usize,
    /// Don't parse to begin with.
    refused: usize,
    tokens_changed: Vec<String>,
    unstable: Vec<String>,
    broken: Vec<String>,
}

impl Report {
    fn merge(&mut self, o: Report) {
        self.files += o.files;
        self.bytes += o.bytes;
        self.lines += o.lines;
        self.lines_changed += o.lines_changed;
        self.unchanged_files += o.unchanged_files;
        self.not_utf8 += o.not_utf8;
        self.refused += o.refused;
        self.tokens_changed.extend(o.tokens_changed);
        self.unstable.extend(o.unstable);
        self.broken.extend(o.broken);
    }

    fn visit(&mut self, path: &Path, source: &str, options: &Options) {
        self.files += 1;
        self.bytes += source.len();
        self.lines += source.lines().count();
        let name = path.display().to_string();
        let edits = match stir_fmt::edits(source, options) {
            Ok(e) => e,
            Err(stir_fmt::Error::Syntax) => {
                self.refused += 1;
                return;
            }
            Err(stir_fmt::Error::TokensChanged) => {
                self.tokens_changed.push(name);
                return;
            }
        };
        if edits.is_empty() {
            self.unchanged_files += 1;
        }
        let starts: Vec<usize> = std::iter::once(0)
            .chain(source.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        let line_of = |at: usize| starts.partition_point(|s| *s <= at) - 1;
        let mut touched = Vec::new();
        for e in &edits {
            touched.push(line_of(e.range.start().into()));
        }
        touched.sort_unstable();
        touched.dedup();
        self.lines_changed += touched.len();
        let once = stir_fmt::apply(source, &edits);
        if tokens(source) != tokens(&once) {
            self.tokens_changed.push(name.clone());
        }
        if !stir_syntax::parse(&once).errors().is_empty() {
            self.broken.push(name.clone());
        }
        match stir_fmt::format(&once, options) {
            Ok(twice) if twice == once => {}
            _ => self.unstable.push(name),
        }
    }

    fn print(&self, label: &str, seconds: f64) {
        println!(
            "{label}: {} files, {:.1} MB in {seconds:.1}s ({:.0} MB/s over {} threads, twice each)",
            self.files,
            self.bytes as f64 / 1e6,
            self.bytes as f64 / 1e6 / seconds,
            std::thread::available_parallelism().map_or(4, |n| n.get()),
        );
        println!(
            "  {} of {} lines changed ({:.1}%), {} files untouched, {} don't parse, {} aren't UTF-8",
            self.lines_changed,
            self.lines,
            self.lines_changed as f64 * 100.0 / self.lines.max(1) as f64,
            self.unchanged_files,
            self.refused,
            self.not_utf8,
        );
        println!(
            "  tokens changed: {}, unstable: {}, newly broken: {}",
            self.tokens_changed.len(),
            self.unstable.len(),
            self.broken.len()
        );
        for name in self.tokens_changed.iter().take(10) {
            println!("    tokens: {name}");
        }
        for name in self.unstable.iter().take(10) {
            println!("    unstable: {name}");
        }
        for name in self.broken.iter().take(10) {
            println!("    broken: {name}");
        }
    }
}

fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus")
}

#[test]
#[ignore = "needs corpus/, and takes a while"]
fn all_of_the_corpus() {
    let mut files = Vec::new();
    php_files(&corpus(), &mut files);
    let start = Instant::now();
    let report = run(&files, &Options::default());
    report.print("corpus", start.elapsed().as_secs_f64());
    assert!(report.tokens_changed.is_empty());
    assert!(report.unstable.is_empty());
    assert!(report.broken.is_empty());
}

/// How far each project's own code is from what comes out, in its own indentation.
#[test]
#[ignore = "needs corpus/"]
fn how_close_to_each_project() {
    for (name, dirs, indent) in [
        ("framework", &["src", "tests"][..], Indent::Spaces(4)),
        ("symfony", &["src"][..], Indent::Spaces(4)),
        ("phpstan-src", &["src"][..], Indent::Tabs),
    ] {
        let mut files = Vec::new();
        for dir in dirs {
            php_files(&corpus().join(name).join(dir), &mut files);
        }
        let start = Instant::now();
        let options = Options { indent };
        run(&files, &options).print(name, start.elapsed().as_secs_f64());
    }
}
