//! Memory over a long editing session, in memory: a project is loaded as the server loads it, then
//! files are edited round after round (a statement in a method body, sometimes a signature), the
//! edited file is checked, other files are hovered and completed in, and every so often the whole
//! project is checked as the server's diagnostics do, and every name in it is looked up as finding
//! references does. What's resident is printed as it goes, and how many values the database holds.
//!
//! With glibc's allocator, what's resident grows for a while even when what's held doesn't, as
//! memory freed by one thread stays with that thread's arena; `MALLOC_ARENA_MAX=1` shows the
//! difference.
//!
//! ```sh
//! MEMORY_PROJECT=/path/to/project MEMORY_ROUNDS=300 MEMORY_REFS_EVERY=50 \
//!   cargo test --release -p stir_lsp --test memory -- --ignored --nocapture session_on_a_project
//! ```
//!
//! The other tests run a shorter session on a small project made up here, and fail if what the
//! database holds grows with the number of edits.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use salsa::Setter as _;
use stir_db::{FileOrigin, RootDatabase, Settings, SourceFile, Workspace};
use stir_project::Project;
use stir_project::workspace::{LoadOptions, builtin_files, load_workspace};
use stir_syntax::TextSize;

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

fn env_number(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Resident and peak resident memory of this process, in MiB.
fn rss() -> (u64, u64) {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let field = |name: &str| {
        status
            .lines()
            .find_map(|l| l.strip_prefix(name))
            .and_then(|v| v.split_whitespace().next()?.parse::<u64>().ok())
            .map_or(0, |kb| kb / 1024)
    };
    (field("VmRSS:"), field("VmHWM:"))
}

/// How many memoized values each query holds and how many of each salsa struct there are, by
/// name (structs have `struct:` in front).
fn counts(db: &mut RootDatabase) -> BTreeMap<String, usize> {
    let info = <dyn salsa::Database>::memory_usage(db);
    let queries = info.queries.iter().map(|(n, q)| (n.to_string(), q.count()));
    let structs = info
        .structs
        .iter()
        .map(|s| (format!("struct:{}", s.debug_name()), s.count()));
    queries.chain(structs).filter(|(_, n)| *n > 0).collect()
}

/// The largest counts, for printing.
fn show_counts(counts: &BTreeMap<String, usize>) -> String {
    let mut big: Vec<_> = counts.iter().filter(|(_, n)| **n >= 100).collect();
    big.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
    big.iter()
        .map(|(name, n)| format!("{name}={n}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Where an edit can go in `text`: just after the `{` of a function's body, and just after the `(`
/// of its parameters.
fn function_places(text: &str) -> Vec<(usize, usize)> {
    let mut places = Vec::new();
    let mut from = 0;
    while let Some(at) = text[from..].find("function ") {
        let start = from + at;
        from = start + "function ".len();
        let Some(open) = text[from..].find('(').map(|i| from + i) else {
            break;
        };
        let Some(close) = text[open..].find(')').map(|i| open + i) else {
            break;
        };
        // a body follows the parameters: not an abstract method, nor a closure's `use`
        let rest = &text[close..];
        let Some(brace) = rest.find(['{', ';']) else {
            break;
        };
        if rest.as_bytes()[brace] == b'{' && !rest[..brace].contains("use") {
            places.push((close + brace + 1, open + 1));
        }
    }
    places
}

/// A file being edited, and its text as loaded.
struct Edited {
    file: SourceFile,
    original: String,
    places: Vec<(usize, usize)>,
}

#[derive(Default)]
struct Times(Vec<Duration>);

impl Times {
    fn add(&mut self, since: Instant) {
        self.0.push(since.elapsed());
    }

    fn summary(&mut self) -> String {
        if self.0.is_empty() {
            return "none".to_owned();
        }
        self.0.sort();
        let at = |p: f64| self.0[((self.0.len() - 1) as f64 * p) as usize];
        format!(
            "n={} p50={:?} p95={:?} max={:?}",
            self.0.len(),
            at(0.5),
            at(0.95),
            self.0[self.0.len() - 1]
        )
    }
}

/// How a session goes.
struct Plan {
    rounds: usize,
    /// Every this many rounds, all sources are checked (0: never).
    full_every: usize,
    /// Every this many rounds, the names all sources use are looked up (0: never).
    refs_every: usize,
    /// How many files are edited in turn.
    editing: usize,
    verbose: bool,
}

/// What a session measured.
struct Session {
    /// Resident MiB after checking everything once, at the end, and at most.
    after_load: u64,
    end: u64,
    peak: u64,
    /// What the database held after the first quarter of the rounds, and at the end.
    early: BTreeMap<String, usize>,
    late: BTreeMap<String, usize>,
}

fn session(db: &mut RootDatabase, ws: Workspace, sources: &[SourceFile], plan: &Plan) -> Session {
    let start = Instant::now();
    for f in sources {
        let _ = stir_checker::check_file(db, ws, *f);
    }
    let after_load = rss().0;
    if plan.verbose {
        println!(
            "checked {} sources in {:?}: rss {} MiB | {}",
            sources.len(),
            start.elapsed(),
            after_load,
            show_counts(&counts(db))
        );
    }
    let mut rng = Rng(0x5eed_0bad_cafe_f00d);
    let mut pool: Vec<Edited> = Vec::new();
    let mut tries = 0;
    while pool.len() < plan.editing && tries < sources.len() * 2 {
        tries += 1;
        let file = sources[rng.below(sources.len())];
        if pool.iter().any(|e| e.file == file) {
            continue;
        }
        let original = file.text(db).clone();
        let places = function_places(&original);
        if !places.is_empty() {
            pool.push(Edited {
                file,
                original,
                places,
            });
        }
    }
    assert!(!pool.is_empty(), "no file has a function to edit");
    let (mut check_time, mut hover_time) = (Times::default(), Times::default());
    let mut complete_time = Times::default();
    let (mut full_time, mut refs_time) = (Times::default(), Times::default());
    let mut peak = after_load;
    let mut early = BTreeMap::new();
    for round in 0..plan.rounds {
        let e = &pool[round % pool.len()];
        let (body, params) = e.places[rng.below(e.places.len())];
        let mut text = e.original.clone();
        // every other visit puts the file back as it was, so what's edited stays the same size
        if (round / pool.len()).is_multiple_of(2) {
            if round % 7 == 6 {
                text.insert_str(params, &format!("int $edited{round} = 0, "));
            } else {
                text.insert_str(body, &format!("\n        $edited{round} = {round} + 1;"));
            }
        }
        e.file.set_text(db).to(text);
        let t = Instant::now();
        let _ = stir_checker::check_file(db, ws, e.file);
        check_time.add(t);
        // what an editor asks of other files meanwhile
        for _ in 0..2 {
            let other = sources[rng.below(sources.len())];
            // where a cursor can be: between characters
            let text = other.text(db);
            let mut at = rng.below(text.len());
            while !text.is_char_boundary(at) {
                at -= 1;
            }
            let at = TextSize::from(at as u32);
            let t = Instant::now();
            let _ = stir_lsp::hover::hover(db, ws, other, at);
            hover_time.add(t);
            let t = Instant::now();
            let _ = stir_lsp::complete::complete(db, ws, other, at);
            complete_time.add(t);
        }
        // what finding references reads: the names every source uses
        if plan.refs_every > 0 && round % plan.refs_every == plan.refs_every - 1 {
            let t = Instant::now();
            for f in sources {
                let _ = stir_lsp::nav::file_refs(db, ws, *f);
            }
            refs_time.add(t);
        }
        if plan.full_every > 0 && round % plan.full_every == plan.full_every - 1 {
            let t = Instant::now();
            for f in sources {
                let _ = stir_checker::check_file(db, ws, *f);
            }
            full_time.add(t);
        }
        if round + 1 == plan.rounds / 4 {
            early = counts(db);
        }
        let (now, hwm) = rss();
        peak = peak.max(hwm);
        if plan.verbose && (round % 10 == 9 || round + 1 == plan.rounds) {
            let shown = if round % 50 == 49 || round + 1 == plan.rounds {
                show_counts(&counts(db))
            } else {
                String::new()
            };
            println!(
                "round {:>4} {:>8.1?}: rss {now} MiB, peak {hwm} MiB {shown}",
                round + 1,
                start.elapsed()
            );
        }
    }
    let late = counts(db);
    for e in &pool {
        e.file.set_text(db).to(e.original.clone());
    }
    if plan.verbose {
        println!("check after an edit: {}", check_time.summary());
        println!("hover: {}", hover_time.summary());
        println!("completion: {}", complete_time.summary());
        println!("whole project: {}", full_time.summary());
        println!("every source's references: {}", refs_time.summary());
    }
    Session {
        after_load,
        end: rss().0,
        peak,
        early,
        late,
    }
}

async fn load(dir: &Path) -> (RootDatabase, Workspace, Vec<SourceFile>) {
    let project = Project::load_or_default(dir).await.expect("the project");
    let loaded = load_workspace(&project, LoadOptions::default())
        .await
        .expect("the workspace");
    (loaded.db, loaded.workspace, loaded.sources)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "takes long, and needs a project"]
async fn session_on_a_project() {
    let dir = std::env::var("MEMORY_PROJECT").expect("MEMORY_PROJECT: the project's folder");
    let started = Instant::now();
    let (mut db, ws, sources) = load(Path::new(&dir)).await;
    println!(
        "{dir}: {} files, {} sources, loaded in {:?}, rss {} MiB",
        ws.files(&db).len(),
        sources.len(),
        started.elapsed(),
        rss().0
    );
    let plan = Plan {
        rounds: env_number("MEMORY_ROUNDS", 300),
        full_every: env_number("MEMORY_FULL_EVERY", 25),
        refs_every: env_number("MEMORY_REFS_EVERY", 0),
        editing: env_number("MEMORY_EDITING", 12),
        verbose: true,
    };
    let s = session(&mut db, ws, &sources, &plan);
    println!(
        "rss after the first check {} MiB, at the end {} MiB, peak {} MiB",
        s.after_load, s.end, s.peak
    );
}

/// Arrays written a part at a time in branches, in a loop: each branch leaves the array another
/// shape, which used to double the union of shapes at each `if` (PHPStan's bug 7903).
const BRANCHY_ARRAYS: &str = r#"<?php
/** @param list<mixed> $rows */
function totals(array $rows): array
{
    $leaf = ['sold' => 0, 'sold_net' => 0, 'left' => 0, 'left_next' => 0];
    $sums = ['sold' => 0, 'sold_eur' => 0];
    $byTeam = [0 => $leaf, 1 => $leaf, 2 => $leaf];
    $byShop = [0 => $sums, 'a_' => $sums, 'b_' => $sums];
    $results = [
        'orders' => ['web' => [0 => $byTeam], 'shop' => [0 => $byTeam], 'all' => [0 => $byTeam]],
        'stock' => ['web' => $byTeam, 'shop' => $byTeam, 'all' => $byTeam],
        'shops' => ['web' => [0 => $byShop, 2 => $byShop], 'shop' => $sums, 'all' => $sums],
    ];
    foreach ($rows as $row) {
        if ((bool)$row['skip']) {
            $results['stock']['all'][0]['left'] += (float)$row['n'];
            continue;
        }
        $id = $row['id'];
        $sold = (float)$row['sold'];
        $net = (float)$row['net'];
        $team = (int)$row['team'];
        $key = (bool)$row['web'] ? 'web' : 'shop';
        if ((bool)$row['order']) {
            $results['orders']['all'][0][0]['sold'] += $sold;
            $results['orders']['all'][0][0]['sold_net'] += $net;
            $results['orders']['all'][0][$id]['sold'] += $sold;
            $results['orders']['all'][0][$id]['sold_net'] += $net;
            $results['orders'][$key][0][0]['sold'] += $sold;
            $results['orders'][$key][0][$id]['sold_net'] += $net;
            $results['orders'][$key][$team][0]['left'] += $sold;
            $results['orders'][$key][$team][$id]['left_next'] = $net;
        } elseif ((bool)$row['stock']) {
            $results['stock']['all'][0]['sold_net'] += $sold;
            $results['stock']['all'][$id]['sold_net'] += $sold;
            $results['stock']['web'][0]['sold_net'] += $sold;
            $results['stock']['web'][$id]['sold_net'] = $sold;
        } else {
            throw new \LogicException('');
        }
    }
    return $results;
}
"#;

/// A small project made up here: classes that use one another, and [`BRANCHY_ARRAYS`].
fn made_up_project() -> (RootDatabase, Workspace, Vec<SourceFile>) {
    let db = RootDatabase::default();
    let mut sources = Vec::new();
    for i in 0..24 {
        let prev = if i == 0 { 23 } else { i - 1 };
        let text = format!(
            r#"<?php
namespace App;

class C{i}
{{
    /** @var list<int> */
    private array $seen = [];

    public function __construct(private ?C{prev} $prev = null) {{}}

    public function step{i}(int $x): int
    {{
        $y = $x + {i};
        $this->seen[] = $y;
        if ($this->prev !== null) {{
            $y += $this->prev->step{prev}($y);
        }}
        return $y;
    }}

    /** @return array{{total: int, names: list<string>}} */
    public function summary(string $name): array
    {{
        $out = ['total' => 0, 'names' => []];
        foreach ($this->seen as $n) {{
            $out['total'] += $n;
            $out['names'][] = $name . $n;
        }}
        return $out;
    }}
}}
"#
        );
        sources.push(SourceFile::new(
            &db,
            PathBuf::from(format!("/project/src/C{i}.php")),
            text,
            FileOrigin::Source,
        ));
    }
    sources.push(SourceFile::new(
        &db,
        PathBuf::from("/project/src/totals.php"),
        BRANCHY_ARRAYS.to_owned(),
        FileOrigin::Source,
    ));
    let mut files = sources.clone();
    files.extend(builtin_files(&db));
    let ws = Workspace::new(&db, files, Settings::default());
    (db, ws, sources)
}

/// A long session holds no more than a short one: what the database keeps is a value per query
/// and file, replaced as files change, not one per edit.
#[test]
fn a_long_session_holds_no_more_than_a_short_one() {
    let (mut db, ws, sources) = made_up_project();
    let plan = Plan {
        rounds: 160,
        full_every: 10,
        refs_every: 40,
        editing: 6,
        verbose: false,
    };
    let s = session(&mut db, ws, &sources, &plan);
    assert!(!s.early.is_empty());
    for (name, late) in &s.late {
        let early = s.early.get(name).copied().unwrap_or(0);
        // what's only asked for after the first quarter, as references are, may come later
        let only_later = name.contains("file_refs");
        assert!(
            *late <= early || (only_later && *late <= sources.len()),
            "{name}: {early} after {} rounds, {late} after {}",
            plan.rounds / 4,
            plan.rounds
        );
    }
}

/// The type of an array written in many branches stays small, rather than a union of every shape
/// it can have.
#[test]
fn arrays_written_in_branches_stay_small() {
    let (db, ws, sources) = made_up_project();
    let file = *sources.last().unwrap();
    let start = Instant::now();
    let _ = stir_checker::check_file(&db, ws, file);
    let at = BRANCHY_ARRAYS.rfind("$results").unwrap();
    let hover = stir_lsp::hover::hover(&db, ws, file, TextSize::from(at as u32 + 1))
        .expect("a type for $results");
    assert!(
        hover.markdown.len() < 100_000,
        "{} bytes, checked in {:?}:\n{}",
        hover.markdown.len(),
        start.elapsed(),
        &hover.markdown[..hover.markdown.len().min(600)]
    );
}
