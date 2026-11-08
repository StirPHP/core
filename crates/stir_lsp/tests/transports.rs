//! The same editing session spoken to the native server and to the session that runs in the
//! browser (compiled natively here): what they answer is the same, but for where the builtin
//! stubs are.

mod client;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use client::{Client, uri, write};
use serde_json::{Value, json};
use stir_lsp::server::TrustOptions;
use stir_lsp::trust::TrustStore;

const RULE: &str = r#"fn visit_func_call(node, ctx) {
    if node.name == "var_dump" { ctx.error("no var_dump", "app.noVarDump"); }
}"#;

const MAIN: &str = r#"<?php
namespace App;

use App\Shapes\Shape;
use App\Shapes\Circle;

function total(Shape ...$shapes): float
{
    $sum = 0.0;
    foreach ($shapes as $shape) {
        $sum += $shape->area();
    }
    return $sum;
}

$all = [new Circle(2.0), new Square(3.0)];
echo total(...$all);
echo strlen('abc');
var_dump($all);
echo missing_function();
new Squ;
"#;

const MESSY: &str =
    "<?php\nnamespace App;\nfunction   messy( $a,$b ){\nif($a){return $b;}\n    return    null;}\n";

/// A Composer project with a package in `vendor/`, a framework, a configuration with a rule, and
/// files that need each other.
fn project(root: &Path) {
    write(
        &root.join("composer.json"),
        r#"{
    "require": {"php": "^8.1", "laravel/framework": "^13", "acme/greeter": "^1"},
    "autoload": {"psr-4": {"App\\": "src/"}}
}"#,
    );
    write(
        &root.join("composer.lock"),
        r#"{"packages": [{"name": "laravel/framework", "version": "13.0.0"}, {"name": "acme/greeter", "version": "1.0.0"}], "packages-dev": []}"#,
    );
    write(
        &root.join("vendor/composer/installed.json"),
        r#"{"packages": [
    {"name": "laravel/framework", "version": "13.0.0", "install-path": "../laravel/framework",
     "autoload": {"psr-4": {"Illuminate\\": "src/Illuminate/"}}},
    {"name": "acme/greeter", "version": "1.0.0", "install-path": "../acme/greeter",
     "autoload": {"psr-4": {"Acme\\": "src/"}}}
]}"#,
    );
    write(
        &root.join("vendor/laravel/framework/src/Illuminate/Database/Eloquent/Model.php"),
        "<?php\nnamespace Illuminate\\Database\\Eloquent;\nabstract class Model\n{\n    public function __get(string $key): mixed { return null; }\n}\n",
    );
    write(
        &root.join("vendor/acme/greeter/src/Greeter.php"),
        "<?php\nnamespace Acme;\ninterface Greeter\n{\n    public function greet(string $name): string;\n}\n",
    );
    write(
        &root.join("stirphp.neon"),
        "includes:\n    - phpstan.neon\nparameters:\n    stirphp:\n        rules:\n            - rules/no_var_dump.rhai\n",
    );
    write(&root.join("phpstan.neon"), "parameters:\n    level: 6\n");
    write(&root.join("rules/no_var_dump.rhai"), RULE);
    write(
        &root.join("src/Shapes/Shape.php"),
        "<?php\nnamespace App\\Shapes;\n\ninterface Shape\n{\n    public function area(): float;\n}\n",
    );
    write(
        &root.join("src/Shapes/Circle.php"),
        "<?php\nnamespace App\\Shapes;\n\nfinal class Circle implements Shape\n{\n    public function __construct(private float $radius) {}\n\n    public function area(): float\n    {\n        return M_PI * $this->radius ** 2;\n    }\n}\n",
    );
    write(
        &root.join("src/Shapes/Square.php"),
        "<?php\nnamespace App\\Shapes;\n\nclass Square implements Shape\n{\n    public function __construct(private float $side) {}\n\n    public function area(): float\n    {\n        return $this->side * $this->side;\n    }\n}\n",
    );
    write(
        &root.join("src/Post.php"),
        "<?php\nnamespace App;\n\nuse Acme\\Greeter;\nuse Illuminate\\Database\\Eloquent\\Model;\n\nclass Post extends Model implements Greeter\n{\n    protected $casts = ['views' => 'integer'];\n\n    public function greet(string $name): string { return $name; }\n\n    public function views(): string { return $this->views; }\n}\n\nreadonly class Point { public function __construct(public int $x) {} }\n",
    );
    write(&root.join("src/main.php"), MAIN);
    write(&root.join("src/messy.php"), MESSY);
}

