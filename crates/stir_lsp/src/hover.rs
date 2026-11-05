//! Hover: declarations and expression types.

use stir_checker::model_api;
use stir_checker::{Ctx, Def, MemberKind, typed_file};
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
        markdown: render::code(&render::ty(ty)),
    })
}

fn with_doc(code: String, owner: Option<&str>, doc: Option<&stir_hir::DocText>) -> String {
    let mut out = String::new();
    if let Some(o) = owner.filter(|o| !o.is_empty()) {
        out.push_str(&render::code(o));
        out.push('\n');
    }
    out.push_str(&render::code(&code));
    if let Some(d) = doc {
        let d = render::doc(d);
        if !d.is_empty() {
            out.push_str("\n\n---\n\n");
            out.push_str(&d);
        }
    }
    out
}

/// A local variable: `int $x`.
pub fn local_label(name: &str, ty: &Ty) -> String {
    let name = name.trim_start_matches('$');
    format!("{} ${name}", render::ty(ty))
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
            let (declared, _) = typed.locals.get(decl)?.clone();
            let name = text_at(db, file, *decl);
            let ty = narrowed
                .filter(|t| !t.is_error())
                .map(|t| stir_checker::widen_literals(&t))
                .unwrap_or(declared);
            Some(render::code(&local_label(&name, &ty)))
        }
        Def::Function(loc) => {
            let f = ctx.function_def(*loc);
            let sig = model_api::function_info(&ctx, *loc);
            let ns = f.name.namespace();
            Some(with_doc(
                render::signature(&sig, "function").label,
                Some(ns),
                f.doc.as_ref(),
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
            Some(with_doc(code, Some(c.name.namespace()), None))
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
            Some(with_doc(
                code,
                Some(model.def.name.namespace()),
                model.def.doc.as_ref(),
            ))
        }
        Def::Member { class, kind, name } => {
            let model = ctx.class(class)?;
            let owner = model.def.name.as_str();
            match kind {
                MemberKind::Method => {
                    let sig = model_api::method_info(&ctx, class, name)?;
                    let m = model
                        .def
                        .methods
                        .iter()
                        .find(|m| m.name.eq_ignore_ascii_case(name))?;
                    let kw = if m.mods.is_static {
                        "static function"
                    } else {
                        "function"
                    };
                    Some(with_doc(
                        render::signature(&sig, kw).label,
                        Some(owner),
                        m.doc.as_ref(),
                    ))
                }
                MemberKind::Property => {
                    let p = ctx.find_property(&model.self_ty(), name)?;
                    let ty = narrowed.filter(|t| !t.is_error()).unwrap_or(p.ty);
                    let readonly = if p.immutable { "readonly " } else { "" };
                    let code = format!(
                        "{} {readonly}{} ${name}",
                        visibility(p.visibility),
                        render::ty(&ty)
                    );
                    let doc = model
                        .def
                        .properties
                        .iter()
                        .find(|q| &q.name == name)
                        .and_then(|q| q.doc.as_ref());
                    Some(with_doc(code, Some(owner), doc))
                }
                MemberKind::Const => {
                    let ty = ctx.find_const(&model.self_ty(), name)?;
                    Some(with_doc(
                        format!("const {} {name}", render::ty(&ty)),
                        Some(owner),
                        None,
                    ))
                }
                MemberKind::Case => Some(with_doc(format!("case {name}"), Some(owner), None)),
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
