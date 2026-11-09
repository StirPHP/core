//! What editing a project does to what its templates are given: which queries run again, and what
//! changes for what.

use std::sync::{Arc, Mutex};

use salsa::{Event, EventKind, Setter as _, Storage};
use stir_checker::views::{OpenReason, view_data};
use stir_checker::{check_file, typed_file};
use stir_db::{FileOrigin, ScriptFile, Settings, SourceFile, Workspace};

/// A database that remembers which queries ran.
#[salsa::db]
#[derive(Clone)]
struct LoggingDb {
    storage: Storage<Self>,
    ran: Arc<Mutex<Vec<String>>>,
}

impl LoggingDb {
    fn new() -> Self {
        let ran: Arc<Mutex<Vec<String>>> = Arc::default();
        let log = ran.clone();
        LoggingDb {
            storage: Storage::new(Some(Box::new(move |event: Event| {
                if let EventKind::WillExecute { database_key } = event.kind {
                    log.lock().unwrap().push(format!("{database_key:?}"));
                }
            }))),
            ran,
        }
    }

    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.ran.lock().unwrap())
    }
}

#[salsa::db]
impl salsa::Database for LoggingDb {}

fn ran(log: &[String], query: &str) -> usize {
    log.iter().filter(|q| q.starts_with(query)).count()
}

const LARAVEL: &str = r"<?php
namespace Illuminate\View { class Factory {} }
namespace { function view(string $name, array $data = []): string { return ''; } }
";

const POST: &str =
    "<?php\nnamespace App;\nclass Post { public string $title = ''; public int $views = 0; }\n";

const TWO_VIEWS: &str = "<?php
namespace App;
class TwoViews
{
    public function a(Post $post) { return view('a', ['post' => $post]); }
    public function b(Post $post) { return view('b', ['post' => $post]); }
}
";

const OTHER: &str =
    "<?php\nnamespace App;\nclass Other { public function f(): int { return 1; } }\n";

struct Project {
    db: LoggingDb,
    ws: Workspace,
    files: Vec<SourceFile>,
}

impl Project {
    fn new(sources: &[(&str, &str)]) -> Project {
        let db = LoggingDb::new();
        let mut files = vec![
            SourceFile::new(
                &db,
                "/p/bootstrap/app.php".into(),
                "<?php\n".into(),
                FileOrigin::Source,
            ),
            SourceFile::new(
                &db,
                "/p/vendor/laravel.php".into(),
                LARAVEL.into(),
                FileOrigin::Vendor,
            ),
        ];
        let stubs = stir_stubs::stub_files().map(|s| {
            SourceFile::new(
                &db,
                format!("stubs://{}", s.path).into(),
                s.text.to_owned(),
                FileOrigin::Stub,
            )
        });
        files.extend(stubs.collect::<Vec<_>>());
        let laravel = stir_stubs::frameworks::framework("laravel").unwrap();
        let script = ScriptFile::new(
            &db,
            format!("{}{}", stir_db::BUNDLED_PREFIX, laravel.script_name).into(),
            Ok(laravel.script.to_owned()),
        );
        let ours: Vec<SourceFile> = sources
            .iter()
            .map(|(path, text)| {
                SourceFile::new(
                    &db,
                    format!("/p/{path}").into(),
                    (*text).to_owned(),
                    FileOrigin::Source,
                )
            })
            .collect();
        files.extend(ours.iter().copied());
        let ws = Workspace::builder(files, Settings::default())
            .bundled_scripts(vec![script])
            .new(&db);
        Project {
            db,
            ws,
            files: ours,
        }
    }

    fn check_all(&self) -> Vec<String> {
        for file in &self.files {
            let _ = check_file(&self.db, self.ws, *file);
        }
        self.db.take()
    }

    fn edit(&mut self, index: usize, text: &str) -> Vec<String> {
        let file = self.files[index];
        let _ = file.set_text(&mut self.db).to(text.to_owned());
        self.check_all()
    }

    fn codes(&self, index: usize) -> Vec<String> {
        check_file(&self.db, self.ws, self.files[index])
            .iter()
            .map(|d| d.code.to_string())
            .collect()
    }
}

fn app() -> Project {
    Project::new(&[
        ("app/Post.php", POST),
        ("app/Http/TwoViews.php", TWO_VIEWS),
        ("app/Other.php", OTHER),
        ("resources/views/a.blade.php", "{{ $post->title }}\n"),
        ("resources/views/b.blade.php", "{{ $post->views }}\n"),
    ])
}

const CLASS_COMPONENT: &str = "<?php
namespace App\\View\\Components;
class Alert extends \\Illuminate\\View\\Component
{
    public function __construct(public string $type = '') {}
    public function shout() { return 'a'; }
    public function render() { return view('components.alert'); }
}
";

const COMPONENT_BASE: &str = "<?php
namespace Illuminate\\View;
abstract class Component
{
    public static function resolve($data) { return new static; }
    abstract public function render();
}
";

