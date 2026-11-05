use std::fs;
use std::path::Path;

use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt as _;
use predicates::str::contains;
use tempfile::TempDir;

fn stirphp() -> Command {
    let mut cmd = Command::cargo_bin("stirphp").unwrap();
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
    stirphp()
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
    stirphp()
        .current_dir(tmp.path())
        .arg("check")
        .assert()
        .code(1)
        .stdout(contains("error[phpstan.parse]"))
        .stdout(contains("src/bad.php"))
        .stdout(contains("1 error, 0 warnings in 1 file"));

    // Restricting to another path hides it.
    stirphp()
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
        "<?php\n// @stirphp-nocheck\n$x = ;\n",
    );
    stirphp()
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
    stirphp()
        .current_dir(tmp.path())
        .args(["check", "--level", "0"])
        .assert()
        .success();
    stirphp()
        .current_dir(tmp.path())
        .args(["check", "--level", "3"])
        .assert()
        .code(1)
        .stdout(contains("return.type"));
    stirphp()
        .current_dir(tmp.path())
        .args(["check", "--level", "11"])
        .assert()
        .code(2);
}

#[test]
fn json_output() {
    let tmp = project();
    write(&tmp.path().join("src/bad.php"), "<?php\n$x = 1;\n$y = (\n");
    let output = stirphp()
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
    let output = stirphp()
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
    stirphp()
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
    stirphp()
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
    stirphp()
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
    let output = stirphp()
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
    stirphp()
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
    stirphp().current_dir(root).arg("check").assert().success();
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

const BASELINE: &str = "parameters:
\tignoreErrors:
\t\t-
\t\t\tidentifier: function.notFound
\t\t\tcount: 1
\t\t\tpath: src/a.php
\t\t-
\t\t\tidentifier: function.notFound
\t\t\tcount: 1
\t\t\tpath: src/b.php
\t\t-
\t\t\tidentifier: return.type
\t\t\tcount: 2
\t\t\tpath: src/a.php
";

#[test]
fn a_baseline_hides_what_was_there_and_only_that() {
    let tmp = project_with_errors();
    let root = tmp.path();
    stirphp()
        .current_dir(root)
        .arg("check")
        .assert()
        .code(1)
        .stdout(contains("4 errors"));

    stirphp()
        .current_dir(root)
        .args(["check", "--generate-baseline"])
        .assert()
        .success()
        .stdout(contains("Baseline generated with 4 errors"))
        .stdout(contains("stirphp.neon"));
    let baseline = root.join("stirphp-baseline.neon");
    assert_eq!(fs::read_to_string(&baseline).unwrap(), BASELINE);
    assert!(
        fs::read_to_string(root.join("stirphp.neon"))
            .unwrap()
            .contains("baseline: stirphp-baseline.neon")
    );

    // nothing's reported, or said about the baseline
    stirphp()
        .current_dir(root)
        .arg("check")
        .assert()
        .success()
        .stdout(contains("No problems found"))
        .stderr(contains("baseline").not());

    // generating again changes nothing, whatever the baseline already hides
    let config = fs::read_to_string(root.join("stirphp.neon")).unwrap();
    stirphp()
        .current_dir(root)
        .args(["check", "--generate-baseline"])
        .assert()
        .success();
    assert_eq!(fs::read_to_string(&baseline).unwrap(), BASELINE);
    assert_eq!(
        fs::read_to_string(root.join("stirphp.neon")).unwrap(),
        config
    );

    // a new error of an identifier that's in there shows once the count is exceeded
    write(
        &root.join("src/a.php"),
        "<?php\nfunction one(): int { return 'a'; }\nfunction two(): int { return 'b'; }\nfunction three(): int { return 'c'; }\necho missing_one();\n",
    );
    stirphp()
        .current_dir(root)
        .arg("check")
        .assert()
        .code(1)
        .stdout(contains("return.type"))
        .stdout(contains("1 error, 0 warnings"));

    // fewer errors than it says: a warning, not an error
    write(
        &root.join("src/a.php"),
        "<?php\nfunction one(): int { return 'a'; }\necho missing_one();\n",
    );
    fs::remove_file(root.join("src/b.php")).unwrap();
    stirphp()
        .current_dir(root)
        .arg("check")
        .assert()
        .success()
        .stderr(contains("warning: 2 entries of the baseline"))
        .stderr(contains("return.type in src/a.php: expected 2, found 1"))
        .stderr(contains(
            "function.notFound in src/b.php: expected 1, found 0",
        ));
}

#[test]
fn a_baseline_at_a_path_of_its_own() {
    let tmp = project_with_errors();
    let root = tmp.path();
    stirphp()
        .current_dir(root)
        .args(["check", "--generate-baseline", "conf/base.neon"])
        .assert()
        .success();
    let text = fs::read_to_string(root.join("conf/base.neon")).unwrap();
    assert!(text.contains("path: ../src/a.php"), "{text}");
    // the configuration's in the project folder, and so is this
    stirphp().current_dir(root).arg("check").assert().success();

    // outside what the configuration's about, it's not touched
    let elsewhere = TempDir::new().unwrap();
    let tmp = project_with_errors();
    let target = elsewhere.path().join("b.neon");
    stirphp()
        .current_dir(tmp.path())
        .args(["check", "--generate-baseline", target.to_str().unwrap()])
        .assert()
        .success()
        .stdout(contains("To use it"));
    assert!(!tmp.path().join("stirphp.neon").exists());
    assert!(target.is_file());
}

#[test]
fn a_baseline_through_a_config_elsewhere() {
    let tmp = project_with_errors();
    let conf = TempDir::new().unwrap();
    let config = conf.path().join("my.neon");
    write(&config, "parameters:\n    level: 5\n");
    let args = ["check", "-c", config.to_str().unwrap()];
    stirphp()
        .current_dir(tmp.path())
        .args(args)
        .arg("--generate-baseline")
        .assert()
        .success();
    assert!(conf.path().join("stirphp-baseline.neon").is_file());
    assert!(!tmp.path().join("stirphp-baseline.neon").exists());
    stirphp()
        .current_dir(tmp.path())
        .args(args)
        .assert()
        .success();
}

#[test]
fn syntax_errors_stay_out_of_baselines() {
    let tmp = project_with_errors();
    let root = tmp.path();
    write(&root.join("src/bad.php"), "<?php\n$x = ;\n");
    stirphp()
        .current_dir(root)
        .args(["check", "--generate-baseline"])
        .assert()
        .code(1)
        .stdout(contains("phpstan.parse"))
        .stdout(contains("1 error can't go in a baseline"));
    let text = fs::read_to_string(root.join("stirphp-baseline.neon")).unwrap();
    assert!(!text.contains("phpstan.parse"), "{text}");
    stirphp()
        .current_dir(root)
        .arg("check")
        .assert()
        .code(1)
        .stdout(contains("1 error, 0 warnings"));

    // and no comment or rule hides them either
    write(
        &root.join("src/bad.php"),
        "<?php\n$x = ; // @phpstan-ignore-line\n",
    );
    fs::remove_file(root.join("stirphp.neon")).unwrap();
    write(
        &root.join("phpstan.neon"),
        "parameters:\n    ignoreErrors:\n        -\n            path: src\n",
    );
    stirphp()
        .current_dir(root)
        .arg("check")
        .assert()
        .code(1)
        .stdout(contains("phpstan.parse"))
        .stdout(contains("1 error, 0 warnings"));
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
    stirphp()
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
    stirphp()
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
    stirphp()
        .current_dir(root)
        .arg("check")
        .assert()
        .code(1)
        .stderr(contains("base.php"));
}

#[test]
fn init_writes_what_check_reads() {
    let tmp = project();
    stirphp()
        .current_dir(tmp.path())
        .arg("init")
        .assert()
        .success()
        .stdout(contains("stirphp.neon"));
    stirphp()
        .current_dir(tmp.path())
        .arg("check")
        .assert()
        .success();

    let tmp = project();
    stirphp()
        .current_dir(tmp.path())
        .args(["init", "--phpstan"])
        .assert()
        .success()
        .stdout(contains("phpstan.neon"));
    assert!(!tmp.path().join("stirphp.neon").exists());
    stirphp()
        .current_dir(tmp.path())
        .arg("check")
        .assert()
        .success();
    stirphp()
        .current_dir(tmp.path())
        .arg("init")
        .assert()
        .success()
        .stdout(contains("set up already"));
}

const NO_VAR_DUMP: &str = r#"
fn visit_func_call(node, ctx) {
    if node.name.to_lower() == "var_dump" {
        ctx.error("Don't leave var_dump in", "app.noVarDump");
    }
}
"#;

/// A project with rules in `rules/`, whose config is in `conf/` to show what paths are relative to.
fn project_with_rule(script: &str) -> TempDir {
    let tmp = project();
    write(&tmp.path().join("conf/rule.rhai"), script);
    write(
        &tmp.path().join("stirphp.neon"),
        "includes:\n    - conf/rules.neon\n",
    );
    write(
        &tmp.path().join("conf/rules.neon"),
        "parameters:\n    stirphp:\n        rules:\n            - rule.rhai\n",
    );
    tmp
}

#[test]
fn a_rule_reports_through_the_usual_pipeline() {
    let tmp = project_with_rule(NO_VAR_DUMP);
    let source = tmp.path().join("src/main.php");
    write(
        &source,
        "<?php\nvar_dump(1);\nstrlen('a');\n\\VAR_DUMP(2);\n",
    );
    let out = stirphp()
        .current_dir(tmp.path())
        .args(["check", "--format", "json"])
        .assert()
        .code(1)
        .get_output()
        .stdout
        .clone();
    let found: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let found = found.as_array().unwrap();
    assert_eq!(found.len(), 2, "{found:?}");
    assert_eq!(found[0]["code"], "app.noVarDump");
    assert_eq!(found[0]["message"], "Don't leave var_dump in");
    assert_eq!(found[0]["severity"], "error");
    assert!(found[0]["file"].as_str().unwrap().ends_with("main.php"));

    // a comment, for one of them
    write(
        &source,
        "<?php\n// @phpstan-ignore app.noVarDump\nvar_dump(1);\nvar_dump(2);\n",
    );
    stirphp()
        .current_dir(tmp.path())
        .arg("check")
        .assert()
        .code(1)
        .stdout(contains("1 error, 0 warnings"));

    // `ignoreErrors`
    write(
        &tmp.path().join("stirphp.neon"),
        "includes:\n    - conf/rules.neon\nparameters:\n    ignoreErrors:\n        - identifier: app.noVarDump\n",
    );
    stirphp()
        .current_dir(tmp.path())
        .arg("check")
        .assert()
        .success();

    // and a baseline
    write(
        &tmp.path().join("stirphp.neon"),
        "includes:\n    - conf/rules.neon\n",
    );
    stirphp()
        .current_dir(tmp.path())
        .args(["check", "--generate-baseline"])
        .assert()
        .success();
    let baseline = fs::read_to_string(tmp.path().join("stirphp-baseline.neon")).unwrap();
    assert!(baseline.contains("app.noVarDump"), "{baseline}");
    stirphp()
        .current_dir(tmp.path())
        .arg("check")
        .assert()
        .success();
}

#[test]
fn a_rule_applies_at_every_level_unless_it_says_otherwise() {
    let tmp = project_with_rule(
        r#"fn visit_echo(node, ctx) {
            ctx.warning("no echo", "app.noEcho");
            ctx.error("no echo here either", "app.noEchoLate", #{ level: 6 });
        }"#,
    );
    let check = |level: &str| {
        let out = stirphp()
            .current_dir(tmp.path())
            .args(["check", "--format", "json", "--level", level])
            .output()
            .unwrap();
        let found: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        found
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["code"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(check("0"), ["app.noEcho"]);
    assert_eq!(check("6"), ["app.noEchoLate", "app.noEcho"]);
}

#[test]
fn a_rule_can_ask_for_types() {
    let tmp = project_with_rule(
        r#"fn visit_method_call(node, ctx) {
            let t = ctx.get_type(node.receiver);
            if t.is_subtype_of("App\\Legacy") {
                ctx.error(`no ${node.name}() on ${t.to_string()}`, "app.noLegacy");
            }
        }"#,
    );
    write(
        &tmp.path().join("src/main.php"),
        "<?php\nnamespace App;\nclass Legacy { public function run(): void {} }\nclass Old extends Legacy {}\nclass Fine { public function run(): void {} }\n(new Old())->run();\n(new Fine())->run();\n",
    );
    stirphp()
        .current_dir(tmp.path())
        .arg("check")
        .assert()
        .code(1)
        .stdout(contains("no run() on App\\Old"))
        .stdout(contains("1 error, 0 warnings"));
}

#[test]
fn a_script_that_fails_says_so_once_and_the_rest_goes_on() {
    let tmp = project_with_rule("fn visit_echo(node, ctx) { loop {} }");
    write(
        &tmp.path().join("src/other.php"),
        "<?php\necho 2;\nmissing();\n",
    );
    stirphp()
        .current_dir(tmp.path())
        .arg("check")
        .assert()
        .code(1)
        .stdout(contains("error[SPHP0070]"))
        .stdout(contains("rule.rhai"))
        .stdout(contains("operations"))
        .stdout(contains("it failed for 2 files"))
        .stdout(contains("function.notFound"))
        .stdout(contains("2 errors, 0 warnings"));
}

#[test]
fn a_script_with_a_syntax_error_says_where() {
    let tmp = project_with_rule("fn visit_echo(node, ctx) {\n  let = 1;\n}\n");
    stirphp()
        .current_dir(tmp.path())
        .arg("check")
        .assert()
        .code(1)
        .stdout(contains("error[SPHP0071]"))
        .stdout(contains("conf/rule.rhai:2:"))
        .stdout(contains("syntax error"));
}

#[test]
fn a_script_that_is_not_there_says_so() {
    let tmp = project_with_rule("");
    fs::remove_file(tmp.path().join("conf/rule.rhai")).unwrap();
    stirphp()
        .current_dir(tmp.path())
        .arg("check")
        .assert()
        .code(1)
        .stdout(contains("error[SPHP0071]"))
        .stdout(contains("can't be read"));
}

#[test]
fn a_rule_for_nothing_php_has_is_refused() {
    let tmp = project_with_rule("fn visit_nope(node, ctx) {}");
    stirphp()
        .current_dir(tmp.path())
        .arg("check")
        .assert()
        .code(1)
        .stdout(contains("no `nope` nodes"))
        .stdout(contains("func_call"));
}

#[test]
fn a_config_from_elsewhere_finds_its_scripts_next_to_it() {
    let tmp = project();
    let scratch = TempDir::new().unwrap();
    write(&scratch.path().join("r/rule.rhai"), NO_VAR_DUMP);
    write(
        &scratch.path().join("c.neon"),
        "parameters:\n    stirphp:\n        rules:\n            - r/rule.rhai\n",
    );
    write(&tmp.path().join("src/main.php"), "<?php\nvar_dump(1);\n");
    stirphp()
        .current_dir(tmp.path())
        .arg("check")
        .arg("-c")
        .arg(scratch.path().join("c.neon"))
        .assert()
        .code(1)
        .stdout(contains("app.noVarDump"));
}

const CONTAINER: &str = r#"
fn return_type_targets() {
    #{
        methods: ["App\\Container::get"],
        static_methods: ["App\\Container::make"],
        functions: ["App\\app"],
    }
}

fn answer(node, ctx) {
    if node.arguments.is_empty() {
        return "App\\Container";
    }
    let class = ctx.get_type(node.arguments[0]).class_string;
    if class == () {
        return;
    }
    class
}

fn return_type_of_method(node, ctx) { answer(node, ctx) }
fn return_type_of_static_method(node, ctx) { answer(node, ctx) }
fn return_type_of_function(node, ctx) { answer(node, ctx) }
"#;

const CONTAINER_PHP: &str = r#"<?php
declare(strict_types=1);
namespace App;
class User { public function name(): string { return ''; } }
class Container {
    public function get(string $id): mixed { return null; }
    public static function make(string $id): mixed { return null; }
}
class Sub extends Container {}
function app(string $id = ''): mixed { return null; }
/** @param string $id */
function wants_int(int $n, string $id): void {}

function run(Sub $c, string $dynamic): void {
    wants_int($c->get(User::class)->name(), $dynamic);
    wants_int(Container::make(User::class)->name(), $dynamic);
    wants_int(app(User::class)->name(), $dynamic);
    wants_int($c->get($dynamic), $dynamic);
    $c->get(User::class)->nope();
}
"#;

fn codes_of(out: &[u8]) -> Vec<(String, String)> {
    let found: serde_json::Value = serde_json::from_slice(out).unwrap();
    found
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            (
                d["code"].as_str().unwrap().to_owned(),
                d["message"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

#[test]
fn a_hook_says_what_a_call_returns() {
    let tmp = project_with_rule(CONTAINER);
    write(&tmp.path().join("src/main.php"), CONTAINER_PHP);
    let out = stirphp()
        .current_dir(tmp.path())
        .args(["check", "--format", "json", "--level", "9"])
        .output()
        .unwrap();
    let found = codes_of(&out.stdout);
    let messages: Vec<&str> = found.iter().map(|(_, m)| m.as_str()).collect();
    // the three calls of `name()` go through what the hook answered, and give `string`
    let wrong = found
        .iter()
        .filter(|(c, m)| c == "argument.type" && m.contains("string") && !m.contains("mixed"));
    assert_eq!(wrong.count(), 3, "{messages:?}");
    // what it doesn't answer for stays as declared
    assert!(messages.iter().any(|m| m.contains("mixed")), "{messages:?}");
    assert!(
        found
            .iter()
            .any(|(c, m)| c == "method.notFound" && m.contains("nope")),
        "{messages:?}"
    );
}

#[test]
fn a_hook_that_fails_says_so_and_the_call_keeps_its_declared_type() {
    let hook = |body: &str| {
        format!(
            r#"fn return_type_targets() {{ #{{ methods: ["App\\Container::get"] }} }}
            fn return_type_of_method(node, ctx) {{ {body} }}"#
        )
    };
    let run = |script: &str| {
        let tmp = project_with_rule(script);
        write(&tmp.path().join("src/main.php"), CONTAINER_PHP);
        let out = stirphp()
            .current_dir(tmp.path())
            .args(["check", "--format", "json", "--level", "0"])
            .output()
            .unwrap();
        codes_of(&out.stdout)
    };
    for (body, says) in [
        ("loop {}", "operations"),
        ("return_type_of_method(node, ctx)", "too deep"),
        (r#""list<""#, "isn't a type"),
        ("1", "PHPDoc type"),
        (r#"ctx.error("x", "app.x")"#, "only rules"),
    ] {
        let found = run(&hook(body));
        let script: Vec<_> = found.iter().filter(|(c, _)| c == "SPHP0070").collect();
        assert_eq!(script.len(), 1, "{body}: {found:?}");
        assert!(script[0].1.contains(says), "{body}: {found:?}");
    }
    // naming what doesn't exist isn't the script's failure: it just doesn't count
    let found = run(&hook(r#""App\\Missing""#));
    assert!(
        !found.iter().any(|(c, _)| c.starts_with("SPHP")),
        "{found:?}"
    );
}

#[test]
fn a_hook_cannot_ask_for_the_call_it_is_answering() {
    let tmp = project_with_rule(
        r#"fn return_type_targets() { #{ methods: ["App\\Container::get"] } }
        fn return_type_of_method(node, ctx) {
            if ctx.get_type(node).is_unknown() && ctx.declared_return_type.to_string() == "mixed" {
                return "App\\User";
            }
        }"#,
    );
    write(&tmp.path().join("src/main.php"), CONTAINER_PHP);
    let out = stirphp()
        .current_dir(tmp.path())
        .args(["check", "--format", "json", "--level", "2"])
        .output()
        .unwrap();
    let found = codes_of(&out.stdout);
    assert!(
        found.iter().all(|(c, _)| !c.starts_with("SPHP")),
        "{found:?}"
    );
    assert!(
        found.iter().any(|(c, _)| c == "method.notFound"),
        "{found:?}"
    );
}

#[test]
fn magic_members_are_found_by_the_checker() {
    let tmp = project_with_rule(
        r#"fn magic_members(class_name) {
            if class_name == "App\\Model" {
                #{ properties: #{ title: "string" }, methods: #{ find: #{ returns: "static", is_static: true } } }
            }
        }"#,
    );
    write(
        &tmp.path().join("src/main.php"),
        "<?php\ndeclare(strict_types=1);\nnamespace App;\nclass Model {}\nclass Post extends Model {}\nfunction f(int $n): void {}\nf(Post::find()->title);\nPost::find()->nope;\n",
    );
    let out = stirphp()
        .current_dir(tmp.path())
        .args(["check", "--format", "json", "--level", "5"])
        .output()
        .unwrap();
    let found = codes_of(&out.stdout);
    assert_eq!(found.len(), 2, "{found:?}");
    assert_eq!(found[0].0, "argument.type", "{found:?}");
    assert_eq!(found[1].0, "property.notFound", "{found:?}");
}
