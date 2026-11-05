//! The type representation: [`Ty`], [`TyKind`] and their building blocks.
//!
//! # Representation
//!
//! A [`Ty`] is an `Arc<TyKind>` with *structural* equality and hashing. We deliberately do
//! not use a hash-consing interner:
//!
//! * there is no global or thread-local state, no interner handle to thread through every
//!   API, and no lifetime parameter, so `Ty` is `Send + Sync + 'static` and can be stored in
//!   salsa query results or sent across tokio tasks freely;
//! * cloning is a reference-count bump, and equality has a pointer-equality fast path, so the
//!   common case (comparing a type with itself or a clone of itself) is O(1);
//! * the leaf types (`int`, `string`, `null`, ...) are cached in statics, so building them
//!   never allocates.
//!
//! The cost is that structurally equal composite types are not deduplicated in memory and a
//! full comparison is O(size). Types are small in practice; if profiling ever shows this
//! matters, the `Ty` API is opaque enough to swap in an interner later.
//!
//! # Normalisation
//!
//! All construction goes through [`Ty::new`] (or the convenience constructors, which call
//! it), which normalises the type so that equal types have equal representations:
//!
//! * unions are flattened and deduplicated; `never` members are dropped; a union containing
//!   `Error` *is* `Error`; `mixed` absorbs everything (and an untyped value absorbs even
//!   `mixed`); literals are absorbed by their base type (`'a'|string` is `string`); PHPStan
//!   refinements are absorbed by their base (`positive-int|int` is `int`); `true|false`
//!   becomes `bool`. A one-member union is that member and an empty union is `never`. Union
//!   equality ignores member order.
//! * `?T` *is* `T|null` (see [`Ty::nullable`]).
//! * intersections are flattened and deduplicated, `mixed` members are dropped, and an
//!   intersection with `never` is `never`.
//! * `int<min, max>` is `int`, `int<n, n>` is the literal `n`, and an empty range is `never`.
//! * class names lose a leading `\`; an untyped `Closure` is the class `Closure`;
//!   numeric-string array-shape keys become integer keys (as in PHP).
//!
//! Class names are compared exactly; the checker is expected to canonicalise PHP's
//! case-insensitive class names to their declared spelling before building types.

use std::hash::{Hash, Hasher};
use std::ops::Deref;
use std::sync::{Arc, LazyLock};

use rustc_hash::FxHasher;
use smol_str::SmolStr;

/// A cheap-to-clone, immutable, normalised type. See the [module docs](self).
#[derive(Clone, Eq)]
pub struct Ty(Arc<TyKind>);

impl PartialEq for Ty {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0) || *self.0 == *other.0
    }
}

impl Hash for Ty {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

/// Identity of a generic type parameter. Allocated by the checker; bounds and variance are
/// supplied through [`TypeEnv`](crate::TypeEnv).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct ParamId(pub u32);

/// A float literal value with bitwise equality (`-0.0` is normalised to `0.0`, all NaNs are
/// equal to each other).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct FloatLit(u64);

impl FloatLit {
    /// Wraps a float.
    pub fn new(value: f64) -> Self {
        let value = if value == 0.0 {
            0.0
        } else if value.is_nan() {
            f64::NAN
        } else {
            value
        };
        Self(value.to_bits())
    }

    /// The wrapped float.
    pub fn get(self) -> f64 {
        f64::from_bits(self.0)
    }
}

impl std::fmt::Debug for FloatLit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.get())
    }
}

/// The value of a literal type such as `5`, `'active'` or `true`.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum LitValue {
    Int(i64),
    Float(FloatLit),
    String(SmolStr),
    Bool(bool),
}

/// A key of an array shape.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum ShapeKey {
    Int(i64),
    Str(SmolStr),
}

impl ShapeKey {
    /// Builds a key from a string, converting canonical decimal integers to integer keys the
    /// way PHP does (`"1"` becomes `1`, but `"01"` stays a string).
    pub fn from_str_key(key: &str) -> Self {
        if let Ok(n) = key.parse::<i64>()
            && n.to_string() == key
        {
            return ShapeKey::Int(n);
        }
        ShapeKey::Str(key.into())
    }
}

impl From<&str> for ShapeKey {
    fn from(key: &str) -> Self {
        ShapeKey::from_str_key(key)
    }
}

