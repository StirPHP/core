//! Editor features with the cursor in a PHP file. `<|>` marks the cursor;
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

// ------------------------------------------------------------------ `use` insertion

const LIB: &str = "//- /vendor/lib.php
<?php
namespace Vendor\\Http;
class Request {}
class Response {}
interface Client {}
trait Retries {}
function fetch(): int { return 1; }
const TIMEOUT = 3;
//- /vendor/other.php
<?php
namespace Vendor\\Other;
class Request {}
//- /vendor/global.php
<?php
class GlobalThing {}
";

impl Fixture {
    /// The file with the item that has this label and full name accepted.
    fn accept(&self, label: &str, full: &str) -> String {
        let (file, offset) = self.at();
        let c = complete::complete(&self.db, self.ws, file, offset);
        let item = c
            .items
            .iter()
            .find(|i| i.label == label && i.qualified.as_deref() == Some(full))
            .unwrap_or_else(|| {
                let found: Vec<_> = c.items.iter().map(|i| &i.label).collect();
                panic!("no {label} {full} in {found:?}")
            });
        let mut text = file.text(&self.db).clone();
        text.replace_range(
            usize::from(c.replace.start())..usize::from(c.replace.end()),
            &item.insert,
        );
        if let Some(edit) = &item.import {
            text.insert_str(usize::from(edit.at), &edit.text);
        }
        text
    }
}

fn with_main(main: &str) -> Fixture {
    Fixture::new(&format!("{LIB}//- /src/main.php\n{main}"))
}

#[test]
fn accepting_a_class_adds_its_use_line() {
    let f = with_main("<?php\nnamespace App;\n\nnew Requ<|>\n");
    assert_eq!(
        f.accept("Request", "Vendor\\Http\\Request"),
        "<?php\nnamespace App;\n\nuse Vendor\\Http\\Request;\n\nnew Request\n"
    );
    // directly below the namespace line
    let f = with_main("<?php\nnamespace App;\nnew Requ<|>\n");
    assert_eq!(
        f.accept("Request", "Vendor\\Http\\Request"),
        "<?php\nnamespace App;\n\nuse Vendor\\Http\\Request;\n\nnew Request\n"
    );
    // without a namespace, below the tag, a declare and a header
    let f = with_main("<?php\n\nnew Requ<|>\n");
    assert_eq!(
        f.accept("Request", "Vendor\\Http\\Request"),
        "<?php\n\nuse Vendor\\Http\\Request;\n\nnew Request\n"
    );
    let f = with_main("<?php\ndeclare(strict_types=1);\n\nnew Requ<|>\n");
    assert_eq!(
        f.accept("Request", "Vendor\\Http\\Request"),
        "<?php\ndeclare(strict_types=1);\n\nuse Vendor\\Http\\Request;\n\nnew Request\n"
    );
    let f = with_main("<?php\n/* Licence */\n\n/** About it. */\nnew Requ<|>\n");
    assert_eq!(
        f.accept("Request", "Vendor\\Http\\Request"),
        "<?php\n/* Licence */\n\nuse Vendor\\Http\\Request;\n\n/** About it. */\nnew Request\n"
    );
    // in a block
    let f = with_main("<?php\nnamespace App {\n    new Requ<|>\n}\n");
    assert_eq!(
        f.accept("Request", "Vendor\\Http\\Request"),
        "<?php\nnamespace App {\n    use Vendor\\Http\\Request;\n\n    new Request\n}\n"
    );
    // with the file's line breaks
    let f = with_main("<?php\r\nnamespace App;\r\n\r\nnew Requ<|>\r\n");
    assert_eq!(
        f.accept("Request", "Vendor\\Http\\Request"),
        "<?php\r\nnamespace App;\r\n\r\nuse Vendor\\Http\\Request;\r\n\r\nnew Request\r\n"
    );
}

