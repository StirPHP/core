//! Statement and expression checking.

use std::rc::Rc;
use std::sync::Arc;

use rustc_hash::FxHashMap;
use smol_str::SmolStr;
use stir_db::{Diagnostic, Severity, SourceFile};
use stir_hir::ast::NodeExt;
use stir_hir::name::Scope;
use stir_hir::type_ref::{parse_int, unquote};
use stir_hir::{Binding, ClassKind, FileItems, Visibility};
use stir_syntax::{Dialect, SyntaxKind, SyntaxKind::*, SyntaxNode, SyntaxToken, TextRange};
use stir_types::{Family, LitValue, Ty, TyKind};

use crate::codes;
use crate::infer::{Inference, widen_literals};
use crate::model::{ClassModel, Ctx, FnSig, LowerCx};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalKind {
    /// `val x`
    Val,
    /// `var $x`
    Var,
    /// A function parameter.
    Param(Binding),
    /// `$this`
    This,
    /// An implicitly declared PHP variable.
    Php,
    /// A `foreach` / `catch` binding.
    Loop,
}

impl LocalKind {
    fn mutable(self) -> bool {
        matches!(
            self,
            LocalKind::Var
                | LocalKind::Php
                | LocalKind::Loop
                | LocalKind::Param(Binding::Mut | Binding::Php)
        )
    }

