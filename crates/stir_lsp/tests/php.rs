//! Editor features with the cursor in a plain PHP file. `<|>` marks the cursor;
//! `//- /path` starts another file.

use stir_db::{FileOrigin, RootDatabase, Settings, SourceFile, Workspace};
use stir_lsp::{complete, hierarchy, hints, hover, nav, symbols, tokens};
use stir_syntax::{TextRange, TextSize};

struct Fixture {
    db: RootDatabase,
    ws: Workspace,
    files: Vec<SourceFile>,
    /// The file and offset of `<|>`.
    cursor: Option<(SourceFile, TextSize)>,
}

impl Fixture {
    fn new(text: &str) -> Fixture {
        let db = RootDatabase::default();
        let mut sources = Vec::new();
        let mut cursor = None;
        let mut chunks: Vec<(String, String)> = Vec::new();
        if text.trim_start().starts_with("//- ") {
            for part in text.split("//- ").filter(|p| !p.trim().is_empty()) {
                let (path, body) = part.split_once('\n').unwrap();
                chunks.push((path.trim().to_owned(), body.to_owned()));
            }
        } else {
            chunks.push(("/src/main.php".to_owned(), text.to_owned()));
        }
        for (path, body) in chunks {
            let at = body.find("<|>");
            let body = body.replace("<|>", "");
            let file = SourceFile::new(&db, path.into(), body, FileOrigin::Source);
            if let Some(at) = at {
                cursor = Some((file, TextSize::from(at as u32)));
            }
            sources.push(file);
        }
        let mut files = sources.clone();
        files.extend(stir_project::workspace::builtin_files(&db));
        let ws = Workspace::new(&db, files, Settings::default());
        Fixture {
            db,
            ws,
            files: sources,
            cursor,
        }
    }

    fn at(&self) -> (SourceFile, TextSize) {
        self.cursor.expect("no <|> cursor")
    }

    fn text(&self, file: SourceFile, range: TextRange) -> &str {
        &file.text(&self.db)[range]
    }

    fn hover(&self) -> String {
        let (file, offset) = self.at();
        hover::hover(&self.db, self.ws, file, offset)
            .map(|h| h.markdown)
            .unwrap_or_default()
    }

    /// `path:text-of-target-name`
    fn definition(&self) -> String {
        let (file, offset) = self.at();
        let Some((_, def)) = nav::def_at(&self.db, self.ws, file, offset) else {
            return "<none>".into();
        };
        let t = nav::target(&self.db, self.ws, file, &def).expect("no target");
        format!(
            "{}:{}",
            t.file.path(&self.db).display(),
            self.text(t.file, t.focus_range)
        )
    }

    fn references(&self) -> Vec<String> {
        let (file, offset) = self.at();
        let (_, def) = nav::def_at(&self.db, self.ws, file, offset).expect("nothing here");
        let mut out: Vec<String> = nav::references(&self.db, self.ws, file, &def)
            .into_iter()
            .map(|(f, r)| {
                let text = f.text(&self.db);
                let line = text[..usize::from(r.start())].matches('\n').count() + 1;
                format!("{}:{line}:{}", f.path(&self.db).display(), &text[r])
            })
            .collect();
        out.sort();
        out
    }

    fn nav_list(&self, targets: Vec<nav::NavTarget>) -> Vec<String> {
        let mut out: Vec<String> = targets
            .into_iter()
            .map(|t| {
                let name = t.file.path(&self.db).display().to_string();
                let line = t.file.text(&self.db)[..usize::from(t.focus_range.start())]
                    .matches('\n')
                    .count()
                    + 1;
                format!("{name}:{line}:{}", self.text(t.file, t.focus_range))
            })
            .collect();
        out.sort();
        out
    }

    fn implementations(&self) -> Vec<String> {
        let (file, offset) = self.at();
        let (_, def) = nav::def_at(&self.db, self.ws, file, offset).expect("nothing here");
        self.nav_list(hierarchy::implementations(&self.db, self.ws, file, &def))
    }

    fn declarations(&self) -> Vec<String> {
        let (file, offset) = self.at();
        let (_, def) = nav::def_at(&self.db, self.ws, file, offset).expect("nothing here");
        self.nav_list(hierarchy::declarations(&self.db, self.ws, file, &def))
    }