/// The line and UTF-16 column of the `nth` `needle` in `text`, plus `offset` characters.
fn at(text: &str, needle: &str, nth: usize, offset: u32) -> Value {
    let i = text.match_indices(needle).nth(nth).unwrap().0;
    let before = &text[..i];
    let line = before.matches('\n').count();
    let col = before.rsplit('\n').next().unwrap().encode_utf16().count() as u32;
    json!({"line": line, "character": col + offset})
}

fn capabilities() -> Value {
    json!({
        "textDocument": {
            "codeAction": {
                "dataSupport": true,
                "resolveSupport": {"properties": ["edit"]},
                "codeActionLiteralSupport": {"codeActionKind": {"valueSet": ["quickfix", "refactor", "source"]}}
            }
        }
    })
}

/// Everything published, latest per file, once the server has settled: once it's published for
/// `doc`, and nothing more has come for a while.
async fn settled(c: &mut Client, doc: &str) -> BTreeMap<String, Value> {
    let first = c.diagnostics(doc).await;
    c.backlog.push(json!({"method": "textDocument/publishDiagnostics", "params": {"uri": doc, "diagnostics": first}}));
    loop {
        let before = c.backlog.len();
        c.pump(Duration::from_millis(1000)).await;
        if c.backlog.len() == before {
            break;
        }
    }
    let mut out = BTreeMap::new();
    for m in std::mem::take(&mut c.backlog) {
        if m["method"] == "textDocument/publishDiagnostics" {
            out.insert(
                m["params"]["uri"].as_str().unwrap().to_owned(),
                m["params"]["diagnostics"].clone(),
            );
        }
    }
    out
}

/// Asks, and keeps the answer.
async fn ask(
    c: &mut Client,
    seen: &mut Vec<(String, Value)>,
    label: &str,
    method: &str,
    params: Value,
) -> Value {
    let result = c.request(method, params).await;
    seen.push((label.to_owned(), result.clone()));
    result
}

