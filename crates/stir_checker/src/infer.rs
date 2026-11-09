//! Generic type-argument inference by structural unification.

use rustc_hash::{FxHashMap, FxHashSet};
use stir_types::{LitValue, ParamId, Ty, TyKind, TypeEnv};

/// Inference variables of one call and their (partial) solutions.
#[derive(Debug, Default, Clone)]
pub struct Inference {
    pub vars: FxHashSet<ParamId>,
    pub solved: FxHashMap<ParamId, Ty>,
    /// Bind literal types as they are (when unifying with an expected type).
    exact: bool,
}

impl Inference {
    pub fn new(vars: impl IntoIterator<Item = ParamId>) -> Self {
        Inference {
            vars: vars.into_iter().collect(),
            solved: FxHashMap::default(),
            exact: false,
        }
    }

    /// Substitutes the variables solved so far.
    pub fn apply(&self, ty: &Ty) -> Ty {
        stir_types::subst(ty, &self.solved)
    }

    /// Does `ty` still mention unsolved variables?
    pub fn has_unsolved(&self, ty: &Ty) -> bool {
        stir_types::free_params(ty)
            .iter()
            .any(|p| self.vars.contains(p) && !self.solved.contains_key(p))
    }

    /// Replaces any still-unsolved variable with what `fallback` makes of it.
    pub fn finish_with(&mut self, fallback: impl Fn(ParamId) -> Ty) {
        for v in self.vars.clone() {
            self.solved.entry(v).or_insert_with(|| fallback(v));
        }
    }

    fn bind(&mut self, env: &dyn TypeEnv, id: ParamId, ty: &Ty) {
        if ty.is_error() {
            return;
        }
        // a value outside the bound doesn't solve the template: it stays at its bound,
        // and the argument gets reported against that
        if let Some(bound) = env.param_bound(id)
            && stir_types::free_params(&bound).is_empty()
            && !stir_types::is_assignable(env, ty, &bound)
            && !ty.is_untyped()
        {
            return;
        }
        // a literal argument fits a solution from the expected type as it is
        if let Some(old) = self.solved.get(&id)
            && stir_types::is_assignable(env, ty, old)
        {
            return;
        }
        // a literal argument gives its base type, but a declared literal union is kept
        let ty = if self.exact
            || !matches!(ty.kind(), TyKind::Literal(_))
            || matches!(ty.kind(), TyKind::Literal(LitValue::Bool(_)))
        {
            ty.clone()
        } else {
            widen_literals(ty)
        };
        let new = match self.solved.get(&id) {
            Some(old) if stir_types::is_assignable(env, &ty, old) => old.clone(),
            Some(old) if stir_types::is_assignable(env, old, &ty) => ty,
            Some(old) => stir_types::join(env, old, &ty),
            None => ty,
        };
        self.solved.insert(id, new);
    }

    /// Unifies a parameter type (mentioning variables) with an argument type.
    pub fn unify(&mut self, env: &dyn TypeEnv, param: &Ty, arg: &Ty) {
        self.unify_depth(env, param, arg, 0);
    }

    /// Unifies a return type with the type the call is expected to have:
    /// literal types are kept.
    pub fn unify_expected(&mut self, env: &dyn TypeEnv, ret: &Ty, expected: &Ty) {
        let saved = std::mem::replace(&mut self.exact, true);
        self.unify_depth(env, ret, expected, 0);
        self.exact = saved;
    }

