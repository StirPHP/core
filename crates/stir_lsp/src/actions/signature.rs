//! Changing a function's signature, and every call of it: removing a parameter nothing uses, moving
//! one left or right, and adding one.
//!
//! The protocol has no dialog, so each is its own action, offered with the cursor on a parameter or
//! on the name in the declaration. What's added needs a name and a type, which an editor that can
//! ask gets through a command of its own ([`NewParameter`]); others get a parameter called
//! `$newParameter`, to rename.
//!
//! A new parameter with a default leaves calls alone. One without is passed at each call as the
//! constant `TODO_NEW_ARGUMENT`, which doesn't exist: every call that needs a real value is an
//! error until it has one, as opposed to something that quietly runs with a wrong one. An override
//! passing it on to its parent passes its own new parameter.
//!
//! It's all of the calls or none: what can't be updated, or can't be told not to be a call, is
//! refused and said, leaving the code alone. That's a spread argument where it matters, an
//! argument with effects that would no longer run, `$this->foo(...)`, `[$x, 'foo']` and such, and
//! `$x->foo()` where what `$x` is isn't known. `$x->$name()` can't be seen at all.

use stir_checker::{Def, MemberKind};
use stir_db::{Db, FileOrigin, SourceFile, Workspace};
use stir_hir::ast::NodeExt;
use stir_hir::{ClassKind, file_items};
use stir_syntax::{SyntaxKind::*, SyntaxNode, TextRange, TextSize};

use super::fix_types::written_type;
use super::flow::trimmed_node;
use super::hierarchy::{self, method_of};
use super::variables::{dynamic_scope, is_pure};
use super::{Change, Cx, FileEdit, Kind, Offer, Refusal};
use crate::hierarchy::{all_subtypes, class_def};
use crate::imports;
use crate::nav;

/// What's passed at a call for a new parameter nobody's said a value of: it isn't defined.
pub const PLACEHOLDER: &str = "TODO_NEW_ARGUMENT";

pub fn offers<'c>(cx: &'c Cx<'c>, out: &mut Vec<Offer<'c>>) {
    let Some(token) = cx.here() else { return };
    let Some(parent) = token.parent() else { return };
    if token.kind() == VARIABLE && parent.kind() == PARAM {
        let Some(function) = declaration_of(&parent) else {
            return;
        };
        let Some(index) = param_index(&function, &parent) else {
            return;
        };
        param_offers(cx, &function, &parent, index, out);
        return;
    }
    if parent.kind() == NAME
        && let Some(function) = parent
            .parent()
            .filter(|f| matches!(f.kind(), FUNCTION_DECL | METHOD_DECL))
        && !cx.abilities.asks_for_parameters
        && !is_magic(&function)
    {
        let at = function.text_range().start();
        out.push(
            Offer::new(
                "signature.add",
                "Add a parameter",
                Kind::RefactorRewrite,
                move || {
                    add_parameter(
                        cx.db,
                        cx.ws,
                        cx.file,
                        at,
                        &NewParameter {
                            declared: "$newParameter".to_owned(),
                            passed: None,
                        },
                    )
                },
            )
            .costly(),
        );
    }
}

/// Where a client that can ask for a new parameter would add one: the declaration at the cursor.
pub fn ask_parameter_at(cx: &Cx<'_>) -> Option<TextSize> {
    let token = cx.here()?;
    let parent = token.parent()?;
    if parent.kind() != NAME {
        return None;
    }
    let function = parent
        .parent()
        .filter(|f| matches!(f.kind(), FUNCTION_DECL | METHOD_DECL))?;
    if is_magic(&function) {
        return None;
    }
    Some(function.text_range().start())
}

fn is_magic(function: &SyntaxNode) -> bool {
    function.kind() == METHOD_DECL
        && function.name_text().is_some_and(|n| {
            let lower = n.to_ascii_lowercase();
            lower.starts_with("__") && lower != "__construct"
        })
}

/// The function or method a parameter's of, if that's the kind this does.
fn declaration_of(param: &SyntaxNode) -> Option<SyntaxNode> {
    let function = param.ancestors().find(|n| {
        matches!(
            n.kind(),
            FUNCTION_DECL | METHOD_DECL | CLOSURE_EXPR | ARROW_FN_EXPR
        )
    })?;
    (matches!(function.kind(), FUNCTION_DECL | METHOD_DECL) && !is_magic(&function))
        .then_some(function)
}

fn params_of(function: &SyntaxNode) -> Vec<SyntaxNode> {
    function
        .child(PARAM_LIST)
        .map(|l| l.children_of(PARAM))
        .unwrap_or_default()
}

fn param_index(function: &SyntaxNode, param: &SyntaxNode) -> Option<usize> {
    params_of(function).iter().position(|p| p == param)
}

fn param_name(param: &SyntaxNode) -> Option<String> {
    Some(param.token(VARIABLE)?.text().to_owned())
}

fn is_optional(param: &SyntaxNode) -> bool {
    param.has_token(EQ) || param.has_token(ELLIPSIS)
}

