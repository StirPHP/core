//! The semantic model the checker works against: class models, type lowering,
//! signatures and member lookup, plus the [`TypeEnv`] implementation.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use rustc_hash::{FxHashMap, FxHashSet};
use smol_str::SmolStr;
use stir_db::{Db, FileOrigin, SourceFile, Workspace};
use stir_hir::name::Scope;
use stir_hir::type_ref::{
    DocKeyword, Literal as LitRef, PhpBuiltin, ShapeKeyRef, TypeRef, TypeRefKind,
};
use stir_hir::{
    ClassDef, ClassKind, DefIndex, FileItems, FunctionDef, ItemLoc, MethodDef, ParamDef,
    PropertyDef, TypeParamDef, Variance as HirVariance, Visibility,
};
use stir_syntax::TextRange;
use stir_types::{
    FnParam, FnTy, LitValue, ParamId, ParamKind, ShapeField, ShapeKey, Ty, TyKind, TypeEnv,
    Variance,
};

use crate::codes;
use crate::script::magic::ScriptMagic;

/// Laravel's collection of models: one type argument is the models', not the keys'.
const ELOQUENT_COLLECTION: &str = "illuminate\\database\\eloquent\\collection";

/// A generic parameter with its id and bound.
#[derive(Debug, Clone)]
pub struct GenericParam {
    pub id: ParamId,
    pub name: SmolStr,
    pub variance: Variance,
    pub ty: Ty,
}

/// A class/interface/trait/enum as seen by the checker.
#[derive(Debug)]
pub struct ClassModel<'db> {
    pub key: SmolStr,
    pub name: SmolStr,
    pub def: &'db ClassDef,
    pub file: SourceFile,
    pub origin: FileOrigin,
    pub scope: Arc<Scope>,
    pub params: Vec<GenericParam>,
}

impl ClassModel<'_> {
    pub fn kind(&self) -> ClassKind {
        self.def.kind
    }

    /// The class type with its own type parameters as arguments.
    pub fn self_ty(&self) -> Ty {
        Ty::class(
            self.name.clone(),
            self.params.iter().map(|p| p.ty.clone()).collect(),
        )
    }

    pub fn is_final(&self) -> bool {
        match self.def.kind {
            ClassKind::Interface | ClassKind::Trait => false,
            ClassKind::Enum => true,
            ClassKind::Class => self.def.mods.is_final,
        }
    }
}

/// Direct supertypes of a class, in terms of its own type parameters.
#[derive(Debug, Clone, Default)]
pub struct Supers {
    /// A supertype names a class that doesn't exist.
    pub has_unknown: bool,
    pub parent: Option<Ty>,
    pub interfaces: Vec<Ty>,
    pub traits: Vec<Ty>,
}

/// A resolved parameter of a signature.
#[derive(Debug, Clone)]
pub struct ParamSig {
    pub name: SmolStr,
    pub ty: Ty,
    pub optional: bool,
    pub variadic: bool,
    pub by_ref: bool,
    pub range: TextRange,
    /// Has a PHPDoc type, its own or inherited: overrides inherit it.
    pub documented: bool,
    /// The type of the default value, when it's a literal.
    pub default: Option<Ty>,
    /// `@param-out`.
    pub out: Option<Ty>,
    /// `@param-closure-this`.
    pub closure_this: Option<Ty>,
}

/// A resolved function or method signature.
#[derive(Debug, Clone)]
pub struct FnSig {
    pub name: SmolStr,
    pub type_params: Vec<GenericParam>,
    pub params: Vec<ParamSig>,
    pub ret: Ty,
    /// Was a return type written (or known from the declaration)?
    pub ret_declared: bool,
    /// The return type comes from PHPDoc, its own or inherited: overrides inherit it.
    pub ret_documented: bool,
    /// A declared function, not a function value: it takes untyped values.
    pub external: bool,
    /// Parameters a conditional return type is about, with the type parameter standing in
    /// for them.
    pub conditional_on: Vec<(usize, ParamId)>,
}

impl FnSig {
    pub fn fn_ty(&self) -> FnTy {
        FnTy::new(
            self.params
                .iter()
                .map(|p| FnParam {
                    ty: p.ty.clone(),
                    kind: if p.variadic {
                        ParamKind::Variadic
                    } else if p.optional {
                        ParamKind::Optional
                    } else {
                        ParamKind::Required
                    },
                })
                .collect(),
            self.ret.clone(),
        )
    }
}

/// A type-lowering problem.
#[derive(Debug, Clone)]
pub struct LowerError {
    pub range: TextRange,
    pub code: &'static str,
    pub message: String,
    /// The class this says is unknown.
    pub symbol: Option<SmolStr>,
}

/// What a type is lowered against.
#[derive(Clone, Default)]
pub struct LowerCx {
    pub generics: Vec<(SmolStr, Ty)>,
    pub self_ty: Option<Ty>,
    pub parent_ty: Option<Ty>,
}

pub struct Ctx<'db> {
    pub db: &'db dyn Db,
    pub ws: Workspace,
    pub index: &'db DefIndex,
    /// What the project's scripts subscribe to.
    pub scripts: &'db crate::script::ScriptIndex,
    /// What went wrong with them for this model.
    pub(crate) script_failures: RefCell<Vec<crate::ScriptFailure>>,
    /// What scripts said a name is on a type: see [`Ctx::magic_answer`].
    pub(crate) magic_answers:
        RefCell<FxHashMap<(Ty, SmolStr, u8), Option<crate::script::magic::Answer>>>,
    /// What scripts said global variables hold: see [`Ctx::global_type`].
    pub(crate) globals: RefCell<FxHashMap<SmolStr, Option<Ty>>>,
    params: RefCell<ParamTable>,
    classes: RefCell<FxHashMap<SmolStr, Option<Rc<ClassModel<'db>>>>>,
    supers: RefCell<FxHashMap<SmolStr, Rc<Supers>>>,
    fn_sigs: RefCell<FxHashMap<(SourceFile, u32), Rc<FnSig>>>,
    /// By the class they're seen in, the declaring class and the lowercase method name.
    method_sigs: RefCell<FxHashMap<(SmolStr, SmolStr, SmolStr), Rc<FnSig>>>,
    ancestors: RefCell<FxHashMap<Ty, Rc<Vec<Ty>>>>,
    in_progress: RefCell<FxHashSet<SmolStr>>,
}

#[derive(Default)]
struct ParamTable {
    ids: FxHashMap<(SmolStr, SmolStr), ParamId>,
    bounds: FxHashMap<ParamId, Ty>,
    next: u32,
}

