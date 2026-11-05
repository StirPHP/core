//! Statement and expression checking.

use std::rc::Rc;
use std::sync::Arc;

use rustc_hash::FxHashMap;
use smol_str::SmolStr;
use stir_db::{Diagnostic, Severity, SourceFile};
use stir_hir::ast::NodeExt;
use stir_hir::name::Scope;
use stir_hir::type_ref::{parse_int, unquote};
use stir_hir::{ClassKind, FileItems, Visibility};
use stir_syntax::{SyntaxKind, SyntaxKind::*, SyntaxNode, SyntaxToken, TextRange};
use stir_types::{Family, LitValue, Ty, TyKind};

use crate::codes;
use crate::infer::{Inference, widen_literals};
use crate::model::{ClassModel, Ctx, FnSig, LowerCx};
use crate::typed::{Def, MemberKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalKind {
    /// A function parameter.
    Param,
    /// `$this`
    This,
    /// A variable, or a property path that narrowing tracks.
    Php,
    /// A `foreach` / `catch` binding.
    Loop,
}

impl LocalKind {
    fn mutable(self) -> bool {
        self != LocalKind::This
    }
}

#[derive(Debug, Clone)]
pub struct Local {
    /// The current (possibly narrowed) type.
    pub ty: Ty,
    /// The declared type (narrowing never goes beyond it).
    pub declared: Ty,
    pub kind: LocalKind,
    #[allow(dead_code)]
    pub range: TextRange,
}

/// The function whose body is being checked.
#[derive(Debug, Clone)]
struct FnCx {
    ret: Ty,
    /// Infer the return type from `return` statements instead of checking it.
    infer_ret: bool,
    returns: Vec<Ty>,
}

/// Narrowings learned from a condition.
#[derive(Debug, Clone, Default)]
struct Facts {
    narrow: Vec<(SmolStr, Ty)>,
}

pub struct Checker<'a, 'db> {
    pub ctx: &'a Ctx<'db>,
    pub file: SourceFile,
    pub path: std::path::PathBuf,
    pub items: &'db FileItems,
    pub diags: Vec<Diagnostic>,
    conditional_depth: u32,
    /// Inside `isset`, `empty` or the left of `??`: the value is allowed not to exist.
    probing: u32,
    /// The function (by `fns` depth) that ran `include` or `extract`, which can
    /// define any variable from then on.
    dynamic_vars_fn: Option<usize>,
    frames: Vec<FxHashMap<SmolStr, Local>>,
    fns: Vec<FnCx>,
    class: Option<Rc<ClassModel<'db>>>,
    lower_cx: LowerCx,
    in_ctor: bool,
    /// Expression types.
    pub types: FxHashMap<TextRange, Ty>,
    /// Function-call nodes and the fully-qualified function they call.
    pub function_calls: FxHashMap<TextRange, SmolStr>,
    /// What names resolve to, by the range of the name (for the language server).
    pub defs: FxHashMap<TextRange, Def>,
    /// Local bindings by declaring-name range: declared type and kind.
    pub locals: FxHashMap<TextRange, (Ty, LocalKind)>,
}

fn is_stmt_kind(k: SyntaxKind) -> bool {
    matches!(
        k,
        EXPR_STMT
            | ECHO_STMT
            | RETURN_STMT
            | IF_STMT
            | WHILE_STMT
            | DO_WHILE_STMT
            | FOR_STMT
            | FOREACH_STMT
            | SWITCH_STMT
            | BREAK_STMT
            | CONTINUE_STMT
            | TRY_STMT
            | THROW_STMT
            | GLOBAL_STMT
            | STATIC_VAR_STMT
            | UNSET_STMT
            | BLOCK
            | ECHO_TAG_STMT
            | EMPTY_STMT
            | INLINE_HTML_STMT
            | DECLARE_STMT
            | NAMESPACE_DECL
            | FUNCTION_DECL
            | CLASS_DECL
            | INTERFACE_DECL
            | TRAIT_DECL
            | ENUM_DECL
            | GOTO_STMT
            | LABEL_STMT
            | USE_DECL
            | CONST_DECL
            | HALT_COMPILER_STMT
    )
}

/// The expression children of a node (skipping tokens and non-expression parts).
fn exprs_of(node: &SyntaxNode) -> Vec<SyntaxNode> {
    node.children().filter(|c| is_expr_kind(c.kind())).collect()
}

pub fn is_expr_kind(k: SyntaxKind) -> bool {
    matches!(
        k,
        LITERAL
            | STRING_LITERAL
            | INTERP_STRING
            | HEREDOC
            | SHELL_EXEC
            | VARIABLE_EXPR
            | VARIABLE_VARIABLE
            | NAME_EXPR
            | ARRAY_EXPR
            | LIST_EXPR
            | PAREN_EXPR
            | BIN_EXPR
            | PREFIX_EXPR
            | POSTFIX_EXPR
            | ASSIGN_EXPR
            | TERNARY_EXPR
            | ELVIS_EXPR
            | CAST_EXPR
            | AS_EXPR
            | INSTANCEOF_EXPR
            | CALL_EXPR
            | METHOD_CALL_EXPR
            | NULLSAFE_METHOD_CALL_EXPR
            | PROPERTY_FETCH_EXPR
            | NULLSAFE_PROPERTY_FETCH_EXPR
            | STATIC_CALL_EXPR
            | STATIC_PROPERTY_FETCH_EXPR
            | CLASS_CONST_FETCH_EXPR
            | INDEX_EXPR
            | NEW_EXPR
            | CLONE_EXPR
            | CLOSURE_EXPR
            | ARROW_FN_EXPR
            | MATCH_EXPR
            | ISSET_EXPR
            | EMPTY_EXPR
            | EVAL_EXPR
            | EXIT_EXPR
            | PRINT_EXPR
            | INCLUDE_EXPR
            | YIELD_EXPR
            | YIELD_FROM_EXPR
            | THROW_EXPR
            | ERROR_SUPPRESS_EXPR
            | MAGIC_CONST
    )
}

fn op_token(node: &SyntaxNode) -> Option<SyntaxToken> {
    node.children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| !t.kind().is_trivia())
}

fn text_of(node: &SyntaxNode) -> String {
    node.text().to_string().trim().to_owned()
}

impl<'a, 'db> Checker<'a, 'db> {
    pub fn new(ctx: &'a Ctx<'db>, file: SourceFile) -> Self {
        Checker {
            ctx,
            file,
            path: file.path(ctx.db).clone(),
            items: ctx.items(file),
            diags: Vec::new(),
            conditional_depth: 0,
            probing: 0,
            dynamic_vars_fn: None,
            frames: vec![FxHashMap::default()],
            fns: Vec::new(),
            class: None,
            lower_cx: LowerCx::default(),
            in_ctor: false,
            types: FxHashMap::default(),
            function_calls: FxHashMap::default(),
            defs: FxHashMap::default(),
            locals: FxHashMap::default(),
        }
    }

    fn def(&mut self, range: TextRange, def: Def) {
        self.defs.insert(range, def);
    }

    fn member_def(
        &mut self,
        range: TextRange,
        class: &ClassModel<'_>,
        kind: MemberKind,
        name: &str,
    ) {
        self.def(
            range,
            Def::Member {
                class: class.key.clone(),
                kind,
                name: SmolStr::new(name),
            },
        );
    }

    /// The range of the token spelling `name` (or `$name`) inside `range`.
    fn name_range(&self, range: TextRange, name: &str) -> TextRange {
        let root = stir_db::parse(self.ctx.db, self.file).syntax();
        if !root.text_range().contains_range(range) {
            return range;
        }
        let tokens: Vec<SyntaxToken> = match root.covering_element(range) {
            stir_syntax::SyntaxElement::Token(t) => vec![t],
            stir_syntax::SyntaxElement::Node(n) => n
                .descendants_with_tokens()
                .filter_map(|e| e.into_token())
                .collect(),
        };
        tokens
            .into_iter()
            .filter(|t| range.contains_range(t.text_range()))
            .find(|t| t.text() == name || t.text().strip_prefix('$') == Some(name))
            .map(|t| t.text_range())
            .unwrap_or(range)
    }

    // ------------------------------------------------------------ diagnostics

    pub fn error(&mut self, range: TextRange, code: &'static str, msg: impl Into<String>) {
        self.diags
            .push(Diagnostic::error(self.path.clone(), range, code, msg));
    }

    /// An override's types are mostly PHPDoc, which PHP doesn't enforce: reported only
    /// above the level WordPress-style projects run at.
    fn tag_php_override(&mut self) {
        if let Some(d) = self.diags.last_mut() {
            d.php_level = Some(6);
        }
    }

    /// The last diagnostic is about only some members of a union: a level 7 problem.
    fn tag_partial_union(&mut self) {
        if let Some(d) = self.diags.last_mut() {
            d.php_level = Some(7);
        }
    }

    fn warn(&mut self, range: TextRange, code: &'static str, msg: impl Into<String>) {
        self.diags.push(
            Diagnostic::error(self.path.clone(), range, code, msg).with_severity(Severity::Warning),
        );
    }

    fn scope(&self, node: &SyntaxNode) -> Arc<Scope> {
        self.items.scope_at(node.text_range().start()).clone()
    }

    /// Lowers a type node written in this file.
    pub fn lower_type_node(&mut self, node: &SyntaxNode) -> Ty {
        let scope = self.scope(node);
        let names: Vec<SmolStr> = self
            .lower_cx
            .generics
            .iter()
            .map(|(n, _)| n.clone())
            .collect();
        let tcx = stir_hir::type_ref::TypeCtx {
            scope: &scope,
            type_params: &names,
        };
        let tr = tcx.lower(node);
        let mut sink = Vec::new();
        let ty = self.ctx.lower(&tr, &self.lower_cx, &mut sink);
        for e in sink {
            self.error(e.range, e.code, e.message);
        }
        self.check_type_bounds(node.text_range(), &ty);
        ty
    }

    fn assignable(&self, from: &Ty, to: &Ty) -> bool {
        stir_types::is_assignable(self.ctx, from, to)
    }

    /// Reports a mismatch if `from` is not assignable to `to`.
    fn check_assign(&mut self, range: TextRange, from: &Ty, to: &Ty, what: &str) {
        if from.is_error() || to.is_error() {
            return;
        }
        // PHP code isn't asked to prove `non-empty-string`, `non-negative-int`, ...
        let widened = widen_refinements(to);
        let to = &widened;
        // a bare `array` is an `array<K, mixed>` of any keys
        if php_array_of_mixed(from, to) {
            return;
        }
        if from.is_union()
            && from.members().iter().any(|m| php_array_of_mixed(m, to))
            && from
                .members()
                .iter()
                .all(|m| php_array_of_mixed(m, to) || self.assignable(m, to))
        {
            return;
        }
        // a template the call couldn't solve says nothing about the value
        if !stir_types::free_params(from).is_empty() || !stir_types::free_params(to).is_empty() {
            return;
        }
        // `$this` in a trait is whichever class uses it
        if self
            .class
            .as_ref()
            .is_some_and(|c| c.kind() == ClassKind::Trait)
            && matches!(from.kind(), TyKind::SelfType { .. })
        {
            return;
        }
        if let Err(m) = stir_types::check_assignable(self.ctx, from, to) {
            let mut msg = format!("type mismatch in {what}: {m}");
            if from.is_any_platform() {
                msg.push_str("; narrow the untyped value first (`is` / `as`)");
            }
            let partial = from.is_union() && from.members().iter().any(|m| self.assignable(m, to));
            let implicit_mixed = self.is_implicitly_mixed(from, to);
            self.error(range, codes::TYPE_MISMATCH, msg);
            if implicit_mixed {
                // PHPStan only checks inside `array` from level 9
                if let Some(d) = self.diags.last_mut() {
                    d.php_level = Some(9);
                }
            } else if partial {
                self.tag_partial_union();
            }
        }
    }

    /// Does `from` fit `to` once the elements of its plain `array`s are ignored?
    /// Also true of an array going to a shape: nothing says which keys it has.
    fn is_implicitly_mixed(&self, from: &Ty, to: &Ty) -> bool {
        let bottom = from.transform(&mut |t| match t.kind() {
            TyKind::Mixed | TyKind::ArrayKey => Some(Ty::never()),
            _ => None,
        });
        self.assignable(&bottom, to)
            || (strip_platform(from)
                .members()
                .iter()
                .all(|m| matches!(m.kind(), TyKind::Array { .. } | TyKind::PhpList { .. }))
                && to
                    .members()
                    .iter()
                    .any(|m| matches!(m.kind(), TyKind::Shape(_) | TyKind::PhpList { .. })))
    }

    // ------------------------------------------------------------ locals

    fn lookup(&self, name: &str) -> Option<&Local> {
        self.frames.iter().rev().find_map(|f| f.get(name))
    }

    fn lookup_mut(&mut self, name: &str) -> Option<&mut Local> {
        self.frames.iter_mut().rev().find_map(|f| f.get_mut(name))
    }

    fn declare(&mut self, name: &str, ty: Ty, kind: LocalKind, range: TextRange) {
        let range = self.name_range(range, name);
        self.def(range, Def::Local { decl: range });
        self.locals.insert(range, (ty.clone(), kind));

        self.frames.last_mut().unwrap().insert(
            SmolStr::new(name),
            Local {
                declared: ty.clone(),
                ty,
                kind,
                range,
            },
        );
    }

    /// Runs `f` for code that may not execute: a PHP variable assigned in it keeps
    /// the types it had as well.
    fn conditional(&mut self, f: impl FnOnce(&mut Self)) {
        self.conditional_depth += 1;
        f(self);
        self.conditional_depth -= 1;
    }

    fn apply_facts(&mut self, facts: &Facts) {
        for (name, ty) in &facts.narrow {
            if let Some(l) = self.lookup_mut(name) {
                // a property found to be null may be filled in by the next call, and treating
                // it as always null would make comparing it with anything look pointless
                l.ty = if name.contains("->") && ty.is_null() {
                    l.declared.clone()
                } else {
                    ty.clone()
                };
            }
        }
    }

    /// Snapshot of current narrowed types (for restoring after a branch).
    fn snapshot(&self) -> Vec<FxHashMap<SmolStr, Local>> {
        self.frames.clone()
    }

    fn restore_types(&mut self, snap: &[FxHashMap<SmolStr, Local>]) {
        for (frame, old) in self.frames.iter_mut().zip(snap) {
            frame.retain(|name, _| !name.contains("->") || old.contains_key(name));
            for (name, local) in frame.iter_mut() {
                if let Some(o) = old.get(name) {
                    local.ty = o.ty.clone();
                }
            }
        }
    }

    /// Like [`Self::restore_types`], keeping property paths that started being tracked.
    fn restore_narrowing(&mut self, snap: &[FxHashMap<SmolStr, Local>]) {
        for (frame, old) in self.frames.iter_mut().zip(snap) {
            for (name, local) in frame.iter_mut() {
                if let Some(o) = old.get(name) {
                    local.ty = o.ty.clone();
                }
            }
        }
    }

    /// Resets narrowings of variables that a loop body may reassign.
    fn widen_all(&mut self) {
        for frame in &mut self.frames {
            for local in frame.values_mut() {
                if local.kind.mutable() {
                    local.ty = local.declared.clone();
                }
            }
        }
    }

    // ------------------------------------------------------------ files

    pub fn check_root(&mut self, root: &SyntaxNode) {
        self.check_declarations();
        self.fns.push(FnCx {
            ret: Ty::unit(),
            infer_ret: true,
            returns: Vec::new(),
        });
        self.stmt_list(root);
        self.fns.pop();
    }

    /// Declaration-level rules (types written on signatures).
    fn check_declarations(&mut self) {
        let items = self.items;
        for (i, f) in items.functions.iter().enumerate() {
            let mut sink = Vec::new();
            let _ = self.ctx.build_sig(
                &format!("check:{i}"),
                &f.name.0,
                &f.sig,
                &LowerCx::default(),
                &mut sink,
            );
            for e in sink {
                self.error(e.range, e.code, e.message);
            }
        }
    }

    fn stmt_list(&mut self, parent: &SyntaxNode) {
        for child in parent.children() {
            if is_stmt_kind(child.kind()) {
                self.stmt(&child);
            } else if is_expr_kind(child.kind()) {
                self.expr(&child, None);
            }
        }
    }

    // ------------------------------------------------------------ statements

    /// Does this statement always leave the enclosing block?
    fn diverges(&self, node: &SyntaxNode) -> bool {
        match node.kind() {
            RETURN_STMT | BREAK_STMT | CONTINUE_STMT | THROW_STMT | GOTO_STMT => true,
            EXPR_STMT => node.children().next().is_some_and(|e| {
                matches!(e.kind(), THROW_EXPR | EXIT_EXPR)
                    || self
                        .types
                        .get(&e.text_range())
                        .is_some_and(|t| t.is_bottom())
            }),
            BLOCK => node.children().any(|s| self.diverges(&s)),
            IF_STMT => {
                let has_else = node.child(ELSE_CLAUSE).is_some();
                has_else
                    && node
                        .children()
                        .filter(|c| {
                            matches!(c.kind(), BLOCK | ELSEIF_CLAUSE | ELSE_CLAUSE)
                                || is_stmt_kind(c.kind())
                        })
                        .all(|c| match c.kind() {
                            ELSEIF_CLAUSE | ELSE_CLAUSE => c
                                .children()
                                .filter(|s| is_stmt_kind(s.kind()))
                                .any(|s| self.diverges(&s)),
                            _ => self.diverges(&c),
                        })
            }
            _ => false,
        }
    }

