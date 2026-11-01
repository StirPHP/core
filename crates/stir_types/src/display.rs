//! Display for diagnostics ([`Ty`]'s `Display`) and PHPDoc rendering ([`to_phpdoc`]).
//!
//! Diagnostics use StirPHP syntax for StirPHP-family types (`Int?`, `List<String>`,
//! `(Int) -> Bool`, `T!`, records by name or as `{ x: Int }`) and PHP syntax for PHP types
//! (`?int`, `array<string, int>`, `array{a: int}`). A union is printed PHP-style
//! (`int|string`, `?int`) when it has a PHP-family member and no StirPHP-family member.

use std::fmt::{self, Write};

use crate::ty::{Family, FnTy, LitValue, Literal, ParamKind, SelfKind, ShapeKey, Ty, TyKind as K};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Display,
    PhpDoc,
}

/// Context precedence: how tightly the surrounding syntax binds.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Prec {
    /// Anywhere (top level, inside `<...>`, a parameter list, ...).
    Top,
    /// A union member: function types need parentheses.
    Union,
    /// An intersection member: unions need parentheses too.
    Intersection,
    /// Operand of a postfix `?`/`!` or a callable return type: anything compound does.
    Postfix,
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        print(f, self, Mode::Display, Prec::Top)
    }
}

impl fmt::Debug for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "`{self}`")
    }
}

/// Renders `ty` as a PHPStan-compatible PHPDoc type string, as used by the transpiler.
///
/// StirPHP types lower to their runtime PHP types (`Int` → `int`, `List<T>` → `list<T>`,
/// `Map<K, V>` → `array<K, V>`, records → `array{...}`, `Unit` → `void`,
/// `Nothing` → `never`, `(A) -> R` → `\Closure(A): R`), platform types to their inner type,
/// `Any!` (and `Error`) to `mixed`, and classes to fully qualified `\Name<...>`.
pub fn to_phpdoc(ty: &Ty) -> String {
    let mut s = String::new();
    print(&mut s, ty, Mode::PhpDoc, Prec::Top).expect("writing to a String cannot fail");
    s
}

