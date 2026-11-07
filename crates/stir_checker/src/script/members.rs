//! Members scripts make available on a type when the code declares nothing by that name: from
//! `magic_method` and `magic_property` for one name, and from what `magic_members` says calls of a
//! class are forwarded to. They're looked up after everything else, the `@mixin` types included,
//! where `__call`, `__callStatic` and `__get` would be reached.

use smol_str::SmolStr;
use stir_syntax::{SyntaxNode, TextRange};
use stir_types::{Ty, TyKind};

use super::facade::TypeFacade;
use super::magic::{Answer, ask_member, member_of_tag};
use super::{MAGIC_METHOD, MAGIC_PROPERTY, ScriptFailure};
use crate::body::Checker;
use crate::model::Ctx;
use crate::{Def, MemberKind, codes};

/// How many forwards a call may go through, past which it's taken to go in circles.
const MAX_FORWARDS: u32 = 4;

impl<'db> Ctx<'db> {
    /// What the scripts say `name` is on a value of `ty`, a class type: a method if `is_static`
    /// says how it's called, a property otherwise. Asked once per model.
    pub(crate) fn magic_answer(
        &self,
        ty: &Ty,
        name: &str,
        is_static: Option<bool>,
    ) -> Option<Answer> {
        let scripts = match is_static {
            Some(_) => &self.scripts.magic_methods,
            None => &self.scripts.magic_properties,
        };
        if scripts.is_empty() || !matches!(ty.kind(), TyKind::Class { .. }) {
            return None;
        }
        let kind = match is_static {
            Some(false) => 0,
            Some(true) => 1,
            None => 2,
        };
        let key = (ty.clone(), SmolStr::new(name.to_ascii_lowercase()), kind);
        if let Some(answer) = self.magic_answers.borrow().get(&key) {
            return answer.clone();
        }
        let mut found = None;
        for &script in scripts {
            let receiver = stir_script::rhai::Dynamic::from(TypeFacade::new(ty.clone(), self.ws));
            match ask_member(script, receiver, name, is_static) {
                Ok(Some(answer)) => {
                    found = Some(answer);
                    break;
                }
                Ok(None) => {}
                Err(error) => {
                    let function = if is_static.is_some() {
                        MAGIC_METHOD
                    } else {
                        MAGIC_PROPERTY
                    };
                    let failure = ScriptFailure::new(script, function, error, None);
                    let mut failures = self.script_failures.borrow_mut();
                    if !failures.contains(&failure) {
                        failures.push(failure);
                    }
                }
            }
        }
        self.magic_answers.borrow_mut().insert(key, found.clone());
        found
    }

    /// Where calls of methods `ty` hasn't got go, as `magic_members` says for it or the nearest
    /// of its ancestors. `static` in it is `ty`.
    pub(crate) fn forwarded_calls(&self, ty: &Ty) -> Option<Ty> {
        if self.scripts.magic.is_empty() {
            return None;
        }
        for anc in self.ancestors(ty).iter() {
            let TyKind::Class { name, .. } = anc.kind() else {
                continue;
            };
            let Some(c) = self.class(name) else { continue };
            if let Some(to) = &self.script_magic(&c).forward_calls {
                return super::hooks::parse_type(self, to, Some(ty)).ok();
            }
        }
        None
    }
}

