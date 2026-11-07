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
    let m = ctx.own_method(&c, name)?;
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

/// The abstract methods and interface methods `model` has to implement and doesn't, with the class
/// of each: none for what doesn't have to, or what has an ancestor that can't be found, which may
/// well implement them.
pub fn unimplemented_methods<'db>(
    ctx: &Ctx<'db>,
    model: &ClassModelRef<'db>,
) -> Vec<(ClassModelRef<'db>, &'db stir_hir::MethodDef)> {
    use stir_hir::{ClassKind, Visibility};
    let kind = model.kind();
    if model.def.mods.is_abstract || matches!(kind, ClassKind::Interface | ClassKind::Trait) {
        return Vec::new();
    }
    let self_ty = model.self_ty();
    if !ctx.hierarchy_known(&self_ty) {
        return Vec::new();
    }
    let ancestors = ctx.ancestors(&self_ty);
    let classes: Vec<ClassModelRef<'db>> = ancestors
        .iter()
        .filter_map(|t| match t.kind() {
            TyKind::Class { name, .. } => ctx.class(name),
            _ => None,
        })
        .collect();
    let mut concrete = rustc_hash::FxHashSet::default();
    for c in &classes {
        for (alias, _) in &c.def.trait_aliases {
            concrete.insert(alias.to_ascii_lowercase());
        }
        if c.kind() == ClassKind::Interface {
            continue;
        }
        for m in &c.def.methods {
            let own = c.key == model.key;
            if !m.magic && !m.mods.is_abstract && (own || m.mods.visibility != Visibility::Private)
            {
                concrete.insert(m.name.to_ascii_lowercase());
            }
        }
    }
    let mut seen = rustc_hash::FxHashSet::default();
    let mut out = Vec::new();
    for c in &classes {
        for m in &c.def.methods {
            let lower = m.name.to_ascii_lowercase();
            if m.magic
                || !(c.kind() == ClassKind::Interface || m.mods.is_abstract)
                || concrete.contains(&lower)
                // what an enum has by being one
                || (kind == ClassKind::Enum && matches!(lower.as_str(), "cases" | "from" | "tryfrom"))
                || !seen.insert(lower)
            {
                continue;
            }
            out.push((c.clone(), m));
        }
    }
    out
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
    /// It, or its class, is marked deprecated.
    pub deprecated: bool,
    /// A method of another type that calls are forwarded to, as scripts say: it can be called
    /// statically or not, whatever it is there.
    pub forwarded: bool,
}

/// Every member of `ty` (a class type), nearest declaration first.
pub fn members(ctx: &Ctx<'_>, ty: &Ty) -> Vec<MemberInfo> {
    members_through(ctx, ty, 0)
}

/// [`members`], through `forwards` forwarded calls so far.
fn members_through(ctx: &Ctx<'_>, ty: &Ty, forwards: u32) -> Vec<MemberInfo> {
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
                deprecated: m.deprecated.is_some() || c.def.deprecated.is_some(),
                forwarded: false,
            };
            push(&mut out, member, 0);
        }
        let props = c.def.properties.iter().map(|p| {
            let deprecated = p.deprecated.is_some() || c.def.deprecated.is_some();
            (
                p.name.clone(),
                p.mods.is_static,
                p.mods.visibility,
                deprecated,
            )
        });
        let promoted = c
            .def
            .methods
            .iter()
            .filter(|m| m.name.eq_ignore_ascii_case("__construct"))
            .flat_map(|m| m.sig.params.iter())
            .filter_map(|p| {
                p.promoted
                    .as_ref()
                    .map(|pr| (p.name.clone(), false, pr.mods.visibility, false))
            });
        for (pname, is_static, visibility, deprecated) in props.chain(promoted).collect::<Vec<_>>()
        {
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
                deprecated,
                forwarded: false,
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
                deprecated: k.deprecated.is_some() || c.def.deprecated.is_some(),
                forwarded: false,
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
                deprecated: k.deprecated.is_some() || c.def.deprecated.is_some(),
                forwarded: false,
            };
            push(&mut out, member, 2);
        }
    }
    // what scripts declare, with everything declared in PHP going first
    if !ctx.scripts.magic.is_empty() {
        for anc in ctx.ancestors(ty).iter() {
            let TyKind::Class { name, args } = anc.kind() else {
                continue;
            };
            let Some(c) = ctx.class(name) else { continue };
            let map = ctx.subst_map(&c, args);
            let magic = ctx.script_magic(&c);
            for m in &magic.methods {
                if seen.insert((0, m.name.to_ascii_lowercase())) {
                    let mut sig = info(ctx, &ctx.method_sig(&c, m));
                    sig.ret = stir_types::subst(&sig.ret, &map);
                    out.push(MemberInfo {
                        name: m.name.clone(),
                        class: c.key.clone(),
                        ty: MemberTy::Method(sig),
                        is_static: m.mods.is_static,
                        visibility: m.mods.visibility,
                        deprecated: false,
                        forwarded: false,
                    });
                }
            }
            for p in &magic.properties {
                if seen.insert((1, p.name.to_ascii_lowercase()))
                    && let Some(found) = ctx.find_property(anc, &p.name)
                {
                    out.push(MemberInfo {
                        name: p.name.clone(),
                        class: c.key.clone(),
                        ty: MemberTy::Property {
                            ty: found.ty,
                            immutable: found.immutable,
                        },
                        is_static: false,
                        visibility: p.mods.visibility,
                        deprecated: false,
                        forwarded: false,
                    });
                }
            }
        }
        // the public methods of what calls are forwarded to, as `__call` and `__callStatic` reach
        if let Some(to) = ctx.forwarded_calls(ty).filter(|_| forwards < 4) {
            for m in members_through(ctx, &to, forwards + 1) {
                let public = m.visibility == stir_hir::Visibility::Public;
                if public
                    && matches!(m.ty, MemberTy::Method(_))
                    && seen.insert((0, m.name.to_ascii_lowercase()))
                {
                    out.push(MemberInfo {
                        forwarded: true,
                        ..m
                    });
                }
            }
        }
    }
    out
}
