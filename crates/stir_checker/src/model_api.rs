//! A small read-only view of the semantic model for the transpiler.

use std::rc::Rc;

use smol_str::SmolStr;
use stir_hir::{ItemLoc, MethodDef};
use stir_types::{Ty, TyKind, Variance};

use crate::model::{ClassModel, Ctx, FnSig};

pub type ClassModelRef<'db> = Rc<ClassModel<'db>>;

/// A resolved signature: type parameters (name, bound, variance), parameter
/// types (element type for variadics), return type, receiver.
#[derive(Debug, Clone)]
pub struct FnSigView {
    pub type_params: Vec<(SmolStr, Option<Ty>, Variance)>,
    pub params: Vec<Ty>,
    pub ret: Ty,
    pub receiver: Option<Ty>,
}

fn view(ctx: &Ctx<'_>, sig: &FnSig) -> FnSigView {
    FnSigView {
        type_params: sig
            .type_params
            .iter()
            .map(|p| {
                (
                    p.name.clone(),
                    stir_types::TypeEnv::param_bound(ctx, p.id),
                    p.variance,
                )
            })
            .collect(),
        params: sig.params.iter().map(|p| p.ty.clone()).collect(),
        ret: sig.ret.clone(),
        receiver: sig.receiver.clone(),
    }
}

pub fn fn_sig_view(ctx: &Ctx<'_>, loc: ItemLoc) -> FnSigView {
    view(ctx, &ctx.function_sig(loc))
}

pub fn method_sig_view<'db>(
    ctx: &Ctx<'db>,
    model: &ClassModelRef<'db>,
    m: &MethodDef,
) -> FnSigView {
    view(ctx, &ctx.method_sig(model, m))
}

/// What the transpiler needs to emit a class header.
#[derive(Debug, Clone)]
pub struct ClassView {
    pub type_params: Vec<(SmolStr, Option<Ty>, Variance)>,
    /// `@extends`/`@implements` tags for generic supertypes.
    pub super_docs: Vec<(&'static str, String)>,
    pub parent: Option<String>,
    pub interfaces: Vec<String>,
    /// Primary-constructor parameter types.
    pub ctor_params: Vec<Ty>,
    pub enum_backing: Option<Ty>,
}

pub fn class_view<'db>(ctx: &Ctx<'db>, model: &ClassModelRef<'db>) -> ClassView {
    let supers = ctx.supers(model);
    let name_of = |t: &Ty| match t.kind() {
        TyKind::Class { name, .. } => Some(name.to_string()),
        _ => None,
    };
    let mut super_docs = Vec::new();
    let generic = |t: &Ty| matches!(t.kind(), TyKind::Class { args, .. } if !args.is_empty());
    if let Some(p) = &supers.parent
        && generic(p)
    {
        super_docs.push(("extends", stir_types::to_phpdoc(p)));
    }
    for i in &supers.interfaces {
        if generic(i) {
            let tag = if model.kind() == stir_hir::ClassKind::Interface {
                "extends"
            } else {
                "implements"
            };
            super_docs.push((tag, stir_types::to_phpdoc(i)));
        }
    }
    let cx = ctx.class_cx(model);
    let mut sink = Vec::new();
    let ctor_params = model
        .def
        .primary_ctor
        .iter()
        .flatten()
        .map(|p| ctx.param_sig(p, &cx, &mut sink).ty)
        .collect();
    let enum_backing = model
        .def
        .enum_backing
        .as_ref()
        .map(|t| ctx.lower(t, &cx, &mut sink));
    let builtin = |n: &String| matches!(n.to_ascii_lowercase().as_str(), "unitenum" | "backedenum");
    ClassView {
        type_params: model
            .params
            .iter()
            .map(|p| {
                (
                    p.name.clone(),
                    stir_types::TypeEnv::param_bound(ctx, p.id),
                    p.variance,
                )
            })
            .collect(),
        super_docs,
        parent: supers.parent.as_ref().and_then(name_of),
        interfaces: supers
            .interfaces
            .iter()
            .filter_map(name_of)
            .filter(|n| !builtin(n))
            .collect(),
        ctor_params,
        enum_backing,
    }
}

/// The declared type of a property of `model`.
pub fn property_ty<'db>(ctx: &Ctx<'db>, model: &ClassModelRef<'db>, name: &str) -> Option<Ty> {
    ctx.find_property(&model.self_ty(), name).map(|p| p.ty)
}