fn param_offers<'c>(
    cx: &'c Cx<'c>,
    function: &SyntaxNode,
    param: &SyntaxNode,
    index: usize,
    out: &mut Vec<Offer<'c>>,
) {
    let params = params_of(function);
    let Some(name) = param_name(param) else { return };
    let at = function.text_range().start();
    // unused here is a start: the others are looked at when it's chosen
    let used_here = function.child(BLOCK).is_some_and(|b| {
        dynamic_scope(&b)
            || b.descendants_with_tokens()
                .filter_map(|e| e.into_token())
                .any(|t| t.kind() == VARIABLE && t.text() == name)
    });
    if !used_here && param.child(MODIFIERS).is_none() {
        out.push(
            Offer::new(
                format!("signature.remove:{index}"),
                format!("Remove unused parameter `{name}`"),
                Kind::RefactorRewrite,
                move || {
                    change_signature(cx.db, cx.ws, cx.file, at, Rewrite::Remove(index))
                },
            )
            .costly(),
        );
    }
    let can_swap = |a: &SyntaxNode, b: &SyntaxNode| {
        !a.has_token(ELLIPSIS) && !b.has_token(ELLIPSIS) && is_optional(a) == is_optional(b)
    };
    if index > 0 && can_swap(&params[index - 1], param) {
        out.push(
            Offer::new(
                format!("signature.left:{index}"),
                format!("Move `{name}` left"),
                Kind::RefactorRewrite,
                move || change_signature(cx.db, cx.ws, cx.file, at, Rewrite::Swap(index - 1)),
            )
            .costly(),
        );
    }
    if index + 1 < params.len() && can_swap(param, &params[index + 1]) {
        out.push(
            Offer::new(
                format!("signature.right:{index}"),
                format!("Move `{name}` right"),
                Kind::RefactorRewrite,
                move || change_signature(cx.db, cx.ws, cx.file, at, Rewrite::Swap(index)),
            )
            .costly(),
        );
    }
}

/// What's asked for.
#[derive(Debug, Clone)]
enum Rewrite {
    Remove(usize),
    /// This parameter and the next.
    Swap(usize),
    Add {
        /// Before this one.
        index: usize,
        name: String,
        /// As it's written, in the file of the declaration.
        declared: String,
        /// With names that work from anywhere.
        complete: String,
        /// What calls pass; none if it has a default.
        passed: Option<String>,
    },
}

/// A parameter to add, from an editor that asked.
#[derive(Debug, Clone)]
pub struct NewParameter {
    /// `int $limit = 10`, as it's to be written.
    pub declared: String,
    /// What existing calls pass, if it has no default. Without this they pass the placeholder.
    pub passed: Option<String>,
}

/// One declaration of the family.
struct Decl {
    file: SourceFile,
    node: SyntaxNode,
    class_key: Option<smol_str::SmolStr>,
}

fn parsed_node(db: &dyn Db, file: SourceFile, range: TextRange, kinds: &[stir_syntax::SyntaxKind]) -> Option<SyntaxNode> {
    let root = stir_db::parse(db, file).syntax();
    if !root.text_range().contains_range(range) {
        return None;
    }
    root.covering_element(range)
        .into_node()?
        .ancestors()
        .find(|n| kinds.contains(&n.kind()) && n.text_range() == range)
}

fn location(db: &dyn Db, file: SourceFile, at: TextSize) -> String {
    let text = file.text(db);
    let line = text[..usize::from(at)].matches('\n').count() + 1;
    let path = file.path(db);
    let shown = path
        .file_name()
        .map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
    format!("{shown}:{line}")
}

/// Everything that goes wrong finding the calls is said at once, up to a point.
struct Blockers(Vec<String>);

impl Blockers {
    fn add(&mut self, why: String) {
        if !self.0.contains(&why) {
            self.0.push(why);
        }
    }

    fn check(self) -> Result<(), Refusal> {
        if self.0.is_empty() {
            return Ok(());
        }
        let total = self.0.len();
        let mut said = self.0.into_iter().take(3).collect::<Vec<_>>().join("; ");
        if total > 3 {
            said.push_str(&format!("; and {} more", total - 3));
        }
        Err(format!("Nothing was changed: {said}."))
    }
}

/// The declarations a change of `function` has to change too, this one first.
fn family(
    db: &dyn Db,
    ws: Workspace,
    file: SourceFile,
    function: &SyntaxNode,
    blockers: &mut Blockers,
) -> Option<Vec<Decl>> {
    if function.kind() == FUNCTION_DECL {
        return Some(vec![Decl {
            file,
            node: function.clone(),
            class_key: None,
        }]);
    }
    // through the checker's view of the class
    let cx = Cx::new(db, ws, file, function.text_range(), &[], None, Default::default());
    let (class, method) = method_of(&cx, function)?;
    let mut out = Vec::new();
    for member in hierarchy::family(&cx, &class.name.key(), &method.name) {
        if member.file.origin(db) != FileOrigin::Source {
            blockers.add(format!(
                "`{}` declares it too, outside this project",
                member.class.name.as_str()
            ));
            continue;
        }
        let Some(node) = parsed_node(db, member.file, member.method.ptr.text_range(), &[METHOD_DECL])
        else {
            blockers.add(format!("`{}`'s declaration can't be found", member.class.name.as_str()));
            continue;
        };
        out.push(Decl {
            file: member.file,
            node,
            class_key: Some(member.key()),
        });
    }
    Some(out)
}

