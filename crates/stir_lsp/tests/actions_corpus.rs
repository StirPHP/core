//! Code actions on real projects, in memory: asks for them at diagnostics and on a sample of places
//! and selections, applies each, and checks that the file still parses and has no new errors.
//!
//! ```sh
//! ACTIONS_PROJECTS=symfony ACTIONS_FILES=300 ACTIONS_REPORT=/tmp/report.txt \
//!   cargo test --release -p stir_lsp --test actions_corpus -- --ignored --nocapture
//! ```

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use salsa::Setter as _;
use stir_db::{FileOrigin, RootDatabase, SourceFile, Workspace};
use stir_lsp::actions::{self, ClientAbilities, Cx, Kind};
use stir_project::Project;
use stir_project::workspace::{LoadOptions, load_workspace};
use stir_syntax::{SyntaxKind::*, SyntaxNode, TextRange, TextSize};

/// A small deterministic generator: what's sampled is the same each run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

#[derive(Default)]
struct Tally {
    offered: usize,
    refused: usize,
    applied: usize,
    no_parse: usize,
    new_errors: usize,
    build_failed: usize,
}

#[derive(Default)]
struct Times(Vec<Duration>);

impl Times {
    fn add(&mut self, d: Duration) {
        self.0.push(d);
    }

    fn summary(&mut self) -> String {
        if self.0.is_empty() {
            return "none".to_owned();
        }
        self.0.sort();
        let at = |p: f64| self.0[((self.0.len() - 1) as f64 * p) as usize];
        format!(
            "n={} p50={:?} p95={:?} p99={:?} max={:?}",
            self.0.len(),
            at(0.5),
            at(0.95),
            at(0.99),
            self.0[self.0.len() - 1]
        )
    }
}

fn code_counts(db: &RootDatabase, ws: Workspace, file: SourceFile) -> BTreeMap<String, i32> {
    let mut counts = BTreeMap::new();
    for d in stir_checker::check_file(db, ws, file).iter() {
        *counts.entry(d.code.to_string()).or_insert(0) += 1;
    }
    counts
}

/// What can be new after an action, because of what the action is:
/// a class that wasn't known is now checked; a new parameter's placeholder is meant to be an error;
/// and a new function's types are only those that are certain, so that where the rule level asks
/// for more, there's less than it wants, and what the old code had narrowed isn't known in it.
fn explained(tag: &str, code: &str) -> bool {
    match tag {
        "import.class" | "typo.class" | "import.function" | "typo.function" | "import.const"
        | "typo.const" => true,
        "signature.add" => code == "constant.notFound" || code.starts_with("missingType."),
        "create.method"
        | "create.property"
        | "accessor.get"
        | "accessor.set"
        | "accessor.both"
        | "extract.function"
        | "override"
        | "implement"
        | "constructor.generate" => code.starts_with("missingType."),
        _ => false,
    }
}

/// The tag an offer is counted under: its id without what's particular to the place.
fn tag(id: &str) -> String {
    id.split(':').next().unwrap_or(id).to_owned()
}

struct Run {
    tally: BTreeMap<String, Tally>,
    offers_time: Times,
    warm_time: Times,
    build_time: Times,
    by_tag_time: BTreeMap<String, Times>,
    report: String,
    cap: usize,
    tried: usize,
    max_tried: usize,
}

