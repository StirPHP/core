//! Code actions, one fixture each, in `tests/actions/`:
//!
//! ```text
//! === action: Import `App\Foo`
//! === php: 8.1                  (optional: the versions the project runs on)
//! === level: 6                  (optional: the rule level, 5 otherwise)
//! === path: /src/a.blade.php    (optional: where the input is, /src/main.php otherwise)
//! === crlf                      (optional: the files have CRLF line endings)
//! === fixes: class.notFound     (optional: this is gone afterwards)
//! === new: a.b                  (optional: this may be new afterwards)
//! === extensions: redis, amqp   (optional: the extensions Composer's files declare)
//! === file: /src/Other.php      (any number of other files; under /vendor/ a dependency's, and
//!                               stubs://redis/Redis.php a stub of PHP's or an extension's)
//! <?php ...
//! === input
//! <?php ...  ‸ the cursor, or ⟦a selection⟧
//! === output                    (or `=== no-offer`, with no text)
//! <?php ...
//! === output: /src/Other.php    (what's expected of another file, if it changes)
//! === error: what it says        (instead of an output: chosen, it says this and does nothing)
//! === refused: what it says      (instead of an output: it's shown greyed out, saying this)
//! ```
//!
//! For each: the edits leave every file parsing, diagnostics they don't explain don't appear, and
//! what `fixes` names has gone.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use salsa::Setter as _;
use stir_db::{FileOrigin, PhpVersion, PhpVersions, RootDatabase, Settings, SourceFile, Workspace};
use stir_lsp::actions::{self, ClientAbilities, Cx};
use stir_syntax::{TextRange, TextSize};

#[derive(Default)]
struct Fixture {
    action: String,
    php: Option<String>,
    level: Option<u8>,
    path: Option<String>,
    crlf: bool,
    extensions: Option<Vec<String>>,
    fixes: Vec<String>,
    new: Vec<String>,
    others: Vec<(String, String)>,
    input: String,
    output: Option<String>,
    other_outputs: Vec<(String, String)>,
    no_offer: bool,
    refused: Option<String>,
    /// It's offered, and when it's chosen says this instead of doing anything.
    error: Option<String>,
}

fn parse_fixture(text: &str) -> Fixture {
    let mut f = Fixture::default();
    #[derive(Clone)]
    enum Part {
        None,
        File(String),
        Input,
        Output,
        OtherOutput(String),
    }
    let mut part = Part::None;
    let mut body = String::new();
    let flush = |part: &Part, body: &mut String, f: &mut Fixture| {
        let text = std::mem::take(body);
        match part {
            Part::None => {}
            Part::File(path) => f.others.push((path.clone(), text)),
            Part::Input => f.input = text,
            Part::Output => f.output = Some(text),
            Part::OtherOutput(path) => f.other_outputs.push((path.clone(), text)),
        }
    };
    for line in text.split_inclusive('\n') {
        let Some(head) = line.strip_prefix("=== ") else {
            body.push_str(line);
            continue;
        };
        flush(&part, &mut body, &mut f);
        part = Part::None;
        let head = head.trim_end();
        let (word, rest) = head.split_once(": ").unwrap_or((head, ""));
        match word {
            "action" => f.action = rest.to_owned(),
            "php" => f.php = Some(rest.to_owned()),
            "level" => f.level = rest.parse().ok(),
            "path" => f.path = Some(rest.to_owned()),
            "crlf" => f.crlf = true,
            "extensions" => {
                f.extensions = Some(rest.split(',').map(|e| e.trim().to_owned()).collect());
            }
            "fixes" => f.fixes.push(rest.to_owned()),
            "new" => f.new.push(rest.to_owned()),
            "file" => part = Part::File(rest.to_owned()),
            "input" => part = Part::Input,
            "output" if rest.is_empty() => part = Part::Output,
            "output" => part = Part::OtherOutput(rest.to_owned()),
            "no-offer" => f.no_offer = true,
            "refused" => f.refused = Some(rest.to_owned()),
            "error" => f.error = Some(rest.to_owned()),
            other => panic!("unknown fixture line: {other}"),
        }
    }
    flush(&part, &mut body, &mut f);
    f
}

/// `text` without the markers, and the selection they marked.
fn take_markers(text: &str) -> (String, TextRange) {
    let mut out = String::with_capacity(text.len());
    let (mut start, mut end) = (None, None);
    for c in text.chars() {
        match c {
            '‸' => {
                start = Some(out.len());
                end = Some(out.len());
            }
            '⟦' => start = Some(out.len()),
            '⟧' => end = Some(out.len()),
            _ => out.push(c),
        }
    }
    let start = start.expect("a fixture marks a cursor or a selection");
    let end = end.expect("a selection has an end");
    (
        out,
        TextRange::new(TextSize::from(start as u32), TextSize::from(end as u32)),
    )
}

