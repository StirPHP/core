//! Rules about what a body does as a whole, not about one value going somewhere: code that can't
//! run, a function that doesn't return, conditions that can only go one way, and types left out.
//! Their meaning, levels and identifiers are those of PHPStan's rules of the same name.

use super::*;
use stir_db::Severity;
use stir_hir::type_ref::{PhpBuiltin, TypeRef, TypeRefKind};

/// What can be said of a value being `===` to another, by what it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValueKind<'t> {
    Null,
    Bool(Option<bool>),
    Int(Option<i64>),
    Float,
    Str(Option<&'t str>),
    Array,
    Object,
    /// Could be anything as far as this goes.
    Unknown,
}

fn value_kind(ty: &Ty) -> ValueKind<'_> {
    use ValueKind as V;
    match ty.kind() {
        TyKind::Null => V::Null,
        TyKind::Bool => V::Bool(None),
        TyKind::Int | TyKind::IntRange { .. } => V::Int(None),
        TyKind::Float => V::Float,
        TyKind::String | TyKind::NonEmptyString | TyKind::NumericString => V::Str(None),
        TyKind::Literal(LitValue::Bool(b)) => V::Bool(Some(*b)),
        TyKind::Literal(LitValue::Int(i)) => V::Int(Some(*i)),
        TyKind::Literal(LitValue::Float(_)) => V::Float,
        TyKind::Literal(LitValue::String(s)) => V::Str(Some(s)),
        TyKind::Array { .. } | TyKind::List { .. } | TyKind::Shape(_) => V::Array,
        TyKind::Class { .. } | TyKind::Object | TyKind::SelfType { .. } => V::Object,
        _ => V::Unknown,
    }
}

/// Can no value of `a` be `===` to one of `b`?
fn never_identical(a: &Ty, b: &Ty) -> bool {
    use ValueKind as V;
    a.members().iter().all(|x| {
        b.members()
            .iter()
            .all(|y| match (value_kind(x), value_kind(y)) {
                (V::Unknown, _) | (_, V::Unknown) => false,
                (V::Bool(Some(p)), V::Bool(Some(q))) => p != q,
                (V::Int(Some(p)), V::Int(Some(q))) => p != q,
                (V::Str(Some(p)), V::Str(Some(q))) => p != q,
                (p, q) => std::mem::discriminant(&p) != std::mem::discriminant(&q),
            })
    })
}

/// Are both the one value that's `===` to the other?
fn always_identical(a: &Ty, b: &Ty) -> bool {
    use ValueKind as V;
    match (value_kind(a), value_kind(b)) {
        (V::Null, V::Null) => !a.is_union() && !b.is_union(),
        (V::Bool(Some(p)), V::Bool(Some(q))) => p == q,
        (V::Int(Some(p)), V::Int(Some(q))) => p == q,
        (V::Str(Some(p)), V::Str(Some(q))) => p == q,
        _ => false,
    }
}

/// What a value of `ty` is as a condition, if it can only be one thing.
pub(super) fn constant_truth(ty: &Ty) -> Option<bool> {
    let mut members = ty.members().iter().map(member_truth);
    let first = members.next()??;
    members.all(|m| m == Some(first)).then_some(first)
}

fn member_truth(ty: &Ty) -> Option<bool> {
    match ty.kind() {
        TyKind::Null | TyKind::Void => Some(false),
        TyKind::Literal(LitValue::Bool(b)) => Some(*b),
        TyKind::Literal(LitValue::Int(i)) => Some(*i != 0),
        TyKind::Literal(LitValue::String(s)) => Some(!s.is_empty() && s != "0"),
        // an empty one of these is falsy
        TyKind::Class { name, .. } if name.eq_ignore_ascii_case("SimpleXMLElement") => None,
        TyKind::Class { .. } | TyKind::SelfType { .. } | TyKind::Object => Some(true),
        // what's known of an array is known of its literal and what's been put in on this path: not
        // enough to say what it'll be by the time it's looked at
        _ => None,
    }
}

