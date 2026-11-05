//! What depends on the PHP versions a project runs on: which versions a piece of code can see,
//! what the library has on them, and which syntax they can parse.

use stir_db::{
    Availability, Db, Diagnostic, PhpVersion, PhpVersions, Severity, SourceFile, Workspace,
};
use stir_hir::ast::NodeExt;
use stir_syntax::{SyntaxKind, SyntaxKind::*, SyntaxNode, SyntaxToken, TextRange};

use crate::codes;

/// The versions `text` is for: the project's, narrowed by a `// lint >= 8.4` comment on the
/// first line, the convention of PHP Parallel Lint for a file only some versions can parse.
pub fn file_versions(text: &str, project: PhpVersions) -> PhpVersions {
    let mut out = project;
    let Some(rest) = text.lines().next().and_then(|l| l.strip_prefix("<?php")) else {
        return out;
    };
    let Some(rest) = rest.trim_start().strip_prefix("//") else {
        return out;
    };
    let mut words = rest.split_whitespace();
    if !words.next().is_some_and(|w| w.eq_ignore_ascii_case("lint")) {
        return out;
    }
    let (Some(op), Some(version)) = (words.next(), words.next().and_then(PhpVersion::parse)) else {
        return out;
    };
    narrow_by(&mut out, op, version);
    out
}

/// `PHP_VERSION op version` holds.
fn narrow_by(versions: &mut PhpVersions, op: &str, version: PhpVersion) {
    match op {
        ">=" => versions.min = versions.min.max(version),
        ">" => versions.min = versions.min.max(version.next()),
        "<" => versions.max = versions.max.min(previous(version)),
        "<=" => versions.max = versions.max.min(version),
        _ => {}
    }
}

/// The release before `version`, for a condition ruling out everything from it on.
fn previous(version: PhpVersion) -> PhpVersion {
    match (version.major, version.minor) {
        (8, 0) => PhpVersion::new(7, 4),
        (7, 0) => PhpVersion::new(5, 6),
        (major, 0) => PhpVersion::new(major - 1, 0),
        (major, minor) => PhpVersion::new(major, minor - 1),
    }
}

/// The opposite of a comparison.
fn negated(op: &str) -> &'static str {
    match op {
        ">=" => "<",
        ">" => "<=",
        "<" => ">=",
        "<=" => ">",
        _ => "",
    }
}

