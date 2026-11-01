//! Hover: declarations and expression types.

use stir_checker::model_api::{self, MemberTy};
use stir_checker::{Ctx, Def, LocalKind, MemberKind, typed_file};
use stir_db::{Db, SourceFile, Workspace};
use stir_hir::{Binding, ClassKind, file_items};
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
    let mut result = hover_in_php_terms(db, ws, file, offset)?;
    if file.dialect(db) == stir_syntax::Dialect::Stir {
        result.markdown = stir_checker::stir_type_names(&result.markdown);
    }
    Some(result)
}

fn hover_in_php_terms(
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
        markdown: render::code(&ty.to_string()),
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

/// A local binding as it is declared: `val x: Int`, `var $x: Int`, `mut x: Int`.
pub fn local_label(name: &str, kind: LocalKind, ty: &Ty) -> String {
    let name = name.trim_start_matches('$');
    match kind {
        LocalKind::Val => format!("val {name}: {ty}"),
        LocalKind::Var => format!("var ${name}: {ty}"),
        LocalKind::Param(Binding::Val) => format!("{name}: {ty}"),
        LocalKind::Param(Binding::Mut) => format!("mut {name}: {ty}"),
        LocalKind::Param(Binding::Php) | LocalKind::Php | LocalKind::Loop | LocalKind::This => {
            format!("${name}: {ty}")
        }
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
    let ctx = Ctx::for_file(db, ws, file);
    match def {
        Def::Local { decl } => {
            let typed = typed_file(db, ws, file);
            let (declared, kind) = typed.locals.get(decl)?.clone();
            let name = text_at(db, file, *decl);
            // a val keeps its literal type internally; show the declared one
            let ty = narrowed
                .filter(|t| !t.is_error())
                .map(|t| stir_checker::widen_literals(&t))
                .unwrap_or(declared);
            Some(render::code(&local_label(&name, kind, &ty)))
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
        Def::TypeAlias(loc) => {
            let a = &file_items(db, loc.file).type_aliases[loc.index as usize];
            let root = stir_db::parse(db, loc.file).syntax();
            let node = root
                .covering_element(a.name_range)
                .ancestors()
                .find(|n| n.text_range() != a.name_range)?;
            let code = node.text().to_string();
            Some(render::code(code.trim()))
        }
        Def::Class(key) => {
            let model = ctx.class(key)?;
            let view = model_api::class_view(&ctx, &model);
            let mut code = format!(
                "{} {}",
                render::class_keyword(model.kind(), &model.def.mods),
                model.def.name.short()
            );
            let tps: Vec<_> = view
                .type_params
                .iter()
                .map(|(n, b, _)| (n.clone(), b.clone()))
                .collect();
            code.push_str(&render::type_params(&tps));
            if model.kind() == ClassKind::Record {
                let fields: Vec<String> = model_api::members(
                    &ctx,
                    &ctx.record_ty(&model, model.params.iter().map(|p| p.ty.clone()).collect()),
                )
                .into_iter()
                .filter_map(|m| match m.ty {
                    MemberTy::Field(t) => Some(format!("{}: {t}", m.name)),
                    _ => None,
                })
                .collect();
                code.push_str(&format!(" {{ {} }}", fields.join("; ")));
            } else {
                let supers: Vec<String> = view
                    .parent
                    .iter()
                    .chain(view.interfaces.iter())
                    .map(|s| s.rsplit('\\').next().unwrap_or(s).to_owned())
                    .collect();
                if !supers.is_empty() {
                    code.push_str(" : ");
                    code.push_str(&supers.join(", "));
                }
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
                    let code = if p.immutable {
                        format!("val {name}: {ty}")
                    } else {
                        format!("var ${name}: {ty}")
                    };
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
                    Some(with_doc(format!("const {name}: {ty}"), Some(owner), None))
                }
                MemberKind::Case => Some(with_doc(format!("case {name}"), Some(owner), None)),
                MemberKind::Field => {
                    let rec =
                        ctx.record_ty(&model, model.params.iter().map(|p| p.ty.clone()).collect());
                    let ty = model_api::members(&ctx, &rec)
                        .into_iter()
                        .find(|m| &m.name == name)
                        .and_then(|m| match m.ty {
                            MemberTy::Field(t) => Some(t),
                            _ => None,
                        })?;
                    let ty = narrowed.filter(|t| !t.is_error()).unwrap_or(ty);
                    Some(with_doc(format!("{name}: {ty}"), Some(owner), None))
                }
            }
        }
    }
}

/// Is the declaration of `def` part of the project (and so renameable)?
pub fn is_project_def(db: &dyn Db, ws: Workspace, file: SourceFile, def: &Def) -> bool {
    let decl_file = match def {
        Def::Local { .. } => Some(file),
        Def::Function(l) | Def::Const(l) | Def::TypeAlias(l) => Some(l.file),
        Def::Class(k) | Def::Member { class: k, .. } => class_loc(db, ws, k).map(|l| l.file),
    };
    decl_file.is_some_and(|f| f.origin(db) == stir_db::FileOrigin::Source)
}