    fn completions(&self) -> Vec<String> {
        let (file, offset) = self.at();
        complete::complete(&self.db, self.ws, file, offset)
            .items
            .into_iter()
            .map(|i| i.label)
            .collect()
    }
}

fn has(list: &[String], want: &str) -> bool {
    list.iter().any(|l| l == want)
}

const USER: &str = "<?php\nnamespace App;\n/** A user. */\nclass User {\n    public string $name = '';\n    private int $secret = 0;\n    public const LIMIT = 3;\n    /** Says hello. */\n    public function greet(string $to, int $times = 1): string { return $to; }\n    public static function make(): self { return new self(); }\n}\n";

// ------------------------------------------------------------------ hover

#[test]
fn hover_shows_inferred_local_types() {
    let f = Fixture::new("<?php\n$count = 1;\necho $cou<|>nt;\n");
    assert!(f.hover().contains("int"), "{}", f.hover());
    let f = Fixture::new("<?php\n$label = 'x';\necho $lab<|>el;\n");
    assert!(f.hover().contains("string"), "{}", f.hover());
}

#[test]
fn hover_reads_like_php() {
    let f = Fixture::new("<?php\n$count = 1;\necho $cou<|>nt;\n");
    assert_eq!(f.hover(), "```php\nint $count\n```");
    let f = Fixture::new("<?php\nfunction f(?string $name): void { echo $na<|>me; }\n");
    assert_eq!(f.hover(), "```php\n?string $name\n```");
    let f = Fixture::new(&format!("{USER}$u = new User();\necho $u->na<|>me;\n"));
    assert!(f.hover().contains("public string $name"), "{}", f.hover());
    let f = Fixture::new(&format!(
        "{USER}$u = new User();\necho $u->gre<|>et('a');\n"
    ));
    assert!(
        f.hover()
            .contains("function greet(string $to, int $times = …): string"),
        "{}",
        f.hover()
    );
}

#[test]
fn hover_follows_narrowing() {
    let f = Fixture::new(
        "<?php\nfunction f(?string $name): void {\n    if ($name !== null) { echo $na<|>me; }\n}\n",
    );
    let h = f.hover();
    assert!(h.contains("string") && !h.contains("?string"), "{h}");
    let f = Fixture::new("<?php\nfunction f(?string $name): void {\n    echo $na<|>me;\n}\n");
    assert!(f.hover().contains("?string"), "{}", f.hover());
}

#[test]
fn hover_function_with_doc() {
    let f = Fixture::new(
        "<?php\nnamespace App;\n/** Doubles a number. */\nfunction double(int $x): int { return $x * 2; }\necho dou<|>ble(2);\n",
    );
    let h = f.hover();
    assert!(h.contains("function double"), "{h}");
    assert!(h.contains("Doubles a number."), "{h}");
}

#[test]
fn hover_class_method_and_property() {
    let f = Fixture::new(&format!("{USER}$u = new User();\necho $u->na<|>me;\n"));
    assert!(f.hover().contains("name"), "{}", f.hover());
    let f = Fixture::new(&format!(
        "{USER}$u = new User();\necho $u->gre<|>et('a');\n"
    ));
    let h = f.hover();
    assert!(h.contains("function greet"), "{h}");
    assert!(h.contains("Says hello."), "{h}");
    let f = Fixture::new(&format!("{USER}function g(Us<|>er $u): void {{}}\n"));
    let h = f.hover();
    assert!(h.contains("class User"), "{h}");
    assert!(h.contains("A user."), "{h}");
}

#[test]
fn hover_uses_phpdoc_types() {
    let f = Fixture::new(
        "<?php\n/**\n * @param list<int> $xs\n */\nfunction f(array $xs): void {\n    echo count($x<|>s);\n}\n",
    );
    assert!(f.hover().contains("list<int>"), "{}", f.hover());
    let f = Fixture::new(
        "<?php\n/**\n * @template T\n * @param T $v\n * @return T\n */\nfunction same(mixed $v): mixed { return $v; }\n$n = same(1);\necho $<|>n;\n",
    );
    assert!(
        f.hover().contains('1') || f.hover().contains("int"),
        "{}",
        f.hover()
    );
}

