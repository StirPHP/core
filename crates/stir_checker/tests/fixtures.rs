//! Fixture tests: every file under `tests/fixtures/` is checked; expected
//! diagnostics are written as trailing comments on the offending line:
//! `// error: SPHP0005` or `// warning: SPHP0021` (several codes separated by `,`).
//! Run a single fixture with `cargo test -p stir_checker --test fixtures -- <name>`.
//!
//! A directory whose name ends in `.multi` is one fixture: all its files are
//! loaded into one workspace and each is checked against its own expectations.
//! Fixture-level settings are comments anywhere in the fixture's files:
//! `// @level N` (PHPStan level for `.php` files, default: everything) and
//! `// @check-php false` (`.php` files are then checked only with a
//! `// @sphp-check` directive; default: all are checked).

use std::path::{Path, PathBuf};

use libtest_mimic::{Arguments, Failed, Trial};
use stir_db::{FileOrigin, RootDatabase, Settings, Severity, SourceFile, Workspace};
use stir_syntax::Dialect;

fn stubs(db: &RootDatabase) -> Vec<SourceFile> {
    stir_stubs::stub_files()
        .map(|s| {
            SourceFile::new(
                db,
                format!("stubs://{}", s.path).into(),
                s.text.to_owned(),
                Dialect::Php,
                FileOrigin::Stub,
            )
        })
        .collect()
}

fn stdlib(db: &RootDatabase) -> Vec<SourceFile> {
    stir_stubs::stdlib_files()
        .map(|f| {
            SourceFile::new(
                db,
                format!("stdlib://{}", f.path).into(),
                f.text.to_owned(),
                Dialect::Stir,
                FileOrigin::Stub,
            )
        })
        .collect()
}

fn expected(text: &str) -> Vec<(usize, String, String)> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        for sev in ["error", "warning"] {
            let marker = format!("// {sev}: ");
            if let Some(pos) = line.find(&marker) {
                for code in line[pos + marker.len()..].split(',') {
                    out.push((i + 1, sev.to_owned(), code.trim().to_owned()));
                }
            }
        }
    }
    out.sort();
    out
}

/// Fixture-level settings from `// @level N` / `// @check-php false` comments.
fn settings(texts: &[String]) -> Settings {
    let mut settings = Settings {
        check_php: true,
        ..Settings::default()
    };
    for text in texts {
        for line in text.lines() {
            if let Some(pos) = line.find("// @level ") {
                let v = line[pos + 10..].trim();
                settings.php_level = Some(if v == "max" { 10 } else { v.parse().unwrap() });
            }
            if line.contains("// @check-php false") {
                settings.check_php = false;
            }
        }
    }
    settings
}

fn run(paths: &[PathBuf]) -> Result<(), Failed> {
    let texts: Vec<String> = paths
        .iter()
        .map(|p| std::fs::read_to_string(p).unwrap())
        .collect();
    let db = RootDatabase::default();
    let sources: Vec<SourceFile> = paths
        .iter()
        .zip(&texts)
        .map(|(path, text)| {
            SourceFile::new(
                &db,
                path.to_path_buf(),
                text.clone(),
                Dialect::from_path(path).unwrap(),
                FileOrigin::Source,
            )
        })
        .collect();
    let mut files = sources.clone();
    files.extend(stubs(&db));
    files.extend(stdlib(&db));
    let ws = Workspace::new(&db, files, settings(&texts));
    let mut msg = String::new();
    for ((path, text), file) in paths.iter().zip(&texts).zip(&sources) {
        if let Err(m) = check_one(&db, ws, *file, text) {
            if paths.len() > 1 {
                msg.push_str(&format!("{}:\n", path.file_name().unwrap().display()));
            }
            msg.push_str(&m);
        }
    }
    if msg.is_empty() {
        Ok(())
    } else {
        Err(msg.into())
    }
}

fn check_one(db: &RootDatabase, ws: Workspace, file: SourceFile, text: &str) -> Result<(), String> {
    let diags = stir_checker::check_file(db, ws, file);
    let line_of = |off: u32| text[..off as usize].matches('\n').count() + 1;
    let mut actual: Vec<(usize, String, String)> = diags
        .iter()
        .map(|d| {
            let sev = match d.severity {
                Severity::Error => "error",
                _ => "warning",
            };
            (
                line_of(u32::from(d.range.start())),
                sev.to_owned(),
                d.code.to_owned(),
            )
        })
        .collect();
    actual.sort();
    actual.dedup();
    let want = expected(text);
    if actual == want {
        return Ok(());
    }
    let mut msg = String::from("diagnostics differ\n");
    for d in diags.iter() {
        msg.push_str(&format!(
            "  got  {}:{} {} {}\n",
            line_of(u32::from(d.range.start())),
            d.code,
            match d.severity {
                Severity::Error => "error",
                _ => "warning",
            },
            d.message
        ));
    }
    for w in &want {
        if !actual.contains(w) {
            msg.push_str(&format!("  MISSING {}:{} {}\n", w.0, w.2, w.1));
        }
    }
    for a in &actual {
        if !want.contains(a) {
            msg.push_str(&format!("  UNEXPECTED {}:{} {}\n", a.0, a.2, a.1));
        }
    }
    Err(msg)
}

fn main() {
    let args = Arguments::from_args();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let mut paths: Vec<PathBuf> = walk(&dir);
    paths.sort();
    let trials = paths
        .into_iter()
        .map(|p| {
            let name = p.strip_prefix(&dir).unwrap().display().to_string();
            Trial::test(name, move || {
                if p.is_dir() {
                    let mut files: Vec<PathBuf> = std::fs::read_dir(&p)
                        .unwrap()
                        .flatten()
                        .map(|e| e.path())
                        .filter(|f| Dialect::from_path(f).is_some())
                        .collect();
                    files.sort();
                    run(&files)
                } else {
                    run(std::slice::from_ref(&p))
                }
            })
        })
        .collect();
    libtest_mimic::run(&args, trials).exit();
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() && p.extension().is_some_and(|e| e == "multi") {
            out.push(p);
        } else if p.is_dir() {
            out.extend(walk(&p));
        } else if Dialect::from_path(&p).is_some() {
            out.push(p);
        }
    }
    out
}