/// The argument list and call a resolved name at `range` of `file` is the name of, if it's a call.
enum Site {
    Call { node: SyntaxNode, args: SyntaxNode },
    /// Not a call: the declaration, say, or a type.
    Other,
    /// Something that calls it, that can't be changed: this says what.
    Unfollowable(String),
}

fn site_at(root: &SyntaxNode, range: TextRange) -> Site {
    let Some(node) = root
        .covering_element(range)
        .into_node()
        .or_else(|| root.token_at_offset(range.start()).right_biased().and_then(|t| t.parent()))
    else {
        return Site::Other;
    };
    let mut at = Some(node);
    while let Some(n) = at {
        match n.kind() {
            CALL_EXPR | METHOD_CALL_EXPR | NULLSAFE_METHOD_CALL_EXPR | STATIC_CALL_EXPR | NEW_EXPR | ATTRIBUTE => {
                let Some(args) = n.child(ARG_LIST) else {
                    // `new Foo` with no parentheses, `#[Foo]`
                    return if matches!(n.kind(), NEW_EXPR | ATTRIBUTE) {
                        Site::Other
                    } else {
                        Site::Unfollowable("it's used as a value".to_owned())
                    };
                };
                if args.children().any(|a| a.kind() == CALLABLE_PLACEHOLDER)
                    || args.descendants().take(4).any(|a| a.kind() == CALLABLE_PLACEHOLDER)
                {
                    return Site::Unfollowable("it's taken as a callable (`foo(...)`)".to_owned());
                }
                return Site::Call {
                    node: n.clone(),
                    args,
                };
            }
            NAME | NAME_REF | NAME_EXPR | PROPERTY_FETCH_EXPR | NULLSAFE_PROPERTY_FETCH_EXPR
            | CLASS_CONST_FETCH_EXPR => {
                at = n.parent();
            }
            _ => return Site::Other,
        }
    }
    Site::Other
}

/// The arguments of a call, as they're written.
fn args_of(list: &SyntaxNode) -> Vec<SyntaxNode> {
    list.children()
        .filter(|c| matches!(c.kind(), ARG | NAMED_ARG))
        .collect()
}

/// The range that takes `item`, an element of a list with commas, out of it: the comma after it
/// up to what's next, or if it's the last, what's before it. A trailing comma stays.
fn list_item_removal(list: &SyntaxNode, item: &SyntaxNode) -> TextRange {
    let core = trimmed_node(item);
    let siblings: Vec<_> = list
        .children_with_tokens()
        .filter(|e| !e.kind().is_trivia())
        .collect();
    let at = siblings
        .iter()
        .position(|e| e.as_node() == Some(item))
        .unwrap_or(0);
    let next_item = siblings[at + 1..]
        .iter()
        .find(|e| !matches!(e.kind(), COMMA | R_PAREN))
        .cloned();
    if let Some(next) = next_item {
        return TextRange::new(core.start(), next.text_range().start());
    }
    // the last: back over the comma before it
    let before = siblings[..at].iter().rev().find(|e| e.kind() == COMMA);
    let trailing = siblings
        .get(at + 1)
        .is_some_and(|e| e.kind() == COMMA);
    match (before, trailing) {
        (Some(_), true) => {
            // leave its comma, which ends what's before it now
            let comma_end = siblings[at + 1].text_range().end();
            let previous_end = before.map_or(core.start(), |b| b.text_range().end());
            TextRange::new(previous_end, comma_end)
        }
        (Some(b), false) => TextRange::new(b.text_range().start(), core.end()),
        (None, true) => TextRange::new(core.start(), siblings[at + 1].text_range().end()),
        (None, false) => core,
    }
}

fn add_edit(edits: &mut Vec<FileEdit>, file: SourceFile, range: TextRange, text: String) {
    edits.push(FileEdit { file, range, text });
}

/// The edit that drops the line of `$name` from a doc comment.
fn doc_tag_removal(db: &dyn Db, file: SourceFile, function: &SyntaxNode, name: &str) -> Option<TextRange> {
    let doc = function.doc_comment()?;
    let text = doc.text();
    let range = doc.text_range();
    let is_tag = |line: &str| {
        let line = line.trim_start_matches([' ', '\t', '*', '/']);
        (line.starts_with("@param") || line.starts_with("@phpstan-param") || line.starts_with("@psalm-param"))
            && line
                .split(|c: char| c.is_whitespace())
                .any(|w| w == name || w == format!("...{name}") || w == format!("&{name}"))
    };
    if !text.contains('\n') {
        // the whole of a comment that says only that
        if is_tag(text.trim_end_matches("*/")) {
            let source = file.text(db);
            let line = imports::line_start(source, range.start());
            let end = imports::line_end(source, range.end());
            let rest = &source[usize::from(range.end())..usize::from(end)];
            let before = &source[usize::from(line)..usize::from(range.start())];
            if before.trim().is_empty() && rest.trim().is_empty() {
                let after = &source[usize::from(end)..];
                let ending = if after.starts_with("\r\n") { 2 } else { u32::from(after.starts_with('\n')) };
                let indent = before.len() as u32;
                return Some(TextRange::new(
                    range.start(),
                    end + TextSize::from(ending + indent),
                ));
            }
            return Some(range);
        }
        return None;
    }
    let mut offset = 0usize;
    for piece in text.split_inclusive('\n') {
        if is_tag(piece) && !piece.contains("*/") {
            let start = range.start() + TextSize::from(offset as u32);
            return Some(TextRange::at(start, TextSize::from(piece.len() as u32)));
        }
        offset += piece.len();
    }
    None
}

