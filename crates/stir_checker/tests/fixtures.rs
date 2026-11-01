//! Fixture tests: every file under `tests/fixtures/` is checked; expected
//! diagnostics are written as trailing comments on the offending line:
//! `// error: SPHP0005` or `// warning: SPHP0021` (several codes separated by `,`).
//! Run a single fixture with `cargo test -p stir_checker --test fixtures -- <name>`.

use std::path::{Path, PathBuf};

use libtest_mimic::{Arguments, Failed, Trial};
use stir_db::{FileOrigin, RootDatabase, Severity, SourceFile, Workspace};
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
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../stdlib");
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
        paths.sort();
        for p in paths {
            if p.extension().is_some_and(|e| e == "sphp") {
                let text = std::fs::read_to_string(&p).unwrap();
                out.push(SourceFile::new(
                    db,
                    p,
                    text,
                    Dialect::Stir,
                    FileOrigin::Vendor,
                ));
            }
        }
    }
    out
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

fn run(path: &Path) -> Result<(), Failed> {
    let text = std::fs::read_to_string(path).unwrap();
    let dialect = Dialect::from_path(path).unwrap();
    let db = RootDatabase::default();
    let file = SourceFile::new(
        &db,
        path.to_path_buf(),
        text.clone(),
        dialect,
        FileOrigin::Source,
    );
    let mut files = vec![file];
    files.extend(stubs(&db));
    files.extend(stdlib(&db));
    let ws = Workspace::new(&db, files);
    let diags = stir_checker::check_file(&db, ws, file);
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
    let want = expected(&text);
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
    Err(msg.into())
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
            Trial::test(name, move || run(&p))
        })
        .collect();
    libtest_mimic::run(&args, trials).exit();
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else if Dialect::from_path(&p).is_some() {
            out.push(p);
        }
    }
    out
}
