use stir_db::{FileOrigin, RootDatabase, SourceFile, Workspace};

use super::*;

fn file(db: &RootDatabase, path: &str, text: &str) -> SourceFile {
    SourceFile::new(db, path.into(), text.into(), FileOrigin::Source)
}

#[test]
fn lowers_signatures_and_promoted_properties() {
    let db = RootDatabase::default();
    let f = file(
        &db,
        "a.php",
        "<?php\nnamespace App;\nuse Lib\\Base;\n\n\
         function greet(string $name, int $times = 1): string { return $name; }\n\
         /** @template-covariant T of Base */\n\
         class User extends Base {\n\
             public function __construct(public readonly int $id, protected string $name) {}\n\
             public int $created = 0;\n    public function rename(string $to): void {}\n}\n",
    );
    let items = file_items(&db, f);
    let names: Vec<_> = items.functions.iter().map(|f| f.name.to_string()).collect();
    assert_eq!(names, ["App\\greet"]);
    let greet = &items.functions[0];
    assert!(!greet.sig.params[0].has_default);
    assert!(greet.sig.params[1].has_default);

    let user = &items.classes[0];
    assert_eq!(user.name.as_str(), "App\\User");
    assert_eq!(user.type_params[0].variance, Variance::Out);
    let bound = user.type_params[0].bound.as_ref().unwrap();
    assert!(matches!(&bound.kind, type_ref::TypeRefKind::Named(n, _) if n.as_str() == "Lib\\Base"));
    let ctor = user
        .methods
        .iter()
        .find(|m| m.name == "__construct")
        .unwrap();
    assert!(ctor.sig.params[0].promoted.as_ref().unwrap().immutable);
    assert!(!ctor.sig.params[1].promoted.as_ref().unwrap().immutable);
    assert_eq!(ctor.sig.params[1].name, "name");
    assert_eq!(user.properties[0].name, "created");
    assert!(!user.properties[0].immutable);
    assert!(user.methods.iter().any(|m| m.name == "rename"));
}

#[test]
fn lowers_php_declarations_and_index() {
    let mut db = RootDatabase::default();
    let a = file(
        &db,
        "a.php",
        "<?php\ndeclare(strict_types=1);\nnamespace Foo;\nuse Bar\\{Baz, Qux as Q};\n\
         /** doc */\nfinal class A extends Baz implements Q { public const X = 1; \
         public function __construct(private readonly int $id) {} \
         protected static ?string $p = null; abstract public function f(int ...$xs): static; }\n\
         enum E: string { case A = 'a'; }\nif (!function_exists('g')) { function g() {} }\n\
         define('LIMIT', 10); const MAX = 2;\n",
    );
    let items = file_items(&db, a);
    assert!(items.strict_types);
    let c = &items.classes[0];
    assert_eq!(c.name.as_str(), "Foo\\A");
    assert!(c.mods.is_final);
    assert!(c.doc.as_ref().unwrap().text.contains("doc"));
    assert!(
        matches!(&c.extends[0].kind, type_ref::TypeRefKind::Named(n, _) if n.as_str() == "Bar\\Baz")
    );
    assert!(
        matches!(&c.implements[0].kind, type_ref::TypeRefKind::Named(n, _) if n.as_str() == "Bar\\Qux")
    );
    let ctor = c.methods.iter().find(|m| m.name == "__construct").unwrap();
    assert!(
        ctor.sig.params[0]
            .promoted
            .as_ref()
            .unwrap()
            .mods
            .is_readonly
    );
    assert_eq!(c.properties[0].name, "p");
    assert!(c.properties[0].mods.is_static);
    let f = c.methods.iter().find(|m| m.name == "f").unwrap();
    assert!(f.sig.params[0].variadic && !f.has_body);
    assert_eq!(items.classes[1].kind, ClassKind::Enum);
    assert_eq!(items.classes[1].cases[0].name, "A");
    assert_eq!(items.functions[0].name.as_str(), "Foo\\g");
    let consts: Vec<_> = items.consts.iter().map(|c| c.name.to_string()).collect();
    assert_eq!(consts, ["LIMIT", "Foo\\MAX"]);

    let b = file(
        &db,
        "b.php",
        "<?php\nnamespace Foo;\nfunction shout(): void {}\n",
    );
    let ws = Workspace::new(&db, vec![a, b], stir_db::Settings::default());
    let index = def_index(&db, ws);
    assert!(index.classes.contains_key("foo\\a"));
    assert!(index.functions.contains_key("foo\\g"));
    assert!(index.functions.contains_key("foo\\shout"));

    // incremental: editing b doesn't change a's items
    use salsa::Setter;
    b.set_text(&mut db)
        .to("<?php\nnamespace Foo;\nclass B {}\n".into());
    let index = def_index(&db, ws);
    assert!(index.classes.contains_key("foo\\b"));
    assert!(!index.functions.contains_key("foo\\shout"));
}