#[test]
fn a_use_line_goes_in_order() {
    let main = "<?php\nnamespace App;\n\nuse Alpha\\A;\nuse Zed\\Z;\n\nnew Requ<|>\n";
    assert_eq!(
        with_main(main).accept("Request", "Vendor\\Http\\Request"),
        "<?php\nnamespace App;\n\nuse Alpha\\A;\nuse Vendor\\Http\\Request;\nuse Zed\\Z;\n\nnew Request\n"
    );
    // not sorted: at the end
    let main = "<?php\nnamespace App;\n\nuse Zed\\Z;\nuse Alpha\\A;\n\nnew Requ<|>\n";
    assert_eq!(
        with_main(main).accept("Request", "Vendor\\Http\\Request"),
        "<?php\nnamespace App;\n\nuse Zed\\Z;\nuse Alpha\\A;\nuse Vendor\\Http\\Request;\n\nnew Request\n"
    );
    // kinds apart, classes first
    let main = "<?php\nnamespace App;\n\nuse function Alpha\\f;\n\nnew Requ<|>\n";
    assert_eq!(
        with_main(main).accept("Request", "Vendor\\Http\\Request"),
        "<?php\nnamespace App;\n\nuse Vendor\\Http\\Request;\n\nuse function Alpha\\f;\n\nnew Request\n"
    );
}

#[test]
fn functions_and_constants_get_their_kind_of_use_line() {
    let main = "<?php\nnamespace App;\n\nuse Alpha\\A;\n\nfet<|>\n";
    assert_eq!(
        with_main(main).accept("fetch", "Vendor\\Http\\fetch"),
        "<?php\nnamespace App;\n\nuse Alpha\\A;\n\nuse function Vendor\\Http\\fetch;\n\nfetch\n"
    );
    let main = "<?php\nnamespace App;\n\nuse Alpha\\A;\n\nuse function Beta\\b;\n\nTIME<|>\n";
    assert_eq!(
        with_main(main).accept("TIMEOUT", "Vendor\\Http\\TIMEOUT"),
        "<?php\nnamespace App;\n\nuse Alpha\\A;\n\nuse function Beta\\b;\n\nuse const Vendor\\Http\\TIMEOUT;\n\nTIMEOUT\n"
    );
    // global functions just work from a namespace
    let main = "<?php\nnamespace App;\n\nstrle<|>\n";
    assert_eq!(
        with_main(main).accept("strlen", "strlen"),
        "<?php\nnamespace App;\n\nstrlen\n"
    );
}

#[test]
fn what_is_already_available_needs_no_use_line() {
    // imported already
    let main = "<?php\nnamespace App;\n\nuse Vendor\\Http\\Request;\n\nnew Requ<|>\n";
    assert_eq!(
        with_main(main).accept("Request", "Vendor\\Http\\Request"),
        "<?php\nnamespace App;\n\nuse Vendor\\Http\\Request;\n\nnew Request\n"
    );
    // imported under another name, or in a group
    let main = "<?php\nnamespace App;\n\nuse Vendor\\Http\\Request as Req;\n\nnew Requ<|>\n";
    assert_eq!(
        with_main(main).accept("Request", "Vendor\\Http\\Request"),
        "<?php\nnamespace App;\n\nuse Vendor\\Http\\Request as Req;\n\nnew Req\n"
    );
    let main = "<?php\nnamespace App;\n\nuse Vendor\\Http\\{Request, Response};\n\nnew Requ<|>\n";
    assert_eq!(
        with_main(main).accept("Request", "Vendor\\Http\\Request"),
        "<?php\nnamespace App;\n\nuse Vendor\\Http\\{Request, Response};\n\nnew Request\n"
    );
    // in the same namespace
    let main = "<?php\nnamespace Vendor\\Http;\n\nnew Requ<|>\n";
    assert_eq!(
        with_main(main).accept("Request", "Vendor\\Http\\Request"),
        "<?php\nnamespace Vendor\\Http;\n\nnew Request\n"
    );
    // imported below the cursor counts
    let main = "<?php\nnamespace App;\nnew Requ<|>;\nuse Vendor\\Http\\Request;\n";
    assert_eq!(
        with_main(main).accept("Request", "Vendor\\Http\\Request"),
        "<?php\nnamespace App;\nnew Request;\nuse Vendor\\Http\\Request;\n"
    );
}

