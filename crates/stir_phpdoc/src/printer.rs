//! Printing of types: canonical PHPStan syntax via `Display`, and an S-expression
//! form ([`TypeNode::to_sexpr`]) that makes structure/precedence explicit.

use std::fmt::{self, Display, Write};

use crate::ast::*;

fn quote_str(s: &str, q: Quote) -> String {
    match q {
        Quote::Single => format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'")),
        Quote::Double => {
            let mut o = String::from("\"");
            for c in s.chars() {
                match c {
                    '"' => o.push_str("\\\""),
                    '\\' => o.push_str("\\\\"),
                    '$' => o.push_str("\\$"),
                    '\n' => o.push_str("\\n"),
                    '\t' => o.push_str("\\t"),
                    '\r' => o.push_str("\\r"),
                    c => o.push(c),
                }
            }
            o.push('"');
            o
        }
    }
}

impl Display for ConstExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConstExpr::Int { raw, .. } | ConstExpr::Float { raw, .. } => f.write_str(raw),
            ConstExpr::String { value, quote } => f.write_str(&quote_str(value, *quote)),
            ConstExpr::ClassConst { class, name } => write!(f, "{class}::{name}"),
        }
    }
}

impl Display for ShapeKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            ShapeKeyKind::Identifier(s) | ShapeKeyKind::Int(s) => f.write_str(s),
            ShapeKeyKind::String { value, quote } => f.write_str(&quote_str(value, *quote)),
        }
    }
}

impl Display for TemplateParam {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name.name)?;
        if let Some(b) = &self.bound {
            write!(f, " of {b}")?;
        }
        if let Some(b) = &self.lower_bound {
            write!(f, " super {b}")?;
        }
        if let Some(d) = &self.default {
            write!(f, " = {d}")?;
        }
        Ok(())
    }
}

impl Display for CallableParam {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.ty)?;
        if self.by_ref || self.variadic || self.name.is_some() {
            f.write_char(' ')?;
        }
        if self.by_ref {
            f.write_char('&')?;
        }
        if self.variadic {
            f.write_str("...")?;
        }
        if let Some(n) = &self.name {
            write!(f, "${}", n.name)?;
        }
        if self.optional {
            f.write_char('=')?;
        }
        Ok(())
    }
}

/// Wraps in parens when `cond` holds.
struct P<'a>(&'a TypeNode, bool);

impl Display for P<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.1 {
            write!(f, "({})", self.0)
        } else {
            write!(f, "{}", self.0)
        }
    }
}

fn is_compound(t: &TypeNode) -> bool {
    matches!(
        t.kind,
        TypeKind::Union(_) | TypeKind::Intersection(_) | TypeKind::Conditional(_)
    )
}

fn needs_parens_as_suffix_base(t: &TypeNode) -> bool {
    is_compound(t) || matches!(t.kind, TypeKind::Nullable(_) | TypeKind::Callable(_))
}

fn needs_parens_in_list(t: &TypeNode) -> bool {
    is_compound(t) || matches!(t.kind, TypeKind::Nullable(_))
}

impl Display for TypeNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            TypeKind::Identifier(n) => f.write_str(n),
            TypeKind::This => f.write_str("$this"),
            TypeKind::Nullable(t) => write!(f, "?{}", P(t, needs_parens_in_list(t))),
            TypeKind::Union(ts) | TypeKind::Intersection(ts) => {
                let sep = if matches!(self.kind, TypeKind::Union(_)) {
                    "|"
                } else {
                    "&"
                };
                for (i, t) in ts.iter().enumerate() {
                    if i > 0 {
                        f.write_str(sep)?;
                    }
                    write!(f, "{}", P(t, needs_parens_in_list(t)))?;
                }
                Ok(())
            }
            TypeKind::Generic { base, args } => {
                write!(f, "{}<", base.name)?;
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    match a.variance {
                        Variance::Invariant => write!(f, "{}", a.ty)?,
                        Variance::Covariant => write!(f, "covariant {}", a.ty)?,
                        Variance::Contravariant => write!(f, "contravariant {}", a.ty)?,
                        Variance::Bivariant => f.write_char('*')?,
                    }
                }
                f.write_char('>')
            }
            TypeKind::Array(t) => write!(f, "{}[]", P(t, needs_parens_as_suffix_base(t))),
            TypeKind::OffsetAccess { ty, offset } => {
                write!(f, "{}[{offset}]", P(ty, needs_parens_as_suffix_base(ty)))
            }
            TypeKind::ArrayShape(s) => {
                write!(f, "{}{{", s.kind.as_str())?;
                let mut first = true;
                for item in &s.items {
                    if !first {
                        f.write_str(", ")?;
                    }
                    first = false;
                    if let Some(k) = &item.key {
                        write!(f, "{k}{}: ", if item.optional { "?" } else { "" })?;
                    }
                    write!(f, "{}", item.value)?;
                }
                if !s.sealed {
                    if !first {
                        f.write_str(", ")?;
                    }
                    f.write_str("...")?;
                    if let Some(u) = &s.unsealed {
                        match &u.key {
                            Some(k) => write!(f, "<{k}, {}>", u.value)?,
                            None => write!(f, "<{}>", u.value)?,
                        }
                    }
                }
                f.write_char('}')
            }
            TypeKind::ObjectShape(items) => {
                f.write_str("object{")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(
                        f,
                        "{}{}: {}",
                        item.key,
                        if item.optional { "?" } else { "" },
                        item.value
                    )?;
                }
                f.write_char('}')
            }
            TypeKind::Const(c) => write!(f, "{c}"),
            TypeKind::Callable(c) => {
                f.write_str(&c.name.name)?;
                if !c.templates.is_empty() {
                    f.write_char('<')?;
                    for (i, t) in c.templates.iter().enumerate() {
                        if i > 0 {
                            f.write_str(", ")?;
                        }
                        write!(f, "{t}")?;
                    }
                    f.write_char('>')?;
                }
                f.write_char('(')?;
                for (i, p) in c.params.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{p}")?;
                }
                let r = &c.return_type;
                write!(f, "): {}", P(r, is_compound(r)))
            }
            TypeKind::Conditional(c) => {
                f.write_char('(')?;
                match &c.subject {
                    ConditionalSubject::Type(t) => write!(f, "{}", P(t, needs_parens_in_list(t)))?,
                    ConditionalSubject::Parameter(v) => write!(f, "${}", v.name)?,
                }
                write!(
                    f,
                    " is {}{} ? {} : {})",
                    if c.negated { "not " } else { "" },
                    c.target,
                    c.if_type,
                    c.else_type
                )
            }
            TypeKind::Error(_) => f.write_str("<error>"),
        }
    }
}