/// The calls of what `decls` declare, by the names the checker resolved in each source file, with
/// what can't be followed said in `blockers`.
fn calls(
    db: &dyn Db,
    ws: Workspace,
    file: SourceFile,
    decls: &[Decl],
    function: &SyntaxNode,
    blockers: &mut Blockers,
) -> Vec<(SourceFile, SyntaxNode, SyntaxNode)> {
    let name = function.name_text().unwrap_or_default();
    let lower = name.to_ascii_lowercase();
    let is_ctor = lower == "__construct";
    let mut defs: Vec<Def> = Vec::new();
    if function.kind() == FUNCTION_DECL {
        let items = file_items(db, file);
        let range = function.text_range();
        if let Some(i) = items.functions.iter().position(|f| f.ptr.text_range() == range) {
            defs.push(Def::Function(stir_hir::ItemLoc {
                file,
                index: i as u32,
            }));
        }
    } else {
        for d in decls {
            let Some(key) = &d.class_key else { continue };
            defs.push(Def::Member {
                class: key.clone(),
                kind: MemberKind::Method,
                name: smol_str::SmolStr::new(&name),
            });
            if let Some((_, c)) = class_def(db, ws, key)
                && let Some(m) = c.methods.iter().find(|m| m.name.eq_ignore_ascii_case(&name))
            {
                defs.push(Def::Member {
                    class: key.clone(),
                    kind: MemberKind::Method,
                    name: m.name.clone(),
                });
            }
            if is_ctor {
                defs.push(Def::Class(key.clone()));
                // what's made through a subclass with no constructor of its own
                for sub in all_subtypes(db, ws, key) {
                    let own = class_def(db, ws, &sub)
                        .is_some_and(|(_, c)| c.methods.iter().any(|m| m.name.eq_ignore_ascii_case("__construct")));
                    if !own {
                        defs.push(Def::Class(sub));
                    }
                }
            }
        }
    }
    let sources: Vec<SourceFile> = ws
        .files(db)
        .iter()
        .copied()
        .filter(|f| f.origin(db) == FileOrigin::Source)
        .collect();
    let mut out: Vec<(SourceFile, SyntaxNode, SyntaxNode)> = Vec::new();
    let lower_short = lower.as_str();
    for f in sources {
        let text = f.text(db);
        // most files don't mention it at all
        let mentioned = if is_ctor {
            decls.iter().any(|d| {
                d.class_key.as_ref().is_some_and(|k| {
                    let short = k.rsplit('\\').next().unwrap_or(k);
                    contains_ignoring_case(text, short)
                })
            }) || text.contains("__construct")
                || text.contains("new static")
                || text.contains("new self")
                || text.contains("new parent")
        } else {
            contains_ignoring_case(text, &name)
        };
        if !mentioned {
            continue;
        }
        let root = stir_db::parse(db, f).syntax();
        let refs = nav::file_refs(db, ws, f);
        for (range, def) in refs.iter() {
            if !defs.contains(def) {
                continue;
            }
            match site_at(&root, *range) {
                Site::Call { node, args } => {
                    // a class named as what's created, or as an attribute: anything else isn't this
                    if matches!(def, Def::Class(_)) && !matches!(node.kind(), NEW_EXPR | ATTRIBUTE) {
                        continue;
                    }
                    if !out.iter().any(|(g, n, _)| *g == f && *n == node) {
                        out.push((f, node, args));
                    }
                }
                Site::Other => {}
                Site::Unfollowable(what) => {
                    blockers.add(format!("{} at {}", what, location(db, f, range.start())));
                }
            }
        }
        // what names it another way
        look_for_unresolved(db, f, &root, &refs.iter().map(|(r, _)| *r).collect::<Vec<_>>(), &name, lower_short, function.kind() == METHOD_DECL && !is_ctor, blockers);
        if is_ctor {
            constructor_extras(db, ws, f, &root, decls, &mut out, blockers);
        }
    }
    out
}

fn contains_ignoring_case(text: &str, word: &str) -> bool {
    if word.is_empty() {
        return false;
    }
    let bytes = text.as_bytes();
    let needle = word.as_bytes();
    bytes
        .windows(needle.len())
        .any(|w| w.eq_ignore_ascii_case(needle))
}

