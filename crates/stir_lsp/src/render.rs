//! Text shown to the user: signatures, declarations and doc comments.

use stir_checker::model_api::{ParamInfo, SigInfo};
use stir_hir::{Binding, ClassKind, DocText};
use stir_types::Ty;

/// `name: T`, `mut name: T`, `$name: T` (PHP), with `...`/` = …` decorations.
pub fn param(p: &ParamInfo) -> String {
    let mut s = String::new();
    if p.variadic {
        s.push_str("...");
    }
    match p.binding {
        Binding::Val => {}
        Binding::Mut => s.push_str("mut "),
        Binding::Php => s.push('$'),
    }
    if p.by_ref {
        s.insert(0, '&');
    }
    s.push_str(&p.name);
    s.push_str(": ");
    s.push_str(&p.ty.to_string());
    if p.optional {
        s.push_str(" = …");
    }
    s
}

/// `<T of Bound, U>`
pub fn type_params(params: &[(smol_str::SmolStr, Option<Ty>)]) -> String {
    if params.is_empty() {
        return String::new();
    }
    let list: Vec<String> = params
        .iter()
        .map(|(n, b)| match b {
            Some(b) => format!("{n} of {b}"),
            None => n.to_string(),
        })
        .collect();
    format!("<{}>", list.join(", "))
}

/// A signature label and the byte ranges of its parameters within it.
pub struct SigLabel {
    pub label: String,
    pub params: Vec<(u32, u32)>,
}

/// `function <T> Recv.name(a: Int, b: String): Ret`
pub fn signature(sig: &SigInfo, keyword: &str) -> SigLabel {
    let mut label = String::new();
    if !keyword.is_empty() {
        label.push_str(keyword);
        label.push(' ');
    }
    let tps = type_params(&sig.type_params);
    if !tps.is_empty() {
        label.push_str(&tps);
        label.push(' ');
    }
    if let Some(r) = &sig.receiver {
        label.push_str(&r.to_string());
        label.push('.');
    }
    label.push_str(sig.name.rsplit('\\').next().unwrap_or(&sig.name));
    label.push('(');
    let mut params = Vec::new();
    for (i, p) in sig.params.iter().enumerate() {
        if i > 0 {
            label.push_str(", ");
        }
        let start = label.len() as u32;
        label.push_str(&param(p));
        params.push((start, label.len() as u32));
    }
    label.push(')');
    if sig.ret_declared || !sig.ret.is_error() {
        label.push_str(": ");
        label.push_str(&sig.ret.to_string());
    }
    SigLabel { label, params }
}

pub fn class_keyword(kind: ClassKind, mods: &stir_hir::ClassMods) -> String {
    let base = match kind {
        ClassKind::Class => "class",
        ClassKind::Interface => "interface",
        ClassKind::Trait => "trait",
        ClassKind::Enum => "enum",
        ClassKind::Record => "record",
    };
    let mut s = String::new();
    for (on, m) in [
        (mods.is_sealed, "sealed "),
        (
            mods.is_abstract && !mods.is_sealed && kind == ClassKind::Class,
            "abstract ",
        ),
        (mods.is_open, "open "),
        (mods.is_data, "data "),
    ] {
        if on {
            s.push_str(m);
        }
    }
    s.push_str(base);
    s
}

/// The description of a doc comment, without the comment markers; PHPDoc
/// tags are kept as a list.
pub fn doc(doc: &DocText) -> String {
    let text = doc.text.as_str();
    let body = text
        .strip_prefix("/**")
        .unwrap_or(text)
        .strip_suffix("*/")
        .unwrap_or(text);
    let mut lines: Vec<String> = Vec::new();
    for line in body.lines() {
        let l = line.trim();
        let l = l.strip_prefix('*').unwrap_or(l);
        let l = l.strip_prefix(' ').unwrap_or(l);
        if let Some(tag) = l.strip_prefix('@') {
            lines.push(format!("*@{}*", tag.trim_end()));
        } else {
            lines.push(l.trim_end().to_owned());
        }
    }
    while lines.first().is_some_and(|l| l.is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

/// A fenced StirPHP code block.
pub fn code(text: &str) -> String {
    format!("```sphp\n{text}\n```")
}
