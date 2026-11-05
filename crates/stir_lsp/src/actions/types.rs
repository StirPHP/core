//! Writing types and names in source: what's expressible on the file's PHP versions, what can only
//! be said in PHPDoc, and the imports that takes.

use stir_checker::php_version as v;
use stir_db::{Db, PhpVersion, SourceFile, Workspace};
use stir_hir::name::{Fqn, UseKind};
use stir_hir::type_ref::{PhpBuiltin, TypeRef, TypeRefKind};
use stir_syntax::TextSize;
use stir_types::{LitValue, ParamKind, ShapeKey, Ty, TyKind};

use super::{Cx, FileEdit};
use crate::imports::{self, ImportEdit, Imports};

/// Names written at one place in a file: remembers what has to be imported for them.
pub struct Namer<'c> {
    db: &'c dyn Db,
    ws: Workspace,
    file: SourceFile,
    imports: Option<Imports<'c>>,
    wanted: Vec<(UseKind, Fqn, ImportEdit)>,
}

impl<'c> Namer<'c> {
    pub fn new(cx: &Cx<'c>, offset: TextSize) -> Namer<'c> {
        Namer {
            db: cx.db,
            ws: cx.ws,
            file: cx.file,
            imports: Imports::new(cx.db, cx.file, offset),
            wanted: Vec::new(),
        }
    }

    /// What to write for the class `fqn`.
    pub fn class(&mut self, fqn: &str) -> String {
        let fqn = Fqn::new(fqn.trim_start_matches('\\'));
        let Some(imports) = &self.imports else {
            return format!("\\{}", fqn.as_str());
        };
        if let Some(short) = self.short_of(UseKind::Class, &fqn) {
            return short;
        }
        let name = imports.class(self.db, self.ws, &fqn);
        // two of one short name: only one of them can have it
        if name.import.is_some()
            && self.wanted.iter().any(|(k, f, _)| {
                *k == UseKind::Class && f.short().eq_ignore_ascii_case(fqn.short())
            })
        {
            return format!("\\{}", fqn.as_str());
        }
        if let Some(edit) = name.import {
            self.wanted.push((UseKind::Class, fqn, edit));
        }
        name.text
    }

    fn short_of(&self, kind: UseKind, fqn: &Fqn) -> Option<String> {
        self.wanted
            .iter()
            .find(|(k, f, _)| *k == kind && f.key() == fqn.key())
            .map(|(_, f, _)| f.short().to_owned())
    }

    /// The `use` lines that what's been asked for takes, sorted as they'd have been one by one.
    pub fn finish(mut self) -> Vec<ImportEdit> {
        self.wanted
            .sort_by_key(|(_, f, _)| imports::sort_key(f.as_str()));
        imports::merged(self.wanted.into_iter().map(|(_, _, e)| e).collect())
    }

    /// [`finish`](Self::finish), as edits.
    pub fn edits(self) -> Vec<FileEdit> {
        let file = self.file;
        self.finish()
            .into_iter()
            .map(|e| FileEdit {
                file,
                range: stir_syntax::TextRange::empty(e.at),
                text: e.text,
            })
            .collect()
    }
}

/// `version` or later is what the file runs on, all of it.
pub fn has(cx: &Cx<'_>, feature: v::Feature) -> bool {
    at_least(cx, feature.since)
}

pub fn at_least(cx: &Cx<'_>, version: PhpVersion) -> bool {
    cx.versions.min >= version
}

/// A type for source, and whether it says all that's known of the value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    /// The native type.
    pub native: String,
    /// What PHPDoc could add to it, if `native` says less than is known.
    pub doc: Option<String>,
}