/// `$x->foo()` that nothing knows the receiver of, and strings that name it as a callable.
#[allow(clippy::too_many_arguments)]
fn look_for_unresolved(
    db: &dyn Db,
    file: SourceFile,
    root: &SyntaxNode,
    resolved: &[TextRange],
    name: &str,
    lower: &str,
    is_method: bool,
    blockers: &mut Blockers,
) {
    for node in root.descendants() {
        match node.kind() {
            METHOD_CALL_EXPR | NULLSAFE_METHOD_CALL_EXPR | STATIC_CALL_EXPR if is_method => {
                let Some(n) = node.children().next().and_then(|f| f.child(NAME)) else {
                    continue;
                };
                let written = n.text().to_string();
                if written.trim().eq_ignore_ascii_case(lower) && !resolved.contains(&n.text_range()) {
                    blockers.add(format!(
                        "`{}` at {} might be a call of it, and what it's called on isn't known",
                        written.trim(),
                        location(db, file, n.text_range().start())
                    ));
                }
            }
            // `[$x, 'foo']`, and `'foo'` given to something that calls it
            ARRAY_EXPR => {
                let elems = node.children_of(ARRAY_ELEM);
                if let [_, second] = elems.as_slice()
                    && string_value(second).is_some_and(|s| s.eq_ignore_ascii_case(name))
                {
                    blockers.add(format!(
                        "an array at {} might be a callable of it",
                        location(db, file, node.text_range().start())
                    ));
                }
            }
            CALL_EXPR => {
                let callee = node.children().next().map(|c| c.text().to_string());
                let callee = callee.as_deref().map(|c| c.trim().trim_start_matches('\\').to_ascii_lowercase());
                let takes_callable = callee.is_some_and(|c| {
                    matches!(
                        c.as_str(),
                        "call_user_func" | "call_user_func_array" | "array_map" | "array_filter"
                            | "array_walk" | "usort" | "uasort" | "uksort" | "array_reduce"
                            | "is_callable" | "function_exists" | "method_exists"
                            | "forward_static_call" | "forward_static_call_array"
                            | "register_shutdown_function" | "spl_autoload_register"
                    )
                });
                if !takes_callable {
                    continue;
                }
                let Some(args) = node.child(ARG_LIST) else { continue };
                for arg in args_of(&args) {
                    let Some(value) = string_value(&arg) else {
                        continue;
                    };
                    let last = value.rsplit("::").next().unwrap_or(&value);
                    if last.rsplit('\\').next().unwrap_or(last).eq_ignore_ascii_case(name) {
                        blockers.add(format!(
                            "a string at {} might be a callable of it",
                            location(db, file, arg.text_range().start())
                        ));
                    }
                }
            }
            _ => {}
        }
    }
}

/// What a string literal says, if `node` is only one.
fn string_value(node: &SyntaxNode) -> Option<String> {
    let t: Vec<_> = node
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| !t.kind().is_trivia())
        .collect();
    match t.as_slice() {
        [only] if matches!(only.kind(), STRING_SINGLE | STRING_DOUBLE) => {
            let text = only.text();
            Some(text[1..text.len().saturating_sub(1)].to_owned())
        }
        _ => None,
    }
}

/// `new self`, `new static` and `parent::__construct` inside the classes concerned, and an
/// anonymous class extending one, whose arguments this doesn't take apart.
fn constructor_extras(
    db: &dyn Db,
    ws: Workspace,
    file: SourceFile,
    root: &SyntaxNode,
    decls: &[Decl],
    out: &mut Vec<(SourceFile, SyntaxNode, SyntaxNode)>,
    blockers: &mut Blockers,
) {
    let items = file_items(db, file);
    let concerned = |key: &str| {
        decls.iter().any(|d| {
            d.class_key.as_deref().is_some_and(|k| {
                k == key
                    || all_subtypes(db, ws, k).iter().any(|s| s == key)
            })
        })
    };
    for node in root.descendants() {
        match node.kind() {
            ANON_CLASS => {
                let Some(extends) = node.child(EXTENDS_CLAUSE) else {
                    continue;
                };
                let name = extends.text().to_string();
                let word = name.split_whitespace().last().unwrap_or_default();
                let key = items.scope_at(node.text_range().start()).resolve_class(word).key();
                if decls.iter().any(|d| d.class_key.as_deref() == Some(&key)) {
                    blockers.add(format!(
                        "an anonymous class at {} extends it",
                        location(db, file, node.text_range().start())
                    ));
                }
            }
            NEW_EXPR => {
                let Some(named) = node.children().find(|c| c.kind() == NAME_EXPR) else {
                    continue;
                };
                let text = named.text().to_string();
                let lower = text.trim().to_ascii_lowercase();
                if !matches!(lower.as_str(), "self" | "static" | "parent") {
                    continue;
                }
                let Some(owner) = node.ancestors().find(|a| a.kind() == CLASS_BODY).and_then(|b| b.parent()) else {
                    continue;
                };
                let range = owner.text_range();
                let Some(class) = items.classes.iter().find(|c| c.ptr.text_range() == range) else {
                    continue;
                };
                // whichever it is, `new static` is the class or one like it
                if !concerned(&class.name.key()) {
                    continue;
                }
                if let Some(args) = node.child(ARG_LIST) {
                    if !out.iter().any(|(f, n, _)| *f == file && *n == node) {
                        out.push((file, node.clone(), args));
                    }
                }
            }
            _ => {}
        }
    }
}

/// Is the method's own name in the family's bodies called through `parent::`?
fn is_parent_forward(call: &SyntaxNode) -> bool {
    call.kind() == STATIC_CALL_EXPR
        && call
            .children()
            .next()
            .and_then(|f| f.children().next())
            .is_some_and(|r| r.text().to_string().trim().eq_ignore_ascii_case("parent"))
}

/// Adds the parameter `parameter` to the function at `at`.
pub fn add_parameter_change(
    db: &dyn Db,
    ws: Workspace,
    file: SourceFile,
    at: TextSize,
    parameter: &NewParameter,
) -> Result<Change, Refusal> {
    add_parameter(db, ws, file, at, parameter)
}

