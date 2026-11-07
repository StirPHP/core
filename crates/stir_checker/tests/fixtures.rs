//! Fixture tests: every file under `tests/fixtures/` is checked; expected
//! diagnostics are written as trailing comments on the offending line:
//! `// error: return.type` or `// warning: SPHP0060` (several codes separated by `,`).
//! Run a single fixture with `cargo test -p stir_checker --test fixtures -- <name>`.
//!
//! A directory whose name ends in `.multi` is one fixture: all its files are
//! loaded into one workspace and each is checked against its own expectations.
//! Fixture-level settings are comments anywhere in the fixture's files:
//! `// @level N` (PHPStan level for `.php` files, default: everything), `// @php 8.1` or
//! `// @php 8.1..8.3` (the PHP versions the project runs on, default: the latest), and
//! `// @framework laravel` (what StirPHP ships for that framework, its script and stubs).
//! A `.multi` fixture's files under `vendor/` are a package's: they're loaded but not checked.
//!
//! `missingType.*`, which would be all over fixtures about something else, only counts in a fixture
//! that expects one.

use std::path::{Path, PathBuf};

use libtest_mimic::{Arguments, Failed, Trial};
use stir_db::{
    FileOrigin, PhpVersion, PhpVersions, RootDatabase, Settings, Severity, SourceFile, Workspace,
};

fn stubs(db: &RootDatabase) -> Vec<SourceFile> {
    stir_stubs::stub_files()
        .map(|s| {
            SourceFile::new(
                db,
                format!("stubs://{}", s.path).into(),
                s.text.to_owned(),
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
    out.dedup();
    out
}

/// Fixture-level settings from `// @level N` and `// @php 8.1` comments.
fn settings(texts: &[String]) -> Settings {
    let mut settings = Settings {
        php_level: stir_db::MAX_PHP_LEVEL,
        ..Settings::default()
    };
    for text in texts {
        for line in text.lines() {
            if let Some(pos) = line.find("// @level ") {
                let v = line[pos + 10..].trim();
                settings.php_level = if v == "max" { 10 } else { v.parse().unwrap() };
            }
            if let Some(pos) = line.find("// @php ") {
                let v = line[pos + 8..].trim();
                let version = |s| PhpVersion::parse(s).unwrap();
                settings.php_version = match v.split_once("..") {
                    Some((min, max)) => PhpVersions {
                        min: version(min),
                        max: version(max),
                    },
                    None => PhpVersions::exactly(version(v)),
                };
            }
        }
    }
    settings
}

/// The frameworks `// @framework name` comments ask for.
fn frameworks(texts: &[String]) -> Vec<&'static stir_stubs::frameworks::Framework> {
    let mut out = Vec::new();
    for text in texts {
        for line in text.lines() {
            if let Some(pos) = line.find("// @framework ") {
                let name = line[pos + 14..].trim();
                out.push(stir_stubs::frameworks::framework(name).expect("a known framework"));
            }
        }
    }
    out
}

fn is_vendor(path: &Path) -> bool {
    path.components().any(|c| c.as_os_str() == "vendor")
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
            let origin = if is_vendor(path) {
                FileOrigin::Vendor
            } else {
                FileOrigin::Source
            };
            SourceFile::new(&db, path.to_path_buf(), text.clone(), origin)
        })
        .collect();
    let mut files = sources.clone();
    files.extend(stubs(&db));
    let mut scripts = Vec::new();
    for f in frameworks(&texts) {
        scripts.push(stir_db::ScriptFile::new(
            &db,
            format!("{}{}", stir_db::BUNDLED_PREFIX, f.script_name).into(),
            Ok(f.script.to_owned()),
        ));
        files.extend(f.stubs.iter().map(|(path, text)| {
            SourceFile::new(
                &db,
                format!("stubs://{path}").into(),
                (*text).to_owned(),
                FileOrigin::Stub,
            )
        }));
    }
    // a `.multi` fixture's `.xml` files are what a framework left for the project, such as
    // Symfony's compiled container
    let data_files = paths
        .first()
        .and_then(|p| {
            p.ancestors()
                .find(|a| a.extension().is_some_and(|e| e == "multi"))
        })
        .map(|dir| {
            php_files_with(dir, "xml")
                .into_iter()
                .map(|p| {
                    let text = std::fs::read_to_string(&p).unwrap();
                    stir_db::DataFile::new(&db, p, text)
                })
                .collect()
        })
        .unwrap_or_default();
    let ws = Workspace::builder(files, settings(&texts))
        .bundled_scripts(scripts)
        .data_files(data_files)
        .new(&db);
    let mut msg = String::new();
    for ((path, text), file) in paths.iter().zip(&texts).zip(&sources) {
        if is_vendor(path) {
            continue;
        }
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
    let about_types = text.contains(": missingType.");
    let mut actual: Vec<(usize, String, String)> = diags
        .iter()
        .filter(|d| about_types || !d.code.starts_with("missingType."))
        .map(|d| {
            let sev = match d.severity {
                Severity::Error => "error",
                _ => "warning",
            };
            (
                line_of(u32::from(d.range.start())),
                sev.to_owned(),
                d.code.to_string(),
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
                    let mut files = php_files(&p);
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
        } else if stir_syntax::is_php_path(&p) {
            out.push(p);
        }
    }
    out
}

/// Every file with the extension `ext` under `dir`.
fn php_files_with(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(php_files_with(&p, ext));
        } else if p.extension().is_some_and(|e| e == ext) {
            out.push(p);
        }
    }
    out
}

/// Every PHP file under `dir`.
fn php_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(php_files(&p));
        } else if stir_syntax::is_php_path(&p) {
            out.push(p);
        }
    }
    out
}