/// The versions on which `node` can run: `base`, narrowed by the `PHP_VERSION_ID` and
/// `version_compare(PHP_VERSION, ...)` conditions around and before it. `None` if it can't run
/// on any.
pub fn versions_at(node: &SyntaxNode, base: PhpVersions) -> Option<PhpVersions> {
    let mut out = base;
    let mut child = node.clone();
    for parent in node.ancestors().skip(1) {
        match parent.kind() {
            IF_STMT | ELSEIF_CLAUSE => in_if(&parent, &child, &mut out),
            TERNARY_EXPR => {
                let parts: Vec<SyntaxNode> = parent.children().collect();
                if parts.get(1) == Some(&child) {
                    assume(&parts[0], true, &mut out);
                } else if parts.get(2) == Some(&child) {
                    assume(&parts[0], false, &mut out);
                }
            }
            BIN_EXPR => {
                let parts: Vec<SyntaxNode> = parent.children().collect();
                if parts.get(1) == Some(&child) {
                    match operator(&parent).as_deref() {
                        Some("&&" | "and") => assume(&parts[0], true, &mut out),
                        Some("||" | "or") => assume(&parts[0], false, &mut out),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        if is_statement_list(parent.kind()) {
            after_early_exits(&child, &mut out);
        }
        child = parent;
    }
    (out.min <= out.max).then_some(out)
}

fn is_statement_list(kind: SyntaxKind) -> bool {
    matches!(kind, SOURCE_FILE | BLOCK | NAMESPACE_DECL)
}

/// `child` is in a branch of the `if` (or `elseif`) `node`.
fn in_if(node: &SyntaxNode, child: &SyntaxNode, out: &mut PhpVersions) {
    let Some(cond) = node.children().next() else {
        return;
    };
    if child.kind() == ELSE_CLAUSE || child.kind() == ELSEIF_CLAUSE {
        assume(&cond, false, out);
        // the clauses before this one didn't apply either
        for clause in node.children().filter(|c| c.kind() == ELSEIF_CLAUSE) {
            if clause == *child {
                break;
            }
            if let Some(c) = clause.children().next() {
                assume(&c, false, out);
            }
        }
    } else if cond != *child {
        assume(&cond, true, out);
    }
}

/// What the statements before `child`, in its list, say by leaving when their condition holds.
fn after_early_exits(child: &SyntaxNode, out: &mut PhpVersions) {
    let mut before = child.prev_sibling();
    while let Some(stmt) = before {
        if stmt.kind() == IF_STMT
            && !stmt
                .children()
                .any(|c| matches!(c.kind(), ELSE_CLAUSE | ELSEIF_CLAUSE))
            && let Some(cond) = stmt.children().next()
            && stmt.children().nth(1).is_some_and(|body| ends_flow(&body))
        {
            assume(&cond, false, out);
        }
        before = stmt.prev_sibling();
    }
}

/// Does a statement always leave the code that follows it?
fn ends_flow(stmt: &SyntaxNode) -> bool {
    match stmt.kind() {
        BLOCK => stmt.children().last().is_some_and(|s| ends_flow(&s)),
        RETURN_STMT | CONTINUE_STMT | BREAK_STMT => true,
        EXPR_STMT => stmt
            .children()
            .next()
            .is_some_and(|e| matches!(e.kind(), THROW_EXPR | EXIT_EXPR)),
        _ => false,
    }
}

fn operator(node: &SyntaxNode) -> Option<String> {
    let token: SyntaxToken = node
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| !t.kind().is_trivia())?;
    Some(token.text().to_ascii_lowercase())
}

fn is_name(node: &SyntaxNode, name: &str) -> bool {
    node.kind() == NAME_EXPR
        && node
            .text()
            .to_string()
            .trim()
            .trim_start_matches('\\')
            .eq_ignore_ascii_case(name)
}

fn int_literal(node: &SyntaxNode) -> Option<u32> {
    if node.kind() != LITERAL {
        return None;
    }
    node.text().to_string().trim().replace('_', "").parse().ok()
}

/// Narrows `out` given that `cond` is `truth`.
fn assume(cond: &SyntaxNode, truth: bool, out: &mut PhpVersions) {
    match cond.kind() {
        PAREN_EXPR => {
            if let Some(inner) = cond.children().next() {
                assume(&inner, truth, out);
            }
        }
        PREFIX_EXPR if operator(cond).as_deref() == Some("!") => {
            if let Some(inner) = cond.children().next() {
                assume(&inner, !truth, out);
            }
        }
        BIN_EXPR => {
            let parts: Vec<SyntaxNode> = cond.children().collect();
            let [left, right] = parts.as_slice() else {
                return;
            };
            let Some(op) = operator(cond) else { return };
            match op.as_str() {
                "&&" | "and" if truth => {
                    assume(left, true, out);
                    assume(right, true, out);
                }
                "||" | "or" if !truth => {
                    assume(left, false, out);
                    assume(right, false, out);
                }
                _ => compared(left, &op, right, truth, out),
            }
        }
        CALL_EXPR => {
            // `version_compare(PHP_VERSION, '8.1', '>=')`
            if let Some((version, op)) = version_compare(cond)
                && let Some(op) = op
            {
                narrow_by(out, if truth { &op } else { negated(&op) }, version);
            }
        }
        _ => {}
    }
}

/// `PHP_VERSION_ID >= 80100`, `PHP_MAJOR_VERSION >= 8` or `version_compare(...) >= 0`, either way
/// around.
fn compared(left: &SyntaxNode, op: &str, right: &SyntaxNode, truth: bool, out: &mut PhpVersions) {
    let flipped = |op: &str| match op {
        ">=" => "<=".to_owned(),
        ">" => "<".to_owned(),
        "<" => ">".to_owned(),
        "<=" => ">=".to_owned(),
        other => other.to_owned(),
    };
    let (name, op, value) = if int_literal(right).is_some() {
        (left, op.to_owned(), int_literal(right))
    } else {
        (right, flipped(op), int_literal(left))
    };
    let Some(value) = value else { return };
    let op = if truth { op.as_str() } else { negated(&op) };
    if is_name(name, "PHP_VERSION_ID") {
        narrow_by(out, op, PhpVersion::from_id(value));
    } else if is_name(name, "PHP_MAJOR_VERSION") {
        // `> 7` and `<= 7` compare with a whole major version
        let major = PhpVersion::new(value as u8, 0);
        match op {
            ">=" | "<" => narrow_by(out, op, major),
            ">" => narrow_by(out, ">=", PhpVersion::new(value as u8 + 1, 0)),
            "<=" => narrow_by(out, "<", PhpVersion::new(value as u8 + 1, 0)),
            _ => {}
        }
    } else if name.kind() == CALL_EXPR
        && value == 0
        && let Some((version, None)) = version_compare(name)
    {
        narrow_by(out, op, version);
    }
}

/// The version and operator of a `version_compare(PHP_VERSION, ...)` call.
fn version_compare(call: &SyntaxNode) -> Option<(PhpVersion, Option<String>)> {
    let callee = call.children().next()?;
    if !is_name(&callee, "version_compare") {
        return None;
    }
    let args: Vec<SyntaxNode> = call
        .children()
        .find(|c| c.kind() == ARG_LIST)?
        .children()
        .filter_map(|a| a.children().next())
        .collect();
    if !is_name(args.first()?, "PHP_VERSION") {
        return None;
    }
    let string = |node: &SyntaxNode| -> Option<String> {
        (node.kind() == STRING_LITERAL).then(|| {
            node.text()
                .to_string()
                .trim()
                .trim_matches(['\'', '"'])
                .to_owned()
        })
    };
    let version = PhpVersion::parse(&string(args.get(1)?)?)?;
    let op = args.get(2).and_then(string).map(|op| match op.as_str() {
        "ge" => ">=".to_owned(),
        "gt" => ">".to_owned(),
        "lt" => "<".to_owned(),
        "le" => "<=".to_owned(),
        _ => op,
    });
    Some((version, op))
}

/// Why `what` can't be relied on across `versions`.
pub fn library_message(what: &str, available: Availability, versions: PhpVersions) -> String {
    let project = versions.described();
    match (available.since, available.removed) {
        (Some(since), _) if versions.min < since => {
            format!("PHP {since} added {what}, but this project {project}")
        }
        (_, Some(removed)) => format!("PHP {removed} removed {what}, but this project {project}"),
        _ => format!("{what} isn't on PHP {versions}"),
    }
}

/// A piece of syntax, and the release that added it. The versions are those of PHP's manual and
/// release announcements (the 8.x "New Features" migration guides).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Feature {
    /// Reads after "PHP 8.4 added".
    pub name: &'static str,
    pub since: PhpVersion,
}

const fn feature(name: &'static str, major: u8, minor: u8) -> Feature {
    Feature {
        name,
        since: PhpVersion::new(major, minor),
    }
}

pub const TYPED_PROPERTIES: Feature = feature("typed properties", 7, 4);
pub const ARROW_FUNCTIONS: Feature = feature("arrow functions", 7, 4);
pub const COALESCE_ASSIGN: Feature = feature("the `??=` operator", 7, 4);
pub const ARRAY_SPREAD: Feature = feature("`...` in array literals", 7, 4);
pub const NUMERIC_SEPARATOR: Feature = feature("`_` in numbers", 7, 4);

pub const MATCH: Feature = feature("`match`", 8, 0);
pub const NULLSAFE: Feature = feature("the `?->` operator", 8, 0);
pub const NAMED_ARGUMENTS: Feature = feature("named arguments", 8, 0);
pub const PROMOTION: Feature = feature("constructor property promotion", 8, 0);
pub const UNION_TYPES: Feature = feature("union types", 8, 0);
pub const ATTRIBUTES: Feature = feature("attributes", 8, 0);
pub const STATIC_TYPE: Feature = feature("the `static` return type", 8, 0);
pub const MIXED_TYPE: Feature = feature("the `mixed` type", 8, 0);
pub const THROW_EXPRESSION: Feature = feature("`throw` as an expression", 8, 0);
pub const CATCH_WITHOUT_VARIABLE: Feature = feature("`catch` without a variable", 8, 0);
pub const TRAILING_COMMA_PARAMS: Feature = feature("trailing commas in parameter lists", 8, 0);
pub const CLASS_ON_OBJECT: Feature = feature("`::class` on an object", 8, 0);

pub const ENUMS: Feature = feature("enums", 8, 1);
pub const READONLY_PROPERTIES: Feature = feature("readonly properties", 8, 1);
pub const FIRST_CLASS_CALLABLES: Feature = feature("first-class callable syntax", 8, 1);
pub const NEW_IN_INITIALIZERS: Feature = feature("`new` in initializers", 8, 1);
pub const NEVER_TYPE: Feature = feature("the `never` type", 8, 1);
pub const INTERSECTION_TYPES: Feature = feature("intersection types", 8, 1);
pub const FINAL_CLASS_CONSTANTS: Feature = feature("final class constants", 8, 1);
pub const EXPLICIT_OCTAL: Feature = feature("the `0o` octal prefix", 8, 1);

pub const READONLY_CLASSES: Feature = feature("readonly classes", 8, 2);
pub const DNF_TYPES: Feature = feature("DNF types", 8, 2);
pub const TRUE_TYPE: Feature = feature("the `true` type", 8, 2);
pub const STANDALONE_NULL_FALSE: Feature =
    feature("`null` and `false` as types of their own", 8, 2);
pub const TRAIT_CONSTANTS: Feature = feature("constants in traits", 8, 2);

pub const TYPED_CONSTANTS: Feature = feature("typed class constants", 8, 3);
pub const DYNAMIC_CONSTANT_FETCH: Feature = feature("dynamic class constant fetch", 8, 3);
pub const OVERRIDE_ATTRIBUTE: Feature = feature("`#[\\Override]`", 8, 3);
pub const READONLY_ANONYMOUS_CLASSES: Feature = feature("readonly anonymous classes", 8, 3);

pub const PROPERTY_HOOKS_FEATURE: Feature = feature("property hooks", 8, 4);
pub const ASYMMETRIC_VISIBILITY: Feature = feature("asymmetric visibility", 8, 4);
pub const NEW_WITHOUT_PARENS: Feature = feature("`new` without parentheses", 8, 4);
pub const PROPERTY_MAGIC_CONSTANT: Feature = feature("`__PROPERTY__`", 8, 4);

pub const PIPE_OPERATOR: Feature = feature("the pipe operator", 8, 5);
pub const CLONE_WITH: Feature = feature("`clone` with properties", 8, 5);
pub const VOID_CAST_FEATURE: Feature = feature("the `(void)` cast", 8, 5);
pub const FINAL_PROMOTED: Feature = feature("final promoted properties", 8, 5);
pub const ATTRIBUTES_ON_CONSTANTS: Feature = feature("attributes on constants", 8, 5);
pub const ASYMMETRIC_STATIC: Feature = feature("asymmetric visibility on static properties", 8, 5);
pub const CLOSURES_IN_CONSTANTS: Feature = feature("closures in constant expressions", 8, 5);

/// Syntax in `file` newer than the versions it's for.
#[salsa::tracked(returns(ref))]
pub fn syntax_version_diagnostics(
    db: &dyn Db,
    ws: Workspace,
    file: SourceFile,
) -> std::sync::Arc<Vec<Diagnostic>> {
    let versions = file_versions(file.text(db), ws.settings(db).php_version);
    let root = stir_db::parse(db, file).syntax();
    let mut pass = SyntaxPass {
        db,
        ws,
        file,
        versions,
        out: Vec::new(),
    };
    for node in root.descendants() {
        pass.visit(&node);
    }
    std::sync::Arc::new(pass.out)
}

struct SyntaxPass<'a> {
    db: &'a dyn Db,
    ws: Workspace,
    file: SourceFile,
    versions: PhpVersions,
    out: Vec<Diagnostic>,
}

/// The range of `node` without its leading comments and whitespace.
fn trimmed(node: &SyntaxNode) -> TextRange {
    let first = node
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| !t.kind().is_trivia());
    match first {
        Some(t) => TextRange::new(t.text_range().start(), node.text_range().end()),
        None => node.text_range(),
    }
}