#[test]
fn hover_builtin_from_stubs() {
    let f = Fixture::new("<?php\necho str<|>len('a');\n");
    let h = f.hover();
    assert!(h.contains("function strlen"), "{h}");
}

// ------------------------------------------------------------------ navigation

#[test]
fn definition_of_locals_params_and_functions() {
    let f = Fixture::new("<?php\n$count = 1;\necho $cou<|>nt;\n");
    assert_eq!(f.definition(), "/src/main.php:$count");
    let f = Fixture::new("<?php\nfunction f(int $times): int {\n    return $tim<|>es;\n}\n");
    assert_eq!(f.definition(), "/src/main.php:$times");
    let f = Fixture::new("<?php\nfunction helper(): int { return 1; }\necho help<|>er();\n");
    assert_eq!(f.definition(), "/src/main.php:helper");
}

#[test]
fn definition_across_files() {
    let f = Fixture::new(
        "//- /src/User.php\n<?php\nnamespace App;\nclass User { public function greet(): string { return ''; } }\nfunction helper(): int { return 1; }\n//- /src/main.php\n<?php\nnamespace App;\nfunction g(Us<|>er $u): void {}\n",
    );
    assert_eq!(f.definition(), "/src/User.php:User");
    let f = Fixture::new(
        "//- /src/User.php\n<?php\nnamespace App;\nclass User { public function greet(): string { return ''; } }\n//- /src/main.php\n<?php\nuse App\\User;\n$u = new User();\necho $u->gre<|>et();\n",
    );
    assert_eq!(f.definition(), "/src/User.php:greet");
    let f = Fixture::new(
        "//- /src/lib.php\n<?php\nnamespace App;\nfunction helper(): int { return 1; }\n//- /src/main.php\n<?php\nnamespace App;\necho help<|>er();\n",
    );
    assert_eq!(f.definition(), "/src/lib.php:helper");
}

#[test]
fn definition_of_members() {
    let f = Fixture::new(&format!("{USER}$u = new User();\necho $u->na<|>me;\n"));
    assert_eq!(f.definition(), "/src/main.php:$name");
    let f = Fixture::new(&format!("{USER}echo User::LIM<|>IT;\n"));
    assert_eq!(f.definition(), "/src/main.php:LIMIT");
    let f = Fixture::new(&format!("{USER}User::ma<|>ke();\n"));
    assert_eq!(f.definition(), "/src/main.php:make");
    let f = Fixture::new(
        "<?php\nclass A {\n    public int $n = 0;\n    public function m(): int { return $this-><|>n; }\n}\n",
    );
    assert_eq!(f.definition(), "/src/main.php:$n");
}

#[test]
fn definition_into_stubs() {
    let f = Fixture::new("<?php\necho str<|>len('a');\n");
    assert!(f.definition().starts_with("stubs://"), "{}", f.definition());
}

#[test]
fn references_across_files() {
    let f = Fixture::new(
        "//- /src/lib.php\n<?php\nnamespace App;\nfunction help<|>er(): int { return 1; }\n//- /src/main.php\n<?php\nnamespace App;\necho helper();\necho helper() + helper();\n",
    );
    assert_eq!(
        f.references(),
        [
            "/src/lib.php:3:helper",
            "/src/main.php:3:helper",
            "/src/main.php:4:helper",
            "/src/main.php:4:helper"
        ]
    );
}

#[test]
fn references_of_a_local_stay_in_its_function() {
    let f = Fixture::new(
        "<?php\nfunction a(): void {\n    $n = 0;\n    $n += 1;\n    echo $<|>n;\n}\nfunction b(): void {\n    $n = 5;\n    echo $n;\n}\n",
    );
    assert_eq!(
        f.references(),
        [
            "/src/main.php:3:$n",
            "/src/main.php:4:$n",
            "/src/main.php:5:$n"
        ]
    );
}

#[test]
fn references_of_a_class_in_types_and_expressions() {
    let f = Fixture::new(
        "<?php\nclass Us<|>er {}\nfunction make(): User { return new User(); }\n/** @param User $v */\nfunction f($v): bool { return $v instanceof User; }\n",
    );
    let refs = f.references();
    assert!(refs.len() >= 4, "{refs:?}");
}

