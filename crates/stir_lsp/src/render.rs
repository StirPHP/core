//! Text shown to the user: signatures, declarations and doc comments.

use stir_checker::model_api::{ParamInfo, SigInfo};
use stir_hir::{ClassKind, DocText};
use stir_types::Ty;

/// A type as PHPDoc writes it, with class names as PHP's errors print them: no
/// leading backslash.
pub fn ty(ty: &Ty) -> String {
    let doc = stir_types::to_phpdoc(ty);
    let mut out = String::with_capacity(doc.len());
    let mut in_name = false;
    for c in doc.chars() {
        if c != '\\' || in_name {
            out.push(c);
        }
        in_name = c.is_alphanumeric() || c == '_' || c == '\\';
    }
    out
}

/// `int $name`, with `&`, `...` and ` = …` decorations.
pub fn param(p: &ParamInfo) -> String {
    let mut s = ty(&p.ty);
    s.push(' ');
    if p.by_ref {
        s.push('&');
    }
    if p.variadic {
        s.push_str("...");
    }
    s.push('$');
    s.push_str(&p.name);
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
            Some(b) => format!("{n} of {}", ty(b)),
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

/// `function name<T>(int $a, string $b): int`
pub fn signature(sig: &SigInfo, keyword: &str) -> SigLabel {
    let mut label = String::new();
    if !keyword.is_empty() {
        label.push_str(keyword);
        label.push(' ');
    }
    label.push_str(sig.name.rsplit('\\').next().unwrap_or(&sig.name));
    label.push_str(&type_params(&sig.type_params));
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
        label.push_str(&ty(&sig.ret));
    }
    SigLabel { label, params }
}

pub fn class_keyword(kind: ClassKind, mods: &stir_hir::ClassMods) -> String {
    let base = match kind {
        ClassKind::Class => "class",
        ClassKind::Interface => "interface",
        ClassKind::Trait => "trait",
        ClassKind::Enum => "enum",
    };
    let mut s = String::new();
    for (on, m) in [
        (mods.is_final, "final "),
        (mods.is_abstract && kind == ClassKind::Class, "abstract "),
        (mods.is_readonly, "readonly "),
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

/// A fenced PHP code block.
pub fn code(text: &str) -> String {
    format!("```php\n{text}\n```")
}
