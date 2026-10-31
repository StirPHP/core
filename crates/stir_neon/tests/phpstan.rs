use std::fs;
use std::path::{Path, PathBuf};

use stir_neon::phpstan::{
    ConfigError, ExcludePaths, IgnoreError, IgnoreMessage, Level, LoadOptions, PhpstanConfig,
};
use stir_neon::{Key, Value};

fn write(dir: &Path, name: &str, contents: &str) -> PathBuf {
    let p = dir.join(name);
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&p, contents).unwrap();
    p
}

fn root(dir: &tempfile::TempDir) -> PathBuf {
    fs::canonicalize(dir.path()).unwrap()
}

fn opts() -> LoadOptions {
    LoadOptions::new()
        .param("currentWorkingDirectory", "/cwd")
        .param("rootDir", "/vendor/phpstan/phpstan")
}

#[test]
fn basic_config() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = root(&tmp);
    let file = write(
        &dir,
        "phpstan.neon",
        r#"
parameters:
    level: 6
    paths:
        - src
        - ./tests/../lib
        - /abs/path
    excludePaths:
        - src/Generated
        - */fixtures/*
    bootstrapFiles:
        - bootstrap.php
    scanFiles: [stubs/a.php]
    scanDirectories:
        - %currentWorkingDirectory%/extra
    treatPhpDocTypesAsCertain: false
    typeAliases:
        UserId: 'int<1, max>'
        Shape: 'array{a: int, b?: string}'
    ignoreErrors:
        - '#^Call to an undefined method#'
        -
            message: '#^Access to an undefined property#'
            path: src/Foo.php
            count: 2
        -
            messages:
                - '#one#'
                - '#two#'
            paths:
                - src/A.php
                - src/B/*
            reportUnmatched: false
        -
            identifier: missingType.iterableValue
        -
            rawMessage: 'Exact message.'
            identifier: argument.type
    customParam: %rootDir%/conf
    percent: '100%% sure'
"#,
    );
    let cfg = PhpstanConfig::load_with(&file, &opts()).unwrap();
    assert_eq!(cfg.level, Some(Level::Level(6)));
    assert_eq!(
        cfg.paths,
        vec![dir.join("src"), dir.join("lib"), PathBuf::from("/abs/path")]
    );
    assert_eq!(
        cfg.exclude_paths,
        ExcludePaths {
            analyse: vec![],
            analyse_and_scan: vec![dir.join("src/Generated"), PathBuf::from("*/fixtures/*")],
        }
    );
    assert_eq!(cfg.bootstrap_files, vec![dir.join("bootstrap.php")]);
    assert_eq!(cfg.scan_files, vec![dir.join("stubs/a.php")]);
    assert_eq!(cfg.scan_directories, vec![PathBuf::from("/cwd/extra")]);
    assert_eq!(cfg.treat_phpdoc_types_as_certain, Some(false));
    assert_eq!(cfg.type_aliases["UserId"], "int<1, max>");
    assert_eq!(cfg.type_aliases["Shape"], "array{a: int, b?: string}");
    assert_eq!(cfg.ignore_errors.len(), 5);
    assert_eq!(
        cfg.ignore_errors[0],
        IgnoreError {
            messages: vec![IgnoreMessage::Regex(
                "#^Call to an undefined method#".into()
            )],
            ..Default::default()
        }
    );
    assert_eq!(
        cfg.ignore_errors[1],
        IgnoreError {
            messages: vec![IgnoreMessage::Regex(
                "#^Access to an undefined property#".into()
            )],
            paths: vec![dir.join("src/Foo.php")],
            count: Some(2),
            ..Default::default()
        }
    );
    assert_eq!(
        cfg.ignore_errors[2].paths,
        vec![dir.join("src/A.php"), dir.join("src/B/*")]
    );
    assert_eq!(cfg.ignore_errors[2].messages.len(), 2);
    assert_eq!(cfg.ignore_errors[2].report_unmatched, Some(false));
    assert_eq!(
        cfg.ignore_errors[3].identifiers,
        vec!["missingType.iterableValue".to_string()]
    );
    assert_eq!(
        cfg.ignore_errors[4].messages,
        vec![IgnoreMessage::Raw("Exact message.".into())]
    );
    assert_eq!(
        cfg.parameters.get(&Key::from("customParam")),
        Some(&Value::String("/vendor/phpstan/phpstan/conf".into()))
    );
    assert_eq!(
        cfg.parameters.get(&Key::from("percent")),
        Some(&Value::String("100% sure".into()))
    );
    assert!(cfg.included_files.is_empty());
}

#[test]
fn includes_merge() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = root(&tmp);
    write(
        &dir,
        "conf/base.neon",
        r#"
parameters:
    level: 3
    paths:
        - ../base-src
    excludePaths:
        analyse:
            - ../base-excluded
    ignoreErrors:
        - '#base#'
    typeAliases:
        A: int
    nested:
        keep: 1
        override: 1
    replaced:
        - a
        - b
"#,
    );
    write(
        &dir,
        "conf/second.neon",
        r#"
parameters:
    level: 4
    nested:
        override: 2
        added: 2
"#,
    );
    let file = write(
        &dir,
        "phpstan.neon",
        r#"
includes:
    - conf/base.neon
    - conf/second.neon
parameters:
    level: max
    paths:
        - src
    excludePaths:
        analyseAndScan:
            - tmp
    ignoreErrors:
        - '#root#'
    typeAliases:
        B: string
    replaced!:
        - c
"#,
    );
    let cfg = PhpstanConfig::load_with(&file, &opts()).unwrap();
    assert_eq!(cfg.level, Some(Level::Max));
    // Lists append, base first; paths resolved relative to their own file.
    assert_eq!(cfg.paths, vec![dir.join("base-src"), dir.join("src")]);
    assert_eq!(cfg.exclude_paths.analyse, vec![dir.join("base-excluded")]);
    assert_eq!(cfg.exclude_paths.analyse_and_scan, vec![dir.join("tmp")]);
    let msgs: Vec<_> = cfg
        .ignore_errors
        .iter()
        .map(|e| e.messages[0].clone())
        .collect();
    assert_eq!(
        msgs,
        vec![
            IgnoreMessage::Regex("#base#".into()),
            IgnoreMessage::Regex("#root#".into())
        ]
    );
    assert_eq!(cfg.type_aliases.keys().collect::<Vec<_>>(), vec!["A", "B"]);
    let nested = cfg.parameters.get(&Key::from("nested")).unwrap();
    assert_eq!(
        nested.to_string(),
        r#"{"keep": 1, "override": 2, "added": 2}"#
    );
    // `!` replaces instead of merging, and the bang is stripped.
    let replaced = cfg.parameters.get(&Key::from("replaced")).unwrap();
    assert_eq!(replaced.to_string(), r#"["c"]"#);
    assert!(!cfg.parameters.contains_key(&Key::from("replaced!")));
    assert_eq!(
        cfg.included_files,
        vec![dir.join("conf/base.neon"), dir.join("conf/second.neon")]
    );
    assert!(cfg.raw.get("includes").is_none());
}

#[test]
fn nested_includes_relative_to_includer() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = root(&tmp);
    write(
        &dir,
        "a/b/leaf.neon",
        "parameters:\n    scanFiles: [leaf.php]\n",
    );
    write(&dir, "a/mid.neon", "includes:\n    - b/leaf.neon\n");
    let file = write(&dir, "phpstan.neon", "includes:\n    - a/mid.neon\n");
    let cfg = PhpstanConfig::load_with(&file, &opts()).unwrap();
    assert_eq!(cfg.scan_files, vec![dir.join("a/b/leaf.php")]);
    assert_eq!(
        cfg.included_files,
        vec![dir.join("a/mid.neon"), dir.join("a/b/leaf.neon")]
    );
}

#[test]
fn include_with_known_param() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = root(&tmp);
    write(
        &dir,
        "vendor/ext/rules.neon",
        "parameters:\n    fromExt: yes\n",
    );
    let file = write(
        &dir,
        "phpstan.neon",
        "includes:\n    - %vendorDir%/ext/rules.neon\n",
    );
    let o = opts().param("vendorDir", dir.join("vendor").to_string_lossy());
    let cfg = PhpstanConfig::load_with(&file, &o).unwrap();
    assert_eq!(
        cfg.parameters.get(&Key::from("fromExt")),
        Some(&Value::Bool(true))
    );
}

#[test]
fn include_cycle_detected() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = root(&tmp);
    write(&dir, "a.neon", "includes:\n    - b.neon\n");
    write(&dir, "b.neon", "includes:\n    - a.neon\n");
    let err = PhpstanConfig::load_with(dir.join("a.neon"), &opts()).unwrap_err();
    match err {
        ConfigError::IncludeCycle { chain } => {
            assert_eq!(
                chain,
                vec![dir.join("a.neon"), dir.join("b.neon"), dir.join("a.neon")]
            );
        }
        other => panic!("expected cycle, got {other}"),
    }
}

#[test]
fn diamond_include_is_not_a_cycle() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = root(&tmp);
    write(&dir, "common.neon", "parameters:\n    paths: [common]\n");
    write(&dir, "x.neon", "includes: [common.neon]\n");
    write(&dir, "y.neon", "includes: [common.neon]\n");
    let file = write(&dir, "phpstan.neon", "includes: [x.neon, y.neon]\n");
    let cfg = PhpstanConfig::load_with(&file, &opts()).unwrap();
    assert_eq!(cfg.paths, vec![dir.join("common"), dir.join("common")]);
}

#[test]
fn errors() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = root(&tmp);

    let missing = PhpstanConfig::load_with(dir.join("nope.neon"), &opts()).unwrap_err();
    assert!(matches!(missing, ConfigError::Io { .. }));

    let f = write(&dir, "bad.neon", "parameters:\n  level: [\n");
    let e = PhpstanConfig::load_with(&f, &opts()).unwrap_err();
    let ConfigError::Parse { source, .. } = &e else {
        panic!("expected parse error, got {e}")
    };
    assert_eq!(source.line, 3);

    let f = write(&dir, "level.neon", "parameters:\n  level: 11\n");
    let e = PhpstanConfig::load_with(&f, &opts()).unwrap_err();
    assert!(
        matches!(&e, ConfigError::Invalid { key, .. } if key == "level"),
        "{e}"
    );

    let f = write(
        &dir,
        "ign.neon",
        "parameters:\n  ignoreErrors:\n    - {path: x.php}\n",
    );
    let e = PhpstanConfig::load_with(&f, &opts()).unwrap_err();
    assert!(
        matches!(&e, ConfigError::Invalid { key, .. } if key == "ignoreErrors"),
        "{e}"
    );
}

#[test]
fn level_as_string_and_empty_file() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = root(&tmp);
    let f = write(&dir, "s.neon", "parameters:\n  level: '5'\n");
    assert_eq!(
        PhpstanConfig::load_with(&f, &opts()).unwrap().level,
        Some(Level::Level(5))
    );
    let f = write(&dir, "empty.neon", "# nothing here\n");
    let cfg = PhpstanConfig::load_with(&f, &opts()).unwrap();
    assert_eq!(cfg.level, None);
    assert!(cfg.paths.is_empty());
}

#[test]
fn parameter_references() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = root(&tmp);
    let f = write(
        &dir,
        "p.neon",
        "parameters:\n  base: /srv\n  derived: %base%/app\n  list: [a, b]\n  copy: %list%\n  unknown: %nope%\n",
    );
    let cfg = PhpstanConfig::load_with(&f, &opts()).unwrap();
    let p = |k: &str| cfg.parameters.get(&Key::from(k)).unwrap().to_string();
    assert_eq!(p("derived"), r#""/srv/app""#);
    assert_eq!(p("copy"), r#"["a", "b"]"#);
    assert_eq!(p("unknown"), r#""%nope%""#);
}