#[test]
fn project_defs_are_renameable_and_builtins_are_not() {
    let f = Fixture::new("<?php\necho str<|>len('a');\n");
    let (file, offset) = f.at();
    let (_, def) = nav::def_at(&f.db, f.ws, file, offset).unwrap();
    assert!(!hover::is_project_def(&f.db, f.ws, file, &def));
    let f = Fixture::new("<?php\n$co<|>unt = 1;\n");
    let (file, offset) = f.at();
    let (_, def) = nav::def_at(&f.db, f.ws, file, offset).unwrap();
    assert!(hover::is_project_def(&f.db, f.ws, file, &def));
}

// ------------------------------------------------------------------ completion

#[test]
fn completes_members() {
    let f = Fixture::new(&format!("{USER}$u = new User();\n$u-><|>\n"));
    let items = f.completions();
    assert!(has(&items, "greet"), "{items:?}");
    assert!(has(&items, "name"), "{items:?}");
    assert!(!has(&items, "secret"), "{items:?}");
}

#[test]
fn completes_this_members_including_private() {
    let f = Fixture::new(
        "<?php\nclass A {\n    private int $secret = 0;\n    public function m(): int { return $this-><|>; }\n}\n",
    );
    assert!(has(&f.completions(), "secret"), "{:?}", f.completions());
}

#[test]
fn completes_locals_in_scope() {
    let f = Fixture::new(
        "<?php\n$outer = 1;\nfunction f(int $times): void {\n    $inner = 2;\n    echo $<|>;\n}\n",
    );
    let items = f.completions();
    for want in ["$inner", "$times"] {
        assert!(has(&items, want), "{want} missing: {items:?}");
    }
    assert!(!has(&items, "$outer"), "{items:?}");
}

#[test]
fn completes_globals_by_prefix() {
    let f = Fixture::new(
        "<?php\nnamespace App;\nfunction helperOne(): int { return 1; }\necho helpe<|>;\n",
    );
    assert!(has(&f.completions(), "helperOne"), "{:?}", f.completions());
    let f = Fixture::new("<?php\necho str_repl<|>;\n");
    assert!(has(&f.completions(), "str_replace"));
}

#[test]
fn completes_static_members() {
    let f = Fixture::new(&format!("{USER}User::<|>\n"));
    let items = f.completions();
    assert!(has(&items, "make") && has(&items, "LIMIT"), "{items:?}");
    let f = Fixture::new("<?php\nenum Color { case Red; case Green; }\necho Color::<|>;\n");
    let items = f.completions();
    assert!(has(&items, "Red") && has(&items, "Green"), "{items:?}");
}

#[test]
fn completes_php_keywords() {
    let f = Fixture::new("<?php\nfunction f(): void {\n    <|>\n}\n");
    let items = f.completions();
    assert!(has(&items, "foreach"), "{items:?}");
    assert!(!has(&items, "val"), "{items:?}");
}

// ------------------------------------------------------------------ hints

fn hint_labels(text: &str) -> Vec<String> {
    let f = Fixture::new(text);
    let file = f.files[0];
    let all = TextRange::up_to(TextSize::of(file.text(&f.db).as_str()));
    hints::inlay_hints(&f.db, f.ws, file, all)
        .into_iter()
        .map(|h| {
            let name_end = usize::from(h.offset);
            let start = file.text(&f.db)[..name_end].rfind('$').unwrap();
            format!("{}{}", &file.text(&f.db)[start..name_end], h.label)
        })
        .collect()
}

#[test]
fn inlay_hints_show_inferred_variable_types() {
    assert_eq!(
        hint_labels("<?php\n$a = 1;\n$b = 'x';\n$a = 2;\n$c = [1, 2];\n"),
        ["$a: int", "$b: string", "$c: array{1, 2}"]
    );
}

#[test]
fn inlay_hints_skip_what_is_already_written_or_unknown() {
    assert_eq!(
        hint_labels(
            "<?php\nfunction f(int $typed, $untyped): void {\n    foreach ([1] as $v) { echo $v; }\n}\n"
        ),
        ["$v: int"]
    );
    assert_eq!(
        hint_labels("<?php\n/** @param list<int> $xs */\nfunction f($xs): void {}\n"),
        ["$xs: list<int>"]
    );
    assert!(hint_labels("<?php\nfunction f(): void { $m = g(); }\n").is_empty());
}