/// Are these checked by a rule of their own, as PHPStan's `ConstantConditionRuleHelper` says?
fn has_its_own_rule(node: &SyntaxNode) -> bool {
    match node.kind() {
        INSTANCEOF_EXPR | TERNARY_EXPR | ISSET_EXPR | EMPTY_EXPR => true,
        PREFIX_EXPR => op_token(node).is_some_and(|t| t.kind() == BANG),
        BIN_EXPR => op_token(node).is_some_and(|t| {
            matches!(
                t.kind(),
                EQ3 | NEQ2 | AMP2 | PIPE2 | L_ANGLE | R_ANGLE | LTEQ | GTEQ
            )
        }),
        _ => false,
    }
}

fn true_or_false(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

fn title_truth(value: bool) -> &'static str {
    if value { "True" } else { "False" }
}

fn strip_parens(node: &SyntaxNode) -> SyntaxNode {
    let mut node = node.clone();
    while node.kind() == PAREN_EXPR {
        match exprs_of(&node).into_iter().next() {
            Some(inner) => node = inner,
            None => break,
        }
    }
    node
}

impl<'a, 'db> Checker<'a, 'db> {
    fn report_flow(&mut self, range: TextRange, code: String, message: String) {
        let mut d = Diagnostic::custom(self.path.clone(), range, Severity::Error, code, message);
        d.php_level = Some(4);
        self.diags.push(d);
    }

    /// Is what's found here worth reporting at all? Code that can't run has been reported, and
    /// what's in a trait depends on the class using it.
    fn condition_rules_apply(&self) -> bool {
        // a project that doesn't take PHPDoc at its word, which types here can't tell from native
        // ones, gets none of these
        !self.dead
            && !self.in_trait_member
            && self
                .ctx
                .ws
                .settings(self.ctx.db)
                .treat_phpdoc_types_as_certain
    }

    /// The type of `node`, if `expr` has seen it.
    fn seen_type(&self, node: &SyntaxNode) -> Option<Ty> {
        self.types.get(&node.text_range()).cloned()
    }

    /// `if ($x)`, where `$x` can only be one thing. `last` is the last of a chain of conditions:
    /// there, always being true is the point.
    pub(super) fn check_constant_condition(
        &mut self,
        cond: &SyntaxNode,
        prefix: &str,
        label: &str,
        last: bool,
    ) {
        if !self.condition_rules_apply() {
            return;
        }
        let inner = strip_parens(cond);
        if has_its_own_rule(&inner) {
            return;
        }
        if self.is_library_call(&inner) {
            return;
        }
        let Some(truth) = self.seen_type(&inner).as_ref().and_then(constant_truth) else {
            return;
        };
        if truth && last {
            return;
        }
        self.report_flow(
            cond.text_range(),
            format!("{prefix}.always{}", title_truth(truth)),
            format!("{label} is always {}.", true_or_false(truth)),
        );
    }

    /// Can `cond` only be true, as far as PHPStan would say?
    pub(super) fn condition_is_always_true(&self, cond: &SyntaxNode) -> bool {
        let inner = strip_parens(cond);
        !has_its_own_rule(&inner)
            && !self.is_library_call(&inner)
            && self
                .ctx
                .ws
                .settings(self.ctx.db)
                .treat_phpdoc_types_as_certain
            && self.seen_type(&inner).as_ref().and_then(constant_truth) == Some(true)
    }

    /// `!$x`, where `$x` can only be one thing.
    pub(super) fn check_negation(&mut self, node: &SyntaxNode, inner: &SyntaxNode) {
        if !self.condition_rules_apply() {
            return;
        }
        let inner = strip_parens(inner);
        if has_its_own_rule(&inner) {
            return;
        }
        let Some(truth) = self.seen_type(&inner).as_ref().and_then(constant_truth) else {
            return;
        };
        let negated = !truth;
        self.report_flow(
            node.text_range(),
            format!("booleanNot.always{}", title_truth(negated)),
            format!(
                "Negated boolean expression is always {}.",
                true_or_false(negated)
            ),
        );
    }