fn crlf(text: &str, on: bool) -> String {
    if on {
        text.replace('\n', "\r\n")
    } else {
        text.to_owned()
    }
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "test") {
            out.push(path);
        }
    }
}

/// How many of each code a file has.
fn code_counts(db: &RootDatabase, ws: Workspace, file: SourceFile) -> BTreeMap<String, i32> {
    let mut counts = BTreeMap::new();
    for d in stir_checker::check_file(db, ws, file).iter() {
        *counts.entry(d.code.to_string()).or_insert(0) += 1;
    }
    counts
}

struct Bench {
    db: RootDatabase,
    ws: Workspace,
    stubs: Vec<SourceFile>,
}

fn settings(f: &Fixture) -> Settings {
    let mut settings = Settings {
        php_level: f.level.unwrap_or(5),
        declared_extensions: f.extensions.clone(),
        ..Settings::default()
    };
    if let Some(php) = &f.php {
        let version = PhpVersion::parse(php).expect("a version");
        settings.php_version = PhpVersions {
            min: version,
            max: version,
        };
    }
    settings
}

/// Runs one fixture: what's wrong with it, if anything.
fn run(bench: &mut Bench, f: &Fixture) -> Result<(), String> {
    let (input, selection) = take_markers(&f.input);
    let input = crlf(&input, f.crlf);
    // the markers were counted without the line endings
    let selection = {
        let shift = |at: TextSize| {
            let before = &crlf(&take_markers(&f.input).0, false)[..usize::from(at)];
            let lines = before.matches('\n').count();
            at + TextSize::from(if f.crlf { lines as u32 } else { 0 })
        };
        TextRange::new(shift(selection.start()), shift(selection.end()))
    };
    let db = &mut bench.db;
    let file = SourceFile::new(
        db,
        f.path.as_deref().unwrap_or("/src/main.php").into(),
        input.clone(),
        FileOrigin::Source,
    );
    let mut others = Vec::new();
    for (path, text) in &f.others {
        // a file under `/vendor/` is a dependency's, as the server has them, and a `stubs://`
        // one a stub of PHP's or an extension's
        let origin = if path.starts_with("/vendor/") {
            FileOrigin::Vendor
        } else if path.starts_with("stubs://") {
            FileOrigin::Stub
        } else {
            FileOrigin::Source
        };
        others.push(SourceFile::new(db, path.into(), crlf(text, f.crlf), origin));
    }
    let mut files = bench.stubs.clone();
    files.push(file);
    files.extend(others.iter().copied());
    bench.ws.set_files(db).to(files);
    bench.ws.set_settings(db).to(settings(f));
    let ws = bench.ws;

    let before: Vec<(SourceFile, BTreeMap<String, i32>)> = std::iter::once(file)
        .chain(others.iter().copied())
        .map(|s| (s, code_counts(db, ws, s)))
        .collect();

    let change = {
        let cx = Cx::new(
            db,
            ws,
            file,
            selection,
            &[],
            None::<Vec<String>>.or(Some(vec![
                "quickfix".into(),
                "refactor".into(),
                "source".into(),
            ])),
            ClientAbilities::default(),
        );
        let offers = actions::offers(&cx);
        let titles: Vec<String> = offers.iter().map(|o| o.title.clone()).collect();
        let found = offers.into_iter().find(|o| o.title == f.action);
        if f.no_offer {
            return match found {
                Some(o) if o.disabled.is_none() => {
                    Err(format!("offered although it shouldn't be: {}", o.title))
                }
                _ => Ok(()),
            };
        }
        let Some(found) = found else {
            return Err(format!("not offered; these were: {titles:?}"));
        };
        if let Some(reason) = &f.refused {
            return match &found.disabled {
                Some(d) if d.contains(reason.as_str()) => Ok(()),
                Some(d) => Err(format!("refused for another reason: {d}")),
                None => Err("offered, not refused".to_owned()),
            };
        }
        if let Some(reason) = &found.disabled {
            return Err(format!("refused: {reason}"));
        }
        // through the protocol's way of finding it again, as an editor gets it
        let id = found.id.clone();
        drop(found);
        match (actions::resolve(&cx, &id), &f.error) {
            (Err(reason), Some(want)) => {
                return if reason.contains(want.as_str()) {
                    Ok(())
                } else {
                    Err(format!("refused for another reason: {reason}"))
                };
            }
            (Err(reason), None) => return Err(format!("couldn't be done: {reason}")),
            (Ok(_), Some(want)) => return Err(format!("done, not refused with: {want}")),
            (Ok(change), None) => change,
        }
    };

    let mut touched: Vec<(SourceFile, String)> = Vec::new();
    for (s, _) in &before {
        if change.edits.iter().any(|e| e.file == *s) {
            let new = actions::apply(s.text(db), &change, *s)?;
            touched.push((*s, new));
        }
    }
    for (path, want) in &f.other_outputs {
        let got = touched
            .iter()
            .find(|(s, _)| s.path(db) == Path::new(path))
            .map_or_else(
                || {
                    others
                        .iter()
                        .find(|s| s.path(db) == Path::new(path))
                        .map(|s| s.text(db).clone())
                        .unwrap_or_default()
                },
                |(_, t)| t.clone(),
            );
        let want = crlf(want, f.crlf);
        if got != want {
            return Err(format!(
                "{path} isn't as expected:\n--- got\n{got}\n--- wanted\n{want}"
            ));
        }
    }
    let got = touched
        .iter()
        .find(|(s, _)| *s == file)
        .map_or_else(|| input.clone(), |(_, t)| t.clone());
    if let Some(want) = &f.output {
        let want = crlf(want, f.crlf);
        if got != want {
            return Err(format!(
                "not as expected:\n--- got\n{got}\n--- wanted\n{want}"
            ));
        }
    }
    for (s, new) in &touched {
        let errors = stir_syntax::parse_path(s.path(db), new);
        if !errors.errors().is_empty() {
            return Err(format!("doesn't parse: {:?}\n{new}", errors.errors()));
        }
        s.set_text(db).to(new.clone());
    }
    for (s, was) in &before {
        let now = code_counts(db, ws, *s);
        if *s == file {
            for code in &f.fixes {
                if now.get(code).copied().unwrap_or(0) >= was.get(code).copied().unwrap_or(0) {
                    return Err(format!("`{code}` isn't fixed: {now:?}, from {was:?}"));
                }
            }
        }
        for (code, n) in &now {
            if *n > was.get(code).copied().unwrap_or(0) && !f.new.contains(code) {
                let found: Vec<_> = stir_checker::check_file(db, ws, *s)
                    .iter()
                    .filter(|d| d.code == *code)
                    .map(|d| d.message.clone())
                    .collect();
                return Err(format!(
                    "new `{code}` in {}: {found:?}\n{}",
                    s.path(db).display(),
                    s.text(db)
                ));
            }
        }
    }
    Ok(())
}