impl<'db> Ctx<'db> {
    pub fn new(db: &'db dyn Db, ws: Workspace) -> Ctx<'db> {
        Ctx {
            db,
            ws,
            index: stir_hir::def_index(db, ws),
            scripts: crate::script::script_index(db, ws),
            script_failures: RefCell::default(),
            magic_answers: RefCell::default(),
            globals: RefCell::default(),
            params: RefCell::default(),
            classes: RefCell::default(),
            supers: RefCell::default(),
            fn_sigs: RefCell::default(),
            method_sigs: RefCell::default(),
            ancestors: RefCell::default(),
            in_progress: RefCell::default(),
        }
    }

    /// What went wrong with the project's scripts for this model, leaving none.
    pub(crate) fn take_script_failures(&self) -> Vec<crate::ScriptFailure> {
        std::mem::take(&mut *self.script_failures.borrow_mut())
    }

    pub fn items(&self, file: SourceFile) -> &'db FileItems {
        stir_hir::file_items(self.db, file)
    }

    pub fn origin(&self, file: SourceFile) -> FileOrigin {
        file.origin(self.db)
    }

    // ---------------------------------------------------------------- generics

    /// A stable id for type parameter `name` of `owner`.
    pub fn param_id(&self, owner: &str, name: &str) -> ParamId {
        let mut t = self.params.borrow_mut();
        let key = (SmolStr::new(owner), SmolStr::new(name));
        if let Some(id) = t.ids.get(&key) {
            return *id;
        }
        let id = ParamId(t.next);
        t.next += 1;
        t.ids.insert(key, id);
        id
    }

    pub fn set_bound(&self, id: ParamId, bound: Ty) {
        self.params.borrow_mut().bounds.insert(id, bound);
    }

    /// Decides the conditional types in `ty`: a subject that's known to fit or not to fit picks
    /// a branch, and one that may fit, like one still unknown, leaves it at either.
    pub fn decide_conditionals(&self, ty: &Ty) -> Ty {
        ty.transform(&mut |t| {
            let TyKind::Conditional(c) = t.kind() else {
                return None;
            };
            let (subject, target) = (&c.subject, &c.target);
            let both = || {
                Ty::union([
                    self.decide_conditionals(&c.then),
                    self.decide_conditionals(&c.otherwise),
                ])
            };
            let unknown = !stir_types::free_params(subject).is_empty()
                || subject.is_error()
                || subject.is_untyped()
                || matches!(subject.kind(), TyKind::Mixed);
            // `non-empty-mixed` and the like are lowered to `mixed`, which says nothing here; and
            // an array with a key isn't an empty one, though it'd fit where one's expected
            let extra_key = matches!((subject.kind(), target.kind()), (TyKind::Shape(a), TyKind::Shape(b))
                if b.sealed && a.fields.iter().any(|f| !f.optional && b.field(&f.key).is_none()));
            Some(if unknown || matches!(target.kind(), TyKind::Mixed) {
                both()
            } else if extra_key {
                self.decide_conditionals(&c.otherwise)
            } else if stir_types::is_assignable(self, subject, target) {
                self.decide_conditionals(&c.then)
            } else if stir_types::meet(self, subject, target).is_never() {
                self.decide_conditionals(&c.otherwise)
            } else {
                both()
            })
        })
    }

    /// Lowers declared type parameters, registering their bounds.
    pub fn generic_params(
        &self,
        owner: &str,
        defs: &[TypeParamDef],
        outer: &LowerCx,
        sink: &mut Vec<LowerError>,
    ) -> Vec<GenericParam> {
        let params = self.declare_params(owner, defs);
        self.lower_bounds(&params, defs, outer, sink);
        params
    }

    fn declare_params(&self, owner: &str, defs: &[TypeParamDef]) -> Vec<GenericParam> {
        defs.iter()
            .map(|d| {
                let id = self.param_id(owner, &d.name);
                GenericParam {
                    id,
                    name: d.name.clone(),
                    variance: match d.variance {
                        HirVariance::In => Variance::In,
                        HirVariance::Out => Variance::Out,
                        HirVariance::Invariant => Variance::Invariant,
                    },
                    ty: Ty::param(id, d.name.clone()),
                }
            })
            .collect()
    }

    fn lower_bounds(
        &self,
        params: &[GenericParam],
        defs: &[TypeParamDef],
        outer: &LowerCx,
        sink: &mut Vec<LowerError>,
    ) {
        let mut cx = outer.clone();
        cx.generics
            .extend(params.iter().map(|p| (p.name.clone(), p.ty.clone())));
        for (p, d) in params.iter().zip(defs) {
            if let Some(bound) = &d.bound {
                let b = self.lower(bound, &cx, sink);
                self.set_bound(p.id, b);
            }
        }
    }

    // ---------------------------------------------------------------- classes

