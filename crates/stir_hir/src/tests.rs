use stir_db::{FileOrigin, RootDatabase, SourceFile, Workspace};
use stir_syntax::Dialect;

use super::*;

fn file(db: &RootDatabase, path: &str, text: &str) -> SourceFile {
    let dialect = stir_syntax::Dialect::from_path(path.as_ref()).unwrap();
    SourceFile::new(db, path.into(), text.into(), dialect, FileOrigin::Source)
}

#[test]
fn lowers_stir_declarations() {
    let db = RootDatabase::default();
    let f = file(
        &db,
        "a.sphp",
        "namespace App\nuse Lib\\Base\n\nfunction <T> List<T>.second(): T = this[1]\n\
         function greet(name: String, mut times: Int = 1): String { return name }\n\
         data class User<out T of Base>(val id: Int, var $name: String) : Base(id), Iface {\n\
             val created: Int = 0\n    function rename(to: String): Unit {}\n}\n\
         record Point { x: Int; y: Int }\n\
         typealias Status = \"a\" | \"b\"\n",
    );
    let items = file_items(&db, f);
    let names: Vec<_> = items.functions.iter().map(|f| f.name.to_string()).collect();
    assert_eq!(names, ["App\\second", "App\\greet"]);
    let ext = &items.functions[0];
    assert!(ext.receiver.is_some() && ext.expr_body);
    assert_eq!(ext.sig.type_params[0].name, "T");
    let greet = &items.functions[1];
    assert_eq!(greet.sig.params[0].binding, Binding::Val);
    assert_eq!(greet.sig.params[1].binding, Binding::Mut);
    assert!(greet.sig.params[1].has_default);

    let user = &items.classes[0];
    assert_eq!(user.name.as_str(), "App\\User");
    assert!(user.mods.is_data);
    assert_eq!(user.type_params[0].variance, Variance::Out);
    let bound = user.type_params[0].bound.as_ref().unwrap();
    assert!(matches!(&bound.kind, type_ref::TypeRefKind::Named(n, _) if n.as_str() == "Lib\\Base"));
    let ctor = user.primary_ctor.as_ref().unwrap();
    assert!(ctor[0].promoted.as_ref().unwrap().immutable);
    assert!(!ctor[1].promoted.as_ref().unwrap().immutable);
    assert_eq!(ctor[1].name, "name");
    assert_eq!(user.supertypes.len(), 2);
    assert!(user.supertypes[0].ctor_call && !user.supertypes[1].ctor_call);
    assert_eq!(user.properties[0].name, "created");
    assert!(user.properties[0].immutable);
    assert_eq!(user.methods[0].name, "rename");

    let point = &items.classes[1];
    assert_eq!(point.kind, ClassKind::Record);
    assert_eq!(
        point
            .fields
            .iter()
            .map(|f| f.name.as_str())
            .collect::<Vec<_>>(),
        ["x", "y"]
    );
    assert_eq!(items.type_aliases[0].name.as_str(), "App\\Status");
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
        "b.sphp",
        "namespace Foo\nfunction String.shout(): String = this\n",
    );
    let ws = Workspace::new(&db, vec![a, b]);
    let index = def_index(&db, ws);
    assert!(index.classes.contains_key("foo\\a"));
    assert!(index.functions.contains_key("foo\\g"));
    assert!(index.extensions.contains_key("shout"));

    // incremental: editing b doesn't change a's items
    use salsa::Setter;
    b.set_text(&mut db).to("namespace Foo\nclass B {}\n".into());
    let index = def_index(&db, ws);
    assert!(index.classes.contains_key("foo\\b"));
    assert!(!index.extensions.contains_key("shout"));
    let _ = Dialect::Php;
}
