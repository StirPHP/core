use std::fs;
use std::path::{Path, PathBuf};

use stir_project::{
    Autoload, CONFIG_FILE, Config, Error, FileKind, InitOptions, Project, SAMPLE_MAIN, discover,
    init, load_autoload, parse_installed,
};
use tempfile::TempDir;

fn write(path: impl AsRef<Path>, text: &str) {
    let path = path.as_ref();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn canon(dir: &TempDir) -> PathBuf {
    fs::canonicalize(dir.path()).unwrap()
}

// ---------- config ----------

#[test]
fn config_defaults() {
    let c = Config::parse("", Path::new("sphp.toml")).unwrap();
    assert_eq!(c, Config::default());
    assert_eq!(c.src_dir, PathBuf::from("src"));
    assert_eq!(c.out_dir, PathBuf::from("build"));
    assert_eq!(c.include, ["**/*.sphp", "**/*.php"]);
    assert_eq!(c.exclude, ["vendor/**"]);
    assert!(c.allow_php && !c.check_php && c.strict && !c.inline_extensions);
    assert_eq!(c.phpstan, None);
    assert_eq!(c.composer, None);
}

#[test]
fn config_parses_camel_case_keys() {
    let text = r#"
        srcDir = "lib"
        outDir = "dist"
        include = ["**/*.sphp"]
        exclude = []
        allowPhp = false
        checkPhp = true
        strict = false
        inlineExtensions = true
        phpstan = "phpstan.neon"
        composer = "app/composer.json"
    "#;
    let c = Config::parse(text, Path::new("sphp.toml")).unwrap();
    assert_eq!(c.src_dir, PathBuf::from("lib"));
    assert_eq!(c.out_dir, PathBuf::from("dist"));
    assert_eq!(c.include, ["**/*.sphp"]);
    assert!(c.exclude.is_empty());
    assert!(!c.allow_php && c.check_php && !c.strict && c.inline_extensions);
    assert_eq!(c.phpstan, Some("phpstan.neon".into()));
    assert_eq!(c.composer, Some("app/composer.json".into()));
}

#[test]
fn config_round_trips() {
    let c = Config {
        name: Some("demo".into()),
        ..Config::default()
    };
    let text = c.to_toml().unwrap();
    assert!(text.contains("srcDir = \"src\""), "{text}");
    assert_eq!(Config::parse(&text, Path::new("x")).unwrap(), c);
}

#[test]
fn config_parse_error_mentions_path() {
    let err = Config::parse("srcDir = 3", Path::new("/p/sphp.toml")).unwrap_err();
    assert!(matches!(err, Error::TomlParse { .. }));
    assert!(err.to_string().contains("/p/sphp.toml"), "{err}");
    let err = Config::parse("srcDri = \"x\"", Path::new("/p/sphp.toml")).unwrap_err();
    assert!(matches!(err, Error::TomlParse { .. }));
}

#[tokio::test]
async fn load_searches_upward() {
    let tmp = TempDir::new().unwrap();
    let root = canon(&tmp);
    write(root.join(CONFIG_FILE), "srcDir = \"lib\"\n");
    let nested = root.join("a/b/c");
    fs::create_dir_all(&nested).unwrap();

    let project = Project::load(&nested).await.unwrap();
    assert_eq!(project.root, root);
    assert_eq!(project.config.src_dir, PathBuf::from("lib"));
    assert_eq!(project.src_dir(), root.join("lib"));
    assert_eq!(project.out_dir(), root.join("build"));
}

#[tokio::test]
async fn load_reports_missing_config() {
    let tmp = TempDir::new().unwrap();
    let err = Project::load(tmp.path()).await.unwrap_err();
    assert!(matches!(err, Error::ConfigNotFound { .. }), "{err}");
}

#[tokio::test]
async fn load_reports_toml_errors_with_path() {
    let tmp = TempDir::new().unwrap();
    write(tmp.path().join(CONFIG_FILE), "srcDir = [");
    let err = Project::load(tmp.path()).await.unwrap_err();
    assert!(matches!(err, Error::TomlParse { .. }));
    assert!(err.to_string().contains(CONFIG_FILE));
}

// ---------- discovery ----------

fn project(root: PathBuf, config: Config) -> Project {
    Project { root, config }
}

fn rel(project: &Project, files: &[stir_project::SourceFile]) -> Vec<(String, FileKind)> {
    files
        .iter()
        .map(|f| {
            let p = f.path.strip_prefix(&project.root).unwrap();
            (p.to_string_lossy().replace('\\', "/"), f.kind)
        })
        .collect()
}

#[tokio::test]
async fn discover_respects_include_exclude_and_gitignore() {
    let tmp = TempDir::new().unwrap();
    let root = canon(&tmp);
    write(root.join("src/main.sphp"), "");
    write(root.join("src/b/z.sphp"), "");
    write(root.join("src/a/legacy.php"), "");
    write(root.join("src/a/readme.txt"), "");
    write(root.join("src/generated/gen.sphp"), "");
    write(root.join("src/skip/x.sphp"), "");
    write(root.join("src/.gitignore"), "generated/\n");
    write(root.join("vendor/pkg/lib.php"), "");

    let mut config = Config::default();
    config.exclude.push("skip/**".into());
    let p = project(root.clone(), config);
    let files = discover(&p).await.unwrap();
    assert_eq!(
        rel(&p, &files),
        [
            ("src/a/legacy.php".to_owned(), FileKind::Php),
            ("src/b/z.sphp".to_owned(), FileKind::Sphp),
            ("src/main.sphp".to_owned(), FileKind::Sphp),
        ]
    );
}

#[tokio::test]
async fn discover_without_allow_php_and_src_at_root() {
    let tmp = TempDir::new().unwrap();
    let root = canon(&tmp);
    write(root.join("main.sphp"), "");
    write(root.join("lib/x.php"), "");
    write(root.join("vendor/pkg/y.sphp"), "");
    write(root.join("build/main.sphp"), "");

    let config = Config {
        src_dir: ".".into(),
        ..Config::default()
    };
    let p = project(root.clone(), config.clone());
    let files = discover(&p).await.unwrap();
    assert_eq!(
        rel(&p, &files),
        [
            ("lib/x.php".to_owned(), FileKind::Php),
            ("main.sphp".to_owned(), FileKind::Sphp)
        ]
    );

    let p = project(
        root,
        Config {
            allow_php: false,
            ..config
        },
    );
    let files = discover(&p).await.unwrap();
    assert_eq!(rel(&p, &files), [("main.sphp".to_owned(), FileKind::Sphp)]);
}

#[tokio::test]
async fn discover_include_globs_are_relative_to_src_dir() {
    let tmp = TempDir::new().unwrap();
    let root = canon(&tmp);
    write(root.join("src/Domain/a.sphp"), "");
    write(root.join("src/Other/b.sphp"), "");
    let config = Config {
        include: vec!["Domain/**/*.sphp".into()],
        ..Config::default()
    };
    let p = project(root, config);
    let files = discover(&p).await.unwrap();
    assert_eq!(
        rel(&p, &files),
        [("src/Domain/a.sphp".to_owned(), FileKind::Sphp)]
    );
}

#[tokio::test]
async fn discover_rejects_bad_glob() {
    let tmp = TempDir::new().unwrap();
    let config = Config {
        include: vec!["a[".into()],
        ..Config::default()
    };
    let err = discover(&project(canon(&tmp), config)).await.unwrap_err();
    assert!(matches!(err, Error::Glob { .. }));
}

// ---------- composer ----------

const ROOT_COMPOSER: &str = r#"{
    "name": "acme/app",
    "autoload": {
        "psr-4": { "App\\": "src/", "App\\Lib\\": ["lib/", "lib2/"], "": "fallback/" },
        "psr-0": { "Legacy_": "old/" },
        "classmap": ["classes/"],
        "files": ["helpers.php"]
    },
    "autoload-dev": { "psr-4": { "App\\Tests\\": "tests/" } }
}"#;