    /// Is the binding written with `$` in StirPHP?
    fn dollar(self) -> bool {
        matches!(
            self,
            LocalKind::Var
                | LocalKind::This
                | LocalKind::Php
                | LocalKind::Loop
                | LocalKind::Param(Binding::Php)
        )
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
    pub dialect: Dialect,
    pub items: &'db FileItems,
    pub diags: Vec<Diagnostic>,
    frames: Vec<FxHashMap<SmolStr, Local>>,
    fns: Vec<FnCx>,
    class: Option<Rc<ClassModel<'db>>>,
    lower_cx: LowerCx,
    in_ctor: bool,
    in_template: u32,
    /// Expression types, for later phases (transpiler).
    pub types: FxHashMap<TextRange, Ty>,
    /// Names resolved as members of `$this` (`id` → `$this->id`), for the transpiler.
    pub implicit_this: rustc_hash::FxHashSet<TextRange>,
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
            | VAL_DECL
            | VAR_DECL
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
            | RECORD_DECL
            | TYPEALIAS_DECL
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
            | NOT_NULL_EXPR
            | CAST_EXPR
            | AS_EXPR
            | IS_EXPR
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
            | LAMBDA_EXPR
            | MATCH_EXPR
            | WHEN_EXPR
            | IF_EXPR
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
            | RECORD_LITERAL
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
            dialect: file.dialect(ctx.db),
            items: ctx.items(file),
            diags: Vec::new(),
            frames: vec![FxHashMap::default()],
            fns: Vec::new(),
            class: None,
            lower_cx: LowerCx {
                dialect: Some(file.dialect(ctx.db)),
                ..LowerCx::default()
            },
            in_ctor: false,
            in_template: 0,
            types: FxHashMap::default(),
            implicit_this: Default::default(),
        }
    }

    /// The type of `$this`, if inside an instance method.
    fn this_ty(&self) -> Option<Ty> {
        self.lookup("this")
            .filter(|l| l.kind == LocalKind::This)
            .map(|l| l.ty.clone())
    }

    /// A property of `$this` referenced without `$this->` (StirPHP only).
    fn implicit_property(&mut self, node: &SyntaxNode, name: &str, mutable: bool) -> Option<Ty> {
        if !self.stir() {
            return None;
        }
        let this = self.this_ty()?;
        let p = self.ctx.find_property(&this, name)?;
        if p.immutable == mutable {
            return None;
        }
        self.implicit_this.insert(node.text_range());
        Some(p.ty)
    }

    fn stir(&self) -> bool {
        self.dialect == Dialect::Stir
    }

    // ------------------------------------------------------------ diagnostics

    pub fn error(&mut self, range: TextRange, code: &'static str, msg: impl Into<String>) {
        self.diags
            .push(Diagnostic::error(self.path.clone(), range, code, msg));
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
            dialect: self.dialect,
            type_params: &names,
        };
        let tr = tcx.lower(node);
        let mut sink = Vec::new();
        let ty = self.ctx.lower(&tr, &self.lower_cx, &mut sink);
        for e in sink {
            self.error(e.range, e.code, e.message);
        }
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
        if let Err(m) = stir_types::check_assignable(self.ctx, from, to) {
            let mut msg = format!("type mismatch in {what}: {m}");
            if from.is_any_platform() {
                msg.push_str("; narrow the untyped value first (`is` / `as`)");
            }
            self.error(range, codes::TYPE_MISMATCH, msg);
        }
    }

    // ------------------------------------------------------------ locals

    fn lookup(&self, name: &str) -> Option<&Local> {
        self.frames.iter().rev().find_map(|f| f.get(name))
    }

    fn lookup_mut(&mut self, name: &str) -> Option<&mut Local> {
        self.frames.iter_mut().rev().find_map(|f| f.get_mut(name))
    }

    fn declare(&mut self, name: &str, ty: Ty, kind: LocalKind, range: TextRange) {
        if self.stir()
            && let Some(prev) = self.lookup(name)
        {
            let prev_dollar = prev.kind.dollar();
            if prev_dollar != kind.dollar() && prev.kind != LocalKind::This {
                let (v, m) = if kind.dollar() {
                    (name.to_string(), format!("${name}"))
                } else {
                    (format!("${name}"), name.to_string())
                };
                self.error(
                    range,
                    codes::VAL_VAR_CLASH,
                    format!("`{m}` clashes with `{v}`: an immutable and a mutable binding can't share an identifier"),
                );
            } else if self.frames.last().is_some_and(|f| f.contains_key(name))
                && kind != LocalKind::Php
            {
                self.error(
                    range,
                    codes::DUPLICATE_DECLARATION,
                    format!("`{name}` is already declared in this scope"),
                );
            }
        }
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

    fn push_frame(&mut self) {
        self.frames.push(FxHashMap::default());
    }

    fn pop_frame(&mut self) {
        self.frames.pop();
    }

    fn apply_facts(&mut self, facts: &Facts) {
        for (name, ty) in &facts.narrow {
            if let Some(l) = self.lookup_mut(name) {
                l.ty = ty.clone();
            }
        }
    }

    /// Snapshot of current narrowed types (for restoring after a branch).
    fn snapshot(&self) -> Vec<FxHashMap<SmolStr, Local>> {
        self.frames.clone()
    }

    fn restore_types(&mut self, snap: &[FxHashMap<SmolStr, Local>]) {
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
            let cx = LowerCx {
                dialect: Some(self.dialect),
                ..LowerCx::default()
            };
            let _ = self.ctx.build_sig(
                &format!("check:{i}"),
                &f.name.0,
                &f.sig,
                f.receiver.as_ref(),
                self.file,
                &cx,
                &mut sink,
            );
            for e in sink {
                self.error(e.range, e.code, e.message);
            }
        }
        for a in &items.type_aliases {
            let mut sink = Vec::new();
            let mut cx = LowerCx {
                dialect: Some(self.dialect),
                ..LowerCx::default()
            };
            for (j, tp) in a.type_params.iter().enumerate() {
                cx.generics.push((
                    tp.name.clone(),
                    Ty::param(
                        self.ctx.param_id(&format!("alias:{}", a.name), &tp.name),
                        tp.name.clone(),
                    ),
                ));
                let _ = j;
            }
            self.ctx.lower(&a.ty, &cx, &mut sink);
            for e in sink {
                self.error(e.range, e.code, e.message);
            }
        }
        if !self.stir() {
            return;
        }
        for f in &items.functions {
            for p in &f.sig.params {
                if p.ty.is_none() {
                    self.error(
                        p.name_range,
                        codes::MISSING_TYPE,
                        format!("parameter `{}` needs a type", p.name),
                    );
                }
            }
        }
        for c in &items.classes {
            for m in &c.methods {
                for p in &m.sig.params {
                    if p.ty.is_none() {
                        self.error(
                            p.name_range,
                            codes::MISSING_TYPE,
                            format!("parameter `{}` needs a type", p.name),
                        );
                    }
                }
            }
            for p in c.primary_ctor.iter().flatten() {
                if p.ty.is_none() {
                    self.error(
                        p.name_range,
                        codes::MISSING_TYPE,
                        format!("parameter `{}` needs a type", p.name),
                    );
                }
            }
            for p in &c.properties {
                if p.ty.is_none() && !p.has_default {
                    self.error(
                        p.range,
                        codes::MISSING_TYPE,
                        format!("property `{}` needs a type or an initializer", p.name),
                    );
                }
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
                    let t = self.expr(&e, None);
                    self.check_stringable(&e, &t, "echo");
                }
            }
            RETURN_STMT => self.return_stmt(node),
            BLOCK => {
                self.push_frame();
                self.block_stmts(node);
                self.pop_frame();
            }
            IF_STMT => self.if_stmt(node),
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
                let es = exprs_of(node);
                let subject = es
                    .first()
                    .map(|e| self.expr(e, None))
                    .unwrap_or_else(Ty::error);
                for case in node.children_of(SWITCH_CASE) {
                    for e in exprs_of(&case) {
                        let t = self.expr(&e, None);
                        self.check_comparable(e.text_range(), &subject, &t);
                    }
                    for s in case.children().filter(|c| is_stmt_kind(c.kind())) {
                        self.stmt(&s);
                    }
                }
            }
            BREAK_STMT | CONTINUE_STMT | GOTO_STMT | LABEL_STMT | EMPTY_STMT | INLINE_HTML_STMT
            | USE_DECL | HALT_COMPILER_STMT | TYPEALIAS_DECL => {}
            TRY_STMT => {
                let snap = self.snapshot();
                for c in node.children() {
                    match c.kind() {
                        BLOCK => self.stmt(&c),
                        CATCH_CLAUSE => {
                            self.restore_types(&snap);
                            self.push_frame();
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
                            self.pop_frame();
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
            VAL_DECL => self.val_decl(node),
            VAR_DECL => self.var_decl(node),
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
            CLASS_DECL | INTERFACE_DECL | TRAIT_DECL | ENUM_DECL | RECORD_DECL => {
                self.class_decl(node)
            }
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
        // After the if: types are those of the fall-through path.
        self.restore_types(&snap);
        if !has_else {
            if let Some(ff) = falls_through_facts {
                self.apply_facts(&ff);
            }
        } else if !all_diverge {
            self.widen_assigned_in(node);
        }
    }

    /// Variables assigned inside `node` lose their narrowings.
    fn widen_assigned_in(&mut self, node: &SyntaxNode) {
        for a in node.descendants().filter(|d| d.kind() == ASSIGN_EXPR) {
            if let Some(lhs) = a.children().next() {
                let name = text_of(&lhs);
                let name = name.trim_start_matches('$').to_owned();
                if let Some(l) = self.lookup_mut(&name) {
                    l.ty = l.declared.clone();
                }
            }
        }
    }

    fn foreach_stmt(&mut self, node: &SyntaxNode) {
        let es = exprs_of(node);
        let Some(subject) = es.first() else { return };
        let st = self.expr(subject, None);
        let (key_ty, val_ty) = self.iter_types(subject.text_range(), &st);
        self.push_frame();
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
        self.pop_frame();
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
        match ty.kind() {
            TyKind::List(e) | TyKind::MutableList(e) => (Ty::int(), e.clone()),
            TyKind::Map(k, v) | TyKind::MutableMap(k, v) => (k.clone(), v.clone()),
            TyKind::PhpList { elem, .. } => (Ty::php_int(), elem.clone()),
            TyKind::Array { key, value, .. } | TyKind::Iterable { key, value } => {
                (key.clone(), value.clone())
            }
            TyKind::Shape(_) => (Ty::array_key(), Ty::mixed()),
            TyKind::Error | TyKind::Mixed => (Ty::error(), Ty::error()),
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
            _ if ty.is_any_platform() => {
                if self.stir() {
                    self.error(
                        range,
                        codes::INVALID_OPERAND,
                        "cannot iterate over an untyped value; narrow it first (`is` / `as`)",
                    );
                }
                (Ty::error(), Ty::error())
            }
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
                    if self.stir() || !t.is_null() {
                        self.error(
                            v.text_range(),
                            codes::RETURN_VALUE,
                            "this function returns `Unit`; it can't return a value",
                        );
                    }
                } else {
                    self.check_assign(v.text_range(), &t, &fcx.ret, "return value");
                }
            }
            None => {
                if !fcx.infer_ret
                    && !matches!(fcx.ret.kind(), TyKind::Unit | TyKind::Void | TyKind::Error)
                    && !stir_types::is_nullable(self.ctx, &fcx.ret)
                    && self.stir()
                {
                    self.error(
                        node.text_range(),
                        codes::RETURN_VALUE,
                        format!("missing return value of type `{}`", fcx.ret),
                    );
                }
                if fcx.infer_ret {
                    self.fns.last_mut().unwrap().returns.push(Ty::unit());
                }
            }
        }
    }

    fn val_decl(&mut self, node: &SyntaxNode) {
        let Some(name) = node.name_text() else { return };
        let ann = stir_hir::first_type(node).map(|t| self.lower_type_node(&t));
        let value = exprs_of(node).into_iter().next();
        let vt = value.as_ref().map(|v| self.expr(v, ann.as_ref()));
        let ty = match (&ann, &vt) {
            (Some(a), Some(v)) => {
                self.check_assign(value.as_ref().unwrap().text_range(), v, a, "declaration");
                a.clone()
            }
            (Some(a), None) => a.clone(),
            (None, Some(v)) => v.clone(),
            (None, None) => Ty::error(),
        };
        let range = node
            .child(SyntaxKind::NAME)
            .map(|n| n.text_range())
            .unwrap_or(node.text_range());
        // vals keep literal types (they never change); declared type is widened
        self.declare(&name, ty.clone(), LocalKind::Val, range);
        if ann.is_none()
            && let Some(l) = self.lookup_mut(&name)
        {
            l.declared = widen_literals(&ty);
        }
    }

    fn var_decl(&mut self, node: &SyntaxNode) {
        let Some(var) = node.token(VARIABLE) else {
            return;
        };
        let name = &var.text()[1..];
        let ann = stir_hir::first_type(node).map(|t| self.lower_type_node(&t));
        let value = exprs_of(node).into_iter().next();
        let vt = value.as_ref().map(|v| self.expr(v, ann.as_ref()));
        let ty = match (&ann, &vt) {
            (Some(a), Some(v)) => {
                self.check_assign(value.as_ref().unwrap().text_range(), v, a, "declaration");
                a.clone()
            }
            (Some(a), None) => a.clone(),
            (None, Some(v)) => widen_literals(v),
            (None, None) => {
                self.error(
                    var.text_range(),
                    codes::MISSING_TYPE,
                    format!("`${name}` needs a type or an initializer"),
                );
                Ty::error()
            }
        };
        if ty.is_any_platform() || ty.contains_any_platform() {
            // allowed in locals
        }
        self.declare(name, ty.clone(), LocalKind::Var, var.text_range());
        if let (Some(v), Some(l)) = (&vt, self.lookup_mut(name)) {
            // the current value is known more precisely than the declared type
            if ann.is_some() {
                l.ty = narrow_assigned(&l.declared, v);
            }
        }
    }

    // ------------------------------------------------------------ functions

    fn function_decl(&mut self, node: &SyntaxNode) {
        let ptr = stir_hir::NodePtr::new(node);
        let items = self.items;
        let Some((idx, f)) = items
            .functions
            .iter()
            .enumerate()
            .find(|(_, f)| f.ptr == ptr)
        else {
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
        let this = sig.receiver.clone();
        self.function_body(node, &sig, this, f.expr_body);
        self.lower_cx = saved_cx;
    }

    /// Checks a function/method body against its signature.
    fn function_body(&mut self, node: &SyntaxNode, sig: &FnSig, this: Option<Ty>, expr_body: bool) {
        let saved_frames = std::mem::replace(&mut self.frames, vec![FxHashMap::default()]);
        if let Some(t) = this {
            // `this` in extension functions is a val; `$this` in methods
            let kind = if self.class.is_some() {
                LocalKind::This
            } else if is_by_ref_receiver(&t) {
                // mutable collections are values in PHP: their extension
                // functions take the receiver by reference
                LocalKind::Param(Binding::Mut)
            } else {
                LocalKind::Val
            };
            self.frames.last_mut().unwrap().insert(
                SmolStr::new("this"),
                Local {
                    ty: t.clone(),
                    declared: t,
                    kind,
                    range: node.text_range(),
                },
            );
        }
        for p in &sig.params {
            let mut ty = p.ty.clone();
            if p.variadic {
                ty = if self.stir() {
                    Ty::list(ty)
                } else {
                    Ty::php_list(ty)
                };
            }
            self.declare(&p.name, ty, LocalKind::Param(p.binding), p.range);
        }
        let infer = !sig.ret_declared && expr_body;
        self.fns.push(FnCx {
            ret: sig.ret.clone(),
            infer_ret: infer,
            returns: Vec::new(),
        });
        if let Some(body) = node.child(FUNCTION_BODY) {
            if let Some(e) = exprs_of(&body).first() {
                let expected = if infer { None } else { Some(sig.ret.clone()) };
                let t = self.expr(e, expected.as_ref());
                if !infer && !matches!(sig.ret.kind(), TyKind::Unit | TyKind::Void) {
                    self.check_assign(e.text_range(), &t, &sig.ret, "return value");
                }
                if infer && t.contains_any_platform() {
                    self.error(e.text_range(), codes::ANY_PLATFORM_ESCAPE, "an untyped value (`Any!`) can't be returned; narrow it or declare a return type");
                }
            }
        } else if let Some(block) = node.child(BLOCK) {
            self.block_stmts(&block);
            let fcx = self.fns.last().unwrap();
            let needs_value = !matches!(
                fcx.ret.kind(),
                TyKind::Unit | TyKind::Void | TyKind::Error | TyKind::Nothing | TyKind::Never
            ) && !stir_types::is_nullable(self.ctx, &fcx.ret);
            if self.stir() && needs_value && sig.ret_declared && !block_diverges(self, &block) {
                let end = block.text_range().end();
                let range = node
                    .child(RETURN_TYPE)
                    .map(|r| r.text_range())
                    .unwrap_or_else(|| TextRange::new(end - stir_syntax::TextSize::new(1), end));
                self.error(
                    range,
                    codes::RETURN_VALUE,
                    format!(
                        "missing `return` of type `{}` at the end of the function",
                        fcx.ret
                    ),
                );
            }
        }
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
        // types on the header
        for t in def.extends.iter().chain(&def.implements).map(|t| t.range) {
            let _ = t;
        }
        self.check_class_header(&model);
        // primary constructor defaults
        if let Some(ctor) = node.child(PRIMARY_CTOR) {
            for p in ctor.descendants().filter(|d| d.kind() == PARAM) {
                for e in exprs_of(&p) {
                    self.expr(&e, None);
                }
            }
        }
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
        for t in def
            .extends
            .iter()
            .chain(&def.implements)
            .chain(def.supertypes.iter().map(|s| &s.ty))
        {
            let ty = self.ctx.lower(t, &cx, &mut sink);
            if let TyKind::Class { name, .. } = ty.kind()
                && let Some(parent) = self.ctx.class(name)
                && parent.kind() == ClassKind::Class
                && parent.is_final()
                && self.stir()
                && def.kind == ClassKind::Class
                && !parent.is_external()
            {
                self.error(
                    t.range,
                    codes::INSTANTIATION,
                    format!(
                        "`{}` is final; mark it `open` to allow subclassing",
                        parent.name
                    ),
                );
            }
        }
        for e in sink {
            self.error(e.range, e.code, e.message);
        }
        // record field types / property types written on the class
        if def.kind == ClassKind::Record {
            let mut sink = Vec::new();
            for f in &def.fields {
                let ty = self.ctx.lower(&f.ty, &cx, &mut sink);
                if ty.contains_any_platform() {
                    self.error(
                        f.range,
                        codes::ANY_PLATFORM_ESCAPE,
                        "record fields can't be untyped (`Any!`)",
                    );
                }
            }
            for e in sink {
                self.error(e.range, e.code, e.message);
            }
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
            None,
            self.file,
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
            Some(model.self_ty())
        };
        if m.has_body {
            self.function_body(node, &sig, this, m.expr_body);
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
        let ty = self.expr(node, Some(&Ty::bool()));
        if self.stir()
            && !ty.is_error()
            && !self.assignable(&ty, &Ty::bool())
            && !self.assignable(&ty, &Ty::php_bool())
        {
            self.error(
                node.text_range(),
                codes::TYPE_MISMATCH,
                format!("condition must be `Bool`, found `{ty}`"),
            );
        }
        self.facts(node)
    }

    fn facts(&mut self, node: &SyntaxNode) -> (Facts, Facts) {
        match node.kind() {
            PAREN_EXPR => match exprs_of(node).first() {
                Some(inner) => self.facts(inner),
                None => Default::default(),
            },
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
                        let (rt, rf) = self.facts(&es[1]);
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
                        let (_, rf) = self.facts(&es[1]);
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
            IS_EXPR | INSTANCEOF_EXPR => {
                let negated = node.has_token(BANG);
                let es = exprs_of(node);
                let Some(subject) = es.first() else {
                    return Default::default();
                };
                let Some((name, local)) = self.local_of(subject) else {
                    return Default::default();
                };
                let test = match node.kind() {
                    IS_EXPR => match stir_hir::first_type(node) {
                        Some(t) => self.pattern_type(&t, Some(&local.ty.clone())),
                        None => return Default::default(),
                    },
                    _ => match node.children().nth(1) {
                        Some(r) if r.kind() == NAME_EXPR => {
                            let n = text_of(&r);
                            self.class_ref_ty(&r, &n)
                        }
                        _ => return Default::default(),
                    },
                };
                let yes = stir_types::narrow_to(self.ctx, &local.ty, &test);
                let no = stir_types::narrow_away(self.ctx, &local.ty, &test);
                let t = Facts {
                    narrow: vec![(name.clone(), yes)],
                };
                let f = Facts {
                    narrow: vec![(name, no)],
                };
                if negated { (f, t) } else { (t, f) }
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
                    "is_int" | "is_integer" | "is_long" => {
                        if self.stir() {
                            Ty::int()
                        } else {
                            Ty::php_int()
                        }
                    }
                    "is_float" | "is_double" => {
                        if self.stir() {
                            Ty::float()
                        } else {
                            Ty::php_float()
                        }
                    }
                    "is_string" => {
                        if self.stir() {
                            Ty::string()
                        } else {
                            Ty::php_string()
                        }
                    }
                    "is_bool" => {
                        if self.stir() {
                            Ty::bool()
                        } else {
                            Ty::php_bool()
                        }
                    }
                    "is_null" => Ty::null(),
                    "is_array" => Ty::php_array(),
                    "is_object" => Ty::object(),
                    _ => return Default::default(),
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
            VARIABLE_EXPR | NAME_EXPR => {
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

    fn local_of(&self, node: &SyntaxNode) -> Option<(SmolStr, Local)> {
        let name = match node.kind() {
            VARIABLE_EXPR => text_of(node).trim_start_matches('$').to_owned(),
            NAME_EXPR if self.stir() => text_of(node),
            PAREN_EXPR => return exprs_of(node).first().and_then(|e| self.local_of(e)),
            _ => return None,
        };
        self.lookup(&name).map(|l| (SmolStr::new(&name), l.clone()))
    }

    /// The type tested by `is T` / `when { is T -> }` against `subject`. A
    /// generic class written without arguments (`is Ok`) takes them from the
    /// subject (`Result<Int>` → `Ok<Int>`).
    fn pattern_type(&mut self, node: &SyntaxNode, subject: Option<&Ty>) -> Ty {
        if node.kind() == NAMED_TYPE
            && node.child(GENERIC_ARG_LIST).is_none()
            && let Some(name_ref) = node.child(NAME_REF)
        {
            let text = text_of(&name_ref);
            let fqn = self.scope(node).resolve_class(&text);
            if let Some(model) = self.ctx.class(fqn.as_str())
                && !model.params.is_empty()
            {
                let mut candidates: Vec<Ty> = Vec::new();
                if let Some(s) = subject {
                    for m in strip_platform(s)
                        .members()
                        .iter()
                        .cloned()
                        .chain(std::iter::once(strip_platform(s)))
                    {
                        if let TyKind::Class { name, args } = m.kind()
                            && let Some(subs) =
                                stir_types::TypeEnv::sealed_subclasses(self.ctx, name, args)
                        {
                            candidates.extend(subs);
                        }
                        candidates.push(m);
                    }
                }
                for c in candidates {
                    let same = match c.kind() {
                        TyKind::Class { name, .. } => name.eq_ignore_ascii_case(&model.name),
                        TyKind::Record(r) => r
                            .name
                            .as_deref()
                            .is_some_and(|n| n.eq_ignore_ascii_case(&model.name)),
                        _ => false,
                    };
                    if same {
                        return c;
                    }
                }
                // not derivable: the class with unknown arguments
                let args = model.params.iter().map(|_| Ty::error()).collect();
                return if model.kind() == ClassKind::Record {
                    self.ctx.record_ty(&model, args)
                } else {
                    Ty::class(model.name.clone(), args)
                };
            }
        }
        self.lower_type_node(node)
    }

    fn lower_type_node_quiet(&mut self, node: &SyntaxNode) -> Ty {
        let n = self.diags.len();
        let t = self.lower_type_node(node);
        self.diags.truncate(n);
        t
    }

    // ------------------------------------------------------------ expressions

    /// Infers the type of an expression, checking it against `expected` where
    /// that helps inference (the caller still checks assignability).
    pub fn expr(&mut self, node: &SyntaxNode, expected: Option<&Ty>) -> Ty {
        let ty = self.expr_inner(node, expected);
        self.types.insert(node.text_range(), ty.clone());
        ty
    }

    fn lit_family(&self) -> Family {
        if self.stir() {
            Family::Safe
        } else {
            Family::Php
        }
    }

    fn expr_inner(&mut self, node: &SyntaxNode, expected: Option<&Ty>) -> Ty {
        match node.kind() {
            LITERAL => {
                let Some(tok) = op_token(node) else {
                    return Ty::error();
                };
                match tok.kind() {
                    INT_NUMBER => match parse_int(tok.text()) {
                        Some(v) => Ty::literal(LitValue::Int(v), self.lit_family()),
                        // overflowing integer literals are floats in PHP
                        None => {
                            if self.stir() {
                                Ty::float()
                            } else {
                                Ty::php_float()
                            }
                        }
                    },
                    FLOAT_NUMBER => match tok.text().replace('_', "").parse::<f64>() {
                        Ok(v) => Ty::literal(
                            LitValue::Float(stir_types::FloatLit::new(v)),
                            self.lit_family(),
                        ),
                        Err(_) => Ty::error(),
                    },
                    _ => Ty::error(),
                }
            }
            STRING_LITERAL => {
                let text = text_of(node);
                Ty::literal(
                    LitValue::String(SmolStr::new(unquote(&text))),
                    self.lit_family(),
                )
            }
            INTERP_STRING | HEREDOC => {
                for part in node.children() {
                    match part.kind() {
                        INTERP_EXPR | INTERP_VAR => {
                            self.in_template += 1;
                            let es = exprs_of(&part);
                            for e in es {
                                let t = self.expr(&e, None);
                                self.check_stringable(&e, &t, "a string template");
                            }
                            self.in_template -= 1;
                        }
                        _ => {}
                    }
                }
                if self.stir() {
                    Ty::string()
                } else {
                    Ty::php_string()
                }
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
                Some(LINE_KW) => {
                    if self.stir() {
                        Ty::int()
                    } else {
                        Ty::php_int()
                    }
                }
                _ => {
                    if self.stir() {
                        Ty::string()
                    } else {
                        Ty::php_string()
                    }
                }
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
                if self.stir() {
                    self.error(
                        node.text_range(),
                        codes::UNSUPPORTED_SYNTAX,
                        "variable variables are not supported in StirPHP",
                    );
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
                self.require_numeric(target.text_range(), &t, "`++`/`--`");
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
                if self.stir() {
                    if !l.is_error()
                        && !stir_types::is_nullable(self.ctx, &l)
                        && !matches!(l.kind(), TyKind::Platform(_))
                    {
                        self.warn(
                            es[0].text_range(),
                            codes::UNNECESSARY_NULL_CHECK,
                            format!("left side of `?:` is never null (`{l}`)"),
                        );
                    }
                    stir_types::join(self.ctx, &stir_types::without_null(&l), &r)
                } else {
                    stir_types::join(self.ctx, &l, &r)
                }
            }
            NOT_NULL_EXPR => {
                let es = exprs_of(node);
                let Some(e) = es.first() else {
                    return Ty::error();
                };
                let t = self.expr(e, None);
                if !t.is_error()
                    && !stir_types::is_nullable(self.ctx, &t)
                    && !matches!(t.kind(), TyKind::Platform(_))
                {
                    self.warn(
                        node.text_range(),
                        codes::UNNECESSARY_NULL_CHECK,
                        format!("`!!` on a non-null value of type `{t}`"),
                    );
                }
                strip_platform(&stir_types::without_null(&t))
            }
            IS_EXPR => {
                let st = exprs_of(node).first().map(|e| self.expr(e, None));
                if let Some(t) = stir_hir::first_type(node) {
                    self.pattern_type(&t, st.as_ref());
                }
                Ty::bool()
            }
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
                let safe = node.has_token(QUESTION);
                if !src.is_error()
                    && !target.is_error()
                    && !src.is_any_platform()
                    && !matches!(src.kind(), TyKind::Platform(_))
                {
                    let overlap = stir_types::is_assignable(self.ctx, &target, &src)
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
                if safe { target.nullable() } else { target }
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
                if self.stir() {
                    Ty::bool()
                } else {
                    Ty::php_bool()
                }
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
                    Some(OBJECT_CAST) => Ty::object(),
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
                if self.stir() && (bt.is_nullable_like(self.ctx)) {
                    self.error(
                        base.text_range(),
                        codes::NULLABLE_ACCESS,
                        format!("indexing a nullable value of type `{bt}`; check for null first"),
                    );
                }
                match self.index_type(&strip_platform(&stir_types::without_null(&bt)), it.as_ref())
                {
                    Some(t) => t,
                    None => {
                        if bt.is_any_platform() && self.stir() {
                            self.error(
                                base.text_range(),
                                codes::INVALID_OPERAND,
                                "cannot index an untyped value; narrow it first (`is` / `as`)",
                            );
                        } else if !bt.is_error() && !bt.is_any_platform() {
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
            LAMBDA_EXPR => self.lambda(node, expected),
            MATCH_EXPR => self.match_expr(node, expected),
            WHEN_EXPR => self.when_expr(node, expected),
            IF_EXPR => self.if_expr(node, expected),
            ISSET_EXPR | EMPTY_EXPR => {
                if let Some(args) = node.child(ARG_LIST) {
                    for a in args.children_of(ARG) {
                        for e in exprs_of(&a) {
                            self.expr(&e, None);
                        }
                    }
                }
                if self.stir() {
                    Ty::bool()
                } else {
                    Ty::php_bool()
                }
            }
            EVAL_EXPR | INCLUDE_EXPR => {
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
                    let t = self.expr(&e, None);
                    self.check_stringable(&e, &t, "print");
                }
                Ty::literal(LitValue::Int(1), self.lit_family())
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
            RECORD_LITERAL => self.record_literal(node, expected),
            _ => Ty::error(),
        }
    }

    fn check_stringable(&mut self, node: &SyntaxNode, t: &Ty, what: &str) {
        if !self.stir() || t.is_error() {
            return;
        }
        let ok = [
            Ty::string(),
            Ty::int(),
            Ty::float(),
            Ty::php_string(),
            Ty::php_int(),
            Ty::php_float(),
        ]
        .iter()
        .any(|s| self.assignable(t, s))
            || self.assignable(t, &Ty::class("Stringable", Vec::new()));
        if !ok {
            let hint = if t.is_any_platform() {
                "; narrow the untyped value first"
            } else {
                ""
            };
            self.error(
                node.text_range(),
                codes::TYPE_MISMATCH,
                format!("`{t}` can't be used in {what}{hint}"),
            );
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
                if self.stir() && !l.kind.dollar() {
                    self.error(
                        node.text_range(),
                        codes::SIGIL_MISMATCH,
                        format!("`{name}` is immutable; refer to it as `{name}` (without `$`)"),
                    );
                }
                l.ty
            }
            None => {
                if let Some(t) = self.implicit_property(node, name, true) {
                    return t;
                }
                if self.stir() || self.fns.len() > 1 {
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

    fn name_expr(&mut self, node: &SyntaxNode) -> Ty {
        let text = text_of(node);
        match text.to_ascii_lowercase().as_str() {
            "true" => return Ty::literal(LitValue::Bool(true), self.lit_family()),
            "false" => return Ty::literal(LitValue::Bool(false), self.lit_family()),
            "null" => return Ty::null(),
            _ => {}
        }
        if self.stir()
            && !text.contains('\\')
            && let Some(l) = self.lookup(&text).cloned()
        {
            if l.kind.dollar() && self.in_template == 0 && l.kind != LocalKind::This {
                self.error(
                    node.text_range(),
                    codes::SIGIL_MISMATCH,
                    format!("`${text}` is mutable; refer to it as `${text}`"),
                );
            }
            return l.ty;
        }
        if !text.contains('\\') {
            if let Some(t) = self.implicit_property(node, &text, false) {
                return t;
            }
            if self.in_template > 0
                && let Some(t) = self.implicit_property(node, &text, true)
            {
                return t;
            }
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
            Some(loc) => self.const_type(loc),
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
        let stir = loc.file.dialect(self.ctx.db) == Dialect::Stir;
        let ty = match node.kind() {
            LITERAL => match op_token(&node).map(|t| (t.kind(), t.text().to_owned())) {
                Some((INT_NUMBER, _)) => Ty::int(),
                Some((FLOAT_NUMBER, _)) => Ty::float(),
                _ => Ty::any_platform(),
            },
            STRING_LITERAL | INTERP_STRING => Ty::string(),
            NAME_EXPR => match text_of(&node).to_ascii_lowercase().as_str() {
                "true" | "false" => Ty::bool(),
                "null" => Ty::null(),
                _ => Ty::any_platform(),
            },
            PREFIX_EXPR => Ty::union([Ty::int(), Ty::float()]),
            _ => Ty::any_platform(),
        };
        let _ = stir;
        ty
    }

    // ------------------------------------------------------------ operators

    fn binary(&mut self, node: &SyntaxNode, expected: Option<&Ty>) -> Ty {
        let es = exprs_of(node);
        let op = node
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| !t.kind().is_trivia())
            .map(|t| (t.kind(), t.text_range()));
        let Some((op, op_range)) = op else {
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
                let l = self.expr(&es[0], expected.map(|e| e.clone().nullable()).as_ref());
                let r = self.expr(&es[1], expected);
                (l, r)
            }
            _ => (self.expr(&es[0], None), self.expr(&es[1], None)),
        };
        if l.is_error() || r.is_error() {
            return match op {
                EQ2 | EQ3 | NEQ | NEQ2 | L_ANGLE | R_ANGLE | LTEQ | GTEQ | AMP2 | PIPE2 => {
                    self.bool_ty()
                }
                _ => Ty::error(),
            };
        }
        let stir = self.stir();
        match op {
            PLUS | MINUS | STAR | SLASH | PERCENT | POW => {
                self.require_numeric(es[0].text_range(), &l, "arithmetic");
                self.require_numeric(es[1].text_range(), &r, "arithmetic");
                self.arith_result(op, &l, &r)
            }
            DOT => {
                if stir {
                    for (e, t) in [(&es[0], &l), (&es[1], &r)] {
                        let ok = self.assignable(t, &Ty::string())
                            || self.assignable(t, &Ty::php_string());
                        if !ok {
                            self.error(
                                e.text_range(),
                                codes::INVALID_OPERAND,
                                format!("`.` concatenates strings, found `{t}`; use a template (`\"${{...}}\"`) or `toString()`"),
                            );
                        }
                    }
                    Ty::string()
                } else {
                    Ty::php_string()
                }
            }
            EQ2 | EQ3 | NEQ | NEQ2 => {
                self.check_comparable(op_range, &l, &r);
                self.bool_ty()
            }
            L_ANGLE | R_ANGLE | LTEQ | GTEQ | SPACESHIP => {
                if stir {
                    let both_num = self.is_numeric(&l) && self.is_numeric(&r);
                    let both_str =
                        self.assignable(&l, &Ty::string()) && self.assignable(&r, &Ty::string());
                    let orderable = Ty::union([Ty::int(), Ty::float(), Ty::string()]);
                    let both_orderable = self.assignable(&l, &orderable)
                        && self.assignable(&r, &orderable)
                        && stir_types::are_comparable(self.ctx, &l, &r);
                    if !both_num && !both_str && !both_orderable {
                        self.error(
                            op_range,
                            codes::INCOMPATIBLE_COMPARISON,
                            format!("cannot order `{l}` and `{r}`"),
                        );
                    }
                }
                if op == SPACESHIP {
                    if stir { Ty::int() } else { Ty::php_int() }
                } else {
                    self.bool_ty()
                }
            }
            AMP2 | PIPE2 | AND_KW | OR_KW | XOR_KW => {
                if stir {
                    for (e, t) in [(&es[0], &l), (&es[1], &r)] {
                        if !self.assignable(t, &Ty::bool()) && !self.assignable(t, &Ty::php_bool())
                        {
                            self.error(
                                e.text_range(),
                                codes::INVALID_OPERAND,
                                format!("logical operators need `Bool` operands, found `{t}`"),
                            );
                        }
                    }
                }
                self.bool_ty()
            }
            AMP | PIPE | CARET | SHL | SHR => {
                if stir {
                    for (e, t) in [(&es[0], &l), (&es[1], &r)] {
                        if !self.assignable(t, &Ty::int()) && !self.assignable(t, &Ty::php_int()) {
                            self.error(
                                e.text_range(),
                                codes::INVALID_OPERAND,
                                format!("bitwise operators need `Int` operands, found `{t}`"),
                            );
                        }
                    }
                    Ty::int()
                } else {
                    Ty::php_int()
                }
            }
            QUESTION2 => {
                if stir
                    && !stir_types::is_nullable(self.ctx, &l)
                    && !matches!(l.kind(), TyKind::Platform(_))
                    && !l.is_any_platform()
                {
                    self.warn(
                        es[0].text_range(),
                        codes::UNNECESSARY_NULL_CHECK,
                        format!("left side of `??` is never null (`{l}`)"),
                    );
                }
                stir_types::join(self.ctx, &stir_types::without_null(&l), &r)
            }
            PIPE_GT => match strip_platform(&r).kind() {
                TyKind::Fn(f) => f.ret.clone(),
                TyKind::Callable { sig: Some(s), .. } => s.ret.clone(),
                _ => Ty::any_platform(),
            },
            _ => Ty::error(),
        }
    }

    fn bool_ty(&self) -> Ty {
        if self.stir() {
            Ty::bool()
        } else {
            Ty::php_bool()
        }
    }

    fn is_numeric(&self, t: &Ty) -> bool {
        [Ty::int(), Ty::float(), Ty::php_int(), Ty::php_float()]
            .iter()
            .any(|n| self.assignable(t, n))
            || self.assignable(t, &Ty::union([Ty::int(), Ty::float()]))
            || self.assignable(t, &Ty::union([Ty::php_int(), Ty::php_float()]))
    }

    fn require_numeric(&mut self, range: TextRange, t: &Ty, what: &str) {
        if t.is_error() {
            return;
        }
        if !self.is_numeric(t)
            && (self.stir() || !matches!(t.kind(), TyKind::Mixed | TyKind::AnyPlatform))
        {
            if t.is_any_platform() {
                self.error(
                    range,
                    codes::INVALID_OPERAND,
                    format!("{what} on an untyped value; narrow it first (`is` / `as`)"),
                );
            } else if self.stir() {
                self.error(
                    range,
                    codes::INVALID_OPERAND,
                    format!("{what} needs numbers, found `{t}`"),
                );
            }
        }
    }

    fn arith_result(&self, op: SyntaxKind, l: &Ty, r: &Ty) -> Ty {
        let php = !self.stir() || l.is_php() || r.is_php();
        let is_float =
            |t: &Ty| self.assignable(t, &Ty::float()) || self.assignable(t, &Ty::php_float());
        let is_int = |t: &Ty| self.assignable(t, &Ty::int()) || self.assignable(t, &Ty::php_int());
        let (int, float) = if php {
            (Ty::php_int(), Ty::php_float())
        } else {
            (Ty::int(), Ty::float())
        };
        if is_int(l) && is_int(r) {
            match op {
                // StirPHP integer division truncates (lowered to `intdiv`); PHP's `/` may produce a float.
                SLASH if php => Ty::union([int, float]),
                POW => Ty::union([int, float]),
                _ => int,
            }
        } else if (is_float(l) || is_int(l)) && (is_float(r) || is_int(r)) {
            if op == PERCENT { int } else { float }
        } else {
            Ty::union([int, float])
        }
    }

    fn check_comparable(&mut self, range: TextRange, l: &Ty, r: &Ty) {
        if l.is_error() || r.is_error() || !self.stir() {
            return;
        }
        if !stir_types::are_comparable(self.ctx, l, r) {
            self.error(
                range,
                codes::INCOMPATIBLE_COMPARISON,
                format!("comparing incompatible types `{l}` and `{r}`"),
            );
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
                let t = self.expr(e, None);
                if self.stir()
                    && !t.is_error()
                    && !self.assignable(&t, &Ty::bool())
                    && !self.assignable(&t, &Ty::php_bool())
                {
                    self.error(
                        e.text_range(),
                        codes::INVALID_OPERAND,
                        format!("`!` needs a `Bool`, found `{t}`"),
                    );
                }
                self.bool_ty()
            }
            MINUS | PLUS => {
                let t = self.expr(e, None);
                self.require_numeric(e.text_range(), &t, "unary arithmetic");
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
                self.require_numeric(e.text_range(), &t, "`++`/`--`");
                widen_literals(&t)
            }
            AMP => self.expr(e, expected),
            _ => self.expr(e, expected),
        }
    }

    // ------------------------------------------------------------ assignment

    fn check_mutable_target(&mut self, target: &SyntaxNode) {
        match target.kind() {
            NAME_EXPR if self.stir() => {
                let name = text_of(target);
                if let Some(l) = self.lookup(&name).cloned()
                    && !l.kind.mutable()
                {
                    let msg = match l.kind {
                        LocalKind::Param(_) => format!(
                            "parameter `{name}` is immutable; declare it `mut {name}` to reassign it"
                        ),
                        _ => format!(
                            "`{name}` is a `val` and can't be reassigned; declare it with `var ${name}`"
                        ),
                    };
                    self.error(target.text_range(), codes::REASSIGN_IMMUTABLE, msg);
                }
            }
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
            PROPERTY_FETCH_EXPR | NULLSAFE_PROPERTY_FETCH_EXPR
                if target.kind() == NULLSAFE_PROPERTY_FETCH_EXPR =>
            {
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
                match self.lookup(&n).cloned() {
                    Some(l) => {
                        let vt = self.expr(rhs, Some(&l.declared));
                        if n == "this" {
                            self.check_mutable_target(lhs);
                        } else if self.stir() {
                            if !l.kind.dollar() {
                                self.error(
                                    lhs.text_range(),
                                    codes::SIGIL_MISMATCH,
                                    format!("`{n}` is immutable; it can't be assigned"),
                                );
                            } else {
                                self.check_assign(rhs.text_range(), &vt, &l.declared, "assignment");
                                if let Some(loc) = self.lookup_mut(&n) {
                                    loc.ty = narrow_assigned(&loc.declared, &vt);
                                }
                            }
                        } else if let Some(loc) = self.lookup_mut(&n) {
                            // PHP variables are dynamically typed
                            loc.ty = widen_literals(&vt);
                            loc.declared = loc.ty.clone();
                        }
                        self.types.insert(lhs.text_range(), l.declared.clone());
                        vt
                    }
                    None if self.stir() && self.implicit_property(lhs, &n, true).is_some() => {
                        let pt = self.implicit_property(lhs, &n, true).unwrap();
                        let vt = self.expr(rhs, Some(&pt));
                        self.check_assign(rhs.text_range(), &vt, &pt, "assignment");
                        vt
                    }
                    None => {
                        let vt = self.expr(rhs, None);
                        if self.stir() {
                            self.error(
                                lhs.text_range(),
                                codes::UNDECLARED_ASSIGNMENT,
                                format!("`{name}` is not declared; declare it with `var {name} = ...` (or `val {n} = ...`)"),
                            );
                        } else {
                            self.declare(&n, widen_literals(&vt), LocalKind::Php, lhs.text_range());
                        }
                        vt
                    }
                }
            }
            ARRAY_EXPR | LIST_EXPR if op == EQ => {
                let vt = self.expr(rhs, None);
                self.bind_target(lhs, vt.clone());
                vt
            }
            _ => {
                let lt = self.expr(lhs, None);
                self.check_mutable_target(lhs);
                self.check_assign_target_writable(lhs);
                let declared = self.declared_target_type(lhs).unwrap_or(lt.clone());
                let vt = self.expr(rhs, Some(&declared));
                let result = match op {
                    EQ => vt.clone(),
                    QUESTION2_EQ => stir_types::join(self.ctx, &stir_types::without_null(&lt), &vt),
                    DOT_EQ => {
                        if self.stir() {
                            Ty::string()
                        } else {
                            Ty::php_string()
                        }
                    }
                    PLUS_EQ | MINUS_EQ | STAR_EQ | SLASH_EQ | PERCENT_EQ | POW_EQ => {
                        self.require_numeric(lhs.text_range(), &lt, "arithmetic");
                        self.require_numeric(rhs.text_range(), &vt, "arithmetic");
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
                if op == DOT_EQ
                    && self.stir()
                    && !self.assignable(&vt, &Ty::string())
                    && !vt.is_error()
                {
                    self.error(
                        rhs.text_range(),
                        codes::INVALID_OPERAND,
                        format!("`.=` appends strings, found `{vt}`"),
                    );
                }
                self.check_assign(rhs.text_range(), &result, &declared, "assignment");
                if let Some(n) = self.local_name(lhs)
                    && let Some(l) = self.lookup_mut(&n)
                {
                    l.ty = narrow_assigned(&l.declared, &result);
                }
                result
            }
        }
    }

    fn local_name(&self, node: &SyntaxNode) -> Option<String> {
        match node.kind() {
            VARIABLE_EXPR => Some(text_of(node).trim_start_matches('$').to_owned()),
            NAME_EXPR if self.stir() => Some(text_of(node)),
            _ => None,
        }
    }

    /// The declared (un-narrowed) type of an assignment target.
    fn declared_target_type(&self, node: &SyntaxNode) -> Option<Ty> {
        let n = self.local_name(node)?;
        self.lookup(&n).map(|l| l.declared.clone())
    }

    /// Writes to `val`/readonly properties and read-only collections.
    fn check_assign_target_writable(&mut self, lhs: &SyntaxNode) {
        match lhs.kind() {
            PROPERTY_FETCH_EXPR => {
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
            INDEX_EXPR => {
                let es = exprs_of(lhs);
                let Some(base) = es.first() else { return };
                let Some(bt) = self.types.get(&base.text_range()).cloned() else {
                    return;
                };
                if matches!(
                    strip_platform(&bt).kind(),
                    TyKind::List(_) | TyKind::Map(_, _)
                ) {
                    self.error(
                        lhs.text_range(),
                        codes::REASSIGN_IMMUTABLE,
                        format!(
                            "`{bt}` is read-only; use a `MutableList`/`MutableMap` to modify it"
                        ),
                    );
                }
                // the base binding itself must be assignable (`$xs[] = 1` changes `$xs`)
                if let Some(n) = self.local_name(base)
                    && let Some(l) = self.lookup(&n).cloned()
                    && !l.kind.mutable()
                    && self.stir()
                    && !matches!(
                        strip_platform(&bt).kind(),
                        TyKind::List(_) | TyKind::Map(_, _)
                    )
                {
                    // MutableList in a `val` is fine for Kotlin-like references, but PHP
                    // arrays are values: modifying it requires a mutable binding.
                    self.error(
                        base.text_range(),
                        codes::REASSIGN_IMMUTABLE,
                        format!("`{n}` is immutable; modifying its contents requires `var ${n}`"),
                    );
                }
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------ collections

    fn index_type(&self, base: &Ty, _index: Option<&Ty>) -> Option<Ty> {
        match base.kind() {
            TyKind::List(e) | TyKind::MutableList(e) => Some(e.clone()),
            TyKind::Map(_, v) | TyKind::MutableMap(_, v) => Some(v.clone().nullable()),
            TyKind::PhpList { elem, .. } => Some(elem.clone()),
            TyKind::Array { value, .. } => Some(value.clone()),
            TyKind::String | TyKind::PhpString => Some(base.clone()),
            TyKind::Literal(l) if matches!(l.value, LitValue::String(_)) => Some(Ty::string()),
            TyKind::Shape(_) | TyKind::Mixed => Some(Ty::mixed()),
            TyKind::Error => Some(Ty::error()),
            TyKind::Class { .. } => Some(Ty::any_platform()),
            _ => None,
        }
    }

    fn array_literal(&mut self, node: &SyntaxNode, expected: Option<&Ty>) -> Ty {
        let elems = node.children_of(ARRAY_ELEM);
        let exp = expected.map(strip_platform);
        let (exp_key, exp_val, mutable) = match exp.as_ref().map(|e| e.kind().clone()) {
            Some(TyKind::List(e)) => (None, Some(e), false),
            Some(TyKind::MutableList(e)) => (None, Some(e), true),
            Some(TyKind::Map(k, v)) => (Some(k), Some(v), false),
            Some(TyKind::MutableMap(k, v)) => (Some(k), Some(v), true),
            _ => (None, None, false),
        };
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
        if !self.stir() {
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
            return Ty::php_list(if vals.is_empty() {
                Ty::never()
            } else {
                widen_literals(&Ty::union(vals))
            });
        }
        let join = |this: &Self, tys: Vec<Ty>, exp: Option<&Ty>| -> Ty {
            match exp {
                Some(e) if tys.iter().all(|t| this.assignable(t, e)) => e.clone(),
                _ if tys.is_empty() => exp.cloned().unwrap_or_else(Ty::nothing),
                _ => widen_literals(
                    &tys.into_iter()
                        .reduce(|a, b| stir_types::join(this.ctx, &a, &b))
                        .unwrap(),
                ),
            }
        };
        if keyed || exp_key.is_some() {
            let k = join(self, keys, exp_key.as_ref());
            let v = join(self, vals, exp_val.as_ref());
            if mutable {
                Ty::mutable_map(k, v)
            } else {
                Ty::map(k, v)
            }
        } else {
            let v = join(self, vals, exp_val.as_ref());
            if mutable {
                Ty::mutable_list(v)
            } else {
                Ty::list(v)
            }
        }
    }

    fn record_literal(&mut self, node: &SyntaxNode, expected: Option<&Ty>) -> Ty {
        let Some(name_node) = node.child(NAME_EXPR) else {
            return Ty::error();
        };
        let name = text_of(&name_node);
        let scope = self.scope(node);
        let fqn = scope.resolve_class(&name);
        let Some(model) = self.ctx.class(fqn.as_str()) else {
            self.error(
                name_node.text_range(),
                codes::UNDEFINED_CLASS,
                format!("unknown record `{name}`"),
            );
            for f in node.children_of(RECORD_LITERAL_FIELD) {
                for e in exprs_of(&f) {
                    self.expr(&e, None);
                }
            }
            return Ty::error();
        };
        if model.kind() != ClassKind::Record {
            self.error(
                name_node.text_range(),
                codes::TYPE_MISMATCH,
                format!(
                    "`{}` is not a record; use `new {}(...)`",
                    model.name, model.name
                ),
            );
            return Ty::error();
        }
        // generic records: infer arguments from the fields
        let mut inf = Inference::new(model.params.iter().map(|p| p.id));
        if let Some(e) = expected
            && let TyKind::Record(r) = strip_platform(e).kind()
            && r.name
                .as_deref()
                .is_some_and(|n| n.eq_ignore_ascii_case(&model.name))
        {
            for (p, a) in model.params.iter().zip(&r.args) {
                inf.solved.insert(p.id, a.clone());
            }
        }
        let generic = model.self_ty();
        let generic_rec = self
            .ctx
            .record_ty(&model, model.params.iter().map(|p| p.ty.clone()).collect());
        let _ = generic;
        let TyKind::Record(decl) = generic_rec.kind() else {
            return Ty::error();
        };
        let mut seen = Vec::new();
        let fields = node.children_of(RECORD_LITERAL_FIELD);
        let mut values = Vec::new();
        for f in &fields {
            let Some(fname) = f.name_text() else { continue };
            let Some(df) = decl.field(&fname) else {
                self.error(
                    f.text_range(),
                    codes::UNDEFINED_MEMBER,
                    format!("record `{}` has no field `{fname}`", model.name),
                );
                for e in exprs_of(f) {
                    self.expr(&e, None);
                }
                continue;
            };
            if seen.contains(&fname) {
                self.error(
                    f.text_range(),
                    codes::DUPLICATE_DECLARATION,
                    format!("field `{fname}` is set twice"),
                );
            }
            seen.push(fname.clone());
            let exp = inf.apply(&df.ty);
            let exp = if inf.has_unsolved(&exp) {
                None
            } else {
                Some(exp)
            };
            if let Some(e) = exprs_of(f).first() {
                let t = self.expr(e, exp.as_ref());
                inf.unify(self.ctx, &df.ty, &t);
                values.push((e.text_range(), t, df.ty.clone()));
            }
        }
        for (df, def) in decl.fields.iter().zip(&model.def.fields) {
            if !seen.iter().any(|s| s == df.name) && !def.has_default {
                self.error(
                    node.text_range(),
                    codes::ARGUMENT_COUNT,
                    format!("missing field `{}` of record `{}`", df.name, model.name),
                );
            }
        }
        inf.finish(&Ty::nothing());
        for (range, t, decl_ty) in values {
            let want = inf.apply(&decl_ty);
            self.check_assign(range, &t, &want, "record field");
        }
        let args: Vec<Ty> = model
            .params
            .iter()
            .map(|p| inf.solved.get(&p.id).cloned().unwrap_or_else(Ty::nothing))
            .collect();
        self.ctx.record_ty(&model, args)
    }

    // ------------------------------------------------------------ calls

    fn args_of(&self, call: &SyntaxNode) -> Vec<SyntaxNode> {
        call.children_of(ARG_LIST)
            .iter()
            .flat_map(|a| {
                a.children()
                    .filter(|c| {
                        matches!(
                            c.kind(),
                            ARG | NAMED_ARG | CALLABLE_PLACEHOLDER | LAMBDA_EXPR
                        )
                    })
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
        receiver: Option<&Ty>,
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
        if let (Some(r), Some(rt)) = (&sig.receiver, receiver) {
            inf.unify(self.ctx, &subst_cls(r), rt);
        }
        if let Some(e) = expected
            && !e.is_error()
            && inf.has_unsolved(&subst_cls(&sig.ret))
        {
            // bidirectional: `val xs: List<Int> = emptyList()`
            let mut probe = inf.clone();
            probe.unify(self.ctx, &subst_cls(&sig.ret), e);
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
        // pass 1: non-lambda arguments
        let mut arg_tys: Vec<Option<Ty>> = vec![None; args.len()];
        let value_of = |a: &SyntaxNode| -> Option<SyntaxNode> {
            if a.kind() == LAMBDA_EXPR {
                Some(a.clone())
            } else {
                exprs_of(a).into_iter().next()
            }
        };
        for (i, a) in args.iter().enumerate() {
            let Some(v) = value_of(a) else { continue };
            if matches!(v.kind(), LAMBDA_EXPR | CLOSURE_EXPR | ARROW_FN_EXPR) {
                continue;
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
        // pass 2: lambdas, with parameter types known from pass 1
        for (i, a) in args.iter().enumerate() {
            let Some(v) = value_of(a) else { continue };
            if !matches!(v.kind(), LAMBDA_EXPR | CLOSURE_EXPR | ARROW_FN_EXPR) {
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
        inf.finish(&Ty::nothing());
        for (i, a) in args.iter().enumerate() {
            let (Some(p), Some(t)) = (assigned[i], &arg_tys[i]) else {
                continue;
            };
            let param = &sig.params[p];
            let want = inf.apply(&subst_cls(&param.ty));
            let range = value_of(a)
                .map(|v| v.text_range())
                .unwrap_or(a.text_range());
            if param.by_ref
                && let Some(v) = value_of(a)
                && !matches!(
                    v.kind(),
                    VARIABLE_EXPR
                        | INDEX_EXPR
                        | PROPERTY_FETCH_EXPR
                        | STATIC_PROPERTY_FETCH_EXPR
                        | NAME_EXPR
                )
                && self.stir()
            {
                self.error(
                    range,
                    codes::TYPE_MISMATCH,
                    format!(
                        "parameter `{}` is passed by reference; pass a variable",
                        param.name
                    ),
                );
            }
            // external PHP functions accept untyped values
            if sig.external && t.is_any_platform() {
                continue;
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
            // a local function value
            if self.stir()
                && !text.contains('\\')
                && let Some(l) = self.lookup(&text).cloned()
            {
                self.types.insert(callee.text_range(), l.ty.clone());
                return self.call_value(node, &l.ty, expected);
            }
            if self.stir()
                && !text.contains('\\')
                && let Some(this) = self.this_ty()
                && let Some((owner, m, map)) = self.ctx.find_method(&this, &text)
            {
                self.implicit_this.insert(callee.text_range());
                let sig = self.ctx.method_sig(&owner, m);
                let ret = self.check_args(node, &sig, &map, None, expected, &[]);
                return resolve_self(&ret, &this);
            }
            let scope = self.scope(node);
            let r = scope.resolve_function(&text);
            let loc = self.ctx.function(r.primary.as_str()).or_else(|| {
                r.fallback
                    .as_ref()
                    .and_then(|f| self.ctx.function(f.as_str()))
            });
            let Some(loc) = loc else {
                self.error(
                    callee.text_range(),
                    codes::UNDEFINED_FUNCTION,
                    format!("unknown function `{text}`"),
                );
                self.check_args_untyped(node);
                return Ty::error();
            };
            let sig = self.ctx.function_sig(loc);
            return self.check_args(node, &sig, &FxHashMap::default(), None, expected, &[]);
        }
        let ct = self.expr(&callee, None);
        self.call_value(node, &ct, expected)
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
                            binding: Binding::Val,
                            range: node.text_range(),
                        })
                        .collect(),
                    ret: f.ret.clone(),
                    ret_declared: true,
                    receiver: None,
                    external: false,
                };
                self.check_args(node, &sig, &FxHashMap::default(), None, expected, &[])
            }
            None => {
                if !ct.is_error()
                    && !matches!(
                        ct.kind(),
                        TyKind::Callable { sig: None, .. } | TyKind::Mixed
                    )
                    && !(ct.is_any_platform() && !self.stir())
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
            if a.kind() == LAMBDA_EXPR {
                self.expr(&a, None);
            }
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
        if self.stir() && nullable && !nullsafe && !bt.is_error() && !bt.is_any_platform() {
            self.error(
                base.text_range(),
                codes::NULLABLE_ACCESS,
                format!("`{bt}` may be null; use `?->`, `!!` or check for null first"),
            );
        }
        if self.stir()
            && nullsafe
            && !nullable
            && !bt.is_error()
            && !matches!(bt.kind(), TyKind::Platform(_))
            && !bt.is_any_platform()
        {
            self.warn(
                base.text_range(),
                codes::UNNECESSARY_NULL_CHECK,
                format!("`?->` on a non-null value of type `{bt}`"),
            );
        }
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
        if nullable { t.nullable() } else { t }
    }

    fn member_type(&mut self, recv: &Ty, name: &str, range: TextRange) -> Ty {
        match recv.kind() {
            TyKind::Error => Ty::error(),
            TyKind::Nothing | TyKind::Never => Ty::nothing(),
            TyKind::Record(r) => match r.field(name) {
                Some(f) => f.ty.clone(),
                None => {
                    self.error(
                        range,
                        codes::UNDEFINED_MEMBER,
                        format!("`{recv}` has no field `{name}`"),
                    );
                    Ty::error()
                }
            },
            TyKind::Class { .. } | TyKind::SelfType { .. } | TyKind::Intersection(_) => {
                let cls = class_of(recv);
                match self.ctx.find_property(&cls, name) {
                    Some(p) => {
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
            _ if recv.is_any_platform() => {
                if self.stir() {
                    self.error(range, codes::INVALID_OPERAND, format!("can't access `{name}` on an untyped value; narrow it first (`is` / `as`)"));
                }
                Ty::error()
            }
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

    fn has_magic(&self, ty: &Ty, name: &str) -> bool {
        self.ctx.find_method(ty, name).is_some()
    }

    fn class_known(&self, ty: &Ty) -> bool {
        match ty.kind() {
            TyKind::Class { name, .. } => self.ctx.class(name).is_some(),
            _ => false,
        }
    }

    fn check_visibility(
        &mut self,
        range: TextRange,
        owner: &Rc<ClassModel<'db>>,
        vis: Visibility,
        what: &str,
    ) {
        let ok = match vis {
            Visibility::Public => true,
            Visibility::Private => self.class.as_ref().is_some_and(|c| c.key == owner.key),
            Visibility::Protected => self.class.as_ref().is_some_and(|c| {
                c.key == owner.key
                    || self.ctx.is_subclass(&c.name, &owner.name)
                    || self.ctx.is_subclass(&owner.name, &c.name)
            }),
        };
        if !ok {
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
        if recv.is_error() {
            self.check_args_untyped(node);
            return Ty::error();
        }
        let cls = class_of(recv);
        if matches!(cls.kind(), TyKind::Class { .. })
            && let Some((owner, m, map)) = self.ctx.find_method(&cls, name)
        {
            self.check_visibility(
                range,
                &owner,
                m.mods.visibility,
                &format!("method `{name}`"),
            );
            let sig = self.ctx.method_sig(&owner, m);
            let mut map = map;
            // `static` return types resolve to the receiver
            let ret = self.check_args(node, &sig, &map, None, expected, &[]);
            map.clear();
            return resolve_self(&ret, recv);
        }
        // a property holding a closure: `($this->f)()` is required in PHP, so not here
        // extension functions (and the StirPHP stdlib)
        if let Some(t) = self.call_extension(node, recv, name, expected) {
            return t;
        }
        if matches!(cls.kind(), TyKind::Class { .. })
            && (self.has_magic(&cls, "__call") || !self.class_known(&cls))
        {
            self.check_args_untyped(node);
            return Ty::any_platform();
        }
        if recv.is_any_platform() || matches!(recv.kind(), TyKind::Mixed | TyKind::Object) {
            if self.stir() {
                self.error(
                    range,
                    codes::INVALID_OPERAND,
                    format!(
                        "can't call `{name}` on an untyped value; narrow it first (`is` / `as`)"
                    ),
                );
            }
            self.check_args_untyped(node);
            return if self.stir() {
                Ty::error()
            } else {
                Ty::any_platform()
            };
        }
        self.error(
            range,
            codes::UNDEFINED_MEMBER,
            format!("`{recv}` has no method `{name}`"),
        );
        self.check_args_untyped(node);
        Ty::error()
    }

    /// Calls `recv->name(...)` through an extension function, if one applies.
    fn call_extension(
        &mut self,
        node: &SyntaxNode,
        recv: &Ty,
        name: &str,
        expected: Option<&Ty>,
    ) -> Option<Ty> {
        let candidates = self.ctx.extensions(name);
        let mut best: Option<Rc<FnSig>> = None;
        for loc in candidates {
            let sig = self.ctx.function_sig(loc);
            let Some(r) = &sig.receiver else { continue };
            let mut inf = Inference::new(sig.type_params.iter().map(|p| p.id));
            inf.unify(self.ctx, r, recv);
            inf.finish(&Ty::nothing());
            let want = inf.apply(r);
            if self.assignable(recv, &want) {
                // prefer the most specific receiver
                let better = match &best {
                    None => true,
                    Some(b) => {
                        let br = b.receiver.clone().unwrap();
                        self.assignable(&want, &br) && !self.assignable(&br, &want)
                    }
                };
                if better {
                    best = Some(sig);
                }
            }
        }
        let sig = best?;
        if sig.receiver.as_ref().is_some_and(is_by_ref_receiver)
            && let Some(base) = node
                .children()
                .next()
                .and_then(|f| exprs_of(&f).into_iter().next())
        {
            self.check_by_ref_receiver(&base, name);
        }
        Some(self.check_args(node, &sig, &FxHashMap::default(), Some(recv), expected, &[]))
    }

    /// `xs->add(1)` modifies `xs` in place: it must be a mutable variable or property.
    fn check_by_ref_receiver(&mut self, base: &SyntaxNode, method: &str) {
        let ok = match base.kind() {
            VARIABLE_EXPR | NAME_EXPR => {
                match self.local_name(base).and_then(|n| self.lookup(&n).cloned()) {
                    Some(l) => l.kind.mutable(),
                    // implicit `$this->prop`
                    None => self.implicit_this.contains(&base.text_range()),
                }
            }
            PROPERTY_FETCH_EXPR | STATIC_PROPERTY_FETCH_EXPR | INDEX_EXPR => true,
            _ => false,
        };
        if !ok {
            self.error(
                base.text_range(),
                codes::REASSIGN_IMMUTABLE,
                format!("`{method}` modifies its receiver in place; call it on a mutable variable (`var $x`) or property"),
            );
        }
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
                if c.kind() == ClassKind::Record {
                    self.ctx
                        .record_ty(&c, c.params.iter().map(|_| Ty::nothing()).collect())
                } else {
                    Ty::class(
                        c.name.clone(),
                        c.params.iter().map(|p| p.ty.clone()).collect(),
                    )
                }
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
                let ret = self.check_args(node, &sig, &map, None, expected, &extra);
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
                self.check_visibility(
                    var.text_range(),
                    &p.class,
                    p.visibility,
                    &format!("property `{name}`"),
                );
                p.ty
            }
            None => {
                self.error(
                    var.text_range(),
                    codes::UNDEFINED_MEMBER,
                    format!("`{cls}` has no static property `${name}`"),
                );
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
            return if self.stir() {
                Ty::string()
            } else {
                Ty::class_string(None)
            };
        }
        let cls = self.static_base(node);
        if cls.is_error() {
            return Ty::error();
        }
        match self.ctx.find_const(&cls, &name) {
            Some(t) => t,
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
            if let TyKind::Record(_) = cls.kind() {
                self.error(
                    target.text_range(),
                    codes::INSTANTIATION,
                    "records are created with a literal: `Name { field = value }`",
                );
            }
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
            _ if model.def.mods.is_abstract || model.def.mods.is_sealed => {
                self.error(
                    target.text_range(),
                    codes::INSTANTIATION,
                    format!("`{}` is abstract and can't be instantiated", model.name),
                );
            }
            _ => {}
        }
        // constructor: primary constructor or `__construct` (possibly inherited)
        let vars: Vec<_> = model.params.iter().map(|p| p.id).collect();
        let exp_args = expected.and_then(|e| match strip_platform(e).kind() {
            TyKind::Class { name: en, args } if en.eq_ignore_ascii_case(&model.name) => {
                Some(args.clone())
            }
            _ => None,
        });
        let sig = if let Some(params) = &model.def.primary_ctor {
            let cx = self.ctx.class_cx(&model);
            let mut sink = Vec::new();
            Some(FnSig {
                name: SmolStr::new(format!("{}::__construct", model.name)),
                type_params: Vec::new(),
                params: params
                    .iter()
                    .map(|p| self.ctx.param_sig(p, &cx, &mut sink))
                    .collect(),
                ret: model.self_ty(),
                ret_declared: true,
                receiver: None,
                external: false,
            })
        } else {
            self.ctx
                .find_method(&model.self_ty(), "__construct")
                .map(|(owner, m, _)| {
                    let mut s = self.ctx.method_sig(&owner, m);
                    s.ret = model.self_ty();
                    s
                })
        };

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
                self.check_args(node, &sig, &FxHashMap::default(), None, None, &extra)
            }
            None => {
                let n = self.args_of(node).len();
                if n > 0 && self.stir() {
                    self.error(
                        node.text_range(),
                        codes::ARGUMENT_COUNT,
                        format!("`{}` has no constructor parameters", model.name),
                    );
                }
                self.check_args_untyped(node);
                model.self_ty()
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
        if arrow || self.stir() {
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
                        .unwrap_or_else(|| {
                            if self.stir() {
                                Ty::error()
                            } else {
                                Ty::mixed()
                            }
                        }),
                };
                let (name, binding) = match p.token(VARIABLE) {
                    Some(v) => (v.text()[1..].to_owned(), Binding::Php),
                    None => (
                        p.name_text().unwrap_or_default(),
                        if p.has_ident("mut") {
                            Binding::Mut
                        } else {
                            Binding::Val
                        },
                    ),
                };
                self.declare(&name, ty.clone(), LocalKind::Param(binding), p.text_range());
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

    /// StirPHP `{ a, b -> body }`: the value of the last expression statement is the result.
    fn lambda(&mut self, node: &SyntaxNode, expected: Option<&Ty>) -> Ty {
        let exp_fn = expected.and_then(|e| match strip_platform(e).kind() {
            TyKind::Fn(f) => Some(f.clone()),
            TyKind::Callable { sig: Some(s), .. } => Some(s.clone()),
            _ => None,
        });
        self.push_frame();
        let mut params = Vec::new();
        match node.child(LAMBDA_PARAM_LIST) {
            Some(pl) => {
                for (i, p) in pl.children_of(PARAM).iter().enumerate() {
                    let ty = match stir_hir::first_type(p) {
                        Some(t) => self.lower_type_node(&t),
                        None => {
                            match exp_fn
                                .as_ref()
                                .and_then(|f| f.params.get(i).map(|p| p.ty.clone()))
                            {
                                Some(t) => t,
                                None => {
                                    self.error(p.text_range(), codes::MISSING_TYPE, "cannot infer the type of this lambda parameter; annotate it");
                                    Ty::error()
                                }
                            }
                        }
                    };
                    let (name, binding) = match p.token(VARIABLE) {
                        Some(v) => (v.text()[1..].to_owned(), Binding::Php),
                        None => (
                            p.name_text().unwrap_or_default(),
                            if p.has_ident("mut") {
                                Binding::Mut
                            } else {
                                Binding::Val
                            },
                        ),
                    };
                    let kind = if binding == Binding::Php {
                        LocalKind::Var
                    } else {
                        LocalKind::Param(binding)
                    };
                    self.declare(&name, ty.clone(), kind, p.text_range());
                    params.push(stir_types::FnParam::required(ty));
                }
            }
            None => {
                if let Some(f) = &exp_fn
                    && f.params.len() == 1
                {
                    let t = f.params[0].ty.clone();
                    self.frames.last_mut().unwrap().insert(
                        SmolStr::new("it"),
                        Local {
                            ty: t.clone(),
                            declared: t.clone(),
                            kind: LocalKind::Param(Binding::Val),
                            range: node.text_range(),
                        },
                    );
                    params.push(stir_types::FnParam::required(t));
                } else if let Some(f) = &exp_fn {
                    params.extend(f.params.iter().cloned());
                }
            }
        }
        let exp_ret = exp_fn.as_ref().map(|f| f.ret.clone());
        // `return` inside a lambda returns from the lambda
        self.fns.push(FnCx {
            ret: exp_ret.clone().unwrap_or_else(Ty::unit),
            infer_ret: true,
            returns: Vec::new(),
        });
        let stmts: Vec<SyntaxNode> = node.children().filter(|c| is_stmt_kind(c.kind())).collect();
        let mut last = Ty::unit();
        let n = stmts.len();
        for (i, s) in stmts.iter().enumerate() {
            if i + 1 == n && matches!(s.kind(), EXPR_STMT | IF_STMT) {
                let unit_expected = exp_ret
                    .as_ref()
                    .is_some_and(|r| matches!(r.kind(), TyKind::Unit));
                let exp = if unit_expected {
                    None
                } else {
                    exp_ret.as_ref()
                };
                let t = self.stmt_value(s, exp);
                last = if unit_expected { Ty::unit() } else { t };
            } else {
                self.stmt(s);
            }
        }
        let fcx = self.fns.pop().unwrap();
        let ret = fcx
            .returns
            .into_iter()
            .fold(last, |a, b| stir_types::join(self.ctx, &a, &b));
        self.pop_frame();
        Ty::func(params, ret)
    }

    // ------------------------------------------------------------ match / when / if

    fn branch_value(&mut self, node: &SyntaxNode, expected: Option<&Ty>) -> Ty {
        match node.kind() {
            BLOCK => {
                self.push_frame();
                let stmts: Vec<SyntaxNode> =
                    node.children().filter(|c| is_stmt_kind(c.kind())).collect();
                let mut last = Ty::unit();
                let n = stmts.len();
                for (i, s) in stmts.iter().enumerate() {
                    if i + 1 == n && matches!(s.kind(), EXPR_STMT | IF_STMT) {
                        last = self.stmt_value(s, expected);
                    } else {
                        self.stmt(s);
                        if self.diverges(s) {
                            last = Ty::nothing();
                        }
                    }
                }
                self.pop_frame();
                last
            }
            k if is_stmt_kind(k) => {
                self.stmt(node);
                if self.diverges(node) {
                    Ty::nothing()
                } else {
                    Ty::unit()
                }
            }
            _ => self.expr(node, expected),
        }
    }

    /// The value of a statement used in value position (last statement of a
    /// lambda or branch block): an expression, or an `if`/`else` chain.
    fn stmt_value(&mut self, s: &SyntaxNode, expected: Option<&Ty>) -> Ty {
        match s.kind() {
            EXPR_STMT => match exprs_of(s).first() {
                Some(e) => self.expr(e, expected),
                None => Ty::unit(),
            },
            IF_STMT if s.child(ELSE_CLAUSE).is_some() => {
                let cond = exprs_of(s).into_iter().next();
                let (t, f) = cond.as_ref().map(|c| self.condition(c)).unwrap_or_default();
                let snap = self.snapshot();
                self.apply_facts(&t);
                let mut values = Vec::new();
                if let Some(body) = s.children().find(|c| is_stmt_kind(c.kind())) {
                    values.push(self.branch_stmt_value(&body, expected));
                }
                self.restore_types(&snap);
                self.apply_facts(&f);
                for clause in s.children() {
                    match clause.kind() {
                        ELSEIF_CLAUSE => {
                            let c = exprs_of(&clause).into_iter().next();
                            let (t2, f2) =
                                c.as_ref().map(|c| self.condition(c)).unwrap_or_default();
                            let snap2 = self.snapshot();
                            self.apply_facts(&t2);
                            if let Some(body) = clause.children().find(|c| is_stmt_kind(c.kind())) {
                                values.push(self.branch_stmt_value(&body, expected));
                            }
                            self.restore_types(&snap2);
                            self.apply_facts(&f2);
                        }
                        ELSE_CLAUSE => {
                            if let Some(body) = clause.children().find(|c| is_stmt_kind(c.kind())) {
                                values.push(self.branch_stmt_value(&body, expected));
                            }
                        }
                        _ => {}
                    }
                }
                self.restore_types(&snap);
                values
                    .into_iter()
                    .reduce(|a, b| stir_types::join(self.ctx, &a, &b))
                    .unwrap_or_else(Ty::unit)
            }
            _ => {
                self.stmt(s);
                Ty::unit()
            }
        }
    }

    fn branch_stmt_value(&mut self, body: &SyntaxNode, expected: Option<&Ty>) -> Ty {
        match body.kind() {
            BLOCK => self.branch_value(body, expected),
            _ => {
                let t = self.stmt_value(body, expected);
                if self.diverges(body) {
                    Ty::nothing()
                } else {
                    t
                }
            }
        }
    }

    fn match_expr(&mut self, node: &SyntaxNode, expected: Option<&Ty>) -> Ty {
        let subject = exprs_of(node).into_iter().next();
        let st = subject
            .as_ref()
            .map(|s| self.expr(s, None))
            .unwrap_or_else(Ty::error);
        let mut result: Option<Ty> = None;
        for arm in node.children_of(MATCH_ARM) {
            if let Some(conds) = arm.child(MATCH_CONDITIONS) {
                for c in exprs_of(&conds) {
                    let t = self.expr(&c, None);
                    if !st.is_error()
                        && !text_of(&subject.clone().unwrap()).eq_ignore_ascii_case("true")
                    {
                        self.check_comparable(c.text_range(), &st, &t);
                    }
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

    fn when_expr(&mut self, node: &SyntaxNode, expected: Option<&Ty>) -> Ty {
        let subject = exprs_of(node).into_iter().next();
        let bound_name = node.child(SyntaxKind::NAME).map(|n| text_of(&n));
        let st = subject.as_ref().map(|s| self.expr(s, None));
        let subject_local = subject
            .as_ref()
            .and_then(|s| self.local_of(s))
            .map(|(n, _)| n);
        self.push_frame();
        if let (Some(n), Some(t)) = (&bound_name, &st) {
            self.declare(n, t.clone(), LocalKind::Val, node.text_range());
        }
        let narrow_name = bound_name.map(SmolStr::new).or(subject_local);
        let mut remaining = st.clone();
        let mut result: Option<Ty> = None;
        let mut has_else = false;
        let snap = self.snapshot();
        for arm in node.children_of(WHEN_ARM) {
            self.restore_types(&snap);
            let conds = arm.children_of(WHEN_CONDITION);
            let is_else = arm.has_token(ELSE_KW);
            has_else |= is_else;
            let mut arm_ty: Option<Ty> = None;
            for c in &conds {
                if let Some(tn) = stir_hir::first_type(c) {
                    // `is T` / `!is T`
                    let test = self.pattern_type(&tn, remaining.as_ref());
                    let neg = c.has_token(BANG);
                    if let Some(cur) = &remaining {
                        let (yes, no) = (
                            stir_types::narrow_to(self.ctx, cur, &test),
                            stir_types::narrow_away(self.ctx, cur, &test),
                        );
                        let (yes, no) = if neg { (no, yes) } else { (yes, no) };
                        arm_ty = Some(match arm_ty {
                            None => yes,
                            Some(a) => Ty::union([a, yes]),
                        });
                        remaining = Some(no);
                    }
                } else if let Some(e) = exprs_of(c).first() {
                    let t = self.expr(e, None);
                    match &st {
                        Some(s) => {
                            self.check_comparable(e.text_range(), s, &t);
                            if let Some(cur) = &remaining
                                && matches!(t.kind(), TyKind::Literal(_) | TyKind::Null)
                            {
                                let yes = stir_types::narrow_to(self.ctx, cur, &t);
                                arm_ty = Some(match arm_ty {
                                    None => yes,
                                    Some(a) => Ty::union([a, yes]),
                                });
                                remaining = Some(stir_types::narrow_away(self.ctx, cur, &t));
                            }
                        }
                        None => {
                            if self.stir() && !t.is_error() && !self.assignable(&t, &Ty::bool()) {
                                self.error(
                                    e.text_range(),
                                    codes::TYPE_MISMATCH,
                                    format!("`when` conditions must be `Bool`, found `{t}`"),
                                );
                            }
                            let (facts, _) = self.facts(e);
                            self.apply_facts(&facts);
                        }
                    }
                }
            }
            if let (Some(n), Some(t)) = (&narrow_name, &arm_ty)
                && let Some(l) = self.lookup_mut(n)
            {
                l.ty = t.clone();
            }
            if is_else
                && let (Some(n), Some(t)) = (&narrow_name, &remaining)
                && let Some(l) = self.lookup_mut(n)
            {
                l.ty = t.clone();
            }
            let body = arm.children().filter(|c| c.kind() != WHEN_CONDITION).last();
            if let Some(b) = body {
                let t = self.branch_value(&b, expected);
                result = Some(match result {
                    None => t,
                    Some(r) => stir_types::join(self.ctx, &r, &t),
                });
            }
        }
        self.restore_types(&snap);
        self.pop_frame();
        let exhaustive = has_else || remaining.as_ref().is_some_and(|r| r.is_bottom());
        let used_as_value = !matches!(node.parent().map(|p| p.kind()), Some(EXPR_STMT));
        if used_as_value && !exhaustive && st.is_some() {
            let rem = remaining
                .map(|r| format!(" (`{r}` is not covered)"))
                .unwrap_or_default();
            self.error(
                node.text_range(),
                codes::TYPE_MISMATCH,
                format!("`when` used as a value must be exhaustive; add an `else` branch{rem}"),
            );
        } else if used_as_value && st.is_none() && !has_else {
            self.error(
                node.text_range(),
                codes::TYPE_MISMATCH,
                "`when` used as a value needs an `else` branch",
            );
        }
        result.unwrap_or_else(Ty::unit)
    }

    fn if_expr(&mut self, node: &SyntaxNode, expected: Option<&Ty>) -> Ty {
        let parts: Vec<SyntaxNode> = node.children().collect();
        let Some(cond) = parts.first() else {
            return Ty::error();
        };
        let (t, f) = self.condition(cond);
        let snap = self.snapshot();
        self.apply_facts(&t);
        let a = parts
            .get(1)
            .map(|b| self.branch_value(b, expected))
            .unwrap_or_else(Ty::unit);
        self.restore_types(&snap);
        self.apply_facts(&f);
        let b = parts.get(2).map(|b| self.branch_value(b, expected));
        self.restore_types(&snap);
        match b {
            Some(b) => stir_types::join(self.ctx, &a, &b),
            None => {
                let used = !matches!(node.parent().map(|p| p.kind()), Some(EXPR_STMT));
                if used {
                    self.error(
                        node.text_range(),
                        codes::TYPE_MISMATCH,
                        "`if` used as a value needs an `else` branch",
                    );
                }
                Ty::unit()
            }
        }
    }
}

fn block_diverges(c: &Checker<'_, '_>, block: &SyntaxNode) -> bool {
    block
        .children()
        .filter(|s| is_stmt_kind(s.kind()))
        .any(|s| c.diverges(&s))
        || block
            .children()
            .filter(|s| is_stmt_kind(s.kind()))
            .last()
            .is_some_and(|s| {
                matches!(s.kind(), WHILE_STMT | FOR_STMT)
                    && s.children()
                        .find(|x| is_expr_kind(x.kind()))
                        .is_some_and(|e| text_of(&e) == "true")
                    || (s.kind() == EXPR_STMT
                        && s.children().next().is_some_and(|e| e.kind() == WHEN_EXPR))
                    || s.kind() == TRY_STMT
                    || s.kind() == SWITCH_STMT
            })
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
fn narrow_assigned(declared: &Ty, value: &Ty) -> Ty {
    if value.is_error() || declared.is_error() {
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

trait NullableLike {
    fn is_nullable_like(&self, env: &dyn stir_types::TypeEnv) -> bool;
}

impl NullableLike for Ty {
    fn is_nullable_like(&self, env: &dyn stir_types::TypeEnv) -> bool {
        !self.is_error() && stir_types::is_nullable(env, self)
    }
}

/// Extension receivers of these types are passed by reference.
pub fn is_by_ref_receiver(ty: &Ty) -> bool {
    matches!(
        stir_types::without_null(ty).kind(),
        TyKind::MutableList(_) | TyKind::MutableMap(_, _)
    )
}