    pub fn stmt(&mut self, node: &SyntaxNode) {
        match node.kind() {
            EXPR_STMT => {
                if let Some(e) = exprs_of(node).first() {
                    self.expr(e, None);
                }
            }
            ECHO_STMT | ECHO_TAG_STMT => {
                for e in exprs_of(node) {
                    self.expr(&e, None);
                }
            }
            RETURN_STMT => self.return_stmt(node),
            BLOCK => self.block_stmts(node),
            IF_STMT => self.conditional(|this| this.if_stmt(node)),
            WHILE_STMT | DO_WHILE_STMT | FOR_STMT | FOREACH_STMT | SWITCH_STMT | TRY_STMT
                if self.conditional_depth == 0 =>
            {
                self.conditional(|this| this.stmt(node));
            }
            WHILE_STMT => {
                let cond = exprs_of(node).into_iter().next();
                self.widen_all();
                let facts = cond.as_ref().map(|c| self.condition(c)).unwrap_or_default();
                let snap = self.snapshot();
                self.apply_facts(&facts.0);
                for s in node.children().filter(|c| is_stmt_kind(c.kind())) {
                    self.stmt(&s);
                }
                self.restore_types(&snap);
                self.widen_all();
                self.apply_facts(&facts.1);
            }
            DO_WHILE_STMT => {
                self.widen_all();
                for s in node.children().filter(|c| is_stmt_kind(c.kind())) {
                    self.stmt(&s);
                }
                if let Some(c) = exprs_of(node).first() {
                    self.condition(c);
                }
                self.widen_all();
            }
            FOR_STMT => {
                for part in [FOR_INIT, FOR_COND, FOR_STEP] {
                    if let Some(p) = node.child(part) {
                        for e in exprs_of(&p) {
                            if part == FOR_COND {
                                self.condition(&e);
                            } else {
                                self.expr(&e, None);
                            }
                        }
                    }
                }
                self.widen_all();
                for s in node.children().filter(|c| is_stmt_kind(c.kind())) {
                    self.stmt(&s);
                }
                self.widen_all();
            }
            FOREACH_STMT => self.foreach_stmt(node),
            SWITCH_STMT => {
                if let Some(e) = exprs_of(node).first() {
                    self.expr(e, None);
                }
                for case in node.children_of(SWITCH_CASE) {
                    for e in exprs_of(&case) {
                        self.expr(&e, None);
                    }
                    for s in case.children().filter(|c| is_stmt_kind(c.kind())) {
                        self.stmt(&s);
                    }
                }
            }
            BREAK_STMT | CONTINUE_STMT | GOTO_STMT | LABEL_STMT | EMPTY_STMT | INLINE_HTML_STMT
            | USE_DECL | HALT_COMPILER_STMT => {}
            TRY_STMT => {
                let snap = self.snapshot();
                for c in node.children() {
                    match c.kind() {
                        BLOCK => self.stmt(&c),
                        CATCH_CLAUSE => {
                            self.restore_types(&snap);
                            let tys: Vec<Ty> = c
                                .children_of(NAME_REF)
                                .iter()
                                .map(|n| self.class_ref_ty(n, &text_of(n)))
                                .collect();
                            if let Some(v) = c.token(VARIABLE) {
                                self.declare(
                                    &v.text()[1..],
                                    Ty::union(tys),
                                    LocalKind::Loop,
                                    v.text_range(),
                                );
                            }
                            if let Some(b) = c.child(BLOCK) {
                                self.stmt(&b);
                            }
                        }
                        FINALLY_CLAUSE => {
                            if let Some(b) = c.child(BLOCK) {
                                self.stmt(&b);
                            }
                        }
                        _ => {}
                    }
                }
                self.widen_all();
            }
            GLOBAL_STMT => {
                for v in node.descendants().filter(|d| d.kind() == VARIABLE_EXPR) {
                    let name = text_of(&v);
                    self.declare(
                        &name[1..],
                        Ty::any_platform(),
                        LocalKind::Php,
                        v.text_range(),
                    );
                }
            }
            STATIC_VAR_STMT => {
                for sv in node.children_of(STATIC_VAR) {
                    let ty = exprs_of(&sv)
                        .first()
                        .map(|e| widen_literals(&self.expr(e, None)))
                        .unwrap_or_else(Ty::null);
                    if let Some(v) = sv.token(VARIABLE) {
                        self.declare(&v.text()[1..], ty, LocalKind::Php, v.text_range());
                    }
                }
            }
            UNSET_STMT => {
                for e in exprs_of(node) {
                    self.expr(&e, None);
                }
            }
            DECLARE_STMT => {
                if let Some(b) = node.child(BLOCK) {
                    self.stmt(&b);
                }
            }
            NAMESPACE_DECL => {
                if let Some(b) = node.child(BLOCK) {
                    self.stmt_list(&b);
                }
            }
            FUNCTION_DECL => self.function_decl(node),
            CLASS_DECL | INTERFACE_DECL | TRAIT_DECL | ENUM_DECL => self.class_decl(node),
            CONST_DECL => {
                for e in node.children_of(CONST_ELEM) {
                    for x in exprs_of(&e) {
                        self.expr(&x, None);
                    }
                }
            }
            _ => {}
        }
    }

    fn block_stmts(&mut self, block: &SyntaxNode) {
        let stmts: Vec<SyntaxNode> = block
            .children()
            .filter(|c| is_stmt_kind(c.kind()))
            .collect();
        for s in stmts {
            self.stmt(&s);
        }
    }

    fn if_stmt(&mut self, node: &SyntaxNode) {
        let cond = exprs_of(node).into_iter().next();
        let (t, f) = cond.as_ref().map(|c| self.condition(c)).unwrap_or_default();
        let snap = self.snapshot();
        let assigned = self.assigned_places(node);
        self.apply_facts(&t);
        let body: Vec<SyntaxNode> = node.children().filter(|c| is_stmt_kind(c.kind())).collect();
        let mut all_diverge = true;
        for s in &body {
            self.stmt(s);
        }
        let then_diverges = body.iter().any(|s| self.diverges(s));
        all_diverge &= then_diverges;
        self.restore_types(&snap);
        self.apply_facts(&f);
        let mut falls_through_facts = if then_diverges { Some(f.clone()) } else { None };
        let mut has_else = false;
        for clause in node.children() {
            match clause.kind() {
                ELSEIF_CLAUSE => {
                    let c = exprs_of(&clause).into_iter().next();
                    let (t2, f2) = c.as_ref().map(|c| self.condition(c)).unwrap_or_default();
                    let snap2 = self.snapshot();
                    self.apply_facts(&t2);
                    let stmts: Vec<SyntaxNode> = clause
                        .children()
                        .filter(|c| is_stmt_kind(c.kind()))
                        .collect();
                    for s in &stmts {
                        self.stmt(s);
                    }
                    let d = stmts.iter().any(|s| self.diverges(s));
                    all_diverge &= d;
                    self.restore_types(&snap2);
                    self.apply_facts(&f2);
                    if d {
                        if let Some(ff) = &mut falls_through_facts {
                            ff.narrow.extend(f2.narrow.clone());
                        }
                    } else {
                        falls_through_facts = None;
                    }
                }
                ELSE_CLAUSE => {
                    has_else = true;
                    let stmts: Vec<SyntaxNode> = clause
                        .children()
                        .filter(|c| is_stmt_kind(c.kind()))
                        .collect();
                    for s in &stmts {
                        self.stmt(s);
                    }
                    let d = stmts.iter().any(|s| self.diverges(s));
                    all_diverge &= d;
                    if !d {
                        falls_through_facts = None;
                    }
                }
                _ => {}
            }
        }
        // After the if: types are those of the fall-through path, and a variable assigned
        // on some path is back to its declared type, which holds what each path assigned.
        self.restore_types(&snap);
        if !has_else && let Some(ff) = falls_through_facts {
            self.apply_facts(&ff);
        }
        if !all_diverge {
            for name in &assigned {
                if let Some(l) = self.lookup_mut(name) {
                    l.ty = l.declared.clone();
                }
            }
        }
    }

    /// What's assigned anywhere inside `node`.
    fn assigned_places(&self, node: &SyntaxNode) -> Vec<String> {
        node.descendants()
            .filter(|d| d.kind() == ASSIGN_EXPR)
            .filter_map(|a| a.children().next())
            .filter_map(|lhs| self.place_key(&lhs))
            .collect()
    }

    fn foreach_stmt(&mut self, node: &SyntaxNode) {
        let es = exprs_of(node);
        let Some(subject) = es.first() else { return };
        let st = self.expr(subject, None);
        let (key_ty, val_ty) = self.iter_types(subject.text_range(), &st);
        let bindings = &es[1..];
        let (key, val) = match bindings.len() {
            2 => (Some(&bindings[0]), Some(&bindings[1])),
            1 => (None, Some(&bindings[0])),
            _ => (None, None),
        };
        if let Some(k) = key {
            self.bind_target(k, key_ty);
        }
        if let Some(v) = val {
            self.bind_target(v, val_ty);
        }
        self.widen_all();
        for s in node.children().filter(|c| is_stmt_kind(c.kind())) {
            self.stmt(&s);
        }
        self.widen_all();
    }

    /// Binds a foreach/destructuring target.
    fn bind_target(&mut self, target: &SyntaxNode, ty: Ty) {
        match target.kind() {
            VARIABLE_EXPR => {
                let name = text_of(target);
                let n = name.trim_start_matches('$');
                if let Some(l) = self.lookup_mut(n)
                    && l.kind.mutable()
                {
                    l.ty = ty;
                    return;
                }
                self.declare(n, ty, LocalKind::Loop, target.text_range());
            }
            PREFIX_EXPR => {
                if let Some(inner) = exprs_of(target).first() {
                    self.bind_target(inner, ty);
                }
            }
            ARRAY_EXPR | LIST_EXPR => {
                for elem in target.children_of(ARRAY_ELEM) {
                    let parts = exprs_of(&elem);
                    if let Some(last) = parts.last() {
                        let elem_ty = self.index_type(&ty, None).unwrap_or_else(Ty::any_platform);
                        self.bind_target(last, elem_ty);
                    }
                }
            }
            _ => {
                self.expr(target, None);
            }
        }
    }

    fn iter_types(&mut self, range: TextRange, ty: &Ty) -> (Ty, Ty) {
        let ty = strip_platform(ty);
        if ty.is_union() {
            let start = self.diags.len();
            let (mut keys, mut vals, mut missing) = (Vec::new(), Vec::new(), false);
            for m in ty.members().to_vec() {
                let before = self.diags.len();
                let (k, v) = self.iter_types(range, &m);
                if self.diags.len() > before {
                    missing = true;
                } else {
                    keys.push(k);
                    vals.push(v);
                }
            }
            self.diags.truncate(start);
            if missing {
                self.error(
                    range,
                    codes::INVALID_OPERAND,
                    format!("`{ty}` is not iterable"),
                );
                if keys.is_empty() {
                    return (Ty::error(), Ty::error());
                }
                self.tag_partial_union();
            }
            return (Ty::union(keys), Ty::union(vals));
        }
        match ty.kind() {
            TyKind::List(e) | TyKind::MutableList(e) => (Ty::int(), e.clone()),
            TyKind::Map(k, v) | TyKind::MutableMap(k, v) => (k.clone(), v.clone()),
            TyKind::PhpList { elem, .. } if matches!(elem.kind(), TyKind::Never) => {
                (Ty::php_int(), Ty::any_platform())
            }
            TyKind::PhpList { elem, .. } => (Ty::php_int(), elem.clone()),
            TyKind::Array { key, value, .. } | TyKind::Iterable { key, value } => {
                (key.clone(), value.clone())
            }
            TyKind::Shape(_) => (Ty::array_key(), Ty::mixed()),
            TyKind::Error | TyKind::Mixed => (Ty::error(), Ty::error()),
            TyKind::Never | TyKind::Nothing => (Ty::never(), Ty::never()),
            TyKind::Class { name, args } => {
                // Traversable / Iterator / IteratorAggregate
                for anc in self.ctx.ancestors(&Ty::class(name.clone(), args.clone())) {
                    if let TyKind::Class { name: n, args: a } = anc.kind()
                        && matches!(
                            n.to_ascii_lowercase().as_str(),
                            "traversable" | "iterator" | "iteratoraggregate"
                        )
                        && a.len() == 2
                    {
                        return (a[0].clone(), a[1].clone());
                    }
                }
                (Ty::any_platform(), Ty::any_platform())
            }
            _ if ty.is_any_platform() => (Ty::error(), Ty::error()),
            _ => {
                self.error(
                    range,
                    codes::INVALID_OPERAND,
                    format!("`{ty}` is not iterable"),
                );
                (Ty::error(), Ty::error())
            }
        }
    }

    fn return_stmt(&mut self, node: &SyntaxNode) {
        let value = exprs_of(node).into_iter().next();
        let Some(fcx) = self.fns.last().cloned() else {
            return;
        };
        match value {
            Some(v) => {
                let expected = if fcx.infer_ret {
                    None
                } else {
                    Some(fcx.ret.clone())
                };
                let t = self.expr(&v, expected.as_ref());
                if fcx.infer_ret {
                    self.fns.last_mut().unwrap().returns.push(t);
                } else if matches!(fcx.ret.kind(), TyKind::Unit | TyKind::Void) {
                    if !t.is_null() {
                        self.error(
                            v.text_range(),
                            codes::RETURN_VALUE,
                            "this function returns `Unit`; it can't return a value",
                        );
                    }
                } else if !in_generator(node) {
                    self.check_assign(v.text_range(), &t, &fcx.ret, "return value");
                }
            }
            None => {
                if fcx.infer_ret {
                    self.fns.last_mut().unwrap().returns.push(Ty::unit());
                }
            }
        }
    }

    // ------------------------------------------------------------ functions

    fn function_decl(&mut self, node: &SyntaxNode) {
        let ptr = stir_hir::NodePtr::new(node);
        let items = self.items;
        let Some(idx) = items.functions.iter().position(|f| f.ptr == ptr) else {
            return;
        };
        let sig = self.ctx.function_sig(stir_hir::ItemLoc {
            file: self.file,
            index: idx as u32,
        });
        let saved_cx = self.lower_cx.clone();
        self.lower_cx.generics.extend(
            sig.type_params
                .iter()
                .map(|p| (p.name.clone(), p.ty.clone())),
        );
        self.function_body(node, &sig, None);
        self.lower_cx = saved_cx;
    }

    /// Checks a function/method body against its signature.
    fn function_body(&mut self, node: &SyntaxNode, sig: &FnSig, this: Option<Ty>) {
        let saved_frames = std::mem::replace(&mut self.frames, vec![FxHashMap::default()]);
        let saved_dynamic_vars = self.dynamic_vars_fn.take();
        if let Some(t) = this {
            self.frames.last_mut().unwrap().insert(
                SmolStr::new("this"),
                Local {
                    ty: t.clone(),
                    declared: t,
                    kind: LocalKind::This,
                    range: node.text_range(),
                },
            );
        }
        // default values, checked against their parameter types
        if let Some(list) = node.child(PARAM_LIST) {
            for (p, pnode) in sig.params.iter().zip(list.children_of(PARAM)) {
                if let Some(d) = exprs_of(&pnode).into_iter().next() {
                    let dt = self.expr(&d, Some(&p.ty));
                    self.check_assign(d.text_range(), &dt, &p.ty, "default value");
                }
            }
        }
        for p in &sig.params {
            let mut ty = p.ty.clone();
            if p.variadic {
                ty = Ty::php_list(ty);
            }
            self.declare(&p.name, ty, LocalKind::Param, p.range);
        }
        self.fns.push(FnCx {
            ret: sig.ret.clone(),
            infer_ret: false,
            returns: Vec::new(),
        });
        if let Some(block) = node.child(BLOCK) {
            self.block_stmts(&block);
        }
        self.dynamic_vars_fn = saved_dynamic_vars;
        self.fns.pop();
        self.frames = saved_frames;
    }

    fn class_decl(&mut self, node: &SyntaxNode) {
        let ptr = stir_hir::NodePtr::new(node);
        let items = self.items;
        let Some(def) = items.classes.iter().find(|c| c.ptr == ptr) else {
            return;
        };
        let Some(model) = self.ctx.class(def.name.as_str()) else {
            return;
        };
        let saved_class = self.class.replace(model.clone());
        let saved_cx = std::mem::replace(&mut self.lower_cx, self.ctx.class_cx(&model));

        self.check_class_header(&model);
        self.check_variance(&model);
        self.check_class_rules(&model);

        if let Some(body) = node.child(CLASS_BODY) {
            for member in body.children() {
                match member.kind() {
                    METHOD_DECL => self.method_decl(&member, &model),
                    PROPERTY_DECL => self.property_decl(&member, &model),
                    CLASS_CONST_DECL | ENUM_CASE => {
                        for e in member.descendants().filter(|d| {
                            is_expr_kind(d.kind())
                                && d.parent()
                                    .is_some_and(|p| matches!(p.kind(), CONST_ELEM | ENUM_CASE))
                        }) {
                            self.expr(&e, None);
                        }
                    }
                    _ => {}
                }
            }
        }
        self.lower_cx = saved_cx;
        self.class = saved_class;
    }