fn print(w: &mut dyn Write, ty: &Ty, mode: Mode, prec: Prec) -> fmt::Result {
    let doc = mode == Mode::PhpDoc;
    let name = |stir: &'static str, php: &'static str| if doc { php } else { stir };
    match ty.kind() {
        K::Error => w.write_str(name("{error}", "mixed")),
        K::Int => w.write_str(name("Int", "int")),
        K::Float => w.write_str(name("Float", "float")),
        K::String => w.write_str(name("String", "string")),
        K::Bool => w.write_str(name("Bool", "bool")),
        K::Unit => w.write_str(name("Unit", "void")),
        K::Nothing => w.write_str(name("Nothing", "never")),
        K::List(t) | K::MutableList(t) => {
            let n = match (ty.kind(), doc) {
                (_, true) => "list",
                (K::List(_), false) => "List",
                _ => "MutableList",
            };
            write!(w, "{n}<")?;
            print(w, t, mode, Prec::Top)?;
            w.write_str(">")
        }
        K::Map(k, v) | K::MutableMap(k, v) => {
            let n = match (ty.kind(), doc) {
                (_, true) => "array",
                (K::Map(..), false) => "Map",
                _ => "MutableMap",
            };
            write!(w, "{n}<")?;
            print(w, k, mode, Prec::Top)?;
            w.write_str(", ")?;
            print(w, v, mode, Prec::Top)?;
            w.write_str(">")
        }
        K::Record(r) => {
            if doc {
                let fields: Vec<(ShapeKey, &Ty, bool)> = r
                    .fields
                    .iter()
                    .map(|f| (ShapeKey::from_str_key(&f.name), &f.ty, false))
                    .collect();
                return print_shape(w, &fields, true, mode);
            }
            if let Some(n) = &r.name {
                w.write_str(n)?;
                return print_args(w, &r.args, mode);
            }
            if r.fields.is_empty() {
                return w.write_str("{}");
            }
            w.write_str("{ ")?;
            for (i, f) in r.fields.iter().enumerate() {
                if i > 0 {
                    w.write_str(", ")?;
                }
                write!(w, "{}: ", f.name)?;
                print(w, &f.ty, mode, Prec::Top)?;
            }
            w.write_str(" }")
        }
        K::Fn(sig) => {
            if doc {
                return print_callable(w, "\\Closure", sig, mode, prec);
            }
            let parens = prec >= Prec::Union;
            if parens {
                w.write_str("(")?;
            }
            w.write_str("(")?;
            for (i, p) in sig.params.iter().enumerate() {
                if i > 0 {
                    w.write_str(", ")?;
                }
                match p.kind {
                    ParamKind::Required => print(w, &p.ty, mode, Prec::Top)?,
                    ParamKind::Optional => {
                        print(w, &p.ty, mode, Prec::Postfix)?;
                        w.write_str("=")?;
                    }
                    ParamKind::Variadic => {
                        w.write_str("...")?;
                        print(w, &p.ty, mode, Prec::Postfix)?;
                    }
                }
            }
            w.write_str(") -> ")?;
            let ret_prec = if matches!(sig.ret.kind(), K::Fn(_)) {
                Prec::Top
            } else {
                Prec::Intersection
            };
            print(w, &sig.ret, mode, ret_prec)?;
            if parens {
                w.write_str(")")?;
            }
            Ok(())
        }
        K::PhpInt => w.write_str("int"),
        K::PhpFloat => w.write_str("float"),
        K::PhpString => w.write_str("string"),
        K::PhpBool => w.write_str("bool"),
        K::Void => w.write_str("void"),
        K::Never => w.write_str("never"),
        K::Mixed => w.write_str("mixed"),
        K::Object => w.write_str("object"),
        K::Resource => w.write_str("resource"),
        K::Scalar => w.write_str("scalar"),
        K::Numeric => w.write_str("numeric"),
        K::ArrayKey => w.write_str("array-key"),
        K::NonEmptyString => w.write_str("non-empty-string"),
        K::NumericString => w.write_str("numeric-string"),
        K::Null => w.write_str("null"),
        K::ClassString(None) => w.write_str("class-string"),
        K::ClassString(Some(t)) => {
            w.write_str("class-string<")?;
            print(w, t, mode, Prec::Top)?;
            w.write_str(">")
        }
        K::IntRange { min, max } => match (min, max) {
            (Some(1), None) => w.write_str("positive-int"),
            (Some(0), None) => w.write_str("non-negative-int"),
            (None, Some(-1)) => w.write_str("negative-int"),
            (None, Some(0)) => w.write_str("non-positive-int"),
            _ => {
                let bound = |b: &Option<i64>, kw: &str| b.map_or(kw.to_string(), |n| n.to_string());
                write!(w, "int<{}, {}>", bound(min, "min"), bound(max, "max"))
            }
        },
        K::Array {
            key,
            value,
            non_empty,
        } => {
            let n = if *non_empty {
                "non-empty-array"
            } else {
                "array"
            };
            w.write_str(n)?;
            let untyped_key = matches!(key.kind(), K::ArrayKey);
            if untyped_key && matches!(value.kind(), K::Mixed) {
                return Ok(());
            }
            w.write_str("<")?;
            if !untyped_key {
                print(w, key, mode, Prec::Top)?;
                w.write_str(", ")?;
            }
            print(w, value, mode, Prec::Top)?;
            w.write_str(">")
        }
        K::PhpList { elem, non_empty } => {
            w.write_str(if *non_empty {
                "non-empty-list<"
            } else {
                "list<"
            })?;
            print(w, elem, mode, Prec::Top)?;
            w.write_str(">")
        }
        K::Shape(s) => {
            let fields: Vec<(ShapeKey, &Ty, bool)> = s
                .fields
                .iter()
                .map(|f| (f.key.clone(), &f.ty, f.optional))
                .collect();
            print_shape(w, &fields, s.sealed, mode)
        }
        K::Iterable { key, value } => {
            w.write_str("iterable")?;
            let untyped_key = matches!(key.kind(), K::Mixed);
            if untyped_key && matches!(value.kind(), K::Mixed) {
                return Ok(());
            }
            w.write_str("<")?;
            if !untyped_key {
                print(w, key, mode, Prec::Top)?;
                w.write_str(", ")?;
            }
            print(w, value, mode, Prec::Top)?;
            w.write_str(">")
        }
        K::Callable { sig, closure } => {
            let n = match (closure, doc) {
                (false, _) => "callable",
                (true, true) => "\\Closure",
                (true, false) => "Closure",
            };
            match sig {
                None => w.write_str(n),
                Some(sig) => print_callable(w, n, sig, mode, prec),
            }
        }
        K::Literal(l) => print_literal(w, l, mode),
        K::Class { name, args } => {
            if doc {
                w.write_str("\\")?;
            }
            w.write_str(name)?;
            print_args(w, args, mode)
        }
        K::Param { name, .. } => w.write_str(name),
        K::SelfType { kind, .. } => w.write_str(match kind {
            SelfKind::Static => "static",
            SelfKind::This => "$this",
        }),
        K::Union(ms) => print_union(w, ms, mode, prec),
        K::Intersection(ms) => {
            let parens = prec >= Prec::Union;
            let php_style = doc || ms.iter().any(Ty::is_php);
            if parens {
                w.write_str("(")?;
            }
            for (i, m) in ms.iter().enumerate() {
                if i > 0 {
                    w.write_str(if php_style { "&" } else { " & " })?;
                }
                print(w, m, mode, Prec::Intersection)?;
            }
            if parens {
                w.write_str(")")?;
            }
            Ok(())
        }
        K::Platform(inner) => {
            if doc {
                return print(w, inner, mode, prec);
            }
            print(w, inner, mode, Prec::Postfix)?;
            w.write_str("!")
        }
        K::AnyPlatform => w.write_str(name("Any!", "mixed")),
    }
}

