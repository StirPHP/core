//! Code actions over the protocol: resolving, what a client without resolving gets, positions in
//! UTF-16, versions in workspace edits, and diagnostics going away afterwards.

mod client;

use std::path::Path;
use std::time::Duration;

use client::{Client, uri, write};
use serde_json::{Value, json};

/// What VS Code says it can do, more or less.
fn full_capabilities() -> Value {
    json!({
        "general": {"positionEncodings": ["utf-16"]},
        "textDocument": {
            "codeAction": {
                "dataSupport": true,
                "disabledSupport": true,
                "isPreferredSupport": true,
                "resolveSupport": {"properties": ["edit"]},
                "codeActionLiteralSupport": {"codeActionKind": {"valueSet": ["quickfix", "refactor", "source"]}},
            },
        },
        "workspace": {"workspaceEdit": {"documentChanges": true}},
    })
}

async fn start(root: &Path, capabilities: Value, options: Value) -> Client {
    let mut c = Client::start();
    c.request(
        "initialize",
        json!({"processId": null, "rootUri": uri(root), "capabilities": capabilities,
               "initializationOptions": options}),
    )
    .await;
    c.notify("initialized", json!({})).await;
    c
}

async fn open(c: &mut Client, path: &Path, version: i32, text: &str) {
    c.notify(
        "textDocument/didOpen",
        json!({"textDocument": {"uri": uri(path), "languageId": "php", "version": version, "text": text}}),
    )
    .await;
}

/// The offset of a position, in UTF-16 as the protocol has it.
fn offset_of(text: &str, position: &Value) -> usize {
    let line = position["line"].as_u64().unwrap() as usize;
    let character = position["character"].as_u64().unwrap() as usize;
    let start: usize = text.split_inclusive('\n').take(line).map(str::len).sum();
    let mut units = 0;
    for (i, c) in text[start..].char_indices() {
        if units >= character {
            return start + i;
        }
        units += c.len_utf16();
    }
    text.len()
}

fn apply_edits(text: &str, edits: &[Value]) -> String {
    let mut spans: Vec<(usize, usize, &str)> = edits
        .iter()
        .map(|e| {
            (
                offset_of(text, &e["range"]["start"]),
                offset_of(text, &e["range"]["end"]),
                e["newText"].as_str().unwrap(),
            )
        })
        .collect();
    spans.sort_by_key(|s| (s.0, s.1));
    let mut out = String::new();
    let mut at = 0;
    for (start, end, new) in spans {
        out.push_str(&text[at..start]);
        out.push_str(new);
        at = end;
    }
    out.push_str(&text[at..]);
    out
}

/// The edits for one document from a workspace edit, in whichever form it's in, with its version.
fn edits_for(edit: &Value, doc: &str) -> (Vec<Value>, Value) {
    if let Some(changes) = edit["documentChanges"].as_array() {
        for change in changes {
            if change["textDocument"]["uri"] == doc {
                return (
                    change["edits"].as_array().unwrap().clone(),
                    change["textDocument"]["version"].clone(),
                );
            }
        }
        panic!("no edits for {doc}: {edit}");
    }
    (
        edit["changes"][doc].as_array().unwrap().clone(),
        Value::Null,
    )
}

fn range_at(line: u32, start: u32, end: u32) -> Value {
    json!({"start": {"line": line, "character": start}, "end": {"line": line, "character": end}})
}

async fn actions_at(c: &mut Client, doc: &str, range: Value, diagnostics: Value) -> Vec<Value> {
    let got = c
        .request(
            "textDocument/codeAction",
            json!({"textDocument": {"uri": doc}, "range": range, "context": {"diagnostics": diagnostics}}),
        )
        .await;
    got.as_array().cloned().unwrap_or_default()
}

fn titled<'a>(actions: &'a [Value], title: &str) -> &'a Value {
    actions
        .iter()
        .find(|a| a["title"] == title)
        .unwrap_or_else(|| panic!("no `{title}` among {actions:?}"))
}

