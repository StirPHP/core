//! The server spoken to over JSON-RPC, as an editor would.

mod client;

use std::path::Path;
use std::time::Duration;

use client::{Client, uri, write};
use serde_json::{Value, json};
use stir_lsp::server::TrustOptions;
use stir_lsp::trust::TrustStore;

#[tokio::test(flavor = "multi_thread")]
async fn editing_a_php_file() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    write(
        &root.join("stirphp.neon"),
        "parameters:\n    stirphp:\n        srcDir: src\n",
    );
    let lib = root.join("src/lib.php");
    write(
        &lib,
        "<?php\nnamespace App;\n\nfunction greet(string $name): string { return $name; }\n",
    );
    let main = root.join("src/main.php");
    let main_text =
        "<?php\nnamespace App;\n\n$msg = greet('x');\necho missing_fn();\necho strlen($msg);\n";
    write(&main, main_text);
    let main_uri = uri(&main);

    let mut c = Client::start();
    c.request(
        "initialize",
        json!({"processId": null, "rootUri": uri(&root), "capabilities": {}}),
    )
    .await;
    c.notify("initialized", json!({})).await;
    c.notify(
        "textDocument/didOpen",
        json!({"textDocument": {"uri": main_uri, "languageId": "php", "version": 1, "text": main_text}}),
    )
    .await;

    let diags = c.diagnostics(&main_uri).await;
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0]["code"], "function.notFound");
    assert_eq!(diags[0]["range"]["start"]["line"], 4);

    let hover = c
        .request(
            "textDocument/hover",
            json!({"textDocument": {"uri": main_uri}, "position": {"line": 3, "character": 8}}),
        )
        .await;
    let text = hover["contents"]["value"].as_str().unwrap();
    assert!(text.contains("function greet"), "{text}");

    let def = c
        .request(
            "textDocument/definition",
            json!({"textDocument": {"uri": main_uri}, "position": {"line": 3, "character": 8}}),
        )
        .await;
    assert_eq!(def["uri"], uri(&lib));

    // rename a local
    let edit = c
        .request(
            "textDocument/rename",
            json!({"textDocument": {"uri": main_uri}, "position": {"line": 3, "character": 1},
                   "newName": "$renamed"}),
        )
        .await;
    let changes = edit["changes"][&main_uri].as_array().unwrap();
    assert_eq!(changes.len(), 2, "{changes:?}");

    // rename `greet` everywhere
    let edit = c
        .request(
            "textDocument/rename",
            json!({"textDocument": {"uri": main_uri}, "position": {"line": 3, "character": 8}, "newName": "welcome"}),
        )
        .await;
    let changes = edit["changes"].as_object().unwrap();
    assert_eq!(changes.len(), 2, "{changes:?}");
    assert_eq!(changes[&main_uri][0]["newText"], "welcome");

    // fix the error with an incremental edit: `missing_fn` → `greet`
    c.notify(
        "textDocument/didChange",
        json!({
            "textDocument": {"uri": main_uri, "version": 2},
            "contentChanges": [{
                "range": {"start": {"line": 4, "character": 5}, "end": {"line": 4, "character": 17}},
                "text": "greet('y')"
            }]
        }),
    )
    .await;
    let diags = c.diagnostics(&main_uri).await;
    assert!(diags.is_empty(), "{diags:?}");

    // completion after `->`
    c.notify(
        "textDocument/didChange",
        json!({
            "textDocument": {"uri": main_uri, "version": 3},
            "contentChanges": [{"text": "<?php\nnamespace App;\n\nclass Box { public function open(): void {} }\n$b = new Box();\n$b->\n"}]
        }),
    )
    .await;
    let items = c
        .request(
            "textDocument/completion",
            json!({"textDocument": {"uri": main_uri}, "position": {"line": 5, "character": 4}}),
        )
        .await;
    let labels: Vec<&str> = items["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["label"].as_str().unwrap())
        .collect();
    assert!(labels.contains(&"open"), "{labels:?}");

    // semantic tokens are produced
    let tokens = c
        .request(
            "textDocument/semanticTokens/full",
            json!({"textDocument": {"uri": main_uri}}),
        )
        .await;
    assert!(!tokens["data"].as_array().unwrap().is_empty());

    let _ = c.request("shutdown", Value::Null).await;
}

impl Client {
    /// The `window/showMessage` notifications seen so far, without waiting for more.
    fn shown_messages(&self) -> Vec<String> {
        self.backlog
            .iter()
            .filter(|m| m["method"] == "window/showMessage")
            .map(|m| m["params"]["message"].as_str().unwrap().to_owned())
            .collect()
    }

