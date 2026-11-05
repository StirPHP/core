//! `stirphp check --with-phpstan`, and that the real server leaves nothing behind. The `phpstan`
//! here is a shell script, so this doesn't depend on PHP being there.
#![cfg(unix)]

use std::fs;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt as _;
use predicates::str::contains;
use serde_json::{Value, json};
use tempfile::TempDir;

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

/// `return.type` is ours at line 3.
const MAIN: &str = "<?php\nfunction f(): int {\n    return 'x';\n}\n";

/// A project with PHPStan installed and configured.
fn project() -> (TempDir, PathBuf) {
    let tmp = TempDir::new().unwrap();
    let root = fs::canonicalize(tmp.path()).unwrap().join("project");
    write(&root.join("src/main.php"), MAIN);
    write(&root.join("phpstan.neon"), "parameters:\n    level: 5\n");
    let fake = root.join("vendor/bin/phpstan");
    write(
        &fake,
        "#!/bin/sh\necho $$ >> pids\necho \"$@\" >> runs\n. ./behave\n",
    );
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
    write(&root.join("behave"), "cat report.json\n");
    (tmp, root)
}

fn report(root: &Path, files: &[(&str, Value)], errors: Value) {
    let files: serde_json::Map<String, Value> = files
        .iter()
        .map(|(path, messages)| {
            (
                root.join(path).display().to_string(),
                json!({"messages": messages}),
            )
        })
        .collect();
    let files = Value::Object(files);
    write(
        &root.join("report.json"),
        &format!(r#"{{"totals":{{}},"files":{files},"errors":{errors}}}"#),
    );
}

fn stirphp(root: &Path) -> Command {
    let mut cmd = Command::cargo_bin("stirphp").unwrap();
    cmd.env("NO_COLOR", "1").current_dir(root);
    cmd
}

fn sample(root: &Path) {
    report(
        root,
        &[(
            "src/main.php",
            json!([
                {"message": "Function f() should return int but returns string.", "line": 3, "ignorable": true, "identifier": "return.type"},
                {"message": "Only PHPStan sees this.", "line": 2, "ignorable": true, "tip": "A tip.", "identifier": "a.only"},
                {"message": "No identifier.", "line": 2, "ignorable": true},
            ]),
        )],
        json!([]),
    );
}

#[test]
fn it_only_runs_when_asked() {
    let (_tmp, root) = project();
    sample(&root);
    stirphp(&root)
        .arg("check")
        .assert()
        .failure()
        .stdout(contains("return.type").and(contains("a.only").not()));
    assert!(!root.join("runs").exists());
}

#[test]
fn what_it_reports_is_shown_with_ours_once() {
    let (_tmp, root) = project();
    sample(&root);
    stirphp(&root)
        .args(["check", "--with-phpstan"])
        .assert()
        .failure()
        .stdout(
            contains("error[a.only]")
                .and(contains("Only PHPStan sees this."))
                .and(contains("A tip."))
                .and(contains("reported by PHPStan"))
                .and(contains("error[phpstan]"))
                .and(contains("should return int").not())
                .and(contains("3 errors")),
        )
        .stderr(contains(
            "PHPStan reported 3, of which 1 are shown as ours.",
        ));
}

#[test]
fn json_says_where_a_report_comes_from() {
    let (_tmp, root) = project();
    sample(&root);
    let output = stirphp(&root)
        .args(["check", "--with-phpstan", "--format", "json"])
        .output()
        .unwrap();
    let items: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(items.len(), 3);
    let ours = items.iter().find(|i| i["code"] == "return.type").unwrap();
    assert!(ours.get("source").is_none(), "{ours}");
    assert_eq!(ours["column"], 12);
    let theirs = items.iter().find(|i| i["code"] == "a.only").unwrap();
    assert_eq!(theirs["source"], "phpstan");
    assert_eq!(theirs["line"], 2);
    assert_eq!(theirs["column"], 1);
    assert_eq!(theirs["notes"], json!(["A tip."]));
}

#[test]
fn paths_limit_what_it_reports_too() {
    let (_tmp, root) = project();
    write(&root.join("src/other.php"), "<?php\n\nfoo();\n");
    report(
        &root,
        &[
            (
                "src/main.php",
                json!([{"message": "In main.", "line": 1, "identifier": "a.main"}]),
            ),
            (
                "src/other.php",
                json!([{"message": "In other.", "line": 3, "identifier": "a.other"}]),
            ),
        ],
        json!([]),
    );
    stirphp(&root)
        .args(["check", "--with-phpstan", "src/other.php"])
        .assert()
        .failure()
        .stdout(contains("In other.").and(contains("In main.").not()));
}

#[test]
fn files_we_do_not_check_are_shown_too() {
    let (_tmp, root) = project();
    write(&root.join("vendor/x/x.php"), "<?php\n\nfoo();\n");
    report(
        &root,
        &[(
            "vendor/x/x.php",
            json!([{"message": "In a package.", "line": 3, "identifier": "a.x"}]),
        )],
        json!([]),
    );
    stirphp(&root)
        .args(["check", "--with-phpstan"])
        .assert()
        .failure()
        .stdout(contains("In a package.").and(contains("foo();")));
}

#[test]
fn our_baseline_and_ignore_rules_are_only_for_ours() {
    let (_tmp, root) = project();
    write(
        &root.join("phpstan.neon"),
        "parameters:\n    level: 5\n    ignoreErrors:\n        - identifier: return.type\n",
    );
    sample(&root);
    // ours is gone by the rule, and what PHPStan still says is PHPStan's to decide: this one it'd
    // have applied the rule to as well, had it really read it
    let output = stirphp(&root)
        .args(["check", "--with-phpstan", "--format", "json"])
        .output()
        .unwrap();
    let items: Vec<Value> = serde_json::from_slice(&output.stdout).unwrap();
    let same = items.iter().find(|i| i["code"] == "return.type").unwrap();
    assert_eq!(same["source"], "phpstan");
    assert_eq!(items.len(), 3);
}

#[test]
fn failing_to_get_an_answer_fails_the_check_and_ours_are_still_shown() {
    // nothing installed
    let (_tmp, root) = project();
    fs::remove_file(root.join("vendor/bin/phpstan")).unwrap();
    write(&root.join("src/main.php"), "<?php\n");
    stirphp(&root)
        .args(["check", "--with-phpstan"])
        .assert()
        .failure()
        .stderr(contains("isn't installed"))
        .stdout(contains("No problems found"));
    // not on the command line it's not; without the option it's fine
    stirphp(&root).arg("check").assert().success();

    // a broken configuration
    let (_tmp, root) = project();
    write(&root.join("src/main.php"), "<?php\n");
    write(
        &root.join("behave"),
        "echo 'Unexpected end of file.' >&2\nexit 1\n",
    );
    stirphp(&root)
        .args(["check", "--with-phpstan"])
        .assert()
        .failure()
        .stderr(contains("Unexpected end of file."))
        .stdout(contains("No problems found"));

    // what it says about the run as a whole
    let (_tmp, root) = project();
    write(&root.join("src/main.php"), "<?php\n");
    report(
        &root,
        &[],
        json!(["Ignored error pattern #x# was not matched."]),
    );
    stirphp(&root)
        .args(["check", "--with-phpstan"])
        .assert()
        .failure()
        .stderr(contains("Ignored error pattern #x#"));
    // and nothing at all to say is a success
    let (_tmp, root) = project();
    write(&root.join("src/main.php"), "<?php\n");
    report(&root, &[], json!([]));
    stirphp(&root)
        .args(["check", "--with-phpstan"])
        .assert()
        .success();
}

#[test]
fn level_and_configuration_reach_it() {
    let (_tmp, root) = project();
    write(&root.join("src/main.php"), "<?php\n");
    write(&root.join("ci.neon"), "parameters:\n    level: 1\n");
    report(&root, &[], json!([]));
    stirphp(&root)
        .args(["check", "--with-phpstan", "--level", "8", "-c", "ci.neon"])
        .assert()
        .success();
    let runs = fs::read_to_string(root.join("runs")).unwrap();
    assert!(runs.contains("--level=8"), "{runs}");
    assert!(
        runs.contains(&format!(
            "--configuration {}",
            root.join("ci.neon").display()
        )),
        "{runs}"
    );
    // our own file isn't one for PHPStan to read
    write(&root.join("stirphp.neon"), "parameters:\n    level: 1\n");
    stirphp(&root)
        .args(["check", "--with-phpstan", "-c", "stirphp.neon"])
        .assert()
        .success();
    let runs = fs::read_to_string(root.join("runs")).unwrap();
    assert!(
        !runs.lines().last().unwrap().contains("stirphp.neon"),
        "{runs}"
    );
}

#[test]
fn it_is_not_for_a_baseline() {
    let (_tmp, root) = project();
    stirphp(&root)
        .args(["check", "--with-phpstan", "--generate-baseline"])
        .assert()
        .failure()
        .stderr(contains("cannot be used with"));
}

/// The real server, spoken to over its stdio, with the workspace's PHPStan allowed.
struct Lsp {
    child: Child,
    stdin: Option<ChildStdin>,
    messages: mpsc::Receiver<Value>,
}

impl Lsp {
    fn start(root: &Path) -> Lsp {
        let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_stirphp"))
            .arg("lsp")
            .current_dir(root)
            .env("XDG_CONFIG_HOME", root.parent().unwrap().join("config"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let (tx, messages) = mpsc::channel();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut len = 0;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    let line = line.trim_end();
                    if line.is_empty() {
                        break;
                    }
                    if let Some(v) = line.strip_prefix("Content-Length: ") {
                        len = v.parse().unwrap();
                    }
                }
                let mut body = vec![0; len];
                if reader.read_exact(&mut body).is_err() {
                    return;
                }
                let _ = tx.send(serde_json::from_slice(&body).unwrap());
            }
        });
        let mut lsp = Lsp {
            child,
            stdin,
            messages,
        };
        lsp.send(
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "processId": null,
                "rootUri": format!("file://{}", root.display()),
                "capabilities": {"window": {"showMessage": {}}},
            }}),
        );
        while lsp.messages.recv_timeout(Duration::from_secs(60)).unwrap()["id"] != 1 {}
        lsp.send(json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}));
        let question = lsp.wait_for("window/showMessageRequest");
        lsp.send(json!({"jsonrpc": "2.0", "id": question["id"], "result": {"title": "Allow"}}));
        lsp
    }

    fn send(&mut self, message: Value) {
        let body = message.to_string();
        let stdin = self.stdin.as_mut().unwrap();
        write!(stdin, "Content-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        stdin.flush().unwrap();
    }

    fn wait_for(&self, method: &str) -> Value {
        loop {
            let message = self.messages.recv_timeout(Duration::from_secs(60)).unwrap();
            if message["method"] == method {
                return message;
            }
        }
    }

    fn exits(&mut self) {
        let end = Instant::now() + Duration::from_secs(20);
        while Instant::now() < end {
            if self.child.try_wait().unwrap().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        panic!("the server didn't exit");
    }
}

fn alive(pid: &str) -> bool {
    fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
        !stat
            .rsplit(')')
            .next()
            .unwrap_or("")
            .trim_start()
            .starts_with('Z')
    })
}