fn add_parameter(
    db: &dyn Db,
    ws: Workspace,
    file: SourceFile,
    at: TextSize,
    parameter: &NewParameter,
) -> Result<Change, Refusal> {
    // read as PHP does, so that it's one parameter and nothing else
    let source = format!("<?php function f({}) {{}}", parameter.declared.trim());
    let parse = stir_syntax::parse(&source);
    if !parse.errors().is_empty() {
        return Err(format!("`{}` isn't a parameter.", parameter.declared.trim()));
    }
    let root = parse.syntax();
    let params: Vec<SyntaxNode> = root.descendants().filter(|n| n.kind() == PARAM).collect();
    let [param] = params.as_slice() else {
        return Err(format!("`{}` isn't one parameter.", parameter.declared.trim()));
    };
    if param.child(MODIFIERS).is_some() || param.has_token(ELLIPSIS) || param.has_token(AMP) {
        return Err("A new parameter can't be promoted, variadic or by reference: add it, then change it.".into());
    }
    let name = param.token(VARIABLE).ok_or("It has no name.")?.text().to_owned();
    if name == "$this" {
        return Err("A parameter can't be `$this`.".into());
    }
    let items = file_items(db, file);
    let complete = complete_type(parameter.declared.trim(), param, items.scope_at(at));
    let has_default = param.has_token(EQ);
    let function = find_function(db, file, at)?;
    let params_now = params_of(&function);
    if params_now
        .iter()
        .any(|p| param_name(p).as_deref() == Some(name.as_str()))
    {
        return Err(format!("It already has a parameter `{name}`."));
    }
    // at the end, ahead of a variadic and, without a default, of any that have one
    let index = params_now
        .iter()
        .position(|p| {
            p.has_token(ELLIPSIS) || (!has_default && is_optional(p))
        })
        .unwrap_or(params_now.len());
    let passed = (!has_default).then(|| {
        parameter
            .passed
            .clone()
            .filter(|p| !p.trim().is_empty())
            .unwrap_or_else(|| PLACEHOLDER.to_owned())
    });
    change_signature(
        db,
        ws,
        file,
        at,
        Rewrite::Add {
            index,
            name,
            declared: parameter.declared.trim().to_owned(),
            complete,
            passed,
        },
    )
}

/// `declared` with the class names of its type written in full, for another file.
fn complete_type(declared: &str, param: &SyntaxNode, scope: &stir_hir::name::Scope) -> String {
    let Some(ty) = written_type(param) else {
        return declared.to_owned();
    };
    let base = "<?php function f(".len();
    let mut out = String::new();
    let mut last = 0usize;
    for name in ty.descendants().filter(|n| n.kind() == NAME_REF) {
        let range = name.text_range();
        let start = usize::from(range.start()) - base;
        let end = usize::from(range.end()) - base;
        let text = name.text().to_string();
        let lower = text.trim().to_ascii_lowercase();
        if stir_hir::type_ref::PhpBuiltin::from_name(&lower).is_some() {
            continue;
        }
        let Some(between) = declared.get(last..start) else {
            return declared.to_owned();
        };
        out.push_str(between);
        out.push_str(&format!("\\{}", scope.resolve_class(text.trim()).as_str()));
        last = end;
    }
    out.push_str(declared.get(last..).unwrap_or_default());
    out
}

fn find_function(db: &dyn Db, file: SourceFile, at: TextSize) -> Result<SyntaxNode, Refusal> {
    let root = stir_db::parse(db, file).syntax();
    root.descendants()
        .find(|n| matches!(n.kind(), FUNCTION_DECL | METHOD_DECL) && n.text_range().start() == at)
        .ok_or_else(|| "The declaration isn't there any more.".to_owned())
}

