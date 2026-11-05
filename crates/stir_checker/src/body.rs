//! Statement and expression checking.

use std::rc::Rc;
use std::sync::Arc;

use rustc_hash::FxHashMap;
use smol_str::SmolStr;
use stir_db::{Diagnostic, FileOrigin, SourceFile};
use stir_hir::ast::NodeExt;
use stir_hir::name::Scope;
use stir_hir::type_ref::{parse_int, unquote};
use stir_hir::{ClassKind, FileItems, Visibility};
use stir_syntax::{SyntaxKind, SyntaxKind::*, SyntaxNode, SyntaxToken, TextRange};
use stir_types::{LitValue, Ty, TyKind};

use crate::codes;
use crate::infer::{Inference, widen_literals};
use crate::model::{ClassModel, Ctx, FnSig, LowerCx, LowerError};
use crate::typed::{Def, MemberKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalKind {
    /// A function parameter.
    Param,
    /// `$this`
    This,
    /// A variable, or a property path that narrowing tracks.
    Variable,
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
    pub range: TextRange,
    /// Assigned on some of the paths that lead here, not all.
    pub maybe_undefined: bool,
}

/// What's known at a point in the code: the variables, by scope.
type FlowState = Vec<FxHashMap<SmolStr, Local>>;

/// The states a loop or `switch` is left or continued from.
#[derive(Debug, Default)]
struct LoopExits {
    breaks: Vec<FlowState>,
    continues: Vec<FlowState>,
}

/// What a closure runs as, when that's not where it's written.
struct ClosureBind<'db> {
    this: Option<Ty>,
    class: Option<Rc<ClassModel<'db>>>,
}

/// What a call that came with `@phpstan-assert`s went to.
#[derive(Clone)]
struct CallAsserts {
    asserts: Vec<stir_hir::AssertDef>,
    /// The callee's parameter names.
    params: Vec<SmolStr>,
    /// What the assert types are lowered against.
    cx: LowerCx,
    /// How the call solved the templates.
    solved: FxHashMap<stir_types::ParamId, Ty>,
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
    /// The current point can't be reached: a `return`, `throw` or the like came before it.
    dead: bool,
    /// Enclosing loops and `switch`es, for `break` and `continue`.
    loops: Vec<LoopExits>,
    /// In a first pass over a loop body, which only finds what the loop's variables hold.
    /// Its diagnostics are thrown away.
    prepass: u32,
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
    /// Calls to something with `@phpstan-assert`s, by the range of the call.
    call_targets: FxHashMap<TextRange, CallAsserts>,
    /// What the next closure checked is bound to.
    closure_bind: Option<ClosureBind<'db>>,
    /// `@var` tags of a `foreach` about to bind its variables.
    pending_var_tags: Vec<(Option<SmolStr>, Ty)>,
    /// How the last checked call solved its templates.
    last_solved: FxHashMap<stir_types::ParamId, Ty>,
    /// Expression types.
    pub types: FxHashMap<TextRange, Ty>,
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
            | SHORT_TERNARY_EXPR
            | CAST_EXPR
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
            dead: false,
            loops: Vec::new(),
            prepass: 0,
            probing: 0,
            dynamic_vars_fn: None,
            frames: vec![FxHashMap::default()],
            fns: Vec::new(),
            class: None,
            lower_cx: LowerCx::default(),
            in_ctor: false,
            call_targets: FxHashMap::default(),
            closure_bind: None,
            pending_var_tags: Vec::new(),
            last_solved: FxHashMap::default(),
            types: FxHashMap::default(),
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

    /// Reports what lowering a type found. A class the stubs have on other versions than these
    /// isn't unknown, and is fine as a type: PHP doesn't load it for that, and code can name
    /// what only some versions have in a union.
    fn lowered_error(&mut self, e: LowerError) {
        if let Some(symbol) = &e.symbol {
            let key = symbol.trim_start_matches('\\').to_ascii_lowercase();
            if self.index().limited_classes.contains_key(key.as_str()) {
                return;
            }
            self.diags.push(
                Diagnostic::error(self.path.clone(), e.range, e.code, e.message)
                    .with_symbol(symbol.as_str()),
            );
            return;
        }
        self.error(e.range, e.code, e.message);
    }

    /// Reports what lowering a class's `extends` and `implements` found: declaring the class
    /// does need these, on every version it runs on.
    fn supertype_error(&mut self, e: LowerError) {
        if let Some(symbol) = &e.symbol {
            let key = symbol.trim_start_matches('\\').to_ascii_lowercase();
            if let Some(&available) = self.index().limited_classes.get(key.as_str()) {
                return self.report_limited(e.range, &format!("`{symbol}`"), available);
            }
        }
        self.lowered_error(e);
    }

