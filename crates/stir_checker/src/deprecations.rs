//! The use of what's marked deprecated, by `#[Deprecated]` or `@deprecated`: PHPStan's deprecation
//! rules, with their identifiers. Kept apart from the rest of what a file's checked for, because
//! whether it's an error is the project's choice, while an editor always marks it.

use std::sync::Arc;

use smol_str::SmolStr;
use stir_db::{Db, Diagnostic, FileOrigin, Severity, SourceFile, Workspace};
use stir_hir::{ClassDef, ClassKind, FileItems, file_items};
use stir_syntax::{SyntaxKind::*, SyntaxNode, TextRange, TextSize};

use crate::typed::{Def, MemberKind, typed_file};
use crate::{Ctx, levels};

/// Is `code` one of those?
pub fn is_deprecation(code: &str) -> bool {
    code.ends_with(".deprecated") || code.contains("eprecated")
}

/// How a class-like is called in an identifier.
fn kind_word(kind: ClassKind) -> &'static str {
    match kind {
        ClassKind::Class => "Class",
        ClassKind::Interface => "Interface",
        ClassKind::Trait => "Trait",
        ClassKind::Enum => "Enum",
    }
}

/// `message`, and why if it says why.
fn with_reason(message: String, reason: &str) -> String {
    if reason.is_empty() {
        message
    } else {
        format!("{}:\n{reason}", message.trim_end_matches('.'))
    }
}

/// Is the code at `offset` itself deprecated? Then it may use what is.
fn in_deprecated_scope(items: &FileItems, offset: TextSize) -> bool {
    let around = |r: TextRange| r.contains(offset);
    items
        .functions
        .iter()
        .any(|f| f.deprecated.is_some() && around(f.ptr.text_range()))
        || items.classes.iter().any(|c| {
            around(c.ptr.text_range())
                && (c.deprecated.is_some()
                    || c.methods
                        .iter()
                        .any(|m| m.deprecated.is_some() && !m.magic && around(m.ptr.text_range())))
        })
}

struct Pass<'c, 'db> {
    ctx: &'c Ctx<'db>,
    file: SourceFile,
    items: &'db FileItems,
    out: Vec<Diagnostic>,
}

impl Pass<'_, '_> {
    fn report(&mut self, range: TextRange, code: String, message: String) {
        if in_deprecated_scope(self.items, range.start()) {
            return;
        }
        let path = self.file.path(self.ctx.db).clone();
        self.out.push(Diagnostic::custom(
            path,
            range,
            Severity::Warning,
            code,
            message,
        ));
    }

    fn class_of(&self, key: &str) -> Option<&ClassDef> {
        let loc = *self.ctx.index.classes.get(key)?.first()?;
        Some(&file_items(self.ctx.db, loc.file).classes[loc.index as usize])
    }

