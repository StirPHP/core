//! Inlay type hints and signature help.

use stir_checker::model_api::{self, SigInfo};
use stir_checker::{Ctx, Def, LocalKind, MemberKind, typed_file};
use stir_db::{Db, SourceFile, Workspace};
use stir_syntax::{Dialect, SyntaxKind::*, SyntaxNode, TextRange, TextSize};

use crate::nav;
use crate::render;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeHint {
    /// Where the hint is shown (after the name).
    pub offset: TextSize,
    pub label: String,
}

/// `: T` after bindings declared without a type: PHP variables and untyped parameters,
/// `val`/`var` and lambda parameters.
pub fn inlay_hints(
    db: &dyn Db,
    ws: Workspace,
    file: SourceFile,
    range: TextRange,
) -> Vec<TypeHint> {
    let root = stir_db::parse(db, file).syntax();
    let typed = typed_file(db, ws, file);
    let php = file.dialect(db) == Dialect::Php;
    let mut out = Vec::new();
    for (decl, (ty, kind)) in typed.locals.iter() {
        if !range.contains_range(*decl) || ty.is_error() || ty.contains_error() {
            continue;
        }
        let Some(token) = root.covering_element(*decl).into_token() else {
            continue;
        };
        if php {
            let ty = stir_checker::widen_literals(&stir_checker::strip_platform(ty));
            let wanted = match kind {
                LocalKind::Php | LocalKind::Loop => true,
                // a parameter's type is only news if it isn't written next to it
                LocalKind::Param(_) => token
                    .parent()
                    .and_then(|n| n.ancestors().find(|a| a.kind() == PARAM))
                    .is_some_and(|p| stir_hir::first_type(&p).is_none()),
                _ => false,
            };
            let label = render::ty(&ty);
            if wanted && label != "mixed" {
                out.push(TypeHint {
                    offset: decl.end(),
                    label: format!(": {label}"),
                });
            }
            continue;
        }
        let Some(holder) = token.parent().and_then(|n| {
            n.ancestors().find(|a| {
                matches!(
                    a.kind(),
                    VAL_DECL | VAR_DECL | PARAM | FOREACH_STMT | CATCH_CLAUSE
                )
            })
        }) else {
            continue;
        };
        let wanted = match (holder.kind(), kind) {
            (VAL_DECL, LocalKind::Val) | (VAR_DECL, LocalKind::Var) => {
                stir_hir::first_type(&holder).is_none()
            }
            // lambda parameters without a written type
            (PARAM, LocalKind::Param(_)) => {
                holder
                    .parent()
                    .is_some_and(|p| p.kind() == LAMBDA_PARAM_LIST)
                    && stir_hir::first_type(&holder).is_none()
            }
            _ => false,
        };
        if wanted {
            out.push(TypeHint {
                offset: decl.end(),
                label: stir_checker::stir_type_names(&format!(": {ty}")),
            });
        }
    }
    out.sort_by_key(|h| h.offset);
    out
}

pub struct SignatureHelp {
    pub label: String,
    pub params: Vec<(u32, u32)>,
    pub active_param: u32,
    pub doc: Option<String>,
}

/// The innermost call around `offset` whose argument list contains it.
fn enclosing_call(
    root: &SyntaxNode,
    text: &str,
    offset: TextSize,
) -> Option<(SyntaxNode, SyntaxNode)> {
    root.descendants()
        .filter(|n| n.kind() == ARG_LIST)
        .filter(|n| {
            let r = n.text_range();
            let closed = n.last_token().is_some_and(|t| t.kind() == R_PAREN);
            r.start() < offset
                && (offset < r.end()
                    || (!closed
                        && text
                            .get(usize::from(r.end())..usize::from(offset))
                            .is_some_and(|gap| gap.trim().is_empty())))
        })
        .last()
        .and_then(|args| Some((args.parent()?, args)))
}

pub fn signature_help(
    db: &dyn Db,
    ws: Workspace,
    file: SourceFile,
    offset: TextSize,
) -> Option<SignatureHelp> {
    let root = stir_db::parse(db, file).syntax();
    let (call, args) = enclosing_call(&root, file.text(db), offset)?;
    // the callee name: `f`, `a->m`, `A::m`, `new A`
    let callee_name = match call.kind() {
        CALL_EXPR => call.children().next()?.text_range(),
        METHOD_CALL_EXPR | NULLSAFE_METHOD_CALL_EXPR | STATIC_CALL_EXPR => {
            let fetch = call.children().next()?;
            fetch
                .children()
                .filter(|c| c.kind() == NAME)
                .last()?
                .text_range()
        }
        NEW_EXPR => call.children().find(|c| c.kind() != ARG_LIST)?.text_range(),
        _ => return None,
    };
    let refs = nav::file_refs(db, ws, file);
    let def = refs
        .iter()
        .find(|(r, _)| *r == callee_name)
        .map(|(_, d)| d.clone())?;
    let ctx = Ctx::for_file(db, ws, file);
    let (sig, kw, doc): (SigInfo, &str, Option<stir_hir::DocText>) = match &def {
        Def::Function(loc) => (
            model_api::function_info(&ctx, *loc),
            "function",
            ctx.function_def(*loc).doc.clone(),
        ),
        Def::Member {
            class,
            kind: MemberKind::Method,
            name,
        } => {
            let doc = ctx.class(class).and_then(|c| {
                c.def
                    .methods
                    .iter()
                    .find(|m| m.name.eq_ignore_ascii_case(name))
                    .and_then(|m| m.doc.clone())
            });
            (model_api::method_info(&ctx, class, name)?, "function", doc)
        }
        Def::Class(key) if call.kind() == NEW_EXPR => {
            (model_api::ctor_info(&ctx, key)?, "new", None)
        }
        _ => return None,
    };
    // the receiver of an extension function is not an argument
    let mut sig = sig;
    sig.receiver = None;
    let label = render::signature(&sig, kw);
    let commas = args
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| t.kind() == COMMA && t.text_range().end() <= offset)
        .count() as u32;
    let last = sig.params.len().saturating_sub(1) as u32;
    let active = if sig.params.last().is_some_and(|p| p.variadic) {
        commas.min(last)
    } else {
        commas
    };
    Some(SignatureHelp {
        label: label.label,
        params: label.params,
        active_param: active,
        doc: doc.map(|d| render::doc(&d)).filter(|d| !d.is_empty()),
    })
}