impl From<i64> for ShapeKey {
    fn from(key: i64) -> Self {
        ShapeKey::Int(key)
    }
}

/// One entry of an array shape: `key?: ty`.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct ShapeField {
    pub key: ShapeKey,
    pub ty: Ty,
    pub optional: bool,
}

impl ShapeField {
    /// A required field.
    pub fn new(key: impl Into<ShapeKey>, ty: Ty) -> Self {
        Self {
            key: key.into(),
            ty,
            optional: false,
        }
    }

    /// An optional field (`key?: ty`).
    pub fn optional(key: impl Into<ShapeKey>, ty: Ty) -> Self {
        Self {
            key: key.into(),
            ty,
            optional: true,
        }
    }
}

/// A PHPStan array shape `array{a: int, b?: string}`. Unsealed shapes (`array{a: int, ...}`)
/// may contain additional unknown keys with `mixed` values.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct ShapeTy {
    pub fields: Vec<ShapeField>,
    pub sealed: bool,
}

impl ShapeTy {
    /// Looks a field up by key.
    pub fn field(&self, key: &ShapeKey) -> Option<&ShapeField> {
        self.fields.iter().find(|f| &f.key == key)
    }
}

/// Whether a function parameter is required, optional (has a default) or variadic.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ParamKind {
    Required,
    Optional,
    Variadic,
}

/// A parameter of a function type.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct FnParam {
    pub ty: Ty,
    pub kind: ParamKind,
}

impl FnParam {
    pub fn required(ty: Ty) -> Self {
        Self {
            ty,
            kind: ParamKind::Required,
        }
    }
    pub fn optional(ty: Ty) -> Self {
        Self {
            ty,
            kind: ParamKind::Optional,
        }
    }
    pub fn variadic(ty: Ty) -> Self {
        Self {
            ty,
            kind: ParamKind::Variadic,
        }
    }
}

/// The signature of a `callable(A, B): R` or `Closure(A, B): R`.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct FnTy {
    pub params: Vec<FnParam>,
    pub ret: Ty,
}

impl FnTy {
    pub fn new(params: Vec<FnParam>, ret: Ty) -> Self {
        Self { params, ret }
    }

    /// The parameter receiving positional argument `index`, taking a trailing variadic
    /// parameter into account.
    pub fn param_at(&self, index: usize) -> Option<&FnParam> {
        self.params
            .get(index)
            .or_else(|| self.params.last().filter(|p| p.kind == ParamKind::Variadic))
    }

    fn map(&self, f: &mut dyn FnMut(&Ty) -> Ty) -> FnTy {
        FnTy {
            params: self
                .params
                .iter()
                .map(|p| FnParam {
                    ty: f(&p.ty),
                    kind: p.kind,
                })
                .collect(),
            ret: f(&self.ret),
        }
    }
}

/// `static` or `$this`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum SelfKind {
    Static,
    This,
}

/// The members of a union or intersection. Equality and hashing ignore member order
/// (members are always deduplicated by the normalising constructors).
#[derive(Clone, Debug)]
pub struct TyList(Box<[Ty]>);

impl TyList {
    /// Wraps a member list. Prefer [`Ty::union`]/[`Ty::intersection`], which normalise.
    pub fn new(members: Vec<Ty>) -> Self {
        Self(members.into_boxed_slice())
    }
}

impl Deref for TyList {
    type Target = [Ty];
    fn deref(&self) -> &[Ty] {
        &self.0
    }
}

impl PartialEq for TyList {
    fn eq(&self, other: &Self) -> bool {
        self.0.len() == other.0.len() && self.0.iter().all(|t| other.0.contains(t))
    }
}

impl Eq for TyList {}

impl Hash for TyList {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Order-independent: sum of the members' individual hashes.
        let mut acc = 0u64;
        for t in self.0.iter() {
            let mut h = FxHasher::default();
            t.hash(&mut h);
            acc = acc.wrapping_add(h.finish());
        }
        state.write_usize(self.0.len());
        state.write_u64(acc);
    }
}