#[test]
fn a_name_that_cannot_be_imported_is_written_in_full() {
    // the short name is another import's
    let main = "<?php\nnamespace App;\n\nuse Vendor\\Other\\Request;\n\nnew Requ<|>\n";
    assert_eq!(
        with_main(main).accept("Request", "Vendor\\Http\\Request"),
        "<?php\nnamespace App;\n\nuse Vendor\\Other\\Request;\n\nnew \\Vendor\\Http\\Request\n"
    );
    // or a class of this namespace that importing would hide
    let main = "//- /src/mine.php\n<?php\nnamespace App;\nclass Request {}\n//- /src/main.php\n<?php\nnamespace App;\n\nnew Requ<|>\n";
    let f = Fixture::new(&format!("{LIB}{main}"));
    assert_eq!(
        f.accept("Request", "Vendor\\Http\\Request"),
        "<?php\nnamespace App;\n\nnew \\Vendor\\Http\\Request\n"
    );
    // or declared in this file
    let main = "<?php\nnamespace App;\n\nclass Request {}\nnew Requ<|>\n";
    assert_eq!(
        with_main(main).accept("Request", "Vendor\\Http\\Request"),
        "<?php\nnamespace App;\n\nclass Request {}\nnew \\Vendor\\Http\\Request\n"
    );
    // a function over a global one
    let main = "//- /vendor/f.php\n<?php\nnamespace Lib;\nfunction strlen(): int { return 0; }\n//- /src/main.php\n<?php\nnamespace App;\n\nstrle<|>\n";
    let f = Fixture::new(main);
    assert_eq!(
        f.accept("strlen", "Lib\\strlen"),
        "<?php\nnamespace App;\n\n\\Lib\\strlen\n"
    );
}

#[test]
fn global_classes_follow_the_file() {
    // an import by default
    let main = "<?php\nnamespace App;\n\nnew GlobalTh<|>\n";
    assert_eq!(
        with_main(main).accept("GlobalThing", "GlobalThing"),
        "<?php\nnamespace App;\n\nuse GlobalThing;\n\nnew GlobalThing\n"
    );
    // `\\Foo` where that's what the file does
    let main = "<?php\nnamespace App;\n\nnew \\Exception();\nnew GlobalTh<|>\n";
    assert_eq!(
        with_main(main).accept("GlobalThing", "GlobalThing"),
        "<?php\nnamespace App;\n\nnew \\Exception();\nnew \\GlobalThing\n"
    );
    // without a namespace, as it is
    let main = "<?php\nnew GlobalTh<|>\n";
    assert_eq!(
        with_main(main).accept("GlobalThing", "GlobalThing"),
        "<?php\nnew GlobalThing\n"
    );
}

#[test]
fn completion_knows_what_a_place_takes() {
    let labels = |main: &str| with_main(main).completions();
    let full = |main: &str, want: &str| {
        let f = with_main(main);
        let (file, offset) = f.at();
        complete::complete(&f.db, f.ws, file, offset)
            .items
            .iter()
            .any(|i| i.qualified.as_deref() == Some(want))
    };
    const CLIENT: &str = "Vendor\\Http\\Client";
    const REQUEST: &str = "Vendor\\Http\\Request";
    assert!(
        !full("<?php\nnew Cli<|>\n", CLIENT),
        "an interface can't be created"
    );
    assert!(full("<?php\nclass A implements Cli<|> {}\n", CLIENT));
    assert!(!full("<?php\nclass A implements Req<|> {}\n", REQUEST));
    assert!(full(
        "<?php\nclass A { use Retr<|> }\n",
        "Vendor\\Http\\Retries"
    ));
    assert!(!full("<?php\nclass A { use Req<|> }\n", REQUEST));
    assert!(full("<?php\nfunction f(Req<|>) {}\n", REQUEST));
    assert!(!full(
        "<?php\nfunction f(fet<|>) {}\n",
        "Vendor\\Http\\fetch"
    ));
    assert!(has(&labels("<?php\nfunction f(in<|>) {}\n"), "int"));
    // nothing in strings and comments
    assert!(labels("<?php\n$a = 'Req<|>';\n").is_empty());
    assert!(labels("<?php\n// Req<|> \n").is_empty());
}