    pub fn class(&self, name: &str) -> Option<Rc<ClassModel<'db>>> {
        let key = SmolStr::new(name.trim_start_matches('\\').to_ascii_lowercase());
        if let Some(c) = self.classes.borrow().get(&key) {
            return c.clone();
        }
        let model = self.build_class(&key).map(Rc::new);
        self.classes.borrow_mut().insert(key, model.clone());
        // After the class is cached, because a bound can name the class itself.
        if let Some(c) = &model {
            self.lower_bounds(
                &c.params,
                &c.def.type_params,
                &LowerCx::default(),
                &mut Vec::new(),
            );
        }
        model
    }

    fn build_class(&self, key: &SmolStr) -> Option<ClassModel<'db>> {
        let loc = *self.index.classes.get(key)?.first()?;
        let items = self.items(loc.file);
        let def = &items.classes[loc.index as usize];
        let scope = items.scope(def.scope).clone();
        let params = self.declare_params(key, &def.type_params);
        Some(ClassModel {
            key: key.clone(),
            name: def.name.0.clone(),
            def,
            file: loc.file,
            origin: self.origin(loc.file),
            scope,
            params,
        })
    }

    /// The lowering context for types written inside class `c`.
    pub fn class_cx(&self, c: &ClassModel<'db>) -> LowerCx {
        let parent_ty = self.supers(c).parent.clone();
        LowerCx {
            generics: c
                .params
                .iter()
                .map(|p| (p.name.clone(), p.ty.clone()))
                .collect(),
            // `self` in a trait is whichever class uses it
            self_ty: Some(if c.kind() == ClassKind::Trait {
                Ty::static_of(c.self_ty())
            } else {
                c.self_ty()
            }),
            parent_ty,
        }
    }

    /// The project's classes using trait `key`, directly or through other traits, sorted.
    pub fn trait_users(&self, key: &str) -> Vec<SmolStr> {
        let uses = stir_hir::trait_uses(self.db, self.ws);
        let mut out = Vec::new();
        let mut seen = FxHashSet::default();
        let mut queue = vec![SmolStr::new(key)];
        while let Some(t) = queue.pop() {
            for user in uses.get(&t).into_iter().flatten() {
                if !seen.insert(user.clone()) {
                    continue;
                }
                match self.class(user) {
                    Some(c) if c.kind() == ClassKind::Trait => queue.push(user.clone()),
                    Some(_) => out.push(user.clone()),
                    None => {}
                }
            }
        }
        out.sort();
        out
    }

    pub fn supers(&self, c: &ClassModel<'db>) -> Rc<Supers> {
        if let Some(s) = self.supers.borrow().get(&c.key) {
            return s.clone();
        }
        if !self.in_progress.borrow_mut().insert(c.key.clone()) {
            return Rc::new(Supers::default()); // inheritance cycle
        }
        let cx = LowerCx {
            generics: c
                .params
                .iter()
                .map(|p| (p.name.clone(), p.ty.clone()))
                .collect(),
            self_ty: Some(c.self_ty()),
            parent_ty: None,
        };
        let mut sink = Vec::new();
        let mut s = Supers::default();
        let def = c.def;
        for (i, t) in def.extends.iter().enumerate() {
            let ty = self.lower(t, &cx, &mut sink);
            if def.kind == ClassKind::Interface || i > 0 {
                s.interfaces.push(ty);
            } else {
                s.parent = Some(ty);
            }
        }
        for t in &def.implements {
            s.interfaces.push(self.lower(t, &cx, &mut sink));
        }

        for t in &def.traits {
            s.traits.push(self.lower(t, &cx, &mut sink));
        }
        if def.kind == ClassKind::Enum {
            s.interfaces.push(Ty::class("UnitEnum", Vec::new()));
            if def.enum_backing.is_some() {
                s.interfaces.push(Ty::class("BackedEnum", Vec::new()));
            }
        }
        s.has_unknown = s
            .parent
            .iter()
            .chain(&s.interfaces)
            .chain(&s.traits)
            .any(Ty::is_error);
        let s = Rc::new(s);
        self.in_progress.borrow_mut().remove(&c.key);
        self.supers.borrow_mut().insert(c.key.clone(), s.clone());
        s
    }

    /// `ty` and all its supertypes (class types only), in the order PHP looks members up:
    /// a class, the traits it uses, then its parent likewise, and finally the interfaces.
    pub fn ancestors(&self, ty: &Ty) -> Rc<Vec<Ty>> {
        if let Some(a) = self.ancestors.borrow().get(ty) {
            return a.clone();
        }
        let mut out: Vec<Ty> = Vec::new();
        let mut interfaces = std::collections::VecDeque::new();
        let mut seen = FxHashSet::default();
        let mut parent = Some(ty.clone());
        while let Some(t) = parent.take() {
            let TyKind::Class { name, args } = t.kind() else {
                break;
            };
            if !seen.insert(name.to_ascii_lowercase()) {
                break;
            }
            out.push(t.clone());
            let Some(c) = self.class(name) else { break };
            let s = self.supers(&c);
            let map = self.subst_map(&c, args);
            self.add_traits(&s.traits, &map, &mut out, &mut seen);
            interfaces.extend(s.interfaces.iter().map(|i| stir_types::subst(i, &map)));
            parent = s.parent.as_ref().map(|p| stir_types::subst(p, &map));
        }
        while let Some(t) = interfaces.pop_front() {
            let TyKind::Class { name, args } = t.kind() else {
                continue;
            };
            if !seen.insert(name.to_ascii_lowercase()) {
                continue;
            }
            out.push(t.clone());
            if let Some(c) = self.class(name) {
                let s = self.supers(&c);
                let map = self.subst_map(&c, args);
                interfaces.extend(
                    s.parent
                        .iter()
                        .chain(&s.interfaces)
                        .map(|i| stir_types::subst(i, &map)),
                );
            }
        }
        let out = Rc::new(out);
        self.ancestors.borrow_mut().insert(ty.clone(), out.clone());
        out
    }

    /// Traits, and the traits they use, in the order PHP looks members up.
    fn add_traits(
        &self,
        traits: &[Ty],
        map: &FxHashMap<ParamId, Ty>,
        out: &mut Vec<Ty>,
        seen: &mut FxHashSet<String>,
    ) {
        for t in traits {
            let t = stir_types::subst(t, map);
            let TyKind::Class { name, args } = t.kind() else {
                continue;
            };
            if !seen.insert(name.to_ascii_lowercase()) {
                continue;
            }
            out.push(t.clone());
            if let Some(c) = self.class(name) {
                let s = self.supers(&c);
                self.add_traits(&s.traits, &self.subst_map(&c, args), out, seen);
            }
        }
    }

    /// Are `ty` and all its ancestors known?
    pub fn hierarchy_known(&self, ty: &Ty) -> bool {
        self.ancestors(ty).iter().all(|a| match a.kind() {
            TyKind::Class { name, .. } => self
                .class(name)
                .is_some_and(|c| !self.supers(&c).has_unknown),
            _ => true,
        })
    }

    /// What a class's type parameters stand for with these arguments. A type argument left
    /// out is whatever the parameter's bound allows.
    pub fn subst_map(&self, c: &ClassModel<'_>, args: &[Ty]) -> FxHashMap<ParamId, Ty> {
        c.params
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let arg = args
                    .get(i)
                    .cloned()
                    .or_else(|| self.param_bound(p.id))
                    .unwrap_or_else(Ty::untyped);
                // what a call-site variance says is about comparing, not about the value
                let arg = match arg.kind() {
                    TyKind::CovariantArg(t) | TyKind::ContravariantArg(t) => t.clone(),
                    _ => arg,
                };
                (p.id, arg)
            })
            .collect()
    }

    /// The PHPDoc type of property `name` on the closest parent that has one.
    pub fn property_doc_from_parents(&self, c: &ClassModel<'db>, name: &str) -> Option<Ty> {
        for anc in self.ancestors(&c.self_ty()).iter().skip(1) {
            let TyKind::Class { name: pn, args } = anc.kind() else {
                continue;
            };
            let Some(pc) = self.class(pn) else { continue };
            let Some(p) = pc.def.properties.iter().find(|p| p.name == name) else {
                continue;
            };
            if p.mods.visibility == Visibility::Private {
                continue;
            }
            if let Some(doc) = &p.doc_ty {
                let cx = self.class_cx(&pc);
                let t = self.lower(doc, &cx, &mut Vec::new());
                return Some(stir_types::subst(&t, &self.subst_map(&pc, args)));
            }
        }
        None
    }

    pub fn is_subclass(&self, child: &str, ancestor: &str) -> bool {
        let lower = ancestor.to_ascii_lowercase();
        self.ancestors(&Ty::class(child, Vec::new()))
            .iter()
            .any(|t| matches!(t.kind(), TyKind::Class { name, .. } if name.to_ascii_lowercase() == lower))
    }

    // ---------------------------------------------------------------- lowering

    pub fn lower(&self, tr: &TypeRef, cx: &LowerCx, sink: &mut Vec<LowerError>) -> Ty {
        match &tr.kind {
            TypeRefKind::Error => Ty::error(),
            TypeRefKind::Php(b) => match b {
                PhpBuiltin::Int => Ty::int(),
                PhpBuiltin::Float => Ty::float(),
                PhpBuiltin::String => Ty::string(),
                PhpBuiltin::Bool => Ty::bool(),
                PhpBuiltin::Array => Ty::plain_array(),
                PhpBuiltin::Mixed => Ty::mixed(),
                PhpBuiltin::Void => Ty::void(),
                PhpBuiltin::Never => Ty::never(),
                PhpBuiltin::Null => Ty::null(),
                PhpBuiltin::False => Ty::literal(LitValue::Bool(false)),
                PhpBuiltin::True => Ty::literal(LitValue::Bool(true)),
                PhpBuiltin::Object => Ty::object(),
                PhpBuiltin::Callable => Ty::callable(None),
                PhpBuiltin::Iterable => Ty::plain_iterable(),
                PhpBuiltin::SelfType => cx.self_ty.clone().unwrap_or_else(Ty::error),
                PhpBuiltin::Static => match &cx.self_ty {
                    Some(s) if matches!(s.kind(), TyKind::SelfType { .. }) => s.clone(),
                    Some(s) => Ty::static_of(s.clone()),
                    None => Ty::error(),
                },
                PhpBuiltin::Parent => cx.parent_ty.clone().unwrap_or_else(Ty::error),
            },
            TypeRefKind::Param(name) => cx
                .generics
                .iter()
                .rev()
                .find(|(n, _)| n == name)
                .map(|(_, t)| t.clone())
                .unwrap_or_else(Ty::error),
            TypeRefKind::Named(fqn, args) => {
                let args: Vec<Ty> = args.iter().map(|a| self.lower(a, cx, sink)).collect();
                self.lower_named(fqn.as_str(), args, tr.range, sink)
            }
            TypeRefKind::Conditional {
                subject,
                target,
                then,
                otherwise,
            } => {
                let subject = match subject {
                    stir_hir::type_ref::ConditionalSubjectRef::Type(t) => self.lower(t, cx, sink),
                    stir_hir::type_ref::ConditionalSubjectRef::Param(name) => {
                        let key = format!("${name}");
                        cx.generics
                            .iter()
                            .rev()
                            .find(|(n, _)| *n == key)
                            .map_or_else(Ty::error, |(_, t)| t.clone())
                    }
                };
                Ty::new(TyKind::Conditional(Box::new(stir_types::ConditionalTy {
                    subject,
                    target: self.lower(target, cx, sink),
                    then: self.lower(then, cx, sink),
                    otherwise: self.lower(otherwise, cx, sink),
                })))
            }
            TypeRefKind::SelfGeneric(builtin, args) => {
                let base = match builtin {
                    PhpBuiltin::Parent => cx.parent_ty.clone(),
                    _ => cx.self_ty.clone(),
                };
                let args: Vec<Ty> = args.iter().map(|a| self.lower(a, cx, sink)).collect();
                let apply = |t: &Ty| match t.kind() {
                    TyKind::Class { name, .. } => Ty::class(name.clone(), args.clone()),
                    _ => t.clone(),
                };
                match base.as_ref().map(Ty::kind) {
                    None => Ty::error(),
                    Some(TyKind::SelfType { class, .. }) => Ty::static_of(apply(class)),
                    Some(_) if *builtin == PhpBuiltin::Static => {
                        Ty::static_of(apply(base.as_ref().unwrap()))
                    }
                    Some(_) => apply(base.as_ref().unwrap()),
                }
            }
            TypeRefKind::CovariantArg(t) => Ty::new(TyKind::CovariantArg(self.lower(t, cx, sink))),
            TypeRefKind::ContravariantArg(t) => {
                Ty::new(TyKind::ContravariantArg(self.lower(t, cx, sink)))
            }
            TypeRefKind::Nullable(inner) => self.lower(inner, cx, sink).nullable(),
            TypeRefKind::Union(members) => {
                Ty::union(members.iter().map(|m| self.lower(m, cx, sink)))
            }
            TypeRefKind::Intersection(members) => {
                Ty::intersection(members.iter().map(|m| self.lower(m, cx, sink)))
            }
            TypeRefKind::Literal(lit) => {
                let value = match lit {
                    LitRef::Int(i) => LitValue::Int(*i),
                    LitRef::Float(f) => match f.parse::<f64>() {
                        Ok(v) => LitValue::Float(stir_types::FloatLit::new(v)),
                        Err(_) => return Ty::error(),
                    },
                    LitRef::String(s) => LitValue::String(s.clone()),
                };
                Ty::literal(value)
            }
            TypeRefKind::PhpList { elem, non_empty } => {
                let elem = self.lower(elem, cx, sink);
                if *non_empty {
                    Ty::non_empty_list(elem)
                } else {
                    Ty::list(elem)
                }
            }
            TypeRefKind::PhpArray {
                key,
                value,
                non_empty,
            } => {
                let key = match key {
                    Some(k) => self.lower(k, cx, sink),
                    None => Ty::array_key(),
                };
                let value = self.lower(value, cx, sink);
                if *non_empty {
                    Ty::non_empty_array(key, value)
                } else {
                    Ty::array(key, value)
                }
            }
            TypeRefKind::Iterable { key, value } => {
                let key = match key {
                    Some(k) => self.lower(k, cx, sink),
                    None => Ty::mixed(),
                };
                Ty::iterable(key, self.lower(value, cx, sink))
            }
            TypeRefKind::Shape { fields, sealed } => Ty::shape(
                fields
                    .iter()
                    .map(|f| {
                        let key = match &f.key {
                            ShapeKeyRef::Int(i) => ShapeKey::Int(*i),
                            ShapeKeyRef::Str(s) => ShapeKey::from_str_key(s),
                        };
                        let ty = self.lower(&f.ty, cx, sink);
                        if f.optional {
                            ShapeField::optional(key, ty)
                        } else {
                            ShapeField::new(key, ty)
                        }
                    })
                    .collect(),
                *sealed,
            ),
            TypeRefKind::IntRange { min, max } => Ty::int_range(*min, *max),
            TypeRefKind::ClassString(of) => {
                Ty::class_string(of.as_ref().map(|t| self.lower(t, cx, sink)))
            }
            TypeRefKind::Callable {
                params,
                ret,
                closure,
            } => {
                let sig = FnTy::new(
                    params
                        .iter()
                        .map(|p| FnParam {
                            ty: self.lower(&p.ty, cx, sink),
                            kind: if p.variadic {
                                ParamKind::Variadic
                            } else if p.optional {
                                ParamKind::Optional
                            } else {
                                ParamKind::Required
                            },
                        })
                        .collect(),
                    self.lower(ret, cx, sink),
                );
                if *closure {
                    Ty::closure(Some(sig))
                } else {
                    Ty::callable(Some(sig))
                }
            }
            TypeRefKind::DocKeyword(k) => match k {
                DocKeyword::ArrayKey => Ty::array_key(),
                DocKeyword::Scalar => Ty::scalar(),
                DocKeyword::Numeric => Ty::numeric(),
                DocKeyword::Resource => Ty::resource(),
                DocKeyword::NonEmptyString => Ty::non_empty_string(),
                DocKeyword::NumericString => Ty::numeric_string(),
            },
            TypeRefKind::ImportedAlias { class, name } => {
                self.imported_alias(class.as_str(), name, tr.range, sink)
            }
        }
    }

    /// `@phpstan-import-type name from class`.
    fn imported_alias(
        &self,
        class: &str,
        name: &str,
        range: TextRange,
        sink: &mut Vec<LowerError>,
    ) -> Ty {
        let Some(c) = self.class(class) else {
            sink.push(LowerError {
                range,
                code: codes::CLASS_NOT_FOUND,
                message: format!("unknown class `{class}` in `@phpstan-import-type`"),
                symbol: Some(SmolStr::new(class)),
            });
            return Ty::error();
        };
        let Some((_, alias)) = c.def.type_aliases.iter().find(|(n, _)| n == name) else {
            sink.push(LowerError {
                range,
                code: codes::TYPE_ALIAS_NOT_FOUND,
                message: format!("`{}` has no type alias `{name}`", c.name),
                symbol: None,
            });
            return Ty::error();
        };
        let guard = SmolStr::new(format!("import:{}:{name}", c.key));
        if !self.in_progress.borrow_mut().insert(guard.clone()) {
            return Ty::error();
        }
        let mut inner = Vec::new();
        let ty = self.lower(alias, &LowerCx::default(), &mut inner);
        self.in_progress.borrow_mut().remove(&guard);
        ty
    }

    /// The type of a declaration with a native type and a PHPDoc type. Like
    /// PHPStan, the PHPDoc type refines the native one when it is a subtype of
    /// it; otherwise the native type wins and the PHPDoc type is reported.
    pub fn merged_ty(
        &self,
        native: Option<&TypeRef>,
        doc: Option<&TypeRef>,
        cx: &LowerCx,
        phpdoc_code: &'static str,
        sink: &mut Vec<LowerError>,
    ) -> Option<Ty> {
        let n = native.map(|t| self.lower(t, cx, sink));
        let Some(doc) = doc else {
            return n;
        };
        let d = self.lower(doc, cx, sink);
        match n {
            None => Some(d),
            Some(n) if n == d || d.is_error() => Some(n),
            Some(n) if n.is_error() => Some(d),
            // a template's bound is checked where it's used
            Some(_) if !stir_types::free_params(&d).is_empty() => Some(d),
            // a PHPDoc type this checker can't express (`self::BOUNDARY_*`)
            Some(n) if matches!(d.kind(), TyKind::Mixed) => Some(n),
            // PHPDoc refines a native type (`string[]` for `array`)
            Some(n) if stir_types::is_assignable(self, &d, &n) => Some(d),
            Some(n) => {
                sink.push(LowerError {
                    range: doc.range,
                    code: phpdoc_code,
                    message: format!("PHPDoc type `{d}` is not a subtype of the native type `{n}`"),
                    symbol: None,
                });
                Some(n)
            }
        }
    }

    fn lower_named(
        &self,
        name: &str,
        args: Vec<Ty>,
        range: TextRange,
        sink: &mut Vec<LowerError>,
    ) -> Ty {
        let key = SmolStr::new(name.to_ascii_lowercase());
        // PHPStan's refinement keywords this checker doesn't model
        if name.contains('-') {
            if key.ends_with("-string") {
                return Ty::string();
            }
            if key.ends_with("-int") {
                return Ty::int();
            }
        }
        if let Some(c) = self.class(name) {
            let mut args = args;
            // as larastan has it, `Collection<User>` of Eloquent is `Collection<int, User>`
            if args.len() == 1 && c.params.len() == 2 && key == ELOQUENT_COLLECTION {
                args.insert(0, Ty::int());
            }
            // PHPDoc may leave out trailing type arguments: `Generator<K, V>`
            if !args.is_empty() && args.len() < c.params.len() {
                args.resize(c.params.len(), Ty::untyped());
            }
            if c.params.len() != args.len() && !args.is_empty() {
                sink.push(LowerError {
                    range,
                    code: if c.params.is_empty() {
                        codes::GENERICS_NOT_GENERIC
                    } else {
                        codes::GENERICS_MORE_TYPES
                    },
                    message: format!(
                        "`{}` takes {} type argument(s), found {}",
                        c.name,
                        c.params.len(),
                        args.len()
                    ),
                    symbol: None,
                });
                return Ty::error();
            }
            return Ty::class(c.name.clone(), args);
        }
        sink.push(LowerError {
            range,
            code: codes::CLASS_NOT_FOUND,
            message: format!("unknown type `{name}`"),
            symbol: Some(SmolStr::new(name)),
        });
        Ty::error()
    }

    // ---------------------------------------------------------------- signatures

    /// `t`, against nothing in particular: for showing.
    pub fn lower_plain(&self, t: &TypeRef) -> Ty {
        self.lower(t, &LowerCx::default(), &mut Vec::new())
    }

    pub fn function_sig(&self, loc: ItemLoc) -> Rc<FnSig> {
        if let Some(s) = self.fn_sigs.borrow().get(&(loc.file, loc.index)) {
            return s.clone();
        }
        let items = self.items(loc.file);
        let f = &items.functions[loc.index as usize];
        let mut sink = Vec::new();
        let sig = Rc::new(self.build_sig(
            &format!("fn:{}", f.name.key()),
            &f.name.0,
            &f.sig,
            &LowerCx::default(),
            &mut sink,
        ));
        self.fn_sigs
            .borrow_mut()
            .insert((loc.file, loc.index), sig.clone());
        sig
    }

    pub fn function_def(&self, loc: ItemLoc) -> &'db FunctionDef {
        &self.items(loc.file).functions[loc.index as usize]
    }

    pub fn build_sig(
        &self,
        owner: &str,
        name: &str,
        sig: &stir_hir::Signature,
        outer: &LowerCx,
        sink: &mut Vec<LowerError>,
    ) -> FnSig {
        let type_params = self.generic_params(owner, &sig.type_params, outer, sink);
        let mut cx = outer.clone();
        cx.generics
            .extend(type_params.iter().map(|p| (p.name.clone(), p.ty.clone())));
        // a parameter a conditional type is about acts as a type parameter
        let mut conditional_on = Vec::new();
        if sig
            .doc_ret
            .as_ref()
            .is_some_and(stir_hir::doc::mentions_parameter)
        {
            for (i, p) in sig.params.iter().enumerate() {
                let name = SmolStr::new(format!("${}", p.name));
                let id = self.param_id(&format!("arg:{owner}"), &name);
                cx.generics.push((name.clone(), Ty::param(id, name)));
                conditional_on.push((i, id));
            }
        }
        let params = sig
            .params
            .iter()
            .map(|p| self.param_sig(p, &cx, sink))
            .collect();
        let (ret, ret_declared) = match self.merged_ty(
            sig.ret.as_ref(),
            sig.doc_ret.as_ref(),
            &cx,
            codes::RETURN_PHPDOC_TYPE,
            sink,
        ) {
            Some(t) => (t, true),
            None => (Ty::untyped(), false),
        };
        FnSig {
            name: SmolStr::new(name),
            type_params,
            params,
            ret,
            ret_declared,
            ret_documented: sig.doc_ret.is_some(),
            external: true,
            conditional_on,
        }
    }

    pub fn param_sig(&self, p: &ParamDef, cx: &LowerCx, sink: &mut Vec<LowerError>) -> ParamSig {
        let ty = self
            .merged_ty(
                p.ty.as_ref(),
                p.doc_ty.as_ref(),
                cx,
                codes::PARAMETER_PHPDOC_TYPE,
                sink,
            )
            .unwrap_or_else(Ty::untyped);
        let ty = if p.default_is_null { ty.nullable() } else { ty };
        ParamSig {
            name: p.name.clone(),
            ty,
            optional: p.has_default,
            variadic: p.variadic,
            by_ref: p.by_ref,
            range: p.range,
            documented: p.doc_ty.is_some(),
            default: p.default_ty.as_ref().map(|t| self.lower(t, cx, sink)),
            out: p.out_ty.as_ref().map(|t| self.lower(t, cx, sink)),
            closure_this: p.closure_this.as_ref().map(|t| self.lower(t, cx, sink)),
        }
    }

    /// The signature of method `m` declared in `c`, with the PHPDoc it inherits.
    pub fn method_sig(&self, c: &ClassModel<'db>, m: &MethodDef) -> FnSig {
        self.method_sig_in(c, m, c)
    }

    /// The signature of method `m` declared in `owner`, as seen in `context`: `owner` itself,
    /// or for a trait, a class using it.
    pub fn method_sig_in(
        &self,
        owner: &ClassModel<'db>,
        m: &MethodDef,
        context: &ClassModel<'db>,
    ) -> FnSig {
        let key = (
            context.key.clone(),
            owner.key.clone(),
            SmolStr::new(m.name.to_ascii_lowercase()),
        );
        if let Some(s) = self.method_sigs.borrow().get(&key) {
            return (**s).clone();
        }
        let cx = self.member_cx(owner, context);
        let mut sink = Vec::new();
        let mut sig = self.build_sig(
            &format!("m:{}::{}", owner.key, m.name.to_ascii_lowercase()),
            &m.name,
            &m.sig,
            &cx,
            &mut sink,
        );
        // a method's magic declaration has nothing to inherit from
        if !m.magic {
            let guard = SmolStr::new(format!("doc:{}::{}", key.0, key.2));
            if self.in_progress.borrow_mut().insert(guard.clone()) {
                self.inherit_method_doc(owner, m, context, &mut sig, &cx);
                self.in_progress.borrow_mut().remove(&guard);
            }
        }
        self.method_sigs
            .borrow_mut()
            .insert(key, Rc::new(sig.clone()));
        sig
    }

    /// What types inside `owner`'s members are lowered against, when seen in `context`.
    /// Seen in `owner` itself, `self` in a trait is whichever class uses it.
    pub fn member_cx(&self, owner: &ClassModel<'db>, context: &ClassModel<'db>) -> LowerCx {
        if owner.key == context.key {
            return self.class_cx(owner);
        }
        let mut cx = self.class_cx(context);
        let args = self
            .ancestors(&context.self_ty())
            .iter()
            .find_map(|a| match a.kind() {
                TyKind::Class { name, args } if name.eq_ignore_ascii_case(&owner.name) => {
                    Some(args.clone())
                }
                _ => None,
            })
            .unwrap_or_default();
        let map = self.subst_map(owner, &args);
        cx.generics.extend(owner.params.iter().map(|p| {
            let ty = map.get(&p.id).cloned().unwrap_or_else(|| p.ty.clone());
            (p.name.clone(), ty)
        }));
        cx
    }

    /// Like PHPStan, a method's parameters and return type without a PHPDoc type of their
    /// own take it from the method it overrides: first along the parent classes, then the
    /// interfaces, then abstract methods of traits.
    fn inherit_method_doc(
        &self,
        owner: &ClassModel<'db>,
        m: &MethodDef,
        context: &ClassModel<'db>,
        sig: &mut FnSig,
        cx: &LowerCx,
    ) {
        if sig.ret_documented && sig.params.iter().all(|p| p.documented) {
            return;
        }
        let ctor = m.name.eq_ignore_ascii_case("__construct");
        let self_ty = context.self_ty();
        for anc in self.ancestors(&self_ty).iter().skip(1) {
            let TyKind::Class { name, args } = anc.kind() else {
                continue;
            };
            let Some(pc) = self.class(name) else { continue };
            if pc.key == owner.key {
                continue;
            }
            let Some(pm) = pc
                .def
                .methods
                .iter()
                .find(|x| !x.magic && x.name.eq_ignore_ascii_case(&m.name))
            else {
                continue;
            };
            let concrete_in_trait = pc.kind() == ClassKind::Trait && !pm.mods.is_abstract;
            if pm.mods.visibility == Visibility::Private
                || concrete_in_trait
                || (ctor && pc.origin == FileOrigin::Stub)
            {
                continue;
            }
            let parent = self.method_sig(&pc, pm);
            if !parent.ret_documented && !parent.params.iter().any(|p| p.documented) {
                continue;
            }
            let map = self.subst_map(&pc, args);
            let inherited = |t: &Ty| {
                let t = stir_types::subst(t, &map);
                t.transform(&mut |x| match x.kind() {
                    TyKind::SelfType { kind, class } if matches!(class.kind(), TyKind::Class { name: n, .. } if n.eq_ignore_ascii_case(&pc.name)) => {
                        Some(match kind {
                            stir_types::SelfKind::Static => Ty::static_of(self_ty.clone()),
                            stir_types::SelfKind::This => Ty::this_of(self_ty.clone()),
                        })
                    }
                    _ => None,
                })
            };
            let mut inherited_any = false;
            if !sig.ret_documented && parent.ret_documented {
                let doc = inherited(&parent.ret);
                let native = m
                    .sig
                    .ret
                    .as_ref()
                    .map(|t| self.lower(t, cx, &mut Vec::new()));
                if let Some(t) = self.refine_native(native, doc) {
                    sig.ret = t;
                    sig.ret_declared = true;
                    sig.ret_documented = true;
                    inherited_any = true;
                }
            }
            for (i, p) in sig.params.iter_mut().enumerate() {
                let Some(pp) = parent.params.get(i) else {
                    break;
                };
                if p.documented || !pp.documented || p.variadic != pp.variadic {
                    continue;
                }
                let doc = inherited(&pp.ty);
                let def = &m.sig.params[i];
                let native = def.ty.as_ref().map(|t| self.lower(t, cx, &mut Vec::new()));
                if let Some(t) = self.refine_native(native, doc) {
                    p.ty = if def.default_is_null { t.nullable() } else { t };
                    p.documented = true;
                    inherited_any = true;
                }
            }
            if inherited_any {
                let mine: Vec<ParamId> = sig.type_params.iter().map(|p| p.id).collect();
                sig.type_params.extend(
                    parent
                        .type_params
                        .iter()
                        .filter(|p| !mine.contains(&p.id))
                        .cloned(),
                );
            }
            return;
        }
    }

    /// An inherited PHPDoc type applies if it refines the native type. Otherwise the native
    /// type stands, as an override may declare a narrower one.
    fn refine_native(&self, native: Option<Ty>, doc: Ty) -> Option<Ty> {
        match native {
            None => Some(doc),
            Some(n)
                if !doc.is_error()
                    && !matches!(doc.kind(), TyKind::Mixed)
                    && (!stir_types::free_params(&doc).is_empty()
                        || stir_types::is_assignable(self, &doc, &n)) =>
            {
                Some(doc)
            }
            Some(_) => None,
        }
    }

    /// What the project's scripts declare on `c`, itself and not what it inherits.
    pub fn script_magic(&self, c: &ClassModel<'db>) -> &'db ScriptMagic {
        static NOTHING: std::sync::LazyLock<ScriptMagic> =
            std::sync::LazyLock::new(Default::default);
        if self.scripts.magic.is_empty() {
            return &NOTHING;
        }
        let name = crate::script::magic::ClassName::new(self.db, c.name.clone());
        let magic = crate::script::magic::script_magic(self.db, self.ws, name);
        if !magic.failures.is_empty() {
            let mut failures = self.script_failures.borrow_mut();
            for failure in &magic.failures {
                if !failures.contains(failure) {
                    failures.push(failure.clone());
                }
            }
        }
        magic
    }

    /// The method named `name` that `c` itself declares, in any way.
    pub fn own_method(&self, c: &ClassModel<'db>, name: &str) -> Option<&'db MethodDef> {
        let found =
            |methods: &'db [MethodDef]| methods.iter().find(|m| m.name.eq_ignore_ascii_case(name));
        found(&c.def.methods).or_else(|| found(&self.script_magic(c).methods))
    }

    /// Finds a method on `ty` (a class type), walking supertypes. Returns the
    /// declaring class, the method and the substitution from that class's params.
    pub fn find_method(
        &self,
        ty: &Ty,
        name: &str,
    ) -> Option<(Rc<ClassModel<'db>>, &'db MethodDef, FxHashMap<ParamId, Ty>)> {
        for anc in self.ancestors(ty).iter() {
            let TyKind::Class { name: cname, args } = anc.kind() else {
                continue;
            };
            let Some(c) = self.class(cname) else { continue };
            if let Some(m) = c
                .def
                .methods
                .iter()
                .find(|m| m.name.eq_ignore_ascii_case(name))
            {
                let map = self.subst_map(&c, args);
                return Some((c, m, map));
            }
            // `use T { method as alias; }`
            if let Some((_, method)) = c
                .def
                .trait_aliases
                .iter()
                .find(|(alias, _)| alias.eq_ignore_ascii_case(name))
                && !method.eq_ignore_ascii_case(name)
            {
                return self.find_method(&anc.clone(), method);
            }
        }
        // what scripts declare comes after everything PHP would run first, on any class
        if !self.scripts.magic.is_empty() {
            for anc in self.ancestors(ty).iter() {
                let TyKind::Class { name: cname, args } = anc.kind() else {
                    continue;
                };
                let Some(c) = self.class(cname) else { continue };
                let methods = &self.script_magic(&c).methods;
                if let Some(m) = methods.iter().find(|m| m.name.eq_ignore_ascii_case(name)) {
                    let map = self.subst_map(&c, args);
                    return Some((c, m, map));
                }
            }
        }
        self.on_mixins(ty, |this, mixin| {
            this.find_method(mixin, name)
                .filter(|(_, m, _)| m.mods.visibility == Visibility::Public)
        })
    }

    /// Looks through the `@mixin` types of `ty` and its ancestors.
    fn on_mixins<T>(&self, ty: &Ty, find: impl Fn(&Self, &Ty) -> Option<T>) -> Option<T> {
        for anc in self.ancestors(ty).iter() {
            let TyKind::Class { name, args } = anc.kind() else {
                continue;
            };
            let Some(c) = self.class(name) else { continue };
            if c.def.mixins.is_empty() {
                continue;
            }
            let guard = SmolStr::new(format!("mixin:{}", c.key));
            if !self.in_progress.borrow_mut().insert(guard.clone()) {
                continue;
            }
            let cx = self.class_cx(&c);
            let map = self.subst_map(&c, args);
            let mut found = None;
            for mixin in &c.def.mixins {
                let t = stir_types::subst(&self.lower(mixin, &cx, &mut Vec::new()), &map);
                // `@mixin T`: what's looked up on it is on its bound
                let t = match t.kind() {
                    TyKind::Param { id, .. } => self.param_bound(*id).unwrap_or(t),
                    _ => t,
                };
                found = find(self, &t);
                if found.is_some() {
                    break;
                }
            }
            self.in_progress.borrow_mut().remove(&guard);
            if found.is_some() {
                return found;
            }
        }
        None
    }

    /// Finds a property (declared or constructor-promoted) on `ty`.
    pub fn find_property(&self, ty: &Ty, name: &str) -> Option<PropertyInfo<'db>> {
        for anc in self.ancestors(ty).iter() {
            let TyKind::Class { name: cname, args } = anc.kind() else {
                continue;
            };
            let Some(c) = self.class(cname) else { continue };
            let map = self.subst_map(&c, args);
            let cx = self.class_cx(&c);
            let mut sink = Vec::new();
            if let Some(p) = c.def.properties.iter().find(|p| p.name == name) {
                let mut ty = self.merged_ty(
                    p.ty.as_ref(),
                    p.doc_ty.as_ref(),
                    &cx,
                    codes::PROPERTY_PHPDOC_TYPE,
                    &mut sink,
                );
                // redeclared without a PHPDoc type: it takes the one it overrides
                if p.doc_ty.is_none()
                    && !p.magic
                    && let Some(doc) = self.property_doc_from_parents(&c, name)
                {
                    let native = p.ty.as_ref().map(|t| self.lower(t, &cx, &mut sink));
                    if let Some(t) = self.refine_native(native, doc) {
                        ty = Some(t);
                    }
                }
                return Some(PropertyInfo::from_def(&c, p, ty, map));
            }
            let promoted = c
                .def
                .methods
                .iter()
                .filter(|m| m.name.eq_ignore_ascii_case("__construct"))
                .flat_map(|m| m.sig.params.iter())
                .find(|p| p.promoted.is_some() && p.name == name);
            if let Some(p) = promoted {
                let promo = p.promoted.as_ref().unwrap();
                let ty = self
                    .merged_ty(
                        p.ty.as_ref(),
                        p.doc_ty.as_ref(),
                        &cx,
                        codes::PROPERTY_PHPDOC_TYPE,
                        &mut sink,
                    )
                    .unwrap_or_else(Ty::mixed);
                return Some(PropertyInfo {
                    class: c.clone(),
                    name: p.name.clone(),
                    ty: stir_types::subst(&ty, &map),
                    visibility: promo.mods.visibility,
                    is_static: false,
                    immutable: promo.immutable || c.def.mods.is_readonly,
                    magic: false,
                });
            }
        }
        if !self.scripts.magic.is_empty() {
            for anc in self.ancestors(ty).iter() {
                let TyKind::Class { name: cname, args } = anc.kind() else {
                    continue;
                };
                let Some(c) = self.class(cname) else { continue };
                let properties = &self.script_magic(&c).properties;
                if let Some(p) = properties.iter().find(|p| p.name == name) {
                    let map = self.subst_map(&c, args);
                    let cx = self.class_cx(&c);
                    let ty = p
                        .doc_ty
                        .as_ref()
                        .map(|t| self.lower(t, &cx, &mut Vec::new()));
                    return Some(PropertyInfo::from_def(&c, p, ty, map));
                }
            }
        }
        self.on_mixins(ty, |this, mixin| {
            this.find_property(mixin, name)
                .filter(|p| p.visibility == Visibility::Public)
        })
    }

    /// Finds a class constant or enum case on class `name`.
    pub fn find_const(&self, ty: &Ty, name: &str) -> Option<Ty> {
        for anc in self.ancestors(ty).iter() {
            let TyKind::Class { name: cname, .. } = anc.kind() else {
                continue;
            };
            let Some(c) = self.class(cname) else { continue };
            if c.def.cases.iter().any(|k| k.name == name) {
                return Some(Ty::class(c.name.clone(), Vec::new()));
            }
            if let Some(k) = c.def.consts.iter().find(|k| k.name == name) {
                let cx = self.class_cx(&c);
                let mut sink = Vec::new();
                return Some(
                    self.merged_ty(
                        k.ty.as_ref(),
                        k.doc_ty.as_ref(),
                        &cx,
                        codes::CLASS_CONSTANT_PHPDOC_TYPE,
                        &mut sink,
                    )
                    .unwrap_or_else(Ty::mixed),
                );
            }
        }
        None
    }

    /// The class declaring constant or enum case `name` visible on `ty`, and
    /// whether it is an enum case.
    pub fn const_owner(&self, ty: &Ty, name: &str) -> Option<(Rc<ClassModel<'db>>, bool)> {
        for anc in self.ancestors(ty).iter() {
            let TyKind::Class { name: cname, .. } = anc.kind() else {
                continue;
            };
            let Some(c) = self.class(cname) else { continue };
            if c.def.cases.iter().any(|k| k.name == name) {
                return Some((c, true));
            }
            if c.def.consts.iter().any(|k| k.name == name) {
                return Some((c, false));
            }
        }
        None
    }

    /// A member that `ty` or an ancestor has in the stubs on some versions of PHP, with those
    /// versions.
    pub fn limited_member(
        &self,
        ty: &Ty,
        kind: stir_hir::LimitedMemberKind,
        name: &str,
    ) -> Option<stir_db::Availability> {
        self.ancestors(ty).iter().find_map(|anc| {
            let TyKind::Class { name: cname, .. } = anc.kind() else {
                return None;
            };
            let c = self.class(cname)?;
            let m = c.def.limited_members.iter().find(|m| {
                m.kind == kind
                    && match kind {
                        stir_hir::LimitedMemberKind::Method => m.name.eq_ignore_ascii_case(name),
                        _ => m.name == name,
                    }
            })?;
            Some(m.availability)
        })
    }

    pub fn function(&self, name: &str) -> Option<ItemLoc> {
        self.index
            .functions
            .get(name.to_ascii_lowercase().as_str())
            .and_then(|v| v.first())
            .copied()
    }

    pub fn function_overloads(&self, name: &str) -> Vec<ItemLoc> {
        self.index
            .functions
            .get(name.to_ascii_lowercase().as_str())
            .cloned()
            .unwrap_or_default()
    }

    /// A stub wins over a project's `define()` fallback for an extension's constant.
    pub fn constant(&self, name: &str) -> Option<ItemLoc> {
        let locs = self.index.consts.get(name)?;
        locs.iter()
            .find(|l| l.file.origin(self.db) == FileOrigin::Stub)
            .or_else(|| locs.first())
            .copied()
    }
}

