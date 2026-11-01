//! Editor features over an in-memory workspace. `<|>` marks the cursor;
//! `//- /path` starts another file.

use stir_checker::Def;
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
            chunks.push(("/src/main.sphp".to_owned(), text.to_owned()));
        }
        for (path, body) in chunks {
            let at = body.find("<|>");
            let body = body.replace("<|>", "");
            let dialect = if path.ends_with(".php") {
                Dialect::Php
            } else {
                Dialect::Stir
            };
            let file = SourceFile::new(&db, path.into(), body, dialect, FileOrigin::Source);
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

// ------------------------------------------------------------------ hover

#[test]
fn hover_local_val_and_var() {
    let f = Fixture::new("val count = 1\nvar $total = \"\"\necho cou<|>nt\n");
    assert_eq!(f.hover(), "```sphp\nval count: Int\n```");
    let f = Fixture::new("var $total = \"\"\n$tot<|>al = \"x\"\n");
    assert_eq!(f.hover(), "```sphp\nvar $total: String\n```");
}

#[test]
fn hover_parameter_and_narrowing() {
    let f = Fixture::new(
        "function f(mut times: Int, name: String?): Int {\n    if (name != null) { echo na<|>me }\n    return times\n}\n",
    );
    assert_eq!(f.hover(), "```sphp\nname: String\n```");
    let f = Fixture::new("function f(mut times: Int): Int {\n    return tim<|>es\n}\n");
    assert_eq!(f.hover(), "```sphp\nmut times: Int\n```");
}

#[test]
fn hover_function_with_doc() {
    let f = Fixture::new(
        "namespace App\n/** Doubles a number. */\nfunction double(x: Int): Int = x * 2\nval y = dou<|>ble(2)\n",
    );
    let h = f.hover();
    assert!(h.contains("function double(x: Int): Int"), "{h}");
    assert!(h.contains("Doubles a number."), "{h}");
    assert!(h.contains("```sphp\nApp\n```"), "{h}");
}

#[test]
fn hover_class_method_and_property() {
    let src = "data class User(val id: Int, val name: String)\nopen class Base\nclass Admin(val level: Int) : Base() {\n    function describe(prefix: String): String = prefix . level\n}\n";
    let f = Fixture::new(&format!("{src}val u = new User(1, \"a\")\necho u->na<|>me\n"));
    assert!(f.hover().contains("val name: String"), "{}", f.hover());
    let f = Fixture::new(&format!("{src}val a = new Admin(1)\necho a->desc<|>ribe(\"x\")\n"));
    assert!(
        f.hover().contains("function describe(prefix: String): String"),
        "{}",
        f.hover()
    );
    let f = Fixture::new(&format!("{src}val a: Ad<|>min = new Admin(1)\n"));
    assert!(f.hover().contains("class Admin : Base"), "{}", f.hover());
}

#[test]
fn hover_expression_type() {
    // right after the lambda: the lambda's type
    let f = Fixture::new("val xs = listOf(1, 2)\nval n = xs->map { it * 2 }<|>\n");
    assert_eq!(f.hover(), "```sphp\n(Int) -> Int\n```");
    let f = Fixture::new("val n = (1 + 2) <|>* 3\n");
    assert_eq!(f.hover(), "```sphp\nInt\n```");
}

#[test]
fn hover_extension_function_from_stdlib() {
    let f = Fixture::new("val s = \"abc\"->upper<|>case()\n");
    let h = f.hover();
    assert!(h.contains("function String.uppercase(): String"), "{h}");
}

// ------------------------------------------------------------------ navigation

#[test]
fn definition_of_locals_params_and_functions() {
    let f = Fixture::new("val count = 1\necho cou<|>nt\n");
    assert_eq!(f.definition(), "/src/main.sphp:count");
    let f = Fixture::new("function f(mut times: Int): Int {\n    return tim<|>es\n}\n");
    assert_eq!(f.definition(), "/src/main.sphp:times");
    let f = Fixture::new("var $n = 0\n$<|>n += 1\n");
    assert_eq!(f.definition(), "/src/main.sphp:$n");
}

#[test]
fn definition_across_files() {
    let f = Fixture::new(
        "//- /src/lib.sphp\nnamespace App\nfunction helper(): Int = 1\nclass Box(val v: Int)\n//- /src/main.sphp\nnamespace App\nval b: Bo<|>x = new Box(helper())\n",
    );
    assert_eq!(f.definition(), "/src/lib.sphp:Box");
    let f = Fixture::new(
        "//- /src/lib.sphp\nnamespace App\nfunction helper(): Int = 1\n//- /src/main.sphp\nnamespace App\nval b = help<|>er()\n",
    );
    assert_eq!(f.definition(), "/src/lib.sphp:helper");
}

#[test]
fn definition_of_members() {
    let src = "class Counter {\n    var $count: Int = 0\n    function inc() { $count += 1 }\n    function get(): Int = count\n}\n";
    let f = Fixture::new(&format!("{src}val c = new Counter()\nc->in<|>c()\n"));
    assert_eq!(f.definition(), "/src/main.sphp:inc");
    // implicit `$this->` member
    let f = Fixture::new(&src.replace("{ $count += 1 }", "{ $cou<|>nt += 1 }"));
    assert_eq!(f.definition(), "/src/main.sphp:$count");
    let f = Fixture::new("data class P(val x: Int)\nval p = new P(1)\necho p-><|>x\n");
    assert_eq!(f.definition(), "/src/main.sphp:x");
    let f = Fixture::new("record Point { x: Int; y: Int }\nvar $p = Point { x = 1, y = 2 }\necho $p->y<|>\n");
    assert_eq!(f.definition(), "/src/main.sphp:y");
}

#[test]
fn definition_into_stdlib_and_php() {
    let f = Fixture::new("val s = listOf<|>(1)\n");
    assert!(f.definition().starts_with("stdlib://"), "{}", f.definition());
    let f = Fixture::new(
        "//- /src/legacy.php\n<?php\nnamespace Legacy;\nclass Mailer { public function send(string $to): bool { return true; } }\n//- /src/main.sphp\nuse Legacy\\Mailer\nval m = new Mailer()\nval ok = m->se<|>nd(\"a\")\n",
    );
    assert_eq!(f.definition(), "/src/legacy.php:send");
}

#[test]
fn references_across_files() {
    let f = Fixture::new(
        "//- /src/lib.sphp\nnamespace App\nfunction help<|>er(): Int = 1\n//- /src/main.sphp\nnamespace App\nval a = helper()\nval b = helper() + helper()\n",
    );
    assert_eq!(
        f.references(),
        [
            "/src/lib.sphp:2:helper",
            "/src/main.sphp:2:helper",
            "/src/main.sphp:3:helper",
            "/src/main.sphp:3:helper"
        ]
    );
}

#[test]
fn references_of_a_mutable_var_and_its_shadowless_scope() {
    let f = Fixture::new(
        "function a() {\n    var $n = 0\n    $n += 1\n    echo $<|>n\n}\nfunction b() {\n    var $n = 5\n    echo $n\n}\n",
    );
    assert_eq!(
        f.references(),
        ["/src/main.sphp:2:$n", "/src/main.sphp:3:$n", "/src/main.sphp:4:$n"]
    );
}

#[test]
fn references_of_a_class_in_types_and_expressions() {
    let f = Fixture::new(
        "class Us<|>er(val id: Int)\nfunction make(): User = new User(1)\nval u: User? = null\nval ok = u is User\n",
    );
    let refs = f.references();
    assert_eq!(refs.len(), 5, "{refs:?}");
}

#[test]
fn project_defs_are_renameable_and_builtins_are_not() {
    let f = Fixture::new("val s = \"a\"->upper<|>case()\n");
    let (file, offset) = f.at();
    let (_, def) = nav::def_at(&f.db, f.ws, file, offset).unwrap();
    assert!(!hover::is_project_def(&f.db, f.ws, file, &def));
    let f = Fixture::new("val co<|>unt = 1\n");
    let (file, offset) = f.at();
    let (_, def) = nav::def_at(&f.db, f.ws, file, offset).unwrap();
    assert!(matches!(def, Def::Local { .. }));
    assert!(hover::is_project_def(&f.db, f.ws, file, &def));
}

// ------------------------------------------------------------------ completion

#[test]
fn completes_members_and_extensions() {
    let src = "class Counter {\n    var $count: Int = 0\n    private var $secret: Int = 0\n    function inc() { $count += 1 }\n}\nval c = new Counter()\n";
    let f = Fixture::new(&format!("{src}c-><|>\n"));
    let items = f.completions();
    assert!(has(&items, "inc"), "{items:?}");
    assert!(has(&items, "count"), "{items:?}");
    assert!(!has(&items, "secret"), "{items:?}");
    let f = Fixture::new("val s = \"abc\"\nval t = s->up<|>\n");
    let items = f.completions();
    assert!(has(&items, "uppercase"), "{items:?}");
    assert!(has(&items, "length"), "{items:?}");
    let f = Fixture::new("val xs = listOf(1, 2)\nxs-><|>\n");
    assert!(has(&f.completions(), "map"), "{:?}", f.completions());
}

#[test]
fn completes_locals_in_scope() {
    let f = Fixture::new(
        "val outer = 1\nfunction f(mut times: Int) {\n    val inner = 2\n    var $acc = 0\n    if (true) { val hidden = 3 }\n    echo <|>\n}\n",
    );
    let items = f.completions();
    for want in ["inner", "$acc", "times"] {
        assert!(has(&items, want), "{want} missing: {items:?}");
    }
    assert!(!has(&items, "hidden"), "{items:?}");
    assert!(!has(&items, "outer"), "{items:?}");
    assert!(has(&items, "val"), "{items:?}");
    let f = Fixture::new("var $acc = 0\nval keep = 1\necho $<|>\n");
    let items = f.completions();
    assert!(has(&items, "$acc"), "{items:?}");
    assert!(!has(&items, "keep"), "{items:?}");
}

#[test]
fn completes_globals_by_prefix() {
    let f = Fixture::new("namespace App\nfunction helperOne(): Int = 1\nval x = helpe<|>\n");
    let items = f.completions();
    assert!(has(&items, "helperOne"), "{items:?}");
    let f = Fixture::new("val x = listO<|>\n");
    assert!(has(&f.completions(), "listOf"), "{:?}", f.completions());
    let f = Fixture::new("val x = str_repl<|>\n");
    assert!(has(&f.completions(), "str_replace"));
}

#[test]
fn completes_static_members() {
    let f = Fixture::new("enum Color { case Red; case Green }\nval c = Color::<|>\n");
    let items = f.completions();
    assert!(has(&items, "Red") && has(&items, "Green"), "{items:?}");
}

// ------------------------------------------------------------------ hints

#[test]
fn inlay_hints_for_untyped_bindings() {
    let f = Fixture::new(
        "val a = 1\nval b: Int = 2\nvar $c = \"s\"\nval xs = listOf(1)->map { x -> x * 2 }\n",
    );
    let file = f.files[0];
    let all = TextRange::up_to(TextSize::of(file.text(&f.db).as_str()));
    let got: Vec<String> = hints::inlay_hints(&f.db, f.ws, file, all)
        .into_iter()
        .map(|h| h.label)
        .collect();
    assert_eq!(got, [": Int", ": String", ": List<Int>", ": Int"]);
}

#[test]
fn signature_help_tracks_the_active_parameter() {
    let f = Fixture::new("function greet(name: String, mut times: Int = 1): String = name\nval g = greet(\"a\", <|>\n");
    let (file, offset) = f.at();
    let h = hints::signature_help(&f.db, f.ws, file, offset).expect("no help");
    assert_eq!(
        h.label,
        "function greet(name: String, mut times: Int = …): String"
    );
    assert_eq!(h.active_param, 1);
    let (s, e) = h.params[1];
    assert_eq!(&h.label[s as usize..e as usize], "mut times: Int = …");
}

#[test]
fn signature_help_for_methods_and_constructors() {
    let f = Fixture::new("class A(val x: Int, val y: Int)\nval a = new A(1, <|>)\n");
    let (file, offset) = f.at();
    let h = hints::signature_help(&f.db, f.ws, file, offset).expect("no help");
    assert_eq!(h.label, "new A(x: Int, y: Int): A");
    assert_eq!(h.active_param, 1);
}

// ------------------------------------------------------------------ symbols, tokens

#[test]
fn document_outline() {
    let f = Fixture::new(
        "namespace App\nfunction top(): Int = 1\nclass User(val id: Int) {\n    function name(): String = \"u\"\n}\nfunction String.shout(): String = this\n",
    );
    let syms = symbols::document_symbols(&f.db, f.files[0]);
    let names: Vec<_> = syms.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["top", "User", "String.shout"]);
    let user: Vec<_> = syms[1].children.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(user, ["id", "name"]);
}

