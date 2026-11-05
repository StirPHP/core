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
use crate::levels::PHPDOC_MISMATCH;

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
    /// A declared function, not a function value: it takes untyped values.
    pub external: bool,
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
    params: RefCell<ParamTable>,
    classes: RefCell<FxHashMap<SmolStr, Option<Rc<ClassModel<'db>>>>>,
    supers: RefCell<FxHashMap<SmolStr, Rc<Supers>>>,
    fn_sigs: RefCell<FxHashMap<(SourceFile, u32), Rc<FnSig>>>,
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
            params: RefCell::default(),
            classes: RefCell::default(),
            supers: RefCell::default(),
            fn_sigs: RefCell::default(),
            in_progress: RefCell::default(),
        }
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
            self_ty: Some(c.self_ty()),
            parent_ty,
        }
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

    /// `ty` and all its supertypes (class types only), breadth-first.
    pub fn ancestors(&self, ty: &Ty) -> Vec<Ty> {
        let mut out: Vec<Ty> = Vec::new();
        let mut queue = vec![ty.clone()];
        let mut seen = FxHashSet::default();
        while let Some(t) = queue.first().cloned() {
            queue.remove(0);
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
                for sup in s.parent.iter().chain(&s.interfaces).chain(&s.traits) {
                    queue.push(stir_types::subst(sup, &map));
                }
            }
            if out.len() > 64 {
                break;
            }
        }
        out
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

    pub fn subst_map(&self, c: &ClassModel<'_>, args: &[Ty]) -> FxHashMap<ParamId, Ty> {
        c.params
            .iter()
            .zip(args.iter())
            .map(|(p, a)| (p.id, a.clone()))
            .collect()
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
                code: codes::UNKNOWN_TYPE,
                message: format!("unknown class `{class}` in `@phpstan-import-type`"),
            });
            return Ty::error();
        };
        let Some((_, alias)) = c.def.type_aliases.iter().find(|(n, _)| n == name) else {
            sink.push(LowerError {
                range,
                code: codes::UNKNOWN_TYPE,
                message: format!("`{}` has no type alias `{name}`", c.name),
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
                    code: PHPDOC_MISMATCH,
                    message: format!("PHPDoc type `{d}` is not a subtype of the native type `{n}`"),
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
            // PHPDoc may leave out trailing type arguments: `Generator<K, V>`
            let mut args = args;
            if !args.is_empty() && args.len() < c.params.len() {
                args.resize(c.params.len(), Ty::untyped());
            }
            if c.params.len() != args.len() && !args.is_empty() {
                sink.push(LowerError {
                    range,
                    code: codes::GENERIC_ARITY,
                    message: format!(
                        "`{}` takes {} type argument(s), found {}",
                        c.name,
                        c.params.len(),
                        args.len()
                    ),
                });
                return Ty::error();
            }
            return Ty::class(c.name.clone(), args);
        }
        sink.push(LowerError {
            range,
            code: codes::UNKNOWN_TYPE,
            message: format!("unknown type `{name}`"),
        });
        Ty::error()
    }

    // ---------------------------------------------------------------- signatures

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
        let params = sig
            .params
            .iter()
            .map(|p| self.param_sig(p, &cx, sink))
            .collect();
        let (ret, ret_declared) =
            match self.merged_ty(sig.ret.as_ref(), sig.doc_ret.as_ref(), &cx, sink) {
                Some(t) => (t, true),
                None => (Ty::mixed(), false),
            };
        FnSig {
            name: SmolStr::new(name),
            type_params,
            params,
            ret,
            ret_declared,
            external: true,
        }
    }

    pub fn param_sig(&self, p: &ParamDef, cx: &LowerCx, sink: &mut Vec<LowerError>) -> ParamSig {
        let ty = self
            .merged_ty(p.ty.as_ref(), p.doc_ty.as_ref(), cx, sink)
            .unwrap_or_else(Ty::mixed);
        let ty = if p.default_is_null { ty.nullable() } else { ty };
        ParamSig {
            name: p.name.clone(),
            ty,
            optional: p.has_default,
            variadic: p.variadic,
            by_ref: p.by_ref,
            range: p.range,
        }
    }

    pub fn method_sig(&self, c: &ClassModel<'db>, m: &MethodDef) -> FnSig {
        let cx = self.class_cx(c);
        let mut sink = Vec::new();
        self.build_sig(
            &format!("m:{}::{}", c.key, m.name.to_ascii_lowercase()),
            &m.name,
            &m.sig,
            &cx,
            &mut sink,
        )
    }

    /// Finds a method on `ty` (a class type), walking supertypes. Returns the
    /// declaring class, the method and the substitution from that class's params.
    pub fn find_method(
        &self,
        ty: &Ty,
        name: &str,
    ) -> Option<(Rc<ClassModel<'db>>, &'db MethodDef, FxHashMap<ParamId, Ty>)> {
        for anc in self.ancestors(ty) {
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
        }
        None
    }

    /// Finds a property (declared or constructor-promoted) on `ty`.
    pub fn find_property(&self, ty: &Ty, name: &str) -> Option<PropertyInfo<'db>> {
        for anc in self.ancestors(ty) {
            let TyKind::Class { name: cname, args } = anc.kind() else {
                continue;
            };
            let Some(c) = self.class(cname) else { continue };
            let map = self.subst_map(&c, args);
            let cx = self.class_cx(&c);
            let mut sink = Vec::new();
            if let Some(p) = c.def.properties.iter().find(|p| p.name == name) {
                let ty = self.merged_ty(p.ty.as_ref(), p.doc_ty.as_ref(), &cx, &mut sink);
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
                    .merged_ty(p.ty.as_ref(), p.doc_ty.as_ref(), &cx, &mut sink)
                    .unwrap_or_else(Ty::mixed);
                return Some(PropertyInfo {
                    class: c.clone(),
                    name: p.name.clone(),
                    ty: stir_types::subst(&ty, &map),
                    visibility: promo.mods.visibility,
                    is_static: false,
                    immutable: promo.immutable || c.def.mods.is_readonly,
                });
            }
        }
        None
    }

    /// Finds a class constant or enum case on class `name`.
    pub fn find_const(&self, ty: &Ty, name: &str) -> Option<Ty> {
        for anc in self.ancestors(ty) {
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
                    self.merged_ty(k.ty.as_ref(), k.doc_ty.as_ref(), &cx, &mut sink)
                        .unwrap_or_else(Ty::mixed),
                );
            }
        }
        None
    }

    /// The class declaring constant or enum case `name` visible on `ty`, and
    /// whether it is an enum case.
    pub fn const_owner(&self, ty: &Ty, name: &str) -> Option<(Rc<ClassModel<'db>>, bool)> {
        for anc in self.ancestors(ty) {
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
