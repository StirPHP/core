//! Hover: declarations and expression types.

use stir_checker::model_api;
use stir_checker::{Ctx, Def, LocalKind, MemberKind, typed_file};
use stir_db::{Db, SourceFile, Workspace};
use stir_hir::{ClassKind, Visibility, file_items};
use stir_syntax::{TextRange, TextSize};
use stir_types::Ty;

use crate::nav::{self, class_loc};
use crate::render;

pub struct HoverResult {
    pub range: TextRange,
    pub markdown: String,
}

pub fn hover(
    db: &dyn Db,
    ws: Workspace,
    file: SourceFile,
    offset: TextSize,
) -> Option<HoverResult> {
    if stir_syntax::is_blade_path(file.path(db))
        && let Some(tag) = stir_checker::script::links::tag_hover(db, ws, file, offset)
    {
        let mut markdown = String::new();
        if let Some(code) = &tag.code {
            markdown.push_str(&render::code(code));
            markdown.push_str("\n\n");
        }
        markdown.push_str(&tag.note);
        return Some(HoverResult {
            range: tag.range,
            markdown,
        });
    }
    let typed = typed_file(db, ws, file);
    if let Some((range, def)) = nav::def_at(db, ws, file, offset) {
        let narrowed = typed.types.get(&range).cloned();
        let markdown = describe(db, ws, file, &def, narrowed)?;
        return Some(HoverResult { range, markdown });
    }
    // the innermost typed expression under the cursor
    let root = stir_db::parse(db, file).syntax();
    let at = root.token_at_offset(offset);
    let token = match at.clone().right_biased() {
        Some(t) if !t.kind().is_trivia() => t,
        _ => at.left_biased().filter(|t| !t.kind().is_trivia())?,
    };
    let (range, ty) = typed
        .types
        .iter()
        .filter(|(r, _)| r.contains_range(token.text_range()))
        .min_by_key(|(r, _)| r.len())?;
    if ty.is_error() {
        return None;
    }
    Some(HoverResult {
        range: *range,
        markdown: render::code(&ty.to_string()),
    })
}

/// A declaration: what it's in, its code, what's up with it, and what its comment says.
fn with_notes(
    code: String,
    owner: Option<&str>,
    doc: Option<&stir_hir::DocText>,
    deprecated: Option<&str>,
    throws: Option<&Ty>,
) -> String {
    let mut out = String::new();
    if let Some(o) = owner.filter(|o| !o.is_empty()) {
        out.push_str(&render::code(o));
        out.push('\n');
    }
    out.push_str(&render::code(&code));
    if let Some(reason) = deprecated {
        out.push_str("\n\n**Deprecated**");
        if !reason.is_empty() {
            out.push_str(": ");
            out.push_str(reason);
        }
    }
    // a comment that has the tag shows it anyway
    let documented = |tag: &str| doc.is_some_and(|d| d.text.contains(tag));
    if let Some(ty) = throws.filter(|_| !documented("@throws")) {
        out.push_str(&format!("\n\n**Throws** `{ty}`"));
    }
    if let Some(d) = doc {
        let d = render::doc(d);
        if !d.is_empty() {
            out.push_str("\n\n---\n\n");
            out.push_str(&d);
        }
    }
    out
}

/// What a variable a template gets without declaring it is.
fn implicit_note(name: &str) -> Option<&'static str> {
    Some(match name {
        "loop" => "The loop around this point, kept by `@foreach` and `@forelse`.",
        "message" => "The first validation error of the field, in `@error`.",
        "value" => "What's stored under the key, in `@session` and `@context`.",
        "errors" => "The validation errors, shared with every template.",
        "slot" => "What's between the tags of this component.",
        "attributes" => "The attributes of this component that aren't among its `@props`.",
        "component" => "The component being rendered.",
        "exception" => "The exception this error page is for.",
        "__env" => "The view factory rendering this template.",
        _ => return None,
    })
}

/// A local variable: `int $x`.
pub fn local_label(name: &str, ty: &Ty) -> String {
    let name = name.trim_start_matches('$');
    format!("{} ${name}", ty)
}

fn visibility(v: Visibility) -> &'static str {
    match v {
        Visibility::Public => "public",
        Visibility::Protected => "protected",
        Visibility::Private => "private",
    }
}

fn text_at(db: &dyn Db, file: SourceFile, range: TextRange) -> String {
    file.text(db)
        .get(usize::from(range.start())..usize::from(range.end()))
        .unwrap_or_default()
        .to_owned()
}