    /// `$a && $b`, where a side can only be one thing.
    pub(super) fn check_logical_side(
        &mut self,
        node: &SyntaxNode,
        op: SyntaxKind,
        side: &SyntaxNode,
        left: bool,
    ) {
        if !self.condition_rules_apply() {
            return;
        }
        let inner = strip_parens(side);
        if has_its_own_rule(&inner) {
            return;
        }
        let Some(truth) = self.seen_type(&inner).as_ref().and_then(constant_truth) else {
            return;
        };
        let (id, sigil) = match op {
            AMP2 => ("booleanAnd", "&&"),
            AND_KW => ("logicalAnd", "and"),
            PIPE2 => ("booleanOr", "||"),
            _ => ("logicalOr", "or"),
        };
        let (place, title) = if left {
            ("Left", "left")
        } else {
            ("Right", "right")
        };
        let _ = node;
        self.report_flow(
            side.text_range(),
            format!("{id}.{title}Always{}", title_truth(truth)),
            format!(
                "{place} side of {sigil} is always {}.",
                true_or_false(truth)
            ),
        );
    }

    /// `$a === $b` where that can only go one way. What PHP's library says is taken loosely:
    /// a call of it can be `false` when that isn't declared.
    pub(super) fn check_identical(&mut self, node: &SyntaxNode, op: SyntaxKind, l: &Ty, r: &Ty) {
        if !self.condition_rules_apply() {
            return;
        }
        let es = exprs_of(node);
        if es.iter().any(|e| self.is_library_call(e)) {
            return;
        }
        // a property PHP doesn't check the type of can be not yet what its PHPDoc says
        let against_null = l.is_null() || r.is_null();
        if against_null && es.iter().any(|e| self.is_loose_property(e)) {
            return;
        }
        let always = if never_identical(l, r) {
            false
        } else if always_identical(l, r) {
            true
        } else {
            return;
        };
        let (identical, sigil) = if op == EQ3 {
            ("identical", "===")
        } else {
            ("notIdentical", "!==")
        };
        let value = if op == EQ3 { always } else { !always };
        if value && self.last_condition == Some(node.text_range()) {
            return;
        }
        self.report_flow(
            node.text_range(),
            format!("{identical}.always{}", title_truth(value)),
            format!(
                "Strict comparison using {sigil} between {l} and {r} will always evaluate to {}.",
                true_or_false(value)
            ),
        );
    }

    /// Is `node` a property without a native type?
    fn is_loose_property(&self, node: &SyntaxNode) -> bool {
        let node = strip_parens(node);
        if !matches!(
            node.kind(),
            PROPERTY_FETCH_EXPR | NULLSAFE_PROPERTY_FETCH_EXPR | STATIC_PROPERTY_FETCH_EXPR
        ) {
            return false;
        }
        let Some(name) = node.child(SyntaxKind::NAME) else {
            return true;
        };
        let Some(Def::Member { class, name, .. }) = self.defs.get(&name.text_range()) else {
            return true;
        };
        self.ctx.class(class).is_none_or(|c| {
            c.def
                .properties
                .iter()
                .find(|p| &p.name == name)
                .is_none_or(|p| p.ty.is_none())
        })
    }

    /// Is `node` a call of something the stubs declare?
    fn is_library_call(&self, node: &SyntaxNode) -> bool {
        let mut node = strip_parens(node);
        if node.kind() == ASSIGN_EXPR
            && let Some(value) = exprs_of(&node).into_iter().last()
        {
            node = strip_parens(&value);
        }
        let name = match node.kind() {
            CALL_EXPR => node.children().find(|c| c.kind() == NAME_EXPR),
            METHOD_CALL_EXPR | NULLSAFE_METHOD_CALL_EXPR | STATIC_CALL_EXPR => node
                .children()
                .next()
                .and_then(|fetch| fetch.child(SyntaxKind::NAME)),
            _ => None,
        };
        let Some(name) = name else {
            return false;
        };
        match self.defs.get(&name.text_range()) {
            Some(Def::Function(loc)) => loc.file.origin(self.ctx.db) == FileOrigin::Stub,
            Some(Def::Member { class, .. }) => self
                .ctx
                .class(class)
                .is_some_and(|c| c.origin == FileOrigin::Stub),
            _ => false,
        }
    }

    /// The statements of a list, saying so of the first that nothing can reach.
    pub(super) fn run_list(&mut self, stmts: impl Iterator<Item = SyntaxNode>) {
        let entered_dead = self.dead;
        let mut told = entered_dead;
        for s in stmts {
            if self.dead && !told && !hoisted_or_empty(s.kind()) {
                told = true;
                self.report_flow(
                    without_leading_comments(&s),
                    "deadCode.unreachable".to_owned(),
                    "Unreachable statement - code above always terminates.".to_owned(),
                );
            }
            self.stmt(&s);
        }
    }

