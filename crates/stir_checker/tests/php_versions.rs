//! Each piece of syntax newer than the target is reported, on the release before the one that
//! added it and not on that one.

use stir_checker::php_version::{self as v, Feature};
use stir_db::{FileOrigin, PhpVersion, PhpVersions, RootDatabase, Settings, SourceFile, Workspace};

/// The diagnostics about versions, of `src` for a project on `version`.
fn check(src: &str, version: PhpVersion) -> Vec<String> {
    let db = RootDatabase::default();
    let file = SourceFile::new(&db, "/a.php".into(), src.into(), FileOrigin::Source);
    let settings = Settings {
        php_version: PhpVersions::exactly(version),
        ..Settings::default()
    };
    let ws = Workspace::new(&db, vec![file], settings);
    assert!(
        stir_db::parse(&db, file).errors().is_empty(),
        "syntax errors in {src}: {:?}",
        stir_db::parse(&db, file).errors()
    );
    v::syntax_version_diagnostics(&db, ws, file)
        .iter()
        .map(|d| d.message.clone())
        .collect()
}

/// The release before `version`.
fn before(version: PhpVersion) -> PhpVersion {
    match (version.major, version.minor) {
        (8, 0) => PhpVersion::new(7, 4),
        (major, minor) => PhpVersion::new(major, minor - 1),
    }
}

const CASES: &[(Feature, &str)] = &[
    (v::TYPED_PROPERTIES, "class A { public int $x; }"),
    (v::ARROW_FUNCTIONS, "$f = fn($x) => $x;"),
    (v::COALESCE_ASSIGN, "$a ??= 1;"),
    (v::ARRAY_SPREAD, "$a = [...$b];"),
    (v::NUMERIC_SEPARATOR, "$a = 1_000;"),
    (v::MATCH, "$a = match ($b) { default => 1 };"),
    (v::NULLSAFE, "$a = $b?->c;"),
    (v::NULLSAFE, "$a = $b?->c();"),
    (v::NAMED_ARGUMENTS, "f(a: 1);"),
    (
        v::PROMOTION,
        "class A { public function __construct(private $x) {} }",
    ),
    (v::UNION_TYPES, "function f(int|string $x) {}"),
    (v::UNION_TYPES, "function f(): int|null {}"),
    (v::ATTRIBUTES, "function f(#[A] $x) {}"),
    (v::STATIC_TYPE, "class A { public function f(): static {} }"),
    (v::MIXED_TYPE, "function f(mixed $x) {}"),
    (v::THROW_EXPRESSION, "$a = $b ?? throw new E();"),
    (v::CATCH_WITHOUT_VARIABLE, "try { } catch (E) { }"),
    (v::TRAILING_COMMA_PARAMS, "function f($a, $b,) {}"),
    (v::TRAILING_COMMA_PARAMS, "$f = function () use ($a,) {};"),
    (v::CLASS_ON_OBJECT, "$a = $b::class;"),
    (v::ENUMS, "enum E { case A; }"),
    (
        v::READONLY_PROPERTIES,
        "class A { public readonly int $x; }",
    ),
    (
        v::READONLY_PROPERTIES,
        "class A { public function __construct(public readonly int $x) {} }",
    ),
    (v::FIRST_CLASS_CALLABLES, "$f = strlen(...);"),
    (v::NEW_IN_INITIALIZERS, "function f($a = new B()) {}"),
    (
        v::NEW_IN_INITIALIZERS,
        "function f() { static $a = new B(); }",
    ),
    (v::NEW_IN_INITIALIZERS, "const A = new B();"),
    (v::NEW_IN_INITIALIZERS, "#[A(new B())] function f() {}"),
    (v::NEVER_TYPE, "function f(): never {}"),
    (v::INTERSECTION_TYPES, "function f(A&B $x) {}"),
    (v::FINAL_CLASS_CONSTANTS, "class A { final const X = 1; }"),
    (v::EXPLICIT_OCTAL, "$a = 0o17;"),
    (v::READONLY_CLASSES, "readonly class A {}"),
    (v::READONLY_CLASSES, "final readonly class A {}"),
    (v::DNF_TYPES, "function f((A&B)|null $x) {}"),
    (v::TRUE_TYPE, "function f(): true {}"),
    (v::STANDALONE_NULL_FALSE, "function f(): null {}"),
    (v::STANDALONE_NULL_FALSE, "function f(): false {}"),
    (v::TRAIT_CONSTANTS, "trait T { const X = 1; }"),
    (v::TYPED_CONSTANTS, "class A { const int X = 1; }"),
    (v::DYNAMIC_CONSTANT_FETCH, "$a = B::{$c};"),
    (v::READONLY_ANONYMOUS_CLASSES, "$a = new readonly class {};"),
    (
        v::PROPERTY_HOOKS_FEATURE,
        "class A { public int $x { get => 1; } }",
    ),
    (
        v::PROPERTY_HOOKS_FEATURE,
        "class A { public function __construct(public int $x { get => 1; }) {} }",
    ),
    (
        v::ASYMMETRIC_VISIBILITY,
        "class A { public private(set) int $x; }",
    ),
    (
        v::ASYMMETRIC_VISIBILITY,
        "class A { public function __construct(public private(set) int $x) {} }",
    ),
    (v::NEW_WITHOUT_PARENS, "$a = new B()->c;"),
    (v::NEW_WITHOUT_PARENS, "$a = new B()->c();"),
    (v::NEW_WITHOUT_PARENS, "$a = new B()::C;"),
    (v::NEW_WITHOUT_PARENS, "$a = new B()[0];"),
    (v::PROPERTY_MAGIC_CONSTANT, "$a = __PROPERTY__;"),
    (v::PIPE_OPERATOR, "$a = $b |> strlen(...);"),
    (v::CLONE_WITH, "$a = clone($b, ['x' => 1]);"),
    (v::VOID_CAST_FEATURE, "(void) f();"),
    (
        v::FINAL_PROMOTED,
        "class A { public function __construct(final public int $x) {} }",
    ),
    (v::ATTRIBUTES_ON_CONSTANTS, "#[A] const X = 1;"),
    (
        v::ASYMMETRIC_STATIC,
        "class A { public static private(set) int $x; }",
    ),
    (v::CLOSURES_IN_CONSTANTS, "const A = static function () {};"),
    (
        v::CLOSURES_IN_CONSTANTS,
        "function f($a = static function () {}) {}",
    ),
];