    fn index(&self) -> &'db stir_hir::DefIndex {
        stir_hir::def_index(self.ctx.db, self.ctx.ws)
    }

    /// The versions the code at `range` can run on. `None`: none of those of the project.
    fn versions_at(&self, range: TextRange) -> Option<stir_db::PhpVersions> {
        let db = self.ctx.db;
        let base = crate::php_version::file_versions(
            self.file.text(db),
            self.ctx.ws.settings(db).php_version,
        );
        let root = stir_db::parse(db, self.file).syntax();
        if !root.text_range().contains_range(range) {
            return Some(base);
        }
        let node = match root.covering_element(range) {
            stir_syntax::SyntaxElement::Node(n) => n,
            stir_syntax::SyntaxElement::Token(t) => t.parent()?,
        };
        crate::php_version::versions_at(&node, base)
    }

    /// Reports `what` of the stubs, if it isn't there on every version the code at `range` can
    /// run on.
    fn report_limited(&mut self, range: TextRange, what: &str, available: stir_db::Availability) {
        let Some(versions) = self.versions_at(range) else {
            return;
        };
        if !available.covers(versions) {
            let message = crate::php_version::library_message(what, available, versions);
            // what isn't installed yet may be a polyfill of it
            self.diags.push(
                Diagnostic::error(
                    self.path.clone(),
                    range,
                    codes::PHP_LIBRARY_VERSION,
                    message,
                )
                .with_symbol(what.trim_matches('`')),
            );
        }
    }

    /// With a real range of versions: reports a declaration of the stubs that's on the lowest
    /// of them, but not on all.
    fn report_going_away(
        &mut self,
        range: TextRange,
        what: &str,
        file: SourceFile,
        limited: &FxHashMap<SmolStr, stir_db::Availability>,
        key: &str,
    ) {
        if !self.ctx.ws.settings(self.ctx.db).php_version.is_single()
            && file.origin(self.ctx.db) == FileOrigin::Stub
            && let Some(&available) = limited.get(key)
        {
            self.report_limited(range, what, available);
        }
    }

    /// A member that `class` or an ancestor has in the stubs, on other versions than the
    /// target. Reported if the code here can run on those; true either way: it isn't unknown.
    fn limited_member(
        &mut self,
        range: TextRange,
        class: &Ty,
        kind: stir_hir::LimitedMemberKind,
        name: &str,
    ) -> bool {
        let Some(available) = self.ctx.limited_member(class, kind, name) else {
            return false;
        };
        let owner = match class.kind() {
            TyKind::Class { name, .. } => name.to_string(),
            _ => class.to_string(),
        };
        self.report_limited(range, &format!("`{owner}::{name}`"), available);
        true
    }

    /// On the last diagnostic, about a property that doesn't exist: assigning one creates it,
    /// which PHP has deprecated since 8.2.
    fn note_dynamic_property(&mut self, range: TextRange) {
        let root = stir_db::parse(self.ctx.db, self.file).syntax();
        if !root.text_range().contains_range(range) {
            return;
        }
        let node = match root.covering_element(range) {
            stir_syntax::SyntaxElement::Node(n) => Some(n),
            stir_syntax::SyntaxElement::Token(t) => t.parent(),
        };
        let fetch = node
            .into_iter()
            .flat_map(|n| n.ancestors())
            .find(|n| n.kind() == PROPERTY_FETCH_EXPR);
        let is_target = fetch.as_ref().is_some_and(|f| {
            f.parent()
                .is_some_and(|p| p.kind() == ASSIGN_EXPR && p.children().next().as_ref() == Some(f))
        });
        if !is_target {
            return;
        }
        let versions = self.ctx.ws.settings(self.ctx.db).php_version;
        let note = if versions.max >= stir_db::PhpVersion::new(8, 2) {
            "this creates a dynamic property, which PHP 8.2 deprecated"
        } else {
            "this creates a dynamic property, which PHP 8.2 will deprecate"
        };
        if let Some(d) = self.diags.last_mut() {
            d.notes.push(note.to_owned());
        }
    }

    /// An override's types are mostly PHPDoc, which PHP doesn't enforce: PHPStan's
    /// `checkPhpDocMethodSignatures`, from level 3.
    fn tag_php_override(&mut self) {
        self.tag_level(3);
    }

    /// The last diagnostic is reported from level `level`.
    fn tag_level(&mut self, level: u8) {
        if let Some(d) = self.diags.last_mut() {
            d.php_level = Some(level);
        }
    }

    /// The last diagnostic is about only some members of a union: a level 7 problem.
    fn tag_partial_union(&mut self) {
        if let Some(d) = self.diags.last_mut() {
            d.php_level = Some(7);
        }
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
            self.lowered_error(e);
        }
        self.check_type_bounds(node.text_range(), &ty);
        ty
    }

    fn assignable(&self, from: &Ty, to: &Ty) -> bool {
        stir_types::is_assignable(self.ctx, from, to)
    }

    /// Reports a mismatch if `from` is not assignable to `to`, at the level PHPStan reports
    /// that kind of mismatch.
    fn check_assign(&mut self, range: TextRange, from: &Ty, to: &Ty, what: &str) {
        if from.is_error() || to.is_error() {
            return;
        }
        // A template in scope is a type like any other. One that isn't, which the call couldn't
        // solve, says nothing about the value. Inside another type they aren't compared yet:
        // that needs PHPStan's rules for variance against templates.
        let checkable = |t: &Ty| {
            t.members().iter().all(|m| match m.kind() {
                TyKind::Param { id, .. } => self
                    .lower_cx
                    .generics
                    .iter()
                    .any(|(_, g)| matches!(g.kind(), TyKind::Param { id: g, .. } if g == id)),
                _ => stir_types::free_params(m).is_empty(),
            })
        };
        if !checkable(from) || !checkable(to) {
            return;
        }
        // `$this` in a trait nobody here uses is whichever class would use it
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
            if from.is_untyped() {
                msg.push_str(
                    "; narrow the untyped value first (an `is_*()` check or `instanceof`)",
                );
            }
            let level =
                self.mismatch_level(from, to, if what.starts_with("argument") { 5 } else { 3 });
            self.error(range, codes::TYPE_MISMATCH, msg);
            if let Some(d) = self.diags.last_mut() {
                d.php_level = Some(level);
            }
        }
    }

    /// The level PHPStan reports `from` not fitting `to` at, by what it's still accepted as
    /// below that level: `RuleLevelHelper::accepts`.
    fn mismatch_level(&self, from: &Ty, to: &Ty, base: u8) -> u8 {
        let no_nulls = stir_types::strip_nulls(from);
        // a plain `array` has no type for its elements: that's an implicit `mixed`
        let mixed_level = |t: &Ty| {
            let t = t.transform(&mut |x| {
                matches!(x.kind(), TyKind::Array { key, value, .. }
                    if matches!(key.kind(), TyKind::ArrayKey)
                        && matches!(value.kind(), TyKind::Mixed))
                .then(Ty::untyped)
            });
            let mut explicit = false;
            t.walk(&mut |x| {
                explicit |= matches!(x.kind(), TyKind::Mixed);
                true
            });
            if explicit { 9 } else { 10 }
        };
        if stir_types::assignable_ignoring_mixed(self.ctx, from, to) {
            mixed_level(from)
        } else if no_nulls != *from
            && stir_types::assignable_ignoring_mixed(self.ctx, &no_nulls, to)
        {
            8
        } else if stir_types::maybe_assignable(self.ctx, &no_nulls, to) {
            7
        } else {
            base
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
                maybe_undefined: false,
            },
        );
    }

    fn apply_facts(&mut self, facts: &Facts) {
        for (name, ty) in &facts.narrow {
            if let Some(l) = self.lookup_mut(name) {
                // a property found to be null may be filled in by the next call, and treating
                // it as always null would make comparing it with anything look pointless
                l.ty = if (name.contains("->") || name.contains("::")) && ty.is_null() {
                    l.declared.clone()
                } else {
                    ty.clone()
                };
                // it was found to hold something
                if !ty.is_null() {
                    l.maybe_undefined = false;
                }
            } else if let Some(frame) = self.frames.first_mut()
                && name.starts_with('?')
            {
                frame.insert(
                    name.clone(),
                    Local {
                        ty: ty.clone(),
                        declared: ty.clone(),
                        kind: LocalKind::Variable,
                        range: TextRange::default(),
                        maybe_undefined: false,
                    },
                );
            }
        }
    }

    fn snapshot(&self) -> FlowState {
        self.frames.clone()
    }

    fn restore(&mut self, state: &FlowState) {
        self.frames.clone_from(state);
    }

    /// Undoes what a sub-expression learned, keeping what's been defined or tracked since.
    fn restore_narrowing(&mut self, snap: &FlowState) {
        for (frame, old) in self.frames.iter_mut().zip(snap) {
            for (name, local) in frame.iter_mut() {
                if let Some(o) = old.get(name) {
                    local.ty = o.ty.clone();
                }
            }
        }
    }

    /// The state after paths with these end states come together.
    fn merge_states(&self, states: Vec<FlowState>) -> Option<FlowState> {
        let mut iter = states.into_iter();
        let mut merged = iter.next()?;
        for other in iter {
            for (frame, o) in merged.iter_mut().zip(&other) {
                for (name, local) in frame.iter_mut() {
                    match o.get(name) {
                        Some(ol) => {
                            if local.ty != ol.ty {
                                local.ty = stir_types::join(self.ctx, &local.ty, &ol.ty);
                            }
                            if local.declared != ol.declared {
                                local.declared =
                                    stir_types::join(self.ctx, &local.declared, &ol.declared);
                            }
                            local.maybe_undefined |= ol.maybe_undefined;
                        }
                        // a tracked property on one path isn't known on the other
                        None if name.contains("->")
                            || name.contains('[')
                            || name.contains("::") =>
                        {
                            local.ty = local.declared.clone();
                        }
                        None => local.maybe_undefined = true,
                    }
                }
                // a class or member known to exist on one path isn't on the other
                frame.retain(|name, _| !name.starts_with('?') || o.contains_key(name));
                for (name, ol) in o {
                    if !frame.contains_key(name) && !name.starts_with('?') {
                        let mut l = ol.clone();
                        if name.contains("->") || name.contains('[') || name.contains("::") {
                            l.ty = l.declared.clone();
                        } else {
                            l.maybe_undefined = true;
                        }
                        frame.insert(name.clone(), l);
                    }
                }
            }
        }
        Some(merged)
    }

    /// Continues from the merge of these end states; none means this point can't be reached.
    fn continue_from(&mut self, states: Vec<FlowState>) {
        match self.merge_states(states) {
            Some(state) => {
                self.frames = state;
                self.dead = false;
            }
            None => self.dead = true,
        }
    }

    // ------------------------------------------------------------ files

    pub fn check_root(&mut self, root: &SyntaxNode) {
        self.check_declarations();
        self.fns.push(FnCx {
            ret: Ty::void(),
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
                self.lowered_error(e);
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

    /// The `@var` tags of a statement's doc comment.
    fn var_tags(&mut self, node: &SyntaxNode) -> Vec<(Option<SmolStr>, Ty)> {
        let Some(token) = node.doc_comment() else {
            return Vec::new();
        };
        if !token.text().contains("@var") && !token.text().contains("-var") {
            return Vec::new();
        }
        let doc = stir_hir::doc::parse(token.text(), token.text_range().start());
        let names: Vec<SmolStr> = self
            .lower_cx
            .generics
            .iter()
            .map(|(n, _)| n.clone())
            .collect();
        let scope = self.scope(node);
        let aliases = self
            .class
            .as_ref()
            .map(|c| c.def.aliases.clone())
            .unwrap_or_default();
        let cx = stir_hir::doc::DocCx {
            scope: &scope,
            type_params: &names,
            aliases: &aliases,
        };
        let vars: Vec<(Option<SmolStr>, stir_hir::type_ref::TypeRef)> = doc
            .vars()
            .iter()
            .map(|v| (v.name.as_ref().map(|n| n.name.clone()), cx.lower(&v.ty)))
            .collect();
        let mut out = Vec::new();
        for (name, tr) in vars {
            let mut sink = Vec::new();
            let ty = self.ctx.lower(&tr, &self.lower_cx, &mut sink);
            for e in sink {
                self.lowered_error(e);
                self.tag_level(2);
            }
            out.push((name, ty));
        }
        out
    }

    /// `@var` before a statement: what a variable holds from here on, whatever it looks like
    /// to the checker. PHPStan takes this on trust, too.
    fn apply_var_tags(&mut self, tags: &[(Option<SmolStr>, Ty)], assigned: Option<&str>) {
        for (name, ty) in tags {
            let target = name.as_deref().or(assigned);
            let Some(target) = target else {
                continue;
            };
            if ty.is_error() {
                continue;
            }
            if let Some(l) = self.lookup_mut(target) {
                l.ty = ty.clone();
                l.declared = ty.clone();
                l.maybe_undefined = false;
            } else {
                self.declare(
                    target,
                    ty.clone(),
                    LocalKind::Variable,
                    TextRange::default(),
                );
            }
        }
    }

    pub fn stmt(&mut self, node: &SyntaxNode) {
        let tags = self.var_tags(node);
        if tags.is_empty() {
            return self.stmt_inner(node);
        }
        if node.kind() == FOREACH_STMT {
            // about what the loop binds: applied when that's bound
            self.pending_var_tags = tags;
            return self.stmt_inner(node);
        }
        let assigned = (node.kind() == EXPR_STMT)
            .then(|| exprs_of(node).into_iter().next())
            .flatten()
            .filter(|e| e.kind() == ASSIGN_EXPR)
            .and_then(|e| e.children().next())
            .filter(|l| l.kind() == VARIABLE_EXPR)
            .map(|l| text_of(&l).trim_start_matches('$').to_owned());
        let (after, before): (Vec<_>, Vec<_>) = tags.into_iter().partition(|(name, _)| {
            assigned.is_some() && (name.is_none() || name.as_deref() == assigned.as_deref())
        });
        self.apply_var_tags(&before, None);
        self.stmt_inner(node);
        self.apply_var_tags(&after, assigned.as_deref());
    }

    fn stmt_inner(&mut self, node: &SyntaxNode) {
        match node.kind() {
            EXPR_STMT => {
                if let Some(e) = exprs_of(node).first() {
                    let t = self.expr(e, None);
                    if t.is_never() || matches!(e.kind(), THROW_EXPR | EXIT_EXPR) {
                        self.dead = true;
                    }
                }
            }
            ECHO_STMT | ECHO_TAG_STMT => {
                for e in exprs_of(node) {
                    self.expr(&e, None);
                }
            }
            RETURN_STMT => {
                self.return_stmt(node);
                self.dead = true;
            }
            BLOCK => self.block_stmts(node),
            IF_STMT => self.if_stmt(node),
            WHILE_STMT | DO_WHILE_STMT | FOR_STMT | FOREACH_STMT => self.loop_stmt(node),
            SWITCH_STMT => self.switch_stmt(node),
            BREAK_STMT => self.jump(node, true),
            CONTINUE_STMT => self.jump(node, false),
            GOTO_STMT => self.dead = true,
            LABEL_STMT | EMPTY_STMT | INLINE_HTML_STMT | USE_DECL | HALT_COMPILER_STMT => {}
            TRY_STMT => self.try_stmt(node),
            GLOBAL_STMT => {
                for v in node.descendants().filter(|d| d.kind() == VARIABLE_EXPR) {
                    let name = text_of(&v);
                    self.declare(
                        &name[1..],
                        Ty::untyped(),
                        LocalKind::Variable,
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
                        self.declare(&v.text()[1..], ty, LocalKind::Variable, v.text_range());
                    }
                }
            }
            UNSET_STMT => {
                self.probing += 1;
                let targets = exprs_of(node);
                for e in &targets {
                    self.expr(e, None);
                }
                self.probing -= 1;
                for e in &targets {
                    let Some(key) = self.place_key(e) else {
                        continue;
                    };
                    self.forget_properties_of(&key);
                    if e.kind() == VARIABLE_EXPR {
                        for frame in &mut self.frames {
                            frame.remove(key.as_str());
                        }
                    } else if e.kind() == INDEX_EXPR
                        && let Some(base) = exprs_of(e).first().and_then(|b| self.place_key(b))
                    {
                        self.forget_properties_of(&base);
                    }
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

    fn run_stmts(&mut self, parent: &SyntaxNode) {
        for s in parent.children().filter(|c| is_stmt_kind(c.kind())) {
            self.stmt(&s);
        }
    }

    fn if_stmt(&mut self, node: &SyntaxNode) {
        let cond = exprs_of(node).into_iter().next();
        let (t, f) = cond.as_ref().map(|c| self.condition(c)).unwrap_or_default();
        let entry_dead = self.dead;
        let after_cond = self.snapshot();
        let mut ends = Vec::new();
        self.apply_facts(&t);
        self.run_stmts(node);
        if !self.dead {
            ends.push(self.snapshot());
        }
        self.restore(&after_cond);
        self.dead = entry_dead;
        self.apply_facts(&f);
        let mut has_else = false;
        for clause in node.children() {
            match clause.kind() {
                ELSEIF_CLAUSE => {
                    let c = exprs_of(&clause).into_iter().next();
                    let (t2, f2) = c.as_ref().map(|c| self.condition(c)).unwrap_or_default();
                    let before = self.snapshot();
                    self.apply_facts(&t2);
                    self.run_stmts(&clause);
                    if !self.dead {
                        ends.push(self.snapshot());
                    }
                    self.restore(&before);
                    self.dead = entry_dead;
                    self.apply_facts(&f2);
                }
                ELSE_CLAUSE => {
                    has_else = true;
                    self.run_stmts(&clause);
                    if !self.dead {
                        ends.push(self.snapshot());
                    }
                }
                _ => {}
            }
        }
        if !has_else {
            ends.push(self.snapshot());
        }
        if entry_dead {
            // dead code: keep going with something, as each statement still gets checked
            let last = self.snapshot();
            self.continue_from(ends);
            self.dead = true;
            if self.frames.is_empty() {
                self.restore(&last);
            }
        } else if ends.is_empty() {
            self.dead = true;
        } else {
            self.continue_from(ends);
        }
    }

    /// `break` or `continue`: the rest of this path goes to the loop's end or next round.
    fn jump(&mut self, node: &SyntaxNode, is_break: bool) {
        let levels = exprs_of(node)
            .first()
            .and_then(|e| parse_int(text_of(e).trim()))
            .unwrap_or(1)
            .max(1) as usize;
        if !self.dead {
            let state = self.snapshot();
            if let Some(i) = self.loops.len().checked_sub(levels)
                && let Some(l) = self.loops.get_mut(i)
            {
                if is_break {
                    l.breaks.push(state);
                } else {
                    l.continues.push(state);
                }
            }
        }
        self.dead = true;
    }

    /// One round of a loop, up to where the condition is checked again.
    fn loop_round(
        &mut self,
        node: &SyntaxNode,
        bindings: &dyn Fn(&mut Self),
    ) -> (Vec<FlowState>, Vec<FlowState>) {
        self.loops.push(LoopExits::default());
        match node.kind() {
            WHILE_STMT => {
                let cond = exprs_of(node).into_iter().next();
                let facts = cond.as_ref().map(|c| self.condition(c)).unwrap_or_default();
                self.apply_facts(&facts.0);
                self.run_stmts(node);
            }
            DO_WHILE_STMT => self.run_stmts(node),
            FOR_STMT => {
                if let Some(p) = node.child(FOR_COND) {
                    for e in exprs_of(&p) {
                        let (t, _) = self.condition(&e);
                        self.apply_facts(&t);
                    }
                }
                self.run_stmts(node);
            }
            _ => {
                bindings(self);
                self.run_stmts(node);
            }
        }
        let exits = self.loops.pop().unwrap_or_default();
        let mut ends = exits.continues;
        if !self.dead {
            ends.push(self.snapshot());
        }
        (ends, exits.breaks)
    }

    /// What a loop's variables hold comes from the rounds before: outside a pre-pass, one
    /// pass without diagnostics finds that out, and the second checks with it. A loop in a
    /// pre-pass gets just the one pass, which keeps this linear in the nesting depth.
    fn loop_stmt(&mut self, node: &SyntaxNode) {
        let mut subject = None;
        let mut subject_ty = None;
        if node.kind() == FOR_STMT
            && let Some(p) = node.child(FOR_INIT)
        {
            for e in exprs_of(&p) {
                self.expr(&e, None);
            }
        }
        if node.kind() == FOREACH_STMT {
            let es = exprs_of(node);
            if let Some(subj) = es.first() {
                let st = self.expr(subj, None);
                subject = Some(self.iter_types(subj.text_range(), &st));
                subject_ty = Some(st);
            }
        }
        let es = exprs_of(node);
        let tags = std::mem::take(&mut self.pending_var_tags);
        let bindings = |this: &mut Self| {
            let Some((key_ty, val_ty)) = subject.clone() else {
                return;
            };
            let targets = &es[1..];
            match targets {
                [k, v] => {
                    this.bind_target(k, key_ty);
                    this.bind_target(v, val_ty);
                }
                [v] => this.bind_target(v, val_ty),
                _ => {}
            }
            let value = targets.last().filter(|v| v.kind() == VARIABLE_EXPR);
            let value = value.map(|v| text_of(v).trim_start_matches('$').to_owned());
            this.apply_var_tags(&tags, value.as_deref());
        };
        let entry = self.snapshot();
        let entry_dead = self.dead;
        let may_change = node.descendants().any(|d| {
            matches!(
                d.kind(),
                ASSIGN_EXPR | POSTFIX_EXPR | PREFIX_EXPR | CALL_EXPR | FOREACH_STMT
            )
        });
        let mut round_start = entry.clone();
        if self.prepass == 0 && may_change {
            self.prepass += 1;
            let found = self.diags.len();
            let dynamic = self.dynamic_vars_fn;
            let (mut ends, breaks) = self.loop_round(node, &bindings);
            self.finish_round(node, &mut ends);
            self.diags.truncate(found);
            self.dynamic_vars_fn = dynamic;
            self.prepass -= 1;
            drop(breaks);
            ends.push(entry.clone());
            round_start = self.merge_states(ends).unwrap_or_else(|| entry.clone());
            // a variable is only there on later rounds: its first read isn't checked by this
            self.restore(&round_start);
            self.dead = entry_dead;
        } else if !may_change {
            self.restore(&round_start);
        }
        let (mut ends, breaks) = self.loop_round(node, &bindings);
        self.finish_round(node, &mut ends);
        let mut exits = breaks;
        match node.kind() {
            // the condition is false at the end of the last round, or at the first check
            WHILE_STMT => {
                let cond = exprs_of(node).into_iter().next();
                let always = cond.as_ref().is_some_and(is_true_literal);
                if !always {
                    ends.push(round_start);
                    let at_check = self.merge_states(ends);
                    if let (Some(state), Some(c)) = (at_check, cond.as_ref()) {
                        self.restore(&state);
                        let (_, f) = self.facts(c);
                        self.apply_facts(&f);
                        exits.push(self.snapshot());
                    }
                }
            }
            DO_WHILE_STMT => {
                if let Some(state) = self.merge_states(ends) {
                    self.restore(&state);
                    self.dead = false;
                    if let Some(c) = exprs_of(node).first() {
                        let (_, f) = self.condition(c);
                        if !is_true_literal(c) {
                            self.apply_facts(&f);
                            exits.push(self.snapshot());
                        }
                    }
                }
            }
            FOR_STMT => {
                let unconditional = node.child(FOR_COND).is_none_or(|c| exprs_of(&c).is_empty());
                if !unconditional {
                    ends.push(round_start);
                    exits.extend(self.merge_states(ends));
                }
            }
            // no rounds at all, or all of them
            _ => {
                let non_empty = subject_ty.as_ref().is_some_and(|t| {
                    t.members().iter().all(|m| match m.kind() {
                        TyKind::List { non_empty, .. } | TyKind::Array { non_empty, .. } => {
                            *non_empty
                        }
                        TyKind::Shape(s) => s.fields.iter().any(|f| !f.optional),
                        _ => false,
                    })
                });
                if !non_empty {
                    ends.push(entry);
                }
                exits.extend(self.merge_states(ends));
            }
        }
        self.continue_from(exits);
        self.dead |= entry_dead;
    }

    /// The step of a `for` runs after each round.
    fn finish_round(&mut self, node: &SyntaxNode, ends: &mut Vec<FlowState>) {
        if node.kind() != FOR_STMT {
            return;
        }
        let Some(state) = self.merge_states(std::mem::take(ends)) else {
            return;
        };
        self.restore(&state);
        self.dead = false;
        if let Some(p) = node.child(FOR_STEP) {
            for e in exprs_of(&p) {
                self.expr(&e, None);
            }
        }
        ends.push(self.snapshot());
    }

    fn switch_stmt(&mut self, node: &SyntaxNode) {
        let subject = exprs_of(node).into_iter().next();
        if let Some(e) = &subject {
            self.expr(e, None);
        }
        let by_true = subject.as_ref().is_some_and(is_true_literal);
        let entry_dead = self.dead;
        // what's left of the subject after the cases before
        let mut after_cases = self.snapshot();
        let mut has_default = false;
        self.loops.push(LoopExits::default());
        let mut falling: Option<FlowState> = None;
        for case in node.children_of(SWITCH_CASE) {
            self.restore(&after_cases);
            self.dead = entry_dead;
            let values = exprs_of(&case);
            let mut yes = Facts::default();
            if values.is_empty() {
                has_default = true;
            }
            for e in &values {
                self.expr(e, None);
                let (t, f) = if by_true {
                    self.facts(e)
                } else {
                    subject
                        .as_ref()
                        .map(|s| self.literal_facts(s, e, true))
                        .unwrap_or_default()
                };
                yes = t;
                self.apply_facts(&f);
                after_cases = self.snapshot();
            }
            self.restore(&after_cases);
            self.apply_facts(&yes);
            if let Some(state) = falling.take() {
                let current = self.snapshot();
                self.continue_from(vec![current, state]);
            }
            self.run_stmts(&case);
            if !self.dead {
                falling = Some(self.snapshot());
            }
        }
        let exits = self.loops.pop().unwrap_or_default();
        let mut ends = exits.breaks;
        // `continue` in a `switch` acts like `break`
        ends.extend(exits.continues);
        ends.extend(falling);
        if !has_default {
            ends.push(after_cases);
        }
        self.dead = entry_dead;
        if ends.is_empty() {
            self.dead = true;
        } else {
            self.continue_from(ends);
            self.dead |= entry_dead;
        }
    }

    fn try_stmt(&mut self, node: &SyntaxNode) {
        let entry = self.snapshot();
        let entry_dead = self.dead;
        let mut ends = Vec::new();
        if let Some(b) = node.child(BLOCK) {
            self.stmt(&b);
        }
        let try_ends = !self.dead;
        let at_try_end = self.snapshot();
        if try_ends {
            ends.push(at_try_end.clone());
        }
        // an exception can come at any point of the `try`, so any of what it assigned may
        // have happened, and what it narrowed may no longer hold
        let mut at_catch = self
            .merge_states(vec![entry.clone(), at_try_end])
            .unwrap_or_else(|| entry.clone());
        if let Some(b) = node.child(BLOCK) {
            for key in self.assigned_places(&b) {
                for frame in &mut at_catch {
                    if let Some(l) = frame.get_mut(key.as_str()) {
                        l.ty = stir_types::join(self.ctx, &l.ty, &l.declared);
                    }
                }
            }
        }
        for c in node.children_of(CATCH_CLAUSE) {
            self.restore(&at_catch);
            self.dead = entry_dead;
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
            if !self.dead {
                ends.push(self.snapshot());
            }
        }
        if let Some(f) = node.child(FINALLY_CLAUSE) {
            // runs on every way out, even ones that leave the code after it behind
            let on_any = ends.iter().chain(std::iter::once(&at_catch)).cloned();
            let start = self.merge_states(on_any.collect());
            if let Some(state) = start {
                self.restore(&state);
            }
            self.dead = entry_dead;
            if let Some(b) = f.child(BLOCK) {
                self.stmt(&b);
            }
            if self.dead {
                return;
            }
            let after = self.snapshot();
            if ends.is_empty() {
                self.dead = true;
                self.restore(&after);
            } else {
                // what `finally` did on top of each normal way through
                let assigned = self.assigned_places(&f);
                let merged = self.merge_states(ends);
                if let Some(mut state) = merged {
                    for key in assigned {
                        for (frame, a) in state.iter_mut().zip(&after) {
                            if let Some(l) = a.get(key.as_str()) {
                                frame.insert(SmolStr::new(&key), l.clone());
                            }
                        }
                    }
                    self.restore(&state);
                    self.dead = entry_dead;
                }
            }
            return;
        }
        if ends.is_empty() {
            self.dead = true;
        } else {
            self.continue_from(ends);
            self.dead |= entry_dead;
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

    /// Binds a foreach/destructuring target.
    fn bind_target(&mut self, target: &SyntaxNode, ty: Ty) {
        match target.kind() {
            VARIABLE_EXPR => {
                let name = text_of(target);
                let n = name.trim_start_matches('$');
                if let Some(l) = self.lookup_mut(n)
                    && l.kind.mutable()
                {
                    l.declared = ty.clone();
                    l.ty = ty;
                    l.maybe_undefined = false;
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
                let mut position = 0;
                for elem in target.children_of(ARRAY_ELEM) {
                    let parts = exprs_of(&elem);
                    let Some(last) = parts.last() else {
                        position += 1;
                        continue;
                    };
                    let key = if elem.has_token(FAT_ARROW) {
                        parts.first().map(|k| self.expr(k, None))
                    } else {
                        position += 1;
                        Some(Ty::lit_int(position - 1))
                    };
                    let elem_ty = self.destructured(&ty, key.as_ref());
                    self.bind_target(last, elem_ty);
                }
            }
            _ => {
                self.expr(target, None);
            }
        }
    }

    /// What destructuring gets at `key` of a value of type `ty`. If it isn't there it's null.
    fn destructured(&self, ty: &Ty, key: Option<&Ty>) -> Ty {
        let found: Vec<Ty> = ty
            .members()
            .iter()
            .filter(|m| !m.is_null())
            .map(|m| self.index_type(m, key).unwrap_or_else(Ty::untyped))
            .collect();
        Ty::union(found)
    }

    fn iter_types(&mut self, range: TextRange, ty: &Ty) -> (Ty, Ty) {
        if ty.is_union() && ty.members().iter().any(Ty::is_null) {
            let rest = stir_types::without_null(ty);
            let start = self.diags.len();
            let found = self.iter_types(range, &rest);
            if self.diags.len() == start {
                self.error(
                    range,
                    codes::NULLABLE_ACCESS,
                    format!("`{ty}` is not iterable when it's null"),
                );
            }
            return found;
        }
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
                    self.tag_level(3);
                    return (Ty::error(), Ty::error());
                }
                self.tag_partial_union();
            }
            return (Ty::union(keys), Ty::union(vals));
        }
        match ty.kind() {
            TyKind::List { elem, .. } if matches!(elem.kind(), TyKind::Never) => {
                (Ty::int(), Ty::untyped())
            }
            TyKind::List { elem, .. } => (Ty::int(), elem.clone()),
            TyKind::Array { key, value, .. } | TyKind::Iterable { key, value } => {
                (key.clone(), value.clone())
            }
            TyKind::Shape(s) => {
                if s.fields.is_empty() {
                    return (Ty::array_key(), Ty::mixed());
                }
                let keys = s.fields.iter().map(|f| match &f.key {
                    stir_types::ShapeKey::Int(n) => Ty::lit_int(*n),
                    stir_types::ShapeKey::Str(k) => Ty::lit_string(k.clone()),
                });
                let keys = widen_literals(&Ty::union(keys.collect::<Vec<_>>()));
                let values = s.fields.iter().map(|f| f.ty.clone()).collect::<Vec<_>>();
                let mut values = widen_literals(&Ty::union(values));
                if !s.sealed {
                    values = values.nullable();
                }
                (keys, values)
            }
            // an object of all those types: if one is iterable, it is
            TyKind::Intersection(ms) => {
                for m in ms.iter() {
                    let before = self.diags.len();
                    let found = self.iter_types(range, m);
                    if self.diags.len() == before {
                        return found;
                    }
                    self.diags.truncate(before);
                }
                self.error(
                    range,
                    codes::INVALID_OPERAND,
                    format!("`{ty}` is not iterable"),
                );
                self.tag_level(3);
                (Ty::error(), Ty::error())
            }
            TyKind::SelfType { class, .. } => self.iter_types(range, &class.clone()),
            TyKind::Param { .. } => {
                let b = self.bound_of(ty);
                if matches!(b.kind(), TyKind::Param { .. }) {
                    (Ty::untyped(), Ty::untyped())
                } else {
                    self.iter_types(range, &b)
                }
            }
            TyKind::Object => {
                self.error(
                    range,
                    codes::INVALID_OPERAND,
                    format!("`{ty}` is not iterable"),
                );
                self.tag_level(7);
                (Ty::untyped(), Ty::untyped())
            }
            TyKind::Error | TyKind::Mixed => (Ty::error(), Ty::error()),
            TyKind::Never => (Ty::never(), Ty::never()),
            TyKind::Class { name, args } => {
                // Traversable / Iterator / IteratorAggregate
                for anc in self
                    .ctx
                    .ancestors(&Ty::class(name.clone(), args.clone()))
                    .iter()
                {
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
                (Ty::untyped(), Ty::untyped())
            }
            _ if ty.is_untyped() => (Ty::error(), Ty::error()),
            _ => {
                self.error(
                    range,
                    codes::INVALID_OPERAND,
                    format!("`{ty}` is not iterable"),
                );
                self.tag_level(3);
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
                } else if matches!(fcx.ret.kind(), TyKind::Void) {
                    if !t.is_null() {
                        self.error(
                            v.text_range(),
                            codes::RETURN_VALUE,
                            "this function returns `void`; it can't return a value",
                        );
                    }
                } else if !in_generator(node) {
                    self.check_assign(v.text_range(), &t, &fcx.ret, "return value");
                }
            }
            None => {
                if fcx.infer_ret {
                    self.fns.last_mut().unwrap().returns.push(Ty::void());
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
        let saved_dead = std::mem::replace(&mut self.dead, false);
        let saved_loops = std::mem::take(&mut self.loops);
        if let Some(t) = this {
            self.frames.last_mut().unwrap().insert(
                SmolStr::new("this"),
                Local {
                    ty: t.clone(),
                    declared: t,
                    kind: LocalKind::This,
                    range: node.text_range(),
                    maybe_undefined: false,
                },
            );
        }
        // default values, checked against their parameter types
        if let Some(list) = node.child(PARAM_LIST) {
            for (p, pnode) in sig.params.iter().zip(list.children_of(PARAM)) {
                if let Some(d) = exprs_of(&pnode).into_iter().next() {
                    let dt = self.expr(&d, Some(&p.ty));
                    // a default solves the template when it's used, so it can't be wrong for it
                    if stir_types::free_params(&p.ty).is_empty() {
                        self.check_assign(d.text_range(), &dt, &p.ty, "default value");
                    }
                }
            }
        }
        for p in &sig.params {
            let mut ty = p.ty.clone();
            if p.variadic {
                ty = Ty::list(ty);
            }
            self.declare(&p.name, ty, LocalKind::Param, p.range);
        }
        self.fns.push(FnCx {
            ret: self.ctx.decide_conditionals(&sig.ret),
            infer_ret: false,
            returns: Vec::new(),
        });
        if let Some(block) = node.child(BLOCK) {
            self.block_stmts(&block);
        }
        self.dynamic_vars_fn = saved_dynamic_vars;
        self.dead = saved_dead;
        self.loops = saved_loops;
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
            // A trait's code runs as part of the classes using it, so that's where it's checked,
            // as PHPStan does. One nobody here uses can only be checked by itself.
            let users = if model.kind() == ClassKind::Trait {
                self.ctx.trait_users(&model.key)
            } else {
                Vec::new()
            };
            if users.is_empty() {
                self.class_members(&body, &model, &model);
            } else {
                let start = self.diags.len();
                for user in &users {
                    let Some(context) = self.ctx.class(user) else {
                        continue;
                    };
                    self.class = Some(context.clone());
                    self.lower_cx = self.ctx.member_cx(&model, &context);
                    let before = self.diags.len();
                    self.class_members(&body, &model, &context);
                    for d in &mut self.diags[before..] {
                        d.notes.push(format!("in class `{}`", context.name));
                    }
                }
                // the same mistake in the context of each class is one mistake
                let found = self.diags.split_off(start);
                let mut seen = rustc_hash::FxHashSet::default();
                self.diags
                    .extend(found.into_iter().filter(|d| seen.insert((d.range, d.code))));
            }
        }
        self.lower_cx = saved_cx;
        self.class = saved_class;
    }

    /// Checks the bodies of the members `model` declares, in class `context`.
    fn class_members(
        &mut self,
        body: &SyntaxNode,
        model: &Rc<ClassModel<'db>>,
        context: &Rc<ClassModel<'db>>,
    ) {
        for member in body.children() {
            match member.kind() {
                METHOD_DECL => self.method_decl(&member, model, context),
                PROPERTY_DECL => self.property_decl(&member, model),
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

    fn check_class_header(&mut self, model: &Rc<ClassModel<'db>>) {
        let def = model.def;
        let mut sink = Vec::new();
        let cx = self.ctx.class_cx(model);
        for t in def.extends.iter().chain(&def.implements) {
            self.ctx.lower(t, &cx, &mut sink);
        }
        for e in sink {
            self.supertype_error(e);
        }
    }

    fn method_decl(
        &mut self,
        node: &SyntaxNode,
        model: &Rc<ClassModel<'db>>,
        context: &Rc<ClassModel<'db>>,
    ) {
        let ptr = stir_hir::NodePtr::new(node);
        let Some(m) = model.def.methods.iter().find(|m| m.ptr == ptr) else {
            return;
        };
        let sig = self.ctx.method_sig_in(model, m, context);
        let mut sink = Vec::new();
        // report type errors written on the method signature
        let _ = self.ctx.build_sig(
            "m:check",
            &m.name,
            &m.sig,
            &self.ctx.member_cx(model, context),
            &mut sink,
        );
        for e in sink {
            self.lowered_error(e);
        }
        let saved_cx = self.lower_cx.clone();
        self.lower_cx.generics.extend(
            sig.type_params
                .iter()
                .map(|p| (p.name.clone(), p.ty.clone())),
        );
        // where a readonly property may be given its value: from PHP 8.3, in `__clone` too
        let clones_readonly = self.ctx.ws.settings(self.ctx.db).php_version.target()
            >= stir_db::PhpVersion::new(8, 3);
        self.in_ctor = ["__construct", "__unserialize"]
            .iter()
            .any(|n| m.name.eq_ignore_ascii_case(n))
            || (clones_readonly && m.name.eq_ignore_ascii_case("__clone"));
        let this = if m.mods.is_static {
            None
        } else {
            Some(Ty::this_of(context.self_ty()))
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
            for e in exprs_of(&elem) {
                let vt = self.expr(&e, declared.as_ref());
                if let Some(t) = &declared {
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
            // `empty($x)` is `isset($x) && !$x` turned around
            EMPTY_EXPR => {
                let arg = node
                    .child(ARG_LIST)
                    .and_then(|l| l.child(ARG))
                    .and_then(|a| exprs_of(&a).into_iter().next());
                let Some(arg) = arg else {
                    return Default::default();
                };
                let (truthy, falsy) = self.truthiness_facts(&arg);
                let mut not_empty = self.isset_facts(&arg);
                not_empty.narrow.extend(truthy.narrow);
                (falsy, not_empty)
            }
            ISSET_EXPR => {
                let mut yes = Facts::default();
                let mut no = Vec::new();
                let args = node.child(ARG_LIST);
                let args = args.iter().flat_map(|l| l.children_of(ARG));
                let exprs: Vec<_> = args
                    .filter_map(|a| exprs_of(&a).into_iter().next())
                    .collect();
                for e in &exprs {
                    yes.narrow.extend(self.isset_facts(e).narrow);
                }
                // not set means null only if there's just the one thing it could be
                if let [e] = exprs.as_slice()
                    && let Some((name, _)) = self.local_of(e)
                {
                    no.push((name, Ty::null()));
                }
                (yes, Facts { narrow: no })
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
                        (
                            Facts {
                                narrow: [lt.narrow, rt.narrow].concat(),
                            },
                            self.either(lf, rf),
                        )
                    }
                    Some(PIPE2 | OR_KW) => {
                        let (lt, lf) = self.facts(&es[0]);
                        // the right side only runs (and narrows) if the left was false
                        let snap = self.snapshot();
                        self.apply_facts(&lf);
                        let (rt, rf) = self.facts(&es[1]);
                        self.restore_narrowing(&snap);
                        (
                            self.either(lt, rt),
                            Facts {
                                narrow: [lf.narrow, rf.narrow].concat(),
                            },
                        )
                    }
                    Some(k @ (EQ2 | EQ3 | NEQ | NEQ2)) => {
                        let positive = matches!(k, EQ2 | EQ3);
                        let strict = matches!(k, EQ3 | NEQ2);
                        // `($x ?? false) !== false`: `$x` is set
                        if let Some(set) = self.coalesce_compared(&es[0], &es[1]) {
                            let narrow = self.isset_facts(&set);
                            return if positive {
                                (Facts::default(), narrow)
                            } else {
                                (narrow, Facts::default())
                            };
                        }
                        let var = if is_null_literal(&es[1]) {
                            &es[0]
                        } else if is_null_literal(&es[0]) {
                            &es[1]
                        } else {
                            let (mut t, mut f) = self.literal_facts(&es[0], &es[1], strict);
                            let (t2, f2) = self.call_compared_facts(&es[0], &es[1]);
                            t.narrow.extend(t2.narrow);
                            f.narrow.extend(f2.narrow);
                            return if positive { (t, f) } else { (f, t) };
                        };
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
                        let found = self.diags.len();
                        let t = self.class_ref_ty(&r, &n);
                        self.diags.truncate(found);
                        if n.eq_ignore_ascii_case("static") {
                            Ty::static_of(t)
                        } else {
                            t
                        }
                    }
                    // `$a instanceof $b`: an object, of a class known only when it runs
                    Some(_) => {
                        return (
                            Facts {
                                narrow: vec![(
                                    name.clone(),
                                    stir_types::narrow_to(self.ctx, &local.ty, &Ty::object()),
                                )],
                            },
                            Facts::default(),
                        );
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
                let Some(callee) = node.child(NAME_EXPR) else {
                    return Default::default();
                };
                let fname = text_of(&callee)
                    .trim_start_matches('\\')
                    .to_ascii_lowercase();
                let args = self.args_of(node);
                let first = args.first().and_then(|a| exprs_of(a).into_iter().next());
                let test = match fname.as_str() {
                    "is_int" | "is_integer" | "is_long" => Ty::int(),
                    "is_float" | "is_double" => Ty::float(),
                    "is_string" => Ty::string(),
                    "is_bool" => Ty::bool(),
                    "is_null" => Ty::null(),
                    "is_array" => Ty::plain_array(),
                    "is_object" => Ty::object(),
                    "is_resource" => Ty::resource(),
                    "is_numeric" => Ty::union([Ty::int(), Ty::float(), Ty::numeric_string()]),
                    "is_scalar" => Ty::scalar(),
                    "is_iterable" => Ty::plain_iterable(),
                    "is_countable" => {
                        Ty::union([Ty::plain_array(), Ty::class("Countable", Vec::new())])
                    }
                    "is_a" | "is_subclass_of" => {
                        let Some(t) = args
                            .get(1)
                            .and_then(|a| exprs_of(a).into_iter().next())
                            .and_then(|e| self.types.get(&e.text_range()).cloned())
                            .and_then(|t| self.named_class(&t))
                        else {
                            return Default::default();
                        };
                        t
                    }
                    // anything callable isn't null; what else it might be, only the value knows
                    "is_callable" => {
                        let Some((name, local)) = first.and_then(|a| self.local_of(&a)) else {
                            return Default::default();
                        };
                        return (
                            Facts {
                                narrow: vec![(name, stir_types::without_null(&local.ty))],
                            },
                            Facts::default(),
                        );
                    }
                    // what follows may rely on the member, whatever the type says
                    "method_exists" | "property_exists" => {
                        let member = args
                            .get(1)
                            .and_then(|a| exprs_of(a).into_iter().next())
                            .filter(|e| e.kind() == STRING_LITERAL)
                            .map(|e| unquote(&text_of(&e)).to_ascii_lowercase());
                        let Some((key, member)) =
                            first.and_then(|a| self.place_key(&a)).zip(member)
                        else {
                            return Default::default();
                        };
                        return (
                            Facts {
                                narrow: vec![(
                                    SmolStr::new(format!("?has::{key}::{member}")),
                                    Ty::lit_bool(true),
                                )],
                            },
                            Facts::default(),
                        );
                    }
                    // what follows may rely on the class, whether this project has it or not
                    "class_exists" | "interface_exists" | "trait_exists" | "enum_exists" => {
                        let Some(arg) = first else {
                            return Default::default();
                        };
                        let Some(name) = self.mentioned_class(&arg) else {
                            return Default::default();
                        };
                        return (
                            Facts {
                                narrow: vec![(
                                    SmolStr::new(format!("?class::{name}")),
                                    Ty::lit_bool(true),
                                )],
                            },
                            Facts::default(),
                        );
                    }
                    "in_array" => {
                        let strict = args
                            .get(2)
                            .and_then(|a| exprs_of(a).into_iter().next())
                            .is_some_and(|e| is_true_literal(&e));
                        let haystack = args
                            .get(1)
                            .and_then(|a| exprs_of(a).into_iter().next())
                            .and_then(|e| self.types.get(&e.text_range()).cloned());
                        let Some(values) = haystack.as_ref().and_then(|h| self.index_type(h, None))
                        else {
                            return Default::default();
                        };
                        let Some((name, local)) = first.and_then(|a| self.local_of(&a)) else {
                            return Default::default();
                        };
                        if !strict || values.is_untyped() || matches!(values.kind(), TyKind::Mixed)
                        {
                            return Default::default();
                        }
                        return (
                            Facts {
                                narrow: vec![(
                                    name,
                                    stir_types::narrow_to(self.ctx, &local.ty, &values),
                                )],
                            },
                            Facts::default(),
                        );
                    }
                    _ => return self.assert_facts(node),
                };
                let Some((name, local)) = first.and_then(|a| self.local_of(&a)) else {
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
            METHOD_CALL_EXPR | NULLSAFE_METHOD_CALL_EXPR | STATIC_CALL_EXPR => {
                let (t, f) = self.assert_facts(node);
                if t.narrow.is_empty() && f.narrow.is_empty() {
                    self.truthiness_facts(node)
                } else {
                    (t, f)
                }
            }
            VARIABLE_EXPR
            | PROPERTY_FETCH_EXPR
            | NULLSAFE_PROPERTY_FETCH_EXPR
            | INDEX_EXPR
            | STATIC_PROPERTY_FETCH_EXPR
            | ASSIGN_EXPR => self.truthiness_facts(node),
            _ => Default::default(),
        }
    }

    /// In `($x ?? false) === false`, the `$x`: if the comparison fails it was set.
    fn coalesce_compared(&self, a: &SyntaxNode, b: &SyntaxNode) -> Option<SyntaxNode> {
        let mut left = a.clone();
        while left.kind() == PAREN_EXPR {
            left = exprs_of(&left).into_iter().next()?;
        }
        if left.kind() != BIN_EXPR || op_token(&left).is_none_or(|t| t.kind() != QUESTION2) {
            return None;
        }
        let parts = exprs_of(&left);
        let fallback = parts.get(1)?;
        (text_of(fallback).eq_ignore_ascii_case(&text_of(b)))
            .then(|| parts.into_iter().next())
            .flatten()
    }

    /// Facts for a path taken when either of two conditions held: only what both say,
    /// which is either of the things they say.
    fn either(&self, a: Facts, b: Facts) -> Facts {
        let mut narrow = Vec::new();
        for (name, ty) in &a.narrow {
            // the last says what the condition added up to
            let ty = a
                .narrow
                .iter()
                .rev()
                .find(|(n, _)| n == name)
                .map_or(ty, |(_, t)| t);
            if narrow.iter().any(|(n, _): &(SmolStr, Ty)| n == name) {
                continue;
            }
            if let Some((_, other)) = b.narrow.iter().rev().find(|(n, _)| n == name) {
                narrow.push((name.clone(), stir_types::join(self.ctx, ty, other)));
            }
        }
        Facts { narrow }
    }

    /// `if ($x)`: what a value being truthy or falsy says about it.
    fn truthiness_facts(&mut self, node: &SyntaxNode) -> (Facts, Facts) {
        let Some((name, local)) = self.local_of(node) else {
            return Default::default();
        };
        let mut yes = Facts {
            narrow: vec![(name.clone(), stir_types::narrow_truthy(&local.ty))],
        };
        // what's reached through something is only there if that is
        yes.narrow.extend(self.base_facts(node));
        (
            yes,
            Facts {
                narrow: vec![(name, stir_types::narrow_falsy(&local.ty))],
            },
        )
    }

    /// `isset($x->y)` or `isset($x['y'])`: it's set and isn't null, and neither is what it's
    /// reached through.
    fn isset_facts(&mut self, node: &SyntaxNode) -> Facts {
        let mut narrow = self.base_facts(node);
        if let Some((name, local)) = self.local_of(node) {
            narrow.push((name, stir_types::without_null(&local.ty)));
        }
        Facts { narrow }
    }

    fn base_facts(&mut self, node: &SyntaxNode) -> Vec<(SmolStr, Ty)> {
        let mut narrow = Vec::new();
        if matches!(
            node.kind(),
            PROPERTY_FETCH_EXPR | NULLSAFE_PROPERTY_FETCH_EXPR | INDEX_EXPR
        ) && let Some(base) = exprs_of(node).first()
        {
            narrow.extend(self.base_facts(base));
            if let Some((name, local)) = self.local_of(base) {
                narrow.push((name, stir_types::without_null(&local.ty)));
            }
        }
        narrow
    }

    /// The lowercase name of the class an argument is about: `Foo::class`, `'Foo'`.
    fn mentioned_class(&self, arg: &SyntaxNode) -> Option<String> {
        let name = match arg.kind() {
            CLASS_CONST_FETCH_EXPR => {
                let base = arg.children().next().filter(|b| b.kind() == NAME_EXPR)?;
                self.scope(arg)
                    .resolve_class(&text_of(&base))
                    .key()
                    .to_string()
            }
            STRING_LITERAL => unquote(&text_of(arg))
                .trim_start_matches('\\')
                .to_ascii_lowercase(),
            _ => return None,
        };
        Some(name)
    }

    /// The class a value names: `Foo::class`, `'Foo'`.
    fn named_class(&self, t: &Ty) -> Option<Ty> {
        match t.kind() {
            TyKind::ClassString(Some(c)) => Some(c.clone()),
            TyKind::Literal(LitValue::String(s)) => {
                let c = self.ctx.class(s)?;
                Some(Ty::class(c.name.clone(), Vec::new()))
            }
            _ => None,
        }
    }

    /// `x == "a"` narrows a literal union, and `x === y` narrows to what both can be.
    fn literal_facts(&mut self, a: &SyntaxNode, b: &SyntaxNode, strict: bool) -> (Facts, Facts) {
        let lit = |this: &Self, n: &SyntaxNode| {
            this.types
                .get(&n.text_range())
                .is_some_and(|t| matches!(t.kind(), TyKind::Literal(_) | TyKind::Null))
        };
        let (var, other) = if lit(self, a) { (b, a) } else { (a, b) };
        let Some(lt) = self.types.get(&other.text_range()).cloned() else {
            return Default::default();
        };
        let Some((name, local)) = self.local_of(var) else {
            return Default::default();
        };
        if !matches!(lt.kind(), TyKind::Literal(_) | TyKind::Null) {
            // two values: if they're identical each is what the other can be
            if strict
                && !lt.is_error()
                && !lt.is_untyped()
                && !matches!(lt.kind(), TyKind::Mixed)
                && !local.ty.is_untyped()
            {
                return (
                    Facts {
                        narrow: vec![(name, stir_types::narrow_to(self.ctx, &local.ty, &lt))],
                    },
                    Facts::default(),
                );
            }
            return Default::default();
        }
        // `==` compares loosely, so `$x == 'a'` can hold of more than that string
        if !strict
            && !local
                .ty
                .members()
                .iter()
                .all(|m| matches!(m.kind(), TyKind::Literal(_) | TyKind::Null))
        {
            return Default::default();
        }
        let yes = stir_types::narrow_to(self.ctx, &local.ty, &lt);
        let no = stir_types::narrow_away(self.ctx, &local.ty, &lt);
        (
            Facts {
                narrow: vec![(name.clone(), yes)],
            },
            Facts {
                narrow: vec![(name, no)],
            },
        )
    }

    /// `is_string($x) === true`, `in_array(...) === false` and such.
    fn call_compared_facts(&mut self, a: &SyntaxNode, b: &SyntaxNode) -> (Facts, Facts) {
        let bool_lit = |n: &SyntaxNode| {
            (n.kind() == NAME_EXPR)
                .then(|| text_of(n).to_ascii_lowercase())
                .and_then(|t| match t.as_str() {
                    "true" => Some(true),
                    "false" => Some(false),
                    _ => None,
                })
        };
        let (cond, value) = match (bool_lit(a), bool_lit(b)) {
            (_, Some(v)) => (a, v),
            (Some(v), _) => (b, v),
            _ => return Default::default(),
        };
        if !matches!(
            cond.kind(),
            CALL_EXPR | METHOD_CALL_EXPR | STATIC_CALL_EXPR | INSTANCEOF_EXPR | ISSET_EXPR
        ) {
            return Default::default();
        }
        let (t, f) = self.facts(cond);
        if value { (t, f) } else { (f, t) }
    }

    fn local_of(&mut self, node: &SyntaxNode) -> Option<(SmolStr, Local)> {
        let name = match node.kind() {
            VARIABLE_EXPR => text_of(node).trim_start_matches('$').to_owned(),
            PAREN_EXPR => return exprs_of(node).first().and_then(|e| self.local_of(e)),
            // `($x = f()) !== null` is about `$x`
            ASSIGN_EXPR => return node.children().next().and_then(|e| self.local_of(&e)),
            _ => return self.property_place(node),
        };
        self.lookup(&name).map(|l| (SmolStr::new(&name), l.clone()))
    }

    /// `$this->x`, `$a->b()->c` or `$a['k']` as something narrowing can track. It stays narrowed
    /// until it, or something it's reached through, is assigned, or a method on what it's
    /// reached through is called that returns nothing: such a call is there for what it
    /// changes. It's held like a local under a name no variable can have.
    fn property_place(&mut self, node: &SyntaxNode) -> Option<(SmolStr, Local)> {
        let key = self.place_key(node)?;
        if let Some(l) = self.lookup(&key) {
            return Some((SmolStr::new(&key), l.clone()));
        }
        let ty = self.types.get(&node.text_range())?.clone();
        let local = Local {
            ty: ty.clone(),
            declared: ty,
            kind: LocalKind::Variable,
            range: node.text_range(),
            maybe_undefined: false,
        };
        // the outermost frame, so it outlives the block it was narrowed in
        self.frames
            .first_mut()?
            .insert(SmolStr::new(&key), local.clone());
        Some((SmolStr::new(&key), local))
    }

    /// What a call to a function or method with `@phpstan-assert-if-true` or
    /// `@phpstan-assert-if-false` says about its arguments.
    fn assert_facts(&mut self, node: &SyntaxNode) -> (Facts, Facts) {
        let (mut when_true, mut when_false) = (Facts::default(), Facts::default());
        let Some((asserts, cx, solved)) = self.call_asserts(node) else {
            return Default::default();
        };
        for assert in asserts {
            let Some((name, local)) = self.assert_target(node, &assert) else {
                continue;
            };
            let (yes, no) = self.assert_types(&assert, &local, &cx, &solved);
            match assert.when {
                stir_hir::AssertWhen::Always => {}
                stir_hir::AssertWhen::IfTrue => {
                    when_true.narrow.push((name.clone(), yes));
                    when_false.narrow.push((name, no));
                }
                stir_hir::AssertWhen::IfFalse => {
                    when_true.narrow.push((name.clone(), no));
                    when_false.narrow.push((name, yes));
                }
            }
        }
        (when_true, when_false)
    }

    /// A call's plain `@phpstan-assert`s hold once it has returned.
    fn apply_call_asserts(&mut self, node: &SyntaxNode) {
        let Some((asserts, cx, solved)) = self.call_asserts(node) else {
            return;
        };
        for assert in asserts {
            if assert.when != stir_hir::AssertWhen::Always {
                continue;
            }
            let Some((name, local)) = self.assert_target(node, &assert) else {
                continue;
            };
            let (yes, _) = self.assert_types(&assert, &local, &cx, &solved);
            if let Some(l) = self.lookup_mut(&name) {
                l.ty = yes;
            }
        }
    }

    fn assert_types(
        &self,
        assert: &stir_hir::AssertDef,
        local: &Local,
        cx: &LowerCx,
        solved: &FxHashMap<stir_types::ParamId, Ty>,
    ) -> (Ty, Ty) {
        let mut sink = Vec::new();
        let test = stir_types::subst(&self.ctx.lower(&assert.ty, cx, &mut sink), solved);
        let yes = stir_types::narrow_to(self.ctx, &local.ty, &test);
        let no = stir_types::narrow_away(self.ctx, &local.ty, &test);
        if assert.negated { (no, yes) } else { (yes, no) }
    }

    /// The asserts of the function or method `node` calls, what their types are lowered
    /// against, and how the call solved its templates.
    #[allow(clippy::type_complexity)]
    fn call_asserts(
        &mut self,
        node: &SyntaxNode,
    ) -> Option<(
        Vec<stir_hir::AssertDef>,
        LowerCx,
        FxHashMap<stir_types::ParamId, Ty>,
    )> {
        let call = self.call_targets.get(&node.text_range())?.clone();
        if call.asserts.is_empty() {
            return None;
        }
        Some((call.asserts, call.cx, call.solved))
    }

    /// The place an assert is about.
    fn assert_target(
        &mut self,
        node: &SyntaxNode,
        assert: &stir_hir::AssertDef,
    ) -> Option<(SmolStr, Local)> {
        let call = self.call_targets.get(&node.text_range())?.clone();
        let base = if assert.param == "this" {
            let fetch = node.children().next()?;
            if node.kind() == STATIC_CALL_EXPR {
                return None;
            }
            exprs_of(&fetch).into_iter().next()?
        } else {
            let index = call.params.iter().position(|p| *p == assert.param)?;
            let args = self.args_of(node);
            let arg = args.iter().find(|a| {
                a.kind() == NAMED_ARG && a.name_text().as_deref() == Some(assert.param.as_str())
            });
            let arg = arg.or_else(|| args.get(index).filter(|a| a.kind() != NAMED_ARG))?;
            exprs_of(arg).into_iter().next()?
        };
        let Some(member) = &assert.member else {
            return self.local_of(&base);
        };
        let key = format!("{}->{member}", self.place_key(&base)?);
        if let Some(l) = self.lookup(&key) {
            return Some((SmolStr::new(&key), l.clone()));
        }
        let bt = self.types.get(&base.text_range())?.clone();
        let ty = member
            .strip_suffix("()")
            .and_then(|m| {
                let (owner, method, map) = self.ctx.find_method(&class_of(&bt), m)?;
                let sig = self.ctx.method_sig(&owner, method);
                Some(stir_types::subst(&sig.ret, &map))
            })
            .or_else(|| self.ctx.find_property(&class_of(&bt), member).map(|p| p.ty))?;
        let local = Local {
            ty: ty.clone(),
            declared: ty,
            kind: LocalKind::Variable,
            range: node.text_range(),
            maybe_undefined: false,
        };
        self.frames
            .first_mut()?
            .insert(SmolStr::new(&key), local.clone());
        Some((SmolStr::new(&key), local))
    }

    /// The name a variable, or an expression narrowing can track, is held under. Calls with
    /// arguments aren't tracked: the next one may be about something else.
    fn place_key(&self, node: &SyntaxNode) -> Option<String> {
        match node.kind() {
            VARIABLE_EXPR => Some(text_of(node).trim_start_matches('$').to_owned()),
            PAREN_EXPR => self.place_key(exprs_of(node).first()?),
            PROPERTY_FETCH_EXPR | NULLSAFE_PROPERTY_FETCH_EXPR => {
                let base = exprs_of(node).into_iter().next()?;
                let name = node.child(SyntaxKind::NAME).map(|n| text_of(&n))?;
                Some(format!("{}->{name}", self.place_key(&base)?))
            }
            METHOD_CALL_EXPR | NULLSAFE_METHOD_CALL_EXPR => {
                let fetch = node.children().next()?;
                if !self.args_of(node).is_empty() {
                    return None;
                }
                let base = exprs_of(&fetch).into_iter().next()?;
                let name = fetch.child(SyntaxKind::NAME).map(|n| text_of(&n))?;
                Some(format!("{}->{name}()", self.place_key(&base)?))
            }
            INDEX_EXPR => {
                let parts = exprs_of(node);
                let index = parts.get(1)?;
                let index = match index.kind() {
                    STRING_LITERAL | LITERAL | NAME_EXPR | CLASS_CONST_FETCH_EXPR => text_of(index),
                    VARIABLE_EXPR => text_of(index),
                    _ => return None,
                };
                Some(format!("{}[{index}]", self.place_key(parts.first()?)?))
            }
            STATIC_PROPERTY_FETCH_EXPR => Some(text_of(node).to_ascii_lowercase()),
            _ => None,
        }
    }

    /// Assigning to `$a` forgets what was known about `$a->b` and `$a['b']`.
    fn forget_properties_of(&mut self, key: &str) {
        let (arrow, bracket) = (format!("{key}->"), format!("{key}["));
        for frame in &mut self.frames {
            frame.retain(|name, _| !name.starts_with(&arrow) && !name.starts_with(&bracket));
        }
    }

    // ------------------------------------------------------------ expressions

    /// Infers the type of an expression, checking it against `expected` where
    /// that helps inference (the caller still checks assignability).
    pub fn expr(&mut self, node: &SyntaxNode, expected: Option<&Ty>) -> Ty {
        let ty = self.expr_inner(node, expected);
        // a tracked expression holds what it was narrowed to
        let ty = if matches!(
            node.kind(),
            PROPERTY_FETCH_EXPR | METHOD_CALL_EXPR | INDEX_EXPR | STATIC_PROPERTY_FETCH_EXPR
        ) && let Some(key) = self.place_key(node)
            && let Some(l) = self.lookup(&key)
        {
            l.ty.clone()
        } else {
            ty
        };
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
                        Some(v) => Ty::literal(LitValue::Int(v)),
                        // overflowing integer literals are floats in PHP
                        None => Ty::float(),
                    },
                    FLOAT_NUMBER => match tok.text().replace('_', "").parse::<f64>() {
                        Ok(v) => Ty::literal(LitValue::Float(stir_types::FloatLit::new(v))),
                        Err(_) => Ty::error(),
                    },
                    _ => Ty::error(),
                }
            }
            STRING_LITERAL => {
                let text = text_of(node);
                Ty::literal(LitValue::String(SmolStr::new(unquote(&text))))
            }
            INTERP_STRING | HEREDOC => {
                for part in node.children() {
                    if matches!(part.kind(), INTERP_EXPR | INTERP_VAR) {
                        for e in exprs_of(&part) {
                            self.expr(&e, None);
                        }
                    }
                }
                Ty::string()
            }
            SHELL_EXEC => {
                for part in node.children() {
                    for e in exprs_of(&part) {
                        self.expr(&e, None);
                    }
                }
                Ty::union([Ty::string(), Ty::null(), Ty::literal(LitValue::Bool(false))])
            }
            MAGIC_CONST => match op_token(node).map(|t| t.kind()) {
                Some(LINE_KW) => Ty::int(),
                _ => Ty::string(),
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

                Ty::untyped()
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
                let after_a = self.snapshot();
                self.restore(&snap);
                self.apply_facts(&f);
                let b = self.expr(&es[2], expected);
                let after_b = self.snapshot();
                if let Some(state) = self.merge_states(vec![after_a, after_b]) {
                    self.frames = state;
                }
                stir_types::join(self.ctx, &a, &b)
            }
            SHORT_TERNARY_EXPR => {
                let es = exprs_of(node);
                if es.len() < 2 {
                    return Ty::error();
                }
                let l = self.expr(&es[0], expected.map(|e| e.clone().nullable()).as_ref());
                let r = self.expr(&es[1], expected);
                stir_types::join(self.ctx, &l, &r)
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
                Ty::bool()
            }
            CAST_EXPR => {
                if let Some(e) = exprs_of(node).first() {
                    self.expr(e, None);
                }
                match op_token(node).map(|t| t.kind()) {
                    Some(INT_CAST) => Ty::int(),
                    Some(FLOAT_CAST) => Ty::float(),
                    Some(STRING_CAST) => Ty::string(),
                    Some(BOOL_CAST) => Ty::bool(),
                    Some(ARRAY_CAST) => Ty::plain_array(),
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

                let indexed = stir_types::without_null(&bt);
                if let (TyKind::Shape(s), Some(key), Some(i)) =
                    (indexed.kind(), it.as_ref().and_then(shape_key_of), idx)
                    && s.field(&key).is_none()
                    && s.sealed
                    && self.probing == 0
                    && !written_through
                    && !is_write_target(node)
                {
                    self.error(
                        i.text_range(),
                        codes::INVALID_OPERAND,
                        format!("offset {} doesn't exist on `{bt}`", text_of(i)),
                    );
                    self.tag_level(3);
                }
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
                        if !bt.is_error() && !bt.is_untyped() {
                            self.error(
                                base.text_range(),
                                codes::INVALID_OPERAND,
                                format!("`{bt}` can't be indexed"),
                            );
                            self.tag_level(3);
                        }
                        Ty::error()
                    }
                }
            }
            ARRAY_EXPR => self.array_literal(node, expected),
            LIST_EXPR => Ty::untyped(),
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
                Ty::bool()
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
                Ty::untyped()
            }
            EXIT_EXPR => {
                if let Some(args) = node.child(ARG_LIST) {
                    for a in args.children_of(ARG) {
                        for e in exprs_of(&a) {
                            self.expr(&e, None);
                        }
                    }
                }
                Ty::never()
            }
            PRINT_EXPR => {
                for e in exprs_of(node) {
                    self.expr(&e, None);
                }
                Ty::literal(LitValue::Int(1))
            }
            YIELD_EXPR | YIELD_FROM_EXPR => {
                for e in exprs_of(node) {
                    self.expr(&e, None);
                }
                Ty::untyped()
            }
            THROW_EXPR => {
                for e in exprs_of(node) {
                    let t = self.expr(&e, None);
                    if !t.is_error()
                        && !t.is_untyped()
                        && !self.assignable(&t, &Ty::class("Throwable", Vec::new()))
                    {
                        self.error(
                            e.text_range(),
                            codes::TYPE_MISMATCH,
                            format!("only `Throwable`s can be thrown, found `{t}`"),
                        );
                    }
                }
                Ty::never()
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
                if l.maybe_undefined
                    && self.probing == 0
                    && self.fns.len() > 1
                    && self.dynamic_vars_fn != Some(self.fns.len())
                {
                    self.error(
                        node.text_range(),
                        codes::UNDEFINED_VARIABLE,
                        format!("variable `${name}` might not be defined"),
                    );
                    self.tag_level(1);
                }
                l.ty
            }
            None => {
                if is_superglobal(name) {
                    return Ty::untyped();
                }
                if self.dynamic_vars_fn == Some(self.fns.len()) {
                    self.declare(name, Ty::untyped(), LocalKind::Variable, node.text_range());
                    return Ty::untyped();
                }
                if self.probing > 0 {
                    // from here on, it may exist
                    self.declare(name, Ty::untyped(), LocalKind::Variable, node.text_range());
                    return Ty::untyped();
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
                    Ty::untyped()
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
            "true" => return Ty::literal(LitValue::Bool(true)),
            "false" => return Ty::literal(LitValue::Bool(false)),
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
                if !self.guarded_by("defined", &text) {
                    let name = text.trim_start_matches('\\');
                    self.report_going_away(
                        node.text_range(),
                        &format!("`{name}`"),
                        loc.file,
                        &self.index().limited_consts,
                        resolved
                            .fallback
                            .as_ref()
                            .unwrap_or(&resolved.primary)
                            .as_str(),
                    );
                }
                self.const_type(loc)
            }
            None if self.guarded_by("defined", &text) => Ty::untyped(),
            None => {
                let global = resolved.fallback.as_ref().unwrap_or(&resolved.primary);
                if let Some(&available) = self.index().limited_consts.get(global.as_str()) {
                    let what = format!("`{}`", global.as_str());
                    self.report_limited(node.text_range(), &what, available);
                    return Ty::error();
                }
                self.diags.push(
                    Diagnostic::error(
                        self.path.clone(),
                        node.text_range(),
                        codes::UNDEFINED_VARIABLE,
                        format!("unknown name `{text}`"),
                    )
                    .with_symbol(global.as_str()),
                );
                Ty::error()
            }
        }
    }

    fn const_type(&mut self, loc: stir_hir::ItemLoc) -> Ty {
        let items = self.ctx.items(loc.file);
        let c = &items.consts[loc.index as usize];
        let Some(ptr) = &c.value else {
            return Ty::untyped();
        };
        let root = stir_db::parse(self.ctx.db, loc.file).syntax();
        let node = ptr.to_node(&root);
        // literal constants only (no evaluation)
        let (int, float, boolean) = (Ty::int(), Ty::float(), Ty::bool());
        match node.kind() {
            LITERAL => match op_token(&node).map(|t| (t.kind(), t.text().to_owned())) {
                Some((INT_NUMBER, _)) => int,
                Some((FLOAT_NUMBER, _)) => float,
                _ => Ty::untyped(),
            },
            STRING_LITERAL => match unquoted_literal(&node) {
                Some(value) => Ty::literal(LitValue::String(value.into())),
                None => Ty::string(),
            },
            INTERP_STRING => Ty::string(),
            NAME_EXPR => match text_of(&node).to_ascii_lowercase().as_str() {
                "true" | "false" => boolean,
                "null" => Ty::null(),
                _ => Ty::untyped(),
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
            _ => Ty::untyped(),
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
                self.restore_narrowing(&snap);
                (l, r)
            }
            QUESTION2 => {
                self.probing += 1;
                let l = self.expr(&es[0], expected.map(|e| e.clone().nullable()).as_ref());
                self.probing -= 1;
                self.check_pointless_coalesce(&es[0], &l);
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
                    Ty::bool()
                }
                _ => Ty::error(),
            };
        }
        match op {
            PLUS | MINUS | STAR | SLASH | PERCENT | POW => self.arith_result(op, &l, &r),
            DOT => Ty::string(),
            SPACESHIP => Ty::int(),
            EQ2 | EQ3 | NEQ | NEQ2 | L_ANGLE | R_ANGLE | LTEQ | GTEQ | AMP2 | PIPE2 | AND_KW
            | OR_KW | XOR_KW => Ty::bool(),
            // `|`, `&` and `^` on strings work on their bytes
            AMP | PIPE | CARET
                if self.assignable(&l, &Ty::string()) && self.assignable(&r, &Ty::string()) =>
            {
                Ty::string()
            }
            AMP | PIPE | CARET | SHL | SHR => Ty::int(),
            QUESTION2 => stir_types::join(self.ctx, &stir_types::without_null(&l), &r),
            PIPE_GT => match r.kind() {
                TyKind::Callable { sig: Some(s), .. } => s.ret.clone(),
                _ => Ty::untyped(),
            },
            _ => Ty::error(),
        }
    }

    /// `$s ?? "x"` on a variable that's always there and can't be null.
    fn check_pointless_coalesce(&mut self, left: &SyntaxNode, ty: &Ty) {
        if left.kind() != VARIABLE_EXPR
            || ty.is_error()
            || ty.is_never()
            || ty.is_untyped()
            || stir_types::is_nullable(self.ctx, ty)
        {
            return;
        }
        let name = text_of(left);
        let name = name.trim_start_matches('$');
        if self.lookup(name).is_none_or(|l| l.maybe_undefined) {
            return;
        }
        self.diags.push(
            Diagnostic::error(
                self.path.clone(),
                left.text_range(),
                codes::UNNECESSARY_NULL_CHECK,
                format!("`${name}` is always set and is `{ty}`, which can't be null"),
            )
            .with_severity(stir_db::Severity::Warning)
            .with_php_level(1),
        );
    }

    fn arith_result(&self, op: SyntaxKind, l: &Ty, r: &Ty) -> Ty {
        let is_float = |t: &Ty| self.assignable(t, &Ty::float());
        let is_int = |t: &Ty| self.assignable(t, &Ty::int());
        let unknown = |t: &Ty| t.is_untyped() || matches!(t.kind(), TyKind::Mixed | TyKind::Error);
        let is_array = |t: &Ty| {
            t.members().iter().all(|m| {
                matches!(
                    m.kind(),
                    TyKind::Array { .. } | TyKind::List { .. } | TyKind::Shape(_) | TyKind::Null
                )
            })
        };
        // `$a + $b` on arrays is a union of their entries
        let any_array = |t: &Ty| {
            t.members().iter().any(|m| {
                matches!(
                    m.kind(),
                    TyKind::Array { .. } | TyKind::List { .. } | TyKind::Shape(_)
                )
            })
        };
        if op == PLUS && (any_array(l) || any_array(r)) {
            if let (TyKind::Shape(a), TyKind::Shape(b)) = (l.kind(), r.kind()) {
                let mut fields = a.fields.clone();
                for f in &b.fields {
                    if !fields.iter().any(|x| x.key == f.key) {
                        fields.push(f.clone());
                    }
                }
                return Ty::shape(fields, a.sealed && b.sealed);
            }
            return Ty::array(Ty::array_key(), Ty::mixed());
        }
        let (int, float) = (Ty::int(), Ty::float());
        // something added to what may be an array may be an array
        if op == PLUS && ((unknown(l) && is_array(r)) || (is_array(l) && unknown(r))) {
            return Ty::array(Ty::array_key(), Ty::mixed());
        }
        if op == PLUS && unknown(l) && unknown(r) {
            return Ty::untyped();
        }
        if unknown(l) || unknown(r) {
            Ty::union([int, float])
        } else if is_int(l) && is_int(r) {
            match op {
                // `/` gives a float unless it divides evenly
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
                Ty::bool()
            }
            MINUS | PLUS => {
                let t = self.expr(e, None);
                if let TyKind::Literal(l) = t.kind()
                    && op.kind() == MINUS
                {
                    return match l {
                        LitValue::Int(i) => Ty::lit_int(-i),
                        LitValue::Float(f) => Ty::lit_float(-f.get()),
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
            // taking a reference makes the variable exist
            AMP => {
                if e.kind() == VARIABLE_EXPR {
                    let n = text_of(e);
                    let n = n.trim_start_matches('$');
                    match self.lookup_mut(n) {
                        Some(l) => l.maybe_undefined = false,
                        None if n != "this" && !is_superglobal(n) => {
                            self.declare(n, Ty::null(), LocalKind::Variable, e.text_range())
                        }
                        None => {}
                    }
                }
                self.expr(e, expected)
            }
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
        let t = self.assign_inner(node);
        // a failed path doesn't assign: `$x = throw ...`
        if t.is_never() {
            self.dead = true;
        }
        t
    }

    fn assign_inner(&mut self, node: &SyntaxNode) -> Ty {
        let es = exprs_of(node);
        // `$a = &$b` makes `$b` exist
        if node.has_token(AMP)
            && let Some(source) = es.get(1).filter(|e| e.kind() == VARIABLE_EXPR)
        {
            let n = text_of(source);
            let n = n.trim_start_matches('$');
            match self.lookup_mut(n) {
                Some(l) => l.maybe_undefined = false,
                None if n != "this" && !is_superglobal(n) => {
                    self.declare(n, Ty::null(), LocalKind::Variable, source.text_range())
                }
                None => {}
            }
        }
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
                        if n == "this" {
                            self.check_mutable_target(lhs);
                        } else if let Some(loc) = self.lookup_mut(&n) {
                            // PHP variables are dynamically typed
                            loc.ty = widen_literals(&vt);
                            loc.declared = loc.ty.clone();
                            loc.maybe_undefined = false;
                        }
                        self.types.insert(lhs.text_range(), l.declared.clone());
                        vt
                    }
                    None => {
                        let vt = self.expr(rhs, None);
                        self.declare(
                            &n,
                            widen_literals(&vt),
                            LocalKind::Variable,
                            lhs.text_range(),
                        );
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
                    DOT_EQ => Ty::string(),
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

                // a variable holds whatever it's given
                if let Some(n) = self.local_name(lhs) {
                    self.forget_properties_of(&n);
                    match self.lookup_mut(&n) {
                        Some(l) if l.kind != LocalKind::This => {
                            l.ty = widen_literals(&result);
                            l.declared = l.ty.clone();
                            l.maybe_undefined = false;
                        }
                        Some(_) => {}
                        None => self.declare(
                            &n,
                            widen_literals(&result),
                            LocalKind::Variable,
                            lhs.text_range(),
                        ),
                    }
                    return result;
                }
                self.check_assign(rhs.text_range(), &result, &declared, "assignment");
                if lhs.kind() == INDEX_EXPR
                    && let Some(key) = exprs_of(lhs).first().and_then(|b| self.place_key(b))
                {
                    self.forget_properties_of(&key);
                }
                if matches!(lhs.kind(), PROPERTY_FETCH_EXPR | STATIC_PROPERTY_FETCH_EXPR)
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
                        narrow_assigned(self.ctx, &declared, &result)
                    };
                    if let Some(frame) = self.frames.first_mut() {
                        frame.insert(
                            SmolStr::new(&key),
                            Local {
                                ty: assigned,
                                declared: declared.clone(),
                                kind: LocalKind::Variable,
                                range: lhs.text_range(),
                                maybe_undefined: false,
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
        let array_like = |t: &Ty| {
            matches!(
                t.kind(),
                TyKind::Array { .. } | TyKind::List { .. } | TyKind::Shape(_)
            )
        };
        let grows = match &current {
            None => true,
            Some(t) if t.is_union() => t.members().iter().any(array_like),
            Some(t) => array_like(t) || matches!(t.kind(), TyKind::Null),
        };
        if !grows {
            return None;
        }
        indexes.reverse();
        let keys: Vec<Option<Ty>> = indexes
            .iter()
            .map(|i| i.as_ref().map(|i| self.expr(i, None)))
            .collect();
        let vt = self.expr(rhs, None);
        self.types.insert(rhs.text_range(), vt.clone());
        let in_loop = !self.loops.is_empty();
        let value = match op {
            EQ => vt,
            DOT_EQ => Ty::string(),
            _ => {
                let at = keys.iter().fold(current.clone(), |c, k| {
                    c.and_then(|c| self.index_type(&c, k.as_ref()))
                });
                match (op, at) {
                    (QUESTION2_EQ, Some(at)) => {
                        stir_types::join(self.ctx, &stir_types::without_null(&at), &vt)
                    }
                    (QUESTION2_EQ, None) => vt,
                    (PLUS_EQ | MINUS_EQ | STAR_EQ | SLASH_EQ | PERCENT_EQ | POW_EQ, Some(at)) => {
                        let k = match op {
                            PLUS_EQ => PLUS,
                            MINUS_EQ => MINUS,
                            STAR_EQ => STAR,
                            SLASH_EQ => SLASH,
                            PERCENT_EQ => PERCENT,
                            _ => POW,
                        };
                        self.arith_result(k, &at, &vt)
                    }
                    _ => Ty::mixed(),
                }
            }
        };
        let grown = self.array_written(current.as_ref(), &keys, &value, in_loop);
        self.forget_properties_of(name);
        match self.lookup_mut(name) {
            Some(l) => {
                l.ty = grown.clone();
                l.declared = grown;
                l.maybe_undefined = false;
            }
            None => self.declare(name, grown, LocalKind::Variable, root.text_range()),
        }
        Some(())
    }

    /// The type of an array after writing `value` at `keys` in it. Outside loops a known key
    /// keeps it a shape. In a loop it can't be said how many rounds there are, so `$a[] = v`
    /// makes a list.
    fn array_written(
        &self,
        current: Option<&Ty>,
        keys: &[Option<Ty>],
        value: &Ty,
        in_loop: bool,
    ) -> Ty {
        let Some((key, rest)) = keys.split_first() else {
            return value.clone();
        };
        if let Some(c) = current.filter(|c| c.is_union()) {
            let members = c.members().iter().filter_map(|m| match m.kind() {
                TyKind::Null | TyKind::Literal(LitValue::Bool(false)) => None,
                _ => Some(self.array_written(Some(m), keys, value, in_loop)),
            });
            return Ty::union(members.collect::<Vec<_>>());
        }
        let at = current.and_then(|c| self.index_type(c, key.as_ref()));
        let new = if rest.is_empty() {
            value.clone()
        } else {
            self.array_written(at.as_ref(), rest, value, in_loop)
        };
        let shape_key = key.as_ref().and_then(shape_key_of);
        let empty = match current.map(Ty::kind) {
            None | Some(TyKind::Null) => true,
            Some(TyKind::List { elem, .. }) => elem.is_never(),
            _ => false,
        };
        if empty {
            return match (key, shape_key) {
                (None, _) if in_loop => Ty::non_empty_list(widen_literals(&new)),
                (None, _) => Ty::shape(vec![stir_types::ShapeField::new(0, new)], true),
                (Some(_), Some(k)) if !in_loop => {
                    Ty::shape(vec![stir_types::ShapeField::new(k, new)], true)
                }
                (Some(k), _) => Ty::non_empty_array(widen_literals(k), widen_literals(&new)),
            };
        }
        match current.map(Ty::kind) {
            Some(TyKind::Shape(s)) => {
                match (key, shape_key) {
                    (Some(_), Some(k))
                        if s.field(&k).is_some()
                            || (!in_loop && s.fields.len() < MAX_SHAPE_FIELDS) =>
                    {
                        let mut fields = s.fields.clone();
                        set_shape_field(&mut fields, k, new);
                        return Ty::shape(fields, s.sealed);
                    }
                    (None, _) if !in_loop && s.fields.len() < MAX_SHAPE_FIELDS => {
                        let next = s
                            .fields
                            .iter()
                            .filter_map(|f| match f.key {
                                stir_types::ShapeKey::Int(n) => Some(n + 1),
                                _ => None,
                            })
                            .max()
                            .unwrap_or(0);
                        let mut fields = s.fields.clone();
                        set_shape_field(&mut fields, stir_types::ShapeKey::Int(next), new);
                        return Ty::shape(fields, s.sealed);
                    }
                    _ => {}
                }
                let found = self.diags.len();
                let _ = found;
                let keys_now = s.fields.iter().map(|f| match &f.key {
                    stir_types::ShapeKey::Int(_) => Ty::int(),
                    stir_types::ShapeKey::Str(_) => Ty::string(),
                });
                let values = s.fields.iter().map(|f| widen_literals(&f.ty));
                let all_ints = s
                    .fields
                    .iter()
                    .all(|f| matches!(f.key, stir_types::ShapeKey::Int(_)));
                if key.is_none() && all_ints {
                    return Ty::non_empty_list(Ty::union(
                        values
                            .chain(std::iter::once(widen_literals(&new)))
                            .collect::<Vec<_>>(),
                    ));
                }
                let new_key = key.as_ref().map_or_else(Ty::int, widen_literals);
                Ty::non_empty_array(
                    Ty::union(keys_now.chain(std::iter::once(new_key)).collect::<Vec<_>>()),
                    Ty::union(
                        values
                            .chain(std::iter::once(widen_literals(&new)))
                            .collect::<Vec<_>>(),
                    ),
                )
            }
            Some(TyKind::List { elem, .. }) => match key {
                None => Ty::non_empty_list(Ty::union([elem.clone(), widen_literals(&new)])),
                Some(k) => Ty::non_empty_array(
                    Ty::union([Ty::int(), widen_literals(k)]),
                    Ty::union([elem.clone(), widen_literals(&new)]),
                ),
            },
            Some(TyKind::Array {
                key: k0, value: v0, ..
            }) => Ty::non_empty_array(
                Ty::union([
                    k0.clone(),
                    key.as_ref().map_or_else(Ty::int, widen_literals),
                ]),
                Ty::union([v0.clone(), widen_literals(&new)]),
            ),
            _ => Ty::array(Ty::array_key(), Ty::mixed()),
        }
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
                l.maybe_undefined = false;
                // what it holds now is what the callee made of it
                let keeps = matches!(l.ty.kind(), TyKind::Array { .. } | TyKind::List { .. });
                if !keeps {
                    l.ty = Ty::untyped();
                    l.declared = Ty::untyped();
                }
            }
            None => self.declare(name, Ty::untyped(), LocalKind::Variable, arg.text_range()),
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
        if matches!(
            node.kind(),
            PROPERTY_FETCH_EXPR | STATIC_PROPERTY_FETCH_EXPR | INDEX_EXPR
        ) {
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
        let bt = stir_types::without_null(&bt);
        if let Some(p) = self.ctx.find_property(&class_of(&bt), &name)
            && p.immutable
        {
            let in_own_ctor = self.in_ctor
                && self.class.as_ref().is_some_and(|c| {
                    c.key == p.class.key
                        || (p.class.kind() == ClassKind::Trait
                            && self.ctx.is_subclass(&c.name, &p.class.name))
                });
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

    fn index_type(&self, base: &Ty, index: Option<&Ty>) -> Option<Ty> {
        match base.kind() {
            TyKind::Shape(s) => {
                let key = index.and_then(shape_key_of);
                if let Some(key) = &key {
                    return Some(match s.field(key) {
                        Some(f) => f.ty.clone(),
                        None => Ty::untyped(),
                    });
                }
                if s.fields.is_empty() || !s.sealed {
                    return Some(Ty::mixed());
                }
                let values = s.fields.iter().map(|f| f.ty.clone()).collect::<Vec<_>>();
                Some(widen_literals(&Ty::union(values)))
            }
            // in a loop, an array that starts empty may have been written to by now
            TyKind::List { elem, .. } if matches!(elem.kind(), TyKind::Never) => {
                Some(Ty::untyped())
            }
            TyKind::List { elem, .. } => Some(elem.clone()),
            TyKind::Array { value, .. } | TyKind::Iterable { value, .. } => Some(value.clone()),
            // a string, if it isn't an int
            TyKind::ArrayKey => Some(Ty::string()),
            // a string or an array, if it's anything that can be indexed
            TyKind::Callable { .. }
            | TyKind::Object
            | TyKind::SelfType { .. }
            | TyKind::Intersection(_) => Some(Ty::untyped()),
            TyKind::String
            | TyKind::NonEmptyString
            | TyKind::NumericString
            | TyKind::ClassString(_) => Some(Ty::string()),
            TyKind::Literal(LitValue::String(_)) => Some(Ty::string()),
            TyKind::Mixed => Some(Ty::mixed()),
            TyKind::Error => Some(Ty::error()),
            TyKind::Never => Some(Ty::never()),
            TyKind::Param { .. } => Some(Ty::untyped()),
            TyKind::Class { .. } => Some(Ty::untyped()),
            _ => None,
        }
    }

    fn array_literal(&mut self, node: &SyntaxNode, expected: Option<&Ty>) -> Ty {
        let elems = node.children_of(ARRAY_ELEM);
        // `[]` where a PHP array type is expected
        if elems.is_empty()
            && let Some(e) = expected
            && let Some(array) = e.members().iter().find(|m| {
                matches!(
                    m.kind(),
                    TyKind::Array { .. } | TyKind::List { .. } | TyKind::Shape(_)
                )
            })
        {
            return array.clone();
        }
        // what the elements are expected to be, for closures and nested literals
        let expected_at = |key: Option<&stir_types::ShapeKey>| {
            let e = expected?;
            e.members().iter().find_map(|m| match m.kind() {
                TyKind::Array { value, .. } => Some(value.clone()),
                TyKind::List { elem, .. } => Some(elem.clone()),
                TyKind::Shape(s) => key.and_then(|k| s.field(k)).map(|f| f.ty.clone()),
                _ => None,
            })
        };
        // while keys are known it's a shape, as long as it's a small one
        let mut fields: Option<Vec<stir_types::ShapeField>> = Some(Vec::new());
        let mut next_index = 0i64;
        let mut keys = Vec::new();
        let mut vals = Vec::new();
        let mut keyed = false;
        for e in &elems {
            let parts = exprs_of(e);
            let spread = e.has_token(ELLIPSIS);
            match parts.as_slice() {
                [k, v] if e.has_token(FAT_ARROW) => {
                    keyed = true;
                    let kt = self.expr(k, None);
                    let key = shape_key_of(&kt);
                    let vt = self.expr(v, expected_at(key.as_ref()).as_ref());
                    match (key, &mut fields) {
                        (Some(key), Some(fs)) => {
                            if let stir_types::ShapeKey::Int(n) = key {
                                next_index = next_index.max(n + 1);
                            }
                            set_shape_field(fs, key, vt.clone());
                        }
                        _ => fields = None,
                    }
                    keys.push(kt);
                    vals.push(vt);
                }
                [v] if spread => {
                    let t = self.expr(v, expected);
                    match t.kind() {
                        TyKind::List { elem, .. } => vals.push(elem.clone()),
                        TyKind::Array { key, value, .. } => {
                            if !matches!(key.kind(), TyKind::Int) {
                                keyed = true;
                                keys.push(key.clone());
                            }
                            vals.push(value.clone());
                        }
                        TyKind::Shape(s) => {
                            for f in &s.fields {
                                vals.push(f.ty.clone());
                            }
                        }
                        _ => {
                            let found = self.diags.len();
                            let (k, it) = self.iter_types(v.text_range(), &t);
                            self.diags.truncate(found);
                            if !matches!(k.kind(), TyKind::Int) {
                                keyed = true;
                                keys.push(k);
                            }
                            vals.push(it);
                        }
                    }
                    fields = None;
                }
                [v] => {
                    let key = stir_types::ShapeKey::Int(next_index);
                    let vt = self.expr(v, expected_at(Some(&key)).as_ref());
                    if let Some(fs) = &mut fields {
                        set_shape_field(fs, key, vt.clone());
                    }
                    next_index += 1;
                    vals.push(vt);
                }
                _ => {}
            }
        }
        if let Some(fs) = fields
            && fs.len() <= MAX_SHAPE_FIELDS
            && !fs.is_empty()
        {
            return Ty::shape(fs, true);
        }
        if keyed {
            let key = if keys.is_empty() {
                Ty::array_key()
            } else {
                widen_literals(&Ty::union(keys))
            };
            let value = if vals.is_empty() {
                Ty::mixed()
            } else {
                widen_literals(&Ty::union(vals))
            };
            return if elems.is_empty() {
                Ty::array(key, value)
            } else {
                Ty::non_empty_array(key, value)
            };
        }
        Ty::list(if vals.is_empty() {
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
                // a class is free to replace a trait's method, unless it's abstract
                if c.kind() == ClassKind::Trait && !sm.mods.is_abstract {
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
                // PHPStan doesn't compare the PHPDoc types of static methods
                // (`reportStaticMethodSignatures`)
                if m.mods.is_static {
                    break;
                }
                // a method's own templates are matched by position
                let mut map = map;
                map.extend(
                    sup.type_params
                        .iter()
                        .zip(&sub.type_params)
                        .map(|(a, b)| (a.id, b.ty.clone())),
                );
                for (sp, bp) in sub.params.iter().zip(&sup.params) {
                    let want = stir_types::subst(&bp.ty, &map);
                    if !self.override_compatible(&want, &sp.ty) {
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
                if sup.ret_declared && !self.override_compatible(&sub_ret, &sup_ret) {
                    self.error(m.name_range, codes::INCOMPATIBLE_OVERRIDE, format!("return type `{sub_ret}` is not compatible with `{sup_ret}` of `{}::{}`", c.name, sm.name));
                    self.tag_php_override();
                }
                break;
            }
        }
    }

    /// Like PHPStan without `reportMaybesInMethodSignatures`, only types that can't hold the
    /// same value are incompatible: a parent's `@return Base` overridden by `@return Child|int`
    /// goes unreported. Templates that aren't resolved say nothing either way.
    fn override_compatible(&self, a: &Ty, b: &Ty) -> bool {
        a.is_error()
            || b.is_error()
            || a.is_never()
            || !stir_types::free_params(a).is_empty()
            || !stir_types::free_params(b).is_empty()
            || !stir_types::meet(self.ctx, a, b).is_never()
            || stir_types::maybe_assignable(self.ctx, a, b)
            || stir_types::maybe_assignable(self.ctx, b, a)
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
            return Ty::closure(Some(sig.fn_ty()));
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
            if let Some(this) = assigned[i].and_then(|p| sig.params[p].closure_this.as_ref()) {
                self.closure_bind = Some(ClosureBind {
                    this: Some(inf.apply(&subst_cls(this))),
                    class: self.class.clone(),
                });
            }
            let t = self.expr(&v, exp.as_ref());
            if let Some(p) = &pty {
                inf.unify(self.ctx, p, &t);
            }
            arg_tys[i] = Some(t);
        }
        // a parameter left out holds its default, which says what its template is
        for (i, p) in sig.params.iter().enumerate() {
            if let Some(default) = &p.default
                && !provided.contains(&i)
                && !has_spread
            {
                let pty = subst_cls(&p.ty);
                if inf.has_unsolved(&pty) {
                    inf.unify_expected(self.ctx, &pty, default);
                }
            }
        }
        // PHP code can't spell type arguments: nothing was said about this variable
        let ctx = self.ctx;
        inf.finish_with(|id| stir_types::TypeEnv::param_bound(ctx, id).unwrap_or_else(Ty::untyped));
        for (i, a) in args.iter().enumerate() {
            let (Some(p), Some(t)) = (assigned[i], &arg_tys[i]) else {
                continue;
            };
            let param = &sig.params[p];
            let want = inf.apply(&subst_cls(&param.ty));
            let range = value_of(a)
                .map(|v| v.text_range())
                .unwrap_or(a.text_range());
            // a variable the call fills in: nothing is known of it yet, and it isn't read
            if param.by_ref && t.is_untyped() {
                continue;
            }
            if !self.items.strict_types && accepts_php_scalars(&want) {
                // without `strict_types`, PHP coerces scalar arguments
                let rest: Vec<Ty> = t
                    .members()
                    .iter()
                    .filter(|m| !coerces_to(m, &want) && !self.is_stringable_for(m, &want))
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
        // `@param-out`: what a by-reference argument holds now
        for (i, a) in args.iter().enumerate() {
            let Some(out) = assigned[i].and_then(|p| sig.params[p].out.as_ref()) else {
                continue;
            };
            let out = inf.apply(&subst_cls(out));
            if let Some(key) = value_of(a).and_then(|v| self.place_key(&v))
                && let Some(l) = self.lookup_mut(&key)
                && l.kind != LocalKind::This
            {
                l.ty = out.clone();
                l.declared = out;
            }
        }
        // what a conditional return type is about
        for (index, id) in &sig.conditional_on {
            let given = assigned
                .iter()
                .position(|p| *p == Some(*index))
                .and_then(|a| arg_tys[a].clone());
            let param = &sig.params[*index];
            let t = given
                .or_else(|| param.default.clone())
                .unwrap_or_else(|| param.ty.clone());
            inf.solved.insert(*id, inf.apply(&t));
        }
        let ret = self
            .ctx
            .decide_conditionals(&inf.apply(&subst_cls(&sig.ret)));
        self.last_solved.clone_from(&inf.solved);
        self.last_solved
            .extend(class_map.iter().map(|(k, v)| (*k, v.clone())));
        ret
    }

    /// Remembers that the call `node` went to a function with these asserts, and applies
    /// the ones that hold whatever it returns.
    fn record_asserts(
        &mut self,
        node: &SyntaxNode,
        def: &stir_hir::Signature,
        sig: &FnSig,
        mut cx: LowerCx,
    ) {
        if def.asserts.is_empty() {
            return;
        }
        cx.generics.extend(
            sig.type_params
                .iter()
                .map(|p| (p.name.clone(), p.ty.clone())),
        );
        self.call_targets.insert(
            node.text_range(),
            CallAsserts {
                asserts: def.asserts.clone(),
                params: sig.params.iter().map(|p| p.name.clone()).collect(),
                cx,
                solved: self.last_solved.clone(),
            },
        );
        self.apply_call_asserts(node);
    }

    /// Without `strict_types`, an object with `__toString` coerces to a `string` argument.
    fn is_stringable_for(&self, ty: &Ty, want: &Ty) -> bool {
        want.members()
            .iter()
            .any(|m| matches!(m.kind(), TyKind::String))
            && matches!(class_of(ty).kind(), TyKind::Class { .. })
            && self.ctx.find_method(&class_of(ty), "__toString").is_some()
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
                    return Ty::untyped();
                }
                let global = r.fallback.as_ref().unwrap_or(&r.primary);
                if let Some(&available) = self.index().limited_functions.get(global.key().as_str())
                {
                    let what = format!("`{}()`", global.as_str());
                    self.report_limited(callee.text_range(), &what, available);
                    self.check_args_untyped(node);
                    return Ty::error();
                }
                self.diags.push(
                    Diagnostic::error(
                        self.path.clone(),
                        callee.text_range(),
                        codes::UNDEFINED_FUNCTION,
                        format!("unknown function `{text}`"),
                    )
                    .with_symbol(global.as_str()),
                );
                self.check_args_untyped(node);
                return Ty::error();
            };
            self.def(callee.text_range(), Def::Function(loc));
            if !self.guarded_by("function_exists", &text) {
                self.report_going_away(
                    callee.text_range(),
                    &format!("`{}()`", fqn.as_str()),
                    loc.file,
                    &self.index().limited_functions,
                    fqn.key().as_str(),
                );
            }
            // several declarations: overloaded stubs, or a PHP conditional declaration
            let overloads = self.ctx.function_overloads(fqn.as_str());
            if overloads.len() > 1 {
                let start = self.diags.len();
                let mut best: Option<(usize, Vec<Diagnostic>, Ty)> = None;
                for candidate in &overloads {
                    let sig = self.ctx.function_sig(*candidate);
                    let t = self.check_args(node, &sig, &FxHashMap::default(), expected, &[]);
                    let found = self.diags.split_off(start);
                    // what counts is what would be reported at this level
                    let level = self.ctx.ws.settings(self.ctx.db).level();
                    let score = found
                        .iter()
                        .filter(|d| crate::levels::min_level(d) <= level)
                        .count();
                    if best.as_ref().is_none_or(|(b, _, _)| score < *b) {
                        best = Some((score, found, t));
                    }
                }
                if let Some((_, found, t)) = best {
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
            self.record_asserts(
                node,
                &self.ctx.function_def(loc).sig,
                &sig,
                LowerCx::default(),
            );
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
                m.kind(),
                TyKind::Array { .. }
                    | TyKind::List { .. }
                    | TyKind::Shape(_)
                    | TyKind::Mixed
                    | TyKind::Untyped
            )
        };
        if t.members().iter().any(is_array) {
            return ret;
        }
        Ty::union(ret.members().iter().filter(|m| !is_array(m)).cloned())
    }

    fn call_value(&mut self, node: &SyntaxNode, ct: &Ty, expected: Option<&Ty>) -> Ty {
        if ct.is_union() && ct.members().iter().any(Ty::is_null) {
            let rest = stir_types::without_null(ct);
            let t = self.call_value(node, &rest, expected);
            self.error(
                node.text_range(),
                codes::NULLABLE_ACCESS,
                format!("`{ct}` can't be called when it's null"),
            );
            return t;
        }
        match ct.kind() {
            TyKind::Union(ms) => {
                // callable if each member is; a level 7 problem if only some are
                let start = self.diags.len();
                let mut results = Vec::new();
                let mut any_ok = false;
                let mut all_ok = true;
                for m in ms.iter() {
                    let before = self.diags.len();
                    results.push(self.call_value(node, m, expected));
                    let bad = self.diags[before..]
                        .iter()
                        .any(|d| d.code == codes::NOT_CALLABLE);
                    any_ok |= !bad;
                    all_ok &= !bad;
                }
                self.diags.truncate(start);
                if !all_ok {
                    self.error(
                        node.text_range(),
                        codes::NOT_CALLABLE,
                        format!("`{ct}` is not callable"),
                    );
                    if any_ok {
                        self.tag_partial_union();
                    }
                }
                return Ty::union(results);
            }
            TyKind::Intersection(ms) => {
                for m in ms.iter() {
                    let before = self.diags.len();
                    let t = self.call_value(node, m, expected);
                    if self.diags.len() == before {
                        return t;
                    }
                    self.diags.truncate(before);
                }
            }
            TyKind::SelfType { class, .. } => {
                return self.call_value(node, &class.clone(), expected);
            }
            TyKind::Param { .. } => {
                let b = self.bound_of(ct);
                if !matches!(b.kind(), TyKind::Param { .. }) {
                    return self.call_value(node, &b, expected);
                }
                self.check_args_untyped(node);
                return Ty::untyped();
            }
            // it's callable if it has `__invoke`, which only the value knows
            TyKind::Object => {
                self.error(
                    node.text_range(),
                    codes::NOT_CALLABLE,
                    format!("`{ct}` is not callable"),
                );
                self.tag_level(7);
                self.check_args_untyped(node);
                return Ty::untyped();
            }
            _ => {}
        }
        let fn_ty = match ct.kind() {
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
                            documented: false,
                            default: None,
                            out: None,
                            closure_this: None,
                        })
                        .collect(),
                    ret: f.ret.clone(),
                    ret_declared: true,
                    ret_documented: false,
                    external: false,
                    conditional_on: Vec::new(),
                };
                self.check_args(node, &sig, &FxHashMap::default(), expected, &[])
            }
            None => {
                if !ct.is_error()
                    && !matches!(ct.kind(), TyKind::Callable { sig: None, .. })
                    && !matches!(ct.kind(), TyKind::Mixed)
                    && !ct.is_untyped()
                    && !php_callable_value(ct)
                    // `Closure` itself takes anything
                    && !matches!(ct.kind(), TyKind::Class { name, .. } if name.eq_ignore_ascii_case("Closure"))
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
                    Ty::untyped()
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
        let nullsafe = nullsafe || in_nullsafe_chain(&base);
        if !nullsafe && self.probing == 0 {
            let what = node
                .child(SyntaxKind::NAME)
                .map(|n| format!("`{}`", text_of(&n)))
                .unwrap_or_else(|| "a member".to_owned());
            if bt.is_null() {
                self.error(
                    node.text_range(),
                    codes::NULLABLE_ACCESS,
                    format!("can't reach {what} on `null`"),
                );
                self.tag_level(2);
                return Some((Ty::error(), false));
            }
            if bt.is_union() && bt.members().iter().any(Ty::is_null) {
                self.error(
                    node.text_range(),
                    codes::NULLABLE_ACCESS,
                    format!("can't reach {what} on `{bt}`: it may be null"),
                );
            }
        }
        Some((stir_types::without_null(&bt), nullsafe && nullable))
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
            return Ty::untyped();
        };
        let name = text_of(&name_node);
        if self.member_is_guarded(node, &name) {
            return Ty::untyped();
        }
        let start = self.diags.len();
        let t = self.member_type(&recv, &name, name_node.text_range());
        self.tag_unless_on_this(node, start);
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

    /// The member of an intersection that has what's looked for: an object of all those types
    /// has the members of any.
    fn intersection_member(&self, ty: &Ty, has: impl Fn(&Self, &Ty) -> bool) -> Option<Ty> {
        match ty.kind() {
            TyKind::Intersection(ms) => {
                let found = ms.iter().find(|m| {
                    let bound = self.bound_of(m);
                    !matches!(bound.kind(), TyKind::Param { .. }) && has(self, &class_of(&bound))
                })?;
                Some(self.bound_of(found))
            }
            _ => None,
        }
    }

    fn member_type(&mut self, recv: &Ty, name: &str, range: TextRange) -> Ty {
        let recv = &self.bound_of(recv);
        let recv = &self
            .intersection_member(recv, |this, m| this.ctx.find_property(m, name).is_some())
            .unwrap_or_else(|| recv.clone());
        // a template without a bound is `mixed`
        if matches!(recv.kind(), TyKind::Param { .. }) {
            return Ty::untyped();
        }
        if recv.is_union() {
            return self.union_member_type(recv, name, range);
        }
        match recv.kind() {
            TyKind::Error => Ty::error(),
            TyKind::Never => Ty::never(),
            TyKind::Class { .. } | TyKind::SelfType { .. } | TyKind::Intersection(_) => {
                let cls = class_of(recv);
                if let Some(t) = self.enum_property(&cls, name) {
                    return t;
                }
                match self.ctx.find_property(&cls, name) {
                    Some(p) => {
                        let class = p.class.clone();
                        self.member_def(range, &class, MemberKind::Property, name);
                        // an inaccessible property goes through `__get`, if there is one
                        if self.has_magic(&cls, "__get") && !self.visible(&p.class, p.visibility) {
                            return Ty::untyped();
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
                        if matches!(cls.kind(), TyKind::Class { name, .. } if name.eq_ignore_ascii_case("stdClass"))
                            || self.probing > 0
                        {
                            return Ty::untyped();
                        }
                        // PHPStan's `reportMagicProperties`: fine at level 0, but from level 1
                        // a property only `__get` knows of has to be declared with `@property`
                        if self.has_magic(&cls, "__get") {
                            self.error(
                                range,
                                codes::UNDEFINED_MEMBER,
                                format!("`{recv}` has no property `{name}`, only `__get`"),
                            );
                            self.tag_level(1);
                            return Ty::untyped();
                        }
                        if self.limited_member(
                            range,
                            &cls,
                            stir_hir::LimitedMemberKind::Property,
                            name,
                        ) {
                            return Ty::error();
                        }
                        if self.class_known(&cls) {
                            self.error(
                                range,
                                codes::UNDEFINED_MEMBER,
                                format!("`{recv}` has no property `{name}`"),
                            );
                            self.note_dynamic_property(range);
                        }
                        Ty::error()
                    }
                }
            }
            TyKind::Mixed | TyKind::Object => Ty::untyped(),
            _ if recv.is_untyped() => Ty::error(),
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

    /// `->name` and, on a backed enum, `->value`.
    fn enum_property(&self, cls: &Ty, name: &str) -> Option<Ty> {
        let TyKind::Class { name: class, .. } = cls.kind() else {
            return None;
        };
        let c = self.ctx.class(class)?;
        if c.kind() != ClassKind::Enum {
            return None;
        }
        match name {
            "name" => Some(Ty::string()),
            "value" => {
                let backing = c.def.enum_backing.as_ref()?;
                Some(
                    self.ctx
                        .lower(backing, &LowerCx::default(), &mut Vec::new()),
                )
            }
            _ => None,
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
        // what a trait has depends on the class using it: this is a trait nobody here uses
        let in_a_trait = matches!(ty.kind(), TyKind::Class { name, .. }
            if self.ctx.class(name).is_some_and(|c| c.kind() == ClassKind::Trait));
        matches!(ty.kind(), TyKind::Class { .. }) && !in_a_trait && self.ctx.hierarchy_known(ty)
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
        // `(function () { ... })->call($object)` and `->bindTo($object, $scope)`
        let base = exprs_of(&fetch).into_iter().next();
        let literal = base.as_ref().is_some_and(|b| {
            let mut b = b.clone();
            while b.kind() == PAREN_EXPR {
                match exprs_of(&b).into_iter().next() {
                    Some(inner) => b = inner,
                    None => break,
                }
            }
            matches!(b.kind(), CLOSURE_EXPR | ARROW_FN_EXPR)
        });
        if literal && let Some(name) = fetch.child(SyntaxKind::NAME) {
            let args = self.args_of(node);
            match text_of(&name).to_ascii_lowercase().as_str() {
                "bindto" => self.prepare_closure_bind(&args, 0, 1),
                "call" => self.prepare_closure_bind(&args, 0, 0),
                _ => {}
            }
        }
        let Some((recv, nullable)) = self.receiver(&fetch) else {
            return Ty::error();
        };
        self.types.insert(fetch.text_range(), recv.clone());
        let Some(name_node) = fetch.child(SyntaxKind::NAME) else {
            self.check_args_untyped(node);
            return Ty::untyped();
        };
        let name = text_of(&name_node);
        if self.member_is_guarded(&fetch, &name) {
            self.check_args_untyped(node);
            return Ty::untyped();
        }
        let start = self.diags.len();
        let t = self.call_member(node, &recv, &name, name_node.text_range(), expected);
        self.tag_unless_on_this(&fetch, start);
        // a method that returns nothing is there for what it changes
        if matches!(t.kind(), TyKind::Void)
            && let Some(key) = exprs_of(&fetch).first().and_then(|b| self.place_key(b))
        {
            self.forget_properties_of(&key);
        }
        if nullable { t.nullable() } else { t }
    }

    /// Did `method_exists()` or `property_exists()` find this member on this object?
    fn member_is_guarded(&self, fetch: &SyntaxNode, name: &str) -> bool {
        let Some(key) = exprs_of(fetch).first().and_then(|b| self.place_key(b)) else {
            return false;
        };
        self.lookup(&format!("?has::{key}::{}", name.to_ascii_lowercase()))
            .is_some()
    }

    /// Below level 2 PHPStan only checks members on `$this` (`checkThisOnly`).
    fn tag_unless_on_this(&mut self, fetch: &SyntaxNode, start: usize) {
        let on_this = exprs_of(fetch)
            .first()
            .is_some_and(|b| b.kind() == VARIABLE_EXPR && text_of(b) == "$this");
        if on_this {
            return;
        }
        for d in &mut self.diags[start..] {
            if d.php_level.is_none_or(|l| l < 2)
                && matches!(d.code, codes::UNDEFINED_MEMBER | codes::ARGUMENT_COUNT)
            {
                d.php_level = Some(2);
            }
        }
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
        let recv = &self
            .intersection_member(recv, |this, m| {
                matches!(m.kind(), TyKind::Class { .. }) && this.ctx.find_method(m, name).is_some()
            })
            .unwrap_or_else(|| recv.clone());
        if recv.is_error() {
            self.check_args_untyped(node);
            return Ty::error();
        }
        // a template without a bound is `mixed`
        if matches!(recv.kind(), TyKind::Param { .. }) {
            self.check_args_untyped(node);
            return Ty::untyped();
        }
        // narrowing ruled this path out, so it can't run
        if matches!(recv.kind(), TyKind::Never) {
            self.check_args_untyped(node);
            return Ty::never();
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
                return Ty::untyped();
            }
            self.check_visibility(
                range,
                &owner,
                m.mods.visibility,
                &format!("method `{name}`"),
            );
            let sig = sig_for_receiver(self.ctx.method_sig(&owner, m), recv);
            // `static` return types resolve to the receiver
            let ret = self.check_args(node, &sig, &map, expected, &[]);
            let cx = self.ctx.class_cx(&owner);
            self.record_asserts(node, &m.sig, &sig, cx);
            return resolve_self(&ret, recv);
        }
        if matches!(cls.kind(), TyKind::Class { .. }) && !self.class_known(&cls) {
            self.check_args_untyped(node);
            return Ty::untyped();
        }
        // PHPStan's `reportMagicMethods`: fine at level 0, but from level 1 a method only
        // `__call` knows of has to be declared with `@method`
        if matches!(cls.kind(), TyKind::Class { .. }) && self.has_magic(&cls, "__call") {
            self.error(
                range,
                codes::UNDEFINED_MEMBER,
                format!("`{recv}` has no method `{name}`, only `__call`"),
            );
            self.tag_level(1);
            self.check_args_untyped(node);
            return Ty::untyped();
        }
        if recv.is_untyped() || matches!(recv.kind(), TyKind::Mixed | TyKind::Object) {
            self.check_args_untyped(node);
            return Ty::untyped();
        }
        let cls = class_of(recv);
        if self.limited_member(range, &cls, stir_hir::LimitedMemberKind::Method, name) {
            self.check_args_untyped(node);
            return Ty::error();
        }
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
        if let TyKind::Intersection(ms) = ty.kind() {
            return ms.iter().any(|m| self.responds_to(m, name));
        }
        if matches!(ty.kind(), TyKind::Param { .. }) {
            let b = self.bound_of(ty);
            return matches!(b.kind(), TyKind::Param { .. }) || self.responds_to(&b, name);
        }
        let cls = class_of(ty);
        matches!(cls.kind(), TyKind::Class { .. })
            && (self.ctx.find_method(&cls, name).is_some()
                || self.has_magic(&cls, "__call")
                || !self.class_known(&cls))
            || matches!(ty.kind(), TyKind::Mixed | TyKind::Object | TyKind::Error)
            || ty.is_untyped()
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
                        // a trait's parent is that of the class using it
                        None if c.kind() == ClassKind::Trait => Ty::error(),
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
                if self.lookup(&format!("?class::{}", fqn.key())).is_none()
                    && !self.only_names_a_class(node)
                {
                    self.report_going_away(
                        node.text_range(),
                        &format!("`{}`", c.name),
                        c.file,
                        &self.index().limited_classes,
                        c.key.as_str(),
                    );
                }
                Ty::class(
                    c.name.clone(),
                    c.params.iter().map(|p| p.ty.clone()).collect(),
                )
            }
            None => {
                // `class_exists()` said it's there
                if self.lookup(&format!("?class::{}", fqn.key())).is_some() {
                    return Ty::error();
                }
                if let Some(&available) = self.index().limited_classes.get(fqn.key().as_str()) {
                    if !self.only_names_a_class(node) {
                        let what = format!("`{}`", fqn.as_str());
                        self.report_limited(node.text_range(), &what, available);
                    }
                    return Ty::error();
                }
                self.diags.push(
                    Diagnostic::error(
                        self.path.clone(),
                        node.text_range(),
                        codes::UNDEFINED_CLASS,
                        format!("unknown class `{text}`"),
                    )
                    .with_symbol(fqn.as_str()),
                );
                Ty::error()
            }
        }
    }

    /// `catch (X $e)` and `$a instanceof X`: PHP doesn't load the class, and code for several
    /// versions of PHP can test for what only some have.
    fn only_names_a_class(&self, node: &SyntaxNode) -> bool {
        node.parent()
            .is_some_and(|p| matches!(p.kind(), CATCH_CLAUSE | INSTANCEOF_EXPR))
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
            self.expr(&base, None)
        }
    }

    /// `Closure::bind(function () { ... }, $object, Foo::class)`
    fn prepare_closure_bind(&mut self, args: &[SyntaxNode], this_arg: usize, scope_arg: usize) {
        let this = args
            .get(this_arg)
            .and_then(|a| exprs_of(a).into_iter().next())
            .map(|e| self.expr(&e, None));
        let scope = args
            .get(scope_arg)
            .and_then(|a| exprs_of(a).into_iter().next())
            .map(|e| self.expr(&e, None));
        let class_of_value = |this: &Self, t: &Ty| {
            let c = match this.named_class(t) {
                Some(c) => c,
                None => class_of(t),
            };
            match class_of(&c).kind() {
                TyKind::Class { name, .. } => this.ctx.class(name),
                _ => None,
            }
        };
        let class = scope.as_ref().and_then(|s| class_of_value(self, s));
        // no scope given: it keeps the class it has
        let class = class.or_else(|| self.class.clone());
        let this_ty = this.filter(|t| !t.is_null() && !t.is_error()).map(|t| {
            if t.is_untyped() || matches!(t.kind(), TyKind::Mixed | TyKind::Object) {
                // an object of the scope's class, if nothing says otherwise
                class.as_ref().map_or_else(Ty::untyped, |c| c.self_ty())
            } else {
                t
            }
        });
        self.closure_bind = Some(ClosureBind {
            this: this_ty,
            class,
        });
    }

    fn static_call(&mut self, node: &SyntaxNode, expected: Option<&Ty>) -> Ty {
        let Some(fetch) = node.children().next() else {
            return Ty::error();
        };
        let cls = self.static_base(&fetch);
        if matches!(cls.kind(), TyKind::Class { name, .. } if name.eq_ignore_ascii_case("Closure"))
            && fetch
                .child(SyntaxKind::NAME)
                .is_some_and(|n| text_of(&n).eq_ignore_ascii_case("bind"))
        {
            let args = self.args_of(node);
            let is_literal = args
                .first()
                .and_then(|a| exprs_of(a).into_iter().next())
                .is_some_and(|e| matches!(e.kind(), CLOSURE_EXPR | ARROW_FN_EXPR));
            if is_literal {
                self.prepare_closure_bind(&args, 1, 2);
            }
        }
        let Some(name_node) = fetch.child(SyntaxKind::NAME) else {
            self.check_args_untyped(node);
            return Ty::untyped();
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
                // `self::`, `static::` and `parent::` keep the class the call came in on
                let forwards = fetch.children().next().is_some_and(|b| {
                    b.kind() == NAME_EXPR
                        && matches!(
                            text_of(&b).to_ascii_lowercase().as_str(),
                            "self" | "static" | "parent"
                        )
                });
                let recv = match &self.class {
                    Some(c) if forwards && c.kind() != ClassKind::Trait => {
                        Ty::static_of(c.self_ty())
                    }
                    _ => cls.clone(),
                };
                let sig = sig_for_receiver(self.ctx.method_sig(&owner, m), &recv);
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
                let cx = self.ctx.class_cx(&owner);
                self.record_asserts(node, &m.sig, &sig, cx);
                resolve_self(&ret, &recv)
            }
            None => {
                if !self.class_known(&cls) {
                    self.check_args_untyped(node);
                    return Ty::untyped();
                }
                if self.limited_member(
                    name_node.text_range(),
                    &cls,
                    stir_hir::LimitedMemberKind::Method,
                    &name,
                ) {
                    self.check_args_untyped(node);
                    return Ty::error();
                }
                if self.has_magic(&cls, "__callStatic") {
                    self.error(
                        name_node.text_range(),
                        codes::UNDEFINED_MEMBER,
                        format!("`{cls}` has no method `{name}`, only `__callStatic`"),
                    );
                    self.tag_level(1);
                    self.check_args_untyped(node);
                    return Ty::untyped();
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
        // `self::$$name`, `self::${$name}`: which one is decided when it runs
        if node.children_with_tokens().any(|e| e.kind() == DOLLAR)
            || node.children().any(|c| c.kind() == VARIABLE_VARIABLE)
        {
            for e in exprs_of(node).iter().skip(1) {
                self.expr(e, None);
            }
            return Ty::untyped();
        }
        let Some(var) = node
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .filter(|t| t.kind() == VARIABLE)
            .last()
        else {
            return Ty::untyped();
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
                if self.limited_member(
                    var.text_range(),
                    &cls,
                    stir_hir::LimitedMemberKind::Property,
                    name,
                ) {
                    return Ty::error();
                }
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
            return Ty::untyped();
        };
        let name = text_of(&name_node);
        if name.eq_ignore_ascii_case("class") {
            let base = node.children().next();
            // `Foo::class` is just the name: no such class needs to exist
            let found = self.diags.len();
            let of = if let Some(b) = &base
                && b.kind() != NAME_EXPR
            {
                self.expr(b, None)
            } else {
                let t = self.static_base(node);
                self.diags.truncate(found);
                t
            };
            return Ty::class_string(
                matches!(class_of(&of).kind(), TyKind::Class { .. }).then_some(of),
            );
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
                if self.limited_member(
                    name_node.text_range(),
                    &cls,
                    stir_hir::LimitedMemberKind::Const,
                    &name,
                ) {
                    return Ty::error();
                }
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

    /// `new class(...) { ... }`: checks it, and it's an object of that class.
    fn anonymous_class(&mut self, anon: &SyntaxNode) -> Ty {
        let ptr = stir_hir::NodePtr::new(anon);
        let Some(def) = self.items.classes.iter().find(|c| c.ptr == ptr) else {
            self.check_args_untyped(anon);
            return Ty::object();
        };
        let Some(model) = self.ctx.class(def.name.as_str()) else {
            self.check_args_untyped(anon);
            return Ty::object();
        };
        match self.ctx.find_method(&model.self_ty(), "__construct") {
            Some((owner, m, map)) => {
                let sig = self.ctx.method_sig(&owner, m);
                self.check_args(anon, &sig, &map, None, &[]);
            }
            None => self.check_args_untyped(anon),
        }
        // the class isn't reachable from a statement of its own
        let saved_ctor = std::mem::replace(&mut self.in_ctor, false);
        let saved_prepass = self.prepass;
        self.class_decl(anon);
        self.prepass = saved_prepass;
        self.in_ctor = saved_ctor;
        model.self_ty()
    }

    fn new_expr_inner(&mut self, node: &SyntaxNode, expected: Option<&Ty>) -> Ty {
        if let Some(anon) = node.child(ANON_CLASS) {
            return self.anonymous_class(&anon);
        }
        let Some(target) = node.children().find(|c| c.kind() != ARG_LIST) else {
            return Ty::error();
        };
        let cls = if target.kind() == NAME_EXPR {
            let t = text_of(&target);
            self.class_ref_ty(&target, &t)
        } else {
            // the class comes from a value: if its type doesn't say which, an object of some class
            let t = self.expr(&target, None);
            self.check_args_untyped(node);
            return self.named_class(&t).unwrap_or_else(Ty::object);
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
            // `new self` in a trait is the class using it
            ClassKind::Trait if self.class.as_ref().is_some_and(|c| c.key == model.key) => {}
            ClassKind::Interface | ClassKind::Trait | ClassKind::Enum => {
                self.error(
                    target.text_range(),
                    codes::INSTANTIATION,
                    format!("`{}` can't be instantiated", model.name),
                );
            }
            // `new static` makes whichever concrete class it's called on
            _ if model.def.mods.is_abstract && !text_of(&target).eq_ignore_ascii_case("static") => {
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
        let exp_args = expected.and_then(|e| match e.kind() {
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
        let exp_fn = expected.and_then(|e| match e.kind() {
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
                                // whatever the outer code puts there, even the closure itself;
                                // and with the reference it exists out there from now on
                                let local = Local {
                                    ty: Ty::untyped(),
                                    declared: Ty::untyped(),
                                    kind: LocalKind::Variable,
                                    range: t.text_range(),
                                    maybe_undefined: false,
                                };
                                frame.insert(SmolStr::new(n), local.clone());
                                if let Some(f) = self.frames.first_mut() {
                                    f.insert(SmolStr::new(n), local);
                                }
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
        // `Closure::bind()` and `@param-closure-this` say what `$this` and the class are in it
        let bound = self.closure_bind.take();
        let is_static = node.has_token(STATIC_KW);
        let this = match &bound {
            Some(b) => b.this.clone().map(|t| Local {
                ty: t.clone(),
                declared: t,
                kind: LocalKind::This,
                range: node.text_range(),
                maybe_undefined: false,
            }),
            None if is_static => None,
            None => self.lookup("this").cloned(),
        };
        if let Some(this) = this {
            frame.insert(SmolStr::new("this"), this);
        }
        let saved_class = bound
            .as_ref()
            .and_then(|b| b.class.clone())
            .map(|c| self.class.replace(c));
        let saved = std::mem::replace(&mut self.frames, vec![frame]);
        let saved_dead = std::mem::replace(&mut self.dead, false);
        let saved_loops = std::mem::take(&mut self.loops);
        let mut params = Vec::new();
        if let Some(pl) = node.child(PARAM_LIST) {
            for (i, p) in pl.children_of(PARAM).iter().enumerate() {
                let expected_param = exp_fn
                    .as_ref()
                    .and_then(|f| f.params.get(i).map(|p| p.ty.clone()))
                    .filter(|t| {
                        // a template of the call itself that nothing has solved says nothing
                        stir_types::free_params(t).iter().all(|id| {
                            self.lower_cx.generics.iter().any(
                                |(_, g)| matches!(g.kind(), TyKind::Param { id: g, .. } if g == id),
                            )
                        })
                    });
                // what the callback is called with, if that says more than its own type
                let ty = match (stir_hir::first_type(p), expected_param) {
                    (Some(t), Some(e)) => {
                        let declared = self.lower_type_node(&t);
                        if !e.is_error()
                            && !matches!(e.kind(), TyKind::Mixed | TyKind::Untyped)
                            && self.assignable(&e, &declared)
                        {
                            e
                        } else {
                            declared
                        }
                    }
                    (Some(t), None) => self.lower_type_node(&t),
                    (None, Some(e)) => e,
                    (None, None) => Ty::untyped(),
                };
                let name = match p.token(VARIABLE) {
                    Some(v) => v.text()[1..].to_owned(),
                    None => p.name_text().unwrap_or_default(),
                };
                let variadic = p.has_token(ELLIPSIS);
                let local = if variadic {
                    Ty::list(ty.clone())
                } else {
                    ty.clone()
                };
                self.declare(&name, local, LocalKind::Param, p.text_range());
                params.push(if variadic {
                    stir_types::FnParam::variadic(ty)
                } else if p.has_token(EQ) {
                    stir_types::FnParam::optional(ty)
                } else {
                    stir_types::FnParam::required(ty)
                });
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
            let ret = declared_ret.clone().unwrap_or_else(Ty::void);
            self.fns.push(FnCx {
                ret: ret.clone(),
                infer_ret: infer,
                returns: Vec::new(),
            });
            if let Some(b) = node.child(BLOCK) {
                self.block_stmts(&b);
            }
            let fcx = self.fns.pop().unwrap();
            if infer && is_generator(node) {
                Ty::class("Generator", Vec::new())
            } else if infer {
                fcx.returns
                    .into_iter()
                    .reduce(|a, b| stir_types::join(self.ctx, &a, &b))
                    .unwrap_or_else(Ty::void)
            } else {
                ret
            }
        };
        // what it left in what it holds by reference
        let by_ref: Vec<(SmolStr, Ty)> = node
            .child(CLOSURE_USE)
            .iter()
            .flat_map(|u| u.children_of(CLOSURE_USE_VAR))
            .filter(|v| v.has_token(AMP))
            .filter_map(|v| {
                let t = v.token(VARIABLE)?;
                let n = SmolStr::new(&t.text()[1..]);
                let ty = self.lookup(&n)?.ty.clone();
                Some((n, ty))
            })
            .collect();
        self.frames = saved;
        for (n, t) in by_ref {
            if let Some(l) = self.lookup_mut(&n) {
                // it may not run, and if it's still unknown it's unknown
                let before = if l.ty.is_untyped() {
                    Ty::null()
                } else {
                    l.ty.clone()
                };
                let joined = if t.is_untyped() {
                    Ty::untyped()
                } else {
                    Ty::union([before, t])
                };
                l.ty = joined.clone();
                l.declared = joined;
            }
        }
        self.dead = saved_dead;
        self.loops = saved_loops;
        if let Some(c) = saved_class {
            self.class = c;
        }
        Ty::closure(Some(stir_types::FnTy::new(params, ret)))
    }

    // ------------------------------------------------------------ match

    fn match_expr(&mut self, node: &SyntaxNode, expected: Option<&Ty>) -> Ty {
        let subject = exprs_of(node).into_iter().next();
        if let Some(subject) = &subject {
            self.expr(subject, None);
        }
        let by_true = subject.as_ref().is_some_and(is_true_literal);
        let mut result: Option<Ty> = None;
        let mut ends = Vec::new();
        for arm in node.children_of(MATCH_ARM) {
            // the arms are tried in turn, so each knows the ones before didn't match
            let before_arm = self.snapshot();
            let mut when_true: Option<Facts> = None;
            if let Some(conds) = arm.child(MATCH_CONDITIONS) {
                for c in exprs_of(&conds) {
                    self.expr(&c, None);
                    let (t, f) = if by_true {
                        self.facts(&c)
                    } else {
                        subject
                            .as_ref()
                            .map(|s| self.literal_facts(s, &c, true))
                            .unwrap_or_default()
                    };
                    when_true = Some(match when_true {
                        Some(prev) => self.either(prev, t),
                        None => t,
                    });
                    self.apply_facts(&f);
                }
            }
            let after_conds = self.snapshot();
            if let Some(t) = &when_true {
                self.restore_narrowing(&before_arm);
                self.apply_facts(t);
            }
            if let Some(body) = exprs_of(&arm).last() {
                let dead = self.dead;
                let t = self.expr(body, expected);
                if !t.is_never() {
                    ends.push(self.snapshot());
                }
                self.dead = dead;
                result = Some(match result {
                    None => t,
                    Some(r) => stir_types::join(self.ctx, &r, &t),
                });
            }
            self.restore(&after_conds);
        }
        if let Some(state) = self.merge_states(ends) {
            self.frames = state;
        }
        result.unwrap_or_else(Ty::never)
    }
}

/// Array literals with more keys than this are treated as arrays of what they hold.
const MAX_SHAPE_FIELDS: usize = 64;

/// The shape key a literal stands for.
fn shape_key_of(t: &Ty) -> Option<stir_types::ShapeKey> {
    match t.kind() {
        TyKind::Literal(LitValue::Int(n)) => Some(stir_types::ShapeKey::Int(*n)),
        TyKind::Literal(LitValue::String(s)) => Some(stir_types::ShapeKey::from_str_key(s)),
        _ => None,
    }
}

fn set_shape_field(fields: &mut Vec<stir_types::ShapeField>, key: stir_types::ShapeKey, ty: Ty) {
    match fields.iter_mut().find(|f| f.key == key) {
        Some(f) => {
            f.ty = ty;
            f.optional = false;
        }
        None => fields.push(stir_types::ShapeField::new(key, ty)),
    }
}

fn is_true_literal(node: &SyntaxNode) -> bool {
    node.kind() == NAME_EXPR && text_of(node).eq_ignore_ascii_case("true")
}

fn is_null_literal(node: &SyntaxNode) -> bool {
    node.kind() == NAME_EXPR && text_of(node).eq_ignore_ascii_case("null")
}

/// Is this inside a chain after `?->`? A null there skips the rest of the chain.
fn in_nullsafe_chain(node: &SyntaxNode) -> bool {
    match node.kind() {
        NULLSAFE_PROPERTY_FETCH_EXPR | NULLSAFE_METHOD_CALL_EXPR => true,
        PROPERTY_FETCH_EXPR | METHOD_CALL_EXPR | INDEX_EXPR | CALL_EXPR | PAREN_EXPR => node
            .children()
            .next()
            .is_some_and(|c| in_nullsafe_chain(&c)),
        _ => false,
    }
}

/// The class type to look members up on (`static`/`$this` → their class).
fn class_of(ty: &Ty) -> Ty {
    match ty.kind() {
        TyKind::SelfType { class, .. } => class.clone(),
        TyKind::Callable { closure: true, .. } => Ty::class("Closure", Vec::new()),
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
    if matches!(ret.kind(), TyKind::SelfType { .. }) {
        return recv.clone();
    }
    // inside another type `$this` is no more than `static`: `Builder<static>`
    let nested = match recv.kind() {
        TyKind::SelfType { class, .. } => Ty::static_of(class.clone()),
        _ => recv.clone(),
    };
    ret.transform(&mut |t| match t.kind() {
        TyKind::SelfType { .. } => Some(nested.clone()),
        _ => None,
    })
}

/// `sig` as seen through a receiver: what `static` in its parameters stands for.
fn sig_for_receiver(mut sig: FnSig, recv: &Ty) -> FnSig {
    for p in &mut sig.params {
        p.ty = resolve_self(&p.ty, recv);
    }
    sig
}

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
            TyKind::String
                | TyKind::NonEmptyString
                | TyKind::ClassString(_)
                | TyKind::List { .. }
                | TyKind::Array { .. }
                | TyKind::Shape(_)
                | TyKind::Callable { .. }
        ) || matches!(m.kind(), TyKind::Literal(LitValue::String(_)))
    })
}

fn is_php_scalar(ty: &Ty) -> bool {
    matches!(
        ty.kind(),
        TyKind::Int
            | TyKind::Float
            | TyKind::String
            | TyKind::Bool
            | TyKind::Numeric
            | TyKind::ArrayKey
            | TyKind::NonEmptyString
            | TyKind::NumericString
            | TyKind::IntRange { .. }
    ) || matches!(ty.kind(), TyKind::Literal(_))
}

/// A scalar coerces in a non-strict call, unless it's a literal that isn't a number
/// going to a parameter that doesn't take strings (a `TypeError` at runtime).
fn coerces_to(ty: &Ty, want: &Ty) -> bool {
    if let TyKind::Literal(LitValue::String(v)) = ty.kind()
        && v.trim().parse::<f64>().is_err()
    {
        return want
            .members()
            .iter()
            .any(|m| matches!(m.kind(), TyKind::String));
    }
    is_php_scalar(ty)
}

/// Does the type have a member a scalar can be coerced to? Literal types don't qualify.
fn accepts_php_scalars(ty: &Ty) -> bool {
    ty.members().iter().any(|m| {
        matches!(
            m.kind(),
            TyKind::Int | TyKind::Float | TyKind::String | TyKind::Bool
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

/// After assigning `value` to a variable declared `declared`, its current type.
fn narrow_assigned(env: &dyn stir_types::TypeEnv, declared: &Ty, value: &Ty) -> Ty {
    if value.is_error() || declared.is_error() {
        return declared.clone();
    }
    // an array variable stays the array it was declared
    if matches!(declared.kind(), TyKind::Array { .. } | TyKind::List { .. }) {
        return declared.clone();
    }
    // keep the declared type unless the value is a strictly narrower non-literal
    if matches!(value.kind(), TyKind::Null) {
        return Ty::null();
    }
    let w = widen_literals(value);
    if stir_types::is_assignable(env, &w, declared) {
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