    /// The end of a function's body can be reached: is that all right, for what it returns?
    /// `what` names it in a message. A closure's PHPDoc isn't read, and it's only about its native
    /// type.
    pub(super) fn check_missing_return(
        &mut self,
        node: &SyntaxNode,
        what: &str,
        ret: &Ty,
        declared: bool,
    ) {
        if self.dead || !declared || ret.is_error() {
            return;
        }
        let native = node.child(RETURN_TYPE).is_some();
        let block = node.child(BLOCK);
        let Some(block) = block else { return };
        let at = end_of_body(node, &block);
        // `void` is fine, and so is anything that has `void` in it
        if ret
            .members()
            .iter()
            .any(|m| matches!(m.kind(), TyKind::Void))
        {
            return;
        }
        if is_generator(node) {
            // a generator doesn't return what it yields, but what its fourth type argument says
            let TyKind::Class { name, args } = ret.kind() else {
                return;
            };
            let returned = args
                .get(3)
                .filter(|_| name.eq_ignore_ascii_case("Generator"));
            let Some(returned) = returned else { return };
            if matches!(
                returned.kind(),
                TyKind::Void | TyKind::Mixed | TyKind::Untyped | TyKind::Error
            ) {
                return;
            }
            let returned = returned.clone();
            self.push_missing_return(at, what, &returned, Some(2));
            return;
        }
        if ret.is_never() {
            let mut d = Diagnostic::custom(
                self.path.clone(),
                at,
                Severity::Error,
                "return.never",
                format!(
                    "{what} should always throw an exception or terminate script execution but doesn't do that."
                ),
            );
            d.php_level = Some(0);
            self.diags.push(d);
            return;
        }
        let level = if native {
            0
        } else if matches!(ret.kind(), TyKind::Mixed) {
            // PHPStan's `checkExplicitMixedMissingReturn`, on at the level of explicit `mixed`
            9
        } else {
            // `checkPhpDocMissingReturn`
            2
        };
        if matches!(ret.kind(), TyKind::Untyped) {
            return;
        }
        self.push_missing_return(at, what, ret, Some(level));
    }

    fn push_missing_return(&mut self, at: TextRange, what: &str, ret: &Ty, level: Option<u8>) {
        let mut d = Diagnostic::custom(
            self.path.clone(),
            at,
            Severity::Error,
            codes::RETURN_MISSING,
            format!("{what} should return {ret} but return statement is missing."),
        );
        d.php_level = level;
        self.diags.push(d);
    }

    /// `new static()` where a subclass's constructor could want something else.
    pub(super) fn check_new_static(&mut self, node: &SyntaxNode) {
        let Some(class) = self.class.clone() else {
            return;
        };
        if class.is_final() || self.has_consistent_constructor(&class) {
            return;
        }
        let self_ty = class.self_ty();
        // a constructor an interface or something abstract promises can't change in a subclass
        for anc in self.ctx.ancestors(&self_ty).iter() {
            let TyKind::Class { name, .. } = anc.kind() else {
                continue;
            };
            let Some(a) = self.ctx.class(name) else {
                continue;
            };
            let ctor = a
                .def
                .methods
                .iter()
                .find(|m| m.name.eq_ignore_ascii_case("__construct"));
            if let Some(ctor) = ctor
                && (a.kind() == ClassKind::Interface || ctor.mods.is_abstract || ctor.mods.is_final)
            {
                return;
            }
        }
        let mut d = Diagnostic::error(
            self.path.clone(),
            node.text_range(),
            codes::NEW_STATIC,
            "Unsafe usage of new static().",
        );
        d.notes.push(
            "See: https://phpstan.org/blog/solving-phpstan-error-unsafe-usage-of-new-static"
                .to_owned(),
        );
        self.diags.push(d);
    }

    /// `@phpstan-consistent-constructor`, on the class or something above it.
    fn has_consistent_constructor(&self, class: &ClassModel<'db>) -> bool {
        self.ctx.ancestors(&class.self_ty()).iter().any(|anc| {
            let TyKind::Class { name, .. } = anc.kind() else {
                return false;
            };
            self.ctx.class(name).is_some_and(|c| {
                c.def
                    .doc
                    .as_ref()
                    .is_some_and(|d| d.text.contains("-consistent-constructor"))
            })
        })
    }