#[test]
fn completion_matches_fuzzily_with_the_best_first() {
    let lib = "//- /vendor/a.php\n<?php\nnamespace V;\nclass HttpClientFactory {}\nclass HelloThere {}\nclass ShouldBeHttp {}\nclass Chart {}\n//- /src/mine.php\n<?php\nnamespace App;\nclass HttpThing {}\n";
    let f = Fixture::new(&format!(
        "{lib}//- /src/main.php\n<?php\nnamespace App;\nnew HCF<|>\n"
    ));
    let (file, offset) = f.at();
    let c = complete::complete(&f.db, f.ws, file, offset);
    assert_eq!(c.items[0].label, "HttpClientFactory");
    let f = Fixture::new(&format!(
        "{lib}//- /src/main.php\n<?php\nnamespace App;\nnew http<|>\n"
    ));
    let (file, offset) = f.at();
    let c = complete::complete(&f.db, f.ws, file, offset);
    let order: Vec<_> = c.items.iter().map(|i| i.label.as_str()).collect();
    // what starts with it before what has it inside, the project's before others'
    let at = |l: &str| order.iter().position(|o| *o == l).unwrap();
    assert!(at("HttpThing") < at("HttpClientFactory"), "{order:?}");
    assert!(at("HttpClientFactory") < at("ShouldBeHttp"), "{order:?}");
}

#[test]
fn a_written_namespace_narrows_completion() {
    let main = "<?php\nnamespace App;\n\nnew \\Vendor\\Http\\Re<|>\n";
    assert_eq!(
        with_main(main).accept("Request", "Vendor\\Http\\Request"),
        "<?php\nnamespace App;\n\nnew \\Vendor\\Http\\Request\n"
    );
    assert!(!has(&with_main(main).completions(), "Client"));
    // through an import of the namespace
    let main = "<?php\nnamespace App;\n\nuse Vendor\\Http;\n\nnew Http\\Re<|>\n";
    assert_eq!(
        with_main(main).accept("Request", "Vendor\\Http\\Request"),
        "<?php\nnamespace App;\n\nuse Vendor\\Http;\n\nnew Http\\Request\n"
    );
    // absolute
    let main = "<?php\nnamespace App;\n\nnew \\GlobalTh<|>\n";
    assert_eq!(
        with_main(main).accept("GlobalThing", "GlobalThing"),
        "<?php\nnamespace App;\n\nnew \\GlobalThing\n"
    );
}

#[test]
fn use_lines_complete_with_whole_names() {
    let main = "<?php\nnamespace App;\n\nuse Requ<|>\n";
    assert_eq!(
        with_main(main).accept("Vendor\\Http\\Request", "Vendor\\Http\\Request"),
        "<?php\nnamespace App;\n\nuse Vendor\\Http\\Request\n"
    );
    let main = "<?php\nnamespace App;\n\nuse Vendor\\Http\\Requ<|>\n";
    assert_eq!(
        with_main(main).accept("Vendor\\Http\\Request", "Vendor\\Http\\Request"),
        "<?php\nnamespace App;\n\nuse Vendor\\Http\\Request\n"
    );
    let main = "<?php\nuse function fet<|>\n";
    assert_eq!(
        with_main(main).accept("Vendor\\Http\\fetch", "Vendor\\Http\\fetch"),
        "<?php\nuse function Vendor\\Http\\fetch\n"
    );
    assert!(!has(
        &with_main("<?php\nuse function Req<|>\n").completions(),
        "Vendor\\Http\\Request"
    ));
}