// ------------------------------------------------------------------ signature help

#[test]
fn signature_help_tracks_the_active_parameter() {
    let f = Fixture::new(
        "<?php\nfunction greet(string $name, int $times = 1): string { return $name; }\necho greet('a', <|>\n",
    );
    let (file, offset) = f.at();
    let h = hints::signature_help(&f.db, f.ws, file, offset).expect("no help");
    assert!(h.label.contains("greet"), "{}", h.label);
    assert_eq!(h.active_param, 1);
}

#[test]
fn signature_help_for_methods_and_constructors() {
    let f = Fixture::new(&format!("{USER}$u = new User();\n$u->greet('a', <|>);\n"));
    let (file, offset) = f.at();
    let h = hints::signature_help(&f.db, f.ws, file, offset).expect("no help");
    assert!(h.label.contains("greet"), "{}", h.label);
    assert_eq!(h.active_param, 1);
    let f = Fixture::new(
        "<?php\nclass A { public function __construct(int $x, int $y) {} }\nnew A(1, <|>);\n",
    );
    let (file, offset) = f.at();
    let h = hints::signature_help(&f.db, f.ws, file, offset).expect("no help");
    assert_eq!(h.active_param, 1);
}

// ------------------------------------------------------------------ symbols, tokens

#[test]
fn document_outline() {
    let f = Fixture::new(USER);
    let syms = symbols::document_symbols(&f.db, f.files[0]);
    let names: Vec<_> = syms.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["User"]);
    let user: Vec<_> = syms[0].children.iter().map(|s| s.name.as_str()).collect();
    for want in ["greet", "make", "LIMIT"] {
        assert!(user.contains(&want), "{want} missing: {user:?}");
    }
}

#[test]
fn workspace_symbols_prefer_the_project() {
    let f = Fixture::new("<?php\nnamespace App;\nclass UserRepository {}\n");
    let syms = symbols::workspace_symbols(&f.db, f.ws, "UsRepo");
    assert_eq!(syms[0].name, "UserRepository");
    assert_eq!(syms[0].container, "App");
}

#[test]
fn semantic_tokens_classify_php() {
    let f = Fixture::new("<?php\n$a = 1;\nfunction f(int $b): int { return $b; }\n");
    let file = f.files[0];
    let toks = tokens::semantic_tokens(&f.db, f.ws, file);
    let find = |text: &str| {
        toks.iter()
            .find(|t| f.text(file, t.range) == text)
            .copied()
            .unwrap_or_else(|| panic!("no token {text}"))
    };
    assert_eq!(find("function").ty, tokens::TokenType::Keyword);
    assert_eq!(find("$a").ty, tokens::TokenType::Variable);
    assert_eq!(find("1").ty, tokens::TokenType::Number);
}

#[test]
fn folding_and_selection() {
    let f = Fixture::new("<?php\nfunction f(): void {\n    $a = 1;\n}\n");
    let file = f.files[0];
    assert_eq!(symbols::folding_ranges(&f.db, file).len(), 1);
    let sel = symbols::selection_ranges(&f.db, file, TextSize::from(32));
    assert!(sel.len() >= 3);
    assert!(sel.windows(2).all(|w| w[1].contains_range(w[0])));
}

// ------------------------------------------------------------------ implementations

const SHAPES: &str = "//- /src/a.php
<?php
namespace App;
interface Sh<|>ape { public function area(): int; }
interface Solid extends Shape {}
abstract class Base implements Shape { abstract public function name(): string; }
class Circle extends Base {
    public function area(): int { return 1; }
    public function name(): string { return 'c'; }
}
class Big extends Circle {
    public function area(): int { return 2; }
}
//- /vendor/b.php
<?php
namespace Lib;
trait Areas { public function area(): int { return 3; } }
class Cube implements \\App\\Solid { use Areas; }
class HasArea { public function area(): int { return 4; } }
class Inherits extends HasArea implements \\App\\Shape {}
";

