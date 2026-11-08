//! The session that runs in the browser: how it schedules work, and what it has that the native
//! server hasn't. What it answers is checked against the native server in `transports.rs`.

use serde_json::{Value, json};
use stir_lsp::session::{DIAGNOSTICS_DELAY, Next, Session};

fn request(id: i64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

fn notification(method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "method": method, "params": params})
}

/// A session with a small project, initialized and with `a.php` open.
fn started() -> Session {
    let mut s = Session::new();
    s.push(request(
        1,
        "initialize",
        json!({"processId": null, "rootUri": "file:///project", "capabilities": {},
               "initializationOptions": {"files": {
                   "/project/composer.json": r#"{"require": {"php": "^8.1"}, "autoload": {"psr-4": {"App\\": "src/"}}}"#,
                   "file:///project/src/Lib.php": "<?php\nnamespace App;\nfunction lib(): int { return 1; }\n",
               }}}),
    ));
    s.push(notification("initialized", json!({})));
    s.push(notification(
        "textDocument/didOpen",
        json!({"textDocument": {"uri": "file:///project/src/a.php", "languageId": "php", "version": 1,
               "text": "<?php\nnamespace App;\necho lib();\necho nope();\n"}}),
    ));
    s
}

fn published(out: &[Value]) -> Vec<(&str, usize)> {
    out.iter()
        .filter(|m| m["method"] == "textDocument/publishDiagnostics")
        .map(|m| {
            (
                m["params"]["uri"].as_str().unwrap(),
                m["params"]["diagnostics"].as_array().unwrap().len(),
            )
        })
        .collect()
}

#[test]
fn diagnostics_wait_for_edits_to_stop_and_come_a_file_at_a_time() {
    let mut s = started();
    // a request a step, and the notifications up to the next
    assert_eq!(s.step(0.0), Next::Now);
    let out = s.take_outgoing();
    assert_eq!(out.len(), 1, "{out:?}");
    assert_eq!(out[0]["id"], 1);
    assert_eq!(s.step(0.0), Next::After(DIAGNOSTICS_DELAY));

    // typing goes on: the wait starts again
    s.push(notification(
        "textDocument/didChange",
        json!({"textDocument": {"uri": "file:///project/src/a.php", "version": 2},
               "contentChanges": [{"range": {"start": {"line": 3, "character": 5}, "end": {"line": 3, "character": 9}}, "text": "lib"}]}),
    ));
    assert_eq!(s.step(100.0), Next::After(DIAGNOSTICS_DELAY));
    assert!(s.take_outgoing().is_empty());
    assert_eq!(s.step(200.0), Next::After(50.0));

    // the edited file first, one file a step
    assert_eq!(s.step(250.0), Next::Now);
    assert_eq!(
        published(&s.take_outgoing()),
        [("file:///project/src/a.php", 0)]
    );
    assert_eq!(s.step(250.0), Next::Now);
    assert_eq!(
        published(&s.take_outgoing()),
        [("file:///project/src/Lib.php", 0)]
    );
    assert_eq!(s.step(250.0), Next::Idle);
}

#[test]
fn a_cancelled_request_is_answered_as_cancelled_and_not_worked_on() {
    let mut s = started();
    s.run_until_idle();
    s.take_outgoing();
    let hover = json!({"textDocument": {"uri": "file:///project/src/a.php"}, "position": {"line": 2, "character": 6}});
    s.push(request(2, "textDocument/hover", hover.clone()));
    s.push(request(3, "textDocument/hover", hover));
    s.push(notification("$/cancelRequest", json!({"id": 2})));
    let out = s.take_outgoing();
    assert_eq!(out[0]["id"], 2);
    assert_eq!(out[0]["error"]["code"], -32800);
    assert_eq!(s.step(0.0), Next::Idle);
    let out = s.take_outgoing();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["id"], 3);
    let shown = out[0]["result"]["contents"]["value"].as_str().unwrap();
    assert!(shown.contains("function lib"), "{shown}");
}

#[test]
fn requests_are_answered_one_a_step_in_order() {
    let mut s = started();
    s.run_until_idle();
    s.take_outgoing();
    let doc = json!({"textDocument": {"uri": "file:///project/src/a.php"}});
    s.push(request(2, "textDocument/documentSymbol", doc.clone()));
    s.push(request(3, "textDocument/foldingRange", doc));
    s.push(request(4, "no/suchMethod", json!({})));
    assert_eq!(s.step(0.0), Next::Now);
    assert_eq!(s.take_outgoing()[0]["id"], 2);
    assert_eq!(s.step(0.0), Next::Now);
    assert_eq!(s.take_outgoing()[0]["id"], 3);
    assert_eq!(s.step(0.0), Next::Idle);
    assert_eq!(s.take_outgoing()[0]["error"]["code"], -32601);
}

#[test]
fn files_from_the_page_and_the_text_of_stubs() {
    let mut s = started();
    s.run_until_idle();
    let out = s.take_outgoing();
    assert!(
        published(&out).contains(&("file:///project/src/a.php", 1)),
        "{out:?}"
    );

    // the page adds what was missing
    s.push(notification(
        "stirphp/setFiles",
        json!({"files": {"/project/src/Nope.php": "<?php\nnamespace App;\nfunction nope(): void {}\n"}}),
    ));
    s.run_until_idle();
    let out = s.take_outgoing();
    assert!(
        published(&out).contains(&("file:///project/src/a.php", 0)),
        "{out:?}"
    );

    // and the PHP version changes with Composer's file
    s.push(notification(
        "textDocument/didChange",
        json!({"textDocument": {"uri": "file:///project/src/a.php", "version": 2},
               "contentChanges": [{"text": "<?php\nreadonly class P {}\n"}]}),
    ));
    s.run_until_idle();
    let out = s.take_outgoing();
    assert!(
        published(&out).contains(&("file:///project/src/a.php", 1)),
        "{out:?}"
    );
    s.push(notification(
        "stirphp/setFiles",
        json!({"files": {"/project/composer.json": r#"{"require": {"php": "^8.2"}}"#}}),
    ));
    s.run_until_idle();
    let out = s.take_outgoing();
    assert!(
        published(&out).contains(&("file:///project/src/a.php", 0)),
        "{out:?}"
    );

    s.push(request(
        9,
        "stirphp/fileText",
        json!({"uri": "stirphp:///stubs/Core/Core.php"}),
    ));
    s.run_until_idle();
    let out = s.take_outgoing();
    assert!(
        out[0]["result"]
            .as_str()
            .unwrap()
            .contains("function strlen")
    );
}
