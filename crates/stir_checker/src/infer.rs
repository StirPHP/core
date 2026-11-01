//! Generic type-argument inference by structural unification.

use rustc_hash::{FxHashMap, FxHashSet};
use stir_types::{Family, LitValue, ParamId, Ty, TyKind, TypeEnv};

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

    /// Replaces any still-unsolved variable with `fallback`.
    pub fn finish(&mut self, fallback: &Ty) {
        for v in self.vars.clone() {
            self.solved.entry(v).or_insert_with(|| fallback.clone());
        }
    }

    fn bind(&mut self, env: &dyn TypeEnv, id: ParamId, ty: &Ty) {
        if ty.is_error() {
            return;
        }
        // a literal argument fits a solution from the expected type as it is
        if let Some(old) = self.solved.get(&id)
            && stir_types::is_assignable(env, ty, old)
        {
            return;
        }
        // `listOf("a")` is a List<String>, but a declared literal union
        // (`List<"a" | "b">`) is kept
        let ty = if self.exact || !matches!(ty.kind(), TyKind::Literal(_)) {
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
    /// literal types are kept (`val k: List<"a" | "b"> = listOf("a")`).
    pub fn unify_expected(&mut self, env: &dyn TypeEnv, ret: &Ty, expected: &Ty) {
        let saved = std::mem::replace(&mut self.exact, true);
        self.unify_depth(env, ret, expected, 0);
        self.exact = saved;
    }

    fn unify_depth(&mut self, env: &dyn TypeEnv, param: &Ty, arg: &Ty, depth: u32) {
        if depth > 16 || arg.is_error() {
            return;
        }
        let arg = match arg.kind() {
            TyKind::Platform(inner) => inner.clone(),
            _ => arg.clone(),
        };
        match (param.kind(), arg.kind()) {
            (TyKind::Param { id, .. }, _) if self.vars.contains(id) => self.bind(env, *id, &arg),
            (TyKind::List(p), TyKind::List(a) | TyKind::MutableList(a))
            | (TyKind::MutableList(p), TyKind::MutableList(a))
            | (TyKind::List(p), TyKind::PhpList { elem: a, .. }) => {
                self.unify_depth(env, p, a, depth + 1)
            }
            (TyKind::Map(pk, pv), TyKind::Map(ak, av) | TyKind::MutableMap(ak, av))
            | (TyKind::MutableMap(pk, pv), TyKind::MutableMap(ak, av))
            | (
                TyKind::Map(pk, pv),
                TyKind::Array {
                    key: ak, value: av, ..
                },
            ) => {
                self.unify_depth(env, pk, ak, depth + 1);
                self.unify_depth(env, pv, av, depth + 1);
            }
            // PHP-family generic parameters (generic stubs / PHPDoc) against StirPHP values
            (
                TyKind::PhpList { elem: p, .. },
                TyKind::List(a) | TyKind::MutableList(a) | TyKind::PhpList { elem: a, .. },
            ) => self.unify_depth(env, p, a, depth + 1),
            (
                TyKind::Array {
                    key: pk, value: pv, ..
                }
                | TyKind::Iterable { key: pk, value: pv },
                TyKind::Map(ak, av)
                | TyKind::MutableMap(ak, av)
                | TyKind::Array {
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
                TyKind::List(a) | TyKind::MutableList(a) | TyKind::PhpList { elem: a, .. },
            ) => {
                self.unify_depth(env, pk, &Ty::int(), depth + 1);
                self.unify_depth(env, pv, a, depth + 1);
            }
            (TyKind::Callable { sig: Some(pf), .. }, TyKind::Fn(af)) => {
                for (pp, ap) in pf.params.iter().zip(&af.params) {
                    self.unify_depth(env, &pp.ty, &ap.ty, depth + 1);
                }
                self.unify_depth(env, &pf.ret, &af.ret, depth + 1);
            }
            (TyKind::ClassString(Some(p)), TyKind::ClassString(Some(a))) => {
                self.unify_depth(env, p, a, depth + 1)
            }
            (TyKind::Fn(pf), TyKind::Fn(af)) => {
                for (pp, ap) in pf.params.iter().zip(&af.params) {
                    self.unify_depth(env, &pp.ty, &ap.ty, depth + 1);
                }
                self.unify_depth(env, &pf.ret, &af.ret, depth + 1);
            }
            (
                TyKind::RecordRef { name: pn, args: pa },
                TyKind::RecordRef { name: an, args: aa },
            ) if pn.eq_ignore_ascii_case(an) => {
                for (p, a) in pa.iter().zip(aa) {
                    self.unify_depth(env, p, a, depth + 1);
                }
            }
            (TyKind::RecordRef { .. }, _) | (_, TyKind::RecordRef { .. }) => {
                let (p, a) = (
                    stir_types::unfold(env, param),
                    stir_types::unfold(env, &arg),
                );
                if &p != param || a != arg {
                    self.unify_depth(env, &p, &a, depth + 1);
                }
            }
            (TyKind::Record(pr), TyKind::Record(ar)) => {
                for pf in &pr.fields {
                    if let Some(af) = ar.field(&pf.name) {
                        self.unify_depth(env, &pf.ty, &af.ty, depth + 1);
                    }
                }
            }
            (TyKind::Class { name: pn, args: pa }, TyKind::Class { .. }) => {
                // Find the argument's view as the parameter's class.
                if let Some(TyKind::Class { args: aa, .. }) =
                    as_class(env, &arg, pn).as_ref().map(|t| t.kind().clone())
                {
                    for (p, a) in pa.iter().zip(aa.iter()) {
                        self.unify_depth(env, p, a, depth + 1);
                    }
                }
            }
            (TyKind::Union(_), _) => {
                // `T?` against `X?` / `X`: strip null from both; other concrete
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
                }
            }
            (TyKind::Platform(p), _) => self.unify_depth(env, p, &arg, depth + 1),
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

/// `1` → `Int`, `"a" | "b"` → `String` (keeps the literal's family).
pub fn widen_literals(ty: &Ty) -> Ty {
    match ty.kind() {
        TyKind::Literal(l) => {
            let safe = l.family == Family::Safe;
            match (&l.value, safe) {
                (LitValue::Int(_), true) => Ty::int(),
                (LitValue::Int(_), false) => Ty::php_int(),
                (LitValue::Float(_), true) => Ty::float(),
                (LitValue::Float(_), false) => Ty::php_float(),
                (LitValue::String(_), true) => Ty::string(),
                (LitValue::String(_), false) => Ty::php_string(),
                (LitValue::Bool(_), true) => Ty::bool(),
                (LitValue::Bool(_), false) => Ty::php_bool(),
            }
        }
        TyKind::Union(_) => Ty::union(ty.members().iter().map(widen_literals)),
        _ => ty.clone(),
    }
}