    /// A member: what `class` declares of it, or else whether `class` is deprecated.
    fn member(&mut self, range: TextRange, class: &str, kind: MemberKind, name: &SmolStr) {
        let Some(c) = self.class_of(class) else {
            return;
        };
        let (is_static, own) = match kind {
            MemberKind::Method => c
                .methods
                .iter()
                .find(|m| m.name.eq_ignore_ascii_case(name))
                .map_or((false, None), |m| (m.mods.is_static, m.deprecated.clone())),
            MemberKind::Property => c
                .properties
                .iter()
                .find(|p| &p.name == name)
                .map_or((false, None), |p| (p.mods.is_static, p.deprecated.clone())),
            MemberKind::Const => c
                .consts
                .iter()
                .find(|k| &k.name == name)
                .map_or((true, None), |k| (true, k.deprecated.clone())),
            MemberKind::Case => c
                .cases
                .iter()
                .find(|k| &k.name == name)
                .map_or((true, None), |k| (true, k.deprecated.clone())),
        };
        let stem = match (kind, is_static) {
            (MemberKind::Method, true) => "staticMethod",
            (MemberKind::Method, false) => "method",
            (MemberKind::Property, true) => "staticProperty",
            (MemberKind::Property, false) => "property",
            _ => "classConstant",
        };
        let display = c.name.as_str().to_owned();
        let word = kind_word(c.kind);
        let lower = word.to_ascii_lowercase();
        let (class_reason, class_deprecated) = (c.deprecated.clone(), c.deprecated.is_some());
        let what = match (kind, is_static) {
            (MemberKind::Method, true) => format!("Call to static method {name}()"),
            (MemberKind::Method, false) => format!("Call to method {name}()"),
            (MemberKind::Property, true) => format!("Access to static property ${name}"),
            (MemberKind::Property, false) => format!("Access to property ${name}"),
            _ => format!("Fetching class constant {name}"),
        };
        if class_deprecated {
            self.report(
                range,
                format!("{stem}.deprecated{word}"),
                with_reason(
                    format!("{what} of deprecated {lower} {display}."),
                    class_reason.as_deref().unwrap_or(""),
                ),
            );
            return;
        }
        if let Some(reason) = own {
            let what = match (kind, is_static) {
                (MemberKind::Method, true) => format!("Call to deprecated static method {name}()"),
                (MemberKind::Method, false) => format!("Call to deprecated method {name}()"),
                (MemberKind::Property, true) => {
                    format!("Access to deprecated static property ${name}")
                }
                (MemberKind::Property, false) => format!("Access to deprecated property ${name}"),
                _ => format!("Fetching deprecated class constant {name}"),
            };
            self.report(
                range,
                format!("{stem}.deprecated"),
                with_reason(format!("{what} of {lower} {display}."), &reason),
            );
        }
    }

    /// A class named at `range`, where `id` and `describe` say how.
    fn class_name(
        &mut self,
        range: TextRange,
        key: &str,
        id: impl FnOnce(&str) -> String,
        describe: impl FnOnce(&str, &str) -> String,
    ) {
        let Some(c) = self.class_of(key) else {
            return;
        };
        let Some(reason) = c.deprecated.clone() else {
            return;
        };
        let word = kind_word(c.kind);
        let display = c.name.as_str().to_owned();
        let what = format!("deprecated {} {display}", word.to_ascii_lowercase());
        self.report(
            range,
            id(&format!("Deprecated{word}")),
            with_reason(describe(&what, &display), &reason),
        );
    }