    /// Everything until a round trip, which the server answers after it's done loading.
    async fn drain(&mut self, uri: &str) {
        let _ = self
            .request(
                "textDocument/documentSymbol",
                json!({"textDocument": {"uri": uri}}),
            )
            .await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn composer_install_state() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    write(
        &root.join("composer.json"),
        r#"{"require": {"acme/lib": "^1"}, "autoload": {"psr-4": {"App\\": "src/"}}}"#,
    );
    let main = root.join("src/main.php");
    let main_text = "<?php\nnamespace App;\n\nuse Acme\\Foo;\n\nnew Foo();\nnew Missing();\n";
    write(&main, main_text);
    let main_uri = uri(&main);

    let mut c = Client::start();
    c.request(
        "initialize",
        json!({"processId": null, "rootUri": uri(&root), "capabilities": {
            "workspace": {"didChangeWatchedFiles": {"dynamicRegistration": true}}
        }}),
    )
    .await;
    c.notify("initialized", json!({})).await;

    // the server asks to watch what says whether packages are installed
    let registration = loop {
        let m = c.next().await;
        if m["method"] == "client/registerCapability" {
            break m;
        }
        c.backlog.push(m);
    };
    let globs =
        registration["params"]["registrations"][0]["registerOptions"]["watchers"].to_string();
    assert!(globs.contains("composer.lock"), "{globs}");
    assert!(globs.contains("composer/installed.json"), "{globs}");
    c.send(json!({"jsonrpc": "2.0", "id": registration["id"], "result": null}))
        .await;

    // one notice, and no error about the package's class
    let diags = c.diagnostics(&main_uri).await;
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0]["code"], "class.notFound");
    assert_eq!(diags[0]["range"]["start"]["line"], 6);
    c.drain(&main_uri).await;
    let shown = c.shown_messages();
    assert_eq!(shown.len(), 1, "{shown:?}");
    assert!(shown[0].contains("composer install"), "{shown:?}");

    // once it's installed, what the package doesn't declare is an error
    write(
        &root.join("vendor/composer/installed.json"),
        r#"{"packages": [{"name": "acme/lib", "version": "1.0.0", "install-path": "../acme/lib",
            "autoload": {"psr-4": {"Acme\\": "src/"}}}], "dev": true}"#,
    );
    write(
        &root.join("vendor/acme/lib/src/Other.php"),
        "<?php\nnamespace Acme;\nclass Other {}\n",
    );
    c.notify(
        "workspace/didChangeWatchedFiles",
        json!({"changes": [{"uri": uri(&root.join("vendor/composer/installed.json")), "type": 1}]}),
    )
    .await;
    let diags = c.diagnostics(&main_uri).await;
    assert_eq!(diags.len(), 2, "{diags:?}");
    c.drain(&main_uri).await;
    assert_eq!(c.shown_messages().len(), 1);

    // a lock that disagrees says so, once more
    write(
        &root.join("composer.lock"),
        r#"{"packages": [{"name": "acme/lib", "version": "1.1.0"}]}"#,
    );
    c.notify(
        "workspace/didChangeWatchedFiles",
        json!({"changes": [{"uri": uri(&root.join("composer.lock")), "type": 1}]}),
    )
    .await;
    let diags = c.diagnostics(&main_uri).await;
    assert_eq!(diags.len(), 1, "{diags:?}");
    c.drain(&main_uri).await;
    let shown = c.shown_messages();
    assert_eq!(shown.len(), 2, "{shown:?}");
    assert!(shown[1].contains("1.1.0"), "{shown:?}");

    let _ = c.request("shutdown", Value::Null).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_editor_follows_the_php_version() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    write(
        &root.join("composer.json"),
        r#"{"require": {"php": "^8.0"}}"#,
    );
    let main = root.join("main.php");
    let main_text = "<?php\necho str_contains('a', 'b');\necho array_is_list([]);\n";
    write(&main, main_text);
    let main_uri = uri(&main);

    let mut c = Client::start();
    c.request(
        "initialize",
        json!({"processId": null, "rootUri": uri(&root), "capabilities": {}}),
    )
    .await;
    c.notify("initialized", json!({})).await;
    let diags = c.diagnostics(&main_uri).await;
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0]["code"], "SPHP0061");
    assert!(diags[0]["message"].as_str().unwrap().contains("PHP 8.1"));

    let hover = c
        .request(
            "textDocument/hover",
            json!({"textDocument": {"uri": main_uri}, "position": {"line": 1, "character": 8}}),
        )
        .await;
    assert!(
        hover["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("str_contains")
    );
    let hover = c
        .request(
            "textDocument/hover",
            json!({"textDocument": {"uri": main_uri}, "position": {"line": 2, "character": 8}}),
        )
        .await;
    assert!(hover.is_null(), "{hover}");

    c.notify(
        "textDocument/didOpen",
        json!({"textDocument": {"uri": main_uri, "languageId": "php", "version": 1,
                                "text": "<?php\necho array_is\n"}}),
    )
    .await;
    let items = c
        .request(
            "textDocument/completion",
            json!({"textDocument": {"uri": main_uri}, "position": {"line": 1, "character": 13}}),
        )
        .await;
    let labels: Vec<&str> = items["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["label"].as_str().unwrap())
        .collect();
    assert!(!labels.contains(&"array_is_list"), "{labels:?}");

    let _ = c.request("shutdown", Value::Null).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn baselines_and_ignore_rules_apply_as_in_the_cli() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    write(
        &root.join("stirphp.neon"),
        "parameters:
    stirphp:
        baseline: base.neon
    ignoreErrors:
        - '#a message alone#'
",
    );
    write(
        &root.join("base.neon"),
        "parameters:
    ignoreErrors:
        -
            identifier: function.notFound
            count: 1
            path: main.php
",
    );
    let main = root.join("main.php");
    write(&main, "<?php\nmissing_one();\nmissing_two();\n");
    let main_uri = uri(&main);

    let mut c = Client::start();
    c.request(
        "initialize",
        json!({"processId": null, "rootUri": uri(&root), "capabilities": {}}),
    )
    .await;
    c.notify("initialized", json!({})).await;

    // the first is in the baseline, and the count's used up after it
    let diags = c.diagnostics(&main_uri).await;
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0]["code"], "function.notFound");
    assert_eq!(diags[0]["range"]["start"]["line"], 2);
    c.drain(&main_uri).await;
    let shown = c.shown_messages();
    assert_eq!(shown.len(), 1, "{shown:?}");
    assert!(
        shown[0].contains("Skipped 1 `ignoreErrors` entry"),
        "{shown:?}"
    );

    let _ = c.request("shutdown", Value::Null).await;
}

