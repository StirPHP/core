//! Scripts answering for the checker: the return types of calls.

use rustc_hash::FxHashMap;
use smol_str::SmolStr;
use stir_db::ScriptFile;
use stir_hir::doc::{Aliases, DocCx};
use stir_hir::name::Scope;
use stir_phpdoc::TypeKind;
use stir_script::ScriptError;
use stir_script::rhai::{Array, Dynamic, Map};
use stir_syntax::{SyntaxNode, TextSize};
use stir_types::{Ty, TyKind};

use super::facade::TypeFacade;
use super::{
    CLOSURE_THIS, ENGINE, Node, RETURN_TYPE_OF_FUNCTION, RETURN_TYPE_OF_METHOD,
    RETURN_TYPE_OF_STATIC_METHOD, ScriptCtx, ScriptFailure, compiled_script, node,
};
use crate::Ctx;
use crate::body::Checker;
use crate::model::LowerCx;

/// Why a type a script wrote can't be used.
pub(crate) enum BadType {
    /// It doesn't read as a type: the script's mistake.
    Syntax(String),
    /// It names what doesn't exist. As likely the code's mistake as the script's: the checker says
    /// what's wrong with the code, and the script's answer doesn't count.
    Unknown,
}

/// Lowers a PHPDoc type written by a script. Names in it are fully qualified: a script isn't in
/// any file's namespace. `self_ty` is what `self` and `static` mean.
pub(crate) fn parse_type(ctx: &Ctx<'_>, text: &str, self_ty: Option<&Ty>) -> Result<Ty, BadType> {
    let node = stir_phpdoc::parse_type(text, TextSize::new(0));
    if let TypeKind::Error(message) = &node.kind {
        return Err(BadType::Syntax(message.clone()));
    }
    let scope = Scope::default();
    let aliases = Aliases::default();
    let cx = DocCx {
        scope: &scope,
        type_params: &[],
        aliases: &aliases,
    };
    let lowered = cx.lower(&node);
    let lower_cx = LowerCx {
        self_ty: self_ty.cloned(),
        ..LowerCx::default()
    };
    let mut problems = Vec::new();
    let ty = ctx.lower(&lowered, &lower_cx, &mut problems);
    if !problems.is_empty() || ty.is_error() {
        return Err(BadType::Unknown);
    }
    Ok(ty)
}

impl Checker<'_, '_> {
    /// What a call of the function `name` returns, given that its signature says `ret`.
    pub(crate) fn function_hook(&mut self, node: &SyntaxNode, name: &str, ret: Ty) -> Ty {
        let key = name.trim_start_matches('\\').to_ascii_lowercase();
        let Some(scripts) = self.ctx.scripts.functions.get(key.as_str()) else {
            return ret;
        };
        self.hooked_return(node, scripts, RETURN_TYPE_OF_FUNCTION, ret, None)
    }

    /// What a call of method `name` on `class` returns, given that its signature says `ret`. It
    /// counts for a target on any class that `class` is.
    pub(crate) fn method_hook(
        &mut self,
        node: &SyntaxNode,
        class: &Ty,
        name: &str,
        ret: Ty,
        is_static: bool,
    ) -> Ty {
        let (targets, function) = if is_static {
            (
                &self.ctx.scripts.static_methods,
                RETURN_TYPE_OF_STATIC_METHOD,
            )
        } else {
            (&self.ctx.scripts.methods, RETURN_TYPE_OF_METHOD)
        };
        if targets.is_empty() {
            return ret;
        }
        let named = targets.get(name.to_ascii_lowercase().as_str());
        // a target of `Class::*` is for what says the class it returns, and no more
        let Some(targets) = named.or_else(|| {
            matches!(ret.kind(), TyKind::Class { args, .. } if args.is_empty())
                .then(|| targets.get("*"))
                .flatten()
        }) else {
            return ret;
        };
        let TyKind::Class {
            name: class_name, ..
        } = class.kind()
        else {
            return ret;
        };
        let scripts: Vec<ScriptFile> = targets
            .iter()
            .filter(|(target, _)| self.ctx.is_subclass(class_name, target))
            .map(|(_, script)| *script)
            .collect();
        if scripts.is_empty() {
            return ret;
        }
        self.hooked_return(node, &scripts, function, ret, Some(class))
    }