/// What a client sees of the session: answers in order, and diagnostics at a few points.
async fn script(mut c: Client, root: &Path) -> Vec<(String, Value)> {
    let main = root.join("src/main.php");
    let doc = uri(&main);
    let shape_doc = uri(&root.join("src/Shapes/Shape.php"));
    let messy_doc = uri(&root.join("src/messy.php"));
    let mut seen: Vec<(String, Value)> = Vec::new();
    let init = c
        .request(
            "initialize",
            json!({"processId": null, "rootUri": uri(root), "capabilities": capabilities()}),
        )
        .await;
    seen.push(("initialize".into(), init["capabilities"].clone()));
    c.notify("initialized", json!({})).await;
    c.notify(
        "textDocument/didOpen",
        json!({"textDocument": {"uri": doc, "languageId": "php", "version": 1, "text": MAIN}}),
    )
    .await;
    let diags = settled(&mut c, &doc).await;
    seen.push(("diagnostics on open".into(), json!(diags)));

    let pos = |needle: &str, nth: usize, offset: u32| json!({"textDocument": {"uri": doc}, "position": at(MAIN, needle, nth, offset)});
    ask(
        &mut c,
        &mut seen,
        "hover total",
        "textDocument/hover",
        pos("total(...", 0, 1),
    )
    .await;
    ask(
        &mut c,
        &mut seen,
        "hover strlen",
        "textDocument/hover",
        pos("strlen", 0, 1),
    )
    .await;
    ask(
        &mut c,
        &mut seen,
        "definition Circle",
        "textDocument/definition",
        pos("Circle(2", 0, 1),
    )
    .await;
    ask(
        &mut c,
        &mut seen,
        "definition strlen",
        "textDocument/definition",
        pos("strlen", 0, 1),
    )
    .await;
    ask(
        &mut c,
        &mut seen,
        "declaration area",
        "textDocument/declaration",
        pos("area()", 0, 1),
    )
    .await;
    ask(
        &mut c,
        &mut seen,
        "type definition",
        "textDocument/typeDefinition",
        pos("$shape->", 0, 1),
    )
    .await;
    let shape =
        json!({"textDocument": {"uri": shape_doc}, "position": {"line": 3, "character": 12}});
    ask(
        &mut c,
        &mut seen,
        "implementation Shape",
        "textDocument/implementation",
        shape.clone(),
    )
    .await;
    let mut refs = pos("area()", 0, 1);
    refs["context"] = json!({"includeDeclaration": true});
    ask(
        &mut c,
        &mut seen,
        "references area",
        "textDocument/references",
        refs,
    )
    .await;
    ask(
        &mut c,
        &mut seen,
        "highlight $sum",
        "textDocument/documentHighlight",
        pos("$sum", 0, 1),
    )
    .await;
    ask(
        &mut c,
        &mut seen,
        "prepare rename",
        "textDocument/prepareRename",
        pos("area()", 0, 1),
    )
    .await;
    let mut rename = pos("area()", 0, 1);
    rename["newName"] = json!("surface");
    ask(
        &mut c,
        &mut seen,
        "rename area",
        "textDocument/rename",
        rename,
    )
    .await;
    ask(
        &mut c,
        &mut seen,
        "completion use",
        "textDocument/completion",
        pos("new Squ;", 0, 7),
    )
    .await;
    ask(
        &mut c,
        &mut seen,
        "signature help",
        "textDocument/signatureHelp",
        pos("total(...", 0, 6),
    )
    .await;
    let docid = json!({"textDocument": {"uri": doc}});
    ask(
        &mut c,
        &mut seen,
        "document symbols",
        "textDocument/documentSymbol",
        docid.clone(),
    )
    .await;
    ask(
        &mut c,
        &mut seen,
        "workspace symbols",
        "workspace/symbol",
        json!({"query": "Square"}),
    )
    .await;
    let mut hints = docid.clone();
    hints["range"] =
        json!({"start": {"line": 0, "character": 0}, "end": {"line": 30, "character": 0}});
    ask(
        &mut c,
        &mut seen,
        "inlay hints",
        "textDocument/inlayHint",
        hints,
    )
    .await;
    ask(
        &mut c,
        &mut seen,
        "semantic tokens",
        "textDocument/semanticTokens/full",
        docid.clone(),
    )
    .await;
    ask(
        &mut c,
        &mut seen,
        "folding",
        "textDocument/foldingRange",
        docid.clone(),
    )
    .await;
    let mut selection = docid.clone();
    selection["positions"] = json!([at(MAIN, "$shape->", 0, 3)]);
    ask(
        &mut c,
        &mut seen,
        "selection",
        "textDocument/selectionRange",
        selection,
    )
    .await;
    let calls = ask(
        &mut c,
        &mut seen,
        "call hierarchy",
        "textDocument/prepareCallHierarchy",
        pos("function total", 0, 10),
    )
    .await;
    ask(
        &mut c,
        &mut seen,
        "incoming calls",
        "callHierarchy/incomingCalls",
        json!({"item": calls[0]}),
    )
    .await;
    ask(
        &mut c,
        &mut seen,
        "outgoing calls",
        "callHierarchy/outgoingCalls",
        json!({"item": calls[0]}),
    )
    .await;
    let types = ask(
        &mut c,
        &mut seen,
        "type hierarchy",
        "textDocument/prepareTypeHierarchy",
        pos("Circle(2", 0, 1),
    )
    .await;
    ask(
        &mut c,
        &mut seen,
        "supertypes",
        "typeHierarchy/supertypes",
        json!({"item": types[0]}),
    )
    .await;
    ask(
        &mut c,
        &mut seen,
        "subtypes",
        "typeHierarchy/subtypes",
        json!({"item": types[0]}),
    )
    .await;
    let format = json!({"textDocument": {"uri": messy_doc}, "options": {"tabSize": 4, "insertSpaces": true}});
    ask(
        &mut c,
        &mut seen,
        "formatting",
        "textDocument/formatting",
        format,
    )
    .await;
    let range_format = json!({"textDocument": {"uri": messy_doc}, "options": {"tabSize": 4, "insertSpaces": true},
        "range": {"start": {"line": 3, "character": 0}, "end": {"line": 4, "character": 0}}});
    ask(
        &mut c,
        &mut seen,
        "range formatting",
        "textDocument/rangeFormatting",
        range_format,
    )
    .await;

    // a quick fix for what's published, resolved
    let published = diags
        .get(&doc)
        .unwrap_or_else(|| panic!("{doc} not in {:?}", diags.keys()))
        .as_array()
        .unwrap()
        .clone();
    let square = published
        .iter()
        .find(|d| d["code"] == "class.notFound")
        .unwrap_or_else(|| panic!("{published:?}"))
        .clone();
    let actions = ask(
        &mut c,
        &mut seen,
        "code actions",
        "textDocument/codeAction",
        json!({"textDocument": {"uri": doc}, "range": square["range"], "context": {"diagnostics": [square]}}),
    )
    .await;
    let import = actions
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["kind"] == "quickfix")
        .unwrap_or_else(|| panic!("{actions}"))
        .clone();
    ask(&mut c, &mut seen, "resolve", "codeAction/resolve", import).await;

    // an edit: the error goes, another comes
    let missing = at(MAIN, "missing_function", 0, 0);
    let end =
        json!({"line": missing["line"], "character": missing["character"].as_u64().unwrap() + 16});
    c.notify(
        "textDocument/didChange",
        json!({"textDocument": {"uri": doc, "version": 2},
               "contentChanges": [{"range": {"start": missing, "end": end}, "text": "total"}]}),
    )
    .await;
    seen.push((
        "diagnostics after an edit".into(),
        json!(settled(&mut c, &doc).await),
    ));

    // a file that appears, and a newer PHP
    let added = root.join("src/Triangle.php");
    write(
        &added,
        "<?php\nnamespace App\\Shapes;\n\nclass Triangle implements Shape\n{\n    public function area(): int { return 1; }\n}\n",
    );
    write(
        &root.join("composer.json"),
        r#"{
    "require": {"php": "^8.2", "laravel/framework": "^13", "acme/greeter": "^1"},
    "autoload": {"psr-4": {"App\\": "src/"}}
}"#,
    );
    c.notify(
        "workspace/didChangeWatchedFiles",
        json!({"changes": [{"uri": uri(&added), "type": 1}, {"uri": uri(&root.join("composer.json")), "type": 2}]}),
    )
    .await;
    let post = uri(&root.join("src/Post.php"));
    seen.push((
        "diagnostics after files change".into(),
        json!(settled(&mut c, &post).await),
    ));
    ask(
        &mut c,
        &mut seen,
        "implementation Shape again",
        "textDocument/implementation",
        shape,
    )
    .await;
    seen
}