fn change_signature(
    db: &dyn Db,
    ws: Workspace,
    file: SourceFile,
    at: TextSize,
    rewrite: Rewrite,
) -> Result<Change, Refusal> {
    let function = find_function(db, file, at)?;
    let mut blockers = Blockers(Vec::new());
    let name = function.name_text().unwrap_or_default();
    let decls = family(db, ws, file, &function, &mut blockers).ok_or("It isn't a method this knows.")?;
    let is_ctor = name.eq_ignore_ascii_case("__construct");
    if is_ctor
        && decls.iter().any(|d| {
            d.class_key
                .as_deref()
                .and_then(|k| class_def(db, ws, k))
                .is_some_and(|(_, c)| c.kind == ClassKind::Interface)
        })
    {
        blockers.add("an interface declares this constructor".to_owned());
    }
    let mut edits: Vec<FileEdit> = Vec::new();
    // the declarations
    for decl in &decls {
        let params = params_of(&decl.node);
        let text = decl.file.text(db);
        match &rewrite {
            Rewrite::Remove(i) => {
                let Some(param) = params.get(*i) else {
                    blockers.add(format!("`{}` has fewer parameters", display_decl(decl)));
                    continue;
                };
                if param.child(MODIFIERS).is_some() {
                    blockers.add(format!("in `{}` it's a promoted property", display_decl(decl)));
                    continue;
                }
                let pname = param_name(param).unwrap_or_default();
                if let Some(block) = decl.node.child(BLOCK)
                    && (dynamic_scope(&block)
                        || block
                            .descendants_with_tokens()
                            .filter_map(|e| e.into_token())
                            .any(|t| t.kind() == VARIABLE && t.text() == pname))
                {
                    blockers.add(format!("`{}` uses it", display_decl(decl)));
                    continue;
                }
                let Some(list) = decl.node.child(PARAM_LIST) else {
                    continue;
                };
                add_edit(&mut edits, decl.file, list_item_removal(&list, param), String::new());
                if let Some(tag) = doc_tag_removal(db, decl.file, &decl.node, &pname) {
                    add_edit(&mut edits, decl.file, tag, String::new());
                }
            }
            Rewrite::Swap(i) => {
                let (Some(a), Some(b)) = (params.get(*i), params.get(*i + 1)) else {
                    blockers.add(format!("`{}` has fewer parameters", display_decl(decl)));
                    continue;
                };
                if a.has_token(ELLIPSIS) || b.has_token(ELLIPSIS) || is_optional(a) != is_optional(b) {
                    blockers.add(format!("in `{}` they can't change places", display_decl(decl)));
                    continue;
                }
                let (ra, rb) = (trimmed_node(a), trimmed_node(b));
                add_edit(&mut edits, decl.file, ra, slice(text, rb));
                add_edit(&mut edits, decl.file, rb, slice(text, ra));
            }
            Rewrite::Add {
                index,
                name: new_name,
                declared,
                complete,
                ..
            } => {
                if params
                    .iter()
                    .any(|p| param_name(p).as_deref() == Some(new_name.as_str()))
                {
                    blockers.add(format!("`{}` already has a `{new_name}`", display_decl(decl)));
                    continue;
                }
                let body_uses = decl.node.child(BLOCK).is_some_and(|b| {
                    b.descendants_with_tokens()
                        .filter_map(|e| e.into_token())
                        .any(|t| t.kind() == VARIABLE && t.text() == new_name.as_str())
                });
                if body_uses {
                    blockers.add(format!("`{}` already has a variable `{new_name}`", display_decl(decl)));
                    continue;
                }
                let written = if decl.file == file { declared } else { complete };
                let Some(list) = decl.node.child(PARAM_LIST) else {
                    continue;
                };
                let mut index_here = (*index).min(params.len());
                // what's optional here too has to stay after it
                if params.get(index_here).is_none()
                    && params.last().is_some_and(|p| p.has_token(ELLIPSIS))
                {
                    index_here = params.len() - 1;
                }
                add_edit(&mut edits, decl.file, insertion_point(&list, &params, index_here), insertion_text(text, &params, index_here, written));
            }
        }
    }
    // the calls
    let found = calls(db, ws, file, &decls, &function, &mut blockers);
    for (f, call, args) in found {
        // the declaration's own recursion is a call like any other
        rewrite_call(db, f, &call, &args, &rewrite, &decls, &mut edits, &mut blockers);
    }
    blockers.check()?;
    // the edits of removing an argument and of a call inside it would overlap
    drop_overlaps(&edits).map_err(|()| {
        "A call being changed is inside an argument being removed or moved, which this doesn't untangle, so nothing was changed.".to_owned()
    })?;
    Ok(Change::new(edits))
}

fn slice(text: &str, range: TextRange) -> String {
    text[usize::from(range.start())..usize::from(range.end())].to_owned()
}

fn display_decl(decl: &Decl) -> String {
    let name = decl.node.name_text().unwrap_or_default();
    match &decl.class_key {
        Some(k) => format!("{k}::{name}"),
        None => name,
    }
}

fn insertion_point(list: &SyntaxNode, params: &[SyntaxNode], index: usize) -> TextRange {
    if let Some(next) = params.get(index) {
        return TextRange::empty(trimmed_node(next).start());
    }
    if let Some(last) = params.last() {
        // after a trailing comma, if there's one, or the last parameter
        let end = trimmed_node(last).end();
        return TextRange::empty(end);
    }
    let open = list.token(L_PAREN).map_or_else(
        || list.text_range().start(),
        |t| t.text_range().end(),
    );
    TextRange::empty(open)
}

fn insertion_text(text: &str, params: &[SyntaxNode], index: usize, written: &str) -> String {
    let separator = |a: &SyntaxNode, b: &SyntaxNode| -> String {
        // the same as between these, if they're on lines of their own
        let between = TextRange::new(trimmed_node(a).end(), trimmed_node(b).start());
        let gap = slice(text, between);
        gap.trim_start_matches(',').to_owned()
    };
    if params.get(index).is_some() {
        let sep = if params.len() >= 2 {
            let at = index.min(params.len() - 2);
            separator(&params[at], &params[at + 1])
        } else {
            " ".to_owned()
        };
        return format!("{written},{sep}");
    }
    if params.is_empty() {
        return written.to_owned();
    }
    let sep = if params.len() >= 2 {
        separator(&params[params.len() - 2], &params[params.len() - 1])
    } else {
        " ".to_owned()
    };
    format!(",{sep}{written}")
}