#[tokio::test]
async fn composer_root_autoload() {
    let tmp = TempDir::new().unwrap();
    let root = canon(&tmp);
    write(root.join("composer.json"), ROOT_COMPOSER);
    let a = Autoload::load(&root.join("composer.json")).await.unwrap();

    assert!(a.psr4.contains(&("App\\".into(), root.join("src"))));
    assert!(a.psr4.contains(&("App\\Lib\\".into(), root.join("lib"))));
    assert!(a.psr4.contains(&("App\\Lib\\".into(), root.join("lib2"))));
    assert!(
        a.psr4
            .contains(&("App\\Tests\\".into(), root.join("tests")))
    );
    assert_eq!(a.psr0, [("Legacy_".to_owned(), root.join("old"))]);
    assert_eq!(a.classmap_paths, [root.join("classes")]);
    assert_eq!(a.files, [root.join("helpers.php")]);

    assert_eq!(
        a.resolve_class("\\App\\Lib\\Util\\Str"),
        [
            root.join("lib/Util/Str.php"),
            root.join("lib2/Util/Str.php"),
            root.join("src/Lib/Util/Str.php"),
            root.join("fallback/App/Lib/Util/Str.php"),
        ]
    );
    assert_eq!(
        a.resolve_class("Legacy_Foo_Bar"),
        [
            root.join("fallback/Legacy_Foo_Bar.php"),
            root.join("old/Legacy/Foo/Bar.php")
        ]
    );
}