    fn check_class_header(&mut self, model: &Rc<ClassModel<'db>>) {
        let def = model.def;
        let mut sink = Vec::new();
        let cx = self.ctx.class_cx(model);
        for t in def.extends.iter().chain(&def.implements) {
            self.ctx.lower(t, &cx, &mut sink);
        }
        for e in sink {
            self.error(e.range, e.code, e.message);
        }
    }

    fn method_decl(&mut self, node: &SyntaxNode, model: &Rc<ClassModel<'db>>) {
        let ptr = stir_hir::NodePtr::new(node);
        let Some(m) = model.def.methods.iter().find(|m| m.ptr == ptr) else {
            return;
        };
        let sig = self.ctx.method_sig(model, m);
        let mut sink = Vec::new();
        // report type errors written on the method signature
        let _ = self.ctx.build_sig(
            "m:check",
            &m.name,
            &m.sig,
            &self.ctx.class_cx(model),
            &mut sink,
        );
        for e in sink {
            self.error(e.range, e.code, e.message);
        }
        let saved_cx = self.lower_cx.clone();
        self.lower_cx.generics.extend(
            sig.type_params
                .iter()
                .map(|p| (p.name.clone(), p.ty.clone())),
        );
        self.in_ctor = m.name.eq_ignore_ascii_case("__construct");
        let this = if m.mods.is_static {
            None
        } else {
            Some(Ty::this_of(model.self_ty()))
        };
        if m.has_body {
            self.function_body(node, &sig, this);
        }
        self.in_ctor = false;
        self.lower_cx = saved_cx;
    }

    fn property_decl(&mut self, node: &SyntaxNode, model: &Rc<ClassModel<'db>>) {
        let declared = stir_hir::first_type(node).map(|t| self.lower_type_node(&t));
        for elem in node.children_of(PROPERTY_ELEM) {
            let ty = declared
                .clone()
                .or_else(|| stir_hir::first_type(&elem).map(|t| self.lower_type_node(&t)));
            if let Some(t) = &ty
                && t.contains_any_platform()
            {
                self.error(
                    elem.text_range(),
                    codes::ANY_PLATFORM_ESCAPE,
                    "properties can't be untyped (`Any!`)",
                );
            }
            for e in exprs_of(&elem) {
                let vt = self.expr(&e, ty.as_ref());
                if let Some(t) = &ty {
                    self.check_assign(e.text_range(), &vt, t, "property initializer");
                }
            }
        }
        let _ = model;
    }

    // ------------------------------------------------------------ conditions

    /// Checks a condition and returns (facts when true, facts when false).
    fn condition(&mut self, node: &SyntaxNode) -> (Facts, Facts) {
        self.expr(node, Some(&Ty::bool()));
        self.facts(node)
    }

    fn facts(&mut self, node: &SyntaxNode) -> (Facts, Facts) {
        match node.kind() {
            PAREN_EXPR => match exprs_of(node).first() {
                Some(inner) => self.facts(inner),
                None => Default::default(),
            },
            // `!empty($x)` rules out null and false
            EMPTY_EXPR => {
                let arg = node
                    .child(ARG_LIST)
                    .and_then(|l| l.child(ARG))
                    .and_then(|a| exprs_of(&a).into_iter().next());
                let Some((name, local)) = arg.and_then(|a| self.local_of(&a)) else {
                    return Default::default();
                };
                let no_false = local
                    .ty
                    .members()
                    .iter()
                    .filter(|m| !matches!(m.kind(), TyKind::Literal(l) if matches!(l.value, LitValue::Bool(false))))
                    .cloned()
                    .collect::<Vec<_>>();
                (
                    Facts::default(),
                    Facts {
                        narrow: vec![(name, stir_types::without_null(&Ty::union(no_false)))],
                    },
                )
            }
            // `isset($x)` rules out null
            ISSET_EXPR => {
                let mut narrow = Vec::new();
                for arg in node.child(ARG_LIST).iter().flat_map(|l| l.children_of(ARG)) {
                    if let Some((name, local)) =
                        exprs_of(&arg).first().and_then(|e| self.local_of(e))
                    {
                        narrow.push((name, stir_types::without_null(&local.ty)));
                    }
                }
                (Facts { narrow }, Facts::default())
            }
            PREFIX_EXPR if op_token(node).is_some_and(|t| t.kind() == BANG) => {
                match exprs_of(node).first() {
                    Some(inner) => {
                        let (t, f) = self.facts(inner);
                        (f, t)
                    }
                    None => Default::default(),
                }
            }
            BIN_EXPR => {
                let op = node
                    .children_with_tokens()
                    .filter_map(|e| e.into_token())
                    .find(|t| !t.kind().is_trivia())
                    .map(|t| t.kind());
                let es = exprs_of(node);
                if es.len() != 2 {
                    return Default::default();
                }
                match op {
                    Some(AMP2 | AND_KW) => {
                        let (lt, lf) = self.facts(&es[0]);
                        // the right side only runs (and narrows) if the left was true
                        let snap = self.snapshot();
                        self.apply_facts(&lt);
                        let (rt, rf) = self.facts(&es[1]);
                        self.restore_narrowing(&snap);
                        let _ = (lf, rf);
                        (
                            Facts {
                                narrow: [lt.narrow, rt.narrow].concat(),
                            },
                            Facts::default(),
                        )
                    }
                    Some(PIPE2 | OR_KW) => {
                        let (_, lf) = self.facts(&es[0]);
                        // the right side only runs (and narrows) if the left was false
                        let snap = self.snapshot();
                        self.apply_facts(&lf);
                        let (_, rf) = self.facts(&es[1]);
                        self.restore_narrowing(&snap);
                        (
                            Facts::default(),
                            Facts {
                                narrow: [lf.narrow, rf.narrow].concat(),
                            },
                        )
                    }
                    Some(k @ (EQ2 | EQ3 | NEQ | NEQ2)) => {
                        let positive = matches!(k, EQ2 | EQ3);
                        let (var, other) = if is_null_literal(&es[1]) {
                            (&es[0], &es[1])
                        } else if is_null_literal(&es[0]) {
                            (&es[1], &es[0])
                        } else {
                            return self.literal_facts(&es[0], &es[1], positive);
                        };
                        let _ = other;
                        let Some((name, local)) = self.local_of(var) else {
                            return Default::default();
                        };
                        let non_null = stir_types::without_null(&local.ty);
                        let is_null = (
                            Facts {
                                narrow: vec![(name.clone(), Ty::null())],
                            },
                            Facts {
                                narrow: vec![(name, non_null)],
                            },
                        );
                        if positive {
                            is_null
                        } else {
                            (is_null.1, is_null.0)
                        }
                    }
                    _ => Default::default(),
                }
            }
            INSTANCEOF_EXPR => {
                let es = exprs_of(node);
                let Some(subject) = es.first() else {
                    return Default::default();
                };
                let Some((name, local)) = self.local_of(subject) else {
                    return Default::default();
                };
                let test = match node.children().nth(1) {
                    Some(r) if r.kind() == NAME_EXPR => {
                        let n = text_of(&r);
                        self.class_ref_ty(&r, &n)
                    }
                    _ => return Default::default(),
                };
                let yes = stir_types::narrow_to(self.ctx, &local.ty, &test);
                let no = stir_types::narrow_away(self.ctx, &local.ty, &test);
                (
                    Facts {
                        narrow: vec![(name.clone(), yes)],
                    },
                    Facts {
                        narrow: vec![(name, no)],
                    },
                )
            }
            CALL_EXPR => {
                // is_int($x), is_string($x), ...
                let Some(callee) = node.child(NAME_EXPR) else {
                    return Default::default();
                };
                let fname = text_of(&callee)
                    .trim_start_matches('\\')
                    .to_ascii_lowercase();
                let test = match fname.as_str() {
                    "is_int" | "is_integer" | "is_long" => Ty::php_int(),
                    "is_float" | "is_double" => Ty::php_float(),
                    "is_string" => Ty::php_string(),
                    "is_bool" => Ty::php_bool(),
                    "is_null" => Ty::null(),
                    "is_array" => Ty::php_array(),
                    "is_object" => Ty::object(),
                    "is_resource" => Ty::resource(),
                    // anything callable isn't null; what else it might be, only the value knows
                    "is_callable" => {
                        let arg = node
                            .child(ARG_LIST)
                            .and_then(|a| a.child(ARG))
                            .and_then(|a| exprs_of(&a).into_iter().next());
                        let Some((name, local)) = arg.and_then(|a| self.local_of(&a)) else {
                            return Default::default();
                        };
                        return (
                            Facts {
                                narrow: vec![(name, stir_types::without_null(&local.ty))],
                            },
                            Facts::default(),
                        );
                    }
                    _ => return self.assert_facts(node, &text_of(&callee)),
                };
                let Some(arg) = node
                    .child(ARG_LIST)
                    .and_then(|a| a.child(ARG))
                    .and_then(|a| exprs_of(&a).into_iter().next())
                else {
                    return Default::default();
                };
                let Some((name, local)) = self.local_of(&arg) else {
                    return Default::default();
                };
                let yes = stir_types::narrow_to(self.ctx, &local.ty, &test);
                let no = stir_types::narrow_away(self.ctx, &local.ty, &test);
                (
                    Facts {
                        narrow: vec![(name.clone(), yes)],
                    },
                    Facts {
                        narrow: vec![(name, no)],
                    },
                )
            }
            VARIABLE_EXPR => {
                // truthiness: `if ($x)` removes null
                let Some((name, local)) = self.local_of(node) else {
                    return Default::default();
                };
                (
                    Facts {
                        narrow: vec![(name, stir_types::without_null(&local.ty))],
                    },
                    Facts::default(),
                )
            }
            _ => Default::default(),
        }
    }

    /// `x == "a"` narrows a literal union.
    fn literal_facts(&mut self, a: &SyntaxNode, b: &SyntaxNode, positive: bool) -> (Facts, Facts) {
        let (var, lit) = if self.local_of(a).is_some() {
            (a, b)
        } else {
            (b, a)
        };
        let Some((name, local)) = self.local_of(var) else {
            return Default::default();
        };
        let Some(lt) = self.types.get(&lit.text_range()).cloned() else {
            return Default::default();
        };
        if !matches!(lt.kind(), TyKind::Literal(_)) {
            return Default::default();
        }
        let yes = stir_types::narrow_to(self.ctx, &local.ty, &lt);
        let no = stir_types::narrow_away(self.ctx, &local.ty, &lt);
        let t = Facts {
            narrow: vec![(name.clone(), yes)],
        };
        let f = Facts {
            narrow: vec![(name, no)],
        };
        if positive { (t, f) } else { (f, t) }
    }

    fn local_of(&mut self, node: &SyntaxNode) -> Option<(SmolStr, Local)> {
        let name = match node.kind() {
            VARIABLE_EXPR => text_of(node).trim_start_matches('$').to_owned(),
            PAREN_EXPR => return exprs_of(node).first().and_then(|e| self.local_of(e)),
            PROPERTY_FETCH_EXPR => return self.property_place(node),
            _ => return None,
        };
        self.lookup(&name).map(|l| (SmolStr::new(&name), l.clone()))
    }

    /// `$this->x` / `$a->b->c` as something narrowing can track, the way TypeScript does: it
    /// stays narrowed until it, or something it's reached through, is assigned. Calls don't
    /// reset it. It's held like a local under a name no variable can have.
    fn property_place(&mut self, node: &SyntaxNode) -> Option<(SmolStr, Local)> {
        let key = self.place_key(node)?;
        if let Some(l) = self.lookup(&key) {
            return Some((SmolStr::new(&key), l.clone()));
        }
        let ty = self.types.get(&node.text_range())?.clone();
        let local = Local {
            ty: ty.clone(),
            declared: ty,
            kind: LocalKind::Php,
            range: node.text_range(),
        };
        // the outermost frame, so it outlives the block it was narrowed in
        self.frames
            .first_mut()?
            .insert(SmolStr::new(&key), local.clone());
        Some((SmolStr::new(&key), local))
    }

    /// What a call to a function with `@phpstan-assert-if-true` says about its arguments.
    fn assert_facts(&mut self, node: &SyntaxNode, callee: &str) -> (Facts, Facts) {
        let scope = self.scope(node);
        let r = scope.resolve_function(callee);
        let Some(loc) = self.ctx.function(r.primary.as_str()).or_else(|| {
            r.fallback
                .as_ref()
                .and_then(|f| self.ctx.function(f.as_str()))
        }) else {
            return Default::default();
        };
        let def = self.ctx.function_def(loc);
        let args = self.args_of(node);
        let cx = LowerCx::default();
        let (mut when_true, mut when_false) = (Facts::default(), Facts::default());
        for assert in &def.sig.asserts {
            let Some(index) = def.sig.params.iter().position(|p| p.name == assert.param) else {
                continue;
            };
            let Some((name, local)) = args
                .get(index)
                .and_then(|a| exprs_of(a).into_iter().next())
                .and_then(|e| self.local_of(&e))
            else {
                continue;
            };
            let mut sink = Vec::new();
            let test = self.ctx.lower(&assert.ty, &cx, &mut sink);
            let yes = stir_types::narrow_to(self.ctx, &local.ty, &test);
            let no = stir_types::narrow_away(self.ctx, &local.ty, &test);
            let (on_true, on_false) = if assert.when { (yes, no) } else { (no, yes) };
            when_true.narrow.push((name.clone(), on_true));
            when_false.narrow.push((name, on_false));
        }
        (when_true, when_false)
    }

    /// The name a variable or property path is tracked under.
    fn place_key(&self, node: &SyntaxNode) -> Option<String> {
        match node.kind() {
            VARIABLE_EXPR => Some(text_of(node).trim_start_matches('$').to_owned()),
            PROPERTY_FETCH_EXPR => {
                let base = exprs_of(node).into_iter().next()?;
                let name = node.child(SyntaxKind::NAME).map(|n| text_of(&n))?;
                Some(format!("{}->{name}", self.place_key(&base)?))
            }
            _ => None,
        }
    }

    /// Assigning to `$a` forgets what was known about `$a->b`.
    fn forget_properties_of(&mut self, key: &str) {
        let prefix = format!("{key}->");
        for frame in &mut self.frames {
            frame.retain(|name, _| !name.starts_with(&prefix));
        }
    }

    // ------------------------------------------------------------ expressions

    /// Infers the type of an expression, checking it against `expected` where
    /// that helps inference (the caller still checks assignability).
    pub fn expr(&mut self, node: &SyntaxNode, expected: Option<&Ty>) -> Ty {
        let ty = self.expr_inner(node, expected);
        self.types.insert(node.text_range(), ty.clone());
        ty
    }