    /// `ret`, or what the first of `scripts` to answer for the call `node` says instead. `parts` are
    /// the expressions a script can ask the type of, with what's known of them. Only the checker
    /// is calling here, so a hook that fails gets no second chance on this file.
    pub(crate) fn hooked_return(
        &mut self,
        node: &SyntaxNode,
        scripts: &[ScriptFile],
        function: &'static str,
        ret: Ty,
        self_ty: Option<&Ty>,
    ) -> Ty {
        let db = self.ctx.db;
        let ws = self.ctx.ws;
        let parse = stir_db::parse(db, self.file);
        let Some(call) = Node::new(parse.green(), self.file, node) else {
            return ret;
        };
        let types = self.types_of_call_parts(node, self_ty);
        for &script in scripts {
            if self.scripts_off.contains(&(script, function)) {
                continue;
            }
            let Some(ast) = &compiled_script(db, script).ast else {
                continue;
            };
            let ctx = ScriptCtx::for_call(ws, self.file, types.clone(), call.range(), ret.clone());
            let args = vec![Dynamic::from(call.clone()), Dynamic::from(ctx)];
            let answer = stir_script::call(&ENGINE, ast, function, args);
            match answer.and_then(|a| self.answered_type(a, self_ty, function)) {
                Ok(Some(ty)) => return ty,
                Ok(None) => {}
                Err(error) => {
                    let path = Some(self.path.clone());
                    self.script_failures
                        .push(ScriptFailure::new(script, function, error, path));
                    self.scripts_off.insert((script, function));
                }
            }
        }
        ret
    }

    /// What the scripts say a closure passed in the call `node` runs bound to, given that the
    /// parameter's `@param-closure-this` says `declared`: its `$this`, and the properties that are
    /// known to be set on it without being declared.
    pub(crate) fn closure_this_hook(
        &mut self,
        node: &SyntaxNode,
        declared: &Ty,
    ) -> Option<(Ty, Vec<SmolStr>)> {
        let db = self.ctx.db;
        let ws = self.ctx.ws;
        for &script in &self.ctx.scripts.closure_this {
            if self.scripts_off.contains(&(script, CLOSURE_THIS)) {
                continue;
            }
            let Some(ast) = &compiled_script(db, script).ast else {
                continue;
            };
            let ctx = ScriptCtx::for_call(
                ws,
                self.file,
                FxHashMap::default(),
                node.text_range(),
                declared.clone(),
            );
            let args = vec![
                Dynamic::from(TypeFacade::new(declared.clone(), ws)),
                Dynamic::from(ctx),
            ];
            let answer = stir_script::call(&ENGINE, ast, CLOSURE_THIS, args);
            let read = answer.and_then(|a| self.closure_this_answer(a));
            match read {
                Ok(Some(found)) => return Some(found),
                Ok(None) => {}
                Err(error) => {
                    let path = Some(self.path.clone());
                    self.script_failures.push(ScriptFailure::new(
                        script,
                        CLOSURE_THIS,
                        error,
                        path,
                    ));
                    self.scripts_off.insert((script, CLOSURE_THIS));
                }
            }
        }
        None
    }

    /// What `closure_this` answered: a PHPDoc type, or `#{ type, properties }`, or nothing.
    fn closure_this_answer(
        &self,
        answer: Dynamic,
    ) -> Result<Option<(Ty, Vec<SmolStr>)>, ScriptError> {
        let (ty, properties) = if answer.is::<Map>() {
            let mut map = answer.cast::<Map>();
            let ty = map.remove("type").unwrap_or(Dynamic::UNIT);
            let names = map
                .remove("properties")
                .and_then(|p| p.try_cast::<Array>())
                .unwrap_or_default();
            let names = names
                .into_iter()
                .filter_map(|n| n.into_string().ok())
                .map(SmolStr::new)
                .collect();
            (ty, names)
        } else {
            (answer, Vec::new())
        };
        let ty = self.answered_type(ty, None, CLOSURE_THIS)?;
        Ok(ty.map(|t| (t, properties)))
    }

    /// What's known of the receiver and arguments of the call `node`.
    fn types_of_call_parts(
        &self,
        node: &SyntaxNode,
        self_ty: Option<&Ty>,
    ) -> FxHashMap<stir_syntax::TextRange, Ty> {
        let mut types = FxHashMap::default();
        if let Some(receiver) = node::receiver_of(node) {
            let range = node::without_parens(receiver).text_range();
            let known = self.types.get(&range).or(self_ty).cloned();
            types.extend(known.map(|t| (range, t)));
        }
        let args = node
            .children()
            .filter(|c| c.kind() == stir_syntax::SyntaxKind::ARG_LIST)
            .flat_map(|list| list.children())
            .filter_map(|arg| arg.last_child());
        for arg in args {
            let range = node::without_parens(arg).text_range();
            types.extend(self.types.get(&range).map(|t| (range, t.clone())));
        }
        types
    }

    /// The type a hook's answer means. Nothing, for `()`: it has no opinion on this call.
    fn answered_type(
        &self,
        answer: Dynamic,
        self_ty: Option<&Ty>,
        function: &str,
    ) -> Result<Option<Ty>, ScriptError> {
        if answer.is_unit() {
            return Ok(None);
        }
        let Ok(text) = answer.into_string() else {
            return Err(ScriptError::new(format!(
                "`{function}` should answer a PHPDoc type as a string, or nothing"
            )));
        };
        match parse_type(self.ctx, &text, self_ty) {
            Ok(ty) => Ok(Some(ty)),
            Err(BadType::Unknown) => Ok(None),
            Err(BadType::Syntax(why)) => Err(ScriptError::new(format!(
                "`{function}` answered `{text}`, which isn't a type: {why}"
            ))),
        }
    }
}