fn print_args(w: &mut dyn Write, args: &[Ty], mode: Mode) -> fmt::Result {
    if args.is_empty() {
        return Ok(());
    }
    w.write_str("<")?;
    for (i, a) in args.iter().enumerate() {
        if i > 0 {
            w.write_str(", ")?;
        }
        print(w, a, mode, Prec::Top)?;
    }
    w.write_str(">")
}

/// `callable(int, string=, int...): R` / `\Closure(...)`.
fn print_callable(
    w: &mut dyn Write,
    name: &str,
    sig: &FnTy,
    mode: Mode,
    prec: Prec,
) -> fmt::Result {
    let parens = prec >= Prec::Union;
    if parens {
        w.write_str("(")?;
    }
    write!(w, "{name}(")?;
    for (i, p) in sig.params.iter().enumerate() {
        if i > 0 {
            w.write_str(", ")?;
        }
        match p.kind {
            ParamKind::Required => print(w, &p.ty, mode, Prec::Top)?,
            ParamKind::Optional => {
                print(w, &p.ty, mode, Prec::Postfix)?;
                w.write_str("=")?;
            }
            ParamKind::Variadic => {
                print(w, &p.ty, mode, Prec::Postfix)?;
                w.write_str("...")?;
            }
        }
    }
    w.write_str("): ")?;
    print(w, &sig.ret, mode, Prec::Postfix)?;
    if parens {
        w.write_str(")")?;
    }
    Ok(())
}

/// Whether `?T` is unambiguous for `T`.
fn qmark_ok(t: &Ty) -> bool {
    match t.kind() {
        K::Fn(_) | K::Callable { sig: Some(_), .. } | K::Intersection(_) | K::Union(_) => false,
        K::AnyPlatform | K::Mixed | K::Error => false,
        K::Platform(inner) => qmark_ok(inner),
        _ => true,
    }
}

