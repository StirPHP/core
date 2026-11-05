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

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

/// A project with no configuration at all.
fn project() -> TempDir {
    let tmp = TempDir::new().unwrap();
    write(&tmp.path().join("src/main.php"), "<?php\necho 1;\n");
    tmp
}

#[test]
fn a_project_needs_no_config() {
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
    write(&tmp.path().join("src/bad.php"), "<?php\n$x = ;\n");
    sphp()
        .current_dir(tmp.path())
        .arg("check")
        .assert()
        .code(1)
        .stdout(contains("error[SPHP0001]"))
        .stdout(contains("src/bad.php"))
        .stdout(contains("1 error, 0 warnings in 1 file"));

    // Restricting to another path hides it.
    sphp()
        .current_dir(tmp.path())
        .args(["check", "src/main.php"])
        .assert()
        .success()
        .stdout(contains("No problems found in 1 file"));
}

#[test]
fn nocheck_opts_a_file_out() {
    let tmp = project();
    write(
        &tmp.path().join("src/skipped.php"),
        "<?php\n// @sphp-nocheck\n$x = ;\n",
    );
    sphp()
        .current_dir(tmp.path())
        .arg("check")
        .assert()
        .success()
        .stdout(contains("No problems found in 1 file"));
}

#[test]
fn level_flag_overrides_the_default() {
    let tmp = project();
    write(
        &tmp.path().join("src/typed.php"),
        "<?php\nfunction f(int $n): string { return $n; }\n",
    );
    sphp()
        .current_dir(tmp.path())
        .args(["check", "--level", "0"])
        .assert()
        .success();
    sphp()
        .current_dir(tmp.path())
        .args(["check", "--level", "3"])
        .assert()
        .code(1)
        .stdout(contains("SPHP0005"));
    sphp()
        .current_dir(tmp.path())
        .args(["check", "--level", "11"])
        .assert()
        .code(2);
}

#[test]
fn json_output() {
    let tmp = project();
    write(&tmp.path().join("src/bad.php"), "<?php\n$x = 1;\n$y = (\n");
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
    assert_eq!(first["file"], "src/bad.php");
    assert_eq!(first["code"], "SPHP0001");
    assert_eq!(first["severity"], "error");
    // The error is at or after the unclosed `(` on line 3.
    let line = first["line"].as_u64().unwrap();
    assert!(line >= 3, "{first}");
    assert!(first["endLine"].as_u64().unwrap() >= line);
    assert!(first["labels"].is_array() && first["notes"].is_array());
}

#[test]
fn json_position_is_one_based() {
    let tmp = project();
    write(&tmp.path().join("src/bad.php"), "<?php\n$ç = ;\n");
    let output = sphp()
        .current_dir(tmp.path())
        .args(["check", "--format", "json"])
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let first = &json[0];
    assert_eq!(first["line"], 2, "{json}");
    // `;` is the 6th character (the 7th byte).
    assert_eq!(first["column"], 6, "{json}");
}

#[test]
fn max_errors_truncates() {
    let tmp = project();
    write(&tmp.path().join("src/a.php"), "<?php\n$x = ;\n");
    write(&tmp.path().join("src/b.php"), "<?php\n$x = ;\n");
    sphp()
        .current_dir(tmp.path())
        .args(["check", "--max-errors", "1"])
        .assert()
        .code(1)
        .stdout(contains("src/a.php"))
        .stdout(contains("src/b.php").not())
        .stdout(contains("1 more not shown"))
        .stdout(contains("2 errors, 0 warnings in 2 files"));
}
