use std::fs;
use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt as _;
use predicates::str::contains;
use tempfile::TempDir;

fn sphp() -> Command {
    let mut cmd = Command::cargo_bin("sphp").unwrap();
    cmd.env("NO_COLOR", "1");
    cmd
}

fn project() -> TempDir {
    let tmp = TempDir::new().unwrap();
    sphp()
        .current_dir(tmp.path())
        .args(["init", "--yes", "--name", "demo"])
        .assert()
        .success();
    tmp
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

#[test]
fn fresh_project_has_no_problems() {
    let tmp = project();
    sphp()
        .current_dir(tmp.path())
        .arg("check")
        .assert()
        .success()
        .stdout(contains("No problems found in 1 file"));
}

#[test]
fn syntax_error_fails() {
    let tmp = project();
    write(&tmp.path().join("src/bad.sphp"), "val = \n");
    sphp()
        .current_dir(tmp.path())
        .arg("check")
        .assert()
        .code(1)
        .stdout(contains("error[SPHP0001]"))
        .stdout(contains("src/bad.sphp"))
        .stdout(contains("1 error, 0 warnings in 1 file"));

    // Restricting to another path hides it.
    sphp()
        .current_dir(tmp.path())
        .args(["check", "src/main.sphp"])
        .assert()
        .success()
        .stdout(contains("No problems found in 1 file"));
}

#[test]
fn json_output() {
    let tmp = project();
    write(&tmp.path().join("src/bad.sphp"), "val x = 1\nval y = (\n");
    let output = sphp()
        .current_dir(tmp.path())
        .args(["check", "--format", "json", "--no-vendor"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let items = json.as_array().unwrap();
    assert!(!items.is_empty(), "{json}");
    let first = &items[0];
    assert_eq!(first["file"], "src/bad.sphp");
    assert_eq!(first["code"], "SPHP0001");
    assert_eq!(first["severity"], "error");
    // The error is at or after the unclosed `(` on line 2.
    let line = first["line"].as_u64().unwrap();
    let column = first["column"].as_u64().unwrap();
    assert!(line > 2 || (line == 2 && column >= 9), "{first}");
    assert!(first["endLine"].as_u64().unwrap() >= line);
    assert!(first["labels"].is_array() && first["notes"].is_array());
}

#[test]
fn json_position_is_one_based() {
    let tmp = project();
    write(&tmp.path().join("src/bad.sphp"), "val ç = ;\n");
    let output = sphp()
        .current_dir(tmp.path())
        .args(["check", "--format", "json"])
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let first = &json[0];
    assert_eq!(first["line"], 1, "{json}");
    // `;` is the 9th character (the 10th byte).
    assert_eq!(first["column"], 9, "{json}");
}

#[test]
fn max_errors_truncates() {
    let tmp = project();
    write(&tmp.path().join("src/a.sphp"), "val = \n");
    write(&tmp.path().join("src/b.sphp"), "val = \n");
    sphp()
        .current_dir(tmp.path())
        .args(["check", "--max-errors", "1"])
        .assert()
        .code(1)
        .stdout(contains("src/a.sphp"))
        .stdout(contains("src/b.sphp").not())
        .stdout(contains("1 more not shown"))
        .stdout(contains("2 errors, 0 warnings in 2 files"));
}

#[test]
fn missing_config_suggests_init() {
    let tmp = TempDir::new().unwrap();
    sphp()
        .current_dir(tmp.path())
        .arg("check")
        .assert()
        .failure()
        .stderr(contains("sphp.toml"))
        .stderr(contains("sphp init"));
}

#[test]
fn php_sources_checked_only_with_check_php() {
    let tmp = project();
    write(&tmp.path().join("src/legacy.php"), "<?php\n$x = ;\n");
    sphp()
        .current_dir(tmp.path())
        .arg("check")
        .assert()
        .success()
        .stdout(contains("No problems found in 1 file"));

    let config_path = tmp.path().join("sphp.toml");
    let config = fs::read_to_string(&config_path).unwrap();
    let config = config.replace("checkPhp = false", "checkPhp = true");
    assert!(config.contains("checkPhp = true"), "{config}");
    fs::write(&config_path, config).unwrap();
    sphp()
        .current_dir(tmp.path())
        .arg("check")
        .assert()
        .code(1)
        .stdout(contains("src/legacy.php"))
        .stdout(contains("SPHP0001"));
}