/// How long completion takes on a big project: `cargo test --release -p stir_lsp --test php
/// completion_latency -- --ignored --nocapture`, after installing `corpus/symfony`'s packages.
#[test]
#[ignore = "needs corpus/symfony, and is about timing"]
fn completion_latency() {
    use salsa::Setter;
    use std::time::{Duration, Instant};
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/symfony");
    let root = std::fs::canonicalize(root).unwrap();
    let start = Instant::now();
    let (mut db, ws, target) = runtime.block_on(async {
        let project = stir_project::Project::load_or_default(&root).await.unwrap();
        let loaded = stir_project::workspace::load_workspace(
            &project,
            stir_project::workspace::LoadOptions::default(),
        )
        .await
        .unwrap();
        let target = loaded
            .sources
            .iter()
            .copied()
            .find(|f| {
                f.path(&loaded.db)
                    .ends_with("src/Symfony/Component/HttpFoundation/Request.php")
            })
            .unwrap();
        (loaded.db, loaded.workspace, target)
    });
    println!(
        "loaded {} files in {:?}",
        ws.files(&db).len(),
        start.elapsed()
    );
    let original = target.text(&db).clone();
    let at = original.rfind('}').unwrap();
    let edited = |word: &str| {
        let mut t = original.clone();
        t.insert_str(at, &format!("\n    public function probe() {{ {word} }}\n"));
        (
            t.clone(),
            at + "\n    public function probe() { ".len() + word.len(),
        )
    };
    let timed = |db: &stir_db::RootDatabase, word: &str, offset: usize| {
        let start = Instant::now();
        let c = complete::complete(db, ws, target, TextSize::from(offset as u32));
        let elapsed = start.elapsed();
        println!(
            "{word:>12}: {elapsed:?}, {} items (incomplete: {})",
            c.items.len(),
            c.incomplete
        );
        elapsed
    };
    // the first one builds everything; after that as an editor does, a keystroke at a time
    let mut warm: Vec<Duration> = Vec::new();
    let mut all = Vec::new();
    for (i, word) in [
        "new R",
        "new Re",
        "new Req",
        "new Reque",
        "new Reques",
        "new Reqst",
        "cach",
        "array_ma",
        "HCF",
    ]
    .iter()
    .enumerate()
    {
        let (text, offset) = edited(word);
        target.set_text(&mut db).to(text);
        let t = timed(&db, word, offset);
        if i > 0 {
            warm.push(t);
        }
        all.push(t);
    }
    // nothing changed but the cursor: the checker's answers are all there
    let (text, offset) = edited("new Reque");
    target.set_text(&mut db).to(text);
    timed(&db, "new Reque", offset);
    let again = timed(&db, "new Reque", offset);
    warm.sort();
    println!(
        "after an edit: median {:?}, max {:?}; no edit: {again:?}; first: {:?}",
        warm[warm.len() / 2],
        warm[warm.len() - 1],
        all[0]
    );
}

// ------------------------------------------------------------------ deprecated and throws

const OLD: &str = "<?php
namespace Old;
/** @deprecated use something else */
function gone(): void {}
/**
 * @deprecated since 2
 */
class Box {
    /** @deprecated no more */
    public int $n = 0;
    /** @deprecated */
    public const X = 1;
    /** @throws \\RuntimeException */
    public function risky(): void {}
    #[\\Deprecated(message: 'stop', since: '1.0')]
    public function attr(): void {}
    public function fine(): void {}
}
class Now {
    #[\\Deprecated(message: 'stop', since: '1.0')]
    public function attr(): void {}
    public function fine(): void {}
    /** @throws \\RuntimeException */
    public function risky(): void {}
}
";

#[test]
fn hover_says_what_is_deprecated_and_what_it_throws() {
    let f = Fixture::new(&format!("{OLD}go<|>ne();\n"));
    assert!(
        f.hover().contains("**Deprecated**: use something else"),
        "{}",
        f.hover()
    );
    let f = Fixture::new(&format!("{OLD}function t(Now $b) {{ $b->at<|>tr(); }}\n"));
    assert!(f.hover().contains("**Deprecated**: stop"), "{}", f.hover());
    let f = Fixture::new(&format!("{OLD}function t(Now $b) {{ $b->fi<|>ne(); }}\n"));
    assert!(!f.hover().contains("**Deprecated**"), "{}", f.hover());
    let f = Fixture::new(&format!("{OLD}function t(Now $b) {{ $b->ri<|>sky(); }}\n"));
    assert!(
        f.hover().contains("@throws \\RuntimeException"),
        "{}",
        f.hover()
    );
}

#[test]
fn completion_says_what_is_deprecated() {
    let f = Fixture::new(&format!("{OLD}function t(Box $b) {{ $b-><|> }}\n"));
    let (file, offset) = f.at();
    let c = complete::complete(&f.db, f.ws, file, offset);
    let deprecated = |l: &str| c.items.iter().find(|i| i.label == l).unwrap().deprecated;
    // the class is deprecated, so all of it is
    assert!(deprecated("fine"));
    let f = Fixture::new(&format!("{OLD}function t(Now $b) {{ $b-><|> }}\n"));
    let (file, offset) = f.at();
    let c = complete::complete(&f.db, f.ws, file, offset);
    let deprecated = |l: &str| c.items.iter().find(|i| i.label == l).unwrap().deprecated;
    assert!(deprecated("attr"));
    assert!(!deprecated("fine"));
    let f = Fixture::new(&format!("{OLD}gon<|>\n"));
    let (file, offset) = f.at();
    let c = complete::complete(&f.db, f.ws, file, offset);
    assert!(c.items.iter().any(|i| i.label == "gone" && i.deprecated));
}
