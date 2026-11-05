//! A small read-only view of the semantic model for the language server.

use std::rc::Rc;

use smol_str::SmolStr;
use stir_hir::ItemLoc;
use stir_types::{Ty, TyKind};

use crate::model::{ClassModel, Ctx, FnSig};

pub type ClassModelRef<'db> = Rc<ClassModel<'db>>;

/// A class header, for display.
#[derive(Debug, Clone)]
pub struct ClassView {
    pub type_params: Vec<(SmolStr, Option<Ty>)>,
    pub parent: Option<String>,
    pub interfaces: Vec<String>,
}

pub fn class_view<'db>(ctx: &Ctx<'db>, model: &ClassModelRef<'db>) -> ClassView {
    let supers = ctx.supers(model);
    let name_of = |t: &Ty| match t.kind() {
        TyKind::Class { name, .. } => Some(name.to_string()),
        _ => None,
    };
    let builtin = |n: &String| matches!(n.to_ascii_lowercase().as_str(), "unitenum" | "backedenum");
    ClassView {
        type_params: model
            .params
            .iter()
            .map(|p| (p.name.clone(), stir_types::TypeEnv::param_bound(ctx, p.id)))
            .collect(),
        parent: supers.parent.as_ref().and_then(name_of),
        interfaces: supers
            .interfaces
            .iter()
            .filter_map(name_of)
            .filter(|n| !builtin(n))
            .collect(),
    }
}

/// A parameter of [`SigInfo`].
#[derive(Debug, Clone)]
pub struct ParamInfo {
    pub name: SmolStr,
    pub ty: Ty,
    pub optional: bool,
    pub variadic: bool,
    pub by_ref: bool,
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
            })
            .collect(),
        ret: sig.ret.clone(),
        ret_declared: sig.ret_declared,
    }
}

/// The signature of a function.
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

/// The constructor of class `class`: `__construct` (possibly inherited).
pub fn ctor_info(ctx: &Ctx<'_>, class: &str) -> Option<SigInfo> {
    let c = ctx.class(class)?;

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
}

/// A member visible on a type, for completion.
#[derive(Debug, Clone)]
pub struct MemberInfo {
    pub name: SmolStr,
    /// Lowercase FQN of the declaring class.
    pub class: SmolStr,
    pub ty: MemberTy,
    pub is_static: bool,
    pub visibility: stir_hir::Visibility,
}

/// Every member of `ty` (a class type), nearest declaration first.
pub fn members(ctx: &Ctx<'_>, ty: &Ty) -> Vec<MemberInfo> {
    let ty = match ty.kind() {
        TyKind::SelfType { class, .. } => class,
        _ => ty,
    };
    let mut out: Vec<MemberInfo> = Vec::new();

    let mut seen = rustc_hash::FxHashSet::default();
    for anc in ctx.ancestors(ty).iter() {
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
        let props = c
            .def
            .properties
            .iter()
            .map(|p| (p.name.clone(), p.mods.is_static, p.mods.visibility));
        let promoted = c
            .def
            .methods
            .iter()
            .filter(|m| m.name.eq_ignore_ascii_case("__construct"))
            .flat_map(|m| m.sig.params.iter())
            .filter_map(|p| {
                p.promoted
                    .as_ref()
                    .map(|pr| (p.name.clone(), false, pr.mods.visibility))
            });
        for (pname, is_static, visibility) in props.chain(promoted).collect::<Vec<_>>() {
            let Some(p) = ctx.find_property(anc, &pname) else {
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
            let ty = ctx.find_const(anc, &k.name).unwrap_or_else(Ty::error);
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
