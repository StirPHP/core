//! What a function returns, from its body, when nothing declares it.
//!
//! A function with no return type and no `@return` gives its callers what its `return`s give:
//! `'paid'|'pending'` for one that returns either string. That's only known for a body that's
//! sure to be the one that runs: any function, and a method nothing in the project overrides.
//! Anything the body can't be sure of, such as a parameter with no type, leaves the call
//! untyped, as it is without this.
//!
//! An empty body is a placeholder, and says nothing.
//!
//! Each body is checked by itself, once, and only when something calls it. A function that
//! calls itself, directly or through others, is a cycle, and stays untyped.

use rustc_hash::FxHashSet;
use smol_str::SmolStr;
use stir_db::{Db, FileOrigin, SourceFile, Workspace};
use stir_hir::ClassKind;
use stir_hir::ast::NodeExt;
use stir_syntax::SyntaxNode;
use stir_types::{LitValue, Ty, TyKind};

use crate::body::Checker;
use crate::model::{ClassModel, Ctx, FnSig};

/// A function or method with a body, as a query's key.
#[salsa::interned(debug)]
pub(crate) struct Body<'db> {
    pub file: SourceFile,
    /// The class's key, for a method; `None` for a function.
    #[returns(ref)]
    pub class: Option<SmolStr>,
    /// Its index among the file's functions, or the class's methods.
    pub index: u32,
}

impl<'db> Ctx<'db> {
    /// The signature of function `loc`, with what its body returns if it doesn't say.
    pub(crate) fn function_body_return(&self, sig: FnSig, loc: stir_hir::ItemLoc) -> FnSig {
        self.with_body_return(sig, loc.file, None, loc.index)
    }

    /// The signature of method `m` of `owner`, with what its body returns if it doesn't say.
    pub(crate) fn method_body_return(
        &self,
        sig: FnSig,
        owner: &ClassModel<'db>,
        m: &stir_hir::MethodDef,
    ) -> FnSig {
        // what `new` gives is the object, whatever a constructor returns
        if !m.has_body
            || m.magic
            || m.mods.is_abstract
            || m.name.eq_ignore_ascii_case("__construct")
            || matches!(owner.kind(), ClassKind::Trait | ClassKind::Interface)
        {
            return sig;
        }
        let Some(index) = owner.def.methods.iter().position(|x| std::ptr::eq(x, m)) else {
            return sig;
        };
        self.with_body_return(sig, owner.file, Some(owner.key.clone()), index as u32)
    }

    fn with_body_return(
        &self,
        sig: FnSig,
        file: SourceFile,
        class: Option<SmolStr>,
        index: u32,
    ) -> FnSig {
        // PHP's own functions have no bodies to go by
        if sig.ret_declared || !sig.ret.is_untyped() || file.origin(self.db) == FileOrigin::Stub {
            return sig;
        }
        let body = Body::new(self.db, file, class, index);
        let ret = body_return(self.db, self.ws, body);
        if ret.is_untyped() {
            return sig;
        }
        FnSig {
            ret: ret.clone(),
            ..sig
        }
    }
}

/// What `body` returns, or untyped where that isn't known.
#[salsa::tracked(returns(ref), cycle_result = on_cycle)]
pub(crate) fn body_return<'db>(db: &'db dyn Db, ws: Workspace, body: Body<'db>) -> Ty {
    let ctx = Ctx::new(db, ws);
    let run = || returned(&ctx, body).unwrap_or_else(Ty::untyped);
    // hooks will run far down in it, as they do checking a file
    if ctx.scripts.has_return_hooks() || !ctx.scripts.magic.is_empty() {
        stir_script::with_stack(run)
    } else {
        run()
    }
}

fn on_cycle<'db>(_: &'db dyn Db, _: salsa::Id, _: Workspace, _: Body<'db>) -> Ty {
    Ty::untyped()
}

fn returned(ctx: &Ctx<'_>, body: Body<'_>) -> Option<Ty> {
    let db = ctx.db;
    let file = *body.file(db);
    let root = stir_db::parse(db, file).syntax();
    let mut checker = Checker::new(ctx, file);
    match body.class(db) {
        None => {
            let def = ctx.items(file).functions.get(*body.index(db) as usize)?;
            let node = def.ptr.to_node(&root);
            checker.wanted_return = Some(node.text_range());
            checker.function_decl(&node);
        }
        Some(key) => {
            let model = ctx.class(key)?;
            let m = model.def.methods.get(*body.index(db) as usize)?;
            if overridden(ctx, &model, &m.name) {
                return None;
            }
            let node = m.ptr.to_node(&root);
            checker.wanted_return = Some(node.text_range());
            checker.class = Some(model.clone());
            checker.set_lower_cx(ctx.class_cx(&model));
            checker.method_decl(&node, &model, &model);
        }
    }
    checker.body_return
}

/// Is method `name` of `model` declared again below it, so that a call may run another body?
fn overridden(ctx: &Ctx<'_>, model: &ClassModel<'_>, name: &str) -> bool {
    let subtypes = stir_hir::direct_subtypes(ctx.db, ctx.ws);
    let mut seen = FxHashSet::default();
    let mut queue = vec![model.key.clone()];
    while let Some(key) = queue.pop() {
        for sub in subtypes.get(&key).into_iter().flatten() {
            if !seen.insert(sub.clone()) {
                continue;
            }
            let Some(c) = ctx.class(sub) else { continue };
            if ctx
                .find_method(&c.self_ty(), name)
                .is_some_and(|(owner, _, _)| owner.key != model.key)
            {
                return true;
            }
            queue.push(sub.clone());
        }
    }
    false
}

/// A union of more literals than this is said as their type.
const LITERAL_LIMIT: usize = 8;

/// What a body returns, from the types of its `return`s, and whether its end can be reached.
pub(crate) fn summary(node: &SyntaxNode, returns: &[Ty], falls_off: bool) -> Ty {
    // `{}` stands in for something, as a placeholder for an extension's class does: it says nothing
    // of what the real one gives
    if node
        .child(stir_syntax::SyntaxKind::BLOCK)
        .is_some_and(|b| b.children().next().is_none())
    {
        return Ty::untyped();
    }
    if crate::body::is_generator(node) {
        return Ty::class("Generator", Vec::new());
    }
    let mut members = Vec::new();
    for t in returns {
        if t.is_error() || t.is_untyped() || t.contains_error() {
            return Ty::untyped();
        }
        if t.members().iter().any(|m| m.is_untyped()) {
            return Ty::untyped();
        }
        // a bare `return;` gives the caller `null`, as falling off the end does
        members.push(if matches!(t.kind(), TyKind::Void) {
            Ty::null()
        } else {
            t.clone()
        });
    }
    if falls_off {
        members.push(Ty::null());
    }
    let ty = Ty::union(members);
    let literals = ty
        .members()
        .iter()
        .filter(|m| matches!(m.kind(), TyKind::Literal(l) if !matches!(l, LitValue::Bool(_))))
        .count();
    if literals > LITERAL_LIMIT {
        crate::infer::widen_literals(&ty)
    } else {
        ty
    }
}