/// The shape of a type. Match on it through [`Ty::kind`]; build it through [`Ty::new`] or the
/// convenience constructors on [`Ty`].
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum TyKind {
    /// Error recovery: assignable to and from everything, never reported.
    Error,

    Int,
    Float,
    String,
    Bool,
    Void,
    Never,
    Mixed,
    /// `object`
    Object,
    Resource,
    Scalar,
    Numeric,
    ArrayKey,
    NonEmptyString,
    NumericString,
    /// `class-string` / `class-string<T>`
    ClassString(Option<Ty>),
    /// `int<min, max>`; `None` is unbounded (`min`/`max`). `positive-int` is `int<1, max>`.
    IntRange {
        min: Option<i64>,
        max: Option<i64>,
    },
    /// `array<K, V>` / `non-empty-array<K, V>`; untyped `array` is `array<array-key, mixed>`.
    Array {
        key: Ty,
        value: Ty,
        non_empty: bool,
    },
    /// `list<T>` / `non-empty-list<T>`.
    List {
        elem: Ty,
        non_empty: bool,
    },
    Shape(ShapeTy),
    /// `iterable<K, V>`; untyped `iterable` is `iterable<mixed, mixed>`.
    Iterable {
        key: Ty,
        value: Ty,
    },
    /// `callable`, `callable(A): R` or (with `closure`) `Closure(A): R`. An untyped
    /// `Closure` is normalised to the class `Closure`.
    Callable {
        sig: Option<FnTy>,
        closure: bool,
    },
    Null,
    Literal(LitValue),
    /// A nominal class, interface or enum type.
    Class {
        name: SmolStr,
        args: Vec<Ty>,
    },
    /// A generic type parameter.
    Param {
        id: ParamId,
        name: SmolStr,
    },
    /// `static` / `$this` inside class `class` (a `Class` type with the class's own params).
    SelfType {
        kind: SelfKind,
        class: Ty,
    },
    Union(TyList),
    Intersection(TyList),
    /// A value the checker has no type for (the result of an unknown call, a `foreach` over
    /// an unknown value, ...). It prints as `mixed` but, unlike a declared `mixed`, absorbs
    /// `mixed` in a union and is accepted where external PHP code takes an argument.
    Untyped,
}