#[test]
fn each_feature_on_either_side_of_its_release() {
    for (feature, code) in CASES {
        let src = format!("<?php\n{code}\n");
        let wanted = format!("PHP {} added {},", feature.since, feature.name);
        let found = check(&src, before(feature.since));
        assert!(
            found.iter().any(|m| m.starts_with(&wanted)),
            "{code} on {}: {found:?}",
            before(feature.since)
        );
        let found = check(&src, feature.since);
        assert!(
            !found.iter().any(|m| m.starts_with(&wanted)),
            "{code} on {}: {found:?}",
            feature.since
        );
    }
}

#[test]
fn older_syntax_stays_quiet_on_its_own_versions() {
    let quiet = [
        "$a = clone($b);",
        "$a = (new B())->c;",
        "$a = (new B)->c();",
        "$a = new B();",
        "$a = 0777;",
        "$a = $b::C;",
        "$a = B::class;",
        "try { } catch (E $e) { }",
        "function f(int $a, ?B $b, $c = 1) {}",
        "f(1, 2,);",
        "class Mixed_ extends Mixed_2 {}",
        "class A { public $x; public function __construct($y) {} }",
        "class A { const X = 1; private const Y = 2; }",
        "throw new E();",
        "function f(self $a): ?array {}",
    ];
    for code in quiet {
        let src = format!("<?php\n{code}\n");
        let found = check(&src, PhpVersion::new(7, 3));
        assert!(found.is_empty(), "{code}: {found:?}");
    }
}

#[test]
fn a_comment_of_a_lint_tool_says_what_a_file_is_for() {
    let found = check("<?php // lint >= 8.1\nenum E {}\n", PhpVersion::new(8, 0));
    assert!(found.is_empty(), "{found:?}");
    let found = check("<?php // lint >= 8.1\nenum E {}\n", PhpVersion::new(8, 1));
    assert!(found.is_empty(), "{found:?}");
    let found = check("<?php\n// lint >= 8.1\nenum E {}\n", PhpVersion::new(8, 0));
    assert_eq!(found.len(), 1, "{found:?}");
}

#[test]
fn an_attribute_on_its_own_line_is_a_comment_before_8_0() {
    let v74 = PhpVersion::new(7, 4);
    let db = RootDatabase::default();
    let file = SourceFile::new(
        &db,
        "/a.php".into(),
        "<?php\n#[A]\nfunction f() {}\nfunction g(#[B] $x) {}\n#[\n  C\n]\nfunction h() {}\n"
            .to_owned(),
        FileOrigin::Source,
    );
    let settings = Settings {
        php_version: PhpVersions::exactly(v74),
        ..Settings::default()
    };
    let ws = Workspace::new(&db, vec![file], settings);
    let severities: Vec<_> = v::syntax_version_diagnostics(&db, ws, file)
        .iter()
        .map(|d| d.severity)
        .collect();
    use stir_db::Severity::{Error, Warning};
    assert_eq!(severities, [Warning, Error, Error]);
}

#[test]
fn override_is_a_warning_unless_a_polyfill_has_it() {
    let run = |extra: Option<&str>| {
        let db = RootDatabase::default();
        let file = SourceFile::new(
            &db,
            "/a.php".into(),
            "<?php\nclass A { #[\\Override] public function f() {} }\n".to_owned(),
            FileOrigin::Source,
        );
        let mut files = vec![file];
        if let Some(text) = extra {
            files.push(SourceFile::new(
                &db,
                "/vendor/polyfill.php".into(),
                text.to_owned(),
                FileOrigin::Vendor,
            ));
        }
        let settings = Settings {
            php_version: PhpVersions::exactly(PhpVersion::new(8, 2)),
            ..Settings::default()
        };
        let ws = Workspace::new(&db, files, settings);
        v::syntax_version_diagnostics(&db, ws, file)
            .iter()
            .map(|d| (d.severity, d.message.clone()))
            .collect::<Vec<_>>()
    };
    let found = run(None);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].0, stir_db::Severity::Warning);
    assert!(found[0].1.contains("#[\\Override]"), "{found:?}");
    assert!(run(Some("<?php\nfinal class Override {}\n")).is_empty());
}