impl TypeNode {
    /// S-expression rendering that makes the tree structure explicit.
    pub fn to_sexpr(&self) -> String {
        let mut s = String::new();
        sexpr(self, &mut s);
        s
    }
}

fn sexpr_all(ts: &[TypeNode], out: &mut String) {
    for t in ts {
        out.push(' ');
        sexpr(t, out);
    }
}

fn sexpr(t: &TypeNode, out: &mut String) {
    match &t.kind {
        TypeKind::Identifier(n) => out.push_str(n),
        TypeKind::This => out.push_str("$this"),
        TypeKind::Nullable(t) => {
            out.push_str("(? ");
            sexpr(t, out);
            out.push(')');
        }
        TypeKind::Union(ts) => {
            out.push_str("(|");
            sexpr_all(ts, out);
            out.push(')');
        }
        TypeKind::Intersection(ts) => {
            out.push_str("(&");
            sexpr_all(ts, out);
            out.push(')');
        }
        TypeKind::Generic { base, args } => {
            let _ = write!(out, "(<> {}", base.name);
            for a in args {
                out.push(' ');
                match a.variance {
                    Variance::Invariant => sexpr(&a.ty, out),
                    Variance::Covariant => {
                        out.push_str("(covariant ");
                        sexpr(&a.ty, out);
                        out.push(')');
                    }
                    Variance::Contravariant => {
                        out.push_str("(contravariant ");
                        sexpr(&a.ty, out);
                        out.push(')');
                    }
                    Variance::Bivariant => out.push('*'),
                }
            }
            out.push(')');
        }
        TypeKind::Array(t) => {
            out.push_str("([] ");
            sexpr(t, out);
            out.push(')');
        }
        TypeKind::OffsetAccess { ty, offset } => {
            out.push_str("(offset ");
            sexpr(ty, out);
            out.push(' ');
            sexpr(offset, out);
            out.push(')');
        }
        TypeKind::ArrayShape(s) => {
            let _ = write!(out, "({}{{}}", s.kind.as_str());
            for item in &s.items {
                out.push_str(" (");
                if let Some(k) = &item.key {
                    let _ = write!(out, "{k}{}: ", if item.optional { "?" } else { "" });
                }
                sexpr(&item.value, out);
                out.push(')');
            }
            if !s.sealed {
                out.push_str(" (...");
                if let Some(u) = &s.unsealed {
                    if let Some(k) = &u.key {
                        out.push(' ');
                        sexpr(k, out);
                    }
                    out.push(' ');
                    sexpr(&u.value, out);
                }
                out.push(')');
            }
            out.push(')');
        }
        TypeKind::ObjectShape(items) => {
            out.push_str("(object{}");
            for item in items {
                let _ = write!(
                    out,
                    " ({}{}: ",
                    item.key,
                    if item.optional { "?" } else { "" }
                );
                sexpr(&item.value, out);
                out.push(')');
            }
            out.push(')');
        }
        TypeKind::Const(c) => {
            let _ = write!(out, "(const {c})");
        }
        TypeKind::Callable(c) => {
            let _ = write!(out, "(callable {}", c.name.name);
            if !c.templates.is_empty() {
                out.push_str(" (templates");
                for tp in &c.templates {
                    let _ = write!(out, " ({tp})");
                }
                out.push(')');
            }
            out.push_str(" (params");
            for p in &c.params {
                out.push_str(" (");
                sexpr(&p.ty, out);
                if p.by_ref || p.variadic || p.name.is_some() || p.optional {
                    out.push(' ');
                }
                if p.by_ref {
                    out.push('&');
                }
                if p.variadic {
                    out.push_str("...");
                }
                if let Some(n) = &p.name {
                    let _ = write!(out, "${}", n.name);
                }
                if p.optional {
                    out.push('=');
                }
                out.push(')');
            }
            out.push_str(") ");
            sexpr(&c.return_type, out);
            out.push(')');
        }
        TypeKind::Conditional(c) => {
            out.push_str("(cond ");
            match &c.subject {
                ConditionalSubject::Type(t) => sexpr(t, out),
                ConditionalSubject::Parameter(v) => {
                    let _ = write!(out, "${}", v.name);
                }
            }
            out.push_str(if c.negated { " is-not " } else { " is " });
            sexpr(&c.target, out);
            out.push(' ');
            sexpr(&c.if_type, out);
            out.push(' ');
            sexpr(&c.else_type, out);
            out.push(')');
        }
        TypeKind::Error(m) => {
            let _ = write!(out, "(error {m:?})");
        }
    }
}