    fn expr_inner(&mut self, node: &SyntaxNode, expected: Option<&Ty>) -> Ty {
        match node.kind() {
            LITERAL => {
                let Some(tok) = op_token(node) else {
                    return Ty::error();
                };
                match tok.kind() {
                    INT_NUMBER => match parse_int(tok.text()) {
                        Some(v) => Ty::literal(LitValue::Int(v), Family::Php),
                        // overflowing integer literals are floats in PHP
                        None => Ty::php_float(),
                    },
                    FLOAT_NUMBER => match tok.text().replace('_', "").parse::<f64>() {
                        Ok(v) => {
                            Ty::literal(LitValue::Float(stir_types::FloatLit::new(v)), Family::Php)
                        }
                        Err(_) => Ty::error(),
                    },
                    _ => Ty::error(),
                }
            }
            STRING_LITERAL => {
                let text = text_of(node);
                Ty::literal(LitValue::String(SmolStr::new(unquote(&text))), Family::Php)
            }
            INTERP_STRING | HEREDOC => {
                for part in node.children() {
                    if matches!(part.kind(), INTERP_EXPR | INTERP_VAR) {
                        for e in exprs_of(&part) {
                            self.expr(&e, None);
                        }
                    }
                }
                Ty::php_string()
            }
            SHELL_EXEC => {
                for part in node.children() {
                    for e in exprs_of(&part) {
                        self.expr(&e, None);
                    }
                }
                Ty::union([
                    Ty::php_string(),
                    Ty::null(),
                    Ty::literal(LitValue::Bool(false), Family::Php),
                ])
            }
            MAGIC_CONST => match op_token(node).map(|t| t.kind()) {
                Some(LINE_KW) => Ty::php_int(),
                _ => Ty::php_string(),
            },
            PAREN_EXPR => match exprs_of(node).first() {
                Some(e) => self.expr(e, expected),
                None => Ty::error(),
            },
            VARIABLE_EXPR => self.variable(node),
            VARIABLE_VARIABLE => {
                for e in exprs_of(node) {
                    self.expr(&e, None);
                }

                Ty::any_platform()
            }
            NAME_EXPR => self.name_expr(node),
            BIN_EXPR => self.binary(node, expected),
            PREFIX_EXPR => self.prefix(node, expected),
            POSTFIX_EXPR => {
                let e = exprs_of(node);
                let Some(target) = e.first() else {
                    return Ty::error();
                };
                let t = self.expr(target, None);
                self.check_mutable_target(target);
                widen_literals(&t)
            }
            ASSIGN_EXPR => self.assign(node),
            TERNARY_EXPR => {
                let es = exprs_of(node);
                if es.len() < 3 {
                    for e in &es {
                        self.expr(e, None);
                    }
                    return Ty::error();
                }
                let (t, f) = self.condition(&es[0]);
                let snap = self.snapshot();
                self.apply_facts(&t);
                let a = self.expr(&es[1], expected);
                self.restore_types(&snap);
                self.apply_facts(&f);
                let b = self.expr(&es[2], expected);
                self.restore_types(&snap);
                stir_types::join(self.ctx, &a, &b)
            }
            ELVIS_EXPR => {
                let es = exprs_of(node);
                if es.len() < 2 {
                    return Ty::error();
                }
                let l = self.expr(&es[0], expected.map(|e| e.clone().nullable()).as_ref());
                let r = self.expr(&es[1], expected);
                stir_types::join(self.ctx, &l, &r)
            }
            // `foreach ($xs as T as $x)`: the parser takes the first `as` for a cast
            AS_EXPR => {
                let es = exprs_of(node);
                let src = es
                    .first()
                    .map(|e| self.expr(e, None))
                    .unwrap_or_else(Ty::error);
                let Some(tn) = stir_hir::first_type(node) else {
                    return Ty::error();
                };
                let target = self.lower_type_node(&tn);
                if !src.is_error()
                    && !target.is_error()
                    && !src.is_any_platform()
                    && !matches!(src.kind(), TyKind::Platform(_))
                {
                    // array types overlap with each other (a map may hold a list)
                    let arrays = |t: &Ty| {
                        t.members().iter().all(|m| {
                            stir_types::php_view(m)
                                .or_else(|| Some(m.clone()))
                                .is_some_and(|v| {
                                    matches!(
                                        v.kind(),
                                        TyKind::Array { .. }
                                            | TyKind::PhpList { .. }
                                            | TyKind::Shape(_)
                                    )
                                })
                        })
                    };
                    let overlap = (arrays(&src) && arrays(&target))
                        || stir_types::is_assignable(self.ctx, &target, &src)
                        || stir_types::is_assignable(self.ctx, &src, &target)
                        || stir_types::are_comparable(self.ctx, &src, &target);
                    if !overlap {
                        self.warn(
                            node.text_range(),
                            codes::TYPE_MISMATCH,
                            format!("cast from `{src}` to `{target}` can never succeed"),
                        );
                    }
                }
                target
            }
            INSTANCEOF_EXPR => {
                if let Some(e) = exprs_of(node).first() {
                    self.expr(e, None);
                }
                if let Some(r) = node.children().nth(1) {
                    if r.kind() == NAME_EXPR {
                        let n = text_of(&r);
                        self.class_ref_ty(&r, &n);
                    } else {
                        self.expr(&r, None);
                    }
                }
                Ty::php_bool()
            }
            CAST_EXPR => {
                if let Some(e) = exprs_of(node).first() {
                    self.expr(e, None);
                }
                match op_token(node).map(|t| t.kind()) {
                    Some(INT_CAST) => Ty::php_int(),
                    Some(FLOAT_CAST) => Ty::php_float(),
                    Some(STRING_CAST) => Ty::php_string(),
                    Some(BOOL_CAST) => Ty::php_bool(),
                    Some(ARRAY_CAST) => Ty::php_array(),
                    Some(OBJECT_CAST) => Ty::class("stdClass", Vec::new()),
                    Some(VOID_CAST) => Ty::void(),
                    _ => Ty::null(),
                }
            }
            CALL_EXPR => self.call(node, expected),
            METHOD_CALL_EXPR | NULLSAFE_METHOD_CALL_EXPR => self.method_call(node, expected),
            PROPERTY_FETCH_EXPR | NULLSAFE_PROPERTY_FETCH_EXPR => self.property_fetch(node),
            STATIC_CALL_EXPR => self.static_call(node, expected),
            STATIC_PROPERTY_FETCH_EXPR => self.static_property(node),
            CLASS_CONST_FETCH_EXPR => self.class_const(node),
            INDEX_EXPR => {
                let es = exprs_of(node);
                let Some(base) = es.first() else {
                    return Ty::error();
                };
                let bt = self.expr(base, None);
                let idx = es.get(1);
                let it = idx.map(|i| self.expr(i, None));
                // `$g[k][] = x`: the inner `$g[k]` is written through, not read
                let written_through = node.parent().is_some_and(|p| {
                    p.kind() == INDEX_EXPR
                        && exprs_of(&p).first().is_some_and(|f| f == node)
                        && is_write_target(&p)
                });
                let bt = if written_through {
                    stir_types::without_null(&bt)
                } else {
                    bt
                };

                let indexed = strip_platform(&stir_types::without_null(&bt));
                if indexed.is_union() {
                    let (ok, bad): (Vec<Ty>, Vec<Ty>) = indexed
                        .members()
                        .iter()
                        .cloned()
                        .partition(|m| self.index_type(m, it.as_ref()).is_some());
                    if !ok.is_empty() {
                        if !bad.is_empty() {
                            self.error(
                                base.text_range(),
                                codes::INVALID_OPERAND,
                                format!("`{bt}` can't be indexed"),
                            );
                            self.tag_partial_union();
                        }
                        return Ty::union(
                            ok.iter().filter_map(|m| self.index_type(m, it.as_ref())),
                        );
                    }
                }
                match self.index_type(&indexed, it.as_ref()) {
                    Some(t) => t,
                    None => {
                        if !bt.is_error() && !bt.is_any_platform() {
                            self.error(
                                base.text_range(),
                                codes::INVALID_OPERAND,
                                format!("`{bt}` can't be indexed"),
                            );
                        }
                        Ty::error()
                    }
                }
            }
            ARRAY_EXPR => self.array_literal(node, expected),
            LIST_EXPR => Ty::any_platform(),
            NEW_EXPR => self.new_expr(node, expected),
            CLONE_EXPR => {
                let es = exprs_of(node);
                let e = es.first().or(None);
                match e {
                    Some(e) => self.expr(e, None),
                    None => {
                        if let Some(args) = node.child(ARG_LIST) {
                            let first = args
                                .child(ARG)
                                .and_then(|a| exprs_of(&a).into_iter().next());
                            first.map(|f| self.expr(&f, None)).unwrap_or_else(Ty::error)
                        } else {
                            Ty::error()
                        }
                    }
                }
            }
            CLOSURE_EXPR | ARROW_FN_EXPR => self.closure(node, expected),
            MATCH_EXPR => self.match_expr(node, expected),
            ISSET_EXPR | EMPTY_EXPR => {
                self.probing += 1;
                if let Some(args) = node.child(ARG_LIST) {
                    for a in args.children_of(ARG) {
                        for e in exprs_of(&a) {
                            self.expr(&e, None);
                        }
                    }
                }
                self.probing -= 1;
                Ty::php_bool()
            }
            EVAL_EXPR | INCLUDE_EXPR => {
                self.dynamic_vars_fn = Some(self.fns.len());
                for e in exprs_of(node) {
                    self.expr(&e, None);
                }
                if let Some(args) = node.child(ARG_LIST) {
                    for a in args.children_of(ARG) {
                        for e in exprs_of(&a) {
                            self.expr(&e, None);
                        }
                    }
                }
                Ty::any_platform()
            }
            EXIT_EXPR => {
                if let Some(args) = node.child(ARG_LIST) {
                    for a in args.children_of(ARG) {
                        for e in exprs_of(&a) {
                            self.expr(&e, None);
                        }
                    }
                }
                Ty::nothing()
            }
            PRINT_EXPR => {
                for e in exprs_of(node) {
                    self.expr(&e, None);
                }
                Ty::literal(LitValue::Int(1), Family::Php)
            }
            YIELD_EXPR | YIELD_FROM_EXPR => {
                for e in exprs_of(node) {
                    self.expr(&e, None);
                }
                Ty::any_platform()
            }
            THROW_EXPR => {
                for e in exprs_of(node) {
                    let t = self.expr(&e, None);
                    if !t.is_error()
                        && !t.is_any_platform()
                        && !self.assignable(&t, &Ty::class("Throwable", Vec::new()))
                    {
                        self.error(
                            e.text_range(),
                            codes::TYPE_MISMATCH,
                            format!("only `Throwable`s can be thrown, found `{t}`"),
                        );
                    }
                }
                Ty::nothing()
            }
            ERROR_SUPPRESS_EXPR => match exprs_of(node).first() {
                Some(e) => self.expr(e, expected),
                None => Ty::error(),
            },
            _ => Ty::error(),
        }
    }

    fn variable(&mut self, node: &SyntaxNode) -> Ty {
        let text = text_of(node);
        let name = text.trim_start_matches('$');
        if name == "this" {
            if let Some(l) = self.lookup("this")
                && l.kind == LocalKind::This
            {
                return l.ty.clone();
            }
            self.error(
                node.text_range(),
                codes::UNDEFINED_VARIABLE,
                "`$this` is only available inside instance methods",
            );
            return Ty::error();
        }
        match self.lookup(name).cloned() {
            Some(l) => {
                self.def(node.text_range(), Def::Local { decl: l.range });

                l.ty
            }
            None => {
                if is_superglobal(name) {
                    return Ty::any_platform();
                }
                if self.dynamic_vars_fn == Some(self.fns.len()) {
                    self.declare(name, Ty::any_platform(), LocalKind::Php, node.text_range());
                    return Ty::any_platform();
                }
                if self.probing > 0 {
                    // from here on, it may exist
                    self.declare(name, Ty::any_platform(), LocalKind::Php, node.text_range());
                    return Ty::any_platform();
                }
                if self.fns.len() > 1 {
                    self.error(
                        node.text_range(),
                        codes::UNDEFINED_VARIABLE,
                        format!("undefined variable `${name}`"),
                    );
                    Ty::error()
                } else {
                    // top-level PHP code: unknown globals
                    Ty::any_platform()
                }
            }
        }
    }

    /// Does this file test for `name` with `defined('name')`, `function_exists('name')`?
    /// Such a symbol is expected to be missing in some environments.
    fn guarded_by(&self, check: &str, name: &str) -> bool {
        let text = self.file.text(self.ctx.db);
        let name = name.trim_start_matches('\\');
        [format!("'{name}'"), format!("\"{name}\"")]
            .iter()
            .any(|quoted| {
                text.match_indices(quoted.as_str()).any(|(at, _)| {
                    text[..at]
                        .trim_end()
                        .strip_suffix('(')
                        .is_some_and(|before| before.trim_end().ends_with(check))
                })
            })
    }

    fn name_expr(&mut self, node: &SyntaxNode) -> Ty {
        let text = text_of(node);
        match text.to_ascii_lowercase().as_str() {
            "true" => return Ty::literal(LitValue::Bool(true), Family::Php),
            "false" => return Ty::literal(LitValue::Bool(false), Family::Php),
            "null" => return Ty::null(),
            _ => {}
        }

        // a constant
        let scope = self.scope(node);
        let resolved = scope.resolve_const(&text);
        let loc = self.ctx.constant(resolved.primary.as_str()).or_else(|| {
            resolved
                .fallback
                .as_ref()
                .and_then(|f| self.ctx.constant(f.as_str()))
        });
        match loc {
            Some(loc) => {
                self.def(node.text_range(), Def::Const(loc));
                self.const_type(loc)
            }
            None if self.guarded_by("defined", &text) => Ty::any_platform(),
            None => {
                self.error(
                    node.text_range(),
                    codes::UNDEFINED_VARIABLE,
                    format!("unknown name `{text}`"),
                );
                Ty::error()
            }
        }
    }

    fn const_type(&mut self, loc: stir_hir::ItemLoc) -> Ty {
        let items = self.ctx.items(loc.file);
        let c = &items.consts[loc.index as usize];
        let Some(ptr) = &c.value else {
            return Ty::any_platform();
        };
        let root = stir_db::parse(self.ctx.db, loc.file).syntax();
        let node = ptr.to_node(&root);
        // literal constants only (no evaluation)
        let (int, float, boolean) = (Ty::php_int(), Ty::php_float(), Ty::php_bool());
        match node.kind() {
            LITERAL => match op_token(&node).map(|t| (t.kind(), t.text().to_owned())) {
                Some((INT_NUMBER, _)) => int,
                Some((FLOAT_NUMBER, _)) => float,
                _ => Ty::any_platform(),
            },
            STRING_LITERAL => match unquoted_literal(&node) {
                Some(value) => Ty::literal(LitValue::String(value.into()), Family::Php),
                None => Ty::php_string(),
            },
            INTERP_STRING => Ty::php_string(),
            NAME_EXPR => match text_of(&node).to_ascii_lowercase().as_str() {
                "true" | "false" => boolean,
                "null" => Ty::null(),
                _ => Ty::any_platform(),
            },
            PREFIX_EXPR => {
                // `-9223372036854775808` (PHP_INT_MIN) is an int, not a float
                let inner = node.children().next();
                let lit = inner
                    .as_ref()
                    .and_then(op_token)
                    .map(|t| (t.kind(), t.text().to_owned()));
                match lit {
                    Some((INT_NUMBER, _)) => int,
                    Some((FLOAT_NUMBER, t)) if t == "9223372036854775808" => int,
                    Some((FLOAT_NUMBER, _)) => float,
                    _ => Ty::union([int, float]),
                }
            }
            _ => Ty::any_platform(),
        }
    }

    // ------------------------------------------------------------ operators