macro_rules! leaf_ctors {
    ($($(#[$m:meta])* $name:ident => $kind:ident;)*) => {
        impl Ty {
            $(
                $(#[$m])*
                pub fn $name() -> Ty {
                    static CELL: LazyLock<Ty> = LazyLock::new(|| Ty::raw(TyKind::$kind));
                    CELL.clone()
                }
            )*
        }
    };
}

leaf_ctors! {
    /// The error-recovery type.
    error => Error;
    /// `int`.
    int => Int;
    /// `float`.
    float => Float;
    /// `string`.
    string => String;
    /// `bool`.
    bool => Bool;
    /// `void`.
    void => Void;
    /// `never`.
    never => Never;
    /// `null`.
    null => Null;
    /// `mixed`.
    mixed => Mixed;
    /// `object`.
    object => Object;
    /// `resource`.
    resource => Resource;
    /// `scalar`.
    scalar => Scalar;
    /// `numeric`.
    numeric => Numeric;
    /// `array-key`.
    array_key => ArrayKey;
    /// PHPStan `non-empty-string`.
    non_empty_string => NonEmptyString;
    /// PHPStan `numeric-string`.
    numeric_string => NumericString;
    /// A value without a known type.
    untyped => Untyped;
}

impl Ty {
    fn raw(kind: TyKind) -> Ty {
        Ty(Arc::new(kind))
    }

    /// Builds a type from its kind, normalising it (see the [module docs](self)).
    pub fn new(kind: TyKind) -> Ty {
        match kind {
            TyKind::Union(ms) => Ty::union(ms.0.into_vec()),
            TyKind::Intersection(ms) => Ty::intersection(ms.0.into_vec()),
            TyKind::IntRange { min, max } => Ty::int_range(min, max),
            TyKind::Class { name, args } => Ty::class(name, args),
            TyKind::Callable { sig, closure } => {
                if closure {
                    Ty::closure(sig)
                } else {
                    Ty::callable(sig)
                }
            }
            TyKind::Error => Ty::error(),
            TyKind::Int => Ty::int(),
            TyKind::Null => Ty::null(),
            TyKind::Mixed => Ty::mixed(),
            kind => Ty::raw(kind),
        }
    }

    /// The kind of this type.
    pub fn kind(&self) -> &TyKind {
        &self.0
    }

    // ----- composite constructors -----

    /// PHP `class-string` / `class-string<T>`.
    pub fn class_string(of: Option<Ty>) -> Ty {
        Ty::raw(TyKind::ClassString(of))
    }

    /// PHPStan `int<min, max>` (`None` means unbounded).
    pub fn int_range(min: Option<i64>, max: Option<i64>) -> Ty {
        match (min, max) {
            (None, None) => Ty::int(),
            (Some(a), Some(b)) if a > b => Ty::never(),
            (Some(a), Some(b)) if a == b => Ty::lit_int(a),
            _ => Ty::raw(TyKind::IntRange { min, max }),
        }
    }

    /// PHPStan `positive-int` (`int<1, max>`).
    pub fn positive_int() -> Ty {
        Ty::int_range(Some(1), None)
    }

    /// PHPStan `non-negative-int` (`int<0, max>`), the key type of lists.
    pub fn non_negative_int() -> Ty {
        Ty::int_range(Some(0), None)
    }

    /// A plain `array` (`array<array-key, mixed>`).
    pub fn plain_array() -> Ty {
        static CELL: LazyLock<Ty> = LazyLock::new(|| Ty::array(Ty::array_key(), Ty::mixed()));
        CELL.clone()
    }

    /// `array<K, V>`.
    pub fn array(key: Ty, value: Ty) -> Ty {
        Ty::raw(TyKind::Array {
            key,
            value,
            non_empty: false,
        })
    }

    /// `non-empty-array<K, V>`.
    pub fn non_empty_array(key: Ty, value: Ty) -> Ty {
        Ty::raw(TyKind::Array {
            key,
            value,
            non_empty: true,
        })
    }

    /// `list<T>`.
    pub fn list(elem: Ty) -> Ty {
        Ty::raw(TyKind::List {
            elem,
            non_empty: false,
        })
    }

    /// `non-empty-list<T>`.
    pub fn non_empty_list(elem: Ty) -> Ty {
        Ty::raw(TyKind::List {
            elem,
            non_empty: true,
        })
    }

    /// An array shape.
    pub fn shape(fields: Vec<ShapeField>, sealed: bool) -> Ty {
        Ty::raw(TyKind::Shape(ShapeTy { fields, sealed }))
    }

    /// `iterable<K, V>`.
    pub fn iterable(key: Ty, value: Ty) -> Ty {
        Ty::raw(TyKind::Iterable { key, value })
    }

    /// A plain `iterable`.
    pub fn plain_iterable() -> Ty {
        Ty::iterable(Ty::mixed(), Ty::mixed())
    }

    /// `callable` / `callable(A): R`.
    pub fn callable(sig: Option<FnTy>) -> Ty {
        Ty::raw(TyKind::Callable {
            sig,
            closure: false,
        })
    }

    /// `Closure(A): R`; `None` gives the plain class `Closure`.
    pub fn closure(sig: Option<FnTy>) -> Ty {
        match sig {
            None => Ty::class("Closure", Vec::new()),
            sig => Ty::raw(TyKind::Callable { sig, closure: true }),
        }
    }

    /// A literal type.
    pub fn literal(value: LitValue) -> Ty {
        Ty::raw(TyKind::Literal(value))
    }

    /// An int literal type.
    pub fn lit_int(value: i64) -> Ty {
        Ty::literal(LitValue::Int(value))
    }

    /// A float literal type.
    pub fn lit_float(value: f64) -> Ty {
        Ty::literal(LitValue::Float(FloatLit::new(value)))
    }

    /// A string literal type.
    pub fn lit_string(value: impl Into<SmolStr>) -> Ty {
        Ty::literal(LitValue::String(value.into()))
    }

    /// A bool literal type.
    pub fn lit_bool(value: bool) -> Ty {
        Ty::literal(LitValue::Bool(value))
    }

    /// A nominal class/interface/enum type. A leading `\` is stripped.
    pub fn class(name: impl Into<SmolStr>, args: Vec<Ty>) -> Ty {
        let name: SmolStr = name.into();
        let name = match name.strip_prefix('\\') {
            Some(stripped) => SmolStr::from(stripped),
            None => name,
        };
        Ty::raw(TyKind::Class { name, args })
    }

    /// A type parameter.
    pub fn param(id: ParamId, name: impl Into<SmolStr>) -> Ty {
        Ty::raw(TyKind::Param {
            id,
            name: name.into(),
        })
    }

    /// `static` inside `class`.
    pub fn static_of(class: Ty) -> Ty {
        Ty::raw(TyKind::SelfType {
            kind: SelfKind::Static,
            class,
        })
    }

    /// `$this` inside `class`.
    pub fn this_of(class: Ty) -> Ty {
        Ty::raw(TyKind::SelfType {
            kind: SelfKind::This,
            class,
        })
    }

    /// `?T`, i.e. `T|null`.
    pub fn nullable(self) -> Ty {
        Ty::union([self, Ty::null()])
    }

    /// A normalised union.
    pub fn union(members: impl IntoIterator<Item = Ty>) -> Ty {
        let mut out: Vec<Ty> = Vec::new();
        let mut has_mixed = false;
        let mut has_untyped = false;
        fn push(t: Ty, out: &mut Vec<Ty>, mixed: &mut bool, untyped: &mut bool) -> bool {
            match t.kind() {
                TyKind::Error => return false,
                TyKind::Never => {}
                TyKind::Mixed => *mixed = true,
                TyKind::Untyped => *untyped = true,
                TyKind::Union(ms) => {
                    for m in ms.iter() {
                        if !push(m.clone(), out, mixed, untyped) {
                            return false;
                        }
                    }
                }
                _ => {
                    if !out.contains(&t) {
                        out.push(t);
                    }
                }
            }
            true
        }
        for m in members {
            if !push(m, &mut out, &mut has_mixed, &mut has_untyped) {
                return Ty::error();
            }
        }
        if has_untyped {
            return Ty::untyped();
        }
        if has_mixed {
            return Ty::mixed();
        }

        // Absorption of literals / refinements by their base type.
        let has = |k: &TyKind| out.iter().any(|t| t.kind() == k);
        let (int, float, string, bool_) = (
            has(&TyKind::Int),
            has(&TyKind::Float),
            has(&TyKind::String),
            has(&TyKind::Bool),
        );
        out.retain(|t| match t.kind() {
            TyKind::Literal(LitValue::Int(_)) | TyKind::IntRange { .. } => !int,
            TyKind::Literal(LitValue::Float(_)) => !float,
            TyKind::Literal(LitValue::String(_))
            | TyKind::NonEmptyString
            | TyKind::NumericString
            | TyKind::ClassString(_) => !string,
            TyKind::Literal(LitValue::Bool(_)) => !bool_,
            _ => true,
        });

        // `true|false` => `bool`.
        let (t, f) = (Ty::lit_bool(true), Ty::lit_bool(false));
        if let (Some(ti), Some(fi)) = (
            out.iter().position(|m| *m == t),
            out.iter().position(|m| *m == f),
        ) {
            let first = ti.min(fi);
            let second = ti.max(fi);
            out.remove(second);
            out[first] = Ty::bool();
        }

        match out.len() {
            0 => Ty::never(),
            1 => out.pop().expect("one member"),
            _ => Ty::raw(TyKind::Union(TyList::new(out))),
        }
    }

    /// A normalised intersection.
    pub fn intersection(members: impl IntoIterator<Item = Ty>) -> Ty {
        let mut out: Vec<Ty> = Vec::new();
        fn push(t: Ty, out: &mut Vec<Ty>) -> Option<Ty> {
            match t.kind() {
                TyKind::Error => return Some(Ty::error()),
                TyKind::Never => return Some(Ty::never()),
                TyKind::Mixed => {}
                TyKind::Intersection(ms) => {
                    for m in ms.iter() {
                        if let Some(r) = push(m.clone(), out) {
                            return Some(r);
                        }
                    }
                }
                _ => {
                    if !out.contains(&t) {
                        out.push(t);
                    }
                }
            }
            None
        }
        for m in members {
            if let Some(r) = push(m, &mut out) {
                return r;
            }
        }
        match out.len() {
            0 => Ty::mixed(),
            1 => out.pop().expect("one member"),
            _ => Ty::raw(TyKind::Intersection(TyList::new(out))),
        }
    }

    // ----- queries -----

    /// The members of a union, or the type itself.
    pub fn members(&self) -> &[Ty] {
        match self.kind() {
            TyKind::Union(ms) => ms,
            _ => std::slice::from_ref(self),
        }
    }

    pub fn is_error(&self) -> bool {
        matches!(self.kind(), TyKind::Error)
    }

    /// `never`.
    pub fn is_never(&self) -> bool {
        matches!(self.kind(), TyKind::Never)
    }

    pub fn is_null(&self) -> bool {
        matches!(self.kind(), TyKind::Null)
    }

    pub fn is_union(&self) -> bool {
        matches!(self.kind(), TyKind::Union(_))
    }

    /// Whether this is a value without a known type (shallow).
    pub fn is_untyped(&self) -> bool {
        matches!(self.kind(), TyKind::Untyped)
    }

    /// Whether an error type occurs anywhere inside this type.
    pub fn contains_error(&self) -> bool {
        let mut found = false;
        self.walk(&mut |t| {
            found |= t.is_error();
            !found
        });
        found
    }

    // ----- traversal -----

    /// Calls `f` on each direct child type.
    pub fn for_each_child(&self, f: &mut dyn FnMut(&Ty)) {
        use TyKind as K;
        match self.kind() {
            K::ClassString(Some(t)) => f(t),
            K::SelfType { class, .. } => f(class),
            K::Array { key, value, .. } | K::Iterable { key, value } => {
                f(key);
                f(value);
            }
            K::List { elem, .. } => f(elem),
            K::Shape(s) => s.fields.iter().for_each(|fl| f(&fl.ty)),
            K::Callable { sig: Some(sig), .. } => {
                sig.params.iter().for_each(|p| f(&p.ty));
                f(&sig.ret);
            }
            K::Class { args, .. } => args.iter().for_each(f),
            K::Union(ms) | K::Intersection(ms) => ms.iter().for_each(f),
            _ => {}
        }
    }

    /// Pre-order walk; `f` returns whether to descend into the visited type's children.
    pub fn walk(&self, f: &mut dyn FnMut(&Ty) -> bool) {
        if f(self) {
            self.for_each_child(&mut |c| c.walk(f));
        }
    }

    /// Rebuilds this type with each direct child replaced by `f(child)` (re-normalising).
    pub fn map_children(&self, f: &mut dyn FnMut(&Ty) -> Ty) -> Ty {
        use TyKind as K;
        let kind = match self.kind() {
            K::ClassString(Some(t)) => K::ClassString(Some(f(t))),
            K::Array {
                key,
                value,
                non_empty,
            } => K::Array {
                key: f(key),
                value: f(value),
                non_empty: *non_empty,
            },
            K::List { elem, non_empty } => K::List {
                elem: f(elem),
                non_empty: *non_empty,
            },
            K::Shape(s) => K::Shape(ShapeTy {
                fields: s
                    .fields
                    .iter()
                    .map(|fl| ShapeField {
                        key: fl.key.clone(),
                        ty: f(&fl.ty),
                        optional: fl.optional,
                    })
                    .collect(),
                sealed: s.sealed,
            }),
            K::Iterable { key, value } => K::Iterable {
                key: f(key),
                value: f(value),
            },
            K::Callable {
                sig: Some(sig),
                closure,
            } => K::Callable {
                sig: Some(sig.map(f)),
                closure: *closure,
            },
            K::Class { name, args } => K::Class {
                name: name.clone(),
                args: args.iter().map(&mut *f).collect(),
            },
            K::SelfType { kind, class } => K::SelfType {
                kind: *kind,
                class: f(class),
            },
            K::Union(ms) => K::Union(TyList::new(ms.iter().map(&mut *f).collect())),
            K::Intersection(ms) => K::Intersection(TyList::new(ms.iter().map(&mut *f).collect())),
            _ => return self.clone(),
        };
        Ty::new(kind)
    }

    /// Top-down rewrite: where `f` returns `Some`, that replaces the subtree; elsewhere the
    /// children are rewritten recursively.
    pub fn transform(&self, f: &mut dyn FnMut(&Ty) -> Option<Ty>) -> Ty {
        match f(self) {
            Some(t) => t,
            None => self.map_children(&mut |c| c.transform(f)),
        }
    }
}