    fn report_missing_type(&mut self, at: TextRange, code: &'static str, message: String) {
        let mut d = Diagnostic::error(self.path.clone(), at, code, message);
        d.php_level = Some(6);
        self.diags.push(d);
    }

    /// `missingType.parameter`, `.return` and `.iterableValue` for a function or method. `what`
    /// names it in a message.
    pub(super) fn check_missing_signature_types(
        &mut self,
        at: TextRange,
        what: &str,
        name: &str,
        def: &stir_hir::Signature,
        sig: &FnSig,
    ) {
        for (i, p) in sig.params.iter().enumerate() {
            if p.name == "__args" {
                continue;
            }
            let written = def
                .params
                .get(i)
                .is_some_and(|d| d.ty.is_some() || d.doc_ty.is_some());
            if !written && !p.documented {
                self.report_missing_type(
                    at,
                    codes::MISSING_TYPE_PARAMETER,
                    format!("{what} has parameter ${} with no type specified.", p.name),
                );
                continue;
            }
            let Some(own) = def.params.get(i) else {
                continue;
            };
            // what it inherits has been reported where that's written
            if own.doc_ty.is_none() && p.documented {
                continue;
            }
            for iterable in written_iterables(own.doc_ty.as_ref().or(own.ty.as_ref())) {
                self.report_missing_type(
                    at,
                    codes::MISSING_TYPE_ITERABLE_VALUE,
                    format!(
                        "{what} has parameter ${} with no value type specified in {iterable}.",
                        p.name
                    ),
                );
            }
        }
        let lower = name.to_ascii_lowercase();
        // these have the one return type there could be
        if matches!(
            lower.as_str(),
            "__construct"
                | "__destruct"
                | "__unset"
                | "__wakeup"
                | "__clone"
                | "__tostring"
                | "__isset"
                | "__set_state"
        ) && def.ret.is_none()
            && def.doc_ret.is_none()
        {
            return;
        }
        if lower == "__sleep" {
            return;
        }
        if def.ret.is_none() && def.doc_ret.is_none() && !sig.ret_documented {
            self.report_missing_type(
                at,
                codes::MISSING_TYPE_RETURN,
                format!("{what} has no return type specified."),
            );
            return;
        }
        if def.doc_ret.is_none() && sig.ret_documented {
            return;
        }
        for iterable in written_iterables(def.doc_ret.as_ref().or(def.ret.as_ref())) {
            self.report_missing_type(
                at,
                codes::MISSING_TYPE_ITERABLE_VALUE,
                format!("{what} return type has no value type specified in {iterable}."),
            );
        }
    }

    /// `missingType.property` and `.iterableValue` for a property.
    pub(super) fn check_missing_property_type(
        &mut self,
        at: TextRange,
        class: &ClassModel<'db>,
        def: &stir_hir::PropertyDef,
    ) {
        let label = format!("Property {}::${}", class.name, def.name);
        let written = def.doc_ty.as_ref().or(def.ty.as_ref());
        if written.is_none() {
            // it may have what it overrides say what it is
            let inherited = self
                .ctx
                .find_property(&class.self_ty(), &def.name)
                .is_some_and(|p| !matches!(p.ty.kind(), TyKind::Mixed | TyKind::Untyped));
            if !inherited {
                self.report_missing_type(
                    at,
                    codes::MISSING_TYPE_PROPERTY,
                    format!("{label} has no type specified."),
                );
            }
            return;
        }
        for iterable in written_iterables(written) {
            self.report_missing_type(
                at,
                codes::MISSING_TYPE_ITERABLE_VALUE,
                format!("{label} type has no value type specified in {iterable}."),
            );
        }
    }
}

