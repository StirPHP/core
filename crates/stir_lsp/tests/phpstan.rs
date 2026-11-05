//! PHPStan next to our own diagnostics, over the protocol. The `phpstan` here is a shell script, so
//! what it says and how it behaves don't depend on PHP being there. Unix only: the real thing is
//! tried by hand elsewhere.
#![cfg(unix)]

mod client;

use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::time::Duration;

use client::{Client, uri, write};
use serde_json::{Value, json};
use stir_lsp::server::TrustOptions;
use stir_lsp::trust::TrustStore;

/// Writes down that it ran and its process, then does whatever the project's `behave` says.
const FAKE: &str = r#"#!/bin/sh
echo $$ >> pids
echo "$@" >> runs
. ./behave
"#;

/// Says what's in `report.json`.
const REPORTS: &str = "cat report.json\n";

/// `return.type` is ours at line 3.
const MAIN: &str = "<?php\nfunction f(): int {\n    return 'x';\n}\n";

struct Project {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    store: PathBuf,
    main: PathBuf,
}

impl Project {
    /// A project with PHPStan installed and configured.
    fn new() -> Project {
        let tmp = tempfile::TempDir::new().unwrap();
        let base = std::fs::canonicalize(tmp.path()).unwrap();
        let root = base.join("project");
        let main = root.join("src/main.php");
        write(&main, MAIN);
        write(&root.join("phpstan.neon"), "parameters:\n    level: 5\n");
        let fake = root.join("vendor/bin/phpstan");
        write(&fake, FAKE);
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        write(&root.join("behave"), REPORTS);
        let project = Project {
            _tmp: tmp,
            root,
            store: base.join("config/trust.json"),
            main,
        };
        project.report(json!({}), json!([]));
        project
    }

    /// What the fake says next, with `files` as `{path: [messages]}`.
    fn report(&self, files: Value, errors: Value) {
        let files: serde_json::Map<String, Value> = files
            .as_object()
            .unwrap()
            .iter()
            .map(|(path, messages)| (path.clone(), json!({"messages": messages})))
            .collect();
        // in PHPStan's order, which `json!` wouldn't keep
        let files = Value::Object(files);
        let report = format!(r#"{{"totals":{{}},"files":{files},"errors":{errors}}}"#);
        write(&self.root.join("report.json"), &report);
    }

    fn behave(&self, script: &str) {
        write(&self.root.join("behave"), script);
    }

    fn main_path(&self) -> String {
        self.main.display().to_string()
    }

    fn runs(&self) -> usize {
        std::fs::read_to_string(self.root.join("runs")).map_or(0, |r| r.lines().count())
    }

    fn pids(&self) -> Vec<String> {
        std::fs::read_to_string(self.root.join("pids"))
            .unwrap_or_default()
            .split_whitespace()
            .map(str::to_owned)
            .collect()
    }

    async fn open(&self, options: Value) -> (Client, String) {
        self.open_with(json!({"window": {"showMessage": {}}}), options)
            .await
    }

    async fn open_with(&self, capabilities: Value, options: Value) -> (Client, String) {
        let mut c = Client::start_with(TrustOptions {
            store: TrustStore::at(&self.store),
            answer_wait: Duration::from_secs(60),
        });
        c.request(
            "initialize",
            json!({"processId": null, "rootUri": uri(&self.root), "capabilities": capabilities, "initializationOptions": options}),
        )
        .await;
        c.notify("initialized", json!({})).await;
        let main = uri(&self.main);
        c.notify(
            "textDocument/didOpen",
            json!({"textDocument": {"uri": main, "languageId": "php", "version": 1, "text": MAIN}}),
        )
        .await;
        (c, main)
    }
}

async fn allow(c: &mut Client) -> Value {
    let question = c.message("window/showMessageRequest").await;
    c.respond(&question, json!({"title": "Allow"})).await;
    question
}

/// Waits for what's published of `uri` to be as `wanted` says.
async fn until(c: &mut Client, uri: &str, wanted: impl Fn(&[Value]) -> bool) -> Vec<Value> {
    for _ in 0..100 {
        let diags = c.diagnostics(uri).await;
        if wanted(&diags) {
            return diags;
        }
    }
    panic!("never published what was waited for, for {uri}");
}

fn from_phpstan(diags: &[Value]) -> Vec<&Value> {
    diags.iter().filter(|d| d["source"] == "phpstan").collect()
}

fn ours(diags: &[Value]) -> Vec<&Value> {
    diags.iter().filter(|d| d["source"] == "stirphp").collect()
}

async fn wait_until(what: &str, ready: impl Fn() -> bool) {
    for _ in 0..1000 {
        if ready() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("never: {what}");
}

fn alive(pid: &str) -> bool {
    // a process that's been killed but not waited for is as good as gone
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
        !stat
            .rsplit(')')
            .next()
            .unwrap_or("")
            .trim_start()
            .starts_with('Z')
    })
}