/// A resolved property.
#[derive(Debug, Clone)]
pub struct PropertyInfo<'db> {
    pub class: Rc<ClassModel<'db>>,
    pub name: SmolStr,
    pub ty: Ty,
    pub visibility: Visibility,
    pub is_static: bool,
    pub immutable: bool,
    /// From an `@property` tag, not a declaration.
    pub magic: bool,
}

impl<'db> PropertyInfo<'db> {
    fn from_def(
        c: &Rc<ClassModel<'db>>,
        p: &PropertyDef,
        ty: Option<Ty>,
        map: FxHashMap<ParamId, Ty>,
    ) -> Self {
        let ty = ty.unwrap_or_else(Ty::mixed);
        PropertyInfo {
            class: c.clone(),
            name: p.name.clone(),
            ty: stir_types::subst(&ty, &map),
            visibility: p.mods.visibility,
            is_static: p.mods.is_static,
            immutable: p.immutable || p.mods.is_readonly || c.def.mods.is_readonly,
            magic: p.magic,
        }
    }
}

impl TypeEnv for Ctx<'_> {
    fn supertypes(&self, class: &str, args: &[Ty]) -> Vec<Ty> {
        let Some(c) = self.class(class) else {
            return Vec::new();
        };
        let s = self.supers(&c);
        let map = self.subst_map(&c, args);
        s.parent
            .iter()
            .chain(&s.interfaces)
            .map(|t| stir_types::subst(t, &map))
            .collect()
    }

    fn variance(&self, class: &str, index: usize) -> Variance {
        self.class(class)
            .and_then(|c| c.params.get(index).map(|p| p.variance))
            .unwrap_or(Variance::Invariant)
    }

    fn param_bound(&self, param: ParamId) -> Option<Ty> {
        self.params.borrow().bounds.get(&param).cloned()
    }

    fn is_final(&self, class: &str) -> bool {
        self.class(class).is_some_and(|c| c.is_final())
    }

    fn is_interface(&self, class: &str) -> bool {
        self.class(class)
            .is_some_and(|c| c.kind() == ClassKind::Interface)
    }

    fn is_class(&self, class: &str) -> bool {
        self.class(class).is_some()
    }

    fn invoke_signature(&self, class: &str, args: &[Ty]) -> Option<FnTy> {
        let ty = Ty::class(class, args.to_vec());
        let (c, m, map) = self.find_method(&ty, "__invoke")?;
        let sig = self.method_sig(&c, m);
        let f = sig.fn_ty();
        Some(FnTy::new(
            f.params
                .into_iter()
                .map(|p| FnParam {
                    ty: stir_types::subst(&p.ty, &map),
                    kind: p.kind,
                })
                .collect(),
            stir_types::subst(&f.ret, &map),
        ))
    }
}
