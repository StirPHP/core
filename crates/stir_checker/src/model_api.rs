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

// ------------------------------------------------------------ language server

/// A parameter of [`SigInfo`].
#[derive(Debug, Clone)]
pub struct ParamInfo {
    pub name: SmolStr,
    pub ty: Ty,
    pub optional: bool,
    pub variadic: bool,
    pub by_ref: bool,
    pub binding: stir_hir::Binding,
}

/// A signature with parameter names, for display.
#[derive(Debug, Clone)]
pub struct SigInfo {
    pub name: SmolStr,
    pub type_params: Vec<(SmolStr, Option<Ty>)>,
    pub params: Vec<ParamInfo>,
    pub ret: Ty,
    /// Was a return type written (or known from the declaration)?
    pub ret_declared: bool,
    pub receiver: Option<Ty>,
}

fn info(ctx: &Ctx<'_>, sig: &FnSig) -> SigInfo {
    SigInfo {
        name: sig.name.clone(),
        type_params: sig
            .type_params
            .iter()
            .map(|p| (p.name.clone(), stir_types::TypeEnv::param_bound(ctx, p.id)))
            .collect(),
        params: sig
            .params
            .iter()
            .map(|p| ParamInfo {
                name: p.name.clone(),
                ty: p.ty.clone(),
                optional: p.optional,
                variadic: p.variadic,
                by_ref: p.by_ref,
                binding: p.binding,
            })
            .collect(),
        ret: sig.ret.clone(),
        ret_declared: sig.ret_declared,
        receiver: sig.receiver.clone(),
    }
}

/// The signature of a function (or extension function).
pub fn function_info(ctx: &Ctx<'_>, loc: ItemLoc) -> SigInfo {
    info(ctx, &ctx.function_sig(loc))
}

/// The signature of method `name` declared by the class with key `class`.
pub fn method_info(ctx: &Ctx<'_>, class: &str, name: &str) -> Option<SigInfo> {
    let c = ctx.class(class)?;
    let m = c
        .def
        .methods
        .iter()
        .find(|m| m.name.eq_ignore_ascii_case(name))?;
    Some(info(ctx, &ctx.method_sig(&c, m)))
}

/// The constructor of class `class`: its primary constructor or `__construct`
/// (possibly inherited).
pub fn ctor_info(ctx: &Ctx<'_>, class: &str) -> Option<SigInfo> {
    let c = ctx.class(class)?;
    if let Some(params) = &c.def.primary_ctor {
        let cx = ctx.class_cx(&c);
        let mut sink = Vec::new();
        return Some(SigInfo {
            name: c.def.name.short().into(),
            type_params: Vec::new(),
            params: params
                .iter()
                .map(|p| {
                    let s = ctx.param_sig(p, &cx, &mut sink);
                    ParamInfo {
                        name: s.name,
                        ty: s.ty,
                        optional: s.optional,
                        variadic: s.variadic,
                        by_ref: s.by_ref,
                        binding: s.binding,
                    }
                })
                .collect(),
            ret: c.self_ty(),
            ret_declared: true,
            receiver: None,
        });
    }
    let (owner, m, _) = ctx.find_method(&c.self_ty(), "__construct")?;
    let mut sig = info(ctx, &ctx.method_sig(&owner, m));
    sig.name = c.def.name.short().into();
    Some(sig)
}

/// What a member is.
#[derive(Debug, Clone)]
pub enum MemberTy {
    Property { ty: Ty, immutable: bool },
    Method(SigInfo),
    Const(Ty),
    Case,
    Field(Ty),
}

/// A member visible on a type, for completion.
#[derive(Debug, Clone)]
pub struct MemberInfo {
    pub name: SmolStr,
    /// Lowercase FQN of the declaring class (empty for record fields).
    pub class: SmolStr,
    pub ty: MemberTy,
    pub is_static: bool,
    pub visibility: stir_hir::Visibility,
}