/// The range of the first token of `node` that isn't a comment or whitespace.
fn head(node: &SyntaxNode) -> TextRange {
    node.descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| !t.kind().is_trivia())
        .map_or_else(|| node.text_range(), |t| t.text_range())
}

impl SyntaxPass<'_> {
    fn message(&self, feature: Feature) -> String {
        format!(
            "PHP {} added {}, but this project {}",
            feature.since,
            feature.name,
            self.versions.described()
        )
    }

    fn report(&mut self, range: TextRange, feature: Feature) {
        if self.versions.min >= feature.since {
            return;
        }
        let message = self.message(feature);
        self.out.push(Diagnostic::error(
            self.file.path(self.db).clone(),
            range,
            codes::PHP_SYNTAX_VERSION,
            message,
        ));
    }

    /// For syntax older versions run, doing nothing with it.
    fn warn(&mut self, range: TextRange, feature: Feature, consequence: &str) {
        if self.versions.min >= feature.since {
            return;
        }
        let message = format!("{}; {consequence}", self.message(feature));
        self.out.push(
            Diagnostic::error(
                self.file.path(self.db).clone(),
                range,
                codes::PHP_SYNTAX_VERSION,
                message,
            )
            .with_severity(Severity::Warning),
        );
    }

    fn visit(&mut self, node: &SyntaxNode) {
        match node.kind() {
            PROPERTY_DECL => {
                if let Some(t) = stir_hir::first_type(node) {
                    self.report(trimmed(&t), TYPED_PROPERTIES);
                }
                self.modifiers(node, Modified::Property);
            }
            ARROW_FN_EXPR => self.report(head(node), ARROW_FUNCTIONS),
            ASSIGN_EXPR => {
                if let Some(t) = node.token(QUESTION2_EQ) {
                    self.report(t.text_range(), COALESCE_ASSIGN);
                }
            }
            ARRAY_ELEM => {
                if let Some(t) = node.token(ELLIPSIS) {
                    self.report(t.text_range(), ARRAY_SPREAD);
                }
            }
            LITERAL => {
                let text = node.text().to_string();
                let text = text.trim();
                if text.contains('_') {
                    self.report(trimmed(node), NUMERIC_SEPARATOR);
                }
                if text.starts_with("0o") || text.starts_with("0O") {
                    self.report(trimmed(node), EXPLICIT_OCTAL);
                }
            }
            MATCH_EXPR => self.report(head(node), MATCH),
            NULLSAFE_PROPERTY_FETCH_EXPR => {
                if let Some(t) = node.token(NULLSAFE_ARROW) {
                    self.report(t.text_range(), NULLSAFE);
                }
                self.new_without_parens(node);
            }
            NAMED_ARG => self.report(head(node), NAMED_ARGUMENTS),
            PARAM => {
                self.modifiers(node, Modified::Param);
                self.constant_expressions(node);
                self.implicit_nullable(node);
            }
            UNION_TYPE => {
                if !only_null_and_false(node) {
                    self.report(trimmed(node), UNION_TYPES);
                } else {
                    self.report(trimmed(node), STANDALONE_NULL_FALSE);
                }
            }
            ATTRIBUTE_LIST => self.attributes(node),
            ATTRIBUTE => {
                self.override_attribute(node);
                self.constant_expressions(node);
            }
            NAMED_TYPE => self.named_type(node),
            THROW_EXPR => {
                if node.parent().is_some_and(|p| p.kind() != EXPR_STMT) {
                    self.report(head(node), THROW_EXPRESSION);
                }
            }
            CATCH_CLAUSE => {
                if !node.has_token(VARIABLE) {
                    self.report(head(node), CATCH_WITHOUT_VARIABLE);
                }
            }
            PARAM_LIST | CLOSURE_USE => {
                let parts: Vec<_> = node
                    .children_with_tokens()
                    .filter(|e| !e.kind().is_trivia())
                    .collect();
                if let [.., comma, close] = parts.as_slice()
                    && comma.kind() == COMMA
                    && close.kind() == R_PAREN
                {
                    self.report(comma.text_range(), TRAILING_COMMA_PARAMS);
                }
            }
            CLASS_CONST_FETCH_EXPR => {
                // `A::{$b}()` and `$a::class()` are calls, which any version has
                let is_callee = node.parent().is_some_and(|p| p.kind() == STATIC_CALL_EXPR);
                if is_callee {
                    // not one of these
                } else if let Some(t) = node.token(L_CURLY) {
                    self.report(t.text_range(), DYNAMIC_CONSTANT_FETCH);
                } else if node
                    .name_text()
                    .is_some_and(|n| n.eq_ignore_ascii_case("class"))
                    && node
                        .children()
                        .next()
                        .is_some_and(|b| b.kind() != NAME_EXPR)
                {
                    self.report(trimmed(node), CLASS_ON_OBJECT);
                }
                self.new_without_parens(node);
            }
            ENUM_DECL => {
                let range = node.child(NAME).map_or_else(|| head(node), |n| trimmed(&n));
                self.report(range, ENUMS);
            }
            CALLABLE_PLACEHOLDER => self.report(trimmed(node), FIRST_CLASS_CALLABLES),
            STATIC_VAR | CONST_ELEM | PROPERTY_ELEM => {
                self.constant_expressions(node);
                if node.kind() == CONST_ELEM {
                    self.class_const_elem(node);
                }
            }
            INTERSECTION_TYPE => {
                if node.parent().is_none_or(|p| p.kind() != PAREN_TYPE) {
                    self.report(trimmed(node), INTERSECTION_TYPES);
                }
            }
            CLASS_CONST_DECL => {
                self.modifiers(node, Modified::Const);
                let in_trait = node
                    .parent()
                    .and_then(|body| body.parent())
                    .is_some_and(|c| c.kind() == TRAIT_DECL);
                if in_trait {
                    self.report(head(node), TRAIT_CONSTANTS);
                }
            }
            CLASS_DECL => {
                if let Some(t) = node.child(MODIFIERS).and_then(|m| m.token(READONLY_KW)) {
                    self.report(t.text_range(), READONLY_CLASSES);
                }
            }
            ANON_CLASS => {
                if let Some(t) = node.token(READONLY_KW) {
                    self.report(t.text_range(), READONLY_ANONYMOUS_CLASSES);
                }
            }
            PAREN_TYPE => self.report(trimmed(node), DNF_TYPES),
            PROPERTY_HOOKS => self.report(head(node), PROPERTY_HOOKS_FEATURE),
            PROPERTY_FETCH_EXPR | STATIC_PROPERTY_FETCH_EXPR | INDEX_EXPR | CALL_EXPR => {
                self.new_without_parens(node);
            }
            MAGIC_CONST => {
                if node.has_token(PROPERTY_C_KW) {
                    self.report(trimmed(node), PROPERTY_MAGIC_CONSTANT);
                }
            }
            BIN_EXPR => {
                if let Some(t) = node.token(PIPE_GT) {
                    self.report(t.text_range(), PIPE_OPERATOR);
                }
            }
            CLONE_EXPR => {
                let args = node.child(ARG_LIST).map_or(0, |l| {
                    l.children()
                        .filter(|a| matches!(a.kind(), ARG | NAMED_ARG))
                        .count()
                });
                if args > 1 {
                    self.report(head(node), CLONE_WITH);
                }
            }
            CAST_EXPR => {
                if let Some(t) = node.token(VOID_CAST) {
                    self.report(t.text_range(), VOID_CAST_FEATURE);
                }
            }
            CONST_DECL => {
                if let Some(list) = node.child(ATTRIBUTE_LIST) {
                    self.report(trimmed(&list), ATTRIBUTES_ON_CONSTANTS);
                }
            }
            _ => {}
        }
    }

    fn modifiers(&mut self, node: &SyntaxNode, of: Modified) {
        let Some(mods) = node.child(MODIFIERS) else {
            return;
        };
        if of == Modified::Param {
            self.report(trimmed(&mods), PROMOTION);
        }
        let tokens = mods.tokens();
        for t in &tokens {
            match t.kind() {
                READONLY_KW if of != Modified::Const => {
                    self.report(t.text_range(), READONLY_PROPERTIES);
                }
                FINAL_KW if of == Modified::Const => {
                    self.report(t.text_range(), FINAL_CLASS_CONSTANTS);
                }
                FINAL_KW if of == Modified::Param => {
                    self.report(t.text_range(), FINAL_PROMOTED);
                }
                _ => {}
            }
        }
        if tokens.iter().any(|t| t.text().eq_ignore_ascii_case("set")) {
            self.report(trimmed(&mods), ASYMMETRIC_VISIBILITY);
            if tokens.iter().any(|t| t.kind() == STATIC_KW) {
                self.report(trimmed(&mods), ASYMMETRIC_STATIC);
            }
        }
    }

    fn class_const_elem(&mut self, elem: &SyntaxNode) {
        if elem.parent().is_some_and(|p| p.kind() == CLASS_CONST_DECL)
            && let Some(t) = stir_hir::first_type(elem)
        {
            self.report(trimmed(&t), TYPED_CONSTANTS);
        }
    }

    fn named_type(&mut self, node: &SyntaxNode) {
        let Some(parent) = node.parent() else {
            return;
        };
        if matches!(parent.kind(), EXTENDS_CLAUSE | IMPLEMENTS_CLAUSE) {
            return;
        }
        let name = node.text().to_string();
        let feature = match name.trim().to_ascii_lowercase().as_str() {
            "mixed" => MIXED_TYPE,
            "never" => NEVER_TYPE,
            "static" => STATIC_TYPE,
            "true" => TRUE_TYPE,
            "null" | "false" if !matches!(parent.kind(), UNION_TYPE) => STANDALONE_NULL_FALSE,
            _ => return,
        };
        self.report(trimmed(node), feature);
    }

    /// Before 8.0, `#[` starts a comment: an attribute alone on its line is skipped, which lets
    /// code use attributes that only newer versions look at, but what shares a line with one
    /// (or the rest of one spread over lines) is then not what it seems.
    fn attributes(&mut self, list: &SyntaxNode) {
        if self.versions.min >= ATTRIBUTES.since {
            return;
        }
        for group in list.children_of(ATTRIBUTE_GROUP) {
            let range = trimmed(&group);
            let text = self.file.text(self.db);
            let (start, end) = (usize::from(range.start()), usize::from(range.end()));
            let line_rest = text[end..].lines().next().unwrap_or_default().trim();
            let alone = !text[start..end].contains('\n')
                && (line_rest.is_empty()
                    || line_rest.starts_with("//")
                    || line_rest.starts_with('#'));
            if !alone {
                self.report(range, ATTRIBUTES);
            } else if !self.only_php_attributes(&group) {
                self.warn(range, ATTRIBUTES, "older versions read it as a comment");
            }
        }
    }

    /// PHP's own attributes are made to be written for versions that skip them.
    fn only_php_attributes(&self, group: &SyntaxNode) -> bool {
        group.children_of(ATTRIBUTE).iter().all(|attr| {
            self.attribute_name(attr).is_some_and(|name| {
                matches!(
                    name.as_str(),
                    "attribute"
                        | "allowdynamicproperties"
                        | "deprecated"
                        | "nodiscard"
                        | "override"
                        | "returntypewillchange"
                        | "sensitiveparameter"
                )
            })
        })
    }

    /// The lowercase name an attribute resolves to.
    fn attribute_name(&self, attr: &SyntaxNode) -> Option<smol_str::SmolStr> {
        let name = attr.child(NAME_REF)?;
        let items = stir_hir::file_items(self.db, self.file);
        let scope = items.scope_at(attr.text_range().start());
        Some(scope.resolve_class(name.text().to_string().trim()).key())
    }

    /// PHP doesn't look at `#[\Override]` before 8.3, so code may use it for newer versions; a
    /// polyfill of the class says that's the intent.
    fn override_attribute(&mut self, node: &SyntaxNode) {
        if self.versions.min >= OVERRIDE_ATTRIBUTE.since {
            return;
        }
        if self.attribute_name(node).as_deref() == Some("override")
            && !stir_hir::def_index(self.db, self.ws)
                .classes
                .contains_key("override")
        {
            self.warn(trimmed(node), OVERRIDE_ATTRIBUTE, "this version ignores it");
        }
    }

    /// `int $x = null` makes the type nullable, which PHP 8.4 deprecated.
    fn implicit_nullable(&mut self, param: &SyntaxNode) {
        if self.versions.max < PhpVersion::new(8, 4) {
            return;
        }
        let Some(ty) = stir_hir::first_type(param) else {
            return;
        };
        let defaults_to_null = param.children().any(|c| {
            c.kind() == NAME_EXPR && c.text().to_string().trim().eq_ignore_ascii_case("null")
        });
        if !defaults_to_null || accepts_null(&ty) {
            return;
        }
        let text = ty.text().to_string();
        let message = format!(
            "`{0} ${1} = null` is implicitly nullable, which PHP 8.4 deprecated: write `?{0}`",
            text.trim(),
            param
                .token(VARIABLE)
                .map_or_else(String::new, |v| v.text()[1..].to_owned()),
        );
        self.out.push(
            Diagnostic::error(
                self.file.path(self.db).clone(),
                trimmed(&ty),
                codes::PHP_DEPRECATED,
                message,
            )
            .with_severity(Severity::Warning),
        );
    }

    /// `new Foo()->bar`: PHP used to need `(new Foo())->bar`.
    fn new_without_parens(&mut self, node: &SyntaxNode) {
        if let Some(base) = node.children().next()
            && base.kind() == NEW_EXPR
        {
            self.report(trimmed(&base), NEW_WITHOUT_PARENS);
        }
    }

    /// What newer versions allow in the constant expressions of a parameter's default, a
    /// constant, a static variable, a property's default or an attribute's arguments.
    fn constant_expressions(&mut self, owner: &SyntaxNode) {
        let exprs = owner.children().filter(|c| {
            !stir_hir::is_type_node(c.kind())
                && !matches!(c.kind(), NAME | MODIFIERS | ATTRIBUTE_LIST | PROPERTY_HOOKS)
        });
        // `new` isn't in a property's default on any version: a syntax error, not a version's
        let allows_new = owner.kind() != PROPERTY_ELEM;
        for expr in exprs.collect::<Vec<_>>() {
            let mut stack = vec![expr];
            while let Some(node) = stack.pop() {
                match node.kind() {
                    NEW_EXPR if allows_new => {
                        self.report(trimmed(&node), NEW_IN_INITIALIZERS);
                    }
                    CLOSURE_EXPR => self.report(head(&node), CLOSURES_IN_CONSTANTS),
                    ANON_CLASS | ARROW_FN_EXPR => {}
                    _ => stack.extend(node.children()),
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Modified {
    Property,
    Param,
    Const,
}

/// Does the type itself allow `null`?
fn accepts_null(ty: &SyntaxNode) -> bool {
    let is_null_or_mixed = |t: &SyntaxNode| {
        t.kind() == NAMED_TYPE
            && matches!(
                t.text().to_string().trim().to_ascii_lowercase().as_str(),
                "null" | "mixed"
            )
    };
    match ty.kind() {
        NULLABLE_TYPE => true,
        UNION_TYPE => ty.children().any(|t| is_null_or_mixed(&t)),
        _ => is_null_or_mixed(ty),
    }
}

/// `null|false`: a union in name only.
fn only_null_and_false(union: &SyntaxNode) -> bool {
    union.children().all(|t| {
        t.kind() == NAMED_TYPE
            && matches!(
                t.text().to_string().trim().to_ascii_lowercase().as_str(),
                "null" | "false"
            )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const V80: PhpVersion = PhpVersion::new(8, 0);
    const V81: PhpVersion = PhpVersion::new(8, 1);
    const V84: PhpVersion = PhpVersion::new(8, 4);

    #[test]
    fn a_lint_comment_narrows_a_file() {
        let project = PhpVersions::exactly(V81);
        assert_eq!(
            file_versions("<?php // lint >= 8.4\n", project).min,
            V84,
            "newer"
        );
        assert_eq!(file_versions("<?php\n// lint >= 8.4\n", project).min, V81);
        assert_eq!(file_versions("<?php echo 1;\n", project).min, V81);
        let under = file_versions("<?php // lint < 8.1\n", PhpVersions { min: V80, max: V84 });
        assert_eq!(under.max, V80);
    }

    /// The versions at the `HERE` call of `src`, from 7.4 to 8.5.
    fn at(src: &str) -> Option<PhpVersions> {
        let parse = stir_syntax::parse(src);
        assert!(parse.errors().is_empty(), "{:?}", parse.errors());
        let root = parse.syntax();
        let here = root
            .descendants()
            .find(|n| n.kind() == CALL_EXPR && n.text().to_string().trim() == "HERE()")
            .unwrap();
        versions_at(
            &here,
            PhpVersions {
                min: PhpVersion::new(7, 4),
                max: PhpVersion::LATEST,
            },
        )
    }

    #[test]
    fn conditions_narrow_the_code_they_guard() {
        let min = |s| at(s).unwrap().min.to_string();
        let max = |s| at(s).unwrap().max.to_string();
        assert_eq!(min("<?php if (PHP_VERSION_ID >= 80100) { HERE(); }"), "8.1");
        assert_eq!(
            min("<?php if (\\PHP_VERSION_ID > 80099) { HERE(); }"),
            "8.1"
        );
        assert_eq!(min("<?php if (80100 <= PHP_VERSION_ID) { HERE(); }"), "8.1");
        assert_eq!(max("<?php if (PHP_VERSION_ID < 80100) { HERE(); }"), "8.0");
        assert_eq!(
            min("<?php if (PHP_VERSION_ID < 80100) { } else { HERE(); }"),
            "8.1"
        );
        assert_eq!(
            min(
                "<?php if (PHP_VERSION_ID < 80000) { } elseif (PHP_VERSION_ID < 80100) { } else { HERE(); }"
            ),
            "8.1"
        );
        assert_eq!(
            min("<?php if (version_compare(PHP_VERSION, '8.1', '>=')) { HERE(); }"),
            "8.1"
        );
        assert_eq!(
            min("<?php if (version_compare(PHP_VERSION, '8.1.0') >= 0) { HERE(); }"),
            "8.1"
        );
        assert_eq!(min("<?php if (PHP_MAJOR_VERSION >= 8) { HERE(); }"), "8.0");
        assert_eq!(min("<?php PHP_VERSION_ID >= 80100 ? HERE() : 1;"), "8.1");
        assert_eq!(min("<?php PHP_VERSION_ID >= 80100 && HERE();"), "8.1");
        assert_eq!(min("<?php PHP_VERSION_ID < 80100 || HERE();"), "8.1");
        assert_eq!(
            min("<?php if (PHP_VERSION_ID < 80100) { return; }\nHERE();"),
            "8.1"
        );
        assert_eq!(
            min("<?php if (\\PHP_VERSION_ID < 80100) return;\nfunction f() { HERE(); }"),
            "8.1"
        );
        assert_eq!(
            min(
                "<?php function f() { if (!(PHP_VERSION_ID >= 80100)) { throw new E(); } HERE(); }"
            ),
            "8.1"
        );
        // not applicable
        assert_eq!(
            min("<?php if (PHP_VERSION_ID >= 80100) { }\nHERE();"),
            "7.4"
        );
        assert_eq!(
            min("<?php if ($x || PHP_VERSION_ID >= 80100) { HERE(); }"),
            "7.4"
        );
    }

    #[test]
    fn code_no_version_runs() {
        assert!(at("<?php if (PHP_VERSION_ID < 70000) { HERE(); }").is_none());
    }
}