/// One part of a native type, without what can't stand alone.
fn native_member(
    cx: &Cx<'_>,
    namer: &mut Namer<'_>,
    ty: &Ty,
    parts: &mut Vec<String>,
) -> Option<bool> {
    let mut push = |s: String| {
        if !parts.contains(&s) {
            parts.push(s);
        }
    };
    // says it says less than is known
    let mut lossy = false;
    match ty.kind() {
        TyKind::Int => push("int".into()),
        TyKind::Float => push("float".into()),
        TyKind::String => push("string".into()),
        TyKind::Bool => push("bool".into()),
        TyKind::Null => push("null".into()),
        TyKind::Object => push("object".into()),
        TyKind::Mixed => push("mixed".into()),
        TyKind::NonEmptyString | TyKind::NumericString | TyKind::ClassString(_) => {
            push("string".into());
            lossy = true;
        }
        TyKind::IntRange { .. } => {
            push("int".into());
            lossy = true;
        }
        TyKind::ArrayKey => {
            push("int".into());
            push("string".into());
        }
        TyKind::Scalar => {
            for s in ["int", "float", "string", "bool"] {
                push(s.into());
            }
        }
        TyKind::Numeric => {
            for s in ["int", "float", "string"] {
                push(s.into());
            }
            lossy = true;
        }
        TyKind::Literal(l) => {
            push(
                match l {
                    LitValue::Bool(_) => "bool",
                    LitValue::Int(_) => "int",
                    LitValue::Float(_) => "float",
                    LitValue::String(_) => "string",
                }
                .into(),
            );
        }
        TyKind::Array { key, value, .. } => {
            push("array".into());
            lossy = !(matches!(
                value.kind(),
                TyKind::Mixed | TyKind::Untyped | TyKind::Error
            ) && matches!(key.kind(), TyKind::ArrayKey));
        }
        TyKind::List { .. } | TyKind::Shape(_) => {
            push("array".into());
            lossy = true;
        }
        TyKind::Iterable { value, .. } => {
            push("iterable".into());
            lossy = !matches!(value.kind(), TyKind::Mixed);
        }
        TyKind::Callable { sig, closure } => {
            let text = if *closure {
                namer.class("Closure")
            } else {
                "callable".to_owned()
            };
            push(text);
            lossy = sig.is_some();
        }
        TyKind::Class { name, args } => {
            // an anonymous class has no name to write
            if !is_class_name(name) {
                return None;
            }
            let text = namer.class(name);
            push(text);
            lossy = !args.is_empty();
        }
        TyKind::SelfType { .. } => {
            if !has(cx, v::STATIC_TYPE) {
                return None;
            }
            push("static".into());
        }
        _ => return None,
    }
    Some(lossy)
}

/// Is `name` one a class can be written by?
fn is_class_name(name: &str) -> bool {
    !name.is_empty()
        && name.trim_start_matches('\\').split('\\').all(|part| {
            part.chars()
                .next()
                .is_some_and(|c| c.is_alphabetic() || c == '_' || !c.is_ascii())
                && part
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || !c.is_ascii())
        })
}

/// `ty` as a native type on the file's versions, if there's one, for a value of that type.
pub fn native(cx: &Cx<'_>, namer: &mut Namer<'_>, ty: &Ty) -> Option<Written> {
    if ty.contains_error() {
        return None;
    }
    let ty = stir_checker::widen_literals(ty);
    let mut lossy = false;
    let text = match ty.kind() {
        TyKind::Void => "void".to_owned(),
        TyKind::Never => {
            if !has(cx, v::NEVER_TYPE) {
                return None;
            }
            "never".to_owned()
        }
        TyKind::Intersection(members) => {
            if !has(cx, v::INTERSECTION_TYPES) {
                return None;
            }
            let mut parts = Vec::new();
            for m in members.iter() {
                lossy |= native_member(cx, namer, m, &mut parts)?;
            }
            parts.join("&")
        }
        _ => {
            let mut parts = Vec::new();
            for m in ty.members() {
                lossy |= native_member(cx, namer, m, &mut parts)?;
            }
            // `true|false` is `bool`, and `mixed` swallows everything
            if parts.iter().any(|p| p == "mixed") {
                if !has(cx, v::MIXED_TYPE) {
                    return None;
                }
                "mixed".to_owned()
            } else {
                let null = parts.iter().any(|p| p == "null");
                parts.retain(|p| p != "null");
                match (parts.len(), null) {
                    (0, _) => {
                        if !has(cx, v::STANDALONE_NULL_FALSE) {
                            return None;
                        }
                        "null".to_owned()
                    }
                    (1, false) => parts.remove(0),
                    (1, true) => format!("?{}", parts[0]),
                    _ => {
                        if !has(cx, v::UNION_TYPES) {
                            return None;
                        }
                        if null {
                            parts.push("null".to_owned());
                        }
                        parts.join("|")
                    }
                }
            }
        }
    };
    if text == "mixed" && !has(cx, v::MIXED_TYPE) {
        return None;
    }
    let doc = if lossy { doc_type(namer, &ty) } else { None };
    Some(Written { native: text, doc })
}

