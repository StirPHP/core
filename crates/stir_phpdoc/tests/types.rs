use stir_phpdoc::{Keyword, TextSize, TypeKind, parse_type};

fn render(cases: &[&str]) -> String {
    let mut out = String::new();
    for src in cases {
        let t = parse_type(src, TextSize::from(0));
        out.push_str(&format!("{src}\n  = {t}\n  = {}\n", t.to_sexpr()));
        // Printing must round-trip (for non-error types).
        if t.errors().is_empty() {
            let again = parse_type(&t.to_string(), TextSize::from(0));
            assert_eq!(again.to_sexpr(), t.to_sexpr(), "round-trip of {src}");
        }
    }
    out
}

#[test]
fn basic_types() {
    insta::assert_snapshot!(render(&[
        "int",
        "\\Foo\\Bar",
        "Foo\\Bar",
        "non-empty-string",
        "$this",
        "static",
        "?int",
        "?int[]",
        "?Foo<int>",
        "int|string|null",
        "A&B&C",
        "(A&B)|C",
        "(A|B)&C",
        "(int|string)[]",
        "int[][]",
        "?(int|string)",
        "array-key",
        "  int  ",
    ]));
}

#[test]
fn generics() {
    insta::assert_snapshot!(render(&[
        "array<int, string>",
        "array<string, array<int, list<Foo>>>",
        "Collection<int, covariant Foo>",
        "Bar<contravariant T, *>",
        "Foo<covariant>",
        "list<int>",
        "non-empty-list<string>",
        "iterable<int, Foo>",
        "class-string<T>",
        "key-of<T>",
        "value-of<Foo::BAR>",
        "int<0, max>",
        "int<min, 5>",
        "int<-5, 0x10>",
        "array<int,>",
        "array<\n  int,\n  string\n>",
        "Generator<int, string, mixed, void>",
        "T[K]",
        "T[K][]",
        "Foo<int>[]",
        "int-mask-of<Foo::BAR_*>",
    ]));
}

#[test]
fn shapes() {
    insta::assert_snapshot!(render(&[
        "array{}",
        "array{a: int, 'b'?: string, 0: x, \"c d\": bool}",
        "array{int, string}",
        "array{a: int, ...}",
        "array{a: int, ...<string, int>}",
        "array{a: int, ...<int>}",
        "list{int, string, ...}",
        "list{int, ...<string>}",
        "non-empty-array{a: int}",
        "array{int: string, list: int}",
        "array{a?: array{b: list<int>}}",
        "array{\n  a: int,\n  b: string,\n}",
        "array{Foo::BAR, int}",
        "object{a: int, b?: string}",
        "object{'a b': int}",
        "object{}",
    ]));
}

#[test]
fn literals_and_constants() {
    insta::assert_snapshot!(render(&[
        "1",
        "-1",
        "0x1F",
        "0o17",
        "0b1010",
        "1_000_000",
        "1.5",
        ".5",
        "1e10",
        "'foo'",
        "\"bar\\n\"",
        "'it\\'s'",
        "'a'|'b'|1|true|null",
        "Foo::BAR",
        "Foo::BAR_*",
        "Foo::*",
        "self::X",
        "\\Ns\\Foo::BAZ",
    ]));
}

#[test]
fn callables() {
    insta::assert_snapshot!(render(&[
        "callable",
        "callable(): void",
        "callable(int, string=, ...mixed): void",
        "callable(int $x, string &$y, int ...$rest): bool",
        "callable(): int|string",
        "callable(): (int|string)",
        "callable(): ?int",
        "callable(): int[]",
        "(callable(): int)[]",
        "Closure(int $x): void",
        "\\Closure(): static",
        "pure-callable(int): int",
        "callable(callable(int): string): void",
        "Closure<T>(T $x): T",
        "Closure<T of object, U = int>(T, U): array{T, U}",
        "callable(): callable(): int",
        "callable(int,): void",
        "callable(\n  int,\n  string\n): void",
        "callable(): array{a: int}",
        "callable(): $this",
    ]));
}

#[test]
fn conditionals() {
    insta::assert_snapshot!(render(&[
        "($x is int ? string : bool)",
        "($x is not null ? Foo : Bar)",
        "(T is int ? A : B)",
        "(T is not array<int> ? A|B : C)",
        "(T is int ? A : T is string ? B : C)",
        "($size is positive-int ? non-empty-list<T> : list<T>)",
        "(T is int ? (U is string ? A : B) : C)",
    ]));
}

#[test]
fn errors() {
    insta::assert_snapshot!(render(&[
        "",
        "array<int",
        "array{a: }",
        "int|",
        "callable(int)",
        "Foo::",
        "int string",
        "?int|string",
        "A|B&C",
        "($x ? A : B)",
        "<int>",
        ")",
    ]));
}

#[test]
fn ranges_are_absolute() {
    let t = parse_type("array<int, Foo>", TextSize::from(100));
    assert_eq!(u32::from(t.range.start()), 100);
    assert_eq!(u32::from(t.range.end()), 115);
    let TypeKind::Generic { base, args } = &t.kind else {
        panic!("expected generic")
    };
    assert_eq!(u32::from(base.range.start()), 100);
    assert_eq!(u32::from(args[1].ty.range.start()), 111);
    assert_eq!(u32::from(args[1].ty.range.end()), 114);
}

#[test]
fn keywords() {
    let t = parse_type("non-empty-list<int>", TextSize::from(0));
    assert_eq!(t.keyword(), Some(Keyword::NonEmptyList));
    let t = parse_type("POSITIVE-INT", TextSize::from(0));
    assert_eq!(t.keyword(), Some(Keyword::PositiveInt));
    let t = parse_type("Foo", TextSize::from(0));
    assert_eq!(t.keyword(), None);
}