fn with_components() -> Project {
    Project::new(&[
        ("app/Post.php", POST),
        ("app/Base.php", COMPONENT_BASE),
        ("app/View/Components/Alert.php", CLASS_COMPONENT),
        (
            "app/Http/Page.php",
            "<?php\nnamespace App\\Http;\nclass Page { public function a(\\App\\Post $post) { return view('page', ['post' => $post]); } }\n",
        ),
        (
            "resources/views/page.blade.php",
            "<x-card :post=\"$post\" /><x-alert>{{ $post->title }}</x-alert>\n",
        ),
        (
            "resources/views/components/card.blade.php",
            "@props(['post'])\n{{ $post->title }}\n",
        ),
        (
            "resources/views/components/alert.blade.php",
            "{{ $type }}\n",
        ),
    ])
}

const ALERT_AT: usize = 2;
const PAGE_AT: usize = 4;
const CARD_AT: usize = 5;
const ALERT_VIEW_AT: usize = 6;

#[test]
fn what_a_tag_gives_is_what_its_component_is_checked_with() {
    let mut p = with_components();
    p.check_all();
    assert_eq!(p.codes(CARD_AT), Vec::<String>::new());
    assert!(view_data(&p.db, p.ws, p.files[CARD_AT]).is_closed());
    p.edit(
        PAGE_AT,
        "<x-card :post=\"1\" /><x-alert>{{ $post->title }}</x-alert>\n",
    );
    assert_eq!(p.codes(CARD_AT), ["property.nonObject"]);
}

#[test]
fn an_edit_of_a_components_class_that_changes_nothing_given_rechecks_no_template() {
    let mut p = with_components();
    p.check_all();
    let edited = CLASS_COMPONENT.replace("return 'a';", "return 'b';");
    let log = p.edit(ALERT_AT, &edited);
    assert_eq!(ran(&log, "typed_file"), 1, "{log:?}");
    assert_eq!(ran(&log, "view_data"), 0, "{log:?}");
}

#[test]
fn a_new_public_property_of_a_component_is_there_for_its_template() {
    let mut p = with_components();
    p.check_all();
    let reads = "{{ $type }} {{ $added }}\n";
    p.edit(ALERT_VIEW_AT, reads);
    assert_eq!(p.codes(ALERT_VIEW_AT), ["variable.undefined"]);
    let edited = CLASS_COMPONENT.replace(
        "public function shout()",
        "public string $added = '';\n    public function shout()",
    );
    p.edit(ALERT_AT, &edited);
    assert_eq!(p.codes(ALERT_VIEW_AT), Vec::<String>::new());
}

const TWO_VIEWS_AT: usize = 1;
const OTHER_AT: usize = 2;

#[test]
fn an_edit_that_changes_nothing_given_rechecks_no_template() {
    let mut p = app();
    p.check_all();
    // the calls are in other places now, and in another form
    let moved = TWO_VIEWS.replace("namespace App;", "namespace App;\n\n// moved");
    let log = p.edit(TWO_VIEWS_AT, &moved);
    assert_eq!(ran(&log, "view_calls"), 1, "{log:?}");
    assert_eq!(ran(&log, "view_data"), 0, "{log:?}");
    // the one edited
    assert_eq!(ran(&log, "typed_file"), 1, "{log:?}");
}

#[test]
fn an_edit_of_a_file_that_renders_nothing_recomputes_no_view() {
    let mut p = app();
    p.check_all();
    let log = p.edit(OTHER_AT, &OTHER.replace("return 1", "return 2"));
    assert_eq!(ran(&log, "view_data"), 0, "{log:?}");
    assert_eq!(ran(&log, "view_calls"), 0, "{log:?}");
    assert_eq!(ran(&log, "view_index"), 0, "{log:?}");
    assert_eq!(ran(&log, "typed_file"), 1, "{log:?}");
}

#[test]
fn only_a_template_whose_variables_changed_is_checked_again() {
    let mut p = app();
    p.check_all();
    let changed = TWO_VIEWS.replace(
        "view('a', ['post' => $post])",
        "view('a', ['post' => $post->title])",
    );
    let log = p.edit(TWO_VIEWS_AT, &changed);
    assert_eq!(ran(&log, "view_calls"), 1, "{log:?}");
    // both have to look, being rendered from the same file
    assert_eq!(ran(&log, "view_data"), 2, "{log:?}");
    // that file and `a`, and not `b`
    assert_eq!(ran(&log, "typed_file"), 2, "{log:?}");
    let a = view_data(&p.db, p.ws, p.files[3]);
    assert!(a.vars["post"].ty.to_string().contains("string"), "{a:?}");
}