/// Asks at `range` of `file`, and tries what's offered that hasn't been tried too many times.
fn try_at(
    db: &mut RootDatabase,
    ws: Workspace,
    file: SourceFile,
    range: TextRange,
    only: Option<Vec<String>>,
    run: &mut Run,
) {
    // what an editor has worked out by the time it asks: the file's diagnostics
    let warm = Instant::now();
    let _ = stir_checker::check_file(db, ws, file);
    let _ = stir_lsp::nav::file_refs(db, ws, file);
    run.warm_time.add(warm.elapsed());
    let start = Instant::now();
    let chosen: Vec<(String, String, Kind)> = {
        let cx = Cx::new(
            db,
            ws,
            file,
            range,
            &[],
            only.clone(),
            ClientAbilities::default(),
        );
        let offers = actions::offers(&cx);
        run.offers_time.add(start.elapsed());
        let mut chosen = Vec::new();
        for o in &offers {
            let tally = run.tally.entry(tag(&o.id)).or_default();
            tally.offered += 1;
            if o.disabled.is_some() {
                tally.refused += 1;
                continue;
            }
            chosen.push((o.id.clone(), o.title.clone(), o.kind));
        }
        chosen
    };
    for (id, title, kind) in chosen {
        let t = tag(&id);
        if t.starts_with("ignore") && run.tally[&t].applied > 300 {
            continue;
        }
        let cap = if t.starts_with("signature") {
            run.cap.min(env_number("ACTIONS_SIGNATURES", 25))
        } else {
            run.cap
        };
        if run.tally[&t].applied + run.tally[&t].build_failed >= cap || run.tried >= run.max_tried {
            continue;
        }
        run.tried += 1;
        // what takes the workspace waits for it to be checked, which an editor has done meanwhile
        if t.starts_with("signature") {
            for f in ws
                .files(db)
                .iter()
                .filter(|f| f.origin(db) == FileOrigin::Source)
            {
                let _ = stir_lsp::nav::file_refs(db, ws, *f);
            }
        }
        let start = Instant::now();
        let change = {
            let cx = Cx::new(
                db,
                ws,
                file,
                range,
                &[],
                Some(vec![kind.as_str().to_owned()]),
                ClientAbilities::default(),
            );
            actions::resolve(&cx, &id)
        };
        run.build_time.add(start.elapsed());
        run.by_tag_time
            .entry(t.clone())
            .or_default()
            .add(start.elapsed());
        let change = match change {
            Ok(c) => c,
            Err(reason) => {
                let tally = run.tally.get_mut(&t).unwrap();
                if t.starts_with("signature") && tally.build_failed < 15 {
                    let _ = writeln!(
                        run.report,
                        "REFUSED {} [{title}]: {reason}",
                        file.path(db).display()
                    );
                }
                if reason.contains("syntax errors") || reason.contains("overlap") {
                    tally.no_parse += 1;
                    let _ = writeln!(
                        run.report,
                        "NO-PARSE {} @{}..{} [{title}]: {reason}",
                        file.path(db).display(),
                        u32::from(range.start()),
                        u32::from(range.end()),
                    );
                } else {
                    tally.build_failed += 1;
                }
                continue;
            }
        };
        let mut files: Vec<SourceFile> = Vec::new();
        for e in &change.edits {
            if !files.contains(&e.file) {
                files.push(e.file);
            }
        }
        let mut undo = Vec::new();
        let mut before = Vec::new();
        for f in &files {
            before.push((*f, code_counts(db, ws, *f)));
            let old = f.text(db).clone();
            let new = actions::apply(&old, &change, *f).unwrap();
            undo.push((*f, old));
            f.set_text(db).to(new);
        }
        let tally = run.tally.get_mut(&t).unwrap();
        tally.applied += 1;
        let mut bad = Vec::new();
        for (f, was) in &before {
            let now = code_counts(db, ws, *f);
            for (code, n) in &now {
                let had = was.get(code).copied().unwrap_or(0);
                if *n > had && !explained(&t, code) {
                    let messages: Vec<String> = stir_checker::check_file(db, ws, *f)
                        .iter()
                        .filter(|d| d.code == *code)
                        .take(2)
                        .map(|d| d.message.clone())
                        .collect();
                    bad.push(format!("{code} {had}->{n} {messages:?}"));
                }
            }
        }
        if !bad.is_empty() && std::env::var_os("ACTIONS_SHOW").is_some() {
            for f in &files {
                println!("--- {}\n{}", f.path(db).display(), f.text(db));
            }
        }
        if !bad.is_empty() {
            let tally = run.tally.get_mut(&t).unwrap();
            tally.new_errors += 1;
            let _ = writeln!(
                run.report,
                "NEW-ERROR {} @{}..{} [{title}]: {}",
                file.path(db).display(),
                u32::from(range.start()),
                u32::from(range.end()),
                bad.join("; ")
            );
        }
        for (f, old) in undo {
            f.set_text(db).to(old);
        }
    }
}