#[tokio::test]
async fn composer_installed_json_v2_format() {
    let tmp = TempDir::new().unwrap();
    let root = canon(&tmp);
    write(
        root.join("composer.json"),
        r#"{"autoload": {"psr-4": {"App\\": "src"}}}"#,
    );
    write(
        root.join("vendor/composer/installed.json"),
        r#"{
            "packages": [
                {
                    "name": "psr/log",
                    "install-path": "../psr/log",
                    "autoload": { "psr-4": { "Psr\\Log\\": "src" } }
                },
                {
                    "name": "acme/helpers",
                    "autoload": { "files": ["fn.php"] }
                },
                { "name": "empty/autoload", "autoload": [] }
            ],
            "dev": true
        }"#,
    );
    let a = Autoload::load(&root.join("composer.json")).await.unwrap();
    assert!(
        a.psr4
            .contains(&("Psr\\Log\\".into(), root.join("vendor/psr/log/src")))
    );
    assert_eq!(a.files, [root.join("vendor/acme/helpers/fn.php")]);
    assert_eq!(
        a.resolve_class("Psr\\Log\\LoggerInterface"),
        [root.join("vendor/psr/log/src/LoggerInterface.php")]
    );
}

#[tokio::test]
async fn composer_installed_json_legacy_array_and_vendor_dir() {
    let tmp = TempDir::new().unwrap();
    let root = canon(&tmp);
    write(
        root.join("composer.json"),
        r#"{"config": {"vendor-dir": "deps"}}"#,
    );
    write(
        root.join("deps/composer/installed.json"),
        r#"[
            { "name": "monolog/monolog", "autoload": { "psr-4": { "Monolog\\": ["src/Monolog"] } } }
        ]"#,
    );
    let config = Config::default();
    let a = load_autoload(&project(root.clone(), config)).await.unwrap();
    assert_eq!(
        a.psr4,
        [(
            "Monolog\\".to_owned(),
            root.join("deps/monolog/monolog/src/Monolog")
        )]
    );
}