#[test]
fn workspace_symbols_prefer_the_project() {
    let f = Fixture::new("namespace App\nclass UserRepository\n");
    let syms = symbols::workspace_symbols(&f.db, f.ws, "UsRepo");
    assert_eq!(syms[0].name, "UserRepository");
    assert_eq!(syms[0].container, "App");
}

#[test]
fn semantic_tokens_mark_mutability() {
    let f = Fixture::new("val a = 1\nvar $b = 2\n$b = a\n");
    let file = f.files[0];
    let toks = tokens::semantic_tokens(&f.db, f.ws, file);
    let find = |text: &str, nth: usize| {
        toks.iter()
            .filter(|t| f.text(file, t.range) == text)
            .nth(nth)
            .copied()
            .unwrap_or_else(|| panic!("no token {text}"))
    };
    let val_kw = find("val", 0);
    assert_eq!(val_kw.ty, tokens::TokenType::Keyword);
    let a_use = find("a", 1);
    assert_eq!(a_use.ty, tokens::TokenType::Variable);
    assert!(a_use.modifiers & tokens::modifier::READONLY != 0);
    let b_decl = find("$b", 0);
    assert!(b_decl.modifiers & tokens::modifier::MUTABLE != 0);
    assert!(b_decl.modifiers & tokens::modifier::DECLARATION != 0);
    assert_eq!(find("1", 0).ty, tokens::TokenType::Number);
}

#[test]
fn folding_and_selection() {
    let f = Fixture::new("function f() {\n    val a = 1\n}\n");
    let file = f.files[0];
    assert_eq!(symbols::folding_ranges(&f.db, file).len(), 1);
    let sel = symbols::selection_ranges(&f.db, file, TextSize::from(25));
    assert!(sel.len() >= 3);
    assert!(sel.windows(2).all(|w| w[1].contains_range(w[0])));
}