#[test]
fn fixtures() {
    let dir = std::env::var_os("ACTION_DIR").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/actions"),
        PathBuf::from,
    );
    let mut files = Vec::new();
    collect_files(&dir, &mut files);
    files.sort();
    assert!(!files.is_empty());
    let filter = std::env::var("ACTION_FIXTURE").unwrap_or_default();
    let db = RootDatabase::default();
    // what fixtures use of PHP's own, as all of it takes long to index for each one
    let stubs: Vec<SourceFile> = stir_stubs::stub_files()
        .filter(|stub| {
            stub.path.starts_with("Core/")
                || stub.path.starts_with("SPL/")
                || stub.path.starts_with("standard/standard_0")
                || stub.path.starts_with("override")
        })
        .map(|stub| {
            SourceFile::new(
                &db,
                PathBuf::from(format!("stubs://{}", stub.path)),
                stub.text.to_owned(),
                FileOrigin::Stub,
            )
        })
        .collect();
    let ws = Workspace::new(&db, stubs.clone(), Settings::default());
    let mut bench = Bench { db, ws, stubs };
    let mut failures = Vec::new();
    let mut ran = 0;
    for path in files {
        let name = path
            .strip_prefix(&dir)
            .unwrap()
            .to_string_lossy()
            .into_owned();
        if !name.contains(&filter) {
            continue;
        }
        ran += 1;
        let text = std::fs::read_to_string(&path).unwrap();
        let fixture = parse_fixture(&text);
        let run =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&mut bench, &fixture)));
        match run {
            Ok(Ok(())) => {}
            Ok(Err(e)) => failures.push(format!("{name}: {e}")),
            Err(_) => failures.push(format!("{name}: panicked")),
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {ran} failed:\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
}
