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
    DocKeyword, Literal as LitRef, PhpBuiltin, ShapeKeyRef, StirBuiltin, TypeRef, TypeRefKind,
};
use stir_hir::{
    Binding, ClassDef, ClassKind, DefIndex, FileItems, FunctionDef, ItemLoc, MethodDef, ParamDef,
    PropertyDef, TypeParamDef, Variance as HirVariance, Visibility,
};
use stir_syntax::{Dialect, TextRange};
use stir_types::{
    Family, FnParam, FnTy, LitValue, ParamId, ParamKind, ShapeField, ShapeKey, Ty, TyKind, TypeEnv,
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

/// A class/interface/trait/enum/record as seen by the checker.
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

    /// Is this declared in a `.sphp` file?
    pub fn is_stir(&self) -> bool {
        self.def.dialect == Dialect::Stir
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
            ClassKind::Enum | ClassKind::Record => true,
            ClassKind::Class => {
                if self.is_stir() {
                    !(self.def.mods.is_open || self.def.mods.is_abstract || self.def.mods.is_sealed)
                } else {
                    self.def.mods.is_final
                }
            }
        }
    }

    pub fn is_external(&self) -> bool {
        self.origin != FileOrigin::Source || !self.is_stir()
    }
}

/// Direct supertypes of a class, in terms of its own type parameters.
#[derive(Debug, Clone, Default)]
pub struct Supers {
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
    pub binding: Binding,
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
    pub receiver: Option<Ty>,
    /// Declared outside the project's `.sphp` sources.
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
    pub dialect: Option<Dialect>,
}

impl LowerCx {
    pub fn dialect(&self) -> Dialect {
        self.dialect.unwrap_or(Dialect::Stir)
    }
}

