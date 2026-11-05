//! `Display` for [`Ty`], in PHP and PHPStan type syntax: `?int`, `int|string`,
//! `array<string, int>`, `array{a: int}`, `Closure(int): bool`, `'a'|'b'`. A value without a
//! known type, and the error-recovery type, print as `mixed`.

use std::fmt::{self, Write};

use crate::ty::{FnTy, LitValue, ParamKind, SelfKind, ShapeKey, Ty, TyKind as K};

/// Context precedence: how tightly the surrounding syntax binds.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Prec {
    /// Anywhere (top level, inside `<...>`, a parameter list, ...).
    Top,
    /// A union member: callables need parentheses.
    Union,
    /// An intersection member: unions need parentheses too.
    Intersection,
    /// Operand of a prefix `?` or a callable return type: anything compound does.
    Postfix,
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        print(f, self, Prec::Top)
    }
}

impl fmt::Debug for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "`{self}`")
    }
}

fn print(w: &mut dyn Write, ty: &Ty, prec: Prec) -> fmt::Result {
    match ty.kind() {
        K::Error | K::Mixed | K::Untyped => w.write_str("mixed"),
        K::Int => w.write_str("int"),
        K::Float => w.write_str("float"),
        K::String => w.write_str("string"),
        K::Bool => w.write_str("bool"),
        K::Void => w.write_str("void"),
        K::Never => w.write_str("never"),
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
            print(w, t, Prec::Top)?;
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
                print(w, key, Prec::Top)?;
                w.write_str(", ")?;
            }
            print(w, value, Prec::Top)?;
            w.write_str(">")
        }
        K::List { elem, non_empty } => {
            w.write_str(if *non_empty {
                "non-empty-list<"
            } else {
                "list<"
            })?;
            print(w, elem, Prec::Top)?;
            w.write_str(">")
        }
        K::Shape(s) => {
            let fields: Vec<(ShapeKey, &Ty, bool)> = s
                .fields
                .iter()
                .map(|f| (f.key.clone(), &f.ty, f.optional))
                .collect();
            print_shape(w, &fields, s.sealed)
        }
        K::Iterable { key, value } => {
            w.write_str("iterable")?;
            let untyped_key = matches!(key.kind(), K::Mixed);
            if untyped_key && matches!(value.kind(), K::Mixed) {
                return Ok(());
            }
            w.write_str("<")?;
            if !untyped_key {
                print(w, key, Prec::Top)?;
                w.write_str(", ")?;
            }
            print(w, value, Prec::Top)?;
            w.write_str(">")
        }
        K::Callable { sig, closure } => {
            let n = if *closure { "Closure" } else { "callable" };
            match sig {
                None => w.write_str(n),
                Some(sig) => print_callable(w, n, sig, prec),
            }
        }
        K::Literal(l) => print_literal(w, l),
        K::Class { name, args } => {
            // an anonymous class's name is only for telling them apart
            w.write_str(name.split('#').next().unwrap_or(name))?;
            print_args(w, args)
        }
        K::Param { name, .. } => w.write_str(name),
        K::CovariantArg(t) => write!(w, "covariant {t}"),
        K::ContravariantArg(t) => write!(w, "contravariant {t}"),
        K::Conditional(c) => write!(
            w,
            "({} is {} ? {} : {})",
            c.subject, c.target, c.then, c.otherwise
        ),
        K::SelfType { kind, .. } => w.write_str(match kind {
            SelfKind::Static => "static",
            SelfKind::This => "$this",
        }),
        K::Union(ms) => print_union(w, ms, prec),
        K::Intersection(ms) => {
            let parens = prec >= Prec::Union;
            if parens {
                w.write_str("(")?;
            }
            for (i, m) in ms.iter().enumerate() {
                if i > 0 {
                    w.write_str("&")?;
                }
                print(w, m, Prec::Intersection)?;
            }
            if parens {
                w.write_str(")")?;
            }
            Ok(())
        }
    }
}

fn print_args(w: &mut dyn Write, args: &[Ty]) -> fmt::Result {
    if args.is_empty() {
        return Ok(());
    }
    w.write_str("<")?;
    for (i, a) in args.iter().enumerate() {
        if i > 0 {
            w.write_str(", ")?;
        }
        print(w, a, Prec::Top)?;
    }
    w.write_str(">")
}

/// `callable(int, string=, int...): R` / `Closure(...)`.
fn print_callable(w: &mut dyn Write, name: &str, sig: &FnTy, prec: Prec) -> fmt::Result {
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
            ParamKind::Required => print(w, &p.ty, Prec::Top)?,
            ParamKind::Optional => {
                print(w, &p.ty, Prec::Postfix)?;
                w.write_str("=")?;
            }
            ParamKind::Variadic => {
                print(w, &p.ty, Prec::Postfix)?;
                w.write_str("...")?;
            }
        }
    }
    w.write_str("): ")?;
    print(w, &sig.ret, Prec::Postfix)?;
    if parens {
        w.write_str(")")?;
    }
    Ok(())
}

/// Whether `?T` is unambiguous for `T`.
fn qmark_ok(t: &Ty) -> bool {
    !matches!(
        t.kind(),
        K::Callable { sig: Some(_), .. }
            | K::Intersection(_)
            | K::Union(_)
            | K::Untyped
            | K::Mixed
            | K::Error
    )
}

fn print_union(w: &mut dyn Write, ms: &[Ty], prec: Prec) -> fmt::Result {
    let has_null = ms.iter().any(Ty::is_null);
    let non_null: Vec<&Ty> = ms.iter().filter(|m| !m.is_null()).collect();
    if let [single] = non_null.as_slice()
        && has_null
        && qmark_ok(single)
    {
        w.write_str("?")?;
        return print(w, single, Prec::Postfix);
    }
    let parens = prec >= Prec::Intersection;
    if parens {
        w.write_str("(")?;
    }
    for (i, m) in non_null.iter().enumerate() {
        if i > 0 {
            w.write_str("|")?;
        }
        print(w, m, Prec::Union)?;
    }
    if has_null {
        w.write_str("|null")?;
    }
    if parens {
        w.write_str(")")?;
    }
    Ok(())
}

fn print_shape(w: &mut dyn Write, fields: &[(ShapeKey, &Ty, bool)], sealed: bool) -> fmt::Result {
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
        print(w, ty, Prec::Top)?;
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

fn print_literal(w: &mut dyn Write, l: &LitValue) -> fmt::Result {
    match l {
        LitValue::Int(n) => write!(w, "{n}"),
        LitValue::Float(x) => write!(w, "{:?}", x.get()),
        LitValue::Bool(b) => write!(w, "{b}"),
        LitValue::String(s) => write_single_quoted(w, s),
    }
}
