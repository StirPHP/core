//! Shared test helpers: a mock `TypeEnv` and short type constructors.
#![allow(dead_code)]

use stir_types::{
    FnTy, FxHashMap, LitValue, ParamId, ShapeField, Ty, TypeEnv, Variance, check_assignable, subst,
};

#[derive(Default)]
pub struct ClassDecl {
    pub params: Vec<(ParamId, Variance)>,
    pub supers: Vec<Ty>,
    pub is_final: bool,
    pub is_interface: bool,
    pub sealed: Option<Vec<Ty>>,
    pub invoke: Option<FnTy>,
}

#[derive(Default)]
pub struct MockEnv {
    pub classes: FxHashMap<String, ClassDecl>,
    pub bounds: FxHashMap<ParamId, Ty>,
}

impl MockEnv {
    fn bind(&self, class: &str, args: &[Ty]) -> FxHashMap<ParamId, Ty> {
        let Some(decl) = self.classes.get(class) else {
            return FxHashMap::default();
        };
        decl.params
            .iter()
            .map(|(id, _)| *id)
            .zip(args.iter().cloned())
            .collect()
    }
}

impl TypeEnv for MockEnv {
    fn supertypes(&self, class: &str, args: &[Ty]) -> Vec<Ty> {
        let map = self.bind(class, args);
        self.classes
            .get(class)
            .map(|d| d.supers.iter().map(|s| subst(s, &map)).collect())
            .unwrap_or_default()
    }
    fn variance(&self, class: &str, index: usize) -> Variance {
        self.classes
            .get(class)
            .and_then(|d| d.params.get(index))
            .map_or(Variance::Invariant, |(_, v)| *v)
    }
    fn param_bound(&self, param: ParamId) -> Option<Ty> {
        self.bounds.get(&param).cloned()
    }
    fn is_final(&self, class: &str) -> bool {
        self.classes.get(class).is_some_and(|d| d.is_final)
    }
    fn is_interface(&self, class: &str) -> bool {
        self.classes.get(class).is_some_and(|d| d.is_interface)
    }
    fn sealed_subclasses(&self, class: &str, args: &[Ty]) -> Option<Vec<Ty>> {
        let map = self.bind(class, args);
        let subs = self.classes.get(class)?.sealed.as_ref()?;
        Some(subs.iter().map(|s| subst(s, &map)).collect())
    }
    fn invoke_signature(&self, class: &str, _args: &[Ty]) -> Option<FnTy> {
        self.classes.get(class)?.invoke.clone()
    }
}

// Type parameters used by the fixtures.
pub const T_ID: ParamId = ParamId(0); // unbounded
pub const U_ID: ParamId = ParamId(1); // U of Pet
pub const N_ID: ParamId = ParamId(2); // N of Int?
pub const BOX_T: ParamId = ParamId(10);
pub const SINK_T: ParamId = ParamId(11);
pub const CELL_T: ParamId = ParamId(12);
pub const COLL_T: ParamId = ParamId(13);
pub const TRAV_K: ParamId = ParamId(14);
pub const TRAV_V: ParamId = ParamId(15);
pub const RES_T: ParamId = ParamId(16);

pub fn tp() -> Ty {
    Ty::param(T_ID, "T")
}
pub fn up() -> Ty {
    Ty::param(U_ID, "U")
}
pub fn np() -> Ty {
    Ty::param(N_ID, "N")
}

pub fn cls(name: &str) -> Ty {
    Ty::class(name, vec![])
}
pub fn cls_args(name: &str, args: Vec<Ty>) -> Ty {
    Ty::class(name, args)
}