fn notices(c: &Client) -> Vec<String> {
    c.backlog
        .iter()
        .filter(|m| m["method"] == "window/showMessage")
        .map(|m| m["params"]["message"].as_str().unwrap().to_owned())
        .collect()
}

async fn save(c: &mut Client, uri: &str) {
    c.notify(
        "textDocument/didSave",
        json!({"textDocument": {"uri": uri}}),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn asks_first_then_merges_without_duplicates() {
    let p = Project::new();
    p.report(
        json!({ p.main_path(): [
            // the same as ours: same line, same identifier, other words
            {"message": "Function f() should return int but returns string.", "line": 3, "ignorable": true, "identifier": "return.type"},
            {"message": "Only PHPStan sees this.", "line": 2, "ignorable": true, "tip": "Here's a tip.", "identifier": "missingType.iterableValue"},
            {"message": "No identifier.", "line": 3, "ignorable": true},
            {"message": "About the whole file.", "line": null, "ignorable": false, "identifier": "ignore.unmatched"},
        ]}),
        json!([]),
    );
    let (mut c, main) = p.open(json!({})).await;

    // nothing runs until someone says so, and it's said what would
    let question = c.message("window/showMessageRequest").await;
    let text = question["params"]["message"].as_str().unwrap();
    assert!(text.contains("vendor/bin/phpstan analyse"), "{text}");
    assert!(text.contains("bootstrap files and extensions"), "{text}");
    assert!(text.contains("sandboxed"), "{text}");
    let diags = c.diagnostics(&main).await;
    assert_eq!(ours(&diags).len(), 1);
    assert!(from_phpstan(&diags).is_empty());
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(p.runs(), 0);
    c.respond(&question, json!({"title": "Allow"})).await;

    // once allowed, it runs without a save
    let diags = until(&mut c, &main, |d| from_phpstan(d).len() == 3).await;
    // ours stays, with its range and its words
    let ours = ours(&diags);
    assert_eq!(ours.len(), 1);
    assert_eq!(ours[0]["code"], "return.type");
    assert_eq!(ours[0]["range"]["start"]["character"], 11);
    let theirs = from_phpstan(&diags);
    let only = theirs
        .iter()
        .find(|d| d["code"] == "missingType.iterableValue")
        .unwrap();
    assert_eq!(only["message"], "Only PHPStan sees this.\nHere's a tip.");
    assert_eq!(only["severity"], 1);
    assert_eq!(
        only["codeDescription"]["href"],
        "https://phpstan.org/error-identifiers/missingType.iterableValue"
    );
    // a whole line, without its indentation
    assert_eq!(only["range"]["start"], json!({"line": 1, "character": 0}));
    assert_eq!(only["range"]["end"], json!({"line": 1, "character": 19}));
    let plain = theirs
        .iter()
        .find(|d| d["message"] == "No identifier.")
        .unwrap();
    assert!(plain.get("code").is_none_or(Value::is_null), "{plain}");
    assert!(
        plain.get("codeDescription").is_none_or(Value::is_null),
        "{plain}"
    );
    assert_eq!(plain["range"]["start"], json!({"line": 2, "character": 4}));
    let whole = theirs
        .iter()
        .find(|d| d["code"] == "ignore.unmatched")
        .unwrap();
    assert_eq!(whole["range"]["start"]["line"], 0);

    // a new session doesn't ask again
    let (mut c, main) = p.open(json!({})).await;
    until(&mut c, &main, |d| from_phpstan(d).len() == 3).await;
    assert!(
        !c.backlog
            .iter()
            .any(|m| m["method"] == "window/showMessageRequest"),
        "{:?}",
        c.backlog
    );

    // a changed configuration can name other code to run, so it asks again, and stops until then
    write(&p.root.join("phpstan.neon"), "parameters:\n    level: 6\n");
    c.notify(
        "workspace/didChangeWatchedFiles",
        json!({"changes": [{"uri": uri(&p.root.join("phpstan.neon")), "type": 2}]}),
    )
    .await;
    let question = c.message("window/showMessageRequest").await;
    assert!(
        question["params"]["message"]
            .as_str()
            .unwrap()
            .contains("PHPStan")
    );
    until(&mut c, &main, |d| from_phpstan(d).is_empty()).await;
    c.respond(&question, json!({"title": "Allow"})).await;
    until(&mut c, &main, |d| from_phpstan(d).len() == 3).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refusal_is_remembered() {
    let p = Project::new();
    let (mut c, main) = p.open(json!({})).await;
    let question = c.message("window/showMessageRequest").await;
    c.respond(&question, json!({"title": "Don't allow"})).await;
    save(&mut c, &main).await;
    c.pump(Duration::from_millis(600)).await;
    assert_eq!(p.runs(), 0);

    let (mut c, _) = p.open(json!({})).await;
    c.pump(Duration::from_millis(600)).await;
    assert!(
        !c.backlog
            .iter()
            .any(|m| m["method"] == "window/showMessageRequest"),
        "{:?}",
        c.backlog
    );
    assert_eq!(p.runs(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn allowing_scripts_does_not_allow_this() {
    let p = Project::new();
    write(
        &p.root.join("phpstan.neon"),
        "parameters:\n    stirphp:\n        rules: [no_echo.rhai]\n",
    );
    write(
        &p.root.join("no_echo.rhai"),
        "fn visit_echo(node, ctx) { ctx.error(\"no\", \"app.no\"); }\n",
    );
    let (mut c, main) = p.open(json!({})).await;
    // one question each: answering one leaves the other
    let first = c.message("window/showMessageRequest").await;
    let second = c.message("window/showMessageRequest").await;
    let is_phpstan = |q: &Value| q["params"]["message"].as_str().unwrap().contains("PHPStan");
    assert_ne!(is_phpstan(&first), is_phpstan(&second));
    let (rules, phpstan) = if is_phpstan(&first) {
        (second, first)
    } else {
        (first, second)
    };
    c.respond(&rules, json!({"title": "Allow"})).await;
    save(&mut c, &main).await;
    c.pump(Duration::from_millis(600)).await;
    assert_eq!(p.runs(), 0);
    c.respond(&phpstan, json!({"title": "Allow"})).await;
    wait_until("it runs", || p.runs() > 0).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_untrusted_workspace_never_runs_it_and_is_not_asked() {
    let p = Project::new();
    // not even with an answer from before, or settings that ask for it
    let (mut c, main) = p.open(json!({})).await;
    allow(&mut c).await;
    wait_until("it runs", || p.runs() > 0).await;
    let before = p.runs();

    let (mut c, _) = p
        .open(json!({"workspaceTrusted": false, "phpstan": {"enable": "on"}}))
        .await;
    save(&mut c, &main).await;
    c.pump(Duration::from_millis(600)).await;
    assert_eq!(p.runs(), before);
    assert!(
        !c.backlog
            .iter()
            .any(|m| m["method"] == "window/showMessageRequest"),
        "{:?}",
        c.backlog
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn nobody_to_ask_means_one_notice_and_no_run() {
    let p = Project::new();
    let (mut c, main) = p.open_with(json!({}), json!({})).await;
    let notice = c.message("window/showMessage").await;
    assert!(
        notice["params"]["message"]
            .as_str()
            .unwrap()
            .contains("isn't running")
    );
    save(&mut c, &main).await;
    c.pump(Duration::from_millis(600)).await;
    assert_eq!(p.runs(), 0);
    assert!(notices(&c).is_empty(), "{:?}", notices(&c));
}

#[tokio::test(flavor = "multi_thread")]
async fn it_stays_quiet_unless_a_project_has_it_or_someone_asks() {
    // no configuration, or no binary
    let p = Project::new();
    std::fs::remove_file(p.root.join("phpstan.neon")).unwrap();
    let (mut c, _) = p.open(json!({})).await;
    c.pump(Duration::from_millis(400)).await;
    assert!(
        !c.backlog
            .iter()
            .any(|m| m["method"] == "window/showMessageRequest"
                || m["method"] == "window/showMessage"),
        "{:?}",
        c.backlog
    );
    let p = Project::new();
    std::fs::remove_file(p.root.join("vendor/bin/phpstan")).unwrap();
    let (mut c, _) = p.open(json!({})).await;
    c.pump(Duration::from_millis(400)).await;
    assert!(
        !c.backlog
            .iter()
            .any(|m| m["method"] == "window/showMessageRequest"
                || m["method"] == "window/showMessage"),
        "{:?}",
        c.backlog
    );
    // asked for by a setting and not there: that's worth saying, once
    let (mut c, main) = p.open(json!({"phpstan": {"enable": "on"}})).await;
    let notice = c.message("window/showMessage").await;
    assert!(
        notice["params"]["message"]
            .as_str()
            .unwrap()
            .contains("isn't installed")
    );
    save(&mut c, &main).await;
    c.pump(Duration::from_millis(400)).await;
    assert!(notices(&c).is_empty(), "{:?}", notices(&c));
    // and off is off
    let p = Project::new();
    let (mut c, _) = p.open(json!({"phpstan": {"enable": "off"}})).await;
    c.pump(Duration::from_millis(400)).await;
    assert!(
        !c.backlog
            .iter()
            .any(|m| m["method"] == "window/showMessageRequest")
    );
    assert_eq!(p.runs(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn settings_come_before_the_project_and_reach_the_command() {
    let p = Project::new();
    write(&p.root.join("ci.neon"), "parameters:\n    level: 1\n");
    write(
        &p.root.join("phpstan.neon"),
        "parameters:\n    stirphp:\n        phpstan:\n            level: 1\n            memoryLimit: 1G\n",
    );
    let options = json!({"phpstan": {"configuration": "ci.neon", "level": "max", "path": "", "memoryLimit": ""}});
    let (mut c, _) = p.open(options).await;
    allow(&mut c).await;
    wait_until("it runs", || p.runs() > 0).await;
    let runs = std::fs::read_to_string(p.root.join("runs")).unwrap();
    assert!(
        runs.contains(&format!(
            "--configuration {}",
            p.root.join("ci.neon").display()
        )),
        "{runs}"
    );
    assert!(runs.contains("--level=max"), "{runs}");
    assert!(runs.contains("--memory-limit=1G"), "{runs}");
    assert!(runs.contains("--error-format=json --no-progress"), "{runs}");
}

#[tokio::test(flavor = "multi_thread")]
async fn results_wait_for_a_save_and_stale_lines_are_not_shown() {
    let p = Project::new();
    let at_line_2 = json!({ p.main_path(): [
        {"message": "First.", "line": 2, "ignorable": true, "identifier": "a.first"},
    ]});
    p.report(at_line_2, json!([]));
    let (mut c, main) = p.open(json!({})).await;
    allow(&mut c).await;
    until(&mut c, &main, |d| from_phpstan(d).len() == 1).await;
    assert_eq!(p.runs(), 1);

    // typing doesn't run anything, and what was said about other text isn't shown
    let edited = format!("{MAIN}// more\n");
    c.notify(
        "textDocument/didChange",
        json!({"textDocument": {"uri": main, "version": 2}, "contentChanges": [{"text": edited}]}),
    )
    .await;
    let diags = until(&mut c, &main, |d| from_phpstan(d).is_empty()).await;
    assert_eq!(ours(&diags).len(), 1);
    c.pump(Duration::from_millis(500)).await;
    assert_eq!(p.runs(), 1);

    // undone: it's the same text again
    c.notify(
        "textDocument/didChange",
        json!({"textDocument": {"uri": main, "version": 3}, "contentChanges": [{"text": MAIN}]}),
    )
    .await;
    until(&mut c, &main, |d| from_phpstan(d).len() == 1).await;

    // a save reads what's now on disk
    write(&p.main, &edited);
    c.notify(
        "textDocument/didChange",
        json!({"textDocument": {"uri": main, "version": 4}, "contentChanges": [{"text": edited}]}),
    )
    .await;
    until(&mut c, &main, |d| from_phpstan(d).is_empty()).await;
    p.report(
        json!({ p.main_path(): [
            {"message": "Second.", "line": 5, "ignorable": true, "identifier": "a.second"},
        ]}),
        json!([]),
    );
    save(&mut c, &main).await;
    let diags = until(&mut c, &main, |d| from_phpstan(d).len() == 1).await;
    assert_eq!(from_phpstan(&diags)[0]["message"], "Second.");
    assert_eq!(p.runs(), 2);

    // unsaved text when it runs: what it read isn't what's in the editor
    c.notify(
        "textDocument/didChange",
        json!({"textDocument": {"uri": main, "version": 5}, "contentChanges": [{"text": MAIN}]}),
    )
    .await;
    until(&mut c, &main, |d| from_phpstan(d).is_empty()).await;
    // another file's save runs it
    let other = p.root.join("src/other.php");
    write(&other, "<?php\n");
    save(&mut c, &uri(&other)).await;
    wait_until("it runs", || p.runs() == 3).await;
    c.pump(Duration::from_millis(400)).await;
    let shown = c.backlog.iter().find(|m| {
        m["method"] == "textDocument/publishDiagnostics"
            && m["params"]["uri"] == main.as_str()
            && !from_phpstan(m["params"]["diagnostics"].as_array().unwrap()).is_empty()
    });
    assert_eq!(shown, None, "shown for other text");

    // a file outside the project isn't a reason to run
    let outside = p.root.join("vendor/x.php");
    write(&outside, "<?php\n");
    save(&mut c, &uri(&outside)).await;
    c.pump(Duration::from_millis(500)).await;
    assert_eq!(p.runs(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn other_files_get_theirs_and_errors_about_the_whole_run_go_on_the_configuration() {
    let p = Project::new();
    // not one of our sources, and PHPStan may still report it
    write(
        &p.root.join("stirphp.neon"),
        "includes:\n    - phpstan.neon\nparameters:\n    stirphp:\n        exclude: ['vendor/**', 'lib/**']\n",
    );
    let elsewhere = p.root.join("lib/elsewhere.php");
    write(&elsewhere, "<?php\nfoo();\n");
    let trait_context = format!("{} (in context of class App\\A)", p.main_path());
    p.report(
        json!({
            elsewhere.display().to_string(): [
                {"message": "Elsewhere.", "line": 2, "ignorable": true, "identifier": "a.elsewhere"},
            ],
            trait_context: [
                {"message": "In a trait.", "line": 2, "ignorable": true, "identifier": "a.trait"},
            ],
            p.main_path(): [
                {"message": "Plain.", "line": 2, "ignorable": true, "identifier": "a.plain"},
            ],
        }),
        json!(["Ignored error pattern #x# was not matched in reported errors."]),
    );
    let (mut c, main) = p.open(json!({})).await;
    allow(&mut c).await;
    let diags = until(&mut c, &main, |d| from_phpstan(d).len() == 2).await;
    assert!(
        from_phpstan(&diags)
            .iter()
            .any(|d| d["message"] == "In a trait. (in context of class App\\A)")
    );
    let diags = until(&mut c, &uri(&elsewhere), |d| !d.is_empty()).await;
    assert_eq!(diags[0]["message"], "Elsewhere.");
    assert_eq!(diags[0]["source"], "phpstan");
    let config = p.root.join("phpstan.neon");
    let diags = until(&mut c, &uri(&config), |d| !d.is_empty()).await;
    assert_eq!(diags[0]["source"], "phpstan");
    assert!(
        diags[0]["message"]
            .as_str()
            .unwrap()
            .contains("Ignored error pattern")
    );

    // what's no longer reported goes
    p.report(json!({}), json!([]));
    save(&mut c, &main).await;
    until(&mut c, &main, |d| from_phpstan(d).is_empty()).await;
    until(&mut c, &uri(&elsewhere), <[Value]>::is_empty).await;
    until(&mut c, &uri(&config), <[Value]>::is_empty).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_save_during_a_run_waits_for_it_and_there_is_one_more() {
    let p = Project::new();
    p.report(
        json!({ p.main_path(): [
            {"message": "Slowly.", "line": 2, "ignorable": true, "identifier": "a.slow"},
        ]}),
        json!([]),
    );
    // holds on until told to go on
    p.behave("while [ ! -f go ]; do sleep 0.02; done\nrm -f go\ncat report.json\n");
    let (mut c, main) = p.open(json!({})).await;
    allow(&mut c).await;
    wait_until("it runs", || p.runs() == 1).await;
    for _ in 0..3 {
        save(&mut c, &main).await;
    }
    c.pump(Duration::from_millis(500)).await;
    // the run isn't killed, as nothing would be left of it, and no second one starts
    assert_eq!(p.runs(), 1);
    assert!(alive(&p.pids()[0]));

    write(&p.root.join("go"), "");
    until(&mut c, &main, |d| from_phpstan(d).len() == 1).await;
    wait_until("a second run", || p.runs() == 2).await;
    write(&p.root.join("go"), "");
    c.pump(Duration::from_millis(600)).await;
    // three saves, and one run for them
    assert_eq!(p.runs(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn failures_are_told_once_and_ours_go_on() {
    let p = Project::new();
    p.behave("echo 'PHP Parse error: oops'; echo 'Something is broken.' >&2; exit 1\n");
    let (mut c, main) = p.open(json!({})).await;
    allow(&mut c).await;
    let notice = c.message("window/showMessage").await;
    let text = notice["params"]["message"].as_str().unwrap();
    assert!(text.contains("Something is broken."), "{text}");
    assert!(text.contains("code 1"), "{text}");
    assert!(text.contains("own diagnostics"), "{text}");

    for run in 2..4 {
        save(&mut c, &main).await;
        wait_until("it runs", || p.runs() == run).await;
    }
    c.pump(Duration::from_millis(400)).await;
    assert!(notices(&c).is_empty(), "{:?}", notices(&c));

    // ours keep up
    let edited =
        "<?php\nfunction f(): int {\n    return 'y';\n}\nfunction g(): int {\n    return 'z';\n}\n";
    c.notify(
        "textDocument/didChange",
        json!({"textDocument": {"uri": main, "version": 2}, "contentChanges": [{"text": edited}]}),
    )
    .await;
    until(&mut c, &main, |d| ours(d).len() == 2).await;

    // a different failure is worth saying, and once it's worked again, so is the same one again
    p.behave("exec /nothing/here\n");
    save(&mut c, &main).await;
    wait_until("it runs", || p.runs() == 4).await;
    c.pump(Duration::from_millis(400)).await;
    // the same kind: a run that ended without a report
    assert!(notices(&c).is_empty(), "{:?}", notices(&c));
    p.behave(REPORTS);
    save(&mut c, &main).await;
    wait_until("it runs", || p.runs() == 5).await;
    c.pump(Duration::from_millis(400)).await;
    p.behave("exit 2\n");
    save(&mut c, &main).await;
    wait_until("it runs", || p.runs() == 6).await;
    c.pump(Duration::from_millis(400)).await;
    assert_eq!(notices(&c).len(), 1, "{:?}", notices(&c));
}

#[tokio::test(flavor = "multi_thread")]
async fn running_out_of_memory_says_what_to_change() {
    let p = Project::new();
    p.report(
        json!({ p.main_path(): [
            {"message": "Before.", "line": 2, "ignorable": true, "identifier": "a.before"},
        ]}),
        json!([]),
    );
    let (mut c, main) = p.open(json!({})).await;
    allow(&mut c).await;
    until(&mut c, &main, |d| from_phpstan(d).len() == 1).await;

    // a worker dies: a report that says nothing about what wasn't analysed
    p.report(
        json!({}),
        json!(["Child process error: PHPStan process crashed because it reached configured PHP memory limit: 40M"]),
    );
    save(&mut c, &main).await;
    let notice = c.message("window/showMessage").await;
    assert!(
        notice["params"]["message"]
            .as_str()
            .unwrap()
            .contains("memory limit")
    );
    c.pump(Duration::from_millis(300)).await;
    assert!(
        !c.backlog.iter().any(|m| {
            m["method"] == "textDocument/publishDiagnostics"
                && m["params"]["uri"] == main.as_str()
                && from_phpstan(m["params"]["diagnostics"].as_array().unwrap()).is_empty()
        }),
        "what it said before was taken back: {:?}",
        c.backlog
    );

    // the main process dies
    p.behave(
        "echo 'PHP Fatal error:  Allowed memory size of 26214400 bytes exhausted' >&2; exit 255\n",
    );
    save(&mut c, &main).await;
    let notice = c.message("window/showMessage").await;
    assert!(
        notice["params"]["message"]
            .as_str()
            .unwrap()
            .contains("memoryLimit")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hanging_run_times_out_and_all_of_it_dies() {
    let p = Project::new();
    p.behave("sleep 1000 &\necho $! >> pids\nwait\n");
    let (mut c, _) = p.open(json!({"phpstan": {"timeout": 1}})).await;
    allow(&mut c).await;
    wait_until("it runs and starts another", || p.pids().len() == 2).await;
    let pids = p.pids();
    assert!(pids.iter().all(|pid| alive(pid)));
    let notice = c.message("window/showMessage").await;
    assert!(
        notice["params"]["message"]
            .as_str()
            .unwrap()
            .contains("after 1 seconds")
    );
    wait_until("all gone", || !pids.iter().any(|pid| alive(pid))).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn shutting_down_kills_a_run_and_all_of_it() {
    let p = Project::new();
    p.behave("sleep 1000 &\necho $! >> pids\nwait\n");
    let (mut c, _) = p.open(json!({})).await;
    allow(&mut c).await;
    wait_until("it runs and starts another", || p.pids().len() == 2).await;
    let pids = p.pids();
    assert!(pids.iter().all(|pid| alive(pid)));
    c.request("shutdown", Value::Null).await;
    wait_until("all gone", || !pids.iter().any(|pid| alive(pid))).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn progress_shows_a_run_on_a_client_that_can() {
    let p = Project::new();
    let capabilities = json!({"window": {"showMessage": {}, "workDoneProgress": true}});
    let (mut c, _) = p.open_with(capabilities, json!({})).await;
    allow(&mut c).await;
    let create = c.message("window/workDoneProgress/create").await;
    let token = create["params"]["token"].clone();
    c.respond(&create, Value::Null).await;
    let begin = c.message("$/progress").await;
    assert_eq!(begin["params"]["token"], token);
    assert_eq!(begin["params"]["value"]["kind"], "begin");
    assert_eq!(begin["params"]["value"]["title"], "PHPStan");
    let end = c.message("$/progress").await;
    assert_eq!(end["params"]["value"]["kind"], "end");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_syntax_error_is_not_shown_twice() {
    let p = Project::new();
    let broken = "<?php\nfunction x( {\n";
    write(&p.main, broken);
    p.report(
        json!({ p.main_path(): [
            {"message": "Syntax error, unexpected '{', expecting T_VARIABLE on line 2", "line": 2, "ignorable": false, "identifier": "phpstan.parse"},
        ]}),
        json!([]),
    );
    let mut c = Client::start_with(TrustOptions {
        store: TrustStore::at(&p.store),
        answer_wait: Duration::from_secs(60),
    });
    c.request(
        "initialize",
        json!({"processId": null, "rootUri": uri(&p.root), "capabilities": {"window": {"showMessage": {}}}}),
    )
    .await;
    c.notify("initialized", json!({})).await;
    allow(&mut c).await;
    wait_until("it runs", || p.runs() == 1).await;
    let main = uri(&p.main);
    c.pump(Duration::from_millis(500)).await;
    let diags = until_last(&mut c, &main);
    assert!(!diags.is_empty());
    assert!(from_phpstan(&diags).is_empty(), "{diags:?}");
}

/// The last of what's been published for `uri`.
fn until_last(c: &mut Client, uri: &str) -> Vec<Value> {
    c.backlog
        .iter()
        .rev()
        .find(|m| m["method"] == "textDocument/publishDiagnostics" && m["params"]["uri"] == uri)
        .map(|m| m["params"]["diagnostics"].as_array().unwrap().clone())
        .unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread")]
async fn caches_written_in_the_project_stay_out_of_it() {
    let p = Project::new();
    write(
        &p.root.join("phpstan.neon"),
        "parameters:\n    tmpDir: phpstan-cache\n",
    );
    let cache = p.root.join("phpstan-cache/resultCache.php");
    write(&cache, "<?php\nreturn 1 + ;\n");
    let (mut c, _) = p.open(json!({})).await;
    allow(&mut c).await;
    wait_until("it runs", || p.runs() == 1).await;
    // PHPStan rewrites it, which a watcher sees
    write(&cache, "<?php\nreturn 2 + ;\n");
    c.notify(
        "workspace/didChangeWatchedFiles",
        json!({"changes": [{"uri": uri(&cache), "type": 2}]}),
    )
    .await;
    c.pump(Duration::from_millis(600)).await;
    assert!(
        !c.backlog.iter().any(|m| {
            m["method"] == "textDocument/publishDiagnostics"
                && m["params"]["uri"] == uri(&cache).as_str()
                && !m["params"]["diagnostics"].as_array().unwrap().is_empty()
        }),
        "{:?}",
        c.backlog
    );
    assert_eq!(p.runs(), 1);
}