#[test]
fn stub_attributes_select_the_target_version() {
    let db = RootDatabase::default();
    let f = file(
        &db,
        "s.php",
        "<?php\nuse JetBrains\\PhpStorm\\Internal\\PhpStormStubsElementAvailable as Avail;\n\
         use JetBrains\\PhpStorm\\Internal\\LanguageLevelTypeAware;\n\
         function array_map(?callable $callback, #[Avail(from: '8.0')] array $array, \
         #[Avail(from: '5.3', to: '7.4')] $arrays, array ...$arrays): array {}\n\
         #[Avail('5.3', '7.4')] function old() {}\n\
         #[Avail(from: '8.0')] function fresh() {}\n\
         #[LanguageLevelTypeAware(['8.0' => 'string[]', '9.0' => 'int'], default: 'string[]|false')]\n\
         function explode(string $s) {}\n\
         /**\n * @param array $array\n * @param mixed $filter_value [optional] <p>x</p>\n */\n\
         function array_keys(array $array, mixed $filter_value, bool $strict = false): array {}\n\
         class C { #[Avail(to: '7.0')] const OLD = 1; #[Avail(from: '8.4')] public function m() {} \
         #[LanguageLevelTypeAware(['8.1' => 'string'], default: '')] public $p; }\n",
    );
    let items = file_items(&db, f);
    let names: Vec<_> = items.functions.iter().map(|f| f.name.to_string()).collect();
    assert_eq!(names, ["array_map", "fresh", "explode", "array_keys"]);
    let map = &items.functions[0].sig.params;
    assert_eq!(map.len(), 3);
    assert!(map[2].variadic && map[2].name == "arrays");
    let explode = &items.functions[2];
    assert!(matches!(
        explode.sig.ret.as_ref().unwrap().kind,
        type_ref::TypeRefKind::PhpArray { .. }
    ));
    let keys = &items.functions[3].sig.params;
    assert!(!keys[0].has_default && keys[1].has_default && keys[2].has_default);
    let c = &items.classes[0];
    assert!(c.consts.is_empty());
    assert_eq!(c.methods.len(), 1);
    assert!(matches!(
        c.properties[0].ty.as_ref().unwrap().kind,
        type_ref::TypeRefKind::Php(type_ref::PhpBuiltin::String)
    ));
}

#[test]
fn phpdoc_types_templates_and_aliases() {
    use type_ref::TypeRefKind as K;
    let db = RootDatabase::default();
    let f = file(
        &db,
        "a.php",
        "<?php\nnamespace App;\nuse Lib\\Thing;\n\
         /**\n * @template-covariant T of Thing\n * @extends Base<T>\n\
          * @phpstan-type Row array{id: int, name?: string}\n\
          * @phpstan-import-type Other from Thing as Imported\n\
          * @property-read int $magic\n * @method static list<T> all(int $n = 1)\n */\n\
         class Box extends Base {\n\
             /** @var list<T> */ private array $items = [];\n\
             /**\n * @param Row $r\n * @return Imported\n */\n public function f(array $r) {}\n\
             /**\n * @template U\n * @param callable(T): U $fn\n * @return Box<U>\n */\n\
             public function map(callable $fn): Box {}\n\
         }\n",
    );
    let items = file_items(&db, f);
    let c = &items.classes[0];
    assert_eq!(c.type_params[0].name, "T");
    assert_eq!(c.type_params[0].variance, Variance::Out);
    assert!(
        matches!(&c.type_params[0].bound.as_ref().unwrap().kind, K::Named(n, _) if n.as_str() == "Lib\\Thing")
    );
    assert!(
        matches!(&c.extends[0].kind, K::Named(n, args) if n.as_str() == "App\\Base" && args.len() == 1)
    );
    assert_eq!(c.type_aliases[0].0, "Row");
    let items_prop = c.properties.iter().find(|p| p.name == "items").unwrap();
    assert!(
        matches!(&items_prop.doc_ty.as_ref().unwrap().kind, K::PhpList { elem, .. } if matches!(elem.kind, K::Param(_)))
    );
    let magic = c.properties.iter().find(|p| p.name == "magic").unwrap();
    assert!(magic.magic && magic.immutable);
    let all = c.methods.iter().find(|m| m.name == "all").unwrap();
    assert!(all.magic && all.mods.is_static && all.sig.params[0].has_default);
    let fm = c.methods.iter().find(|m| m.name == "f").unwrap();
    assert!(
        matches!(&fm.sig.params[0].doc_ty.as_ref().unwrap().kind, K::Shape { fields, sealed: true } if fields.len() == 2 && fields[1].optional)
    );
    assert!(
        matches!(&fm.sig.doc_ret.as_ref().unwrap().kind, K::ImportedAlias { class, name } if class.as_str() == "Lib\\Thing" && name == "Other")
    );
    let map = c.methods.iter().find(|m| m.name == "map").unwrap();
    assert_eq!(map.sig.type_params[0].name, "U");
    assert!(
        matches!(&map.sig.params[0].doc_ty.as_ref().unwrap().kind, K::Callable { params, closure: false, .. } if params.len() == 1)
    );
    assert!(
        matches!(&map.sig.doc_ret.as_ref().unwrap().kind, K::Named(n, args) if n.as_str() == "App\\Box" && matches!(args[0].kind, K::Param(_)))
    );
}