fn print_union(w: &mut dyn Write, ms: &[Ty], mode: Mode, prec: Prec) -> fmt::Result {
    let doc = mode == Mode::PhpDoc;
    let has_null = ms.iter().any(Ty::is_null);
    let non_null: Vec<&Ty> = ms.iter().filter(|m| !m.is_null()).collect();
    let php_style =
        doc || (non_null.iter().any(|m| m.is_php()) && !non_null.iter().any(|m| m.is_safe()));
    if let [single] = non_null.as_slice()
        && has_null
    {
        if !php_style {
            print(w, single, mode, Prec::Postfix)?;
            return w.write_str("?");
        }
        if qmark_ok(single) {
            w.write_str("?")?;
            return print(w, single, mode, Prec::Postfix);
        }
    }
    let parens = prec >= Prec::Intersection;
    if parens {
        w.write_str("(")?;
    }
    let sep = if php_style { "|" } else { " | " };
    for (i, m) in non_null.iter().enumerate() {
        if i > 0 {
            w.write_str(sep)?;
        }
        print(w, m, mode, Prec::Union)?;
    }
    if has_null {
        w.write_str(sep)?;
        w.write_str("null")?;
    }
    if parens {
        w.write_str(")")?;
    }
    Ok(())
}

fn print_shape(
    w: &mut dyn Write,
    fields: &[(ShapeKey, &Ty, bool)],
    sealed: bool,
    mode: Mode,
) -> fmt::Result {
    let positional = fields
        .iter()
        .enumerate()
        .all(|(i, (k, _, opt))| *k == ShapeKey::Int(i as i64) && !opt);
    w.write_str("array{")?;
    for (i, (key, ty, optional)) in fields.iter().enumerate() {
        if i > 0 {
            w.write_str(", ")?;
        }
        if !positional {
            match key {
                ShapeKey::Int(n) => write!(w, "{n}")?,
                ShapeKey::Str(s) if is_bare_key(s) => w.write_str(s)?,
                ShapeKey::Str(s) => write_single_quoted(w, s)?,
            }
            w.write_str(if *optional { "?: " } else { ": " })?;
        }
        print(w, ty, mode, Prec::Top)?;
    }
    if !sealed {
        w.write_str(if fields.is_empty() { "..." } else { ", ..." })?;
    }
    w.write_str("}")
}

fn is_bare_key(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn write_single_quoted(w: &mut dyn Write, s: &str) -> fmt::Result {
    w.write_str("'")?;
    for c in s.chars() {
        if c == '\'' || c == '\\' {
            w.write_char('\\')?;
        }
        w.write_char(c)?;
    }
    w.write_str("'")
}

fn write_double_quoted(w: &mut dyn Write, s: &str) -> fmt::Result {
    w.write_str("\"")?;
    for c in s.chars() {
        match c {
            '"' => w.write_str("\\\"")?,
            '\\' => w.write_str("\\\\")?,
            '$' => w.write_str("\\$")?,
            '\n' => w.write_str("\\n")?,
            '\r' => w.write_str("\\r")?,
            '\t' => w.write_str("\\t")?,
            c => w.write_char(c)?,
        }
    }
    w.write_str("\"")
}

fn print_literal(w: &mut dyn Write, l: &Literal, mode: Mode) -> fmt::Result {
    match &l.value {
        LitValue::Int(n) => write!(w, "{n}"),
        LitValue::Float(x) => write!(w, "{:?}", x.get()),
        LitValue::Bool(b) => write!(w, "{b}"),
        LitValue::String(s) => {
            if mode == Mode::Display && l.family == Family::Safe {
                write_double_quoted(w, s)
            } else {
                write_single_quoted(w, s)
            }
        }
    }
}
