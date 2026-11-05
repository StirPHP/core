//! Signatures of stub functions.

use stir_db::{FileOrigin, RootDatabase, Settings, SourceFile, Workspace};

use crate::Ctx;

fn workspace(db: &RootDatabase) -> Workspace {
    let files = stir_stubs::stub_files()
        .map(|s| {
            SourceFile::new(
                db,
                format!("stubs://{}", s.path).into(),
                s.text.to_owned(),
                FileOrigin::Stub,
            )
        })
        .collect();
    Workspace::new(db, files, Settings::default())
}

/// `name(params): ret` of a stub function.
fn sig(ctx: &Ctx<'_>, name: &str) -> String {
    let sig = ctx.function_sig(ctx.function(name).unwrap());
    let params: Vec<String> = sig
        .params
        .iter()
        .map(|p| {
            format!(
                "{}{}${}{}",
                p.ty,
                if p.variadic { " ..." } else { " " },
                p.name,
                if p.optional { "?" } else { "" }
            )
        })
        .collect();
    format!("{name}({}): {}", params.join(", "), sig.ret)
}

#[test]
fn stub_signatures_follow_attributes_and_phpdoc() {
    let db = RootDatabase::default();
    let ws = workspace(&db);
    let ctx = Ctx::new(&db, ws);
    let cases = [
        // A declaration of our own replaces the stub's.
        "array_keys(array<K, mixed> $array, mixed $filter_value?, bool $strict?): list<K>",
        "str_contains(string $haystack, string $needle): bool",
        "json_decode(string $json, ?bool $associative?, int $depth?, int $flags?): mixed",
        // `#[LanguageLevelTypeAware(['8.0' => 'string'])]` is the native type.
        "substr(string $string, int $offset, ?int $length?): string",
        // `int<0, max>` from the PHPDoc refines `int`.
        "strlen(string $string): non-negative-int",
        "array_is_list(array $array): bool",
    ];
    for want in cases {
        let name = &want[..want.find('(').unwrap()];
        assert_eq!(sig(&ctx, name), want);
    }
    // `@template` on functions
    let iter = ctx.function_sig(ctx.function("iterator_to_array").unwrap());
    assert_eq!(
        iter.type_params
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        ["K", "V"]
    );
}

#[test]
fn a_new_property_is_a_dynamic_one_on_any_version() {
    let db = RootDatabase::default();
    let file = SourceFile::new(
        &db,
        "/a.php".into(),
        "<?php\nclass A {}\n$a = new A();\n$a->x = 1;\necho $a->y;\n".to_owned(),
        FileOrigin::Source,
    );
    let ws = Workspace::new(&db, vec![file], Settings::default());
    let diags = crate::check_file(&db, ws, file);
    let notes: Vec<usize> = diags.iter().map(|d| d.notes.len()).collect();
    assert_eq!(notes, [1, 0], "{diags:?}");
    assert!(diags[0].notes[0].contains("PHP 8.2"));
}
