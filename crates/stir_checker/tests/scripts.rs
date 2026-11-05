//! Scripts in the queries: what they answer, what changing them invalidates, and what a run costs.

use std::sync::{Arc, Mutex};

use salsa::{Event, EventKind, Setter as _, Storage};
use stir_checker::{check_file, script, script_diagnostics, typed_file};
use stir_db::{Db, FileOrigin, ScriptFile, Settings, SourceFile, Workspace};

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

    /// How many times a query with this name ran, since the last time this was asked.
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.ran.lock().unwrap())
    }
}

#[salsa::db]
impl salsa::Database for LoggingDb {}

fn ran(log: &[String], query: &str) -> usize {
    log.iter().filter(|q| q.starts_with(query)).count()
}

struct Setup {
    db: LoggingDb,
    ws: Workspace,
    files: Vec<SourceFile>,
    scripts: Vec<ScriptFile>,
}

impl Setup {
    fn set_script(&mut self, text: impl Into<String>) {
        let _ = self.scripts[0].set_text(&mut self.db).to(Ok(text.into()));
    }
}

fn setup(sources: &[(&str, &str)], scripts: &[&str]) -> Setup {
    let db = LoggingDb::new();
    let files: Vec<SourceFile> = sources
        .iter()
        .map(|(path, text)| {
            SourceFile::new(&db, (*path).into(), (*text).to_owned(), FileOrigin::Source)
        })
        .collect();
    let scripts: Vec<ScriptFile> = scripts
        .iter()
        .enumerate()
        .map(|(i, text)| {
            ScriptFile::new(
                &db,
                format!("/rules/r{i}.rhai").into(),
                Ok((*text).to_owned()),
            )
        })
        .collect();
    let mut all = files.clone();
    all.extend(stir_stubs::stub_files().map(|s| {
        SourceFile::new(
            &db,
            format!("stubs://{}", s.path).into(),
            s.text.to_owned(),
            FileOrigin::Stub,
        )
    }));
    let ws = Workspace::builder(all, Settings::default())
        .scripts(scripts.clone())
        .new(&db);
    Setup {
        db,
        ws,
        files,
        scripts,
    }
}

fn codes(db: &dyn Db, ws: Workspace, file: SourceFile) -> Vec<String> {
    check_file(db, ws, file)
        .iter()
        .map(|d| d.code.to_string())
        .collect()
}

const NO_ECHO: &str = r#"fn visit_echo(node, ctx) { ctx.error("no", "app.noEcho"); }"#;

