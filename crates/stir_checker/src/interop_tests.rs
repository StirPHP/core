//! Signatures of stub functions.

use stir_db::{FileOrigin, RootDatabase, Settings, SourceFile, Workspace};
use stir_syntax::Dialect;

use crate::Ctx;

fn workspace(db: &RootDatabase) -> Workspace {
    let files = stir_stubs::stub_files()
        .map(|s| {
            SourceFile::new(
                db,
                format!("stubs://{}", s.path).into(),
                s.text.to_owned(),
                Dialect::Php,
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
        // `$arrays` (PHP <= 7.4 only) is dropped, leaving the variadic.
        "array_map(?callable $callback, array $array, array ...$arrays): array",
        // `[optional]` in the PHPDoc; `int[]|string[]` refines `array`.
        "array_keys(array $array, mixed $filter_value?, bool $strict?): array<int>|array<string>",
        "str_contains(string $haystack, string $needle): bool",
        "json_decode(string $json, ?bool $associative?, int $depth?, int $flags?): mixed",
        // `#[LanguageLevelTypeAware(['8.0' => 'string[]'])]` is the native type.
        "explode(string $separator, string $string, int $limit?): array<string>",
        // `int<0, max>` from the PHPDoc refines `int`.
        "strlen(string $string): non-negative-int",
        "array_key_first(array $array): string|int|null",
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
        ["TKey", "TValue"]
    );
}
