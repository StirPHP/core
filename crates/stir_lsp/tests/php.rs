//! Editor features with the cursor in a plain PHP file. `<|>` marks the cursor;
//! `//- /path` starts another file.

use stir_db::{FileOrigin, RootDatabase, Settings, SourceFile, Workspace};
use stir_lsp::{complete, hints, hover, nav, symbols, tokens};
use stir_syntax::{Dialect, TextRange, TextSize};

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
            let file = SourceFile::new(&db, path.into(), body, Dialect::Php, FileOrigin::Source);
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
fn completes_php_keywords_not_superset_ones() {
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
        ["$a: int", "$b: string", "$c: list<int>"]
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
    // every PHP variable is mutable, so marking it says nothing
    assert_eq!(find("$a").modifiers & tokens::modifier::MUTABLE, 0);
    assert_eq!(find("$b").modifiers & tokens::modifier::MUTABLE, 0);
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