#[test]
fn a_template_is_open_from_when_a_composer_is_added() {
    let page = "{{ $post->wrong() }}\n{{ $from_composer }}\n";
    let mut p = Project::new(&[
        ("app/Post.php", POST),
        ("app/Http/TwoViews.php", TWO_VIEWS),
        ("app/Providers/Composers.php", "<?php\n"),
        ("resources/views/a.blade.php", page),
        ("resources/views/b.blade.php", "{{ $post->views }}\n"),
    ]);
    p.check_all();
    assert_eq!(view_data(&p.db, p.ws, p.files[3]).open, None);
    assert_eq!(p.codes(3), ["method.notFound", "variable.undefined"]);

    let composer = "<?php\n\\Illuminate\\Support\\Facades\\View::composer('a', fn ($v) => $v);\n";
    p.edit(2, composer);
    assert_eq!(
        view_data(&p.db, p.ws, p.files[3]).open,
        Some(OpenReason::Composer)
    );
    assert_eq!(p.codes(3), Vec::<String>::new());
    // the other one has nothing to do with that
    assert_eq!(view_data(&p.db, p.ws, p.files[4]).open, None);

    p.edit(2, "<?php\n");
    assert_eq!(p.codes(3), ["method.notFound", "variable.undefined"]);
}

#[test]
fn templates_that_include_each_other_are_open_whichever_is_checked_first() {
    let sources = [
        (
            "app/Http/Show.php",
            "<?php\nnamespace App;\nclass Show { public function f() { return view('a', ['n' => 1]); } }\n",
        ),
        (
            "resources/views/a.blade.php",
            "{{ $n->x }}\n@include('b', ['n' => 2])\n",
        ),
        (
            "resources/views/b.blade.php",
            "{{ $n->x }}\n@include('a', ['n' => 3])\n@include('c', ['m' => 4])\n",
        ),
        ("resources/views/c.blade.php", "{{ $m->x }}\n{{ $k }}\n"),
    ];
    for order in [[1, 2, 3], [2, 1, 3], [3, 2, 1]] {
        let p = Project::new(&sources);
        for i in order {
            let _ = typed_file(&p.db, p.ws, p.files[i]);
        }
        let open = |i: usize| view_data(&p.db, p.ws, p.files[i]).open;
        assert_eq!(open(1), Some(OpenReason::Recursive));
        assert_eq!(open(2), Some(OpenReason::Recursive));
        assert_eq!(open(3), Some(OpenReason::ParentOpen));
        for i in [1, 2, 3] {
            assert_eq!(p.codes(i), Vec::<String>::new());
        }
    }
}

#[test]
fn includes_that_fan_out_stay_small() {
    // each includes the next two: 2^40 ways through, with a type for each place
    const DEPTH: usize = 40;
    let mut sources: Vec<(String, String)> = vec![(
        "app/Show.php".to_owned(),
        "<?php\nnamespace App;\nclass Show { public function f() { return view('t0', ['from_show' => 1]); } }\n"
            .to_owned(),
    )];
    for i in 0..DEPTH {
        let text = format!(
            "@php $v{i} = {i}; $both = {i}; @endphp\n@include('t{}')\n@include('t{}', ['extra{i}' => 'a'])\n",
            i + 1,
            i + 2,
        );
        sources.push((format!("resources/views/t{i}.blade.php"), text));
    }
    for i in DEPTH..DEPTH + 2 {
        sources.push((
            format!("resources/views/t{i}.blade.php"),
            "{{ $both }}\n".to_owned(),
        ));
    }
    let refs: Vec<(&str, &str)> = sources
        .iter()
        .map(|(p, t)| (p.as_str(), t.as_str()))
        .collect();
    let p = Project::new(&refs);
    let start = std::time::Instant::now();
    p.check_all();
    assert!(start.elapsed().as_secs() < 30, "{:?}", start.elapsed());
    let last = view_data(&p.db, p.ws, p.files[DEPTH + 2]);
    assert_eq!(last.open, None);
    assert!(last.vars.len() < 100, "{}", last.vars.len());
    assert_eq!(last.vars["both"].ty.to_string(), "int");
}

#[test]
fn past_a_limit_a_union_of_what_is_given_says_nothing() {
    let mut sources: Vec<(String, String)> = Vec::new();
    for i in 0..40 {
        sources.push((
            format!("app/C{i}.php"),
            format!(
                "<?php\nnamespace App;\nclass C{i} {{ public function f() {{ return view('many', ['given' => $this]); }} }}\n"
            ),
        ));
    }
    sources.push((
        "resources/views/many.blade.php".to_owned(),
        "{{ $given->anything() }}\n{{ $not_given }}\n".to_owned(),
    ));
    let refs: Vec<(&str, &str)> = sources
        .iter()
        .map(|(p, t)| (p.as_str(), t.as_str()))
        .collect();
    let p = Project::new(&refs);
    p.check_all();
    let many = view_data(&p.db, p.ws, p.files[40]);
    assert!(many.vars["given"].ty.is_untyped(), "{many:?}");
    assert_eq!(p.codes(40), ["variable.undefined"]);
}