/// Every member of `ty` (a class or record type), nearest declaration first.
pub fn members(ctx: &Ctx<'_>, ty: &Ty) -> Vec<MemberInfo> {
    let mut out: Vec<MemberInfo> = Vec::new();
    if let TyKind::Record(r) = ty.kind() {
        for f in &r.fields {
            out.push(MemberInfo {
                name: f.name.clone(),
                class: SmolStr::default(),
                ty: MemberTy::Field(f.ty.clone()),
                is_static: false,
                visibility: stir_hir::Visibility::Public,
            });
        }
        return out;
    }
    let mut seen = rustc_hash::FxHashSet::default();
    for anc in ctx.ancestors(ty) {
        let TyKind::Class { name, args } = anc.kind() else {
            continue;
        };
        let Some(c) = ctx.class(name) else { continue };
        let map = ctx.subst_map(&c, args);
        let mut push = |out: &mut Vec<MemberInfo>, m: MemberInfo, kind: u8| {
            if seen.insert((kind, m.name.to_ascii_lowercase())) {
                out.push(m);
            }
        };
        for m in &c.def.methods {
            if m.name.starts_with("__") {
                continue;
            }
            let mut sig = info(ctx, &ctx.method_sig(&c, m));
            sig.params
                .iter_mut()
                .for_each(|p| p.ty = stir_types::subst(&p.ty, &map));
            sig.ret = stir_types::subst(&sig.ret, &map);
            let member = MemberInfo {
                name: m.name.clone(),
                class: c.key.clone(),
                ty: MemberTy::Method(sig),
                is_static: m.mods.is_static,
                visibility: m.mods.visibility,
            };
            push(&mut out, member, 0);
        }
        let props = c.def.properties.iter().map(|p| (p.name.clone(), p.mods.is_static, p.mods.visibility));
        let promoted = c
            .def
            .primary_ctor
            .iter()
            .flatten()
            .chain(
                c.def
                    .methods
                    .iter()
                    .filter(|m| m.name.eq_ignore_ascii_case("__construct"))
                    .flat_map(|m| m.sig.params.iter()),
            )
            .filter_map(|p| p.promoted.as_ref().map(|pr| (p.name.clone(), false, pr.mods.visibility)));
        for (pname, is_static, visibility) in props.chain(promoted).collect::<Vec<_>>() {
            let Some(p) = ctx.find_property(&anc, &pname) else {
                continue;
            };
            let member = MemberInfo {
                name: pname,
                class: c.key.clone(),
                ty: MemberTy::Property {
                    ty: p.ty,
                    immutable: p.immutable,
                },
                is_static,
                visibility,
            };
            push(&mut out, member, 1);
        }
        for k in &c.def.consts {
            let ty = ctx.find_const(&anc, &k.name).unwrap_or_else(Ty::error);
            let member = MemberInfo {
                name: k.name.clone(),
                class: c.key.clone(),
                ty: MemberTy::Const(ty),
                is_static: true,
                visibility: k.mods.visibility,
            };
            push(&mut out, member, 2);
        }
        for k in &c.def.cases {
            let member = MemberInfo {
                name: k.name.clone(),
                class: c.key.clone(),
                ty: MemberTy::Case,
                is_static: true,
                visibility: stir_hir::Visibility::Public,
            };
            push(&mut out, member, 2);
        }
    }
    out
}

/// Extension functions callable on a receiver of type `recv`.
pub fn applicable_extensions(ctx: &Ctx<'_>, recv: &Ty) -> Vec<ItemLoc> {
    let mut out = Vec::new();
    for locs in ctx.index.extensions.values() {
        for &loc in locs {
            let sig = ctx.function_sig(loc);
            let Some(r) = &sig.receiver else { continue };
            let mut inf = crate::infer::Inference::new(sig.type_params.iter().map(|p| p.id));
            inf.unify(ctx, r, recv);
            inf.finish(&Ty::nothing());
            let want = inf.apply(r);
            if stir_types::is_assignable(ctx, recv, &want) {
                out.push(loc);
            }
        }
    }
    out
}