/// A hanging run, with what it started.
fn hanging_run(root: &Path) -> Vec<String> {
    let pids = root.join("pids");
    let end = Instant::now() + Duration::from_secs(60);
    while Instant::now() < end {
        let found: Vec<String> = fs::read_to_string(&pids)
            .unwrap_or_default()
            .split_whitespace()
            .map(str::to_owned)
            .collect();
        if found.len() == 2 {
            assert!(found.iter().all(|pid| alive(pid)));
            return found;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("no run");
}

fn assert_gone(pids: &[String]) {
    let end = Instant::now() + Duration::from_secs(10);
    while Instant::now() < end {
        if !pids.iter().any(|pid| alive(pid)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("still there: {pids:?}");
}

fn hanging_project() -> (TempDir, PathBuf) {
    let (tmp, root) = project();
    write(
        &root.join("behave"),
        "sleep 1000 &\necho $! >> pids\nwait\n",
    );
    (tmp, root)
}

#[test]
fn shutdown_and_exit_leave_nothing_running() {
    let (_tmp, root) = hanging_project();
    let mut lsp = Lsp::start(&root);
    let pids = hanging_run(&root);
    lsp.send(json!({"jsonrpc": "2.0", "id": 2, "method": "shutdown"}));
    lsp.send(json!({"jsonrpc": "2.0", "method": "exit"}));
    lsp.exits();
    assert_gone(&pids);
}

#[test]
fn a_client_going_away_leaves_nothing_running() {
    let (_tmp, root) = hanging_project();
    let mut lsp = Lsp::start(&root);
    let pids = hanging_run(&root);
    drop(lsp.stdin.take());
    lsp.exits();
    assert_gone(&pids);
}

#[test]
fn being_told_to_stop_leaves_nothing_running() {
    let (_tmp, root) = hanging_project();
    let mut lsp = Lsp::start(&root);
    let pids = hanging_run(&root);
    let status = std::process::Command::new("kill")
        .args(["-TERM", &lsp.child.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    lsp.exits();
    assert_gone(&pids);
}