/// `ty` in PHPDoc, with names as they're written here. `None` for what PHPDoc can't say here, or
/// isn't known well enough to be worth saying.
pub fn doc_type(namer: &mut Namer<'_>, ty: &Ty) -> Option<String> {
    let bare = |s: &str| Some(s.to_owned());
    match ty.kind() {
        TyKind::Error | TyKind::Untyped | TyKind::Param { .. } | TyKind::Conditional(_) => None,
        TyKind::Mixed => bare("mixed"),
        TyKind::Void => bare("void"),
        TyKind::Never => bare("never"),
        TyKind::Int
        | TyKind::Float
        | TyKind::String
        | TyKind::Bool
        | TyKind::Null
        | TyKind::Object
        | TyKind::Resource
        | TyKind::Scalar
        | TyKind::Numeric
        | TyKind::ArrayKey
        | TyKind::NonEmptyString
        | TyKind::NumericString
        | TyKind::IntRange { .. } => Some(ty.to_string()),
        // what it is now, not all it can be
        TyKind::Literal(_) => Some(stir_checker::widen_literals(ty).to_string()),
        TyKind::ClassString(None) => bare("class-string"),
        TyKind::ClassString(Some(of)) => Some(format!("class-string<{}>", doc_type(namer, of)?)),
        TyKind::Array {
            key,
            value,
            non_empty,
        } => {
            let word = if *non_empty {
                "non-empty-array"
            } else {
                "array"
            };
            let value = doc_type(namer, value)?;
            if matches!(key.kind(), TyKind::ArrayKey) {
                Some(format!("{word}<{value}>"))
            } else {
                Some(format!("{word}<{}, {value}>", doc_type(namer, key)?))
            }
        }
        TyKind::List { elem, non_empty } => {
            let word = if *non_empty { "non-empty-list" } else { "list" };
            Some(format!("{word}<{}>", doc_type(namer, elem)?))
        }
        TyKind::Shape(shape) => {
            let mut fields = Vec::new();
            let positional =
                shape.fields.iter().enumerate().all(|(i, f)| {
                    !f.optional && matches!(&f.key, ShapeKey::Int(k) if *k == i as i64)
                });
            for f in &shape.fields {
                if positional {
                    fields.push(doc_type(namer, &f.ty)?);
                    continue;
                }
                let key = match &f.key {
                    ShapeKey::Int(i) => i.to_string(),
                    ShapeKey::Str(s) => {
                        if !s.chars().all(|c| c.is_alphanumeric() || c == '_') || s.is_empty() {
                            return None;
                        }
                        s.to_string()
                    }
                };
                let mark = if f.optional { "?" } else { "" };
                fields.push(format!("{key}{mark}: {}", doc_type(namer, &f.ty)?));
            }
            Some(format!("array{{{}}}", fields.join(", ")))
        }
        TyKind::Iterable { key, value } => {
            let value = doc_type(namer, value)?;
            if matches!(key.kind(), TyKind::Mixed) {
                Some(format!("iterable<{value}>"))
            } else {
                Some(format!("iterable<{}, {value}>", doc_type(namer, key)?))
            }
        }
        TyKind::Callable { sig, closure } => {
            let word = if *closure {
                namer.class("Closure")
            } else {
                "callable".to_owned()
            };
            let Some(sig) = sig else { return Some(word) };
            let mut params = Vec::new();
            for p in &sig.params {
                let t = doc_type(namer, &p.ty)?;
                params.push(match p.kind {
                    ParamKind::Required => t,
                    ParamKind::Optional => format!("{t}="),
                    ParamKind::Variadic => format!("{t}..."),
                });
            }
            let ret = doc_type(namer, &sig.ret)?;
            let ret = if ret.contains('|') {
                format!("({ret})")
            } else {
                ret
            };
            Some(format!("{word}({}): {ret}", params.join(", ")))
        }
        TyKind::Class { name, args } => {
            if !is_class_name(name) {
                return None;
            }
            let text = namer.class(name);
            if args.is_empty() {
                return Some(text);
            }
            let mut written = Vec::new();
            for a in args {
                written.push(doc_type(namer, a)?);
            }
            Some(format!("{text}<{}>", written.join(", ")))
        }
        TyKind::SelfType { .. } => bare("static"),
        TyKind::Union(members) => {
            let mut parts = Vec::new();
            for m in members.iter() {
                let part = doc_type(namer, m)?;
                parts.push(if part.contains("): ") {
                    format!("({part})")
                } else {
                    part
                });
            }
            parts.dedup();
            Some(parts.join("|"))
        }
        TyKind::Intersection(members) => {
            let mut parts = Vec::new();
            for m in members.iter() {
                parts.push(doc_type(namer, m)?);
            }
            Some(parts.join("&"))
        }
        _ => None,
    }
}