pub struct Ctx<'db> {
    pub db: &'db dyn Db,
    pub ws: Workspace,
    pub index: &'db DefIndex,
    /// The dialect of the code looking at declarations: external PHP
    /// signatures are viewed in the StirPHP family only from `.sphp` code.
    pub viewer: Dialect,
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
            viewer: Dialect::Stir,
            params: RefCell::default(),
            classes: RefCell::default(),
            supers: RefCell::default(),
            fn_sigs: RefCell::default(),
            in_progress: RefCell::default(),
        }
    }

    /// A context for checking `file` (declarations are viewed from its dialect).
    pub fn for_file(db: &'db dyn Db, ws: Workspace, file: SourceFile) -> Ctx<'db> {
        let mut ctx = Ctx::new(db, ws);
        ctx.viewer = file.dialect(db);
        ctx
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
        let params: Vec<GenericParam> = defs
            .iter()
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
            .collect();
        let mut cx = outer.clone();
        cx.generics
            .extend(params.iter().map(|p| (p.name.clone(), p.ty.clone())));
        for (p, d) in params.iter().zip(defs) {
            if let Some(bound) = &d.bound {
                let b = self.lower(bound, &cx, sink);
                self.set_bound(p.id, b);
            }
        }
        params
    }

    // ---------------------------------------------------------------- classes

    pub fn class(&self, name: &str) -> Option<Rc<ClassModel<'db>>> {
        let key = SmolStr::new(name.trim_start_matches('\\').to_ascii_lowercase());
        if let Some(c) = self.classes.borrow().get(&key) {
            return c.clone();
        }
        let model = self.build_class(&key).map(Rc::new);
        self.classes.borrow_mut().insert(key, model.clone());
        model
    }

    fn build_class(&self, key: &SmolStr) -> Option<ClassModel<'db>> {
        let loc = *self.index.classes.get(key)?.first()?;
        let items = self.items(loc.file);
        let def = &items.classes[loc.index as usize];
        let scope = items.scope(def.scope).clone();
        let mut sink = Vec::new();
        let cx = LowerCx {
            dialect: Some(def.dialect),
            ..LowerCx::default()
        };
        let params = self.generic_params(key, &def.type_params, &cx, &mut sink);
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
            dialect: Some(c.def.dialect),
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
            dialect: Some(c.def.dialect),
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
        for st in &def.supertypes {
            let ty = self.lower(&st.ty, &cx, &mut sink);
            let is_interface = match ty.kind() {
                TyKind::Class { name, .. } => self
                    .class(name)
                    .is_some_and(|m| m.kind() == ClassKind::Interface),
                _ => false,
            };
            if !is_interface && s.parent.is_none() && def.kind == ClassKind::Class {
                s.parent = Some(ty);
            } else {
                s.interfaces.push(ty);
            }
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
        let err = |sink: &mut Vec<LowerError>, code, message: String| {
            sink.push(LowerError {
                range: tr.range,
                code,
                message,
            });
            Ty::error()
        };
        let stir = cx.dialect() == Dialect::Stir;
        match &tr.kind {
            TypeRefKind::Error => Ty::error(),
            TypeRefKind::Stir(b, args) => {
                let lowered: Vec<Ty> = args.iter().map(|a| self.lower(a, cx, sink)).collect();
                let want = match b {
                    StirBuiltin::List | StirBuiltin::MutableList => 1,
                    StirBuiltin::Map | StirBuiltin::MutableMap => 2,
                    _ => 0,
                };
                if lowered.len() != want {
                    return err(
                        sink,
                        codes::GENERIC_ARITY,
                        format!(
                            "`{b:?}` takes {want} type argument(s), found {}",
                            lowered.len()
                        ),
                    );
                }
                match b {
                    StirBuiltin::Int => Ty::int(),
                    StirBuiltin::Float => Ty::float(),
                    StirBuiltin::String => Ty::string(),
                    StirBuiltin::Bool => Ty::bool(),
                    StirBuiltin::Unit => Ty::unit(),
                    StirBuiltin::Nothing => Ty::nothing(),
                    StirBuiltin::Any => err(
                        sink,
                        codes::TOP_TYPE,
                        "`Any` is not allowed: StirPHP has no top type; use a generic type parameter instead".into(),
                    ),
                    StirBuiltin::List => Ty::list(lowered[0].clone()),
                    StirBuiltin::MutableList => Ty::mutable_list(lowered[0].clone()),
                    StirBuiltin::Map => Ty::map(lowered[0].clone(), lowered[1].clone()),
                    StirBuiltin::MutableMap => Ty::mutable_map(lowered[0].clone(), lowered[1].clone()),
                }
            }
            TypeRefKind::Php(b) => match b {
                PhpBuiltin::Int => Ty::php_int(),
                PhpBuiltin::Float => Ty::php_float(),
                PhpBuiltin::String => Ty::php_string(),
                PhpBuiltin::Bool => Ty::php_bool(),
                PhpBuiltin::Array => Ty::php_array(),
                PhpBuiltin::Mixed => {
                    if stir {
                        err(
                            sink,
                            codes::TOP_TYPE,
                            "`mixed` is not allowed in StirPHP files; use a generic type parameter or a union".into(),
                        )
                    } else {
                        Ty::mixed()
                    }
                }
                PhpBuiltin::Void => Ty::void(),
                PhpBuiltin::Never => Ty::never(),
                PhpBuiltin::Null => Ty::null(),
                PhpBuiltin::False => Ty::literal(
                    LitValue::Bool(false),
                    if stir { Family::Safe } else { Family::Php },
                ),
                PhpBuiltin::True => Ty::literal(
                    LitValue::Bool(true),
                    if stir { Family::Safe } else { Family::Php },
                ),
                PhpBuiltin::Object => Ty::object(),
                PhpBuiltin::Callable => Ty::callable(None),
                PhpBuiltin::Iterable => Ty::php_iterable(),
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
                self.lower_named(fqn.as_str(), args, tr.range, stir, sink)
            }
            TypeRefKind::Nullable(inner) => self.lower(inner, cx, sink).nullable(),
            TypeRefKind::Union(members) => {
                Ty::union(members.iter().map(|m| self.lower(m, cx, sink)))
            }
            TypeRefKind::Intersection(members) => {
                Ty::intersection(members.iter().map(|m| self.lower(m, cx, sink)))
            }
            TypeRefKind::Function { params, ret } => Ty::simple_func(
                params
                    .iter()
                    .map(|p| self.lower(p, cx, sink))
                    .collect::<Vec<_>>(),
                self.lower(ret, cx, sink),
            ),
            TypeRefKind::Literal(lit) => {
                let family = if stir { Family::Safe } else { Family::Php };
                let value = match lit {
                    LitRef::Int(i) => LitValue::Int(*i),
                    LitRef::Float(f) => match f.parse::<f64>() {
                        Ok(v) => LitValue::Float(stir_types::FloatLit::new(v)),
                        Err(_) => return Ty::error(),
                    },
                    LitRef::String(s) => LitValue::String(s.clone()),
                };
                Ty::literal(value, family)
            }
            TypeRefKind::PhpList { elem, non_empty } => {
                let elem = self.lower(elem, cx, sink);
                if *non_empty {
                    Ty::non_empty_list(elem)
                } else {
                    Ty::php_list(elem)
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
        let cx = LowerCx {
            dialect: Some(c.def.dialect),
            ..LowerCx::default()
        };
        let mut inner = Vec::new();
        let ty = self.lower(alias, &cx, &mut inner);
        self.in_progress.borrow_mut().remove(&guard);
        ty
    }

    /// The type of a declaration with a native type and a PHPDoc type. Like
    /// PHPStan, the PHPDoc type refines the native one when it is a subtype of
    /// it; otherwise the native type wins and the PHPDoc type is reported.
    /// Returns the type and whether it is (partly) known only from PHPDoc, i.e.
    /// whether its StirPHP view differs from the native type's.
    pub fn merged_ty(
        &self,
        native: Option<&TypeRef>,
        doc: Option<&TypeRef>,
        cx: &LowerCx,
        sink: &mut Vec<LowerError>,
    ) -> Option<(Ty, bool)> {
        let n = native.map(|t| self.lower(t, cx, sink));
        let Some(doc) = doc else {
            return n.map(|t| (t, false));
        };
        let d = self.lower(doc, cx, sink);
        match n {
            None => Some((d, true)),
            Some(n) if n == d || d.is_error() => Some((n, false)),
            Some(n) if n.is_error() || stir_types::is_assignable(self, &d, &n) => {
                // Only what PHPDoc adds on top of the native type is unchecked.
                let refined = view(&d) != view(&n);
                Some((d, refined))
            }
            Some(n) => {
                sink.push(LowerError {
                    range: doc.range,
                    code: PHPDOC_MISMATCH,
                    message: format!("PHPDoc type `{d}` is not a subtype of the native type `{n}`"),
                });
                Some((n, false))
            }
        }
    }

    fn lower_named(
        &self,
        name: &str,
        args: Vec<Ty>,
        range: TextRange,
        stir: bool,
        sink: &mut Vec<LowerError>,
    ) -> Ty {
        let key = SmolStr::new(name.to_ascii_lowercase());
        if let Some(c) = self.class(name) {
            if c.params.len() != args.len() && !(args.is_empty() && !stir) {
                if stir || !args.is_empty() {
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
                }
                return Ty::error();
            }
            if c.kind() == ClassKind::Record {
                return self.record_ty(&c, args);
            }
            return Ty::class(c.name.clone(), args);
        }
        if let Some(loc) = self
            .index
            .type_aliases
            .get(&key)
            .and_then(|v| v.first())
            .copied()
        {
            let items = self.items(loc.file);
            let alias = &items.type_aliases[loc.index as usize];
            let guard = SmolStr::new(format!("alias:{key}"));
            if !self.in_progress.borrow_mut().insert(guard.clone()) {
                sink.push(LowerError {
                    range,
                    code: codes::UNKNOWN_TYPE,
                    message: format!("type alias `{name}` refers to itself"),
                });
                return Ty::error();
            }
            let mut cx = LowerCx {
                dialect: Some(loc.file.dialect(self.db)),
                ..LowerCx::default()
            };
            cx.generics = alias
                .type_params
                .iter()
                .zip(args.iter().cloned().chain(std::iter::repeat(Ty::error())))
                .map(|(p, a)| (p.name.clone(), a))
                .collect();
            let mut inner = Vec::new();
            let ty = self.lower(&alias.ty, &cx, &mut inner);
            self.in_progress.borrow_mut().remove(&guard);
            return ty;
        }
        sink.push(LowerError {
            range,
            code: codes::UNKNOWN_TYPE,
            message: format!("unknown type `{name}`"),
        });
        Ty::error()
    }

    /// The structural type of a record declaration instantiated with `args`.
    pub fn record_ty(&self, c: &ClassModel<'db>, args: Vec<Ty>) -> Ty {
        let mut cx = self.class_cx(c);
        cx.generics = c
            .params
            .iter()
            .zip(args.iter())
            .map(|(p, a)| (p.name.clone(), a.clone()))
            .collect();
        let mut sink = Vec::new();
        let fields: Vec<(SmolStr, Ty)> = c
            .def
            .fields
            .iter()
            .map(|f| (f.name.clone(), self.lower(&f.ty, &cx, &mut sink)))
            .collect();
        Ty::named_record(c.name.clone(), args, fields)
    }

    // ---------------------------------------------------------------- external view

    /// How a PHP-declared type looks from StirPHP code: native types are
    /// trusted and map onto the StirPHP family; `mixed`/untyped is `Any!`;
    /// types known only from PHPDoc (not enforced at runtime) are platform types.
    pub fn external_view(&self, ty: &Ty, from_doc: bool) -> Ty {
        if self.viewer == Dialect::Php {
            // PHP code sees PHP declarations as they are.
            return ty.clone();
        }
        let viewed = view(ty);
        if from_doc && !stir_types::is_nullable(self, &viewed) {
            Ty::platform(viewed)
        } else {
            viewed
        }
    }

    // ---------------------------------------------------------------- signatures

    pub fn function_sig(&self, loc: ItemLoc) -> Rc<FnSig> {
        if let Some(s) = self.fn_sigs.borrow().get(&(loc.file, loc.index)) {
            return s.clone();
        }
        let items = self.items(loc.file);
        let f = &items.functions[loc.index as usize];
        let cx = LowerCx {
            dialect: Some(loc.file.dialect(self.db)),
            ..LowerCx::default()
        };
        let mut sink = Vec::new();
        let sig = Rc::new(self.build_sig(
            &format!("fn:{}", f.name.key()),
            &f.name.0,
            &f.sig,
            f.receiver.as_ref(),
            loc.file,
            &cx,
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

    #[allow(clippy::too_many_arguments)]
    pub fn build_sig(
        &self,
        owner: &str,
        name: &str,
        sig: &stir_hir::Signature,
        receiver: Option<&TypeRef>,
        file: SourceFile,
        outer: &LowerCx,
        sink: &mut Vec<LowerError>,
    ) -> FnSig {
        let type_params = self.generic_params(owner, &sig.type_params, outer, sink);
        let mut cx = outer.clone();
        cx.generics
            .extend(type_params.iter().map(|p| (p.name.clone(), p.ty.clone())));
        let external =
            self.origin(file) != FileOrigin::Source || file.dialect(self.db) == Dialect::Php;
        let params = sig
            .params
            .iter()
            .map(|p| self.param_sig(p, &cx, sink))
            .collect();
        let (ret, ret_declared) =
            match self.merged_ty(sig.ret.as_ref(), sig.doc_ret.as_ref(), &cx, sink) {
                Some((t, from_doc)) => (
                    if external {
                        self.external_view(&t, from_doc)
                    } else {
                        t
                    },
                    true,
                ),
                None if external => (self.external_view(&Ty::mixed(), false), false),
                None => (Ty::unit(), false),
            };
        FnSig {
            name: SmolStr::new(name),
            type_params,
            params,
            ret,
            ret_declared,
            receiver: receiver.map(|r| self.lower(r, &cx, sink)),
            external,
        }
    }

    pub fn param_sig(&self, p: &ParamDef, cx: &LowerCx, sink: &mut Vec<LowerError>) -> ParamSig {
        let ty = match self.merged_ty(p.ty.as_ref(), p.doc_ty.as_ref(), cx, sink) {
            Some((t, _)) => t,
            None if cx.dialect() == Dialect::Php => Ty::mixed(),
            None => Ty::error(),
        };
        ParamSig {
            name: p.name.clone(),
            ty,
            optional: p.has_default,
            variadic: p.variadic,
            by_ref: p.by_ref,
            binding: p.binding,
            range: p.range,
        }
    }

    pub fn method_sig(&self, c: &ClassModel<'db>, m: &MethodDef) -> FnSig {
        let cx = self.class_cx(c);
        let mut sink = Vec::new();
        let mut sig = self.build_sig(
            &format!("m:{}::{}", c.key, m.name.to_ascii_lowercase()),
            &m.name,
            &m.sig,
            None,
            c.file,
            &cx,
            &mut sink,
        );
        if !c.is_stir() || c.origin != FileOrigin::Source {
            sig.external = true;
        }
        sig
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
                return Some(PropertyInfo::from_def(&c, p, ty, map, self));
            }
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
                .find(|p| p.promoted.is_some() && p.name == name);
            if let Some(p) = promoted {
                let promo = p.promoted.as_ref().unwrap();
                let merged = self.merged_ty(p.ty.as_ref(), p.doc_ty.as_ref(), &cx, &mut sink);
                let (mut ty, from_doc) = merged.unwrap_or_else(|| {
                    if c.is_stir() {
                        (Ty::error(), false)
                    } else {
                        (Ty::mixed(), false)
                    }
                });
                if c.is_external() {
                    ty = self.external_view(&ty, from_doc);
                }
                return Some(PropertyInfo {
                    class: c.clone(),
                    name: p.name.clone(),
                    ty: stir_types::subst(&ty, &map),
                    visibility: promo.mods.visibility,
                    is_static: false,
                    immutable: promo.immutable || c.def.mods.is_readonly || c.def.mods.is_data,
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
                    match self.merged_ty(k.ty.as_ref(), k.doc_ty.as_ref(), &cx, &mut sink) {
                        Some((t, from_doc)) => {
                            if c.is_external() {
                                self.external_view(&t, from_doc)
                            } else {
                                t
                            }
                        }
                        None => self.external_view(&Ty::mixed(), false),
                    },
                );
            }
        }
        None
    }

    /// Extension functions named `name` (lowercase lookup).
    pub fn extensions(&self, name: &str) -> Vec<ItemLoc> {
        self.index
            .extensions
            .get(name.to_ascii_lowercase().as_str())
            .cloned()
            .unwrap_or_default()
    }

    pub fn function(&self, name: &str) -> Option<ItemLoc> {
        self.index
            .functions
            .get(name.to_ascii_lowercase().as_str())
            .and_then(|v| v.first())
            .copied()
    }

    pub fn constant(&self, name: &str) -> Option<ItemLoc> {
        self.index.consts.get(name).and_then(|v| v.first()).copied()
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
        ty: Option<(Ty, bool)>,
        map: FxHashMap<ParamId, Ty>,
        ctx: &Ctx<'db>,
    ) -> Self {
        let (mut ty, from_doc) = ty.unwrap_or_else(|| {
            if c.is_stir() {
                (Ty::error(), false)
            } else {
                (Ty::mixed(), false)
            }
        });
        if c.is_external() {
            ty = ctx.external_view(&ty, from_doc);
        }
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

/// The StirPHP-family view of a PHP type (see [`Ctx::external_view`]).
fn view(ty: &Ty) -> Ty {
    match ty.kind() {
        TyKind::PhpInt | TyKind::IntRange { .. } => Ty::int(),
        TyKind::PhpFloat => Ty::float(),
        TyKind::PhpString | TyKind::NonEmptyString | TyKind::NumericString => Ty::string(),
        TyKind::PhpBool => Ty::bool(),
        TyKind::Void => Ty::unit(),
        TyKind::Never => Ty::nothing(),
        TyKind::Mixed => Ty::any_platform(),
        TyKind::ArrayKey => Ty::union([Ty::int(), Ty::string()]),
        TyKind::Numeric => Ty::union([Ty::int(), Ty::float()]),
        TyKind::Scalar => Ty::union([Ty::int(), Ty::float(), Ty::string(), Ty::bool()]),
        TyKind::PhpList { elem, .. } => Ty::list(view(elem)),
        TyKind::Array { key, value, .. } => {
            if matches!(key.kind(), TyKind::ArrayKey) && matches!(value.kind(), TyKind::Mixed) {
                // untyped `array`
                Ty::any_platform()
            } else {
                Ty::map(view(key), view(value))
            }
        }
        TyKind::Literal(l) if l.family == Family::Php => Ty::literal(l.value.clone(), Family::Safe),
        TyKind::Union(_) => Ty::union(ty.members().iter().map(view)),
        TyKind::Callable { sig: Some(sig), .. } => Ty::func(
            sig.params
                .iter()
                .map(|p| FnParam {
                    ty: view(&p.ty),
                    kind: p.kind,
                })
                .collect(),
            view(&sig.ret),
        ),
        TyKind::Class { name, args } => Ty::class(name.clone(), args.iter().map(view).collect()),
        _ => ty.clone(),
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

    fn sealed_subclasses(&self, class: &str, args: &[Ty]) -> Option<Vec<Ty>> {
        let c = self.class(class)?;
        if !c.def.mods.is_sealed {
            return None;
        }
        let items = self.items(c.file);
        let mut out = Vec::new();
        for sub in &items.classes {
            if sub.name.key() == c.key {
                continue;
            }
            let Some(sm) = self.class(sub.name.as_str()) else {
                continue;
            };
            let s = self.supers(&sm);
            for sup in s.parent.iter().chain(&s.interfaces) {
                let TyKind::Class {
                    name,
                    args: sup_args,
                } = sup.kind()
                else {
                    continue;
                };
                if name.to_ascii_lowercase() != c.key {
                    continue;
                }
                // Map the subclass's params through the supertype's arguments.
                let mut sub_args: Vec<Ty> = sm.params.iter().map(|_| Ty::nothing()).collect();
                for (i, sa) in sup_args.iter().enumerate() {
                    if let TyKind::Param { id, .. } = sa.kind()
                        && let Some(pos) = sm.params.iter().position(|p| p.id == *id)
                        && let Some(a) = args.get(i)
                    {
                        sub_args[pos] = a.clone();
                    }
                }
                let ty = if sm.kind() == ClassKind::Record {
                    self.record_ty(&sm, sub_args)
                } else {
                    Ty::class(sm.name.clone(), sub_args)
                };
                out.push(ty);
            }
        }
        Some(out)
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