impl Checker<'_, '_> {
    /// A method scripts make available on `cls`, called on `recv` (the same, unless it's `static`
    /// or `$this`): the type of the call, or nothing if they don't know of one.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn magic_call(
        &mut self,
        node: &SyntaxNode,
        cls: &Ty,
        recv: &Ty,
        name: &str,
        range: TextRange,
        expected: Option<&Ty>,
        is_static: bool,
    ) -> Option<Ty> {
        if self.forwards >= MAX_FORWARDS || !matches!(cls.kind(), TyKind::Class { .. }) {
            return None;
        }
        match self.ctx.magic_answer(cls, name, Some(is_static)) {
            Some(Answer::Member { tag, origin }) => {
                let TyKind::Class { name: class, args } = cls.kind() else {
                    return None;
                };
                let owner = self.ctx.class(class)?;
                let (_, methods) = member_of_tag(owner.def, &tag);
                let m = methods.into_iter().next()?;
                match origin {
                    Some(o) => self.def(
                        range,
                        Def::Member {
                            class: o.class,
                            kind: o.kind,
                            name: o.name,
                        },
                    ),
                    None => self.def(
                        range,
                        Def::Member {
                            class: owner.key.clone(),
                            kind: MemberKind::Method,
                            name: SmolStr::new(name),
                        },
                    ),
                }
                let cx = self.ctx.class_cx(&owner);
                let sig = self.ctx.build_sig(
                    &format!("m:{}::{}", owner.key, m.name.to_ascii_lowercase()),
                    &m.name,
                    &m.sig,
                    &cx,
                    &mut Vec::new(),
                );
                let sig = crate::body::sig_for_receiver(sig, recv);
                let map = self.ctx.subst_map(&owner, args);
                let ret = self.check_args(node, &sig, &map, expected, &[]);
                return Some(crate::body::resolve_self(&ret, recv));
            }
            Some(Answer::Forward { to, fluent }) => {
                let to = super::hooks::parse_type(self.ctx, &to, Some(recv)).ok()?;
                return self.forward_call(node, recv, &to, fluent, name, range, expected);
            }
            None => {}
        }
        let to = self.ctx.forwarded_calls(cls)?;
        self.forward_call(node, recv, &to, false, name, range, expected)
    }

    /// The call of `name` made on `to` instead, unless `to` hasn't got it either: then nothing,
    /// for the call to be reported as it's written.
    #[allow(clippy::too_many_arguments)]
    fn forward_call(
        &mut self,
        node: &SyntaxNode,
        recv: &Ty,
        to: &Ty,
        fluent: bool,
        name: &str,
        range: TextRange,
        expected: Option<&Ty>,
    ) -> Option<Ty> {
        let before = self.diags.len();
        self.forwards += 1;
        let t = self.call_member(node, to, name, range, expected);
        self.forwards -= 1;
        let missing = self.diags[before..].iter().any(|d| {
            d.range == range
                && matches!(
                    d.code.as_str(),
                    codes::METHOD_NOT_FOUND | codes::METHOD_NON_OBJECT
                )
        });
        if missing {
            self.diags.truncate(before);
            return None;
        }
        // a call that gives back what it was forwarded to gives back what it was made on
        if fluent && same_class(&t, to) {
            return Some(recv.clone());
        }
        Some(t)
    }

    /// A property scripts make available on `cls`: its type, or nothing if they don't know of one.
    pub(crate) fn magic_property(&mut self, cls: &Ty, name: &str, range: TextRange) -> Option<Ty> {
        if self.forwards >= MAX_FORWARDS {
            return None;
        }
        match self.ctx.magic_answer(cls, name, None)? {
            Answer::Member { tag, origin } => {
                let TyKind::Class { name: class, args } = cls.kind() else {
                    return None;
                };
                let owner = self.ctx.class(class)?;
                let (properties, _) = member_of_tag(owner.def, &tag);
                let p = properties.into_iter().next()?;
                let def = match origin {
                    Some(o) => Def::Member {
                        class: o.class,
                        kind: o.kind,
                        name: o.name,
                    },
                    None => Def::Member {
                        class: owner.key.clone(),
                        kind: MemberKind::Property,
                        name: SmolStr::new(name),
                    },
                };
                self.def(range, def);
                let cx = self.ctx.class_cx(&owner);
                let ty = p
                    .doc_ty
                    .as_ref()
                    .map_or_else(Ty::mixed, |t| self.ctx.lower(t, &cx, &mut Vec::new()));
                let map = self.ctx.subst_map(&owner, args);
                Some(crate::body::resolve_self(
                    &stir_types::subst(&ty, &map),
                    cls,
                ))
            }
            Answer::Forward { to, .. } => {
                let to = super::hooks::parse_type(self.ctx, &to, Some(cls)).ok()?;
                let before = self.diags.len();
                self.forwards += 1;
                let t = self.member_type(&to, name, range);
                self.forwards -= 1;
                let missing = self.diags[before..].iter().any(|d| {
                    d.range == range
                        && matches!(
                            d.code.as_str(),
                            codes::PROPERTY_NOT_FOUND | codes::PROPERTY_NON_OBJECT
                        )
                });
                if missing {
                    self.diags.truncate(before);
                    return None;
                }
                Some(t)
            }
        }
    }
}

/// Are both the same class, whatever their type arguments?
fn same_class(a: &Ty, b: &Ty) -> bool {
    let a = stir_types::without_null(a);
    match (a.kind(), b.kind()) {
        (TyKind::Class { name: x, .. }, TyKind::Class { name: y, .. }) => x.eq_ignore_ascii_case(y),
        (TyKind::SelfType { class, .. }, _) => same_class(class, b),
        _ => false,
    }
}

/// Where the member `name` that scripts declare on the class with lowercase name `class` is
/// declared, if they say: for going to its definition.
pub fn magic_origin(
    db: &dyn stir_db::Db,
    ws: stir_db::Workspace,
    class: &str,
    kind: MemberKind,
    name: &str,
) -> Option<super::magic::Origin> {
    let ctx = Ctx::new(db, ws);
    let c = ctx.class(class)?;
    ctx.script_magic(&c).origin(kind, name).cloned()
}