#[test]
fn only_what_a_rule_subscribes_to_is_visited() {
    let s = setup(
        &[("/a.php", "<?php\necho 1;\necho 2;\nstrlen('a');\n")],
        &[r#"fn visit_echo(node, ctx) { ctx.error(node.kind, "app.kinds"); }"#],
    );
    let diags = check_file(&s.db, s.ws, s.files[0]);
    let messages: Vec<&str> = diags.iter().map(|d| d.message.as_str()).collect();
    assert_eq!(messages, ["echo", "echo"]);
}

#[test]
fn the_node_facade_reads_like_php() {
    let rule = r#"
        fn describe(node) {
            let r = node.range;
            `${node.kind} ${node.name} [${node.text}] ${r.line}:${r.column}`
        }
        fn visit_func_call(node, ctx) {
            ctx.error(describe(node), "app.a");
            for arg in node.arguments { ctx.error(describe(arg), "app.b", #{ node: arg }); }
        }
        fn visit_method_call(node, ctx) {
            ctx.error(describe(node), "app.c");
            ctx.error(describe(node.receiver), "app.d", #{ node: node.receiver });
        }
        fn visit_static_call(node, ctx) {
            ctx.error(describe(node), "app.e");
            ctx.error(describe(node.receiver), "app.f", #{ node: node.receiver });
        }
        fn visit_property_fetch(node, ctx) { ctx.error(describe(node), "app.g"); }
        fn visit_class(node, ctx) {
            let kinds = node.children.map(|c| c.kind);
            ctx.error(`${describe_short(node)} ${kinds}`, "app.h");
        }
        fn describe_short(node) { `${node.kind} ${node.name}` }
        fn visit_method(node, ctx) { ctx.error(describe_short(node), "app.i"); }
        fn visit_property(node, ctx) { ctx.error(describe_short(node), "app.j"); }
        fn visit_new(node, ctx) { ctx.error(describe_short(node), "app.k"); }
        fn visit_variable(node, ctx) { ctx.error(describe_short(node), "app.l"); }
    "#;
    let php = "<?php\n\\foo\\bar($x, y: 1);\n$o->m();\n(new K())->p;\n\\A\\B::s();\nclass K {\n    public int $p = 1;\n    function m(): void {}\n}\n";
    let s = setup(&[("/a.php", php)], &[rule]);
    let diags = check_file(&s.db, s.ws, s.files[0]);
    let by_code = |code: &str| -> Vec<String> {
        diags
            .iter()
            .filter(|d| d.code == code)
            .map(|d| d.message.clone())
            .collect()
    };
    assert_eq!(
        by_code("app.a"),
        [r"func_call foo\bar [\foo\bar($x, y: 1)] 2:1"]
    );
    assert_eq!(
        by_code("app.b"),
        ["variable x [$x] 2:10", "literal  [1] 2:17"]
    );
    assert_eq!(by_code("app.c"), ["method_call m [$o->m()] 3:1"]);
    assert_eq!(by_code("app.d"), ["variable o [$o] 3:1"]);
    assert_eq!(by_code("app.e"), [r"static_call s [\A\B::s()] 5:1"]);
    assert_eq!(by_code("app.f"), [r"name A\B [\A\B] 5:1"]);
    // the `$o->m` in `$o->m()` isn't a fetch
    assert_eq!(by_code("app.g"), ["property_fetch p [(new K())->p] 4:1"]);
    assert_eq!(by_code("app.h"), [r#"class K ["property", "method"]"#]);
    assert_eq!(by_code("app.i"), ["method m"]);
    assert_eq!(by_code("app.j"), ["property p"]);
    assert_eq!(by_code("app.k"), ["new K"]);
    assert_eq!(by_code("app.l"), ["variable x", "variable o"]);
}

#[test]
fn identifiers_have_to_look_like_identifiers() {
    for bad in ["noDot", "a..b", "a.b c", ".a", "SPHP0001"] {
        let rule = format!(r#"fn visit_echo(node, ctx) {{ ctx.error("m", "{bad}"); }}"#);
        let s = setup(&[("/a.php", "<?php\necho 1;\n")], &[&rule]);
        assert_eq!(
            codes(&s.db, s.ws, s.files[0]),
            Vec::<String>::new(),
            "{bad}"
        );
        let d = script_diagnostics(&s.db, s.ws, &s.files);
        assert!(d[0].message.contains("isn't an identifier"), "{d:?}");
    }
}

#[test]
fn a_rule_reports_nothing_more_than_a_cap_allows() {
    let s = setup(
        &[("/a.php", "<?php\necho 1;\n")],
        &[r#"fn visit_echo(node, ctx) { loop { ctx.error("m", "app.m"); } }"#],
    );
    let d = script_diagnostics(&s.db, s.ws, &s.files);
    assert!(d[0].message.contains("more than"), "{d:?}");
}

#[test]
fn changing_a_script_changes_what_it_found_and_nothing_else_runs() {
    let mut s = setup(
        &[
            ("/a.php", "<?php\necho 1;\n"),
            ("/b.php", "<?php\necho 2;\n"),
        ],
        &[NO_ECHO],
    );
    assert_eq!(codes(&s.db, s.ws, s.files[0]), ["app.noEcho"]);
    assert_eq!(codes(&s.db, s.ws, s.files[1]), ["app.noEcho"]);
    s.db.take();

    // the same rule says something else: rules run again, type checks don't
    let renamed = r#"fn visit_echo(node, ctx) { ctx.error("no", "app.other"); }"#;
    s.set_script(renamed);
    assert_eq!(codes(&s.db, s.ws, s.files[0]), ["app.other"]);
    assert_eq!(codes(&s.db, s.ws, s.files[1]), ["app.other"]);
    let log = s.db.take();
    assert_eq!(ran(&log, "rule_check"), 2, "{log:?}");
    assert_eq!(ran(&log, "typed_file"), 0, "{log:?}");

    // now there's a new kind of node: the index changes, and it's still the rules that run
    let other_kind = r#"fn visit_func_call(node, ctx) { ctx.error("no", "app.call"); }"#;
    s.set_script(other_kind);
    assert_eq!(codes(&s.db, s.ws, s.files[0]), Vec::<String>::new());
    assert_eq!(ran(&s.db.take(), "rule_check"), 1);
}

#[test]
fn an_edit_runs_the_rules_for_the_file_edited() {
    let mut s = setup(
        &[
            ("/a.php", "<?php\necho 1;\n"),
            ("/b.php", "<?php\necho 2;\n"),
        ],
        &[NO_ECHO],
    );
    codes(&s.db, s.ws, s.files[0]);
    codes(&s.db, s.ws, s.files[1]);
    s.db.take();
    s.files[0].set_text(&mut s.db).to("<?php\n".to_owned());
    assert_eq!(codes(&s.db, s.ws, s.files[0]), Vec::<String>::new());
    assert_eq!(codes(&s.db, s.ws, s.files[1]), ["app.noEcho"]);
    let log = s.db.take();
    assert_eq!(ran(&log, "rule_check"), 1, "{log:?}");
}

#[test]
fn no_scripts_nothing_runs() {
    let s = setup(&[("/a.php", "<?php\necho 1;\n")], &[]);
    assert!(codes(&s.db, s.ws, s.files[0]).is_empty());
    let log = s.db.take();
    assert_eq!(ran(&log, "compiled_script"), 0, "{log:?}");
    assert!(script_diagnostics(&s.db, s.ws, &s.files).is_empty());
}

const HOOK: &str = r#"
    fn return_type_targets() { #{ functions: ["pick"] } }
    fn return_type_of_function(node, ctx) { "list<int>" }
"#;

#[test]
fn changing_a_hook_changes_the_type_and_a_new_answer_rechecks() {
    let php = "<?php\nfunction pick(): mixed {}\n\npick();\n";
    assert_eq!(&php[33..37], "pick");
    let hook_at = |s: &Setup| {
        let typed = typed_file(&s.db, s.ws, s.files[0]);
        let t = typed.types.iter().find(|(r, _)| u32::from(r.start()) == 33);
        t.map(|(_, t)| t.to_string())
    };
    let mut s = setup(&[("/a.php", php)], &[HOOK]);
    assert_eq!(hook_at(&s).as_deref(), Some("list<int>"));
    let other = HOOK.replace("list<int>", "string");
    s.set_script(other);
    assert_eq!(hook_at(&s).as_deref(), Some("string"));
    // gone: the declared type's back
    s.set_script("fn nothing() {}");
    assert_eq!(hook_at(&s).as_deref(), Some("mixed"));
}

#[test]
fn a_rule_only_edit_leaves_hooks_alone() {
    let script = |id: &str| {
        format!(
            r#"{HOOK}
            fn visit_echo(node, ctx) {{ ctx.error("no", "{id}"); }}"#
        )
    };
    let mut s = setup(&[("/a.php", "<?php\necho 1;\n")], &[&script("app.a")]);
    assert_eq!(codes(&s.db, s.ws, s.files[0]), ["app.a"]);
    s.db.take();
    // hooks may have changed with the text, so what uses them has to see, and sees it can stay
    s.set_script(script("app.b"));
    assert_eq!(codes(&s.db, s.ws, s.files[0]), ["app.b"]);
    let log = s.db.take();
    assert_eq!(ran(&log, "rule_check"), 1, "{log:?}");
    assert_eq!(ran(&log, "typed_file"), 0, "{log:?}");
}

#[test]
fn the_same_input_gets_the_same_answer() {
    let rule = r#"
        fn visit_func_call(node, ctx) {
            let t = ctx.get_type(node);
            let m = #{ b: 1, a: 2, c: 3 };
            ctx.error(`${t.to_string()} ${m.keys()} ${node.range.line}`, "app.same");
        }
    "#;
    let php = "<?php\nstrlen('a');\nstrtolower('b');\ncount([]);\n";
    let run = || {
        let s = setup(&[("/a.php", php)], &[rule, HOOK]);
        check_file(&s.db, s.ws, s.files[0]).as_ref().clone()
    };
    let first = run();
    assert_eq!(first.len(), 3);
    for _ in 0..5 {
        assert_eq!(run(), first);
    }
}

#[test]
fn a_script_that_cannot_be_read_says_so_at_the_script() {
    let db = LoggingDb::new();
    let file = SourceFile::new(
        &db,
        "/a.php".into(),
        "<?php\n".to_owned(),
        FileOrigin::Source,
    );
    let script = ScriptFile::new(
        &db,
        "/gone.rhai".into(),
        Err("it can't be read: nope".to_owned()),
    );
    let ws = Workspace::builder(vec![file], Settings::default())
        .scripts(vec![script])
        .new(&db);
    assert!(check_file(&db, ws, file).is_empty());
    let d = script_diagnostics(&db, ws, &[file]);
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].code, "SPHP0071");
    assert_eq!(d[0].file.to_string_lossy(), "/gone.rhai");
    assert!(d[0].message.contains("nope"));
}

#[test]
fn failing_on_many_files_is_said_once() {
    let s = setup(
        &[
            ("/a.php", "<?php\necho 1;\n"),
            ("/b.php", "<?php\necho 1;\n"),
            ("/c.php", "<?php\n// no echo\n"),
        ],
        &[r#"fn visit_echo(node, ctx) { let a = []; a[2] = 1; }"#],
    );
    let d = script_diagnostics(&s.db, s.ws, &s.files);
    assert_eq!(d.len(), 1, "{d:?}");
    assert_eq!(d[0].code, "SPHP0070");
    assert_eq!(d[0].file.to_string_lossy(), "/rules/r0.rhai");
    assert!(d[0].message.contains("a.php"), "{d:?}");
    assert!(d[0].notes[0].contains("2 files"), "{d:?}");
    // and an infinite loop on every node of a file costs one run of the cap: the rest are skipped
    let many = format!("<?php\n{}", "echo 1;\n".repeat(1000));
    let s = setup(
        &[("/a.php", &many)],
        &[r#"fn visit_echo(node, ctx) { loop {} }"#],
    );
    let start = std::time::Instant::now();
    let d = script_diagnostics(&s.db, s.ws, &s.files);
    assert_eq!(d.len(), 1);
    assert!(start.elapsed().as_secs() < 60);
}

const MAGIC: &str = r#"
    fn magic_members(class_name) {
        if class_name != "App\\Model" { return; }
        #{
            properties: #{
                title: "string",
                id: #{ type: "int", read_only: true },
                hidden: "App\\Secret",
            },
            methods: #{
                touch: "void",
                find: #{ returns: "?static", params: "int $id", is_static: true },
                declared: "int",
            },
        }
    }
"#;

const MAGIC_PHP: &str = r#"<?php
declare(strict_types=1);
namespace App;
class Secret {}
class Model { public function declared(): string { return ''; } }
class Post extends Model {}
function wants_int(int $n): void {}
function run(Post $post, Secret $s): void {
    wants_int($post->title);
    wants_int($post->id);
    $post->id = 2;
    $post->touch(1, 2);
    $found = Post::find(1);
    $found->nope();
    Post::find('x');
    wants_int($post->declared());
    $post->missing;
    $s->title;
}
"#;

#[test]
fn magic_members_are_on_a_class_and_its_subclasses() {
    let s = setup(&[("/a.php", MAGIC_PHP)], &[MAGIC]);
    let ws = s.ws;
    let settings = ws.settings(&s.db).clone();
    let mut db = s.db;
    ws.set_settings(&mut db).to(Settings {
        php_level: 8,
        ..settings
    });
    let diags = check_file(&db, ws, s.files[0]);
    let found: Vec<String> = diags
        .iter()
        .map(|d| {
            let line = MAGIC_PHP[..usize::from(d.range.start())]
                .matches('\n')
                .count()
                + 1;
            format!("{line} {}", d.code)
        })
        .collect();
    assert_eq!(
        found,
        [
            "9 argument.type",            // a string
            "11 assign.propertyReadOnly", // read-only
            "14 method.nonObject",        // ?static: null
            "14 method.notFound",         // no `nope`
            "15 argument.type",           // takes an int
            "16 argument.type",           // what's declared wins: a string
            "17 property.notFound",
            "18 property.notFound", // not on any other class
        ],
        "{diags:#?}"
    );
    assert!(script_diagnostics(&db, ws, &s.files).is_empty());
}

#[test]
fn what_magic_members_answer_is_checked() {
    for (answer, says) in [
        ("1", "isn't a map"),
        ("#{ properties: 1 }", "isn't a map"),
        (r#"#{ properties: #{ a: "list<" } }"#, "isn't a type"),
        (r#"#{ properties: #{ "a b": "int" } }"#, "isn't a name"),
        (r#"#{ methods: #{ a: "int*/" } }"#, "can't be part"),
        ("#{ methods: #{ a: #{} } }", "no `returns`"),
        (r#"#{ methods: #{ a: 1 } }"#, "should be a string"),
    ] {
        let script = format!("fn magic_members(class_name) {{ {answer} }}");
        let php = "<?php\nclass A {}\n(new A())->b();\n";
        let s = setup(&[("/a.php", php)], &[&script]);
        let d = script_diagnostics(&s.db, s.ws, &s.files);
        assert_eq!(d.len(), 1, "{answer}: {d:?}");
        assert_eq!(d[0].code, "SPHP0070", "{answer}");
        assert!(d[0].message.contains(says), "{answer}: {d:?}");
    }
}

#[test]
fn a_hook_or_magic_that_never_ends_leaves_the_analysis_going() {
    let s = setup(
        &[("/a.php", "<?php\nclass A {}\n(new A())->b();\nmissing();\n")],
        &[r#"fn magic_members(class_name) { loop {} }"#],
    );
    assert!(codes(&s.db, s.ws, s.files[0]).contains(&"function.notFound".to_owned()));
    let d = script_diagnostics(&s.db, s.ws, &s.files);
    assert_eq!(d.len(), 1, "{d:?}");
    assert!(d[0].message.contains("operations"), "{d:?}");
}

#[test]
fn the_type_facade_answers() {
    let rule = r#"
        fn visit_method_call(node, ctx) {
            let t = ctx.get_type(node.receiver);
            let members = t.members().map(|m| `${m.kind} ${m.name}: ${m.type}`);
            ctx.error(
                `${t.to_string()}|${t.class_name}|${t.is_nullable()}|${t.is_null()}|${t.is_object()}|${t.is_string()}|${t.is_subtype_of("App\\Base")}|${t.is_subtype_of("App\\Nope")}|${t.has_method("run")}|${t.has_method("nah")}|${t.has_property("p")}|${t.without_null().to_string()}|${members}`,
                "app.t",
            );
        }
        fn visit_func_call(node, ctx) {
            if node.name != "show" { return; }
            let t = ctx.get_type(node.arguments[0]);
            ctx.error(
                `${t.to_string()}|${t.literal}|${t.class_string}|${t.is_string()}|${t.is_int()}|${t.is_float()}|${t.is_bool()}|${t.is_array()}|${t.is_unknown()}`,
                "app.v",
            );
        }
    "#;
    let php = r#"<?php
namespace App;
function show(mixed $x): void {}
class Base { public int $p = 1; public function run(): int { return 1; } }
class Kid extends Base {}
function go(?Kid $k, array $a, $u): void {
    $k?->run();
    show('a');
    show(1);
    show(1.5);
    show(true);
    show(Kid::class);
    show([1]);
    show($u);
}
"#;
    let s = setup(&[("/a.php", php)], &[rule]);
    let diags = check_file(&s.db, s.ws, s.files[0]);
    let of = |code: &str| -> Vec<String> {
        diags
            .iter()
            .filter(|d| d.code == code)
            .map(|d| d.message.clone())
            .collect()
    };
    assert_eq!(
        of("app.t"),
        [
            r#"?App\Kid|App\Kid|true|false|true|false|false|false|true|false|true|App\Kid|["method run: int", "property p: int"]"#
        ]
    );
    assert_eq!(
        of("app.v"),
        [
            "'a'|a||true|false|false|false|false|false",
            "1|1||false|true|false|false|false|false",
            "1.5|1.5||false|false|true|false|false|false",
            "true|true||false|false|false|true|false|false",
            r"class-string<App\Kid>||App\Kid|true|false|false|false|false|false",
            "array{1}|||false|false|false|false|true|false",
            "mixed|||false|false|false|false|false|true",
        ]
    );
}

#[test]
fn queries_stay_reachable_from_other_threads() {
    // what's shared between files on a parallel run
    let s = setup(
        &[
            ("/a.php", "<?php\necho 1;\n"),
            ("/b.php", "<?php\necho 2;\n"),
        ],
        &[NO_ECHO],
    );
    let threads: Vec<_> = s
        .files
        .iter()
        .map(|&f| {
            let (db, ws) = (s.db.clone(), s.ws);
            std::thread::spawn(move || codes(&db, ws, f))
        })
        .collect();
    for t in threads {
        assert_eq!(t.join().unwrap(), ["app.noEcho"]);
    }
    let _ = script::script_index(&s.db, s.ws);
}