pub fn describe(
    db: &dyn Db,
    ws: Workspace,
    file: SourceFile,
    def: &Def,
    narrowed: Option<Ty>,
) -> Option<String> {
    let ctx = Ctx::new(db, ws);
    match def {
        Def::Local { decl } => {
            let typed = typed_file(db, ws, file);
            let (declared, kind) = typed.locals.get(decl)?.clone();
            let name = typed
                .local_names
                .get(decl)
                .map_or_else(|| text_at(db, file, *decl), |n| n.to_string());
            let ty = narrowed
                .filter(|t| !t.is_error())
                .map(|t| stir_checker::widen_literals(&t))
                .unwrap_or(declared);
            let bare = name.trim_start_matches('$');
            if ty.is_untyped() && kind == LocalKind::Open {
                return Some(format!(
                    "{}\n\nNothing in this template declares it, so it's whatever is given to \
                     the template: that isn't known here. A `/** @var Type ${bare} */` in a \
                     `@php` block can say what it is.",
                    render::code(&format!("${bare}"))
                ));
            }
            let code = render::code(&local_label(&name, &ty));
            let given = kind == LocalKind::Open
                && stir_checker::views::view_data(db, ws, file)
                    .vars
                    .contains_key(bare);
            if given {
                return Some(format!(
                    "{code}\n\nGiven to this template by what renders it: this is what all of \
                     that comes to."
                ));
            }
            Some(
                match implicit_note(bare).filter(|_| kind == LocalKind::Implicit) {
                    Some(note) => format!("{code}\n\n{note}"),
                    None => code,
                },
            )
        }
        Def::Function(loc) => {
            let f = ctx.function_def(*loc);
            let sig = model_api::function_info(&ctx, *loc);
            let ns = f.name.namespace();
            let throws = f.sig.throws.as_ref().map(|t| ctx.lower_plain(t));
            Some(with_notes(
                render::signature(&sig, "function").label,
                Some(ns),
                f.doc.as_ref(),
                f.deprecated.as_deref(),
                throws.as_ref(),
            ))
        }
        Def::Const(loc) => {
            let c = &file_items(db, loc.file).consts[loc.index as usize];
            let value = c
                .value
                .as_ref()
                .map(|p| text_at(db, loc.file, p.text_range()))
                .filter(|v| v.len() <= 80);
            let code = match value {
                Some(v) => format!("const {} = {v}", c.name.short()),
                None => format!("const {}", c.name.short()),
            };
            Some(with_notes(
                code,
                Some(c.name.namespace()),
                None,
                c.deprecated.as_deref(),
                None,
            ))
        }

        Def::Class(key) => {
            let model = ctx.class(key)?;
            let view = model_api::class_view(&ctx, &model);
            let mut code = format!(
                "{} {}",
                render::class_keyword(model.kind(), &model.def.mods),
                model.def.name.short()
            );
            code.push_str(&render::type_params(&view.type_params));
            let short = |s: &str| s.rsplit('\\').next().unwrap_or(s).to_owned();
            if let Some(parent) = &view.parent {
                code.push_str(" extends ");
                code.push_str(&short(parent));
            }
            if !view.interfaces.is_empty() {
                let keyword = if model.kind() == ClassKind::Interface {
                    " extends "
                } else {
                    " implements "
                };
                let list: Vec<String> = view.interfaces.iter().map(|i| short(i)).collect();
                code.push_str(keyword);
                code.push_str(&list.join(", "));
            }
            Some(with_notes(
                code,
                Some(model.def.name.namespace()),
                model.def.doc.as_ref(),
                model.def.deprecated.as_deref(),
                None,
            ))
        }
        Def::Member { class, kind, name } => {
            let model = ctx.class(class)?;
            let owner = model.def.name.as_str();
            match kind {
                MemberKind::Method => {
                    let sig = model_api::method_info(&ctx, class, name)?;
                    let m = ctx.own_method(&model, name)?;
                    let kw = if m.mods.is_static {
                        "static function"
                    } else {
                        "function"
                    };
                    let throws = m.sig.throws.as_ref().map(|t| ctx.lower_plain(t));
                    Some(with_notes(
                        render::signature(&sig, kw).label,
                        Some(owner),
                        m.doc.as_ref(),
                        m.deprecated.as_deref(),
                        throws.as_ref(),
                    ))
                }
                MemberKind::Property => {
                    let p = ctx.find_property(&model.self_ty(), name)?;
                    let ty = narrowed.filter(|t| !t.is_error()).unwrap_or(p.ty);
                    let readonly = if p.immutable { "readonly " } else { "" };
                    let code = format!("{} {readonly}{} ${name}", visibility(p.visibility), ty);
                    let def = model.def.properties.iter().find(|q| &q.name == name);
                    let doc = def.and_then(|q| q.doc.as_ref());
                    let deprecated = def.and_then(|q| q.deprecated.as_deref());
                    Some(with_notes(code, Some(owner), doc, deprecated, None))
                }
                MemberKind::Const => {
                    let ty = ctx.find_const(&model.self_ty(), name)?;
                    let deprecated = model
                        .def
                        .consts
                        .iter()
                        .find(|k| &k.name == name)
                        .and_then(|k| k.deprecated.as_deref());
                    Some(with_notes(
                        format!("const {} {name}", ty),
                        Some(owner),
                        None,
                        deprecated,
                        None,
                    ))
                }
                MemberKind::Case => {
                    let deprecated = model
                        .def
                        .cases
                        .iter()
                        .find(|k| &k.name == name)
                        .and_then(|k| k.deprecated.as_deref());
                    Some(with_notes(
                        format!("case {name}"),
                        Some(owner),
                        None,
                        deprecated,
                        None,
                    ))
                }
            }
        }
    }
}

/// Is the declaration of `def` part of the project (and so renameable)?
pub fn is_project_def(db: &dyn Db, ws: Workspace, file: SourceFile, def: &Def) -> bool {
    let decl_file = match def {
        Def::Local { .. } => Some(file),
        Def::Function(l) | Def::Const(l) => Some(l.file),
        Def::Class(k) | Def::Member { class: k, .. } => class_loc(db, ws, k).map(|l| l.file),
    };
    decl_file.is_some_and(|f| f.origin(db) == stir_db::FileOrigin::Source)
}
