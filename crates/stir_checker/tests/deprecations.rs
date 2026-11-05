//! What's reported with `parameters.stirphp.deprecations` on. `// warning: code` marks what's expected.

use stir_db::{FileOrigin, RootDatabase, Settings, SourceFile, Workspace};

fn check(text: &str, deprecations: bool) -> Vec<(usize, String)> {
    let db = RootDatabase::default();
    let source = SourceFile::new(
        &db,
        "/src/a.php".into(),
        text.to_owned(),
        FileOrigin::Source,
    );
    let mut files = vec![source];
    files.extend(stir_stubs::stub_files().map(|s| {
        SourceFile::new(
            &db,
            format!("stubs://{}", s.path).into(),
            s.text.to_owned(),
            FileOrigin::Stub,
        )
    }));
    let settings = Settings {
        deprecations,
        php_level: 0,
        ..Settings::default()
    };
    let ws = Workspace::new(&db, files, settings);
    let mut out: Vec<(usize, String)> = stir_checker::check_file(&db, ws, source)
        .iter()
        .map(|d| {
            let line = text[..usize::from(d.range.start())].matches('\n').count() + 1;
            (line, d.code.to_string())
        })
        .collect();
    out.sort();
    out
}

fn expected(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if let Some(pos) = line.find("// warning: ") {
            for code in line[pos + 12..].split(',') {
                out.push((i + 1, code.trim().to_owned()));
            }
        }
    }
    out.sort();
    out
}

const USES: &str = r#"<?php
/** @deprecated use new_one */
function old_function(): void {}

/** @deprecated */
const OLD_CONSTANT = 1;

/** @deprecated */
class OldClass
{
    public const X = 1;
    public static int $count = 0;
    public int $n = 0;
    public function run(): void {}
    public static function make(): void {}
}

/** @deprecated */
interface OldInterface {}

/** @deprecated */
trait OldTrait {}

class Fine
{
    /** @deprecated */
    public const OLD = 1;
    /** @deprecated */
    public int $oldProperty = 0;
    /** @deprecated */
    public static int $oldStatic = 0;
    public int $fine = 0;
    /** @deprecated */
    public function oldMethod(): void {}
    #[\Deprecated]
    public static function oldStaticMethod(): void {}
    public function fine(): void {}
}

enum Status
{
    /** @deprecated */
    case Old;
    case Fine;
}

old_function(); // warning: function.deprecated
echo OLD_CONSTANT; // warning: constant.deprecated
$c = new OldClass(); // warning: new.deprecatedClass
$c->run(); // warning: method.deprecatedClass
$c->n = 1; // warning: property.deprecatedClass
OldClass::make(); // warning: staticMethod.deprecatedClass
echo OldClass::X; // warning: classConstant.deprecatedClass
echo OldClass::$count; // warning: staticProperty.deprecatedClass
echo OldClass::class;

class Extending extends OldClass {} // warning: class.extendsDeprecatedClass
class Implementing implements OldInterface {} // warning: class.implementsDeprecatedInterface
interface ExtendingInterface extends OldInterface {} // warning: interface.extendsDeprecatedInterface
enum ImplementingEnum implements OldInterface {} // warning: enum.implementsDeprecatedInterface
class UsingTrait
{
    use OldTrait; // warning: traitUse.deprecatedTrait
}

function typed(OldClass $o): OldClass // warning: parameter.deprecatedClass, return.deprecatedClass
{
    return $o;
}

$f = new Fine();
$f->oldMethod(); // warning: method.deprecated
$f->oldProperty = 1; // warning: property.deprecated
echo Fine::$oldStatic; // warning: staticProperty.deprecated
echo Fine::OLD; // warning: classConstant.deprecated
Fine::oldStaticMethod(); // warning: staticMethod.deprecated
$f->fine();
echo $f->fine;
echo Status::Old; // warning: classConstant.deprecated
echo Status::Fine;

// what's deprecated itself may use what is
/** @deprecated */
function also_old(): void
{
    old_function();
    new OldClass();
}

/** @deprecated */
class AlsoOld
{
    public function run(): void
    {
        old_function();
    }
}

class Partly
{
    /** @deprecated */
    public function old(): void
    {
        old_function();
    }
}
"#;

#[test]
fn every_kind_of_use_with_phpstans_identifier() {
    assert_eq!(check(USES, true), expected(USES));
}

#[test]
fn nothing_without_the_setting() {
    assert_eq!(check(USES, false), []);
}

#[test]
fn the_stubs_say_what_php_deprecates_from_when() {
    // `utf8_encode` has `#[Deprecated(since: '8.2')]`; the default target's newer
    let text = "<?php\necho utf8_encode('a'); // warning: function.deprecated\necho strlen('a');\n";
    assert_eq!(check(text, true), expected(text));
}