/// Places worth asking about, from a fixed-seed sample of what's in `root`.
fn sample_places(root: &SyntaxNode, rng: &mut Rng, per_file: usize) -> Vec<TextRange> {
    let mut places = Vec::new();
    let tokens: Vec<_> = root
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| {
            matches!(
                t.kind(),
                VARIABLE | IF_KW | ELSE_KW | SWITCH_KW | NAME_FULLY_QUALIFIED | FUNCTION_KW
            ) || t.kind() == IDENT && t.parent().is_some_and(|p| p.kind() == NAME)
        })
        .collect();
    for _ in 0..per_file {
        if tokens.is_empty() {
            break;
        }
        let t = &tokens[rng.below(tokens.len())];
        places.push(TextRange::empty(t.text_range().start()));
    }
    places
}

fn is_statement(n: &SyntaxNode) -> bool {
    matches!(
        n.kind(),
        EXPR_STMT
            | ECHO_STMT
            | RETURN_STMT
            | IF_STMT
            | WHILE_STMT
            | DO_WHILE_STMT
            | FOR_STMT
            | FOREACH_STMT
            | SWITCH_STMT
            | TRY_STMT
            | UNSET_STMT
    )
}

fn trimmed(n: &SyntaxNode) -> TextRange {
    let tokens: Vec<_> = n
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| !t.kind().is_trivia())
        .collect();
    match (tokens.first(), tokens.last()) {
        (Some(a), Some(b)) => TextRange::new(a.text_range().start(), b.text_range().end()),
        _ => n.text_range(),
    }
}

/// Runs of statements in a function, for extracting, and expressions.
fn sample_selections(root: &SyntaxNode, rng: &mut Rng, per_file: usize) -> Vec<TextRange> {
    let blocks: Vec<SyntaxNode> = root
        .descendants()
        .filter(|n| {
            n.kind() == BLOCK
                && n.ancestors()
                    .skip(1)
                    .any(|a| matches!(a.kind(), METHOD_DECL | FUNCTION_DECL))
        })
        .collect();
    let exprs: Vec<SyntaxNode> = root
        .descendants()
        .filter(|n| {
            matches!(
                n.kind(),
                METHOD_CALL_EXPR
                    | CALL_EXPR
                    | BIN_EXPR
                    | STATIC_CALL_EXPR
                    | PROPERTY_FETCH_EXPR
                    | NEW_EXPR
                    | ARRAY_EXPR
                    | CLOSURE_EXPR
                    | MATCH_EXPR
            )
        })
        .collect();
    let mut out = Vec::new();
    for i in 0..per_file {
        if i % 2 == 0 && !blocks.is_empty() {
            let block = &blocks[rng.below(blocks.len())];
            let statements: Vec<SyntaxNode> = block.children().filter(is_statement).collect();
            if statements.is_empty() {
                continue;
            }
            let first = rng.below(statements.len());
            let len = 1 + rng.below((statements.len() - first).min(4));
            let a = trimmed(&statements[first]);
            let b = trimmed(&statements[first + len - 1]);
            out.push(TextRange::new(a.start(), b.end()));
        } else if !exprs.is_empty() {
            out.push(trimmed(&exprs[rng.below(exprs.len())]));
        }
    }
    out
}