/// `value` with the native server's URIs of the builtin stubs as the browser's are.
fn stubs_as_in_the_browser(value: &Value) -> Value {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    let dir = base
        .join("stirphp")
        .join(env!("CARGO_PKG_VERSION"))
        .join("stubs");
    let native = format!("{}/", uri(&dir));
    let text = value.to_string().replace(&native, "stirphp:///stubs/");
    serde_json::from_str(&text).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_browser_session_answers_as_the_native_server_does() {
    let tmp = tempfile::TempDir::new().unwrap();
    let base = std::fs::canonicalize(tmp.path()).unwrap();
    let (native_root, browser_root) = (base.join("native"), base.join("browser"));
    // the same paths in both, for the same answers: the project's moved in between
    let root = base.join("project");
    project(&native_root);
    project(&browser_root);

    // the native server runs the rules once they're allowed; the page's session runs them
    let store = base.join("trust.json");
    let script_path = root.join("rules/no_var_dump.rhai");
    let hash = stir_lsp::trust::scripts_hash(&root, &[(script_path.as_path(), Ok(RULE))]);
    TrustStore::at(&store).remember(&root, &hash, true).unwrap();

    std::fs::rename(&native_root, &root).unwrap();
    let native = Client::start_with(TrustOptions {
        store: TrustStore::at(&store),
        answer_wait: Duration::from_secs(60),
    });
    let from_native = script(native, &root).await;
    std::fs::rename(&root, &native_root).unwrap();

    std::fs::rename(&browser_root, &root).unwrap();
    let from_browser = script(Client::start_session(), &root).await;

    assert_eq!(from_native.len(), from_browser.len());
    for ((label, native), (_, browser)) in from_native.iter().zip(&from_browser) {
        assert_eq!(
            stubs_as_in_the_browser(native),
            *browser,
            "{label}:\nnative:  {native}\nbrowser: {browser}"
        );
    }
    // and they're about something
    let answers: BTreeMap<&str, &Value> =
        from_browser.iter().map(|(l, v)| (l.as_str(), v)).collect();
    let open = answers["diagnostics on open"].to_string();
    for code in [
        "function.notFound",
        "class.notFound",
        "app.noVarDump",
        "return.type",
    ] {
        assert!(open.contains(code), "{code}: {open}");
    }
    assert!(
        answers["definition strlen"]["uri"]
            .as_str()
            .unwrap()
            .starts_with("stirphp:///stubs/")
    );
    // the interface's and the call's
    assert_eq!(
        answers["rename area"]["changes"].as_object().unwrap().len(),
        2
    );
    assert_eq!(answers["implementation Shape"].as_array().unwrap().len(), 2);
    assert_eq!(
        answers["implementation Shape again"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    let completion = answers["completion use"].to_string();
    assert!(
        completion.contains("use App\\\\Shapes\\\\Square;"),
        "{completion}"
    );
    assert!(
        answers["resolve"]["edit"].is_object(),
        "{}",
        answers["resolve"]
    );
    assert!(!answers["formatting"].as_array().unwrap().is_empty());
    let after = answers["diagnostics after files change"].to_string();
    assert!(after.contains("Triangle"), "{after}");
}