const NO_ECHO: &str = r#"fn visit_echo(node, ctx) { ctx.error("no echo", "app.noEcho"); }"#;

/// Waits for a `publishDiagnostics` for `uri` with `count` diagnostics of the code `code`.
async fn diagnostics_settle(c: &mut Client, uri: &str, code: &str, count: usize) -> Vec<Value> {
    for _ in 0..20 {
        let diags = c.diagnostics(uri).await;
        let found = diags.iter().filter(|d| d["code"] == code).count();
        if found == count {
            return diags;
        }
    }
    panic!("never got {count} of {code} for {uri}");
}

/// A project with the rule `NO_ECHO`, opened in a new server. Returns the file with an `echo`.
async fn open_with_rules(
    root: &Path,
    trust: TrustOptions,
    capabilities: Value,
    options: Value,
) -> (Client, String) {
    write(
        &root.join("stirphp.neon"),
        "parameters:\n    stirphp:\n        rules:\n            - rules/no_echo.rhai\n",
    );
    if !root.join("rules/no_echo.rhai").is_file() {
        write(&root.join("rules/no_echo.rhai"), NO_ECHO);
    }
    let main = root.join("main.php");
    let text = "<?php\necho 1;\n";
    write(&main, text);
    let main_uri = uri(&main);
    let mut c = Client::start_with(trust);
    c.request(
        "initialize",
        json!({"processId": null, "rootUri": uri(root), "capabilities": capabilities, "initializationOptions": options}),
    )
    .await;
    c.notify("initialized", json!({})).await;
    c.notify(
        "textDocument/didOpen",
        json!({"textDocument": {"uri": main_uri, "languageId": "php", "version": 1, "text": text}}),
    )
    .await;
    (c, main_uri)
}

fn asking_client() -> Value {
    json!({"window": {"showMessage": {}}})
}