/// Waits until `doc`'s diagnostics have `count` of `code`.
async fn settled(c: &mut Client, doc: &str, code: &str, count: usize) -> Vec<Value> {
    loop {
        let d = c.diagnostics(doc).await;
        if d.iter().filter(|x| x["code"] == code).count() == count {
            return d;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_quick_fix_is_resolved_applied_and_gone() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    write(&root.join("src/User.php"), "<?php\nnamespace App\\Models;\nclass User {}\n");
    let main = root.join("src/main.php");
    let text = "<?php\nnamespace App;\n\nnew User();\n";
    write(&main, text);
    let doc = uri(&main);
    let mut c = start(&root, full_capabilities(), json!({})).await;
    open(&mut c, &main, 3, text).await;
    let diags = settled(&mut c, &doc, "class.notFound", 1).await;

    let actions = actions_at(&mut c, &doc, range_at(3, 4, 8), json!(diags)).await;
    let import = titled(&actions, "Import `App\\Models\\User`");
    assert_eq!(import["kind"], "quickfix");
    assert_eq!(import["isPreferred"], true);
    assert!(import.get("edit").is_none_or(Value::is_null), "{import}");
    assert_eq!(import["diagnostics"][0]["code"], "class.notFound");

    let resolved = c.request("codeAction/resolve", import.clone()).await;
    let (edits, version) = edits_for(&resolved["edit"], &doc);
    assert_eq!(version, 3);
    let new = apply_edits(text, &edits);
    assert_eq!(new, "<?php\nnamespace App;\n\nuse App\\Models\\User;\n\nnew User();\n");
    assert!(stir_syntax::parse(&new).errors().is_empty());

    c.notify(
        "textDocument/didChange",
        json!({"textDocument": {"uri": doc, "version": 4}, "contentChanges": [{"text": new}]}),
    )
    .await;
    let after = settled(&mut c, &doc, "class.notFound", 0).await;
    assert!(after.is_empty(), "{after:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn positions_are_utf16() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let main = root.join("src/main.php");
    // 𝄞 is two UTF-16 units and four bytes, é one unit and two bytes
    let text = "<?php\nfunction f(string $é): int {\n    return strlen(\"é𝄞\" . $é) + strlen($é . 'x');\n}\n";
    write(&main, text);
    let doc = uri(&main);
    let mut c = start(&root, full_capabilities(), json!({})).await;
    open(&mut c, &main, 1, text).await;
    let line = text.lines().nth(2).unwrap();
    let byte = line.find("\"é𝄞\"").unwrap();
    let units = |s: &str| s.encode_utf16().count() as u32;
    let start_at = units(&line[..byte]);
    let end_at = start_at + units("\"é𝄞\"");
    let actions = actions_at(&mut c, &doc, range_at(2, start_at, end_at), json!([])).await;
    let extract = titled(&actions, "Extract to a variable");
    let resolved = c.request("codeAction/resolve", extract.clone()).await;
    let (edits, _) = edits_for(&resolved["edit"], &doc);
    let new = apply_edits(text, &edits);
    assert_eq!(
        new,
        "<?php\nfunction f(string $é): int {\n    $value = \"é𝄞\";\n    return strlen($value . $é) + strlen($é . 'x');\n}\n"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_without_resolve_gets_edits_and_not_what_takes_the_workspace() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let main = root.join("src/main.php");
    let text = "<?php\nfunction f(int $a, int $b): int { return $a; }\nnew Nope();\n";
    write(&main, text);
    let doc = uri(&main);
    let caps = json!({"textDocument": {"codeAction": {"codeActionLiteralSupport": {"codeActionKind": {"valueSet": ["quickfix", "refactor", "source"]}}}}});
    let mut c = start(&root, caps, json!({})).await;
    open(&mut c, &main, 1, text).await;
    settled(&mut c, &doc, "class.notFound", 1).await;
    let actions = actions_at(&mut c, &doc, range_at(2, 4, 8), json!([])).await;
    let ignore = titled(&actions, "Ignore `class.notFound` on this line");
    let edits = ignore["edit"]["changes"][&doc].as_array().unwrap();
    assert_eq!(
        apply_edits(text, edits),
        "<?php\nfunction f(int $a, int $b): int { return $a; }\nnew Nope(); // @phpstan-ignore class.notFound\n"
    );
    assert!(ignore.get("data").is_none_or(Value::is_null));

    let at_param = actions_at(&mut c, &doc, range_at(1, 24, 24), json!([])).await;
    assert!(
        !at_param.iter().any(|a| a["title"] == "Remove unused parameter `$b`"),
        "{at_param:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_change_across_files_has_each_files_version() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let main = root.join("src/main.php");
    let other = root.join("src/other.php");
    let closed = root.join("src/closed.php");
    let text = "<?php\nfunction ff(int $a, int $b): int { return $a; }\n";
    let other_text = "<?php\necho ff(1, 2);\n";
    write(&main, text);
    write(&other, other_text);
    write(&closed, "<?php\necho ff(3, 4);\n");
    let doc = uri(&main);
    let other_doc = uri(&other);
    let closed_doc = uri(&closed);
    let mut c = start(&root, full_capabilities(), json!({})).await;
    open(&mut c, &main, 7, text).await;
    open(&mut c, &other, 11, other_text).await;
    c.pump(Duration::from_millis(400)).await;
    let actions = actions_at(&mut c, &doc, range_at(1, 24, 24), json!([])).await;
    let remove = titled(&actions, "Remove unused parameter `$b`");
    let resolved = c.request("codeAction/resolve", remove.clone()).await;
    let edit = &resolved["edit"];
    let (edits, version) = edits_for(edit, &doc);
    assert_eq!(version, 7);
    assert_eq!(apply_edits(text, &edits), "<?php\nfunction ff(int $a): int { return $a; }\n");
    let (edits, version) = edits_for(edit, &other_doc);
    assert_eq!(version, 11);
    assert_eq!(apply_edits(other_text, &edits), "<?php\necho ff(1);\n");
    let (edits, version) = edits_for(edit, &closed_doc);
    assert_eq!(version, Value::Null);
    assert_eq!(apply_edits("<?php\necho ff(3, 4);\n", &edits), "<?php\necho ff(3);\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn what_cant_be_done_is_shown_greyed_or_told() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let main = root.join("src/main.php");
    let text = "<?php\nfunction g(): int { return 1; }\nfunction f(int $a, int $b): int {\n    if ($a) { return 1; }\n    return $a;\n}\necho f(1, g());\n";
    write(&main, text);
    let doc = uri(&main);
    let mut c = start(&root, full_capabilities(), json!({})).await;
    open(&mut c, &main, 1, text).await;
    c.pump(Duration::from_millis(400)).await;

    // greyed out, with why
    let actions = actions_at(&mut c, &doc, range_at(3, 4, 25), json!([])).await;
    let extract = titled(&actions, "Extract function");
    let reason = extract["disabled"]["reason"].as_str().unwrap();
    assert!(reason.contains("every path"), "{reason}");
    assert!(extract.get("data").is_none_or(Value::is_null));

    // told when it's chosen, and nothing done
    let actions = actions_at(&mut c, &doc, range_at(2, 24, 24), json!([])).await;
    let remove = titled(&actions, "Remove unused parameter `$b`");
    let resolved = c.request("codeAction/resolve", remove.clone()).await;
    assert!(resolved.get("edit").is_none_or(Value::is_null), "{resolved}");
    let told = c.message("window/showMessage").await;
    let said = told["params"]["message"].as_str().unwrap();
    assert!(said.contains("no longer happen"), "{said}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stale_action_isnt_applied_to_other_text() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let main = root.join("src/main.php");
    let text = "<?php\nnew Nope();\n";
    write(&main, text);
    let doc = uri(&main);
    let mut c = start(&root, full_capabilities(), json!({})).await;
    open(&mut c, &main, 1, text).await;
    settled(&mut c, &doc, "class.notFound", 1).await;
    let actions = actions_at(&mut c, &doc, range_at(1, 4, 8), json!([])).await;
    let ignore = titled(&actions, "Ignore `class.notFound` on this line").clone();
    c.notify(
        "textDocument/didChange",
        json!({"textDocument": {"uri": doc, "version": 2}, "contentChanges": [{"text": "<?php\n\nnew Nope();\n"}]}),
    )
    .await;
    c.send(json!({"jsonrpc": "2.0", "id": 777, "method": "codeAction/resolve", "params": ignore}))
        .await;
    loop {
        let m = c.next().await;
        if m["id"] == 777 {
            assert_eq!(m["error"]["code"], -32801, "{m}");
            break;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn only_what_was_asked_for_comes_back() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let main = root.join("src/main.php");
    let text = "<?php\nuse App\\B;\nuse App\\A;\nnew A; new B;\n";
    write(&main, text);
    let doc = uri(&main);
    let mut c = start(&root, full_capabilities(), json!({})).await;
    open(&mut c, &main, 1, text).await;
    c.pump(Duration::from_millis(400)).await;
    let got = c
        .request(
            "textDocument/codeAction",
            json!({"textDocument": {"uri": doc}, "range": range_at(0, 0, 0),
                   "context": {"diagnostics": [], "only": ["source.organizeImports"]}}),
        )
        .await;
    let actions = got.as_array().unwrap();
    assert_eq!(actions.len(), 1, "{actions:?}");
    assert_eq!(actions[0]["kind"], "source.organizeImports");
    let resolved = c.request("codeAction/resolve", actions[0].clone()).await;
    let (edits, _) = edits_for(&resolved["edit"], &doc);
    assert_eq!(
        apply_edits(text, &edits),
        "<?php\nuse App\\A;\nuse App\\B;\nnew A; new B;\n"
    );
    // not without being asked: an editor that runs them on save asks
    let plain = actions_at(&mut c, &doc, range_at(0, 0, 0), json!([])).await;
    assert!(!plain.iter().any(|a| a["kind"] == "source.fixAll"), "{plain:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn what_phpstan_reports_gets_fixes_where_its_line_says_enough() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let main = root.join("src/main.php");
    let text = "<?php\nclass A {\n    public function a($x) { return 1; }\n}\n";
    write(&main, text);
    let doc = uri(&main);
    let mut c = start(&root, full_capabilities(), json!({})).await;
    open(&mut c, &main, 1, text).await;
    c.pump(Duration::from_millis(400)).await;
    let line = range_at(2, 0, 39);
    let reported = json!({"range": line, "severity": 1, "source": "phpstan",
                          "code": "missingType.return", "message": "whatever"});
    let actions = actions_at(&mut c, &doc, range_at(2, 4, 4), json!([reported])).await;
    let ignore = titled(&actions, "Ignore `missingType.return` on this line");
    let resolved = c.request("codeAction/resolve", ignore.clone()).await;
    let (edits, _) = edits_for(&resolved["edit"], &doc);
    assert_eq!(
        apply_edits(text, &edits),
        "<?php\nclass A {\n    public function a($x) { return 1; } // @phpstan-ignore missingType.return\n}\n"
    );
    let add = titled(&actions, "Add return type `int`");
    assert_eq!(add["diagnostics"][0]["source"], "phpstan");
    let resolved = c.request("codeAction/resolve", add.clone()).await;
    let (edits, _) = edits_for(&resolved["edit"], &doc);
    assert_eq!(
        apply_edits(text, &edits),
        "<?php\nclass A {\n    public function a($x): int { return 1; }\n}\n"
    );
    // two functions on the line: which one it means can't be told
    let text2 = "<?php\nfunction a() {} function b() {}\n";
    c.notify(
        "textDocument/didChange",
        json!({"textDocument": {"uri": doc, "version": 2}, "contentChanges": [{"text": text2}]}),
    )
    .await;
    let reported = json!({"range": range_at(1, 0, 31), "severity": 1, "source": "phpstan",
                          "code": "missingType.return", "message": "whatever"});
    let actions = actions_at(&mut c, &doc, range_at(1, 0, 0), json!([reported])).await;
    assert!(
        !actions.iter().any(|a| a["kind"] == "quickfix" && a["title"].as_str().unwrap().starts_with("Add return")),
        "{actions:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_new_name_is_renamed_and_a_parameter_is_asked_for_by_an_editor_that_can() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let main = root.join("src/main.php");
    let text = "<?php\nfunction f(int $a): void {\n    echo $a;\n}\nf(1);\n";
    write(&main, text);
    let doc = uri(&main);
    let options = json!({"clientCommands": ["stirphp.renameAt", "stirphp.askParameter"]});
    let mut c = start(&root, full_capabilities(), options).await;
    open(&mut c, &main, 1, text).await;
    c.pump(Duration::from_millis(400)).await;

    let actions = actions_at(&mut c, &doc, range_at(2, 4, 12), json!([])).await;
    let extract = titled(&actions, "Extract function");
    let resolved = c.request("codeAction/resolve", extract.clone()).await;
    assert_eq!(resolved["command"]["command"], "stirphp.renameAt");
    let args = resolved["command"]["arguments"].as_array().unwrap();
    let (edits, _) = edits_for(&resolved["edit"], &doc);
    let new = apply_edits(text, &edits);
    let at = offset_of(
        &new,
        &json!({"line": args[1], "character": args[2]}),
    );
    assert!(new[at..].starts_with("extracted($a);"), "{new} at {at}");

    let actions = actions_at(&mut c, &doc, range_at(1, 9, 9), json!([])).await;
    let add = titled(&actions, "Add parameter...");
    assert_eq!(add["command"]["command"], "stirphp.askParameter");
    let args = add["command"]["arguments"].as_array().unwrap().clone();
    c.send(json!({"jsonrpc": "2.0", "id": 900, "method": "workspace/executeCommand", "params": {
        "command": "stirphp.addParameter",
        "arguments": [{"uri": args[0], "offset": args[1], "hash": args[2], "parameter": "string $n = 'x'"}],
    }}))
    .await;
    let apply = c.message("workspace/applyEdit").await;
    let (edits, _) = edits_for(&apply["params"]["edit"], &doc);
    assert_eq!(
        apply_edits(text, &edits),
        "<?php\nfunction f(int $a, string $n = 'x'): void {\n    echo $a;\n}\nf(1);\n"
    );
    c.respond(&apply, json!({"applied": true})).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unused_import_is_greyed_out_only_in_the_editor() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let main = root.join("src/main.php");
    let text = "<?php\nuse App\\Gone;\necho 1;\n";
    write(&main, text);
    let doc = uri(&main);
    let mut c = start(&root, full_capabilities(), json!({})).await;
    open(&mut c, &main, 1, text).await;
    let diags = settled(&mut c, &doc, "SPHP0080", 1).await;
    assert_eq!(diags[0]["severity"], 4);
    assert_eq!(diags[0]["tags"][0], 1);
    assert_eq!(diags[0]["range"]["start"]["line"], 1);
}
