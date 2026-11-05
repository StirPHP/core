use std::fs;
use std::path::{Path, PathBuf};

use stir_project::{
    Autoload, CONFIG_FILE, Config, Error, PHPSTAN_CONFIG_FILES, Project, discover, init,
    load_autoload, parse_installed,
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
    let c = Config::default();
    assert_eq!(c.src_dir, PathBuf::from("."));
    assert_eq!(c.include, ["**/*.php"]);
    assert_eq!(c.exclude, ["vendor/**"]);
    assert_eq!(c.composer, None);
    assert_eq!(c.baseline, None);
    assert!(c.rules.is_empty());
}

#[tokio::test]
async fn config_reads_the_section() {
    let tmp = TempDir::new().unwrap();
    let root = canon(&tmp);
    write(
        root.join(CONFIG_FILE),
        r#"
parameters:
    stirphp:
        srcDir: lib
        include: ["app/**/*.php"]
        exclude: []
        composer: app/composer.json
        baseline: stirphp-baseline.neon
        rules: [rules/no-var-dump.rhai]
"#,
    );
    let p = Project::load_or_default(&root).await.unwrap();
    assert_eq!(p.config_file, Some(root.join(CONFIG_FILE)));
    let c = p.config;
    assert_eq!(c.src_dir, root.join("lib"));
    assert_eq!(c.include, ["app/**/*.php"]);
    assert!(c.exclude.is_empty());
    assert_eq!(c.composer, Some(root.join("app/composer.json")));
    assert_eq!(c.baseline, Some(root.join("stirphp-baseline.neon")));
    assert_eq!(c.rules, [root.join("rules/no-var-dump.rhai")]);
}

#[tokio::test]
async fn config_errors_mention_the_file() {
    let tmp = TempDir::new().unwrap();
    let root = canon(&tmp);
    write(root.join(CONFIG_FILE), "parameters:\n    stirphp: [\n");
    let err = Project::load_or_default(&root).await.unwrap_err();
    assert!(matches!(err, Error::Config(_)), "{err}");
    assert!(err.to_string().contains(CONFIG_FILE), "{err}");

    write(
        root.join(CONFIG_FILE),
        "parameters:\n    stirphp:\n        srcDri: x\n",
    );
    let err = Project::load_or_default(&root).await.unwrap_err();
    assert!(err.to_string().contains("srcDri"), "{err}");
}

#[tokio::test]
async fn load_searches_upward() {
    let tmp = TempDir::new().unwrap();
    let root = canon(&tmp);
    write(
        root.join(CONFIG_FILE),
        "parameters:\n    stirphp:\n        srcDir: lib\n",
    );
    let nested = root.join("a/b/c");
    fs::create_dir_all(&nested).unwrap();

    let project = Project::load_or_default(&nested).await.unwrap();
    assert_eq!(project.root, root);
    assert_eq!(project.src_dir(), root.join("lib"));
}

#[tokio::test]
async fn phpstan_config_is_found_and_stirphp_neon_wins() {
    for name in PHPSTAN_CONFIG_FILES {
        let tmp = TempDir::new().unwrap();
        let root = canon(&tmp);
        write(
            root.join(name),
            "parametersSchema:\n    ?stirphp: mixed()\nparameters:\n    stirphp:\n        srcDir: lib\n",
        );
        let project = Project::load_or_default(&root).await.unwrap();
        assert_eq!(project.config_file, Some(root.join(name)));
        assert_eq!(project.src_dir(), root.join("lib"), "{name}");

        write(
            root.join(CONFIG_FILE),
            "parameters:\n    stirphp:\n        srcDir: own\n",
        );
        let project = Project::load_or_default(&root).await.unwrap();
        assert_eq!(project.config_file, Some(root.join(CONFIG_FILE)));
        assert_eq!(project.src_dir(), root.join("own"), "{name}");
    }
}

#[tokio::test]
async fn a_config_elsewhere_keeps_the_project_root() {
    let tmp = TempDir::new().unwrap();
    let root = canon(&tmp);
    write(root.join("app/composer.json"), "{}");
    let file = root.join("conf/my.neon");
    write(
        &file,
        "parameters:\n    stirphp:\n        baseline: b.neon\n",
    );
    let project = Project::load_with_config(root.join("app"), &file)
        .await
        .unwrap();
    assert_eq!(project.root, root.join("app"));
    assert_eq!(project.config_file, Some(file));
    assert_eq!(project.config.baseline, Some(root.join("conf/b.neon")));
}

#[tokio::test]
async fn load_or_default_finds_the_composer_root() {
    let tmp = TempDir::new().unwrap();
    let root = canon(&tmp);
    write(root.join("composer.json"), "{}");
    let nested = root.join("a/b");
    fs::create_dir_all(&nested).unwrap();

    let project = Project::load_or_default(&nested).await.unwrap();
    assert_eq!(project.root, root);
    assert_eq!(project.config, Config::default());
    assert_eq!(project.src_dir(), root.join("."));
}

#[tokio::test]
async fn load_or_default_without_anything_uses_the_start_dir() {
    let tmp = TempDir::new().unwrap();
    let root = canon(&tmp);
    let project = Project::load_or_default(&root).await.unwrap();
    assert_eq!(project.root, root);
}

// ---------- init ----------

#[test]
fn init_writes_a_minimal_config() {
    let tmp = TempDir::new().unwrap();
    let root = canon(&tmp);
    let outcome = init::init(&root, false).unwrap();
    assert_eq!(outcome, init::Outcome::Created(root.join(CONFIG_FILE)));
    let text = fs::read_to_string(root.join(CONFIG_FILE)).unwrap();
    assert!(text.contains("stirphp:"), "{text}");
    assert!(!text.contains("parametersSchema"), "{text}");
    assert_eq!(
        init::init(&root, false).unwrap(),
        init::Outcome::AlreadySetUp(root.join(CONFIG_FILE))
    );
}