    fn binary(&mut self, node: &SyntaxNode, expected: Option<&Ty>) -> Ty {
        let es = exprs_of(node);
        let op = node
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| !t.kind().is_trivia())
            .map(|t| t.kind());
        let Some(op) = op else {
            return Ty::error();
        };
        if es.len() != 2 {
            for e in &es {
                self.expr(e, None);
            }
            return Ty::error();
        }
        // short-circuit narrowing: `x != null && x->foo()`
        let (l, r) = match op {
            AMP2 | AND_KW | PIPE2 | OR_KW => {
                let l = self.expr(&es[0], None);
                let (t, f) = self.facts(&es[0]);
                let snap = self.snapshot();
                self.apply_facts(if matches!(op, AMP2 | AND_KW) { &t } else { &f });
                let r = self.expr(&es[1], None);
                self.restore_types(&snap);
                (l, r)
            }
            QUESTION2 => {
                self.probing += 1;
                let l = self.expr(&es[0], expected.map(|e| e.clone().nullable()).as_ref());
                self.probing -= 1;
                // `$m[$k] ?? []`: the fallback takes the left side's type
                let fallback = stir_types::without_null(&l);
                let r = self.expr(&es[1], expected.or((!l.is_error()).then_some(&fallback)));
                (l, r)
            }
            _ => (self.expr(&es[0], None), self.expr(&es[1], None)),
        };
        if l.is_error() || r.is_error() {
            return match op {
                EQ2 | EQ3 | NEQ | NEQ2 | L_ANGLE | R_ANGLE | LTEQ | GTEQ | AMP2 | PIPE2 => {
                    Ty::php_bool()
                }
                _ => Ty::error(),
            };
        }
        match op {
            PLUS | MINUS | STAR | SLASH | PERCENT | POW => self.arith_result(op, &l, &r),
            DOT => Ty::php_string(),
            SPACESHIP => Ty::php_int(),
            EQ2 | EQ3 | NEQ | NEQ2 | L_ANGLE | R_ANGLE | LTEQ | GTEQ | AMP2 | PIPE2 | AND_KW
            | OR_KW | XOR_KW => Ty::php_bool(),
            AMP | PIPE | CARET | SHL | SHR => Ty::php_int(),
            QUESTION2 => stir_types::join(self.ctx, &stir_types::without_null(&l), &r),
            PIPE_GT => match strip_platform(&r).kind() {
                TyKind::Fn(f) => f.ret.clone(),
                TyKind::Callable { sig: Some(s), .. } => s.ret.clone(),
                _ => Ty::any_platform(),
            },
            _ => Ty::error(),
        }
    }

    fn arith_result(&self, op: SyntaxKind, l: &Ty, r: &Ty) -> Ty {
        let is_float =
            |t: &Ty| self.assignable(t, &Ty::float()) || self.assignable(t, &Ty::php_float());
        let is_int = |t: &Ty| self.assignable(t, &Ty::int()) || self.assignable(t, &Ty::php_int());
        let unknown =
            |t: &Ty| t.is_any_platform() || matches!(t.kind(), TyKind::Mixed | TyKind::Error);
        let is_array = |t: &Ty| {
            t.members().iter().all(|m| {
                matches!(
                    strip_platform(m).kind(),
                    TyKind::Array { .. } | TyKind::PhpList { .. } | TyKind::Shape(_) | TyKind::Null
                )
            })
        };
        // `$a + $b` on arrays is a union of their entries
        if op == PLUS && is_array(l) && is_array(r) {
            return Ty::array(Ty::array_key(), Ty::mixed());
        }
        let (int, float) = (Ty::php_int(), Ty::php_float());
        if unknown(l) || unknown(r) {
            Ty::union([int, float])
        } else if is_int(l) && is_int(r) {
            match op {
                // as in PHP, `/` gives a float unless it divides evenly (`Int.div()` truncates)
                SLASH => Ty::union([int, float]),
                POW => Ty::union([int, float]),
                _ => int,
            }
        } else if (is_float(l) || is_int(l)) && (is_float(r) || is_int(r)) {
            if op == PERCENT {
                int
            } else if l.is_union() || r.is_union() {
                Ty::union([int, float])
            } else {
                float
            }
        } else {
            Ty::union([int, float])
        }
    }

    fn prefix(&mut self, node: &SyntaxNode, expected: Option<&Ty>) -> Ty {
        let Some(op) = op_token(node) else {
            return Ty::error();
        };
        let es = exprs_of(node);
        let Some(e) = es.first() else {
            return Ty::error();
        };
        match op.kind() {
            BANG => {
                self.expr(e, None);
                Ty::php_bool()
            }
            MINUS | PLUS => {
                let t = self.expr(e, None);
                if let TyKind::Literal(l) = t.kind()
                    && op.kind() == MINUS
                {
                    return match &l.value {
                        LitValue::Int(i) => Ty::literal(LitValue::Int(-i), l.family),
                        LitValue::Float(f) => Ty::literal(
                            LitValue::Float(stir_types::FloatLit::new(-f.get())),
                            l.family,
                        ),
                        _ => widen_literals(&t),
                    };
                }
                widen_literals(&t)
            }
            TILDE => {
                let t = self.expr(e, None);
                widen_literals(&t)
            }
            INC | DEC => {
                let t = self.expr(e, None);
                self.check_mutable_target(e);
                widen_literals(&t)
            }
            AMP => self.expr(e, expected),
            _ => self.expr(e, expected),
        }
    }

    // ------------------------------------------------------------ assignment

    fn check_mutable_target(&mut self, target: &SyntaxNode) {
        match target.kind() {
            VARIABLE_EXPR => {
                let name = text_of(target);
                let n = name.trim_start_matches('$');
                if n == "this" {
                    self.error(
                        target.text_range(),
                        codes::REASSIGN_IMMUTABLE,
                        "`$this` can't be reassigned",
                    );
                }
            }
            NULLSAFE_PROPERTY_FETCH_EXPR => {
                self.error(
                    target.text_range(),
                    codes::INVALID_OPERAND,
                    "can't assign through `?->`",
                );
            }
            _ => {}
        }
    }

    fn assign(&mut self, node: &SyntaxNode) -> Ty {
        let es = exprs_of(node);
        let op = node
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| !t.kind().is_trivia())
            .map(|t| t.kind())
            .unwrap_or(EQ);
        if es.len() != 2 {
            for e in &es {
                self.expr(e, None);
            }
            return Ty::error();
        }
        let (lhs, rhs) = (&es[0], &es[1]);
        match lhs.kind() {
            VARIABLE_EXPR if op == EQ => {
                let name = text_of(lhs);
                let n = name.trim_start_matches('$').to_owned();
                self.forget_properties_of(&n);
                match self.lookup(&n).cloned() {
                    Some(l) => {
                        if l.kind != LocalKind::This {
                            self.def(lhs.text_range(), Def::Local { decl: l.range });
                        }
                        let vt = self.expr(rhs, Some(&l.declared));
                        let conditional = self.conditional_depth > 0;
                        if n == "this" {
                            self.check_mutable_target(lhs);
                        } else if let Some(loc) = self.lookup_mut(&n) {
                            // PHP variables are dynamically typed
                            loc.ty = widen_literals(&vt);
                            loc.declared = if conditional {
                                Ty::union([loc.declared.clone(), loc.ty.clone()])
                            } else {
                                loc.ty.clone()
                            };
                        }
                        self.types.insert(lhs.text_range(), l.declared.clone());
                        vt
                    }
                    None => {
                        let vt = self.expr(rhs, None);
                        self.declare(&n, widen_literals(&vt), LocalKind::Php, lhs.text_range());
                        vt
                    }
                }
            }
            ARRAY_EXPR | LIST_EXPR if op == EQ => {
                let vt = self.expr(rhs, None);
                self.bind_target(lhs, vt.clone());
                vt
            }
            INDEX_EXPR if self.php_array_write(lhs, rhs, op).is_some() => self
                .types
                .get(&rhs.text_range())
                .cloned()
                .unwrap_or_else(Ty::mixed),
            _ => {
                let lt = self.expr(lhs, None);
                self.check_mutable_target(lhs);
                self.check_assign_target_writable(lhs);
                let declared = self.declared_target_type(lhs).unwrap_or(lt.clone());
                let vt = self.expr(rhs, Some(&declared));
                let result = match op {
                    EQ => vt.clone(),
                    QUESTION2_EQ => stir_types::join(self.ctx, &stir_types::without_null(&lt), &vt),
                    DOT_EQ => Ty::php_string(),
                    PLUS_EQ | MINUS_EQ | STAR_EQ | SLASH_EQ | PERCENT_EQ | POW_EQ => {
                        let k = match op {
                            PLUS_EQ => PLUS,
                            MINUS_EQ => MINUS,
                            STAR_EQ => STAR,
                            SLASH_EQ => SLASH,
                            PERCENT_EQ => PERCENT,
                            _ => POW,
                        };
                        self.arith_result(k, &lt, &vt)
                    }
                    _ => widen_literals(&lt),
                };

                self.check_assign(rhs.text_range(), &result, &declared, "assignment");
                if let Some(n) = self.local_name(lhs)
                    && let Some(l) = self.lookup_mut(&n)
                {
                    l.ty = narrow_assigned(&l.declared, &result);
                }
                if lhs.kind() == PROPERTY_FETCH_EXPR
                    && !lt.is_error()
                    && let Some(key) = self.place_key(lhs)
                {
                    self.forget_properties_of(&key);
                    for frame in &mut self.frames {
                        frame.remove(key.as_str());
                    }
                    // what it holds now, within what it's declared to hold; `null` isn't kept, since
                    // a call could fill it in and comparing with it would then look pointless
                    let assigned = if result.is_null() {
                        declared.clone()
                    } else {
                        narrow_assigned(&declared, &result)
                    };
                    if let Some(frame) = self.frames.first_mut() {
                        frame.insert(
                            SmolStr::new(&key),
                            Local {
                                ty: assigned,
                                declared: declared.clone(),
                                kind: LocalKind::Php,
                                range: lhs.text_range(),
                            },
                        );
                    }
                }
                result
            }
        }
    }

    /// `$a[] = v` / `$a[k] = v` / `$a[k][l] = v` on a PHP array in a variable (which
    /// may not exist yet): the array's type grows. Returns `None` if `lhs` isn't that.
    fn php_array_write(
        &mut self,
        lhs: &SyntaxNode,
        rhs: &SyntaxNode,
        op: SyntaxKind,
    ) -> Option<()> {
        let mut indexes = Vec::new();
        let mut root = lhs.clone();
        while root.kind() == INDEX_EXPR {
            let parts = exprs_of(&root);
            indexes.push(parts.get(1).cloned());
            root = parts.first()?.clone();
        }
        if root.kind() != VARIABLE_EXPR {
            return None;
        }
        let text = text_of(&root);
        let name = text.trim_start_matches('$');
        if is_superglobal(name) || name == "this" {
            return None;
        }
        let current = self.lookup(name).map(|l| l.ty.clone());
        let array_like = |t: &Ty| matches!(t.kind(), TyKind::Array { .. } | TyKind::PhpList { .. });
        let grows = match &current {
            None => true,
            Some(t) if t.is_union() => t.members().iter().any(array_like),
            Some(t) => array_like(t) || matches!(t.kind(), TyKind::Null),
        };
        if !grows {
            return None;
        }
        let keys: Vec<Option<Ty>> = indexes
            .iter()
            .map(|i| i.as_ref().map(|i| widen_literals(&self.expr(i, None))))
            .collect();
        let vt = widen_literals(&self.expr(rhs, None));
        self.types.insert(rhs.text_range(), vt.clone());
        let value = match op {
            EQ => vt,
            _ => Ty::mixed(),
        };
        // an empty array takes the shape of what is written to it
        let existing = current.as_ref().filter(|t| {
            !matches!(t.kind(), TyKind::PhpList { elem, .. } if matches!(elem.kind(), TyKind::Never))
        });
        let grown = match (keys.as_slice(), existing.map(Ty::kind)) {
            ([None], None | Some(TyKind::Null)) => Ty::php_list(value),
            ([None], Some(TyKind::PhpList { elem, .. })) => {
                Ty::php_list(Ty::union([elem.clone(), value]))
            }
            ([Some(k)], Some(TyKind::PhpList { elem, .. })) => Ty::array(
                Ty::union([Ty::php_int(), k.clone()]),
                Ty::union([elem.clone(), value]),
            ),
            ([None], Some(TyKind::Array { key, value: v, .. })) => Ty::array(
                Ty::union([key.clone(), Ty::php_int()]),
                Ty::union([v.clone(), value]),
            ),
            ([Some(k)], Some(TyKind::Array { key, value: v, .. })) => Ty::array(
                Ty::union([key.clone(), k.clone()]),
                Ty::union([v.clone(), value]),
            ),
            ([Some(k)], None | Some(TyKind::Null)) => Ty::array(k.clone(), value),
            _ => Ty::array(Ty::array_key(), Ty::mixed()),
        };
        let conditional = self.conditional_depth > 0;
        match self.lookup_mut(name) {
            Some(l) => {
                l.ty = grown.clone();
                l.declared = if conditional {
                    Ty::union([l.declared.clone(), grown])
                } else {
                    grown
                };
            }
            None => self.declare(name, grown, LocalKind::Php, root.text_range()),
        }
        Some(())
    }

    /// A variable passed by reference in PHP code may be written by the callee, and
    /// needn't exist before the call: `preg_match($re, $s, $matches)`.
    fn bind_out_argument(&mut self, arg: &SyntaxNode) {
        if arg.kind() != VARIABLE_EXPR {
            return;
        }
        let text = text_of(arg);
        let name = text.trim_start_matches('$');
        match self.lookup_mut(name) {
            Some(l) if l.kind == LocalKind::This => {}
            Some(l) => {
                if !matches!(l.ty.kind(), TyKind::Array { .. } | TyKind::PhpList { .. }) {
                    l.ty = Ty::any_platform();
                    l.declared = Ty::any_platform();
                }
            }
            None => self.declare(name, Ty::any_platform(), LocalKind::Php, arg.text_range()),
        }
    }

    fn local_name(&self, node: &SyntaxNode) -> Option<String> {
        match node.kind() {
            VARIABLE_EXPR => Some(text_of(node).trim_start_matches('$').to_owned()),
            _ => None,
        }
    }

    /// The declared (un-narrowed) type of an assignment target.
    fn declared_target_type(&self, node: &SyntaxNode) -> Option<Ty> {
        if node.kind() == INDEX_EXPR {
            // writing `m[k] = v`: the element type, without the read's `?`
            let base = exprs_of(node).into_iter().next()?;
            let bt = strip_platform(&stir_types::without_null(
                self.types.get(&base.text_range())?,
            ));
            return match bt.kind() {
                TyKind::MutableList(e) | TyKind::List(e) => Some(e.clone()),
                TyKind::MutableMap(_, v) | TyKind::Map(_, v) => Some(v.clone()),
                _ => None,
            };
        }
        if node.kind() == PROPERTY_FETCH_EXPR {
            let key = self.place_key(node)?;
            return self.lookup(&key).map(|l| l.declared.clone());
        }
        let n = self.local_name(node)?;
        self.lookup(&n).map(|l| l.declared.clone())
    }

    /// Writes to readonly properties.
    fn check_assign_target_writable(&mut self, lhs: &SyntaxNode) {
        if lhs.kind() != PROPERTY_FETCH_EXPR {
            return;
        }
        let es = exprs_of(lhs);
        let Some(base) = es.first() else { return };
        let Some(bt) = self.types.get(&base.text_range()).cloned() else {
            return;
        };
        let Some(name) = lhs.child(SyntaxKind::NAME).map(|n| text_of(&n)) else {
            return;
        };
        let bt = strip_platform(&stir_types::without_null(&bt));
        if let Some(p) = self.ctx.find_property(&class_of(&bt), &name)
            && p.immutable
        {
            let in_own_ctor =
                self.in_ctor && self.class.as_ref().is_some_and(|c| c.key == p.class.key);
            if !in_own_ctor {
                self.error(
                    lhs.text_range(),
                    codes::REASSIGN_IMMUTABLE,
                    format!("property `{name}` is immutable"),
                );
            }
        }
    }

    // ------------------------------------------------------------ collections

    fn index_type(&self, base: &Ty, _index: Option<&Ty>) -> Option<Ty> {
        match base.kind() {
            TyKind::List(e) | TyKind::MutableList(e) => Some(e.clone()),
            TyKind::Map(_, v) | TyKind::MutableMap(_, v) => Some(v.clone().nullable()),
            // in a loop, an array that starts empty may have been written to by now
            TyKind::PhpList { elem, .. } if matches!(elem.kind(), TyKind::Never) => {
                Some(Ty::any_platform())
            }
            TyKind::PhpList { elem, .. } => Some(elem.clone()),
            TyKind::Array { value, .. } => Some(value.clone()),
            TyKind::String | TyKind::PhpString => Some(base.clone()),
            TyKind::Literal(l) if matches!(l.value, LitValue::String(_)) => Some(Ty::string()),
            TyKind::Shape(_) | TyKind::Mixed => Some(Ty::mixed()),
            TyKind::Error => Some(Ty::error()),
            TyKind::Never | TyKind::Nothing => Some(Ty::never()),
            TyKind::Param { .. } => Some(Ty::any_platform()),
            TyKind::Class { .. } => Some(Ty::any_platform()),
            _ => None,
        }
    }

    fn array_literal(&mut self, node: &SyntaxNode, expected: Option<&Ty>) -> Ty {
        let elems = node.children_of(ARRAY_ELEM);
        let exp = expected.map(strip_platform);
        let (exp_key, exp_val) = match exp.as_ref().map(|e| e.kind().clone()) {
            Some(TyKind::List(e) | TyKind::MutableList(e)) => (None, Some(e)),
            Some(TyKind::Map(k, v) | TyKind::MutableMap(k, v)) => (Some(k), Some(v)),
            _ => (None, None),
        };
        // `[]` where a PHP array type is expected
        if elems.is_empty()
            && let Some(e) = &exp
            && let Some(array) = e.members().iter().find(|m| {
                matches!(
                    m.kind(),
                    TyKind::Array { .. } | TyKind::PhpList { .. } | TyKind::Shape(_)
                )
            })
        {
            return array.clone();
        }
        let mut keys = Vec::new();
        let mut vals = Vec::new();
        let mut keyed = false;
        for e in &elems {
            let parts = exprs_of(e);
            let spread = e.has_token(ELLIPSIS);
            match parts.as_slice() {
                [k, v] if e.has_token(FAT_ARROW) => {
                    keyed = true;
                    keys.push(self.expr(k, exp_key.as_ref()));
                    vals.push(self.expr(v, exp_val.as_ref()));
                }
                [v] if spread => {
                    let t = self.expr(v, exp.as_ref());
                    match strip_platform(&t).kind() {
                        TyKind::List(x)
                        | TyKind::MutableList(x)
                        | TyKind::PhpList { elem: x, .. } => vals.push(x.clone()),
                        TyKind::Map(k, x) | TyKind::MutableMap(k, x) => {
                            keyed = true;
                            keys.push(k.clone());
                            vals.push(x.clone());
                        }
                        _ => vals.push(Ty::error()),
                    }
                }
                [v] => vals.push(self.expr(v, exp_val.as_ref())),
                _ => {}
            }
        }
        if keyed {
            return Ty::array(
                if keys.is_empty() {
                    Ty::array_key()
                } else {
                    widen_literals(&Ty::union(keys))
                },
                if vals.is_empty() {
                    Ty::mixed()
                } else {
                    widen_literals(&Ty::union(vals))
                },
            );
        }
        Ty::php_list(if vals.is_empty() {
            Ty::never()
        } else {
            widen_literals(&Ty::union(vals))
        })
    }

    /// Explicit type arguments in a written type must satisfy their bounds.
    fn check_type_bounds(&mut self, range: TextRange, ty: &Ty) {
        let mut problems = Vec::new();
        ty.walk(&mut |t| {
            if let TyKind::Class { name, args } = t.kind()
                && let Some(c) = self.ctx.class(name)
            {
                let map = self.ctx.subst_map(&c, args);
                for (p, a) in c.params.iter().zip(args) {
                    if let Some(b) = stir_types::TypeEnv::param_bound(self.ctx, p.id) {
                        let b = stir_types::subst(&b, &map);
                        if !a.is_error() && !stir_types::is_assignable(self.ctx, a, &b) {
                            problems.push(format!(
                                "type argument `{a}` of `{}` does not satisfy the bound `{b}`",
                                c.name
                            ));
                        }
                    }
                }
            }
            true
        });
        for p in problems {
            self.error(range, codes::TYPE_BOUND, p);
        }
    }

    /// Overrides must be compatible with what they override.
    fn check_class_rules(&mut self, model: &Rc<ClassModel<'db>>) {
        let def = model.def;
        if model.origin != stir_db::FileOrigin::Source {
            return;
        }
        let self_ty = model.self_ty();
        let ancestors = self.ctx.ancestors(&self_ty);
        for m in &def.methods {
            if m.name.eq_ignore_ascii_case("__construct") {
                continue;
            }
            let sub = self.ctx.method_sig(model, m);
            for anc in ancestors.iter().skip(1) {
                let TyKind::Class { name, args } = anc.kind() else {
                    continue;
                };
                let Some(c) = self.ctx.class(name) else {
                    continue;
                };
                let Some(sm) = c
                    .def
                    .methods
                    .iter()
                    .find(|x| x.name.eq_ignore_ascii_case(&m.name))
                else {
                    continue;
                };
                if sm.mods.visibility == Visibility::Private {
                    continue;
                }
                let map = self.ctx.subst_map(&c, args);
                let sup = self.ctx.method_sig(&c, sm);
                let required = |s: &FnSig| {
                    s.params
                        .iter()
                        .filter(|p| !p.optional && !p.variadic)
                        .count()
                };
                if required(&sub) > required(&sup)
                    || (sub.params.len() < sup.params.len()
                        && !sub.params.last().is_some_and(|p| p.variadic))
                {
                    self.error(
                        m.name_range,
                        codes::INCOMPATIBLE_OVERRIDE,
                        format!(
                            "`{}` doesn't accept the same parameters as `{}::{}`",
                            m.name, c.name, sm.name
                        ),
                    );
                    break;
                }
                for (sp, bp) in sub.params.iter().zip(&sup.params) {
                    let want = stir_types::subst(&bp.ty, &map);
                    if !sp.ty.is_error() && !want.is_error() && !self.assignable(&want, &sp.ty) {
                        self.error(
                            sp.range,
                            codes::INCOMPATIBLE_OVERRIDE,
                            format!(
                                "parameter `{}` must accept `{want}` (overriding `{}::{}`)",
                                sp.name, c.name, sm.name
                            ),
                        );
                        self.tag_php_override();
                    }
                }
                let sup_ret = stir_types::subst(&sup.ret, &map);
                let sup_ret = resolve_self(&sup_ret, &self_ty);
                let sub_ret = resolve_self(&sub.ret, &self_ty);
                if sup.ret_declared
                    && !sub_ret.is_error()
                    && !sup_ret.is_error()
                    && !self.assignable(&sub_ret, &sup_ret)
                {
                    self.error(m.name_range, codes::INCOMPATIBLE_OVERRIDE, format!("return type `{sub_ret}` is not compatible with `{sup_ret}` of `{}::{}`", c.name, sm.name));
                    self.tag_php_override();
                }
                break;
            }
        }
    }

    /// Declaration-site variance: `out T` only in output positions, `in T` only in inputs.
    fn check_variance(&mut self, model: &Rc<ClassModel<'db>>) {
        use stir_types::Variance;
        let annotated: Vec<(stir_types::ParamId, SmolStr, Variance)> = model
            .params
            .iter()
            .filter(|p| p.variance != Variance::Invariant)
            .map(|p| (p.id, p.name.clone(), p.variance))
            .collect();
        if annotated.is_empty() {
            return;
        }
        let mut found: Vec<(TextRange, String)> = Vec::new();
        for m in &model.def.methods {
            if m.name.eq_ignore_ascii_case("__construct")
                || m.mods.visibility == Visibility::Private
            {
                continue;
            }
            let sig = self.ctx.method_sig(model, m);
            for p in &sig.params {
                self.variance_walk(&p.ty, true, false, &annotated, p.range, &mut found);
            }
            self.variance_walk(&sig.ret, false, false, &annotated, m.name_range, &mut found);
        }
        for p in &model.def.properties {
            if p.mods.visibility == Visibility::Private {
                continue;
            }
            if let Some(info) = self.ctx.find_property(&model.self_ty(), &p.name) {
                self.variance_walk(
                    &info.ty,
                    false,
                    !info.immutable,
                    &annotated,
                    p.range,
                    &mut found,
                );
            }
        }
        for (range, msg) in found {
            self.error(range, codes::VARIANCE, msg);
        }
    }

    /// Walks `ty` in a position that is contravariant (`input`) or covariant,
    /// or both (`invariant`), reporting misplaced variance-annotated params.
    #[allow(clippy::too_many_arguments)]
    fn variance_walk(
        &self,
        ty: &Ty,
        input: bool,
        invariant: bool,
        params: &[(stir_types::ParamId, SmolStr, stir_types::Variance)],
        range: TextRange,
        out: &mut Vec<(TextRange, String)>,
    ) {
        use stir_types::Variance;
        match ty.kind() {
            TyKind::Param { id, .. } => {
                if let Some((_, name, v)) = params.iter().find(|(p, _, _)| p == id) {
                    let bad = invariant
                        || (*v == Variance::Out && input)
                        || (*v == Variance::In && !input);
                    if bad {
                        let pos = if invariant {
                            "an invariant"
                        } else if input {
                            "an input (`in`)"
                        } else {
                            "an output (`out`)"
                        };
                        let kw = if *v == Variance::Out { "out" } else { "in" };
                        out.push((range, format!("type parameter `{name}` is declared `{kw}` but occurs in {pos} position")));
                    }
                }
            }
            TyKind::List(e) => self.variance_walk(e, input, invariant, params, range, out),
            TyKind::MutableList(e) => self.variance_walk(e, input, true, params, range, out),
            TyKind::Map(k, v) => {
                self.variance_walk(k, input, true, params, range, out);
                self.variance_walk(v, input, invariant, params, range, out);
            }
            TyKind::MutableMap(k, v) => {
                self.variance_walk(k, input, true, params, range, out);
                self.variance_walk(v, input, true, params, range, out);
            }
            TyKind::Fn(f) => {
                for p in &f.params {
                    self.variance_walk(&p.ty, !input, invariant, params, range, out);
                }
                self.variance_walk(&f.ret, input, invariant, params, range, out);
            }
            TyKind::Class { name, args } => {
                for (i, a) in args.iter().enumerate() {
                    match stir_types::TypeEnv::variance(self.ctx, name, i) {
                        Variance::Out => {
                            self.variance_walk(a, input, invariant, params, range, out)
                        }
                        Variance::In => {
                            self.variance_walk(a, !input, invariant, params, range, out)
                        }
                        Variance::Invariant => {
                            self.variance_walk(a, input, true, params, range, out)
                        }
                    }
                }
            }
            TyKind::Union(ms) | TyKind::Intersection(ms) => {
                for m in ms.iter() {
                    self.variance_walk(m, input, invariant, params, range, out);
                }
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------ calls

    fn args_of(&self, call: &SyntaxNode) -> Vec<SyntaxNode> {
        call.children_of(ARG_LIST)
            .iter()
            .flat_map(|a| {
                a.children()
                    .filter(|c| matches!(c.kind(), ARG | NAMED_ARG | CALLABLE_PLACEHOLDER))
                    .collect::<Vec<_>>()
            })
            .chain(call.children().filter(|c| c.kind() == ARG))
            .collect()
    }

    /// Checks arguments against a signature, inferring generics. `class_map`
    /// substitutes the declaring class's parameters.
    fn check_args(
        &mut self,
        call: &SyntaxNode,
        sig: &FnSig,
        class_map: &FxHashMap<stir_types::ParamId, Ty>,
        expected: Option<&Ty>,
        extra_vars: &[stir_types::ParamId],
    ) -> Ty {
        let subst_cls = |t: &Ty| stir_types::subst(t, class_map);
        let mut inf = Inference::new(
            sig.type_params
                .iter()
                .map(|p| p.id)
                .chain(extra_vars.iter().copied()),
        );
        inf.php_caller = true;

        if let Some(e) = expected
            && !e.is_error()
            && inf.has_unsolved(&subst_cls(&sig.ret))
        {
            // bidirectional: the call's result is expected to have a type
            let mut probe = inf.clone();
            probe.unify_expected(self.ctx, &subst_cls(&sig.ret), e);
            inf = probe;
        }
        let args = self.args_of(call);
        if args.iter().any(|a| a.kind() == CALLABLE_PLACEHOLDER) {
            return Ty::func(sig.fn_ty().params, sig.ret.clone());
        }
        // map arguments to parameters
        let mut assigned: Vec<Option<usize>> = Vec::new(); // arg -> param index
        let mut pos = 0usize;
        let mut has_spread = false;
        for a in &args {
            match a.kind() {
                NAMED_ARG => {
                    let n = a.name_text().unwrap_or_default();
                    let idx = sig.params.iter().position(|p| p.name == n);
                    if idx.is_none() && !sig.params.last().is_some_and(|p| p.variadic) {
                        self.error(
                            a.text_range(),
                            codes::ARGUMENT_COUNT,
                            format!("`{}` has no parameter named `{n}`", sig.name),
                        );
                    }
                    assigned.push(idx);
                }
                _ => {
                    if a.has_token(ELLIPSIS) {
                        has_spread = true;
                        assigned.push(None);
                        continue;
                    }
                    let idx = if pos < sig.params.len() {
                        Some(pos)
                    } else if sig.params.last().is_some_and(|p| p.variadic) {
                        Some(sig.params.len() - 1)
                    } else {
                        None
                    };
                    if idx.is_none() {
                        self.error(
                            a.text_range(),
                            codes::ARGUMENT_COUNT,
                            format!(
                                "too many arguments to `{}`: it takes {}",
                                sig.name,
                                sig.params.len()
                            ),
                        );
                    }
                    assigned.push(idx);
                    pos += 1;
                }
            }
        }
        let provided: Vec<usize> = assigned.iter().flatten().copied().collect();
        if !has_spread {
            for (i, p) in sig.params.iter().enumerate() {
                if !p.optional && !p.variadic && !provided.contains(&i) {
                    let end = call.text_range();
                    self.error(
                        end,
                        codes::ARGUMENT_COUNT,
                        format!("missing argument `{}` of `{}`", p.name, sig.name),
                    );
                }
            }
        }
        // pass 1: non-function arguments
        let mut arg_tys: Vec<Option<Ty>> = vec![None; args.len()];
        let value_of = |a: &SyntaxNode| exprs_of(a).into_iter().next();
        for (i, a) in args.iter().enumerate() {
            let Some(v) = value_of(a) else { continue };
            if matches!(v.kind(), CLOSURE_EXPR | ARROW_FN_EXPR) {
                continue;
            }
            if assigned[i].is_some_and(|p| sig.params[p].by_ref) {
                self.bind_out_argument(&v);
            }
            let pty = assigned[i].map(|p| subst_cls(&sig.params[p].ty));
            let exp = pty
                .as_ref()
                .map(|t| inf.apply(t))
                .filter(|t| !inf.has_unsolved(t));
            let t = self.expr(&v, exp.as_ref());
            if let Some(p) = &pty {
                inf.unify(self.ctx, p, &t);
            }
            arg_tys[i] = Some(t);
        }
        // pass 2: closures, with parameter types known from pass 1
        for (i, a) in args.iter().enumerate() {
            let Some(v) = value_of(a) else { continue };
            if !matches!(v.kind(), CLOSURE_EXPR | ARROW_FN_EXPR) {
                continue;
            }
            let pty = assigned[i].map(|p| subst_cls(&sig.params[p].ty));
            let exp = pty.as_ref().map(|t| inf.apply(t));
            let t = self.expr(&v, exp.as_ref());
            if let Some(p) = &pty {
                inf.unify(self.ctx, p, &t);
            }
            arg_tys[i] = Some(t);
        }
        // PHP code can't spell type arguments: nothing was said about this variable
        let ctx = self.ctx;
        inf.finish_with(|id| {
            stir_types::TypeEnv::param_bound(ctx, id).unwrap_or_else(Ty::any_platform)
        });
        for (i, a) in args.iter().enumerate() {
            let (Some(p), Some(t)) = (assigned[i], &arg_tys[i]) else {
                continue;
            };
            let param = &sig.params[p];
            let want = inf.apply(&subst_cls(&param.ty));
            let range = value_of(a)
                .map(|v| v.text_range())
                .unwrap_or(a.text_range());
            // external PHP functions accept untyped values
            if sig.external && t.is_any_platform() {
                continue;
            }
            // PHP code isn't asked to prove `non-negative-int`, `non-empty-string`, ...
            let want = widen_refinements(&want);
            // a PHP callback may ignore arguments it's passed
            if matches!(t.kind(), TyKind::Fn(_))
                && want
                    .members()
                    .iter()
                    .any(|m| matches!(m.kind(), TyKind::Callable { .. }))
            {
                continue;
            }
            if !self.items.strict_types && accepts_php_scalars(&want) {
                // without `strict_types`, PHP coerces scalar arguments
                let rest: Vec<Ty> = t
                    .members()
                    .iter()
                    .filter(|m| !coerces_to(m, &want))
                    .cloned()
                    .collect();
                if rest.is_empty() {
                    continue;
                }
                if rest.len() < t.members().len() {
                    let before = self.diags.len();
                    self.check_assign(
                        range,
                        &Ty::union(rest),
                        &want,
                        &format!("argument `{}` of `{}`", param.name, sig.name),
                    );
                    if self.diags.len() > before {
                        self.tag_partial_union();
                    }
                    continue;
                }
            }
            self.check_assign(
                range,
                t,
                &want,
                &format!("argument `{}` of `{}`", param.name, sig.name),
            );
        }
        inf.apply(&subst_cls(&sig.ret))
    }

    fn call(&mut self, node: &SyntaxNode, expected: Option<&Ty>) -> Ty {
        let Some(callee) = node.children().next() else {
            return Ty::error();
        };
        if callee.kind() == NAME_EXPR {
            let text = text_of(&callee);
            let scope = self.scope(node);
            let r = scope.resolve_function(&text);
            let found = [Some(r.primary.clone()), r.fallback.clone()]
                .into_iter()
                .flatten()
                .find_map(|f| self.ctx.function(f.as_str()).map(|loc| (f, loc)));
            let Some((fqn, loc)) = found else {
                if self.guarded_by("function_exists", &text) {
                    self.check_args_untyped(node);
                    return Ty::any_platform();
                }
                self.error(
                    callee.text_range(),
                    codes::UNDEFINED_FUNCTION,
                    format!("unknown function `{text}`"),
                );
                self.check_args_untyped(node);
                return Ty::error();
            };
            self.def(callee.text_range(), Def::Function(loc));
            self.function_calls.insert(
                node.text_range(),
                SmolStr::new(self.ctx.function_def(loc).name.as_str()),
            );
            // several declarations: overloaded stubs, or a PHP conditional declaration
            let overloads = self.ctx.function_overloads(fqn.as_str());
            if overloads.len() > 1 {
                let start = self.diags.len();
                let mut best: Option<(Vec<Diagnostic>, Ty)> = None;
                for candidate in &overloads {
                    let sig = self.ctx.function_sig(*candidate);
                    let t = self.check_args(node, &sig, &FxHashMap::default(), expected, &[]);
                    let found = self.diags.split_off(start);
                    if best.as_ref().is_none_or(|(b, _)| found.len() < b.len()) {
                        best = Some((found, t));
                    }
                }
                if let Some((found, t)) = best {
                    self.diags.extend(found);
                    return t;
                }
            }
            let sig = self.ctx.function_sig(loc);
            if self
                .ctx
                .function_def(loc)
                .name
                .as_str()
                .eq_ignore_ascii_case("extract")
            {
                self.dynamic_vars_fn = Some(self.fns.len());
            }
            let ret = self.check_args(node, &sig, &FxHashMap::default(), expected, &[]);
            let name = self
                .ctx
                .function_def(loc)
                .name
                .as_str()
                .to_ascii_lowercase();
            return self.refine_by_subject(node, &name, ret);
        }
        let ct = self.expr(&callee, None);
        self.call_value(node, &ct, expected)
    }

    /// `str_replace` and friends return an array only when given an array subject.
    fn refine_by_subject(&mut self, node: &SyntaxNode, name: &str, ret: Ty) -> Ty {
        let subject = match name {
            "str_replace"
            | "str_ireplace"
            | "preg_replace"
            | "preg_replace_callback"
            | "preg_filter" => 2,
            "substr_replace" => 0,
            _ => return ret,
        };
        let Some(arg) = self.args_of(node).into_iter().nth(subject) else {
            return ret;
        };
        let Some(t) = exprs_of(&arg)
            .first()
            .and_then(|e| self.types.get(&e.text_range()))
            .cloned()
        else {
            return ret;
        };
        let is_array = |m: &Ty| {
            matches!(
                strip_platform(m).kind(),
                TyKind::Array { .. }
                    | TyKind::PhpList { .. }
                    | TyKind::Shape(_)
                    | TyKind::List(_)
                    | TyKind::MutableList(_)
                    | TyKind::Map(..)
                    | TyKind::MutableMap(..)
                    | TyKind::Mixed
                    | TyKind::AnyPlatform
            )
        };
        if t.members().iter().any(is_array) {
            return ret;
        }
        Ty::union(ret.members().iter().filter(|m| !is_array(m)).cloned())
    }

    fn call_value(&mut self, node: &SyntaxNode, ct: &Ty, expected: Option<&Ty>) -> Ty {
        let ct = strip_platform(ct);
        let fn_ty = match ct.kind() {
            TyKind::Fn(f) => Some(f.clone()),
            TyKind::Callable { sig: Some(s), .. } => Some(s.clone()),
            TyKind::Class { name, args } => {
                stir_types::TypeEnv::invoke_signature(self.ctx, name, args)
            }
            _ => None,
        };
        match fn_ty {
            Some(f) => {
                let sig = FnSig {
                    name: SmolStr::new("function value"),
                    type_params: Vec::new(),
                    params: f
                        .params
                        .iter()
                        .enumerate()
                        .map(|(i, p)| crate::model::ParamSig {
                            name: SmolStr::new(format!("#{}", i + 1)),
                            ty: p.ty.clone(),
                            optional: p.kind == stir_types::ParamKind::Optional,
                            variadic: p.kind == stir_types::ParamKind::Variadic,
                            by_ref: false,
                            range: node.text_range(),
                        })
                        .collect(),
                    ret: f.ret.clone(),
                    ret_declared: true,
                    external: false,
                };
                self.check_args(node, &sig, &FxHashMap::default(), expected, &[])
            }
            None => {
                if !ct.is_error()
                    && !matches!(ct.kind(), TyKind::Callable { sig: None, .. })
                    && !matches!(ct.kind(), TyKind::Mixed)
                    && !ct.is_any_platform()
                    && !php_callable_value(&ct)
                {
                    self.error(
                        node.text_range(),
                        codes::NOT_CALLABLE,
                        format!("`{ct}` is not callable"),
                    );
                }
                self.check_args_untyped(node);
                if ct.is_error() {
                    Ty::error()
                } else {
                    Ty::any_platform()
                }
            }
        }
    }

    fn check_args_untyped(&mut self, node: &SyntaxNode) {
        for a in self.args_of(node) {
            for e in exprs_of(&a) {
                self.expr(&e, None);
            }
        }
    }

    /// `$x->name` / `$x?->name` receivers: returns the non-null receiver type.
    fn receiver(&mut self, node: &SyntaxNode) -> Option<(Ty, bool)> {
        let base = exprs_of(node).into_iter().next()?;
        let bt = self.expr(&base, None);
        let nullsafe = matches!(
            node.kind(),
            NULLSAFE_PROPERTY_FETCH_EXPR | NULLSAFE_METHOD_CALL_EXPR
        ) || node
            .children()
            .next()
            .is_some_and(|c| c.kind() == NULLSAFE_PROPERTY_FETCH_EXPR);
        let nullable = stir_types::is_nullable(self.ctx, &bt);

        Some((
            strip_platform(&stir_types::without_null(&bt)),
            nullsafe && nullable,
        ))
    }

    fn property_fetch(&mut self, node: &SyntaxNode) -> Ty {
        let Some((recv, nullable)) = self.receiver(node) else {
            return Ty::error();
        };
        let Some(name_node) = node.child(SyntaxKind::NAME) else {
            // dynamic `$a->$b` / `$a->{expr}`
            for e in exprs_of(node).iter().skip(1) {
                self.expr(e, None);
            }
            return Ty::any_platform();
        };
        let name = text_of(&name_node);
        let t = self.member_type(&recv, &name, name_node.text_range());
        if node.kind() == PROPERTY_FETCH_EXPR
            && let Some(key) = self.place_key(node)
            && let Some(l) = self.lookup(&key)
        {
            return l.ty.clone();
        }
        if nullable { t.nullable() } else { t }
    }

    /// A type parameter's members are those of its bound.
    fn bound_of(&self, ty: &Ty) -> Ty {
        let mut t = ty.clone();
        for _ in 0..8 {
            match t.kind() {
                TyKind::Param { id, .. } => match stir_types::TypeEnv::param_bound(self.ctx, *id) {
                    Some(b) => t = b,
                    None => return t,
                },
                _ => return t,
            }
        }
        t
    }

    fn member_type(&mut self, recv: &Ty, name: &str, range: TextRange) -> Ty {
        let recv = &self.bound_of(recv);
        if recv.is_union() {
            return self.union_member_type(recv, name, range);
        }
        match recv.kind() {
            TyKind::Error => Ty::error(),
            TyKind::Nothing | TyKind::Never => Ty::nothing(),
            TyKind::Class { .. } | TyKind::SelfType { .. } | TyKind::Intersection(_) => {
                let cls = class_of(recv);
                match self.ctx.find_property(&cls, name) {
                    Some(p) => {
                        let class = p.class.clone();
                        self.member_def(range, &class, MemberKind::Property, name);
                        // an inaccessible property goes through `__get`, if there is one
                        if self.has_magic(&cls, "__get") && !self.visible(&p.class, p.visibility) {
                            return Ty::any_platform();
                        }
                        self.check_visibility(
                            range,
                            &p.class,
                            p.visibility,
                            &format!("property `{name}`"),
                        );
                        p.ty
                    }
                    None => {
                        if self.has_magic(&cls, "__get")
                            || matches!(cls.kind(), TyKind::Class { name, .. } if name.eq_ignore_ascii_case("stdClass"))
                        {
                            return Ty::any_platform();
                        }
                        if self.probing > 0 {
                            return Ty::any_platform();
                        }
                        if self.class_known(&cls) {
                            self.error(
                                range,
                                codes::UNDEFINED_MEMBER,
                                format!("`{recv}` has no property `{name}`"),
                            );
                        }
                        Ty::error()
                    }
                }
            }
            TyKind::Mixed | TyKind::Object => Ty::any_platform(),
            _ if recv.is_any_platform() => Ty::error(),
            _ => {
                self.error(
                    range,
                    codes::UNDEFINED_MEMBER,
                    format!("`{recv}` has no property `{name}`"),
                );
                Ty::error()
            }
        }
    }

    /// A property of a PHP union: fine on every member, a level 7 problem on some.
    fn union_member_type(&mut self, recv: &Ty, name: &str, range: TextRange) -> Ty {
        let start = self.diags.len();
        let mut found = Vec::new();
        let mut missing = false;
        for m in recv.members().to_vec() {
            let before = self.diags.len();
            let t = self.member_type(&m, name, range);
            if self.diags.len() > before {
                self.diags.truncate(before);
                missing = true;
            } else {
                found.push(t);
            }
        }
        self.diags.truncate(start);
        if missing {
            self.error(
                range,
                codes::UNDEFINED_MEMBER,
                format!("`{recv}` has no property `{name}`"),
            );
            if !found.is_empty() {
                self.tag_partial_union();
            }
        }
        if found.is_empty() {
            Ty::error()
        } else {
            Ty::union(found)
        }
    }

    fn has_magic(&self, ty: &Ty, name: &str) -> bool {
        self.ctx.find_method(ty, name).is_some()
    }

    /// Are `ty` and all its ancestors known? A member can come from an unknown ancestor, so
    /// a missing one isn't an error: the unknown class is reported where it's named.
    fn class_known(&self, ty: &Ty) -> bool {
        matches!(ty.kind(), TyKind::Class { .. }) && self.ctx.hierarchy_known(ty)
    }

    fn visible(&self, owner: &Rc<ClassModel<'db>>, vis: Visibility) -> bool {
        match vis {
            Visibility::Public => true,
            // a trait's private members belong to the classes using it
            Visibility::Private => {
                owner.kind() == ClassKind::Trait
                    || self.class.as_ref().is_some_and(|c| c.key == owner.key)
            }
            Visibility::Protected => self.class.as_ref().is_some_and(|c| {
                c.key == owner.key
                    || self.ctx.is_subclass(&c.name, &owner.name)
                    || self.ctx.is_subclass(&owner.name, &c.name)
            }),
        }
    }

    fn check_visibility(
        &mut self,
        range: TextRange,
        owner: &Rc<ClassModel<'db>>,
        vis: Visibility,
        what: &str,
    ) {
        if !self.visible(owner, vis) {
            let v = match vis {
                Visibility::Private => "private",
                _ => "protected",
            };
            self.error(
                range,
                codes::VISIBILITY,
                format!("{what} of `{}` is {v}", owner.name),
            );
        }
    }

    fn method_call(&mut self, node: &SyntaxNode, expected: Option<&Ty>) -> Ty {
        // shape: METHOD_CALL_EXPR(PROPERTY_FETCH_EXPR(base, ->, NAME), ARG_LIST)
        let Some(fetch) = node.children().next() else {
            return Ty::error();
        };
        let Some((recv, nullable)) = self.receiver(&fetch) else {
            return Ty::error();
        };
        self.types.insert(fetch.text_range(), recv.clone());
        let Some(name_node) = fetch.child(SyntaxKind::NAME) else {
            self.check_args_untyped(node);
            return Ty::any_platform();
        };
        let name = text_of(&name_node);
        let t = self.call_member(node, &recv, &name, name_node.text_range(), expected);
        if nullable { t.nullable() } else { t }
    }

    fn call_member(
        &mut self,
        node: &SyntaxNode,
        recv: &Ty,
        name: &str,
        range: TextRange,
        expected: Option<&Ty>,
    ) -> Ty {
        let bounded = self.bound_of(recv);
        let recv = if matches!(recv.kind(), TyKind::Param { .. })
            && !matches!(bounded.kind(), TyKind::Param { .. })
        {
            &bounded
        } else {
            recv
        };
        if recv.is_error() {
            self.check_args_untyped(node);
            return Ty::error();
        }
        // narrowing ruled this path out, so it can't run
        if matches!(recv.kind(), TyKind::Never | TyKind::Nothing) {
            self.check_args_untyped(node);
            return Ty::nothing();
        }
        if recv.is_union() {
            return self.union_method_call(node, recv, name, range, expected);
        }
        let cls = class_of(recv);
        if matches!(cls.kind(), TyKind::Class { .. })
            && let Some((owner, m, map)) = self.ctx.find_method(&cls, name)
        {
            self.member_def(range, &owner, MemberKind::Method, &m.name);
            // an inaccessible method goes through `__call`, if there is one
            if self.has_magic(&cls, "__call") && !self.visible(&owner, m.mods.visibility) {
                self.check_args_untyped(node);
                return Ty::any_platform();
            }
            self.check_visibility(
                range,
                &owner,
                m.mods.visibility,
                &format!("method `{name}`"),
            );
            let sig = self.ctx.method_sig(&owner, m);
            let mut map = map;
            // `static` return types resolve to the receiver
            let ret = self.check_args(node, &sig, &map, expected, &[]);
            map.clear();
            return resolve_self(&ret, recv);
        }
        if matches!(cls.kind(), TyKind::Class { .. })
            && (self.has_magic(&cls, "__call") || !self.class_known(&cls))
        {
            self.check_args_untyped(node);
            return Ty::any_platform();
        }
        if recv.is_any_platform() || matches!(recv.kind(), TyKind::Mixed | TyKind::Object) {
            self.check_args_untyped(node);
            return Ty::any_platform();
        }
        let cls = class_of(recv);
        if self.class_known(&cls) || !matches!(cls.kind(), TyKind::Class { .. }) {
            self.error(
                range,
                codes::UNDEFINED_MEMBER,
                format!("`{recv}` has no method `{name}`"),
            );
        }
        self.check_args_untyped(node);
        Ty::error()
    }

    /// A method of a PHP union: fine on every member, a level 7 problem on some.
    /// Each member checks the call, so only the first one's diagnostics are kept.
    fn union_method_call(
        &mut self,
        node: &SyntaxNode,
        recv: &Ty,
        name: &str,
        range: TextRange,
        expected: Option<&Ty>,
    ) -> Ty {
        let (has, lacks): (Vec<Ty>, Vec<Ty>) = recv
            .members()
            .iter()
            .cloned()
            .partition(|m| self.responds_to(m, name));
        if has.is_empty() {
            self.check_args_untyped(node);
            self.error(
                range,
                codes::UNDEFINED_MEMBER,
                format!("`{recv}` has no method `{name}`"),
            );
            return Ty::error();
        }
        let mut results = Vec::new();
        for (i, m) in has.iter().enumerate() {
            let before = self.diags.len();
            results.push(self.call_member(node, m, name, range, expected));
            if i > 0 {
                self.diags.truncate(before);
            }
        }
        if !lacks.is_empty() {
            self.error(
                range,
                codes::UNDEFINED_MEMBER,
                format!("`{recv}` has no method `{name}`"),
            );
            self.tag_partial_union();
        }
        Ty::union(results)
    }

    fn responds_to(&self, ty: &Ty, name: &str) -> bool {
        let cls = class_of(ty);
        matches!(cls.kind(), TyKind::Class { .. })
            && (self.ctx.find_method(&cls, name).is_some()
                || self.has_magic(&cls, "__call")
                || !self.class_known(&cls))
            || matches!(ty.kind(), TyKind::Mixed | TyKind::Object | TyKind::Error)
            || ty.is_any_platform()
    }

    // ------------------------------------------------------------ static access

    /// Resolves a class reference (`Foo`, `self`, `static`, `parent`).
    fn class_ref_ty(&mut self, node: &SyntaxNode, text: &str) -> Ty {
        match text.to_ascii_lowercase().as_str() {
            "self" | "static" => {
                return match &self.class {
                    Some(c) => c.self_ty(),
                    None => {
                        self.error(
                            node.text_range(),
                            codes::UNDEFINED_CLASS,
                            format!("`{text}` used outside a class"),
                        );
                        Ty::error()
                    }
                };
            }
            "parent" => {
                return match &self.class {
                    Some(c) => match &self.ctx.supers(c).parent {
                        Some(p) => p.clone(),
                        None => {
                            self.error(
                                node.text_range(),
                                codes::UNDEFINED_CLASS,
                                "this class has no parent",
                            );
                            Ty::error()
                        }
                    },
                    None => Ty::error(),
                };
            }
            _ => {}
        }
        let scope = self.scope(node);
        let fqn = scope.resolve_class(text);
        match self.ctx.class(fqn.as_str()) {
            Some(c) => {
                self.def(node.text_range(), Def::Class(c.key.clone()));
                Ty::class(
                    c.name.clone(),
                    c.params.iter().map(|p| p.ty.clone()).collect(),
                )
            }
            None => {
                self.error(
                    node.text_range(),
                    codes::UNDEFINED_CLASS,
                    format!("unknown class `{text}`"),
                );
                Ty::error()
            }
        }
    }

    /// The class on the left of `::`.
    fn static_base(&mut self, node: &SyntaxNode) -> Ty {
        let Some(base) = node.children().next() else {
            return Ty::error();
        };
        if base.kind() == NAME_EXPR {
            let t = text_of(&base);
            self.class_ref_ty(&base, &t)
        } else {
            let t = self.expr(&base, None);
            strip_platform(&t)
        }
    }

    fn static_call(&mut self, node: &SyntaxNode, expected: Option<&Ty>) -> Ty {
        let Some(fetch) = node.children().next() else {
            return Ty::error();
        };
        let cls = self.static_base(&fetch);
        let Some(name_node) = fetch.child(SyntaxKind::NAME) else {
            self.check_args_untyped(node);
            return Ty::any_platform();
        };
        let name = text_of(&name_node);
        if cls.is_error() {
            self.check_args_untyped(node);
            return Ty::error();
        }
        match self.ctx.find_method(&cls, &name) {
            Some((owner, m, map)) => {
                self.member_def(name_node.text_range(), &owner, MemberKind::Method, &m.name);
                self.check_visibility(
                    name_node.text_range(),
                    &owner,
                    m.mods.visibility,
                    &format!("method `{name}`"),
                );
                let sig = self.ctx.method_sig(&owner, m);
                // static generic classes: class params are inferred like function params
                let extra: Vec<_> = if m.mods.is_static {
                    owner.params.iter().map(|p| p.id).collect()
                } else {
                    Vec::new()
                };
                let map = if m.mods.is_static {
                    FxHashMap::default()
                } else {
                    map
                };
                let ret = self.check_args(node, &sig, &map, expected, &extra);
                resolve_self(&ret, &cls)
            }
            None => {
                if self.has_magic(&cls, "__callStatic") || !self.class_known(&cls) {
                    self.check_args_untyped(node);
                    return Ty::any_platform();
                }
                self.error(
                    name_node.text_range(),
                    codes::UNDEFINED_MEMBER,
                    format!("`{cls}` has no method `{name}`"),
                );
                self.check_args_untyped(node);
                Ty::error()
            }
        }
    }

    fn static_property(&mut self, node: &SyntaxNode) -> Ty {
        let cls = self.static_base(node);
        let Some(var) = node
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .filter(|t| t.kind() == VARIABLE)
            .last()
        else {
            return Ty::any_platform();
        };
        let name = &var.text()[1..];
        if cls.is_error() {
            return Ty::error();
        }
        match self.ctx.find_property(&cls, name) {
            Some(p) => {
                let class = p.class.clone();
                self.member_def(var.text_range(), &class, MemberKind::Property, name);
                self.check_visibility(
                    var.text_range(),
                    &p.class,
                    p.visibility,
                    &format!("property `{name}`"),
                );
                p.ty
            }
            None => {
                if self.class_known(&cls) {
                    self.error(
                        var.text_range(),
                        codes::UNDEFINED_MEMBER,
                        format!("`{cls}` has no static property `${name}`"),
                    );
                }
                Ty::error()
            }
        }
    }

    fn class_const(&mut self, node: &SyntaxNode) -> Ty {
        let Some(name_node) = node.child(SyntaxKind::NAME) else {
            return Ty::any_platform();
        };
        let name = text_of(&name_node);
        if name.eq_ignore_ascii_case("class") {
            let base = node.children().next();
            if let Some(b) = &base
                && b.kind() != NAME_EXPR
            {
                self.expr(b, None);
            } else {
                self.static_base(node);
            }
            return Ty::class_string(None);
        }
        let cls = self.static_base(node);
        if cls.is_error() {
            return Ty::error();
        }
        match self.ctx.find_const(&cls, &name) {
            Some(t) => {
                if let Some((owner, kind)) = self.ctx.const_owner(&cls, &name) {
                    let kind = if kind {
                        MemberKind::Case
                    } else {
                        MemberKind::Const
                    };
                    self.member_def(name_node.text_range(), &owner, kind, &name);
                }
                t
            }
            None => {
                if self.class_known(&cls) {
                    self.error(
                        name_node.text_range(),
                        codes::UNDEFINED_MEMBER,
                        format!("`{cls}` has no constant `{name}`"),
                    );
                }
                Ty::error()
            }
        }
    }

    fn new_expr(&mut self, node: &SyntaxNode, expected: Option<&Ty>) -> Ty {
        let t = self.new_expr_inner(node, expected);
        // `new static()` is an instance of the late-bound class
        let is_static = node
            .children()
            .find(|c| c.kind() != ARG_LIST)
            .is_some_and(|c| c.kind() == NAME_EXPR && text_of(&c).eq_ignore_ascii_case("static"));
        if is_static && matches!(t.kind(), TyKind::Class { .. }) {
            Ty::static_of(t)
        } else {
            t
        }
    }

    fn new_expr_inner(&mut self, node: &SyntaxNode, expected: Option<&Ty>) -> Ty {
        if node.child(ANON_CLASS).is_some() {
            self.check_args_untyped(node);
            return Ty::object();
        }
        let Some(target) = node.children().find(|c| c.kind() != ARG_LIST) else {
            return Ty::error();
        };
        let cls = if target.kind() == NAME_EXPR {
            let t = text_of(&target);
            self.class_ref_ty(&target, &t)
        } else {
            self.expr(&target, None);
            self.check_args_untyped(node);
            return Ty::any_platform();
        };
        let TyKind::Class { name, .. } = cls.kind() else {
            self.check_args_untyped(node);
            return Ty::error();
        };
        let Some(model) = self.ctx.class(name) else {
            self.check_args_untyped(node);
            return Ty::error();
        };
        match model.kind() {
            ClassKind::Interface | ClassKind::Trait | ClassKind::Enum => {
                self.error(
                    target.text_range(),
                    codes::INSTANTIATION,
                    format!("`{}` can't be instantiated", model.name),
                );
            }
            _ if model.def.mods.is_abstract => {
                self.error(
                    target.text_range(),
                    codes::INSTANTIATION,
                    format!("`{}` is abstract and can't be instantiated", model.name),
                );
            }
            _ => {}
        }
        // the constructor: `__construct` (possibly inherited)
        let vars: Vec<_> = model.params.iter().map(|p| p.id).collect();
        let exp_args = expected.and_then(|e| match strip_platform(e).kind() {
            TyKind::Class { name: en, args } if en.eq_ignore_ascii_case(&model.name) => {
                Some(args.clone())
            }
            _ => None,
        });
        let sig = self
            .ctx
            .find_method(&model.self_ty(), "__construct")
            .map(|(owner, m, _)| {
                let mut s = self.ctx.method_sig(&owner, m);
                s.ret = model.self_ty();
                s
            });

        match sig {
            Some(mut sig) => {
                let mut extra = vars.clone();
                if let Some(args) = &exp_args {
                    // the expected type fixes the class arguments
                    let map: FxHashMap<_, _> = model
                        .params
                        .iter()
                        .map(|p| p.id)
                        .zip(args.iter().cloned())
                        .collect();
                    sig.ret = stir_types::subst(&sig.ret, &map);
                    for p in &mut sig.params {
                        p.ty = stir_types::subst(&p.ty, &map);
                    }
                    extra.clear();
                }
                self.check_args(node, &sig, &FxHashMap::default(), None, &extra)
            }
            None => {
                self.check_args_untyped(node);
                // nothing to infer type arguments from but the expected type
                match exp_args {
                    Some(args) if args.len() == model.params.len() => {
                        Ty::class(model.name.clone(), args)
                    }
                    _ => model.self_ty(),
                }
            }
        }
    }

    // ------------------------------------------------------------ functions as values

    fn closure(&mut self, node: &SyntaxNode, expected: Option<&Ty>) -> Ty {
        let exp_fn = expected.and_then(|e| match strip_platform(e).kind() {
            TyKind::Fn(f) => Some(f.clone()),
            TyKind::Callable { sig: Some(s), .. } => Some(s.clone()),
            _ => None,
        });
        let arrow = node.kind() == ARROW_FN_EXPR;
        // arrow functions capture by value automatically; closures only via `use`
        let outer = self.frames.clone();
        let mut frame = FxHashMap::default();
        if arrow {
            for f in &outer {
                for (k, v) in f {
                    frame.insert(k.clone(), v.clone());
                }
            }
        } else if let Some(u) = node.child(CLOSURE_USE) {
            for v in u.children_of(CLOSURE_USE_VAR) {
                if let Some(t) = v.token(VARIABLE) {
                    let n = &t.text()[1..];
                    match self.lookup(n).cloned() {
                        Some(l) => {
                            frame.insert(SmolStr::new(n), l);
                        }
                        None => {
                            if v.has_token(AMP) {
                                frame.insert(
                                    SmolStr::new(n),
                                    Local {
                                        ty: Ty::null(),
                                        declared: Ty::mixed(),
                                        kind: LocalKind::Php,
                                        range: t.text_range(),
                                    },
                                );
                            } else {
                                self.error(
                                    t.text_range(),
                                    codes::UNDEFINED_VARIABLE,
                                    format!("undefined variable `${n}`"),
                                );
                            }
                        }
                    }
                }
            }
        }
        if let Some(this) = self.lookup("this").cloned() {
            frame.insert(SmolStr::new("this"), this);
        }
        let saved = std::mem::replace(&mut self.frames, vec![frame]);
        let mut params = Vec::new();
        if let Some(pl) = node.child(PARAM_LIST) {
            for (i, p) in pl.children_of(PARAM).iter().enumerate() {
                let ty = match stir_hir::first_type(p) {
                    Some(t) => self.lower_type_node(&t),
                    None => exp_fn
                        .as_ref()
                        .and_then(|f| f.params.get(i).map(|p| p.ty.clone()))
                        .unwrap_or_else(Ty::mixed),
                };
                let name = match p.token(VARIABLE) {
                    Some(v) => v.text()[1..].to_owned(),
                    None => p.name_text().unwrap_or_default(),
                };
                self.declare(&name, ty.clone(), LocalKind::Param, p.text_range());
                params.push(stir_types::FnParam::required(ty));
            }
        }
        let declared_ret = node
            .child(RETURN_TYPE)
            .and_then(|r| stir_hir::first_type(&r))
            .map(|t| self.lower_type_node(&t));
        let ret = if arrow {
            let body = exprs_of(node).into_iter().last();
            let exp_ret = declared_ret
                .clone()
                .or_else(|| exp_fn.as_ref().map(|f| f.ret.clone()));
            let t = body
                .map(|b| self.expr(&b, exp_ret.as_ref()))
                .unwrap_or_else(Ty::error);
            declared_ret.unwrap_or(t)
        } else {
            let infer = declared_ret.is_none();
            let ret = declared_ret.clone().unwrap_or_else(Ty::unit);
            self.fns.push(FnCx {
                ret: ret.clone(),
                infer_ret: infer,
                returns: Vec::new(),
            });
            if let Some(b) = node.child(BLOCK) {
                self.block_stmts(&b);
            }
            let fcx = self.fns.pop().unwrap();
            if infer {
                fcx.returns
                    .into_iter()
                    .reduce(|a, b| stir_types::join(self.ctx, &a, &b))
                    .unwrap_or_else(Ty::unit)
            } else {
                ret
            }
        };
        self.frames = saved;
        Ty::func(params, ret)
    }

    // ------------------------------------------------------------ match

    fn match_expr(&mut self, node: &SyntaxNode, expected: Option<&Ty>) -> Ty {
        if let Some(subject) = exprs_of(node).first() {
            self.expr(subject, None);
        }
        let mut result: Option<Ty> = None;
        for arm in node.children_of(MATCH_ARM) {
            if let Some(conds) = arm.child(MATCH_CONDITIONS) {
                for c in exprs_of(&conds) {
                    self.expr(&c, None);
                }
            }
            if let Some(body) = exprs_of(&arm).last() {
                let t = self.expr(body, expected);
                result = Some(match result {
                    None => t,
                    Some(r) => stir_types::join(self.ctx, &r, &t),
                });
            }
        }
        result.unwrap_or_else(Ty::nothing)
    }
}

fn is_null_literal(node: &SyntaxNode) -> bool {
    node.kind() == NAME_EXPR && text_of(node).eq_ignore_ascii_case("null")
}

/// `T!` → `T` (platform types behave like their inner type when used).
pub fn strip_platform(ty: &Ty) -> Ty {
    match ty.kind() {
        TyKind::Platform(inner) => inner.clone(),
        TyKind::Union(_)
            if ty
                .members()
                .iter()
                .any(|m| matches!(m.kind(), TyKind::Platform(_))) =>
        {
            Ty::union(ty.members().iter().map(strip_platform))
        }
        _ => ty.clone(),
    }
}

/// The class type to look members up on (`static`/`$this` → their class).
fn class_of(ty: &Ty) -> Ty {
    match ty.kind() {
        TyKind::SelfType { class, .. } => class.clone(),
        TyKind::Intersection(_) => ty
            .members()
            .iter()
            .find(|m| matches!(m.kind(), TyKind::Class { .. }))
            .cloned()
            .unwrap_or_else(|| ty.clone()),
        _ => ty.clone(),
    }
}

/// Replaces `static`/`$this` in a return type with the receiver's type.
fn resolve_self(ret: &Ty, recv: &Ty) -> Ty {
    ret.transform(&mut |t| match t.kind() {
        TyKind::SelfType { .. } => Some(recv.clone()),
        _ => None,
    })
}

/// After assigning `value` to a variable declared `declared`, its current type.
/// The value of a string literal without escapes or interpolation.
fn unquoted_literal(node: &SyntaxNode) -> Option<String> {
    let text = text_of(node);
    let inner = text
        .strip_prefix('\'')
        .and_then(|t| t.strip_suffix('\''))
        .or_else(|| text.strip_prefix('"').and_then(|t| t.strip_suffix('"')))?;
    (!inner.contains(['\\', '$'])).then(|| inner.to_owned())
}

/// A function with `yield` in its own body: its declared type describes the generator,
/// and `return` gives the generator's final value.
fn is_generator(function: &SyntaxNode) -> bool {
    fn has_yield(node: &SyntaxNode) -> bool {
        node.children().any(|c| {
            matches!(c.kind(), YIELD_EXPR | YIELD_FROM_EXPR)
                || (!matches!(
                    c.kind(),
                    FUNCTION_DECL | METHOD_DECL | CLOSURE_EXPR | ARROW_FN_EXPR
                ) && has_yield(&c))
        })
    }
    has_yield(function)
}

fn in_generator(node: &SyntaxNode) -> bool {
    node.ancestors()
        .find(|a| {
            matches!(
                a.kind(),
                FUNCTION_DECL | METHOD_DECL | CLOSURE_EXPR | ARROW_FN_EXPR
            )
        })
        .is_some_and(|f| is_generator(&f))
}

/// A PHP string or array in a variable may name a function: `$callback()`.
fn php_callable_value(ty: &Ty) -> bool {
    ty.members().iter().any(|m| {
        matches!(
            m.kind(),
            TyKind::PhpString
                | TyKind::NonEmptyString
                | TyKind::ClassString(_)
                | TyKind::PhpList { .. }
                | TyKind::Array { .. }
                | TyKind::Shape(_)
                | TyKind::Callable { .. }
        ) || matches!(m.kind(), TyKind::Literal(l) if matches!(l.value, LitValue::String(_)))
    })
}

fn php_array_of_mixed(from: &Ty, to: &Ty) -> bool {
    let bare = matches!(
        strip_platform(from).kind(),
        TyKind::Array { key, value, .. }
            if matches!(key.kind(), TyKind::ArrayKey) && matches!(value.kind(), TyKind::Mixed)
    );
    bare && to.members().iter().any(|m| {
        matches!(m.kind(), TyKind::Array { value, non_empty: false, .. } if matches!(value.kind(), TyKind::Mixed))
    })
}

fn is_php_scalar(ty: &Ty) -> bool {
    matches!(
        ty.kind(),
        TyKind::PhpInt
            | TyKind::PhpFloat
            | TyKind::PhpString
            | TyKind::PhpBool
            | TyKind::Numeric
            | TyKind::ArrayKey
            | TyKind::NonEmptyString
            | TyKind::NumericString
            | TyKind::IntRange { .. }
    ) || matches!(ty.kind(), TyKind::Literal(l) if l.family == Family::Php)
}

/// A scalar coerces in a non-strict call, unless it's a literal that isn't a number
/// going to a parameter that doesn't take strings (a `TypeError` at runtime).
fn coerces_to(ty: &Ty, want: &Ty) -> bool {
    if let TyKind::Literal(l) = ty.kind()
        && let LitValue::String(v) = &l.value
        && v.trim().parse::<f64>().is_err()
    {
        return want
            .members()
            .iter()
            .any(|m| matches!(m.kind(), TyKind::PhpString));
    }
    is_php_scalar(ty)
}

/// Does the type have a member a scalar can be coerced to? Literal types don't qualify.
fn accepts_php_scalars(ty: &Ty) -> bool {
    ty.members().iter().any(|m| {
        matches!(
            m.kind(),
            TyKind::PhpInt | TyKind::PhpFloat | TyKind::PhpString | TyKind::PhpBool
        )
    })
}

fn is_superglobal(name: &str) -> bool {
    matches!(
        name,
        "GLOBALS"
            | "_SERVER"
            | "_GET"
            | "_POST"
            | "_FILES"
            | "_COOKIE"
            | "_SESSION"
            | "_REQUEST"
            | "_ENV"
    )
}

fn narrow_assigned(declared: &Ty, value: &Ty) -> Ty {
    if value.is_error() || declared.is_error() {
        return declared.clone();
    }
    // a literal in a PHP array variable is a PHP array, not a read-only collection
    if matches!(
        strip_platform(declared).kind(),
        TyKind::Array { .. } | TyKind::PhpList { .. }
    ) {
        return declared.clone();
    }
    // keep the declared type unless the value is a strictly narrower non-literal
    if matches!(value.kind(), TyKind::Null) {
        return Ty::null();
    }
    let w = widen_literals(value);
    if stir_types::is_assignable(&stir_types::EmptyEnv, &w, declared) {
        w
    } else {
        declared.clone()
    }
}

/// Is `node` (an index/property chain) the target of an assignment?
fn is_write_target(node: &SyntaxNode) -> bool {
    let mut cur = node.clone();
    loop {
        let Some(p) = cur.parent() else { return false };
        match p.kind() {
            ASSIGN_EXPR => return p.children().next().as_ref() == Some(&cur),
            INDEX_EXPR if exprs_of(&p).first() == Some(&cur) => cur = p,
            _ => return false,
        }
    }
}

/// `int<0, max>` → `int`, `non-empty-string` → `string`, `non-empty-list<T>` → `list<T>`.
fn widen_refinements(t: &Ty) -> Ty {
    t.transform(&mut |x| match x.kind() {
        TyKind::IntRange { .. } => Some(Ty::php_int()),
        TyKind::NonEmptyString | TyKind::NumericString | TyKind::ClassString(_) => {
            Some(Ty::php_string())
        }
        TyKind::PhpList {
            elem,
            non_empty: true,
        } => Some(Ty::php_list(widen_refinements(elem))),
        TyKind::Array {
            key,
            value,
            non_empty: true,
        } => Some(Ty::array(widen_refinements(key), widen_refinements(value))),
        _ => None,
    })
}
