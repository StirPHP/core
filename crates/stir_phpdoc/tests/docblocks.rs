use std::fmt::Write;

use stir_phpdoc::*;

fn t(o: &Option<Text>) -> String {
    match o {
        Some(t) => format!("{:?}@{:?}", t.text, t.range),
        None => "-".into(),
    }
}

fn ty(o: &TypeNode) -> String {
    match &o.kind {
        TypeKind::Error(m) => format!("<error {m:?} @{:?}>", o.range),
        _ => format!("{} @{:?}", o.to_sexpr(), o.range),
    }
}

fn v(o: &Option<Variable>) -> String {
    o.as_ref()
        .map(|v| format!("${}@{:?}", v.name, v.range))
        .unwrap_or_else(|| "-".into())
}

fn tp(p: &TemplateParam) -> String {
    let opt = |o: &Option<TypeNode>| o.as_ref().map(ty).unwrap_or_else(|| "-".into());
    format!(
        "{} of={} super={} default={}",
        p.name.name,
        opt(&p.bound),
        opt(&p.lower_bound),
        opt(&p.default)
    )
}

fn dump(src: &str, offset: u32) -> String {
    let doc = parse_doc_comment(src, TextSize::from(offset));
    let mut o = String::new();
    writeln!(o, "range: {:?}", doc.range).unwrap();
    writeln!(o, "summary: {}", t(&doc.summary)).unwrap();
    writeln!(o, "description: {}", t(&doc.description)).unwrap();
    for tag in &doc.tags {
        write!(o, "@{} [{:?}] {:?}: ", tag.name, tag.prefix, tag.range).unwrap();
        match &tag.kind {
            TagKind::Param(p) => write!(
                o,
                "Param ty={} ref={} variadic={} name={} desc={}",
                p.ty.as_ref().map(ty).unwrap_or_else(|| "-".into()),
                p.by_ref,
                p.variadic,
                v(&p.name),
                t(&p.description)
            ),
            TagKind::Return(r) => write!(o, "Return {} desc={}", ty(&r.ty), t(&r.description)),
            TagKind::Var(r) => write!(
                o,
                "Var {} name={} desc={}",
                ty(&r.ty),
                v(&r.name),
                t(&r.description)
            ),
            TagKind::Throws(r) => write!(o, "Throws {} desc={}", ty(&r.ty), t(&r.description)),
            TagKind::Template(r) => write!(
                o,
                "Template {:?} {} desc={}",
                r.variance,
                tp(&r.param),
                t(&r.description)
            ),
            TagKind::Extends(r) => write!(o, "Extends {} desc={}", ty(&r.ty), t(&r.description)),
            TagKind::Implements(r) => {
                write!(o, "Implements {} desc={}", ty(&r.ty), t(&r.description))
            }
            TagKind::Use(r) => write!(o, "Use {} desc={}", ty(&r.ty), t(&r.description)),
            TagKind::Property(p) => write!(
                o,
                "Property {:?} {} ${} desc={}",
                p.access,
                ty(&p.ty),
                p.name.name,
                t(&p.description)
            ),
            TagKind::Method(m) => {
                write!(
                    o,
                    "Method static={} ret={} name={}@{:?} templates=[{}] params=[",
                    m.is_static,
                    m.return_type.as_ref().map(ty).unwrap_or_else(|| "-".into()),
                    m.name.name,
                    m.name.range,
                    m.templates.iter().map(tp).collect::<Vec<_>>().join("; ")
                )
                .unwrap();
                for p in &m.params {
                    write!(
                        o,
                        "({} {}{}${} default={}) ",
                        p.ty.as_ref()
                            .map(|t| t.to_string())
                            .unwrap_or_else(|| "-".into()),
                        if p.by_ref { "&" } else { "" },
                        if p.variadic { "..." } else { "" },
                        p.name.name,
                        t(&p.default)
                    )
                    .unwrap();
                }
                write!(o, "] desc={}", t(&m.description))
            }
            TagKind::TypeAlias(a) => write!(o, "TypeAlias {} = {}", a.name.name, ty(&a.ty)),
            TagKind::TypeAliasImport(a) => write!(
                o,
                "TypeAliasImport {} from {} as {}",
                a.name.name,
                ty(&a.from),
                a.alias.as_ref().map(|a| a.name.as_str()).unwrap_or("-")
            ),
            TagKind::Deprecated(d) => write!(o, "Deprecated desc={}", t(&d.description)),
            TagKind::Internal(d) => write!(o, "Internal desc={}", t(&d.description)),
            TagKind::Assert(a) => write!(
                o,
                "Assert {:?} negated={} equality={} {} target={} desc={}",
                a.kind,
                a.negated,
                a.equality,
                ty(&a.ty),
                a.target
                    .as_ref()
                    .map(|t| format!("{}@{:?}", t.to_source(), t.range))
                    .unwrap_or_else(|| "-".into()),
                t(&a.description)
            ),
            TagKind::ParamOut(p) => write!(
                o,
                "ParamOut {} name={} desc={}",
                ty(&p.ty),
                v(&p.name),
                t(&p.description)
            ),
            TagKind::SelfOut(r) => write!(o, "SelfOut {} desc={}", ty(&r.ty), t(&r.description)),
            TagKind::Pure => write!(o, "Pure"),
            TagKind::Impure => write!(o, "Impure"),
            TagKind::Ignore(ids) => write!(
                o,
                "Ignore {}",
                ids.iter()
                    .map(|i| format!("{}@{:?} comment={:?}", i.name, i.range, i.comment))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            TagKind::IgnoreNextLine => write!(o, "IgnoreNextLine"),
            TagKind::IgnoreLine => write!(o, "IgnoreLine"),
            TagKind::Generic(d) => write!(o, "Generic desc={}", t(&d.description)),
            TagKind::Invalid { message, text } => {
                write!(o, "Invalid {message:?} text={}", t(text))
            }
        }
        .unwrap();
        o.push('\n');
    }
    o
}

macro_rules! doc_snapshot {
    ($name:ident, $src:expr) => {
        #[test]
        fn $name() {
            let src: &str = $src;
            insta::assert_snapshot!(format!("{src}\n---\n{}", dump(src, 1000)));
        }
    };
}

doc_snapshot!(
    function_doc,
    r#"/**
     * Maps the items.
     * Second summary line.
     *
     * Longer description
     *     with indented code.
     *
     * @template T
     * @template-covariant U of object = \stdClass
     * @param array<int, T> $items The items
     *        spanning two lines.
     * @param callable(T): U $fn
     * @param int &$count by ref
     * @param string ...$rest
     * @param $untyped no type
     * @return list<U>|null mapped values
     * @throws \InvalidArgumentException when bad
     * @deprecated use other()
     * @internal
     */"#
);

doc_snapshot!(
    single_line_var,
    "/** @var array{a: int, 'b'?: string} $x */"
);

doc_snapshot!(var_without_name, "/** @var int */");

doc_snapshot!(
    class_doc,
    r#"/**
 * @phpstan-type UserShape array{
 *     id: int,
 *     name?: non-empty-string,
 * }
 * @phpstan-type Id int
 * @psalm-type Name = string
 * @phpstan-import-type Foo from \Other\Klass as Bar
 * @phpstan-import-type Baz from Klass
 * @extends Base<int, string>
 * @implements \IteratorAggregate<int, T>
 * @use SomeTrait<T>
 * @template-extends Legacy<int>
 * @property int $id
 * @property-read string $name read only
 * @property-write ?Foo $foo
 * @method static self create(int $a, string $b = 'x', ...$rest) Factory
 * @method int count()
 * @method foo()
 * @method static bar(array $opts = [1, 2])
 * @method static static make()
 * @method T get<T of object>(class-string<T> $cls)
 * @method void cb(callable(int): void $cb, &$out)
 */"#
);

doc_snapshot!(
    prefixed_tags,
    r#"/**
 * @param int $x
 * @psalm-param positive-int $x
 * @phpstan-param int<1, 10> $x
 * @param string $y
 * @return mixed
 * @phpstan-return non-empty-string
 * @phpstan-template T of int
 * @phpstan-assert int $x
 * @phpstan-assert-if-true !null $this->prop
 * @psalm-assert-if-false =string $y
 * @phpstan-assert non-empty-string $this->getName()
 * @phpstan-param-out int $z
 * @param-out string $w
 * @phpstan-self-out static<int>
 * @phpstan-this-out self
 * @phpstan-pure
 * @phpstan-impure
 * @phpstan-ignore argument.type, return.type (because reasons)
 * @phpstan-ignore-next-line
 * @phpstan-ignore-line
 * @see Something::else()
 * @phpstan-unknown foo bar
 * @ORM\Column(type="string")
 */"#
);

doc_snapshot!(
    error_recovery,
    r#"/**
 * @param array<int $x description
 * @param int|, $y
 * @return array{a: int,
 *   b: }
 * @template
 * @property int
 * @var callable(int) $cb
 * @method
 * @throws
 */"#
);

doc_snapshot!(
    tricky,
    r#"/**
 * @param array<string, array<int, Foo>> $x nested >>
 * @return callable(): int|string ambiguity resolved as union
 * @param Foo (some description in parens)
 * @param ($x is int ? A : B) $y
 * @return ($flag is true ? list<int> : array<string, int>)
 * @var Foo::BAR_*|Foo::BAZ
 * @param \Closure(int $x): void $cb trailing text
 * @return int[] description with [brackets] and <angles>
 * @param int$x
 * @return static|$this
 */"#
);

doc_snapshot!(
    text_only,
    "/**\n * Just a summary.\n *\n * And a description {@see Foo} with inline tag.\n */"
);

doc_snapshot!(no_stars, "/**\n    @param int $a\n    @return void\n*/");

#[test]
fn effective_precedence() {
    let doc = parse_doc_comment(
        r#"/**
 * @param int $x
 * @psalm-param positive-int $x
 * @phpstan-param int<1, 10> $x
 * @param string $y
 * @psalm-param non-empty-string $y
 * @return mixed
 * @psalm-return int
 * @throws A
 * @throws B
 * @template T
 * @psalm-template U
 * @phpstan-template T of int
 */"#,
        TextSize::from(0),
    );
    let params: Vec<String> = doc
        .params()
        .iter()
        .map(|p| {
            format!(
                "{} {}",
                p.ty.as_ref().unwrap(),
                p.name.as_ref().unwrap().name
            )
        })
        .collect();
    assert_eq!(params, ["int<1, 10> x", "non-empty-string y"]);
    assert_eq!(
        doc.param("y").unwrap().ty.as_ref().unwrap().to_string(),
        "non-empty-string"
    );
    assert_eq!(doc.return_tag().unwrap().ty.to_string(), "int");
    assert_eq!(doc.throws().len(), 2);
    let templates: Vec<String> = doc
        .templates()
        .iter()
        .map(|t| t.param.to_string())
        .collect();
    assert_eq!(templates, ["U", "T of int"]);
    assert_eq!(doc.tags.len(), 12);
    assert!(doc.type_errors().is_empty());
}

#[test]
fn type_errors_collected() {
    let doc = parse_doc_comment("/** @param array<int $x */", TextSize::from(10));
    let errs = doc.type_errors();
    assert_eq!(errs.len(), 1);
    // `array<int` starts at 10 + 11
    assert_eq!(u32::from(errs[0].range.start()), 21);
    assert_eq!(u32::from(errs[0].range.end()), 30);
    // The variable is still recovered.
    assert_eq!(doc.params()[0].name.as_ref().unwrap().name, "x");
}

#[test]
fn never_panics_on_garbage() {
    for src in [
        "",
        "/**",
        "*/",
        "/** */",
        "/** @ */",
        "/** @param */",
        "/** @param ((((((( */",
        "/** @method static */",
        "/** @phpstan-ignore */",
        "/** @phpstan-type */",
        "/** @phpstan-import-type A */",
        "/** @phpstan-import-type A from B as */",
        "/** @template T of */",
        "/** @param array{a: int, ...<> $x */",
        "/** @return 'unterminated */",
        "/** @return \"x\\\" */",
        "/** \u{1F600} @param \u{e9}t\u{e9} $\u{e9} */",
        "/**\r\n * @param int $x\r\n */",
    ] {
        let _ = parse_doc_comment(src, TextSize::from(0));
        let _ = parse_type(src, TextSize::from(0));
    }
}