    fn unify_depth(&mut self, env: &dyn TypeEnv, param: &Ty, arg: &Ty, depth: u32) {
        if depth > 16 || arg.is_error() {
            return;
        }
        match (param.kind(), arg.kind()) {
            (TyKind::CovariantArg(p) | TyKind::ContravariantArg(p), _) => {
                self.unify_depth(env, p, arg, depth + 1)
            }
            (TyKind::Param { id, .. }, _) if self.vars.contains(id) => self.bind(env, *id, arg),
            (TyKind::List { elem: p, .. }, TyKind::List { elem: a, .. }) => {
                self.unify_depth(env, p, a, depth + 1)
            }
            (
                TyKind::Array {
                    key: pk, value: pv, ..
                }
                | TyKind::Iterable { key: pk, value: pv },
                TyKind::Array {
                    key: ak, value: av, ..
                }
                | TyKind::Iterable { key: ak, value: av },
            ) => {
                self.unify_depth(env, pk, ak, depth + 1);
                self.unify_depth(env, pv, av, depth + 1);
            }
            (
                TyKind::Array {
                    key: pk, value: pv, ..
                }
                | TyKind::Iterable { key: pk, value: pv },
                TyKind::List { elem: a, .. },
            ) => {
                self.unify_depth(env, pk, &Ty::int(), depth + 1);
                self.unify_depth(env, pv, a, depth + 1);
            }
            (
                TyKind::Array {
                    key: pk, value: pv, ..
                }
                | TyKind::Iterable { key: pk, value: pv },
                TyKind::Shape(s),
            ) => {
                let keys = s.fields.iter().map(|f| match &f.key {
                    stir_types::ShapeKey::Int(_) => Ty::int(),
                    stir_types::ShapeKey::Str(_) => Ty::string(),
                });
                let (keys, values) = (
                    Ty::union(keys.collect::<Vec<_>>()),
                    Ty::union(
                        s.fields
                            .iter()
                            .map(|f| widen_literals(&f.ty))
                            .collect::<Vec<_>>(),
                    ),
                );
                if !s.fields.is_empty() {
                    self.unify_depth(env, pk, &keys, depth + 1);
                    self.unify_depth(env, pv, &values, depth + 1);
                }
            }
            (TyKind::List { elem: p, .. }, TyKind::Shape(s)) => {
                let values = s.fields.iter().map(|f| widen_literals(&f.ty));
                let values = Ty::union(values.collect::<Vec<_>>());
                if !s.fields.is_empty() {
                    self.unify_depth(env, p, &values, depth + 1);
                }
            }
            (
                TyKind::Callable { sig: Some(pf), .. },
                TyKind::Callable {
                    sig: Some(af),
                    closure: true,
                },
            ) => {
                for (pp, ap) in pf.params.iter().zip(&af.params) {
                    self.unify_depth(env, &pp.ty, &ap.ty, depth + 1);
                }
                self.unify_depth(env, &pf.ret, &af.ret, depth + 1);
            }
            (TyKind::ClassString(Some(p)), TyKind::ClassString(Some(a))) => {
                self.unify_depth(env, p, a, depth + 1)
            }
            // `new_table('WP_Posts_List_Table')` names the class
            (TyKind::ClassString(Some(p)), TyKind::Literal(LitValue::String(name))) => {
                let name = name.trim_start_matches('\\');
                self.unify_depth(env, p, &Ty::class(name, Vec::new()), depth + 1);
            }
            (TyKind::Class { name: pn, args: pa }, TyKind::Class { .. }) => {
                // Find the argument's view as the parameter's class.
                if let Some(TyKind::Class { args: aa, .. }) =
                    as_class(env, arg, pn).as_ref().map(|t| t.kind().clone())
                {
                    for (p, a) in pa.iter().zip(aa.iter()) {
                        self.unify_depth(env, p, a, depth + 1);
                    }
                }
            }
            (TyKind::Union(_), _) => {
                // `?T` against `?X` / `X`: strip null from both; other concrete
                // members of the parameter union are removed from the argument.
                let members = param.members();
                let var_members: Vec<&Ty> =
                    members.iter().filter(|m| self.has_unsolved(m)).collect();
                if var_members.len() == 1 {
                    let concrete: Vec<&Ty> =
                        members.iter().filter(|m| !self.has_unsolved(m)).collect();
                    let rest: Vec<Ty> = arg
                        .members()
                        .iter()
                        .filter(|a| {
                            !concrete
                                .iter()
                                .any(|c| stir_types::is_assignable(env, a, c))
                        })
                        .cloned()
                        .collect();
                    if !rest.is_empty() {
                        self.unify_depth(env, var_members[0], &Ty::union(rest), depth + 1);
                    }
                } else {
                    // `Box<T>|Crate<T>` against a `Crate<X>`: it's the one it is
                    for a in arg.members() {
                        let found = var_members.iter().find(|m| {
                            matches!(m.kind(), TyKind::Class { name, .. } if as_class(env, a, name).is_some())
                        });
                        if let Some(m) = found {
                            self.unify_depth(env, m, a, depth + 1);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// Finds `ty`'s supertype that is an instance of class `name`.
fn as_class(env: &dyn TypeEnv, ty: &Ty, name: &str) -> Option<Ty> {
    let lower = name.to_ascii_lowercase();
    let mut queue = vec![ty.clone()];
    let mut steps = 0;
    while let Some(t) = queue.pop() {
        steps += 1;
        if steps > 64 {
            break;
        }
        if let TyKind::Class { name: n, args } = t.kind() {
            if n.to_ascii_lowercase() == lower {
                return Some(t.clone());
            }
            queue.extend(env.supertypes(n, args));
        }
    }
    None
}

/// `1` → `int`, `'a'|'b'` → `string`.
pub fn widen_literals(ty: &Ty) -> Ty {
    match ty.kind() {
        TyKind::Literal(l) => match l {
            LitValue::Int(_) => Ty::int(),
            LitValue::Float(_) => Ty::float(),
            LitValue::String(_) => Ty::string(),
            LitValue::Bool(_) => Ty::bool(),
        },
        // `string|false`: `false` here is a real part of the type, not a value that happens
        // to be false, and checks for it must be able to rule it out
        TyKind::Union(_) => Ty::union(ty.members().iter().map(|m| match m.kind() {
            TyKind::Literal(LitValue::Bool(_)) => m.clone(),
            _ => widen_literals(m),
        })),
        _ => ty.clone(),
    }
}