/// Hierarchy:
/// - `interface Animal`, `interface Named`
/// - `open class Pet : Animal`, `final class Dog : Pet, Named`, `final class Cat : Pet`
/// - `open class Robot` (unrelated)
/// - `sealed interface Shape` permits `final Circle`, `sealed Poly` permits `final Square`,
///   `final Triangle`
/// - `class Box<out T>`, `class Sink<in T>`, `class Cell<T>`, `final class IntBox : Box<Int>`
/// - `interface Traversable<K, V>`, `class Coll<T> : Traversable<int, T>`
/// - `final class Doubler` with `__invoke(int): int`
/// - `sealed class Res<out T>` permits `Ok<T>`, `Err`
pub fn env() -> MockEnv {
    let mut e = MockEnv::default();
    let mut add = |name: &str, decl: ClassDecl| {
        e.classes.insert(name.to_string(), decl);
    };
    add(
        "Animal",
        ClassDecl {
            is_interface: true,
            ..Default::default()
        },
    );
    add(
        "Named",
        ClassDecl {
            is_interface: true,
            ..Default::default()
        },
    );
    add(
        "Pet",
        ClassDecl {
            supers: vec![cls("Animal")],
            ..Default::default()
        },
    );
    add(
        "Dog",
        ClassDecl {
            supers: vec![cls("Pet"), cls("Named")],
            is_final: true,
            ..Default::default()
        },
    );
    add(
        "Cat",
        ClassDecl {
            supers: vec![cls("Pet")],
            is_final: true,
            ..Default::default()
        },
    );
    add("Robot", ClassDecl::default());
    add(
        "Shape",
        ClassDecl {
            is_interface: true,
            sealed: Some(vec![cls("Circle"), cls("Poly")]),
            ..Default::default()
        },
    );
    add(
        "Circle",
        ClassDecl {
            supers: vec![cls("Shape")],
            is_final: true,
            ..Default::default()
        },
    );
    add(
        "Poly",
        ClassDecl {
            supers: vec![cls("Shape")],
            sealed: Some(vec![cls("Square"), cls("Triangle")]),
            ..Default::default()
        },
    );
    add(
        "Square",
        ClassDecl {
            supers: vec![cls("Poly")],
            is_final: true,
            ..Default::default()
        },
    );
    add(
        "Triangle",
        ClassDecl {
            supers: vec![cls("Poly")],
            is_final: true,
            ..Default::default()
        },
    );
    add(
        "Box",
        ClassDecl {
            params: vec![(BOX_T, Variance::Out)],
            ..Default::default()
        },
    );
    add(
        "Sink",
        ClassDecl {
            params: vec![(SINK_T, Variance::In)],
            ..Default::default()
        },
    );
    add(
        "Cell",
        ClassDecl {
            params: vec![(CELL_T, Variance::Invariant)],
            ..Default::default()
        },
    );
    add(
        "IntBox",
        ClassDecl {
            supers: vec![cls_args("Box", vec![Ty::int()])],
            is_final: true,
            ..Default::default()
        },
    );
    add(
        "Traversable",
        ClassDecl {
            params: vec![(TRAV_K, Variance::Out), (TRAV_V, Variance::Out)],
            is_interface: true,
            ..Default::default()
        },
    );
    add(
        "Coll",
        ClassDecl {
            params: vec![(COLL_T, Variance::Invariant)],
            supers: vec![cls_args(
                "Traversable",
                vec![Ty::php_int(), Ty::param(COLL_T, "T")],
            )],
            ..Default::default()
        },
    );
    add(
        "Doubler",
        ClassDecl {
            is_final: true,
            invoke: Some(FnTy::new(
                vec![stir_types::FnParam::required(Ty::php_int())],
                Ty::php_int(),
            )),
            ..Default::default()
        },
    );
    let res_t = Ty::param(RES_T, "T");
    add(
        "Res",
        ClassDecl {
            params: vec![(RES_T, Variance::Out)],
            sealed: Some(vec![cls_args("Ok", vec![res_t.clone()]), cls("Err")]),
            ..Default::default()
        },
    );
    add(
        "Ok",
        ClassDecl {
            params: vec![(ParamId(17), Variance::Out)],
            supers: vec![cls_args("Res", vec![Ty::param(ParamId(17), "T")])],
            is_final: true,
            ..Default::default()
        },
    );
    add(
        "Err",
        ClassDecl {
            supers: vec![cls_args("Res", vec![Ty::nothing()])],
            is_final: true,
            ..Default::default()
        },
    );
    e.bounds.insert(U_ID, cls("Pet"));
    e.bounds.insert(N_ID, Ty::int().nullable());
    e
}

// ----- short constructors -----

pub fn s(v: &str) -> Ty {
    Ty::lit_string(v)
}
pub fn ps(v: &str) -> Ty {
    Ty::php_lit(LitValue::String(v.into()))
}
pub fn pi(v: i64) -> Ty {
    Ty::php_lit(LitValue::Int(v))
}
pub fn union(ms: Vec<Ty>) -> Ty {
    Ty::union(ms)
}
pub fn rec(fields: &[(&str, Ty)]) -> Ty {
    Ty::anon_record(fields.iter().map(|(n, t)| (*n, t.clone())))
}
pub fn shape(fields: &[(&str, Ty)], sealed: bool) -> Ty {
    Ty::shape(
        fields
            .iter()
            .map(|(k, t)| ShapeField::new(*k, t.clone()))
            .collect(),
        sealed,
    )
}
pub fn fun(params: Vec<Ty>, ret: Ty) -> Ty {
    Ty::simple_func(params, ret)
}

/// Runs a table of `(from, to, expected)` assignability cases and reports every failure.
#[track_caller]
pub fn assert_assign_table(env: &dyn TypeEnv, cases: &[(Ty, Ty, bool)]) {
    let mut failures = Vec::new();
    for (from, to, expected) in cases {
        let got = check_assignable(env, from, to);
        if got.is_ok() != *expected {
            failures.push(format!(
                "  {from} -> {to}: expected {}, got {}",
                if *expected {
                    "assignable"
                } else {
                    "NOT assignable"
                },
                match &got {
                    Ok(()) => "assignable".to_string(),
                    Err(e) => format!("not assignable ({e})"),
                }
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "assignability failures:\n{}",
        failures.join("\n")
    );
}
