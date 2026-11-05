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
        .stdout(contains("error[phpstan.parse]"))
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
        .stdout(contains("return.type"));
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
    assert_eq!(first["code"], "phpstan.parse");
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

#[test]
fn missing_composer_packages_are_one_notice_not_a_flood() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    write(
        &root.join("composer.json"),
        r#"{"require": {"monolog/monolog": "^3"}, "autoload": {"psr-4": {"App\\": "src/"}}}"#,
    );
    write(
        &root.join("src/Service.php"),
        "<?php\nnamespace App;\nuse Monolog\\Logger;\nuse Monolog\\Handler\\StreamHandler;\n\
         class Service extends Logger {\n    public function __construct(private Logger $log) {\n\
         $this->log->pushHandler(new StreamHandler('php://stdout'));\n        $this->anything();\n\
         \\Monolog\\Registry::getInstance('x');\n        helper_from_a_package();\n    }\n}\n",
    );
    sphp()
        .current_dir(root)
        .args(["check", "--no-vendor"])
        .assert()
        .success()
        .stderr(contains("Run `composer install`"))
        .stderr(contains("Held back so far: 5."))
        .stdout(contains("No problems found"));

    // what's the project's own is still wrong
    write(
        &root.join("src/Own.php"),
        "<?php\nnamespace App;\nnew Missing();\n",
    );
    sphp()
        .current_dir(root)
        .arg("check")
        .assert()
        .code(1)
        .stdout(contains("class.notFound"))
        .stdout(contains("Own.php"));
}

#[test]
fn notices_stay_out_of_the_json() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    write(&root.join("composer.json"), r#"{"require": {"a/b": "*"}}"#);
    write(&root.join("a.php"), "<?php\nnew \\A\\B();\n");
    let output = sphp()
        .current_dir(root)
        .args(["check", "--format", "json"])
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json, serde_json::json!([]));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(stderr.matches("notice:").count(), 1, "{stderr}");
}

#[test]
fn newer_syntax_than_composer_declares() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    write(
        &root.join("composer.json"),
        r#"{"require": {"php": "^8.0"}}"#,
    );
    write(&root.join("a.php"), "<?php\nenum E {}\n");
    sphp()
        .current_dir(root)
        .arg("check")
        .assert()
        .code(1)
        .stdout(contains("SPHP0060"))
        .stdout(contains(
            "PHP 8.1 added enums, but this project targets PHP 8.0",
        ));
    write(
        &root.join("composer.json"),
        r#"{"require": {"php": "^8.1"}}"#,
    );
    sphp().current_dir(root).arg("check").assert().success();
}

/// A project with errors: two of one identifier in one file, one in another.
fn project_with_errors() -> TempDir {
    let tmp = TempDir::new().unwrap();
    write(
        &tmp.path().join("src/a.php"),
        "<?php\nfunction one(): int { return 'a'; }\nfunction two(): int { return 'b'; }\necho missing_one();\n",
    );
    write(
        &tmp.path().join("src/b.php"),
        "<?php\necho missing_two();\n",
    );
    tmp
}

#[test]
fn phpstans_ignore_errors_by_identifier_path_and_count() {
    let tmp = project_with_errors();
    let root = tmp.path();
    write(
        &root.join("phpstan.neon"),
        "parameters:
    ignoreErrors:
        -
            # a message on top of both: those two, however it's worded
            message: '#^Function one\\(\\) should return#'
            identifier: return.type
            path: src/a.php
            count: 1
        -
            identifier: function.notFound
        - '#a message alone#'
        -
            message: '#a message and an identifier, no path#'
            identifier: return.type
",
    );
    sphp()
        .current_dir(root)
        .arg("check")
        .assert()
        .code(1)
        .stdout(contains("1 error, 0 warnings"))
        .stdout(contains("return.type"))
        .stderr(contains("Skipped 2 `ignoreErrors` entries"));
    // what's left of PHPStan's matching nothing is never mentioned
    write(
        &root.join("phpstan.neon"),
        "parameters:\n    ignoreErrors:\n        -\n            identifier: nothing.reportsThis\n",
    );
    sphp()
        .current_dir(root)
        .arg("check")
        .assert()
        .code(1)
        .stderr(contains("nothing.reportsThis").not());
}

#[test]
fn a_php_baseline_is_part_of_the_notice() {
    let tmp = project_with_errors();
    let root = tmp.path();
    write(&root.join("base.php"), "<?php return [];\n");
    write(
        &root.join("phpstan.neon"),
        "includes:\n    - base.php\nparameters:\n    level: 5\n",
    );
    sphp()
        .current_dir(root)
        .arg("check")
        .assert()
        .code(1)
        .stderr(contains("base.php"));
}

#[test]
fn nothing_hides_a_syntax_error() {
    let tmp = project_with_errors();
    let root = tmp.path();
    write(
        &root.join("src/bad.php"),
        "<?php\n$x = ; // @phpstan-ignore-line\n",
    );
    write(
        &root.join("phpstan.neon"),
        "parameters:\n    ignoreErrors:\n        -\n            path: src\n",
    );
    sphp()
        .current_dir(root)
        .arg("check")
        .assert()
        .code(1)
        .stdout(contains("phpstan.parse"))
        .stdout(contains("1 error, 0 warnings"));
}

#[test]
fn init_writes_what_check_reads() {
    let tmp = project();
    sphp()
        .current_dir(tmp.path())
        .arg("init")
        .assert()
        .success()
        .stdout(contains("stirphp.neon"));
    sphp()
        .current_dir(tmp.path())
        .arg("check")
        .assert()
        .success();

    let tmp = project();
    sphp()
        .current_dir(tmp.path())
        .args(["init", "--phpstan"])
        .assert()
        .success()
        .stdout(contains("phpstan.neon"));
    assert!(!tmp.path().join("stirphp.neon").exists());
    sphp()
        .current_dir(tmp.path())
        .arg("check")
        .assert()
        .success();
    sphp()
        .current_dir(tmp.path())
        .arg("init")
        .assert()
        .success()
        .stdout(contains("set up already"));
}