fn env_number(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

async fn run_project(dir: &Path, report_path: Option<&Path>) {
    let project = Project::load_or_default(dir).await.expect("the project");
    let loaded = load_workspace(&project, LoadOptions::default())
        .await
        .expect("the workspace");
    let mut db = loaded.db;
    let ws = loaded.workspace;
    let sources: Vec<SourceFile> = ws
        .files(&db)
        .iter()
        .copied()
        .filter(|f| f.origin(&db) == FileOrigin::Source)
        .collect();
    println!("{}: {} source files", dir.display(), sources.len());
    let start = Instant::now();
    let mut total_diags = 0;
    for f in &sources {
        total_diags += stir_checker::check_file(&db, ws, *f).len();
    }
    println!(
        "checked in {:?}, {total_diags} diagnostics",
        start.elapsed()
    );
    let mut rng = Rng(0x5eed_1234_abcd_ef01);
    let files_wanted = env_number("ACTIONS_FILES", 300);
    let per_file = env_number("ACTIONS_PLACES", 6);
    let cap = env_number("ACTIONS_CAP", 400);
    let mut picked: Vec<SourceFile> = Vec::new();
    let only = std::env::var("ACTIONS_ONLY").unwrap_or_default();
    let mut pool: Vec<SourceFile> = sources
        .iter()
        .copied()
        .filter(|f| f.path(&db).to_string_lossy().contains(&only))
        .collect();
    while picked.len() < files_wanted && !pool.is_empty() {
        let at = rng.below(pool.len());
        picked.push(pool.swap_remove(at));
    }
    let mut run = Run {
        tally: BTreeMap::new(),
        offers_time: Times::default(),
        warm_time: Times::default(),
        build_time: Times::default(),
        by_tag_time: BTreeMap::new(),
        report: String::new(),
        cap,
        tried: 0,
        max_tried: env_number("ACTIONS_MAX", 20000),
    };
    let all_kinds = Some(vec![
        "quickfix".to_owned(),
        "refactor".to_owned(),
        "source".to_owned(),
    ]);
    let began = Instant::now();
    if let Ok(at) = std::env::var("ACTIONS_RANGE") {
        let (a, b) = at.split_once("..").expect("start..end");
        let range = TextRange::new(
            TextSize::from(a.parse::<u32>().unwrap()),
            TextSize::from(b.parse::<u32>().unwrap()),
        );
        for &file in &picked {
            try_at(&mut db, ws, file, range, all_kinds.clone(), &mut run);
        }
        print!("{}", run.report);
        return;
    }
    for (i, &file) in picked.iter().enumerate() {
        // at each diagnostic
        let diags: Vec<TextRange> = stir_checker::check_file(&db, ws, file)
            .iter()
            .map(|d| d.range)
            .take(12)
            .collect();
        for range in diags {
            try_at(
                &mut db,
                ws,
                file,
                range,
                Some(vec!["quickfix".to_owned()]),
                &mut run,
            );
        }
        let root = stir_db::parse(&db, file).syntax();
        for range in sample_places(&root, &mut rng, per_file) {
            try_at(&mut db, ws, file, range, all_kinds.clone(), &mut run);
        }
        for range in sample_selections(&root, &mut rng, per_file) {
            try_at(&mut db, ws, file, range, all_kinds.clone(), &mut run);
        }
        // the whole-file ones
        try_at(
            &mut db,
            ws,
            file,
            TextRange::empty(TextSize::from(0)),
            Some(vec!["source".to_owned()]),
            &mut run,
        );
        if i % 50 == 49 {
            println!(
                "  {} files, {:?}, {} applied",
                i + 1,
                began.elapsed(),
                run.tried
            );
        }
    }
    println!(
        "{:<28} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8}",
        "action", "offered", "refused", "applied", "failed", "noparse", "newerr"
    );
    let (mut applied, mut no_parse, mut new_errors) = (0, 0, 0);
    for (tag, t) in &run.tally {
        println!(
            "{tag:<28} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8}",
            t.offered, t.refused, t.applied, t.build_failed, t.no_parse, t.new_errors
        );
        applied += t.applied;
        no_parse += t.no_parse;
        new_errors += t.new_errors;
    }
    println!(
        "total: {applied} applied, {no_parse} didn't parse, {new_errors} with new errors, {} panics",
        actions::panics()
    );
    println!("offers, on a checked file: {}", run.offers_time.summary());
    println!(
        "checking the file first, after the last action was undone: {}",
        run.warm_time.summary()
    );
    println!("resolve: {}", run.build_time.summary());
    for (tag, times) in &mut run.by_tag_time {
        if tag.starts_with("signature") || tag.starts_with("extract") || tag.starts_with("imports")
        {
            println!("  resolve {tag}: {}", times.summary());
        }
    }
    if let Some(path) = report_path {
        let name = dir.file_name().unwrap_or_default().to_string_lossy();
        std::fs::write(path.with_extension(format!("{name}.txt")), &run.report).unwrap();
    } else {
        print!("{}", run.report);
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "takes long, and needs the corpus"]
async fn corpus() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus");
    let names = std::env::var("ACTIONS_PROJECTS").unwrap_or_else(|_| "symfony".to_owned());
    let report = std::env::var("ACTIONS_REPORT").ok().map(PathBuf::from);
    for name in names.split(',') {
        run_project(&root.join(name), report.as_deref()).await;
    }
}