#[test]
fn parse_installed_both_formats() {
    let vendor = Path::new("/p/vendor");
    let v2: serde_json::Value =
        serde_json::from_str(r#"{"packages":[{"name":"a/b","install-path":"../x/y"}]}"#).unwrap();
    let legacy: serde_json::Value = serde_json::from_str(r#"[{"name":"a/b"}]"#).unwrap();
    let v2 = parse_installed(&v2, vendor);
    let legacy = parse_installed(&legacy, vendor);
    assert_eq!(v2[0].0.install_path, Path::new("/p/vendor/x/y"));
    assert_eq!(legacy[0].0.install_path, Path::new("/p/vendor/a/b"));
    assert_eq!(legacy[0].0.name, "a/b");
}

#[tokio::test]
async fn load_autoload_without_composer_is_empty() {
    let tmp = TempDir::new().unwrap();
    let a = load_autoload(&project(canon(&tmp), Config::default()))
        .await
        .unwrap();
    assert_eq!(a, Autoload::default());
}

// ---------- init ----------

#[tokio::test]
async fn init_creates_files() {
    let tmp = TempDir::new().unwrap();
    let root = canon(&tmp);
    let report = init(
        &root,
        InitOptions {
            name: Some("demo".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(report.config_path, root.join(CONFIG_FILE));
    assert_eq!(
        report.created,
        [
            root.join(CONFIG_FILE),
            root.join("src/main.sphp"),
            root.join(".gitignore")
        ]
    );
    assert!(report.gitignore_updated);
    assert_eq!(
        fs::read_to_string(root.join("src/main.sphp")).unwrap(),
        SAMPLE_MAIN
    );
    assert_eq!(
        fs::read_to_string(root.join(".gitignore")).unwrap(),
        "/build\n"
    );

    let project = Project::load(&root).await.unwrap();
    assert_eq!(project.config.name.as_deref(), Some("demo"));
    assert_eq!(project.config, report.config);
    assert_eq!(project.config.composer, None);
    assert_eq!(project.config.phpstan, None);
}

#[tokio::test]
async fn init_detects_composer_and_phpstan_and_custom_dirs() {
    let tmp = TempDir::new().unwrap();
    let root = canon(&tmp);
    write(root.join("composer.json"), "{}");
    write(root.join("phpstan.neon.dist"), "");
    write(root.join(".gitignore"), "/vendor");
    let options = InitOptions {
        src_dir: "app".into(),
        out_dir: "dist".into(),
        allow_php: false,
        ..Default::default()
    };
    let report = init(&root, options).await.unwrap();
    assert_eq!(report.composer, Some("composer.json".into()));
    assert_eq!(report.phpstan, Some("phpstan.neon.dist".into()));
    assert!(!report.created.contains(&root.join(".gitignore")));
    assert_eq!(
        fs::read_to_string(root.join(".gitignore")).unwrap(),
        "/vendor\n/dist\n"
    );
    assert!(root.join("app/main.sphp").is_file());

    let config = Project::load(&root).await.unwrap().config;
    assert_eq!(config.composer, Some("composer.json".into()));
    assert_eq!(config.phpstan, Some("phpstan.neon.dist".into()));
    assert!(!config.allow_php);
    let expected_name = root.file_name().unwrap().to_string_lossy().into_owned();
    assert_eq!(config.name, Some(expected_name));
}

#[tokio::test]
async fn init_does_not_duplicate_gitignore_entry() {
    let tmp = TempDir::new().unwrap();
    let root = canon(&tmp);
    write(root.join(".gitignore"), "build/\n");
    let report = init(&root, InitOptions::default()).await.unwrap();
    assert!(!report.gitignore_updated);
    assert_eq!(
        fs::read_to_string(root.join(".gitignore")).unwrap(),
        "build/\n"
    );

    let report = init(
        &root,
        InitOptions {
            force: true,
            out_dir: "out".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(report.gitignore_updated);
    let report = init(
        &root,
        InitOptions {
            force: true,
            out_dir: "out".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(!report.gitignore_updated);
    assert_eq!(
        fs::read_to_string(root.join(".gitignore")).unwrap(),
        "build/\n/out\n"
    );
}

#[tokio::test]
async fn init_refuses_overwrite_without_force() {
    let tmp = TempDir::new().unwrap();
    let root = canon(&tmp);
    write(root.join(CONFIG_FILE), "strict = false\n");
    let err = init(&root, InitOptions::default()).await.unwrap_err();
    assert!(matches!(err, Error::AlreadyInitialized { .. }));
    assert_eq!(
        fs::read_to_string(root.join(CONFIG_FILE)).unwrap(),
        "strict = false\n"
    );

    write(root.join("src/main.sphp"), "custom");
    let report = init(
        &root,
        InitOptions {
            force: true,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(report.skipped, [root.join("src/main.sphp")]);
    assert_eq!(
        fs::read_to_string(root.join("src/main.sphp")).unwrap(),
        "custom"
    );
    assert!(Project::load(&root).await.unwrap().config.strict);
}