fn trust_options(store: &Path) -> TrustOptions {
    TrustOptions {
        store: TrustStore::at(store),
        answer_wait: Duration::from_secs(60),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn rules_wait_for_an_answer_and_it_is_remembered() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap().join("project");
    std::fs::create_dir_all(&root).unwrap();
    let store = tmp.path().join("config/trust.json");

    let (mut c, main) =
        open_with_rules(&root, trust_options(&store), asking_client(), json!({})).await;
    // nothing runs before the answer
    let question = c.message("window/showMessageRequest").await;
    let text = question["params"]["message"].as_str().unwrap();
    assert!(
        text.contains("custom rule") && text.contains("Run"),
        "{text}"
    );
    let titles: Vec<&str> = question["params"]["actions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["Allow", "Don't allow"]);
    assert!(c.diagnostics(&main).await.is_empty());

    c.respond(&question, json!({"title": "Allow"})).await;
    let diags = diagnostics_settle(&mut c, &main, "app.noEcho", 1).await;
    assert_eq!(diags[0]["message"], "no echo");
    assert_eq!(diags[0]["range"]["start"]["line"], 1);

    // a new session doesn't ask again
    let (mut c, main) =
        open_with_rules(&root, trust_options(&store), asking_client(), json!({})).await;
    diagnostics_settle(&mut c, &main, "app.noEcho", 1).await;
    assert!(
        !c.backlog
            .iter()
            .any(|m| m["method"] == "window/showMessageRequest"),
        "{:?}",
        c.backlog
    );

    // changed scripts are other scripts: they stop, and it asks again
    write(
        &root.join("rules/no_echo.rhai"),
        &NO_ECHO.replace("no echo", "not this"),
    );
    c.notify(
        "workspace/didChangeWatchedFiles",
        json!({"changes": [{"uri": uri(&root.join("rules/no_echo.rhai")), "type": 2}]}),
    )
    .await;
    let question = c.message("window/showMessageRequest").await;
    diagnostics_settle(&mut c, &main, "app.noEcho", 0).await;
    c.respond(&question, json!({"title": "Allow"})).await;
    let diags = diagnostics_settle(&mut c, &main, "app.noEcho", 1).await;
    assert_eq!(diags[0]["message"], "not this");
}

#[tokio::test(flavor = "multi_thread")]
async fn rules_stay_off_when_refused_and_it_is_remembered() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap().join("project");
    std::fs::create_dir_all(&root).unwrap();
    let store = tmp.path().join("trust.json");

    let (mut c, main) =
        open_with_rules(&root, trust_options(&store), asking_client(), json!({})).await;
    let question = c.message("window/showMessageRequest").await;
    c.respond(&question, json!({"title": "Don't allow"})).await;
    assert!(c.diagnostics(&main).await.is_empty());

    // the answer has been kept by the next session, which has nothing to ask
    for _ in 0..100 {
        if store.is_file() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let (mut c, main) =
        open_with_rules(&root, trust_options(&store), asking_client(), json!({})).await;
    assert!(c.diagnostics(&main).await.is_empty());
    let hover = c
        .request(
            "textDocument/hover",
            json!({"textDocument": {"uri": main}, "position": {"line": 0, "character": 0}}),
        )
        .await;
    assert!(hover.is_null());
    assert!(
        !c.backlog
            .iter()
            .any(|m| m["method"] == "window/showMessageRequest"),
        "{:?}",
        c.backlog
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn rules_never_run_in_a_workspace_the_editor_does_not_trust() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap().join("project");
    std::fs::create_dir_all(&root).unwrap();
    let store = tmp.path().join("trust.json");
    // even with an answer on file
    let hash = trust_hash(&root);
    TrustStore::at(&store).remember(&root, &hash, true).unwrap();

    let (mut c, main) = open_with_rules(
        &root,
        trust_options(&store),
        asking_client(),
        json!({"workspaceTrusted": false}),
    )
    .await;
    assert!(c.diagnostics(&main).await.is_empty());
    assert!(
        !c.backlog
            .iter()
            .any(|m| m["method"] == "window/showMessageRequest"),
        "{:?}",
        c.backlog
    );
    // a trusted workspace with that answer runs them
    let (mut c, main) = open_with_rules(
        &root,
        trust_options(&store),
        asking_client(),
        json!({"workspaceTrusted": true}),
    )
    .await;
    diagnostics_settle(&mut c, &main, "app.noEcho", 1).await;
}

fn trust_hash(root: &Path) -> String {
    let script = root.join("rules/no_echo.rhai");
    write(&script, NO_ECHO);
    stir_lsp::trust::scripts_hash(root, &[(script.as_path(), Ok(NO_ECHO))])
}

#[tokio::test(flavor = "multi_thread")]
async fn rules_stay_off_with_one_notice_when_nobody_can_answer() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap().join("project");
    std::fs::create_dir_all(&root).unwrap();
    let store = tmp.path().join("trust.json");

    // a client that can't show questions
    let (mut c, main) = open_with_rules(&root, trust_options(&store), json!({}), json!({})).await;
    let notice = c.message("window/showMessage").await;
    assert!(
        notice["params"]["message"]
            .as_str()
            .unwrap()
            .contains("aren't running")
    );
    assert!(c.diagnostics(&main).await.is_empty());

    // a reload doesn't say it again
    c.notify(
        "workspace/didChangeWatchedFiles",
        json!({"changes": [{"uri": uri(&root.join("stirphp.neon")), "type": 2}]}),
    )
    .await;
    c.request(
        "textDocument/hover",
        json!({"textDocument": {"uri": main}, "position": {"line": 0, "character": 0}}),
    )
    .await;
    assert!(
        !c.backlog
            .iter()
            .any(|m| m["method"] == "window/showMessage"),
        "{:?}",
        c.backlog
    );

    // a client that says it can, and doesn't answer
    let quick = TrustOptions {
        store: TrustStore::at(&store),
        answer_wait: Duration::from_millis(100),
    };
    let (mut c, main) = open_with_rules(&root, quick, asking_client(), json!({})).await;
    let question = c.message("window/showMessageRequest").await;
    let notice = c.message("window/showMessage").await;
    assert!(
        notice["params"]["message"]
            .as_str()
            .unwrap()
            .contains("aren't running")
    );
    assert!(c.diagnostics(&main).await.is_empty());
    // a late answer still counts
    c.respond(&question, json!({"title": "Allow"})).await;
    diagnostics_settle(&mut c, &main, "app.noEcho", 1).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn what_is_wrong_with_a_script_is_said_at_the_script() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap().join("project");
    std::fs::create_dir_all(&root).unwrap();
    write(
        &root.join("rules/no_echo.rhai"),
        "fn visit_echo(node, ctx) { loop {} }",
    );
    let store = tmp.path().join("trust.json");
    let (mut c, _) =
        open_with_rules(&root, trust_options(&store), asking_client(), json!({})).await;
    let question = c.message("window/showMessageRequest").await;
    c.respond(&question, json!({"title": "Allow"})).await;
    let script = uri(&root.join("rules/no_echo.rhai"));
    let diags = c.diagnostics(&script).await;
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0]["code"], "SPHP0070");
    assert!(
        diags[0]["message"]
            .as_str()
            .unwrap()
            .contains("no_echo.rhai")
    );
    // and it's gone when the script's fixed, and allowed
    write(&root.join("rules/no_echo.rhai"), NO_ECHO);
    c.notify(
        "workspace/didChangeWatchedFiles",
        json!({"changes": [{"uri": script, "type": 2}]}),
    )
    .await;
    let question = c.message("window/showMessageRequest").await;
    c.respond(&question, json!({"title": "Allow"})).await;
    assert!(c.diagnostics(&script).await.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn hooks_and_magic_members_reach_hover_and_completion() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap().join("project");
    std::fs::create_dir_all(&root).unwrap();
    write(
        &root.join("rules/no_echo.rhai"),
        r#"
        fn return_type_targets() { #{ methods: ["Container::get"] } }
        fn return_type_of_method(node, ctx) { ctx.get_type(node.arguments[0]).class_string }
        fn magic_members(class_name) {
            if class_name == "Model" {
                #{ properties: #{ title: "string" }, methods: #{ publish: "bool" } }
            }
        }
        "#,
    );
    let store = tmp.path().join("trust.json");
    let (mut c, _) =
        open_with_rules(&root, trust_options(&store), asking_client(), json!({})).await;
    let question = c.message("window/showMessageRequest").await;
    let main = root.join("main.php");
    let text = "<?php\nclass Model {}\nclass Post extends Model {}\nclass Container { public function get(string $id): mixed { return null; } }\n$post = (new Container())->get(Post::class);\n$post->title;\n$post->publish();\n$post->\n";
    let main_uri = uri(&main);
    c.notify(
        "textDocument/didChange",
        json!({"textDocument": {"uri": main_uri, "version": 2}, "contentChanges": [{"text": text}]}),
    )
    .await;
    let hover = |line: u32, character: u32| json!({"textDocument": {"uri": main_uri}, "position": {"line": line, "character": character}});
    let complete_at =
        json!({"textDocument": {"uri": main_uri}, "position": {"line": 7, "character": 7}});

    // without the answer, they aren't there
    let got = c.request("textDocument/hover", hover(5, 1)).await;
    let shown = got["contents"]["value"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(!shown.contains("Post"), "{shown}");

    c.respond(&question, json!({"title": "Allow"})).await;
    let mut shown = String::new();
    for _ in 0..200 {
        let got = c.request("textDocument/hover", hover(5, 1)).await;
        shown = got["contents"]["value"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        if shown.contains("Post") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(shown.contains("Post"), "{shown}");

    // a return type hook, then members
    let got = c.request("textDocument/hover", hover(5, 8)).await;
    let shown = got["contents"]["value"].as_str().unwrap().to_owned();
    assert!(
        shown.contains("string") && shown.contains("title"),
        "{shown}"
    );
    let got = c.request("textDocument/hover", hover(6, 8)).await;
    let shown = got["contents"]["value"].as_str().unwrap().to_owned();
    assert!(
        shown.contains("bool") && shown.contains("publish"),
        "{shown}"
    );

    let got = c.request("textDocument/completion", complete_at).await;
    let items = got["items"]
        .as_array()
        .cloned()
        .or(got.as_array().cloned())
        .unwrap();
    let labels: Vec<&str> = items.iter().map(|i| i["label"].as_str().unwrap()).collect();
    assert!(labels.contains(&"title"), "{labels:?}");
    assert!(labels.contains(&"publish"), "{labels:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn implementations_and_hierarchies() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    write(
        &root.join("stirphp.neon"),
        "parameters:\n    stirphp:\n        srcDir: src\n",
    );
    let path = root.join("src/a.php");
    let text = "<?php\ninterface Job { public function run(): void; }\nclass Work implements Job { public function run(): void { helper(); } }\nfunction helper(): void {}\nfunction go(Job $j): void { $j->run(); }\n";
    write(&path, text);
    let doc = uri(&path);

    let mut c = Client::start();
    let init = c
        .request(
            "initialize",
            json!({"processId": null, "rootUri": uri(&root), "capabilities": {
                "textDocument": {"typeHierarchy": {"dynamicRegistration": true}}
            }}),
        )
        .await;
    assert_eq!(init["capabilities"]["implementationProvider"], true);
    assert_eq!(init["capabilities"]["declarationProvider"], true);
    assert_eq!(init["capabilities"]["callHierarchyProvider"], true);
    c.notify("initialized", json!({})).await;
    let registration = c.message("client/registerCapability").await;
    assert_eq!(
        registration["params"]["registrations"][0]["method"],
        "textDocument/prepareTypeHierarchy"
    );
    c.respond(&registration, json!(null)).await;
    c.notify(
        "textDocument/didOpen",
        json!({"textDocument": {"uri": doc, "languageId": "php", "version": 1, "text": text}}),
    )
    .await;

    let at = |line: u32, character: u32| json!({"textDocument": {"uri": doc}, "position": {"line": line, "character": character}});
    let impls = c.request("textDocument/implementation", at(1, 12)).await;
    assert_eq!(impls[0]["range"]["start"]["line"], 2, "{impls}");
    assert_eq!(impls[0]["uri"], json!(doc));

    let decl = c.request("textDocument/declaration", at(2, 45)).await;
    assert_eq!(decl["range"]["start"]["line"], 1, "{decl}");

    let prepared = c
        .request("textDocument/prepareCallHierarchy", at(2, 45))
        .await;
    let item = prepared[0].clone();
    assert_eq!(item["name"], "run");
    let incoming = c
        .request("callHierarchy/incomingCalls", json!({"item": item}))
        .await;
    assert_eq!(incoming[0]["from"]["name"], "go", "{incoming}");
    assert_eq!(incoming[0]["fromRanges"][0]["start"]["line"], 4);
    let outgoing = c
        .request("callHierarchy/outgoingCalls", json!({"item": item}))
        .await;
    assert_eq!(outgoing[0]["to"]["name"], "helper", "{outgoing}");

    let types = c
        .request("textDocument/prepareTypeHierarchy", at(2, 8))
        .await;
    let item = types[0].clone();
    assert_eq!(item["name"], "Work");
    let up = c
        .request("typeHierarchy/supertypes", json!({"item": item}))
        .await;
    assert_eq!(up[0]["name"], "Job", "{up}");
    let down = c
        .request("typeHierarchy/subtypes", json!({"item": up[0]}))
        .await;
    assert_eq!(down[0]["name"], "Work", "{down}");
}

#[tokio::test(flavor = "multi_thread")]
async fn completion_adds_a_use_line() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    write(
        &root.join("stirphp.neon"),
        "parameters:\n    stirphp:\n        srcDir: src\n",
    );
    write(
        &root.join("src/lib.php"),
        "<?php\nnamespace Lib;\nclass Widget {}\n",
    );
    let path = root.join("src/a.php");
    let text = "<?php\nnamespace App;\n\nuse Alpha\\A;\nuse Zed\\Z;\n\nnew Widg\n";
    write(&path, text);
    let doc = uri(&path);

    let mut c = Client::start();
    c.request(
        "initialize",
        json!({"processId": null, "rootUri": uri(&root), "capabilities": {}}),
    )
    .await;
    c.notify("initialized", json!({})).await;
    c.notify(
        "textDocument/didOpen",
        json!({"textDocument": {"uri": doc, "languageId": "php", "version": 1, "text": text}}),
    )
    .await;
    let list = c
        .request(
            "textDocument/completion",
            json!({"textDocument": {"uri": doc}, "position": {"line": 6, "character": 8}}),
        )
        .await;
    let items = list["items"].as_array().unwrap();
    let widget = items.iter().find(|i| i["label"] == "Widget").unwrap();
    assert_eq!(widget["labelDetails"]["description"], "Lib\\Widget");
    assert_eq!(widget["textEdit"]["newText"], "Widget");
    let edit = &widget["additionalTextEdits"][0];
    assert_eq!(edit["newText"], "use Lib\\Widget;\n");
    assert_eq!(edit["range"]["start"], json!({"line": 4, "character": 0}));
    assert_eq!(edit["range"]["end"], json!({"line": 4, "character": 0}));
}

#[tokio::test(flavor = "multi_thread")]
async fn formatting_a_document_and_a_range() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    write(&root.join("composer.json"), "{}");
    let path = root.join("a.php");
    let text = "<?php\n$a=1;\n$b=2;\n";
    write(&path, text);
    let doc = uri(&path);

    let mut c = Client::start();
    let init = c
        .request(
            "initialize",
            json!({"processId": null, "rootUri": uri(&root), "capabilities": {}}),
        )
        .await;
    assert_eq!(init["capabilities"]["documentFormattingProvider"], true);
    assert_eq!(
        init["capabilities"]["documentRangeFormattingProvider"],
        true
    );
    c.notify("initialized", json!({})).await;
    c.notify(
        "textDocument/didOpen",
        json!({"textDocument": {"uri": doc, "languageId": "php", "version": 1, "text": text}}),
    )
    .await;
    let options = json!({"tabSize": 4, "insertSpaces": true});
    let edits = c
        .request(
            "textDocument/formatting",
            json!({"textDocument": {"uri": doc}, "options": options}),
        )
        .await;
    let edits = edits.as_array().unwrap();
    assert_eq!(edits.len(), 4, "{edits:?}");
    assert_eq!(
        edits[0],
        json!({
            "range": {"start": {"line": 1, "character": 2}, "end": {"line": 1, "character": 2}},
            "newText": " "
        })
    );
    let edits = c
        .request(
            "textDocument/rangeFormatting",
            json!({
                "textDocument": {"uri": doc},
                "options": options,
                "range": {"start": {"line": 2, "character": 0}, "end": {"line": 2, "character": 5}}
            }),
        )
        .await;
    let edits = edits.as_array().unwrap();
    assert_eq!(edits.len(), 2, "{edits:?}");
    assert_eq!(edits[0]["range"]["start"]["line"], 2);
    // nothing for a file that doesn't parse
    c.notify(
        "textDocument/didChange",
        json!({"textDocument": {"uri": doc, "version": 2},
               "contentChanges": [{"text": "<?php\nif (\n"}]}),
    )
    .await;
    let edits = c
        .request(
            "textDocument/formatting",
            json!({"textDocument": {"uri": doc}, "options": options}),
        )
        .await;
    assert_eq!(edits, json!(null));
}

#[tokio::test(flavor = "multi_thread")]
async fn what_is_deprecated_is_struck_through() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    write(&root.join("composer.json"), "{}");
    let path = root.join("a.php");
    let text = "<?php\n/** @deprecated */\nfunction old(): void {}\nold();\n";
    write(&path, text);
    let doc = uri(&path);

    let mut c = Client::start();
    c.request(
        "initialize",
        json!({"processId": null, "rootUri": uri(&root), "capabilities": {}}),
    )
    .await;
    c.notify("initialized", json!({})).await;
    c.notify(
        "textDocument/didOpen",
        json!({"textDocument": {"uri": doc, "languageId": "php", "version": 1, "text": text}}),
    )
    .await;
    let diags = c.diagnostics(&doc).await;
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0]["code"], "function.deprecated");
    assert_eq!(diags[0]["severity"], 4, "a hint");
    assert_eq!(diags[0]["tags"], json!([2]), "the tag for deprecated");
    assert_eq!(diags[0]["range"]["start"]["line"], 3);
}