#[allow(clippy::too_many_arguments)]
fn rewrite_call(
    db: &dyn Db,
    file: SourceFile,
    call: &SyntaxNode,
    list: &SyntaxNode,
    rewrite: &Rewrite,
    decls: &[Decl],
    edits: &mut Vec<FileEdit>,
    blockers: &mut Blockers,
) {
    let text = file.text(db);
    let args = args_of(list);
    let where_ = || location(db, file, call.text_range().start());
    let names_at = |i: usize| -> Vec<String> {
        decls
            .iter()
            .filter_map(|d| params_of(&d.node).get(i).and_then(param_name))
            .map(|n| n.trim_start_matches('$').to_owned())
            .collect()
    };
    let spread_at = args.iter().position(|a| a.has_token(ELLIPSIS));
    let first_named = args.iter().position(|a| a.kind() == NAMED_ARG);
    match rewrite {
        Rewrite::Remove(i) => {
            if spread_at.is_some_and(|s| s <= *i) {
                blockers.add(format!("the call at {} spreads its arguments", where_()));
                return;
            }
            let names = names_at(*i);
            let target = args
                .iter()
                .find(|a| {
                    a.kind() == NAMED_ARG
                        && a.name_text().is_some_and(|n| names.contains(&n))
                })
                .or_else(|| {
                    args.get(*i)
                        .filter(|a| a.kind() == ARG && first_named.is_none_or(|n| n > *i))
                });
            let Some(target) = target else { return };
            let value = target.children().filter(|c| c.kind() != NAME).last();
            if value.as_ref().is_some_and(|v| !is_pure(v)) {
                blockers.add(format!(
                    "the argument at {} does something that would no longer happen",
                    where_()
                ));
                return;
            }
            add_edit(edits, file, list_item_removal(list, target), String::new());
        }
        Rewrite::Swap(i) => {
            if spread_at.is_some_and(|s| s <= *i + 1) {
                blockers.add(format!("the call at {} spreads its arguments", where_()));
                return;
            }
            // named, they're as good as they were
            let (Some(a), Some(b)) = (args.get(*i), args.get(*i + 1)) else {
                if args.len() == *i + 1 && first_named.is_none_or(|n| n > *i) {
                    blockers.add(format!("the call at {} leaves one of them out", where_()));
                }
                return;
            };
            match (a.kind(), b.kind()) {
                (ARG, ARG) => {
                    let (ra, rb) = (trimmed_node(a), trimmed_node(b));
                    let (va, vb) = (first_value(a), first_value(b));
                    // the two are evaluated the other way round now
                    if !(va.as_ref().is_some_and(is_pure) || vb.as_ref().is_some_and(is_pure)) {
                        blockers.add(format!(
                            "the arguments at {} both do something, and would swap order",
                            where_()
                        ));
                        return;
                    }
                    add_edit(edits, file, ra, slice(text, rb));
                    add_edit(edits, file, rb, slice(text, ra));
                }
                (NAMED_ARG, NAMED_ARG) => {}
                _ => {
                    blockers.add(format!(
                        "the call at {} passes one of them by name and the other not",
                        where_()
                    ));
                }
            }
        }
        Rewrite::Add {
            index,
            name,
            passed,
            ..
        } => {
            let Some(passed) = passed else { return };
            // an override handing on what it gets hands this on too
            let value = if is_parent_forward(call)
                && call
                    .ancestors()
                    .find(|a| a.kind() == METHOD_DECL)
                    .is_some_and(|m| decls.iter().any(|d| d.node == m))
            {
                name.clone()
            } else {
                passed.clone()
            };
            if spread_at.is_some_and(|s| s <= *index) {
                blockers.add(format!("the call at {} spreads its arguments", where_()));
                return;
            }
            let named = first_named.is_some_and(|n| n <= *index);
            let piece = if named {
                format!("{}: {value}", name.trim_start_matches('$'))
            } else {
                value
            };
            let separator = " ";
            if let Some(next) = args.get(*index).filter(|_| !named) {
                add_edit(
                    edits,
                    file,
                    TextRange::empty(trimmed_node(next).start()),
                    format!("{piece},{separator}"),
                );
            } else if let Some(last) = args.last() {
                add_edit(
                    edits,
                    file,
                    TextRange::empty(trimmed_node(last).end()),
                    format!(",{separator}{piece}"),
                );
            } else {
                let open = list
                    .token(L_PAREN)
                    .map_or_else(|| list.text_range().start(), |t| t.text_range().end());
                add_edit(edits, file, TextRange::empty(open), piece);
            }
        }
    }
}

fn first_value(arg: &SyntaxNode) -> Option<SyntaxNode> {
    arg.children().find(|c| c.kind() != NAME)
}

/// Edits of one file have to be apart.
fn drop_overlaps(edits: &[FileEdit]) -> Result<(), ()> {
    let mut by_file: Vec<(SourceFile, Vec<TextRange>)> = Vec::new();
    for e in edits {
        match by_file.iter_mut().find(|(f, _)| *f == e.file) {
            Some((_, ranges)) => ranges.push(e.range),
            None => by_file.push((e.file, vec![e.range])),
        }
    }
    for (_, ranges) in &mut by_file {
        ranges.sort_by_key(|r| (r.start(), r.end()));
        for pair in ranges.windows(2) {
            if pair[0].end() > pair[1].start() {
                return Err(());
            }
        }
    }
    Ok(())
}