/// `iterable type array`, for each array or iterable as written in `ty` with nothing said of what
/// it holds. As types they're no different from `array<mixed>`, which does say.
fn written_iterables(ty: Option<&TypeRef>) -> Vec<&'static str> {
    fn walk(t: &TypeRef, out: &mut Vec<&'static str>) {
        match &t.kind {
            TypeRefKind::Php(PhpBuiltin::Array) => {
                if !out.contains(&"iterable type array") {
                    out.push("iterable type array");
                }
            }
            TypeRefKind::Php(PhpBuiltin::Iterable) => {
                if !out.contains(&"iterable type iterable") {
                    out.push("iterable type iterable");
                }
            }
            TypeRefKind::Nullable(inner)
            | TypeRefKind::CovariantArg(inner)
            | TypeRefKind::ContravariantArg(inner) => walk(inner, out),
            // with `mixed` in it, it's `mixed`
            TypeRefKind::Union(list)
                if list
                    .iter()
                    .any(|m| matches!(m.kind, TypeRefKind::Php(PhpBuiltin::Mixed))) => {}
            TypeRefKind::Union(list)
            | TypeRefKind::Intersection(list)
            | TypeRefKind::Named(_, list)
            | TypeRefKind::SelfGeneric(_, list) => list.iter().for_each(|m| walk(m, out)),
            TypeRefKind::PhpList { elem, .. } => walk(elem, out),
            TypeRefKind::PhpArray { value, .. } | TypeRefKind::Iterable { value, .. } => {
                walk(value, out);
            }
            TypeRefKind::Shape { fields, .. } => fields.iter().for_each(|f| walk(&f.ty, out)),
            TypeRefKind::Callable { params, ret, .. } => {
                params.iter().for_each(|p| walk(&p.ty, out));
                walk(ret, out);
            }
            TypeRefKind::Conditional {
                then, otherwise, ..
            } => {
                walk(then, out);
                walk(otherwise, out);
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    if let Some(t) = ty {
        walk(t, &mut out);
    }
    out
}

/// Where a body's end is reached from, for saying so where PHPStan does: its last statement, the
/// comment in an empty one, or the function's name.
fn end_of_body(function: &SyntaxNode, block: &SyntaxNode) -> TextRange {
    let last = block.children().filter(|c| is_stmt_kind(c.kind())).last();
    if let Some(last) = last {
        return last_open_statement(&last);
    }
    let comment = block
        .children_with_tokens()
        .filter(|e| matches!(e.kind(), COMMENT | DOC_COMMENT))
        .last();
    match comment {
        Some(c) => c.text_range(),
        None => function
            .child(SyntaxKind::NAME)
            .map_or(function.text_range(), |n| n.text_range()),
    }
}

/// `stmt`, or for an `if` with branches the last statement of the last that doesn't stop.
fn last_open_statement(stmt: &SyntaxNode) -> TextRange {
    let branch_end = |parent: &SyntaxNode| {
        let body = parent.child(BLOCK).unwrap_or_else(|| parent.clone());
        body.children().filter(|c| is_stmt_kind(c.kind())).last()
    };
    let stops = |s: &SyntaxNode| {
        matches!(s.kind(), RETURN_STMT)
            || (s.kind() == EXPR_STMT
                && exprs_of(s)
                    .first()
                    .is_some_and(|e| matches!(e.kind(), THROW_EXPR | EXIT_EXPR)))
    };
    if stmt.kind() == IF_STMT {
        let mut open = None;
        if let Some(end) = branch_end(stmt).filter(|e| !stops(e)) {
            open = Some(end);
        }
        for clause in stmt.children() {
            if matches!(clause.kind(), ELSEIF_CLAUSE | ELSE_CLAUSE)
                && let Some(end) = branch_end(&clause).filter(|e| !stops(e))
            {
                open = Some(end);
            }
        }
        let has_else = stmt.children().any(|c| c.kind() == ELSE_CLAUSE);
        if let Some(end) = open.filter(|_| has_else) {
            return last_open_statement(&end);
        }
    }
    without_leading_comments(stmt)
}

/// A statement's range, from its first token that isn't a comment: where it's on.
fn without_leading_comments(stmt: &SyntaxNode) -> TextRange {
    let first = stmt
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| !t.kind().is_trivia());
    first.map_or(stmt.text_range(), |t| {
        TextRange::new(t.text_range().start(), stmt.text_range().end())
    })
}

/// PHP declares these before running anything, and they're not code that runs.
fn hoisted_or_empty(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        FUNCTION_DECL
            | CLASS_DECL
            | INTERFACE_DECL
            | TRAIT_DECL
            | ENUM_DECL
            | EMPTY_STMT
            | INLINE_HTML_STMT
            | USE_DECL
            | NAMESPACE_DECL
            | CONST_DECL
            | HALT_COMPILER_STMT
            | LABEL_STMT
    )
}