/// A native type as a declaration has it, to be written elsewhere: names are complete in it.
/// `owner` is the class it's declared in, for `self`; `parent` the class's parent.
pub fn written_ref(
    namer: &mut Namer<'_>,
    ty: &TypeRef,
    owner: Option<&str>,
    parent: Option<&str>,
) -> Option<String> {
    fn go(
        namer: &mut Namer<'_>,
        ty: &TypeRef,
        owner: Option<&str>,
        parent: Option<&str>,
    ) -> Option<String> {
        Some(match &ty.kind {
            TypeRefKind::Php(p) => match p {
                PhpBuiltin::Int => "int".into(),
                PhpBuiltin::Float => "float".into(),
                PhpBuiltin::String => "string".into(),
                PhpBuiltin::Bool => "bool".into(),
                PhpBuiltin::Array => "array".into(),
                PhpBuiltin::Mixed => "mixed".into(),
                PhpBuiltin::Void => "void".into(),
                PhpBuiltin::Never => "never".into(),
                PhpBuiltin::Null => "null".into(),
                PhpBuiltin::False => "false".into(),
                PhpBuiltin::True => "true".into(),
                PhpBuiltin::Object => "object".into(),
                PhpBuiltin::Callable => "callable".into(),
                PhpBuiltin::Iterable => "iterable".into(),
                PhpBuiltin::Static => "static".into(),
                PhpBuiltin::SelfType => namer.class(owner?),
                PhpBuiltin::Parent => namer.class(parent?),
            },
            TypeRefKind::Named(fqn, args) if args.is_empty() => namer.class(fqn.as_str()),
            TypeRefKind::Nullable(inner) => {
                let inner = go(namer, inner, owner, parent)?;
                format!("?{inner}")
            }
            TypeRefKind::Union(parts) => {
                let mut out = Vec::new();
                for p in parts {
                    let text = go(namer, p, owner, parent)?;
                    out.push(if matches!(p.kind, TypeRefKind::Intersection(_)) {
                        format!("({text})")
                    } else {
                        text
                    });
                }
                out.join("|")
            }
            TypeRefKind::Intersection(parts) => {
                let mut out = Vec::new();
                for p in parts {
                    out.push(go(namer, p, owner, parent)?);
                }
                out.join("&")
            }
            _ => return None,
        })
    }
    go(namer, ty, owner, parent)
}