    /// Names outside expressions, which the checker doesn't record.
    fn written_names(&mut self, root: &SyntaxNode) {
        for node in root.descendants().filter(|n| n.kind() == NAME_REF) {
            let Some(parent) = node.parent() else {
                continue;
            };
            let text = node.text().to_string();
            let scope = self.items.scope_at(node.text_range().start()).clone();
            let key = scope.resolve_class(text.trim()).key();
            let range = node.text_range();
            // the supertypes in a clause are types, or plain names
            let place = match parent.parent() {
                Some(clause)
                    if parent.kind() == NAMED_TYPE
                        && matches!(clause.kind(), EXTENDS_CLAUSE | IMPLEMENTS_CLAUSE) =>
                {
                    clause
                }
                _ => parent.clone(),
            };
            let parent = place;
            match parent.kind() {
                EXTENDS_CLAUSE => {
                    let interface = parent.parent().is_some_and(|d| d.kind() == INTERFACE_DECL);
                    let owner = if interface { "interface" } else { "class" };
                    let declaring = declaring_name(&parent);
                    self.class_name(
                        range,
                        &key,
                        |d| format!("{owner}.extends{d}"),
                        |what, _| format!("{} {declaring} extends {what}.", title(owner)),
                    );
                }
                IMPLEMENTS_CLAUSE => {
                    let is_enum = parent.parent().is_some_and(|d| d.kind() == ENUM_DECL);
                    let owner = if is_enum { "enum" } else { "class" };
                    let declaring = declaring_name(&parent);
                    self.class_name(
                        range,
                        &key,
                        |d| format!("{owner}.implements{d}"),
                        |what, _| format!("{} {declaring} implements {what}.", title(owner)),
                    );
                }
                TRAIT_USE => self.class_name(
                    range,
                    &key,
                    |d| format!("traitUse.{}", lower_first(d)),
                    |what, _| format!("Usage of {what}."),
                ),
                NAMED_TYPE => {
                    let place = parent
                        .ancestors()
                        .find(|a| matches!(a.kind(), PARAM | RETURN_TYPE | BLOCK | CLASS_BODY));
                    match place.map(|p| p.kind()) {
                        Some(PARAM) => self.class_name(
                            range,
                            &key,
                            |d| format!("parameter.{}", lower_first(d)),
                            |what, _| format!("Parameter has typehint with {what}."),
                        ),
                        Some(RETURN_TYPE) => self.class_name(
                            range,
                            &key,
                            |d| format!("return.{}", lower_first(d)),
                            |what, _| format!("Return type has typehint with {what}."),
                        ),
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }
}

fn lower_first(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_ascii_lowercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

fn title(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

fn declaring_name(clause: &SyntaxNode) -> String {
    clause
        .parent()
        .and_then(|d| d.children().find(|c| c.kind() == NAME))
        .map_or_else(String::new, |n| n.text().to_string().trim().to_owned())
}

/// Every use of something deprecated in `file`, as warnings.
#[salsa::tracked(returns(ref))]
pub fn deprecations(db: &dyn Db, ws: Workspace, file: SourceFile) -> Arc<Vec<Diagnostic>> {
    let settings = ws.settings(db);
    if file.origin(db) != FileOrigin::Source
        || levels::is_nocheck(file.text(db))
        || !levels::is_analysed(file.path(db), settings)
        || !stir_db::syntax_diagnostics(db, file).is_empty()
    {
        return Arc::new(Vec::new());
    }
    let ctx = Ctx::new(db, ws);
    let items = file_items(db, file);
    let root = stir_db::parse(db, file).syntax();
    let typed = typed_file(db, ws, file);
    let mut pass = Pass {
        ctx: &ctx,
        file,
        items,
        out: Vec::new(),
    };
    let mut defs: Vec<_> = typed.defs.iter().collect();
    defs.sort_by_key(|(r, _)| (r.start(), r.end()));
    for (range, def) in defs {
        match def {
            Def::Function(loc) => {
                let f = &file_items(db, loc.file).functions[loc.index as usize];
                if let Some(reason) = &f.deprecated {
                    let message = format!("Call to deprecated function {}().", f.name);
                    pass.report(
                        *range,
                        "function.deprecated".to_owned(),
                        with_reason(message, reason),
                    );
                }
            }
            Def::Const(loc) => {
                let c = &file_items(db, loc.file).consts[loc.index as usize];
                if let Some(reason) = &c.deprecated {
                    let message = format!("Use of constant {} is deprecated.", c.name);
                    pass.report(
                        *range,
                        "constant.deprecated".to_owned(),
                        with_reason(message, reason),
                    );
                }
            }
            Def::Member { class, kind, name } => pass.member(*range, class, *kind, name),
            Def::Class(key) => {
                let Some(token) = root.token_at_offset(range.start()).right_biased() else {
                    continue;
                };
                let around = token
                    .parent_ancestors()
                    .find(|n| !matches!(n.kind(), NAME | NAME_REF | NAME_EXPR));
                match around.map(|n| n.kind()) {
                    Some(NEW_EXPR) => pass.class_name(
                        *range,
                        key,
                        |d| format!("new.{}", lower_first(d)),
                        |what, _| format!("Instantiation of {what}."),
                    ),
                    _ => {}
                }
            }
            Def::Local { .. } => {}
        }
    }
    pass.written_names(&root);
    let mut out = pass.out;
    out.sort_by_key(|d| (d.range.start(), d.range.end()));
    out.dedup();
    Arc::new(out)
}