#[test]
fn implementations_of_an_interface() {
    let f = Fixture::new(SHAPES);
    assert_eq!(
        f.implementations(),
        [
            "/src/a.php:10:Big",
            "/src/a.php:5:Base",
            "/src/a.php:6:Circle",
            "/vendor/b.php:4:Cube",
            "/vendor/b.php:6:Inherits",
        ]
    );
}

#[test]
fn implementations_of_a_method_find_what_runs() {
    let f = Fixture::new(
        &SHAPES
            .replace("Sh<|>ape", "Shape")
            .replace("function area(): int; }", "function ar<|>ea(): int; }"),
    );
    // overrides, a method from a trait, and one inherited from elsewhere; not the interface's
    assert_eq!(
        f.implementations(),
        [
            "/src/a.php:11:area",
            "/src/a.php:7:area",
            "/vendor/b.php:3:area",
            "/vendor/b.php:5:area",
        ]
    );
}

#[test]
fn implementations_of_a_concrete_method_include_it() {
    let f = Fixture::new(
        "<?php\nclass A { public function run(): void {} private function p(): void {} final public function f(): void {} }\nclass B extends A { public function run(): void {} private function p(): void {} }\n(new B)->r<|>un();\n",
    );
    assert_eq!(f.implementations().len(), 1);
    let f = Fixture::new(
        "<?php\nclass A { public function r<|>un(): void {} }\nclass B extends A { public function run(): void {} }\n",
    );
    assert_eq!(
        f.implementations(),
        ["/src/main.php:2:run", "/src/main.php:3:run"]
    );
    // a private method isn't overridden
    let f = Fixture::new(
        "<?php\nclass A { private function r<|>un(): void {} }\nclass B extends A { private function run(): void {} }\n",
    );
    assert_eq!(f.implementations(), ["/src/main.php:2:run"]);
}

#[test]
fn implementations_reach_the_stubs() {
    let f = Fixture::new(
        "<?php\nclass Mine implements \\Cou<|>ntable { public function count(): int { return 0; } }\n",
    );
    let found = f.implementations();
    assert!(has(&found, "/src/main.php:2:Mine"), "{found:?}");
    assert!(found.iter().any(|l| l.starts_with("stubs://")), "{found:?}");
}

#[test]
fn declaration_is_what_a_method_overrides() {
    let f = Fixture::new(
        "<?php\ninterface I { public function run(): void; }\nabstract class A implements I {}\nclass B extends A { public function r<|>un(): void {} }\n",
    );
    assert_eq!(f.declarations(), ["/src/main.php:2:run"]);
    // one with no such thing above it is declared where it is
    let f = Fixture::new("<?php\nclass B { public function r<|>un(): void {} }\n");
    assert_eq!(f.declarations(), ["/src/main.php:2:run"]);
    // both, if there are two
    let f = Fixture::new(
        "<?php\ninterface I { public function run(): void; }\ninterface J { public function run(): void; }\nclass B implements I, J { public function r<|>un(): void {} }\n",
    );
    assert_eq!(
        f.declarations(),
        ["/src/main.php:2:run", "/src/main.php:3:run"]
    );
}

fn call_names(calls: Vec<(hierarchy::CallItem, Vec<TextRange>)>) -> Vec<String> {
    let mut out: Vec<String> = calls
        .into_iter()
        .map(|(item, ranges)| {
            format!(
                "{}{} x{}",
                item.detail.map(|d| format!("{d}::")).unwrap_or_default(),
                item.name,
                ranges.len()
            )
        })
        .collect();
    out.sort();
    out
}

const CALLS: &str = "<?php
namespace App;
function help(): void {}
interface Job { public function run(): void; }
class Work implements Job {
    public function __construct() { help(); }
    public function run(): void { help(); help(); \\strlen('x'); }
}
function start(Job $j): void { $j->run(); new Work(); $f = function () { help(); }; }
function other(Work $w): void { $w->run(); }
help();
";

#[test]
fn call_hierarchy_of_a_function() {
    let at = CALLS.find("function help").unwrap() + "function ".len() + 1;
    let f = Fixture::new(&format!("{}<|>{}", &CALLS[..at], &CALLS[at..]));
    let (file, offset) = f.at();
    let (def, item) = hierarchy::callable_at(&f.db, f.ws, file, offset).unwrap();
    assert_eq!(item.name, "help");
    let incoming = hierarchy::incoming_calls(&f.db, f.ws, &def);
    assert_eq!(
        call_names(incoming),
        [
            "App::start x1",
            "App\\Work::__construct x1",
            "App\\Work::run x2",
            "main.php x1"
        ]
    );
}