/// The position of `needle` in `text`, `at` characters into it.
fn position_of(text: &str, needle: &str, at: usize) -> Value {
    let start = text.find(needle).unwrap_or_else(|| panic!("no {needle}")) + at;
    let line = text[..start].matches('\n').count();
    let character = start - text[..start].rfind('\n').map_or(0, |n| n + 1);
    json!({"line": line, "character": character})
}

#[tokio::test(flavor = "multi_thread")]
async fn framework_magic_reaches_completion_hover_and_definition() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap().join("project");
    let fixtures =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../stir_checker/tests/fixtures/frameworks");
    let laravel = fixtures.join("laravel.multi");
    for file in [
        "vendor/laravel/eloquent.php",
        "vendor/laravel/support.php",
        "app/Models/Post.php",
        "app/Models/User.php",
        "database/migrations/2026_01_01_000000_create_posts_table.php",
    ] {
        write(
            &root.join(file.replace("vendor/laravel", "framework")),
            &std::fs::read_to_string(laravel.join(file)).unwrap(),
        );
    }
    write(
        &root.join("framework/wordpress.php"),
        &std::fs::read_to_string(fixtures.join("wordpress.multi/vendor/wordpress.php")).unwrap(),
    );
    // ours run without asking, even where the editor doesn't trust the workspace
    write(
        &root.join("stirphp.neon"),
        "parameters:\n    stirphp:\n        frameworks:\n            laravel: true\n            wordpress: true\n",
    );
    write(
        &root.join("routes/web.php"),
        "<?php\nRoute::get('/posts/{post}', fn () => 1)->name('posts.show');\n",
    );
    write(
        &root.join("resources/views/posts/show.blade.php"),
        "<h1>{{ $title }}</h1>\n",
    );
    write(
        &root.join("plugin.php"),
        "<?php\nfunction plugin_boot(): void { do_action('plugin_ready', 1); }\n",
    );
    let main = root.join("main.php");
    let text = "<?php\nuse App\\Models\\Post;\nfunction run(Post $post): void {\n    $post->views;\n    $post->author;\n    Post::published(2024);\n    route('posts.show');\n    view('posts.show');\n    add_action('plugin_ready', 'run');\n    Post::\n}\n";
    write(&main, text);
    let main_uri = uri(&main);
    let mut c = Client::start_with(trust_options(&tmp.path().join("trust.json")));
    c.request(
        "initialize",
        json!({"processId": null, "rootUri": uri(&root), "capabilities": asking_client(),
               "initializationOptions": {"workspaceTrusted": false}}),
    )
    .await;
    c.notify("initialized", json!({})).await;
    c.notify(
        "textDocument/didOpen",
        json!({"textDocument": {"uri": main_uri, "languageId": "php", "version": 1, "text": text}}),
    )
    .await;
    let at = |needle: &str, offset: usize| json!({"textDocument": {"uri": main_uri}, "position": position_of(text, needle, offset)});

    // a column from the migrations
    let mut shown = String::new();
    for _ in 0..200 {
        let got = c.request("textDocument/hover", at("views", 1)).await;
        shown = got["contents"]["value"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        if shown.contains("int") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(shown.contains("int $views"), "{shown}");

    // a relation's property leads to its method, a scope to its method
    let def = c.request("textDocument/definition", at("author", 1)).await;
    assert_eq!(def["uri"], uri(&root.join("app/Models/Post.php")), "{def}");
    let def = c
        .request("textDocument/definition", at("published", 1))
        .await;
    assert_eq!(def["uri"], uri(&root.join("app/Models/Post.php")), "{def}");
    let line = def["range"]["start"]["line"].as_u64().unwrap() as usize;
    let post = std::fs::read_to_string(root.join("app/Models/Post.php")).unwrap();
    assert!(
        post.lines().nth(line).unwrap().contains("scopePublished"),
        "{def}"
    );

    // names in strings
    let def = c
        .request("textDocument/definition", at("posts.show')", 1))
        .await;
    assert_eq!(def["uri"], uri(&root.join("routes/web.php")), "{def}");
    let def = c
        .request("textDocument/definition", at("view('posts", 7))
        .await;
    assert_eq!(
        def["uri"],
        uri(&root.join("resources/views/posts/show.blade.php")),
        "{def}"
    );
    let def = c
        .request("textDocument/definition", at("plugin_ready", 1))
        .await;
    assert_eq!(def["uri"], uri(&root.join("plugin.php")), "{def}");

    // scopes and what's forwarded to the query, statically
    let got = c
        .request("textDocument/completion", at("Post::\n", 6))
        .await;
    let items = got["items"]
        .as_array()
        .cloned()
        .or(got.as_array().cloned())
        .unwrap();
    let labels: Vec<&str> = items.iter().map(|i| i["label"].as_str().unwrap()).collect();
    assert!(labels.contains(&"published"), "{labels:?}");
    assert!(labels.contains(&"where"), "{labels:?}");
}