#[test]
fn init_adds_to_a_phpstan_config_and_keeps_the_rest() {
    let tmp = TempDir::new().unwrap();
    let root = canon(&tmp);
    let file = root.join("phpstan.neon.dist");
    let text = "# top comment\nincludes:\n\t- vendor/x.neon # why\n\nparameters:\n\t# the level\n\tlevel: 5 # enough\n\tpaths:\n\t\t- src\n\n# about services\nservices: []\n";
    write(&file, text);
    assert_eq!(
        init::init(&root, false).unwrap(),
        init::Outcome::Updated(file.clone())
    );
    let new = fs::read_to_string(&file).unwrap();
    assert_eq!(
        new,
        "# top comment\nincludes:\n\t- vendor/x.neon # why\n\nparameters:\n\t# the level\n\tlevel: 5 # enough\n\tpaths:\n\t\t- src\n\tstirphp:\n\t\trules: []\n\n# about services\nservices: []\nparametersSchema:\n\t?stirphp: mixed()\n"
    );
    assert_eq!(
        init::init(&root, false).unwrap(),
        init::Outcome::AlreadySetUp(file.clone())
    );
    assert_eq!(fs::read_to_string(&file).unwrap(), new);
}

#[test]
fn init_follows_the_files_indent_and_its_schema() {
    let tmp = TempDir::new().unwrap();
    let root = canon(&tmp);
    let file = root.join("phpstan.neon");
    write(
        &file,
        "parametersSchema:\n  foo: string()\nparameters:\n  foo: x\n",
    );
    init::init(&root, true).unwrap();
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "parametersSchema:\n  foo: string()\n  ?stirphp: mixed()\nparameters:\n  foo: x\n  stirphp:\n    rules: []\n"
    );
}

#[test]
fn init_with_phpstan_creates_that_config() {
    let tmp = TempDir::new().unwrap();
    let root = canon(&tmp);
    assert_eq!(
        init::init(&root, true).unwrap(),
        init::Outcome::Created(root.join("phpstan.neon"))
    );
    assert_eq!(
        fs::read_to_string(root.join("phpstan.neon")).unwrap(),
        "parameters:\n\tstirphp:\n\t\trules: []\nparametersSchema:\n\t?stirphp: mixed()\n"
    );
}

#[test]
fn init_leaves_what_it_cant_follow_alone() {
    let tmp = TempDir::new().unwrap();
    let root = canon(&tmp);
    let file = root.join("phpstan.neon");
    let text = "parameters: {level: 5}\n";
    write(&file, text);
    let err = init::init(&root, false).unwrap_err();
    assert!(matches!(err, Error::CantEditConfig { .. }), "{err}");
    assert_eq!(fs::read_to_string(&file).unwrap(), text);
}

// ---------- discovery ----------

fn project(root: PathBuf, config: Config) -> Project {
    Project {
        root,
        config,
        config_file: None,
    }
}

fn rel(project: &Project, files: &[stir_project::SourceFile]) -> Vec<String> {
    files
        .iter()
        .map(|f| {
            let p = f.path.strip_prefix(&project.root).unwrap();
            p.to_string_lossy().replace('\\', "/")
        })
        .collect()
}

#[tokio::test]
async fn discover_respects_include_exclude_and_gitignore() {
    let tmp = TempDir::new().unwrap();
    let root = canon(&tmp);
    write(root.join("src/main.php"), "");
    write(root.join("src/b/z.php"), "");
    write(root.join("src/a/readme.txt"), "");
    write(root.join("src/generated/gen.php"), "");
    write(root.join("src/skip/x.php"), "");
    write(root.join("src/.gitignore"), "generated/\n");
    write(root.join("vendor/pkg/lib.php"), "");

    let mut config = Config {
        src_dir: "src".into(),
        ..Config::default()
    };
    config.exclude.push("skip/**".into());
    let p = project(root.clone(), config);
    let files = discover(&p).await.unwrap();
    assert_eq!(rel(&p, &files), ["src/b/z.php", "src/main.php"]);
}

#[tokio::test]
async fn discover_at_the_root_skips_vendor_but_not_build() {
    let tmp = TempDir::new().unwrap();
    let root = canon(&tmp);
    write(root.join("main.php"), "");
    write(root.join("lib/x.php"), "");
    write(root.join("vendor/pkg/y.php"), "");
    write(root.join("build/z.php"), "");

    let p = project(root.clone(), Config::default());
    let files = discover(&p).await.unwrap();
    assert_eq!(rel(&p, &files), ["build/z.php", "lib/x.php", "main.php"]);
}

#[tokio::test]
async fn discover_include_globs_are_relative_to_src_dir() {
    let tmp = TempDir::new().unwrap();
    let root = canon(&tmp);
    write(root.join("src/Domain/a.php"), "");
    write(root.join("src/Other/b.php"), "");
    let config = Config {
        src_dir: "src".into(),
        include: vec!["Domain/**/*.php".into()],
        ..Config::default()
    };
    let p = project(root, config);
    let files = discover(&p).await.unwrap();
    assert_eq!(rel(&p, &files), ["src/Domain/a.php"]);
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
    let a = load_autoload(&project(root.clone(), Config::default()))
        .await
        .unwrap();

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
    let a = load_autoload(&project(root.clone(), Config::default()))
        .await
        .unwrap();
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