#[test]
fn call_hierarchy_of_an_implementation_has_the_calls_through_its_interface() {
    let at = CALLS.find("function run(): void { help").unwrap() + "function ".len() + 1;
    let f = Fixture::new(&format!("{}<|>{}", &CALLS[..at], &CALLS[at..]));
    let (file, offset) = f.at();
    let (def, item) = hierarchy::callable_at(&f.db, f.ws, file, offset).unwrap();
    assert_eq!(item.detail.as_deref(), Some("App\\Work"));
    let incoming = hierarchy::incoming_calls(&f.db, f.ws, &def);
    assert_eq!(call_names(incoming), ["App::other x1", "App::start x1"]);
    let outgoing = hierarchy::outgoing_calls(&f.db, f.ws, file, item.full_range);
    assert_eq!(call_names(outgoing), ["App::help x2", "strlen x1"]);
}

#[test]
fn call_hierarchy_of_a_constructor_is_who_says_new() {
    let at = CALLS.find("new Work").unwrap() + "new ".len() + 1;
    let f = Fixture::new(&format!("{}<|>{}", &CALLS[..at], &CALLS[at..]));
    let (file, offset) = f.at();
    let (def, item) = hierarchy::callable_at(&f.db, f.ws, file, offset).unwrap();
    assert_eq!(item.kind, hierarchy::CallKind::Constructor);
    let incoming = hierarchy::incoming_calls(&f.db, f.ws, &def);
    assert_eq!(call_names(incoming), ["App::start x1"]);
    let start = hierarchy::callable_at(
        &f.db,
        f.ws,
        file,
        TextSize::from(CALLS.find("function start").unwrap() as u32 + 10),
    )
    .unwrap();
    let outgoing = hierarchy::outgoing_calls(&f.db, f.ws, file, start.1.full_range);
    assert_eq!(
        call_names(outgoing),
        [
            "App::help x1",
            "App\\Job::run x1",
            "App\\Work::__construct x1"
        ]
    );
}

#[test]
fn type_hierarchy() {
    let f = Fixture::new(
        SHAPES
            .replace("Sh<|>ape", "Shape")
            .replace("class Cir", "class Ci<|>r")
            .as_str(),
    );
    let (file, offset) = f.at();
    let item = hierarchy::type_at(&f.db, f.ws, file, offset).unwrap();
    assert_eq!(item.name, "Circle");
    let up: Vec<String> = hierarchy::supertypes(&f.db, f.ws, &item.key)
        .into_iter()
        .map(|i| i.name)
        .collect();
    assert_eq!(up, ["Base"]);
    let down: Vec<String> = hierarchy::subtypes(&f.db, f.ws, &item.key)
        .into_iter()
        .map(|i| i.name)
        .collect();
    assert_eq!(down, ["Big"]);
}

// ------------------------------------------------------------------ `use` lines

#[test]
fn use_lines_resolve_what_they_import() {
    let lib = "//- /src/lib.php\n<?php\nnamespace Lib;\n/** Doc of A. */\nclass A {}\nclass B {}\nfunction helper(): int { return 1; }\nconst LIMIT = 3;\n";
    for (main, want) in [
        ("use Lib\\<|>A;", "class A"),
        ("use Lib\\{A, <|>B};", "class B"),
        ("use function Lib\\hel<|>per;", "function helper"),
        ("use const Lib\\LI<|>MIT;", "const LIMIT"),
        (
            "use Lib\\{function hel<|>per, const LIMIT};",
            "function helper",
        ),
        ("use Lib\\{function helper, const LI<|>MIT};", "const LIMIT"),
    ] {
        let f = Fixture::new(&format!("{lib}//- /src/main.php\n<?php\n{main}\n"));
        assert!(f.hover().contains(want), "{main}: {}", f.hover());
        assert!(f.definition().starts_with("/src/lib.php:"), "{main}");
    }
}
