//! What a template is given: the variables of a Blade template, from the places that render it.
//!
//! This goes in layers, each cut off from the next when what it comes to doesn't change:
//!
//! - [`view_sites`], for a file: what it renders, and with what, as written. No types are read, so
//!   nothing here can depend on a template's variables.
//! - [`view_index`], for the workspace: which file renders which template, and what opens one.
//!   It has no positions in it, so that editing a controller changes nothing here.
//! - [`view_calls`], for a file: its sites with their types, and no positions either.
//! - [`view_data`], for a template: the union of what its sources give it, or why it can't be known.
//!
//! A template is *open* if there may be more to what it's given than this can see. Its variables
//! are then unknown. Otherwise it's *closed*: its variables have the types its sources give them,
//! and one that none of them gives is undefined.
//!
//! A component is one more way of being rendered, by Laravel's rules for it:
//!
//! - `<x-name>` stands for what [`component_index`] says, as `ComponentTagCompiler` looks for it: a
//!   class, or else a template under `components/`.
//! - An anonymous component's template is given the attributes of its tags, and its slots. Those
//!   are its variables, unless `@props` takes them away again by not naming them. Nothing else of
//!   the tag's template gets to it, so one that's open doesn't open it.
//! - A class's template, which is whatever its methods make with `view()`, is given its public
//!   properties and methods, and the slots of the tags for that class.
//! - `{{ $attributes }}` passes on what can't be seen: only what a component's `@props` doesn't
//!   name is known not to be there, and that it's not known what those are.
//! - A slot that can't be told which component it's for, or a `<x-dynamic-component>`, is given to
//!   them all, as far as it's said.
//!
//! A template's data depends on the checked bodies of the templates that include it, which depend
//! on what those are given. A recursive `@include` would make that a cycle, and a cycle through
//! `typed_file` would have a different outcome by which file is checked first. So templates are
//! looked at as a graph first, from the syntax alone: those on a cycle are open and never read
//! their sources, which leaves nothing for Salsa to go round.

use std::collections::BTreeMap;

use rustc_hash::{FxHashMap, FxHashSet};
use smol_str::SmolStr;
use stir_db::{Db, FileOrigin, SourceFile, Workspace};
use stir_hir::ast::NodeExt;
use stir_hir::type_ref::unquote;
use stir_syntax::SyntaxKind::*;
use stir_syntax::{SyntaxElement, SyntaxNode, TextRange};
use stir_types::{Ty, TyKind};

use crate::Ctx;
use crate::body::template::directive_name;
use crate::infer::widen_literals;
use crate::model_api::{self, MemberTy};
use crate::script::links::{view_names, views_are_all_known};
use crate::typed::typed_file;

/// Past this many types for a variable, it isn't worth saying what it is.
const UNION_LIMIT: usize = 32;

/// Why what a template is given can't be known.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum OpenReason {
    /// Not in `resources/views`, not a Laravel application, or not all of the project is looked at.
    ProjectNotKnown,
    /// The view of a class that's looked at here as a component without being known for one, as
    /// in Livewire's `render()`, or that has more for its template than its public members.
    Component,
    /// Under `errors/`: rendered by Laravel's exception handler.
    ErrorPage,
    /// Under `vendor/`: a package's, published to be changed.
    PublishedByPackage,
    /// Uses `$this`: it belongs to a Livewire component or the like.
    UsesThis,
    /// `View::share`, which adds to every template.
    Shared,
    /// A composer or a creator for it, which can add anything.
    Composer,
    /// `Blade::extend` and the like, or a directive of the project's own in it.
    CustomDirective,
    /// A view name worked out when it runs, which may be this one.
    DynamicView,
    /// Its name is in a place that isn't known to render it the way `view()` does.
    NamedElsewhere,
    /// A package's code renders it by name.
    RenderedByPackage,
    /// It's on a cycle of `@include`s.
    Recursive,
    /// Nothing in the project renders it: it may be used from outside.
    NoSource,
    /// A place that renders it gives what can't be read, such as an array built elsewhere.
    SourceNotUnderstood,
    /// A template that includes it, or extends it, is open itself, and passes on all it has.
    ParentOpen,
}

/// A variable a template is given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewVar {
    pub ty: Ty,
    /// Not all that render the template give it.
    pub optional: bool,
}

/// What a template is given.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ViewData {
    /// Why there may be more than this: none means it's closed.
    pub open: Option<OpenReason>,
    pub vars: BTreeMap<SmolStr, ViewVar>,
    /// It's rendered as a component: `$slot` and `$attributes` are there, whatever its name.
    pub component: bool,
    /// `{{ $attributes }}` passes on to it what can't be seen: there's no saying what its `@props`
    /// are given.
    pub spread: bool,
}

impl ViewData {
    fn opened(reason: OpenReason) -> ViewData {
        ViewData {
            open: Some(reason),
            ..ViewData::default()
        }
    }

    pub fn is_closed(&self) -> bool {
        self.open.is_none()
    }
}

/// What's given to a view, as written.
#[derive(Debug, Clone, PartialEq, Eq)]
enum DataPart {
    /// `'name' => $value`
    Pair(SmolStr, TextRange),
    /// `compact('name')`, at the string.
    Compact(SmolStr, TextRange),
    /// Whatever the type of this says.
    FromType(TextRange),
    /// A key that's worked out, or the like.
    NotUnderstood,
    /// An attribute that's data of an anonymous component. If it's in its `$attributes` for all
    /// that's known, `@props` takes that variable away again, unless it says it's one of its own.
    Attribute {
        name: SmolStr,
        value: AttributeValue,
        kept_by_props: bool,
    },
    /// `<x-slot:name>` or `@slot('name')`.
    Slot(SmolStr),
    /// `{{ $attributes }}` or `:attributes="$bag"` in a tag.
    AttributeBag,
}

/// What an attribute of a component's tag holds.
#[derive(Debug, Clone, PartialEq, Eq)]
enum AttributeValue {
    Php(TextRange),
    Text,
    True,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SiteKind {
    /// `view()` and what's like it.
    Plain { mail: bool },
    /// `@include` and `@extends`, which also pass on what's defined where they are.
    Include { inherits: bool },
    /// `@each('name', $items, 'item')`
    Each { variable: Option<SmolStr> },
    /// `->view()`, `->markdown()` and `->text()` on what may be a mailable or a notification's mail
    /// message, and `new Content(...)`: what's given also depends on the class.
    Mail {
        receiver: Option<TextRange>,
        /// The class this is in, if the receiver is `$this`, or for `Content`.
        class: Option<String>,
    },
    /// `<x-name>`, or `@component`, which give it what's given and no more. `names` of the first
    /// are left to find later, by what the workspace has.
    Component { tag: Option<String> },
    /// `view()` in a class. If that's a component's, it's its view, by whichever method, with what
    /// its class has. A `render()` method of anything else is likely to be another such thing.
    ComponentView { class: Option<String>, render: bool },
}

/// A place that renders a template by name.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Site {
    names: Vec<String>,
    kind: SiteKind,
    data: Vec<DataPart>,
    /// What's done with the view can add to it.
    escapes: bool,
    range: TextRange,
}

/// Which templates something applies to, as a name or a pattern with `*`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Target {
    Name(String),
    Pattern(String),
}

/// What a file says about views, as written.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ViewSites {
    sites: Vec<Site>,
    /// What this opens, and why.
    opens: Vec<(Target, OpenReason)>,
    /// `Blade::directive('name', ..)`, in lower case, with what it declares.
    custom_directives: Vec<(String, DirectiveVars)>,
    /// The variables `View::share` is known to add.
    shared: Vec<SmolStr>,
    /// Of a template: has `$this`.
    uses_this: bool,
    /// Of a template: the `@name`s in it, in lower case, that a project could have defined.
    at_names: Vec<String>,
    /// Those of them with something in parentheses, which could name a variable.
    at_calls: Vec<String>,
    /// Of a template: what `@props` declares, if it's all written out and comes before anything is
    /// read, and so is what's taken away.
    props: Option<Vec<SmolStr>>,
    /// What's registered for component tags in this file.
    registrations: Vec<Registration>,
    /// The names of slots with no component around them in this file, as in a template that's
    /// included in one, or in a component this can't tell which that is.
    stray_slots: Vec<SmolStr>,
}

/// `Blade::component` and the like, as written.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Registration {
    /// A tag for a class, by its name, or a view.
    Alias(String, ComponentTarget),
    /// `Blade::componentNamespace('Ns\\Components', 'prefix')`.
    ClassNamespace(String, String),
    /// `Blade::anonymousComponentNamespace('dir', 'prefix')`.
    ViewNamespace(String, String),
}

/// What a component's tag stands for.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ComponentTarget {
    /// A class, by its name as written where it's found.
    Class(String),
    /// A template, by its dotted name.
    View(String),
    /// `<x-dynamic-component>`: it could be any.
    Dynamic,
    /// A package's, or what can't be told.
    Unknown,
}

/// An attribute of a component's tag.
pub(crate) struct TagAttribute {
    /// As Laravel has it, without what says how it's read.
    pub(crate) key: String,
    /// Where its name is.
    pub(crate) name_range: TextRange,
    /// Its PHP.
    pub(crate) php: Option<SyntaxNode>,
    /// All of its text, if it's nothing but that.
    pub(crate) text: Option<String>,
    /// Has an echo in its text.
    pub(crate) echoes: bool,
    /// No value: it's `true`.
    pub(crate) bare: bool,
}

/// The attributes in a tag.
pub(crate) fn tag_attributes(tag: &SyntaxNode) -> Vec<TagAttribute> {
    let mut out = Vec::new();
    for attr in tag.children().filter(|c| c.kind() == BLADE_ATTR) {
        let Some(name) = attr.token(BLADE_ATTR_NAME) else {
            continue;
        };
        let php = attr
            .children()
            .find(|c| crate::body::is_expr_kind(c.kind()));
        let spelled = name.text();
        let key = if spelled == ":" {
            // `:$name`
            let Some(variable) = &php else { continue };
            text(variable).trim_start_matches('$').to_owned()
        } else {
            stir_syntax::blade_attr_key(spelled).to_owned()
        };
        let bound = spelled == ":" || stir_syntax::blade_attr_is_bound(spelled);
        let echoes = attr.children().any(|c| c.kind() == BLADE_ATTR_ECHO);
        let words: String = attr
            .tokens()
            .iter()
            .filter(|t| t.kind() == BLADE_ATTR_TEXT)
            .map(|t| t.text().to_owned())
            .collect();
        out.push(TagAttribute {
            key,
            name_range: name.text_range(),
            php: if bound { php } else { None },
            text: (!bound && !echoes).then_some(words),
            echoes,
            bare: !attr.has_token(EQ) && spelled != ":",
        });
    }
    out
}

/// The name in a tag.
pub(crate) fn tag_name(tag: &SyntaxNode) -> String {
    tag.token(BLADE_TAG_NAME)
        .map(|t| t.text().to_owned())
        .unwrap_or_default()
}

/// Is this tag a slot, by what Laravel looks for first? One that closes itself isn't.
pub(crate) fn tag_is_slot(tag: &SyntaxNode) -> bool {
    let name = tag_name(tag);
    (name == "slot" || name.starts_with("slot:"))
        && tag.token(BLADE_TAG_END).is_none_or(|t| t.text() == ">")
}

/// Is this a name that a template could read as a variable?
fn is_variable_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// What a tag with these attributes gives its component: see [`DataPart::Attribute`].
fn read_tag_data(tag: &SyntaxNode, out: &mut Vec<DataPart>) {
    if tag.children().any(|c| c.kind() == BLADE_ATTR_ECHO) {
        out.push(DataPart::AttributeBag);
    }
    for attr in tag_attributes(tag) {
        if attr.key == "attributes" {
            out.push(DataPart::AttributeBag);
            continue;
        }
        let value = match &attr.php {
            Some(php) => AttributeValue::Php(php.text_range()),
            None if attr.bare => AttributeValue::True,
            None => AttributeValue::Text,
        };
        // the data has it by this name, while it's by the other in `$attributes`, so that
        // `@props` doesn't get to what's a different variable
        let camel = stir_syntax::blade_camel(&attr.key);
        if is_variable_name(&camel) {
            out.push(DataPart::Attribute {
                name: SmolStr::new(&camel),
                value: value.clone(),
                kept_by_props: camel != attr.key,
            });
        }
        if camel != attr.key && is_variable_name(&attr.key) {
            out.push(DataPart::Attribute {
                name: SmolStr::new(&attr.key),
                value,
                kept_by_props: false,
            });
        }
    }
}

/// `<x-slot:name>`, `<x-slot name="name">`, `<x-slot :name="'name'">`: `Err` if it's worked out.
fn slot_name(tag: &SyntaxNode) -> Result<Option<SmolStr>, ()> {
    let name = tag_name(tag);
    if let Some(inline) = name.strip_prefix("slot:") {
        return Ok(Some(SmolStr::new(stir_syntax::blade_camel(inline))));
    }
    let attrs = tag_attributes(tag);
    let Some(attr) = attrs.iter().find(|a| a.key == "name") else {
        return Ok(None);
    };
    match (&attr.php, &attr.text) {
        (Some(php), _) => plain_string(php).map(|s| Some(SmolStr::new(&s))).ok_or(()),
        (None, Some(words)) => Ok(Some(SmolStr::new(words))),
        _ => Err(()),
    }
}

/// Laravel's `formatClassName`: `forms.text-input` is `Forms\\TextInput`.
fn component_class_path(tag: &str) -> String {
    let mut out = String::new();
    for piece in tag.split('.') {
        if !out.is_empty() {
            out.push('\\');
        }
        let camel = stir_syntax::blade_camel(piece);
        let mut chars = camel.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    out
}

/// Laravel's `Str::kebab`.
fn kebab(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 2);
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                out.push('-');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// Laravel's alias for a class that wasn't given one.
fn default_alias(class: &str) -> String {
    if let Some((_, rest)) = class.split_once("\\View\\Components\\") {
        return rest.split('\\').map(kebab).collect::<Vec<_>>().join(":");
    }
    kebab(class.rsplit('\\').next().unwrap_or(class))
}

/// The variables a template gets from a custom directive it uses, as far as they can be read from
/// what the directive compiles to: `None` if that can't be told.
pub(crate) type DirectiveVars = Option<Vec<SmolStr>>;

/// Could this file have anything in it for [`view_sites`]?
fn might_name_views(text: &str) -> bool {
    const NEEDLES: &[&str] = &[
        "view", "View", "markdown", "Mail::", "Content", "composer", "creator", "Blade::",
        "layout", "Layout", "template", "Template", "omponent",
    ];
    NEEDLES.iter().any(|n| text.contains(n))
}

/// Does `name` match `pattern`, with `*` for any text, as `Str::is` has it?
fn matches_pattern(pattern: &str, name: &str) -> bool {
    let mut parts = pattern.split('*');
    let first = parts.next().unwrap_or("");
    let Some(mut rest) = name.strip_prefix(first) else {
        return false;
    };
    let parts: Vec<&str> = parts.collect();
    let Some((last, middle)) = parts.split_last() else {
        return rest.is_empty();
    };
    for part in middle {
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    rest.ends_with(last)
}

fn normal_view_name(name: &str) -> String {
    name.replace('/', ".")
}

/// The dotted name of the template at `path`.
pub(crate) fn view_name_of(path: &std::path::Path) -> Option<String> {
    let path = path.to_string_lossy().replace('\\', "/");
    let (_, relative) = path.rsplit_once("/resources/views/")?;
    let name = relative.strip_suffix(".blade.php")?;
    Some(normal_view_name(name))
}

fn text(node: &SyntaxNode) -> String {
    node.text().to_string().trim().to_owned()
}

fn plain_string(node: &SyntaxNode) -> Option<String> {
    if node.kind() != STRING_LITERAL {
        return None;
    }
    let spelled = text(node);
    if spelled.starts_with('"') && spelled.contains('$') {
        return None;
    }
    Some(unquote(&spelled))
}

/// The expressions in a call's arguments, by the position or the name of the parameter. `None` if
/// there's a spread, which could be any of them.
fn args_by_param(call: &SyntaxNode, params: &[&str]) -> Option<Vec<Option<SyntaxNode>>> {
    let mut out = vec![None; params.len()];
    let Some(list) = call.child(ARG_LIST) else {
        return Some(out);
    };
    let mut position = 0;
    for arg in list.children() {
        match arg.kind() {
            ARG => {
                if arg.has_token(ELLIPSIS) {
                    return None;
                }
                if let Some(slot) = out.get_mut(position) {
                    *slot = arg.children().last();
                }
                position += 1;
            }
            NAMED_ARG => {
                let name = arg.name_text();
                if let Some(at) = params.iter().position(|p| Some(*p) == name.as_deref()) {
                    out[at] = arg.children().last();
                }
            }
            _ => {}
        }
    }
    Some(out)
}

fn method_name(call: &SyntaxNode) -> Option<String> {
    let fetch = call.first_child()?;
    fetch
        .children()
        .find(|c| c.kind() == NAME)
        .map(|n| text(&n))
}

fn receiver_of(call: &SyntaxNode) -> Option<SyntaxNode> {
    call.first_child()?.first_child()
}

/// `view()`, `app('view')`, `$this->app['view']`, `$view`, `$viewFactory`: what's likely to be the
/// factory of views, going by how it's written.
fn looks_like_factory(receiver: &str) -> bool {
    let lower = receiver.to_ascii_lowercase();
    lower == "view()"
        || lower.contains("'view'")
        || lower.contains("\"view\"")
        || lower.ends_with("$view")
        || lower.contains("viewfactory")
        || lower.contains("view\\factory")
}

/// Is `class`, as resolved, the facade of views? Not a model of the project that's named `View`.
fn is_view_facade(class: &str) -> bool {
    class == "View" || class.ends_with("\\Facades\\View")
}

/// The templates an expression names, as far as that's written.
fn read_targets(expr: &SyntaxNode, out: &mut Vec<Target>, consumed: &mut Vec<TextRange>) {
    match expr.kind() {
        STRING_LITERAL => {
            if let Some(name) = plain_string(expr) {
                consumed.push(expr.text_range());
                // text instead of a name, for `@each`
                if name.contains('|') {
                    return;
                }
                match name.rsplit_once("::") {
                    // a package's, or a namespace the project adds for its own: going by the rest
                    // of the name, as there's no telling where that one is
                    Some((_, rest)) => {
                        out.push(Target::Pattern(format!("*.{}", normal_view_name(rest))));
                    }
                    None => out.push(Target::Name(normal_view_name(&name))),
                }
            } else {
                out.push(prefix_pattern(&text(expr)));
            }
        }
        INTERP_STRING => out.push(prefix_pattern(&text(expr))),
        PAREN_EXPR => {
            for inner in expr.children() {
                read_targets(&inner, out, consumed);
            }
        }
        ARRAY_EXPR => {
            for elem in expr.children().filter(|c| c.kind() == ARRAY_ELEM) {
                match elem.children().last() {
                    Some(value) if !elem.has_token(ELLIPSIS) => {
                        read_targets(&value, out, consumed);
                    }
                    _ => out.push(Target::Pattern("*".to_owned())),
                }
            }
        }
        TERNARY_EXPR | SHORT_TERNARY_EXPR => {
            let parts: Vec<_> = expr
                .children()
                .filter(|c| crate::body::is_expr_kind(c.kind()))
                .collect();
            // not the condition, unless it's the value as well
            let skip = usize::from(expr.kind() == TERNARY_EXPR);
            for part in parts.iter().skip(skip) {
                read_targets(part, out, consumed);
            }
        }
        BIN_EXPR if expr.has_token(DOT) => {
            let left = expr.first_child();
            match left.as_ref().and_then(leftmost_text) {
                Some(prefix) => {
                    out.push(Target::Pattern(format!("{}*", normal_view_name(&prefix))))
                }
                None => out.push(Target::Pattern("*".to_owned())),
            }
        }
        // `@component(Alert::class)`: a class, not a template
        CLASS_CONST_FETCH_EXPR => {}
        _ => out.push(Target::Pattern("*".to_owned())),
    }
}

/// The text a concatenation starts with.
fn leftmost_text(expr: &SyntaxNode) -> Option<String> {
    match expr.kind() {
        BIN_EXPR if expr.has_token(DOT) => expr.first_child().as_ref().and_then(leftmost_text),
        PAREN_EXPR => expr.first_child().as_ref().and_then(leftmost_text),
        _ => plain_string(expr),
    }
}

/// `"notifications.{$type}"`: all of those it could be, by what it starts with.
fn prefix_pattern(spelled: &str) -> Target {
    let inner = spelled.trim_start_matches('"');
    let end = inner.find(['$', '{']).unwrap_or(inner.len());
    Target::Pattern(format!("{}*", normal_view_name(&inner[..end])))
}

/// What `View::share` is given, as far as it's written.
fn read_shared(key: &SyntaxNode, out: &mut Vec<SmolStr>) {
    match key.kind() {
        STRING_LITERAL => out.extend(plain_string(key).map(|k| SmolStr::new(&k))),
        ARRAY_EXPR => {
            for elem in key.children().filter(|c| c.kind() == ARRAY_ELEM) {
                let name = elem.first_child().filter(|_| elem.has_token(FAT_ARROW));
                out.extend(
                    name.as_ref()
                        .and_then(plain_string)
                        .map(|k| SmolStr::new(&k)),
                );
            }
        }
        _ => {}
    }
}

/// `$data` in `view('name', $data)` and the like.
fn read_data(expr: &SyntaxNode, out: &mut Vec<DataPart>) {
    match expr.kind() {
        PAREN_EXPR => {
            for inner in expr.children() {
                read_data(&inner, out);
            }
        }
        ARRAY_EXPR => {
            for elem in expr.children().filter(|c| c.kind() == ARRAY_ELEM) {
                let parts: Vec<_> = elem
                    .children()
                    .filter(|c| crate::body::is_expr_kind(c.kind()))
                    .collect();
                if elem.has_token(ELLIPSIS) {
                    out.extend(parts.first().map(|p| DataPart::FromType(p.text_range())));
                    continue;
                }
                match parts.as_slice() {
                    // no key: not a variable, as its key is a number
                    [_] => {}
                    [key, value] => match plain_string(key) {
                        Some(name) => {
                            out.push(DataPart::Pair(SmolStr::new(&name), value.text_range()));
                        }
                        None if key.kind() == LITERAL => {}
                        None => out.push(DataPart::NotUnderstood),
                    },
                    _ => out.push(DataPart::NotUnderstood),
                }
            }
        }
        CALL_EXPR => {
            let name = expr
                .children()
                .find(|c| c.kind() == NAME_EXPR)
                .map(|n| text(&n))
                .unwrap_or_default();
            let name = name.trim_start_matches('\\').to_ascii_lowercase();
            let args: Vec<SyntaxNode> = expr
                .child(ARG_LIST)
                .map(|l| {
                    l.children()
                        .filter(|a| a.kind() == ARG)
                        .filter_map(|a| {
                            if a.has_token(ELLIPSIS) {
                                out.push(DataPart::NotUnderstood);
                            }
                            a.children().last()
                        })
                        .collect()
                })
                .unwrap_or_default();
            match name.as_str() {
                "compact" => {
                    for arg in &args {
                        read_compact(arg, out);
                    }
                }
                "array_merge" => {
                    for arg in &args {
                        read_data(arg, out);
                    }
                }
                _ => out.push(DataPart::FromType(expr.text_range())),
            }
        }
        BIN_EXPR if expr.has_token(PLUS) => {
            for inner in expr
                .children()
                .filter(|c| crate::body::is_expr_kind(c.kind()))
            {
                read_data(&inner, out);
            }
        }
        LITERAL if text(expr).eq_ignore_ascii_case("null") => {}
        _ => out.push(DataPart::FromType(expr.text_range())),
    }
}

fn read_compact(arg: &SyntaxNode, out: &mut Vec<DataPart>) {
    match arg.kind() {
        STRING_LITERAL => match plain_string(arg) {
            Some(name) => out.push(DataPart::Compact(SmolStr::new(&name), arg.text_range())),
            None => out.push(DataPart::NotUnderstood),
        },
        ARRAY_EXPR => {
            for elem in arg.children().filter(|c| c.kind() == ARRAY_ELEM) {
                match elem.children().last() {
                    Some(value) if !elem.has_token(ELLIPSIS) && !elem.has_token(FAT_ARROW) => {
                        read_compact(&value, out);
                    }
                    _ => out.push(DataPart::NotUnderstood),
                }
            }
        }
        _ => out.push(DataPart::NotUnderstood),
    }
}

/// The name of the variable `->withFooBar($x)` gives: `fooBar`.
fn with_variable(method: &str) -> Option<SmolStr> {
    let rest = method.strip_prefix("with")?;
    let mut chars = rest.chars();
    let first = chars.next()?;
    if !first.is_uppercase() {
        return None;
    }
    let mut name = String::with_capacity(rest.len());
    name.extend(first.to_lowercase());
    name.push_str(chars.as_str());
    Some(SmolStr::new(&name))
}

/// `->with('a', $b)` and `->with([...])`.
fn read_with(call: &SyntaxNode, data: &mut Vec<DataPart>) {
    let Some(args) = args_by_param(call, &["key", "value"]) else {
        data.push(DataPart::NotUnderstood);
        return;
    };
    match (&args[0], &args[1]) {
        (Some(key), value) if key.kind() == STRING_LITERAL => match plain_string(key) {
            Some(name) => {
                let at = value
                    .as_ref()
                    .map_or_else(|| key.text_range(), |v| v.text_range());
                data.push(DataPart::Pair(SmolStr::new(&name), at));
            }
            None => data.push(DataPart::NotUnderstood),
        },
        (Some(array), None) => read_data(array, data),
        _ => data.push(DataPart::NotUnderstood),
    }
}

/// Is this the method call `at` is the receiver of? Gives that call.
fn call_on(at: &SyntaxNode) -> Option<SyntaxNode> {
    let fetch = at.parent()?;
    if fetch.first_child().as_ref() != Some(at) {
        return None;
    }
    let call = fetch.parent()?;
    let is_call = matches!(call.kind(), METHOD_CALL_EXPR | NULLSAFE_METHOD_CALL_EXPR)
        && call.first_child().as_ref() == Some(&fetch);
    is_call.then_some(call)
}

/// What's done with a view: adds to what `data` says from a chain of calls on it. Does anything
/// else get hold of it, which could add what can't be seen from here?
fn read_view_chain(view: &SyntaxNode, data: &mut Vec<DataPart>) -> bool {
    let mut at = view.clone();
    while let Some(call) = call_on(&at) {
        let name = method_name(&call).unwrap_or_default();
        match name.to_ascii_lowercase().as_str() {
            "with" => read_with(&call, data),
            // what's rendered is a string: nothing can add to it from here on
            "render" | "tohtml" | "__tostring" | "rendersections" => return false,
            "witherrors" | "name" | "getname" | "getdata" | "getpath" | "fragment"
            | "fragments" | "fragmentif" | "fragmentsif" => {}
            _ => match with_variable(&name) {
                Some(variable) => {
                    let value = args_by_param(&call, &["value"])
                        .and_then(|mut a| a.remove(0))
                        .map_or_else(|| call.text_range(), |v| v.text_range());
                    data.push(DataPart::Pair(variable, value));
                }
                // a macro of the project's, or Livewire's `->layout()`
                None => return true,
            },
        }
        at = call;
    }
    escapes(&at)
}

/// Is a view that's this expression used in a place other than where it's finished with?
fn escapes(expr: &SyntaxNode) -> bool {
    let Some(parent) = expr.parent() else {
        return true;
    };
    match parent.kind() {
        RETURN_STMT | ECHO_STMT | ECHO_TAG_STMT | EXPR_STMT | CAST_EXPR | ARROW_FN_EXPR => false,
        BIN_EXPR => !parent.has_token(DOT),
        PAREN_EXPR | TERNARY_EXPR | SHORT_TERNARY_EXPR | MATCH_EXPR | MATCH_ARM => escapes(&parent),
        _ => true,
    }
}

/// The name of the class around a node, resolved.
fn enclosing_class(db: &dyn Db, file: SourceFile, node: &SyntaxNode) -> Option<String> {
    let class = node.ancestors().find(|n| n.kind() == CLASS_DECL)?;
    let name = class.name_text()?;
    let items = stir_hir::file_items(db, file);
    let scope = items.scope_at(node.text_range().start());
    Some(scope.resolve_class(&name).as_str().to_owned())
}

/// The name of the method or function a node is in.
fn enclosing_method(node: &SyntaxNode) -> Option<String> {
    node.ancestors()
        .find(|n| matches!(n.kind(), METHOD_DECL | FUNCTION_DECL))
        .and_then(|m| m.name_text())
}

const VIEW_WORDS: &[&str] = &[
    "view",
    "template",
    "layout",
    "markdown",
    "render",
    "extends",
    "component",
    "include",
];

fn says_view(word: &str) -> bool {
    let lower = word.to_ascii_lowercase();
    // `$response->assertViewIs('home')`, which only looks
    if lower.starts_with("assert") {
        return false;
    }
    VIEW_WORDS.iter().any(|w| lower.contains(w)) || lower == "make"
}

/// Is a template's name, as this string, somewhere that looks to use it as a view? `route('home')`
/// isn't, but `->layout('home')`, `#[Layout('home')]`, `$view = 'home'` and `'view' => 'home'` are.
fn in_view_like_place(literal: &SyntaxNode) -> bool {
    let Some(parent) = literal.parent() else {
        return false;
    };
    match parent.kind() {
        ARG | NAMED_ARG => {
            let Some(call) = parent.parent().and_then(|l| l.parent()) else {
                return false;
            };
            match call.kind() {
                CALL_EXPR => call
                    .children()
                    .find(|c| c.kind() == NAME_EXPR)
                    .is_some_and(|n| says_view(&text(&n))),
                METHOD_CALL_EXPR | NULLSAFE_METHOD_CALL_EXPR | STATIC_CALL_EXPR => {
                    method_name(&call).is_some_and(|n| says_view(&n))
                }
                NEW_EXPR | ATTRIBUTE => says_view(&text(&call)),
                _ => false,
            }
        }
        ARRAY_ELEM => {
            let key = parent
                .children()
                .filter(|c| crate::body::is_expr_kind(c.kind()))
                .collect::<Vec<_>>();
            match key.as_slice() {
                [key, value] => value == literal && says_view(&text(key)),
                // a list of them, which is what the array is passed as
                [_] => parent
                    .parent()
                    .is_some_and(|array| in_view_like_place(&array)),
                _ => false,
            }
        }
        ASSIGN_EXPR => parent.first_child().is_some_and(|t| says_view(&text(&t))),
        PROPERTY_DECL => says_view(&text(&parent)),
        RETURN_STMT => enclosing_method(literal).is_some_and(|m| says_view(&m)),
        _ => parent.name_text().is_some_and(|n| says_view(&n)),
    }
}

/// The arguments of a directive by their position: the template, and what it's given.
fn directive_positions(directive: &str) -> Option<(usize, usize)> {
    match directive {
        "include" | "includeif" | "includefirst" | "includeisolated" | "extends"
        | "extendsfirst" | "component" | "componentfirst" => Some((0, 1)),
        "includewhen" | "includeunless" => Some((1, 2)),
        _ => None,
    }
}

/// The variables what a custom directive compiles to assigns, if that can be read: a closure with
/// nothing in it but text to return. More variables than there are does no harm.
fn variables_compiled_to(handler: &SyntaxNode) -> DirectiveVars {
    if !matches!(handler.kind(), CLOSURE_EXPR | ARROW_FN_EXPR) {
        return None;
    }
    let calls_something = handler.descendants().any(|n| {
        matches!(
            n.kind(),
            CALL_EXPR | METHOD_CALL_EXPR | NULLSAFE_METHOD_CALL_EXPR | STATIC_CALL_EXPR | NEW_EXPR
        )
    });
    let body = handler.text().to_string();
    let anything = [
        "$$", "${", "extract(", "foreach", "list(", "[$", " as $", "include", "global ",
    ]
    .iter()
    .any(|n| body.contains(n));
    if calls_something || anything {
        return None;
    }
    let bytes = body.as_bytes();
    let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut found: Vec<SmolStr> = Vec::new();
    for (at, _) in body.match_indices('$') {
        let len = bytes[at + 1..].iter().take_while(|b| word(**b)).count();
        let after = body[at + 1 + len..].trim_start();
        let assigned =
            after.starts_with('=') && !after.starts_with("==") && !after.starts_with("=>");
        if len > 0 && assigned {
            let name = SmolStr::new(&body[at + 1..at + 1 + len]);
            if !found.contains(&name) {
                found.push(name);
            }
        }
    }
    Some(found)
}

struct Reader<'a> {
    db: &'a dyn Db,
    file: SourceFile,
    out: ViewSites,
    /// The strings that have been read as what they are.
    consumed: Vec<TextRange>,
    /// The components open at this point.
    open_components: Vec<OpenComponent>,
}

/// A component whose end hasn't been seen.
#[derive(Debug, Clone, Copy)]
enum OpenComponent {
    Site(usize),
    /// It has no site, as with a class: which of them gets its slots can't be told.
    Unknown,
    /// `@component('package::name')`, which is that package's.
    Package,
}

impl Reader<'_> {
    /// Adds what's said about these templates.
    fn add_opens(&mut self, targets: Vec<Target>, why: OpenReason) {
        self.out.opens.extend(targets.into_iter().map(|t| (t, why)));
    }

    /// A site for `target`: those that are names, with anything else being what it opens.
    fn add_site(
        &mut self,
        target: &SyntaxNode,
        kind: SiteKind,
        data: Vec<DataPart>,
        escapes: bool,
        range: TextRange,
    ) {
        let mut targets = Vec::new();
        read_targets(target, &mut targets, &mut self.consumed);
        let mut names = Vec::new();
        for t in targets {
            match t {
                Target::Name(name) => names.push(name),
                pattern => self.out.opens.push((pattern, OpenReason::DynamicView)),
            }
        }
        if !names.is_empty() {
            self.out.sites.push(Site {
                names,
                kind,
                data,
                escapes,
                range,
            });
        }
    }

    /// A view made where it's known to be, with what's done with it after.
    fn view_call(
        &mut self,
        call: &SyntaxNode,
        target: Option<SyntaxNode>,
        given: Option<SyntaxNode>,
    ) {
        let Some(target) = target else { return };
        // a component's own view: what it's given is its class's public properties
        if matches!(target.kind(), CLOSURE_EXPR | ARROW_FN_EXPR) {
            return;
        }
        let class = enclosing_class(self.db, self.file, call);
        let render = enclosing_method(call).as_deref() == Some("render");
        let kind = if render || class.is_some() {
            SiteKind::ComponentView { class, render }
        } else {
            SiteKind::Plain { mail: false }
        };
        let mut data = Vec::new();
        if let Some(given) = &given {
            read_data(given, &mut data);
        }
        let escapes = read_view_chain(call, &mut data);
        self.add_site(&target, kind, data, escapes, call.text_range());
    }

    fn factory_method(&mut self, call: &SyntaxNode, name: &str) -> bool {
        match name.to_ascii_lowercase().as_str() {
            "make" => {
                let Some(mut args) = args_by_param(call, &["view", "data", "mergeData"]) else {
                    self.add_opens(vec![Target::Pattern("*".into())], OpenReason::DynamicView);
                    return true;
                };
                let (target, given) = (args[0].take(), args[1].take());
                self.view_call(call, target, given);
            }
            "first" => {
                let Some(mut args) = args_by_param(call, &["views", "data", "mergeData"]) else {
                    self.add_opens(vec![Target::Pattern("*".into())], OpenReason::DynamicView);
                    return true;
                };
                let (target, given) = (args[0].take(), args[1].take());
                self.view_call(call, target, given);
            }
            "renderwhen" | "renderunless" => {
                let Some(mut args) = args_by_param(call, &["condition", "view", "data"]) else {
                    self.add_opens(vec![Target::Pattern("*".into())], OpenReason::DynamicView);
                    return true;
                };
                let (target, given) = (args[1].take(), args[2].take());
                self.view_call(call, target, given);
            }
            "rendereach" => {
                if let Some(Some(target)) = args_by_param(call, &["view"]).map(|mut a| a.remove(0))
                {
                    let mut targets = Vec::new();
                    read_targets(&target, &mut targets, &mut self.consumed);
                    self.add_opens(targets, OpenReason::SourceNotUnderstood);
                }
            }
            "share" => {
                self.add_opens(vec![Target::Pattern("*".into())], OpenReason::Shared);
                if let Some(Some(key)) =
                    args_by_param(call, &["key", "value"]).map(|mut a| a.remove(0))
                {
                    read_shared(&key, &mut self.out.shared);
                }
            }
            "composer" | "creator" => {
                let mut targets = Vec::new();
                match args_by_param(call, &["views", "callback"]).map(|mut a| a.remove(0)) {
                    Some(Some(views)) => read_targets(&views, &mut targets, &mut self.consumed),
                    _ => targets.push(Target::Pattern("*".into())),
                }
                self.add_opens(targets, OpenReason::Composer);
            }
            "composers" | "creators" => {
                let mut targets = Vec::new();
                match args_by_param(call, &["composers"]).map(|mut a| a.remove(0)) {
                    Some(Some(map)) if map.kind() == ARRAY_EXPR => {
                        for elem in map.children().filter(|c| c.kind() == ARRAY_ELEM) {
                            match elem.children().last() {
                                Some(views) if !elem.has_token(ELLIPSIS) => {
                                    read_targets(&views, &mut targets, &mut self.consumed);
                                }
                                _ => targets.push(Target::Pattern("*".into())),
                            }
                        }
                    }
                    _ => targets.push(Target::Pattern("*".into())),
                }
                self.add_opens(targets, OpenReason::Composer);
            }
            _ => return false,
        }
        true
    }

    fn call(&mut self, call: &SyntaxNode) {
        let name = call
            .children()
            .find(|c| c.kind() == NAME_EXPR)
            .map(|n| text(&n))
            .unwrap_or_default();
        if name.trim_start_matches('\\').eq_ignore_ascii_case("view") {
            let Some(mut args) = args_by_param(call, &["view", "data", "mergeData"]) else {
                self.add_opens(vec![Target::Pattern("*".into())], OpenReason::DynamicView);
                return;
            };
            let (target, given) = (args[0].take(), args[1].take());
            self.view_call(call, target, given);
        }
    }

    fn static_call(&mut self, call: &SyntaxNode) {
        let Some(method) = method_name(call) else {
            return;
        };
        let class = call
            .first_child()
            .and_then(|f| f.first_child())
            .map(|c| text(&c))
            .unwrap_or_default();
        let class = class.trim_start_matches('\\');
        let base = class.rsplit('\\').next().unwrap_or(class);
        let items = stir_hir::file_items(self.db, self.file);
        let resolved = items
            .scope_at(call.text_range().start())
            .resolve_class(class);
        if is_view_facade(resolved.as_str()) {
            self.factory_method(call, &method);
            return;
        }
        let method = method.to_ascii_lowercase();
        match (base, method.as_str()) {
            ("Route", "view") => {
                if let Some(mut args) = args_by_param(call, &["uri", "view", "data"]) {
                    self.consumed
                        .extend(args[0].as_ref().map(|u| u.text_range()));
                    if let Some(target) = args[1].take() {
                        let mut data = Vec::new();
                        if let Some(given) = args[2].take() {
                            read_data(&given, &mut data);
                        }
                        let range = call.text_range();
                        self.add_site(&target, SiteKind::Plain { mail: false }, data, false, range);
                    }
                }
            }
            ("Response", "view") => self.response_view(call),
            ("Mail", "send" | "sendnow" | "plain" | "queue") => {
                let Some(mut args) = args_by_param(call, &["view", "data"]) else {
                    return;
                };
                let Some(target) = args[0].take() else { return };
                if !matches!(
                    target.kind(),
                    STRING_LITERAL | INTERP_STRING | ARRAY_EXPR | BIN_EXPR
                ) {
                    return;
                }
                let mut data = Vec::new();
                if method != "queue"
                    && let Some(given) = args[1].take()
                {
                    read_data(&given, &mut data);
                }
                let range = call.text_range();
                self.add_site(&target, SiteKind::Plain { mail: true }, data, false, range);
            }
            ("Blade", "directive" | "if") => self.directive_declared(call, method == "if"),
            ("Blade", "component" | "components") => self.component_registered(call, &method),
            ("Blade", "componentnamespace" | "anonymouscomponentnamespace") => {
                self.namespace_registered(call, &method);
            }
            ("Blade", "anonymouscomponentpath") => self.component_path_registered(call),
            ("Blade", "extend" | "precompiler" | "preparestringsforcompilationusing") => {
                self.add_opens(
                    vec![Target::Pattern("*".into())],
                    OpenReason::CustomDirective,
                );
            }
            _ => {}
        }
    }

    /// What this says the class or view is, if it's written out.
    fn component_named(&mut self, expr: &SyntaxNode) -> Option<ComponentTarget> {
        if let Some(name) = plain_string(expr) {
            self.consumed.push(expr.text_range());
            return Some(if name.contains('\\') {
                ComponentTarget::Class(name.trim_start_matches('\\').to_owned())
            } else {
                ComponentTarget::View(normal_view_name(&name))
            });
        }
        if expr.kind() == CLASS_CONST_FETCH_EXPR {
            let class = expr.first_child()?;
            let items = stir_hir::file_items(self.db, self.file);
            let resolved = items
                .scope_at(expr.text_range().start())
                .resolve_class(&text(&class));
            return Some(ComponentTarget::Class(resolved.as_str().to_owned()));
        }
        None
    }

    /// `Blade::component(Alert::class, 'alert')`, `Blade::component('alert', Alert::class)`,
    /// `Blade::component(Alert::class)`, `Blade::components([...])`, as Laravel reads them.
    fn component_registered(&mut self, call: &SyntaxNode, method: &str) {
        let not_read = |reader: &mut Self| {
            // what's made of it isn't known: any template may be rendered this way
            reader.add_opens(vec![Target::Pattern("*".into())], OpenReason::Component);
        };
        if method == "components" {
            let Some(mut args) = args_by_param(call, &["components", "prefix"]) else {
                return not_read(self);
            };
            let Some(list) = args[0].take().filter(|a| a.kind() == ARRAY_EXPR) else {
                return not_read(self);
            };
            let prefix = args[1].as_ref().map_or(Some(String::new()), plain_string);
            for elem in list.children().filter(|c| c.kind() == ARRAY_ELEM) {
                let parts: Vec<_> = elem
                    .children()
                    .filter(|c| crate::body::is_expr_kind(c.kind()))
                    .collect();
                match parts.as_slice() {
                    [class] => self.alias_of(class, None, prefix.as_deref()),
                    [class, alias] => self.alias_of(class, Some(alias), prefix.as_deref()),
                    _ => not_read(self),
                }
            }
            return;
        }
        let Some(mut args) = args_by_param(call, &["class", "alias", "prefix"]) else {
            return not_read(self);
        };
        let Some(class) = args[0].take() else { return };
        let prefix = args[2].as_ref().map_or(Some(String::new()), plain_string);
        self.alias_of(&class, args[1].as_ref(), prefix.as_deref());
    }

    fn alias_of(&mut self, class: &SyntaxNode, alias: Option<&SyntaxNode>, prefix: Option<&str>) {
        let alias_text = alias.and_then(plain_string);
        let swapped = alias_text.as_deref().is_some_and(|a| a.contains('\\'))
            || alias.is_some_and(|a| a.kind() == CLASS_CONST_FETCH_EXPR);
        let (target, name) = if swapped {
            (
                alias.and_then(|a| self.component_named(a)),
                plain_string(class),
            )
        } else {
            (self.component_named(class), alias_text)
        };
        let (Some(target), Some(prefix)) = (target, prefix) else {
            self.add_opens(vec![Target::Pattern("*".into())], OpenReason::Component);
            return;
        };
        let name = match (name, &target, alias) {
            (Some(name), ..) => name,
            (None, ComponentTarget::Class(class), None) => default_alias(class),
            (None, ComponentTarget::View(view), None) => default_alias(view),
            _ => {
                self.add_opens(vec![Target::Pattern("*".into())], OpenReason::Component);
                return;
            }
        };
        let name = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}-{name}")
        };
        self.out
            .registrations
            .push(Registration::Alias(name, target));
    }

    /// `Blade::componentNamespace('Ns', 'prefix')`, `Blade::anonymousComponentNamespace('dir', 'prefix')`.
    fn namespace_registered(&mut self, call: &SyntaxNode, method: &str) {
        let args = if method == "componentnamespace" {
            args_by_param(call, &["namespace", "prefix"])
        } else {
            args_by_param(call, &["directory", "prefix"])
        };
        let read = args.and_then(|a| {
            let from = a[0].as_ref().and_then(plain_string)?;
            match &a[1] {
                Some(prefix) => Some((from, plain_string(prefix)?)),
                None => Some((from.clone(), from)),
            }
        });
        let Some((from, prefix)) = read else {
            self.add_opens(vec![Target::Pattern("*".into())], OpenReason::Component);
            return;
        };
        if method == "componentnamespace" {
            self.out
                .registrations
                .push(Registration::ClassNamespace(prefix, from));
        } else {
            let directory = normal_view_name(from.trim_matches(['.', ' ', '/']));
            self.out
                .registrations
                .push(Registration::ViewNamespace(prefix, directory));
        }
    }

    /// `Blade::anonymousComponentPath(..)`: what's there is rendered in a way that isn't followed,
    /// so it's open if it's in the project's views, and otherwise anything may be.
    fn component_path_registered(&mut self, call: &SyntaxNode) {
        let written = call
            .descendants()
            .find(|n| n.kind() == STRING_LITERAL)
            .and_then(|l| plain_string(&l));
        let under_views = written.as_deref().and_then(|w| {
            let views = w.replace('\\', "/");
            let (_, rest) = views.rsplit_once("views/")?;
            Some(normal_view_name(rest.trim_matches('/')))
        });
        let target = match under_views {
            Some(dir) if !dir.is_empty() => format!("{dir}.*"),
            _ => "*".to_owned(),
        };
        self.add_opens(vec![Target::Pattern(target)], OpenReason::Component);
    }

    /// `$this->loadViewComponentsAs('prefix', [Alert::class])`, which is a package's.
    fn view_components_loaded(&mut self, call: &SyntaxNode) {
        let Some(args) = args_by_param(call, &["prefix", "components"]) else {
            return;
        };
        let Some(prefix) = args[0].as_ref().and_then(plain_string) else {
            return;
        };
        let Some(list) = args[1].as_ref().filter(|a| a.kind() == ARRAY_EXPR) else {
            return;
        };
        for elem in list.children().filter(|c| c.kind() == ARRAY_ELEM) {
            let parts: Vec<_> = elem
                .children()
                .filter(|c| crate::body::is_expr_kind(c.kind()))
                .collect();
            match parts.as_slice() {
                [class] => self.alias_of(class, None, Some(&prefix)),
                [alias, class] => self.alias_of(class, Some(alias), Some(&prefix)),
                _ => {}
            }
        }
    }

    /// `<x-name ...>`.
    fn tag(&mut self, tag: &SyntaxNode) {
        if tag_is_slot(tag) {
            match slot_name(tag) {
                Ok(Some(name)) => self.slot(DataPart::Slot(name)),
                Ok(None) => {}
                Err(()) => self.slot(DataPart::NotUnderstood),
            }
            return;
        }
        let mut data = Vec::new();
        read_tag_data(tag, &mut data);
        let index = self.out.sites.len();
        self.out.sites.push(Site {
            names: Vec::new(),
            kind: SiteKind::Component {
                tag: Some(tag_name(tag)),
            },
            data,
            escapes: false,
            range: tag.text_range(),
        });
        if tag.token(BLADE_TAG_END).is_some_and(|t| t.text() == ">") {
            self.open_components.push(OpenComponent::Site(index));
        }
    }

    /// A slot of what's the current component here.
    fn slot(&mut self, part: DataPart) {
        match self.open_components.last().copied() {
            Some(OpenComponent::Package) => {}
            Some(OpenComponent::Site(index)) => self.out.sites[index].data.push(part),
            Some(OpenComponent::Unknown) | None => match part {
                DataPart::Slot(name) | DataPart::Pair(name, _) => {
                    self.out.stray_slots.push(name);
                }
                // that could be anything of any component: nothing to go by there
                _ => self.add_opens(
                    vec![Target::Pattern("*".into())],
                    OpenReason::SourceNotUnderstood,
                ),
            },
        }
    }

    /// `Blade::directive('name', fn ($e) => "...")`.
    fn directive_declared(&mut self, call: &SyntaxNode, is_condition: bool) {
        let Some(args) = args_by_param(call, &["name", "handler"]) else {
            return;
        };
        match args[0].as_ref().and_then(plain_string) {
            Some(name) => {
                let vars = if is_condition {
                    Some(Vec::new())
                } else {
                    args[1].as_ref().and_then(variables_compiled_to)
                };
                self.out
                    .custom_directives
                    .push((name.to_ascii_lowercase(), vars));
            }
            // named when it runs: it can be any
            None => self.add_opens(
                vec![Target::Pattern("*".into())],
                OpenReason::CustomDirective,
            ),
        }
    }

    fn response_view(&mut self, call: &SyntaxNode) {
        let Some(mut args) = args_by_param(call, &["view", "data"]) else {
            return;
        };
        let Some(target) = args[0].take() else { return };
        let mut data = Vec::new();
        if let Some(given) = args[1].take() {
            read_data(&given, &mut data);
        }
        let range = call.text_range();
        self.add_site(&target, SiteKind::Plain { mail: false }, data, false, range);
    }

    fn method_call(&mut self, call: &SyntaxNode) {
        let Some(method) = method_name(call) else {
            return;
        };
        let Some(receiver) = receiver_of(call) else {
            return;
        };
        let spelled = text(&receiver);
        let lower = method.to_ascii_lowercase();
        // `$compiler->directive(..)`: on nothing else is it likely to have a name and a closure
        if lower == "directive" {
            self.directive_declared(call, false);
            return;
        }
        if lower == "loadviewcomponentsas" {
            self.view_components_loaded(call);
            return;
        }
        // `$blade->component(..)`, for whichever name the compiler goes by there
        let says_blade = {
            let receiver = spelled.to_ascii_lowercase();
            receiver.contains("blade") || receiver.contains("compiler")
        };
        if says_blade {
            match lower.as_str() {
                "component" | "components" => {
                    self.component_registered(call, &lower);
                    return;
                }
                "componentnamespace" | "anonymouscomponentnamespace" => {
                    self.namespace_registered(call, &lower);
                    return;
                }
                "anonymouscomponentpath" => {
                    self.component_path_registered(call);
                    return;
                }
                _ => {}
            }
        }
        if looks_like_factory(&spelled)
            && lower != "composer"
            && lower != "creator"
            && self.factory_method(call, &method)
        {
            return;
        }
        match lower.as_str() {
            // `->creator()` is a likely name for a relation: with two arguments it's this
            "composer" | "creator" | "composers" | "creators" => {
                let count = call
                    .child(ARG_LIST)
                    .map_or(0, |l| l.children().filter(|a| a.kind() == ARG).count());
                let wanted = if lower.ends_with('s') { 1 } else { 2 };
                if count >= wanted {
                    self.factory_method(call, &method);
                }
            }
            "view" if spelled == "response()" || spelled == "Response" => self.response_view(call),
            "view" | "markdown" | "text" => self.mail_call(call, &receiver),
            _ => {}
        }
    }

    /// `->view()`, `->markdown()` or `->text()` on something that may be a mailable.
    fn mail_call(&mut self, call: &SyntaxNode, receiver: &SyntaxNode) {
        let Some(mut args) = args_by_param(call, &["view", "data"]) else {
            return;
        };
        let Some(target) = args[0].take() else { return };
        let root = chain_root(receiver);
        let on_this = text(&root) == "$this";
        let named = matches!(target.kind(), STRING_LITERAL | ARRAY_EXPR);
        if !named {
            // `$converter->markdown($text)` is no view: only if it's surely on a mail
            let spelled = text(receiver).to_ascii_lowercase();
            let class = call.ancestors().find(|n| n.kind() == CLASS_DECL);
            let in_a_mail = class.is_some_and(|c| {
                let whole = text(&c);
                let head = whole.split('{').next().unwrap_or_default();
                head.contains("Mailable") || head.contains("Notification")
            });
            if !(on_this && in_a_mail) && !spelled.contains("mail") {
                return;
            }
        }
        let mut data = Vec::new();
        if let Some(given) = args[1].take() {
            read_data(&given, &mut data);
        }
        let mut escapes = read_mail_chain(call, &mut data);
        let class = if on_this {
            if let Some(class) = call.ancestors().find(|n| n.kind() == CLASS_DECL) {
                escapes |= adds_to_itself_elsewhere(&class, call);
            }
            enclosing_class(self.db, self.file, call)
        } else {
            None
        };
        let kind = SiteKind::Mail {
            receiver: (!on_this).then(|| receiver.text_range()),
            class,
        };
        self.add_site(&target, kind, data, escapes, call.text_range());
    }

    /// `new Content(view: 'name', with: [...])`.
    fn new_expr(&mut self, new: &SyntaxNode) {
        let class = new
            .children()
            .find(|c| matches!(c.kind(), NAME_EXPR | NAME))
            .map(|n| text(&n))
            .unwrap_or_default();
        if class.rsplit('\\').next() != Some("Content") {
            return;
        }
        let params = ["view", "html", "text", "markdown", "with", "htmlString"];
        let Some(mut args) = args_by_param(new, &params) else {
            self.add_opens(vec![Target::Pattern("*".into())], OpenReason::DynamicView);
            return;
        };
        let mut data = Vec::new();
        if let Some(with) = args[4].take() {
            read_data(&with, &mut data);
        }
        let class = enclosing_class(self.db, self.file, new);
        for target in [&args[0], &args[1], &args[2], &args[3]]
            .into_iter()
            .flatten()
        {
            let kind = SiteKind::Mail {
                receiver: None,
                class: class.clone(),
            };
            self.add_site(target, kind, data.clone(), false, new.text_range());
        }
    }

    fn directive(&mut self, node: &SyntaxNode) {
        let Some(name) = directive_name(node) else {
            return;
        };
        let args: Vec<SyntaxNode> = node
            .child(ARG_LIST)
            .map(|l| l.children().filter(|a| a.kind() == ARG).collect())
            .unwrap_or_default();
        let value = |i: usize| args.get(i).and_then(|a| a.children().last());
        match name.as_str() {
            "slot" => {
                let part = match (value(0), value(1)) {
                    (Some(slot), content) => match plain_string(&slot) {
                        Some(slot) => match content {
                            Some(content) => {
                                DataPart::Pair(SmolStr::new(&slot), content.text_range())
                            }
                            None => DataPart::Slot(SmolStr::new(&slot)),
                        },
                        None => DataPart::NotUnderstood,
                    },
                    (None, _) => return,
                };
                self.slot(part);
                return;
            }
            "endcomponent" | "endcomponentfirst" | "endcomponentclass" => {
                self.open_components.pop();
                return;
            }
            _ => {}
        }
        if name == "each" {
            let range = node.text_range();
            if let Some(target) = value(0) {
                let variable = value(2)
                    .and_then(|v| plain_string(&v))
                    .map(|v| SmolStr::new(&v));
                self.add_site(
                    &target,
                    SiteKind::Each { variable },
                    Vec::new(),
                    false,
                    range,
                );
            }
            // what's shown for no items, given nothing
            if let Some(empty) = value(3) {
                self.add_site(
                    &empty,
                    SiteKind::Plain { mail: false },
                    Vec::new(),
                    false,
                    range,
                );
            }
            return;
        }
        let Some((at, given_at)) = directive_positions(&name) else {
            return;
        };
        let Some(target) = value(at) else { return };
        let mut data = Vec::new();
        if let Some(given) = value(given_at) {
            read_data(&given, &mut data);
        }
        if name.starts_with("component") {
            let index = self.out.sites.len();
            let kind = SiteKind::Component { tag: None };
            self.add_site(&target, kind, data, false, node.text_range());
            self.open_components.push(if self.out.sites.len() > index {
                OpenComponent::Site(index)
            } else if plain_string(&target).is_some_and(|name| name.contains("::")) {
                OpenComponent::Package
            } else {
                OpenComponent::Unknown
            });
            return;
        }
        let kind = SiteKind::Include {
            inherits: name != "includeisolated",
        };
        self.add_site(&target, kind, data, false, node.text_range());
    }

    /// What's in a template's text, or the like, that decides if it can be closed at all.
    fn template_text(&mut self, root: &SyntaxNode) {
        for element in root.descendants_with_tokens() {
            let SyntaxElement::Token(t) = element else {
                continue;
            };
            match t.kind() {
                VARIABLE if t.text() == "$this" => self.out.uses_this = true,
                INLINE_HTML => {
                    let html = t.text();
                    for (at, name) in crate::body::template::at_names(html) {
                        let after = html[at + 1 + name.len()..].trim_start_matches(' ');
                        if after.starts_with('(') {
                            self.out.at_calls.push(name.clone());
                        }
                        self.out.at_names.push(name);
                    }
                }
                BLADE_DIRECTIVE_NAME => {
                    let name = t.text().trim_start_matches('@').to_ascii_lowercase();
                    self.out.at_names.push(name);
                }
                _ => {}
            }
        }
        self.out.at_names.sort();
        self.out.at_names.dedup();
        self.out.at_calls.sort();
        self.out.at_calls.dedup();
        self.out.props = declared_props(root);
    }
}

/// What the file's `@props` declares, if that's known and nothing comes before it to see the
/// variables it takes away.
fn declared_props(root: &SyntaxNode) -> Option<Vec<SmolStr>> {
    let directive = root
        .children()
        .find(|n| n.kind() == BLADE_DIRECTIVE && directive_name(n).as_deref() == Some("props"))?;
    let read_before = root
        .descendants_with_tokens()
        .any(|e| e.kind() == VARIABLE && e.text_range().end() <= directive.text_range().start());
    let again = root
        .descendants()
        .filter(|n| n.kind() == BLADE_DIRECTIVE && directive_name(n).as_deref() == Some("props"))
        .count()
        > 1;
    if read_before || again {
        return None;
    }
    let args = directive.child(ARG_LIST)?;
    let array = args
        .children()
        .filter(|a| a.kind() == ARG)
        .find_map(|a| a.children().find(|c| c.kind() == ARRAY_EXPR))?;
    let mut names = Vec::new();
    for elem in array.children().filter(|c| c.kind() == ARRAY_ELEM) {
        if elem.has_token(ELLIPSIS) {
            return None;
        }
        let name = elem
            .children()
            .find(|c| crate::body::is_expr_kind(c.kind()))
            .as_ref()
            .and_then(plain_string)?;
        names.push(SmolStr::new(&name));
    }
    Some(names)
}

/// What a chain of calls starts from.
fn chain_root(expr: &SyntaxNode) -> SyntaxNode {
    let mut at = expr.clone();
    while matches!(at.kind(), METHOD_CALL_EXPR | NULLSAFE_METHOD_CALL_EXPR) {
        let Some(receiver) = receiver_of(&at) else {
            break;
        };
        at = receiver;
    }
    at
}

/// The `with`s of a chain with a mail's `->view()` in it, before or after that. Mailables return
/// themselves, so this doesn't get lost, unless it's what's made of the mail elsewhere.
fn read_mail_chain(call: &SyntaxNode, data: &mut Vec<DataPart>) -> bool {
    let mut outermost = call.clone();
    while let Some(outer) = call_on(&outermost) {
        outermost = outer;
    }
    let mut at = outermost;
    loop {
        if !matches!(at.kind(), METHOD_CALL_EXPR | NULLSAFE_METHOD_CALL_EXPR) {
            return false;
        }
        if method_name(&at).is_some_and(|m| m.eq_ignore_ascii_case("with")) {
            read_with(&at, data);
        }
        let Some(receiver) = receiver_of(&at) else {
            return false;
        };
        at = receiver;
    }
}

/// Does the class `call` is in add to what its mail is given, other than in the chain of `call`?
fn adds_to_itself_elsewhere(class: &SyntaxNode, call: &SyntaxNode) -> bool {
    let mut outermost = call.clone();
    while let Some(outer) = call_on(&outermost) {
        outermost = outer;
    }
    class.descendants().any(|n| {
        let is_with = matches!(n.kind(), METHOD_CALL_EXPR | NULLSAFE_METHOD_CALL_EXPR)
            && method_name(&n).is_some_and(|m| m.eq_ignore_ascii_case("with"))
            && text(&chain_root(&n)) == "$this";
        let in_the_chain = n.ancestors().any(|a| a == outermost);
        let touches_data = n.kind() == PROPERTY_FETCH_EXPR
            && n.children()
                .find(|c| c.kind() == NAME)
                .is_some_and(|c| text(&c) == "viewData");
        (is_with && !in_the_chain) || touches_data
    })
}

/// What `file` says about views, as written: see the top of this module.
#[salsa::tracked(returns(ref))]
pub fn view_sites(db: &dyn Db, ws: Workspace, file: SourceFile) -> ViewSites {
    let is_template = stir_syntax::is_blade_path(file.path(db));
    if !is_template && !might_name_views(file.text(db)) {
        return ViewSites::default();
    }
    let names = view_names(db, ws);
    let root = stir_db::parse(db, file).syntax();
    let mut reader = Reader {
        db,
        file,
        out: ViewSites::default(),
        consumed: Vec::new(),
        open_components: Vec::new(),
    };
    for node in root.descendants() {
        match node.kind() {
            CALL_EXPR => reader.call(&node),
            METHOD_CALL_EXPR | NULLSAFE_METHOD_CALL_EXPR => reader.method_call(&node),
            STATIC_CALL_EXPR => reader.static_call(&node),
            NEW_EXPR => reader.new_expr(&node),
            BLADE_DIRECTIVE => reader.directive(&node),
            BLADE_TAG => reader.tag(&node),
            BLADE_TAG_CLOSE if !tag_name(&node).starts_with("slot") => {
                reader.open_components.pop();
            }
            _ => {}
        }
    }
    // what's only a template's name in a place that looks like it's used as one
    for node in root.descendants().filter(|n| n.kind() == STRING_LITERAL) {
        if reader.consumed.contains(&node.text_range()) {
            continue;
        }
        let Some(value) = plain_string(&node) else {
            continue;
        };
        let name = normal_view_name(&value);
        if names.contains(&name) && in_view_like_place(&node) {
            reader
                .out
                .opens
                .push((Target::Name(name), OpenReason::NamedElsewhere));
        }
    }
    if is_template {
        reader.template_text(&root);
    }
    reader.out
}

/// Which templates are rendered where, and what's known to open them.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ViewIndex {
    /// For each template, the files that render it by name, in the workspace's order.
    sources: FxHashMap<String, Vec<SourceFile>>,
    /// Names and patterns that are open, and why.
    opens: Vec<(Target, OpenReason)>,
    custom_directives: FxHashMap<String, DirectiveVars>,
    /// Those of packages that can't be read. Taken to be up to nothing if given nothing, as with
    /// `@livewireStyles`, for which that's most of them.
    package_directives: FxHashSet<String>,
    /// On a cycle of templates including each other.
    recursive: FxHashSet<String>,
    /// The slots given to each class's component, by its name in lower case, with whether there's
    /// one that's worked out.
    class_slots: FxHashMap<String, (Vec<SmolStr>, bool)>,
    /// The slots that can't be told what they're given to, and might be any component's.
    stray_slots: Vec<SmolStr>,
    /// `<x-dynamic-component>` is used, which goes with whatever it's given to any component.
    dynamic_components: bool,
}

/// The templates a site renders.
fn site_names(site: &Site, components: &ComponentIndex) -> Vec<String> {
    if let SiteKind::Component { tag: Some(tag) } = &site.kind {
        return match components.resolve(tag) {
            ComponentTarget::View(view) => vec![view],
            _ => Vec::new(),
        };
    }
    site.names.clone()
}

impl ViewIndex {
    /// What a tag that's for a class, or for what can't be told, gives by its slots.
    fn note_tag_slots(&mut self, site: &Site, components: &ComponentIndex) {
        let SiteKind::Component { tag } = &site.kind else {
            return;
        };
        let target = match tag {
            Some(tag) => components.resolve(tag),
            None => ComponentTarget::View(String::new()),
        };
        // a package's, unless a namespace is the project's own: which goes by the rest of the name
        if target == ComponentTarget::Unknown
            && let Some((_, rest)) = tag.as_deref().and_then(|t| t.split_once("::"))
        {
            let pattern = format!("*.components.{}*", normal_view_name(rest));
            self.opens
                .push((Target::Pattern(pattern), OpenReason::RenderedByPackage));
            return;
        }
        let class = match target {
            ComponentTarget::View(_) => return,
            ComponentTarget::Class(class) => Some(class.to_ascii_lowercase()),
            ComponentTarget::Dynamic => {
                self.dynamic_components = true;
                None
            }
            ComponentTarget::Unknown => None,
        };
        for part in &site.data {
            let name = match part {
                DataPart::Slot(name) | DataPart::Pair(name, _) => Some(name.clone()),
                DataPart::NotUnderstood => None,
                _ => continue,
            };
            match (&class, name) {
                (Some(class), Some(name)) => {
                    self.class_slots
                        .entry(class.clone())
                        .or_default()
                        .0
                        .push(name);
                }
                (Some(class), None) => {
                    self.class_slots.entry(class.clone()).or_default().1 = true;
                }
                (None, Some(name)) => self.stray_slots.push(name),
                (None, None) => self.dynamic_components = true,
            }
        }
    }

    fn open_reason(&self, name: &str) -> Option<OpenReason> {
        self.opens
            .iter()
            .filter(|(target, _)| match target {
                Target::Name(n) => n == name,
                Target::Pattern(p) => matches_pattern(p, name),
            })
            .map(|(_, why)| *why)
            .min()
    }
}

/// What packages say about the project's templates.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PackageViews {
    /// What's opened by name.
    opens: Vec<(Target, OpenReason)>,
    /// The custom directives that can be read.
    directives: Vec<(String, Vec<SmolStr>)>,
    /// Those that can't: see [`ViewIndex::package_directives`].
    unread_directives: FxHashSet<String>,
    /// What's given to templates it renders by name, without types.
    calls: FxHashMap<String, Vec<ViewCall>>,
    /// The variables it shares with all templates.
    shared: Vec<SmolStr>,
    /// What it registers for component tags.
    registrations: Vec<Registration>,
    /// The tags its templates have, none of them of a package by name, such as `app-layout`, and
    /// the slots in each: it may be the project's component that those are for.
    tags: Vec<(String, Vec<DataPart>)>,
}

/// See [`PackageViews`]: `view('auth.login')` from a controller's trait, read as it's written, as
/// a package's code isn't checked. Only what's rendered by a name counts: a name worked out in a
/// package is no more likely to be this template than any other. On its own, as nothing here
/// changes with an edit of the project.
#[salsa::tracked(returns(ref))]
pub fn package_views(db: &dyn Db, ws: Workspace) -> PackageViews {
    let names = view_names(db, ws);
    let mut out = PackageViews::default();
    for &file in ws.files(db) {
        if file.origin(db) != FileOrigin::Vendor {
            continue;
        }
        let source = file.text(db);
        // a package's template that's written for an application's component
        if stir_syntax::is_blade_path(file.path(db)) {
            if source.contains("<x") {
                let root = stir_db::parse(db, file).syntax();
                let mut reader = Reader {
                    db,
                    file,
                    out: ViewSites::default(),
                    consumed: Vec::new(),
                    open_components: Vec::new(),
                };
                for node in root.descendants() {
                    match node.kind() {
                        BLADE_TAG => reader.tag(&node),
                        BLADE_TAG_CLOSE if !tag_name(&node).starts_with("slot") => {
                            reader.open_components.pop();
                        }
                        _ => {}
                    }
                }
                for site in reader.out.sites {
                    if let SiteKind::Component { tag: Some(tag) } = site.kind
                        && !tag.contains("::")
                    {
                        let slots = site
                            .data
                            .into_iter()
                            .filter(|p| matches!(p, DataPart::Slot(_) | DataPart::NotUnderstood))
                            .collect();
                        out.tags.push((tag, slots));
                    }
                }
            }
            continue;
        }
        let renders =
            !names.is_empty() && (source.contains("view(") || source.contains("View::make("));
        let registers = [
            "omponent(",
            "omponents(",
            "omponentNamespace(",
            "ComponentsAs(",
        ]
        .iter()
        .any(|call| source.contains(call));
        if !renders && !registers && !source.contains("directive(") && !source.contains("share(") {
            continue;
        }
        let root = stir_db::parse(db, file).syntax();
        let mut reader = Reader {
            db,
            file,
            out: ViewSites::default(),
            consumed: Vec::new(),
            open_components: Vec::new(),
        };
        for node in root.descendants() {
            match node.kind() {
                CALL_EXPR => reader.call(&node),
                STATIC_CALL_EXPR => reader.static_call(&node),
                METHOD_CALL_EXPR => {
                    let Some(method) = method_name(&node) else {
                        continue;
                    };
                    if method == "directive" {
                        reader.directive_declared(&node, false);
                    } else if method == "share"
                        && receiver_of(&node).is_some_and(|r| looks_like_factory(&text(&r)))
                    {
                        reader.factory_method(&node, &method);
                    } else if method.eq_ignore_ascii_case("loadViewComponentsAs") {
                        reader.view_components_loaded(&node);
                    } else if method_says_a_compiler(&node) {
                        match method.to_ascii_lowercase().as_str() {
                            lower @ ("component" | "components") => {
                                reader.component_registered(&node, lower);
                            }
                            lower @ ("componentnamespace" | "anonymouscomponentnamespace") => {
                                reader.namespace_registered(&node, lower);
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        out.registrations.extend(reader.out.registrations);
        for (name, vars) in reader.out.custom_directives {
            match vars {
                Some(vars) => out.directives.push((name, vars)),
                // what renders a component, which has its own variables
                None if name == "livewire" => {}
                None => {
                    out.unread_directives.insert(name);
                }
            }
        }
        out.shared.extend(reader.out.shared);
        for (target, why) in reader.out.opens {
            if matches!(&target, Target::Name(n) if names.contains(n)) {
                out.opens.push((target, why));
            }
        }
        for site in reader.out.sites {
            let known: Vec<String> = site
                .names
                .iter()
                .filter(|n| names.contains(*n))
                .cloned()
                .collect();
            if known.is_empty() {
                continue;
            }
            let mut call = ViewCall::of(known.clone());
            if site.escapes {
                call.open = Some(OpenReason::RenderedByPackage);
            }
            // a package's component, or the like: its class says the rest
            if matches!(site.kind, SiteKind::ComponentView { render: true, .. }) {
                call.open = Some(OpenReason::Component);
            }
            for part in &site.data {
                match part {
                    DataPart::Pair(name, _) | DataPart::Compact(name, _) => {
                        give(&mut call.vars, name, &Ty::untyped(), false);
                    }
                    DataPart::FromType(_) | DataPart::NotUnderstood => {
                        call.open = Some(OpenReason::RenderedByPackage);
                    }
                    DataPart::Attribute { name, .. } | DataPart::Slot(name) => {
                        give(&mut call.vars, name, &Ty::untyped(), false);
                    }
                    DataPart::AttributeBag => call.open = Some(OpenReason::RenderedByPackage),
                }
            }
            for name in known {
                out.calls.entry(name).or_default().push(call.clone());
            }
        }
    }
    out.shared.sort();
    out.shared.dedup();
    out
}

/// `$blade->component(..)`, as a package may have it.
fn method_says_a_compiler(call: &SyntaxNode) -> bool {
    receiver_of(call).is_some_and(|r| {
        let receiver = text(&r).to_ascii_lowercase();
        receiver.contains("blade") || receiver.contains("compiler")
    })
}

/// What the workspace has for tags to stand for.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ComponentIndex {
    /// The classes in `app/View/Components`, by the path under it: `Forms\\Input`.
    classes: BTreeMap<String, String>,
    /// `Blade::component`, by the tag it's for.
    aliases: BTreeMap<String, ComponentTarget>,
    /// `Blade::componentNamespace`: its prefix and namespace.
    class_namespaces: BTreeMap<String, String>,
    /// `Blade::anonymousComponentNamespace`: its prefix and directory, as a view's name.
    view_namespaces: BTreeMap<String, String>,
    /// The names of templates under `components/`.
    views: FxHashSet<String>,
}

/// See [`ComponentIndex`].
#[salsa::tracked(returns(ref))]
pub fn component_index(db: &dyn Db, ws: Workspace) -> ComponentIndex {
    let mut index = ComponentIndex {
        views: view_names(db, ws).clone(),
        ..ComponentIndex::default()
    };
    let packages = package_views(db, ws);
    for registration in &packages.registrations {
        index.register(registration);
    }
    // of the application itself, and not a module with a folder of the same name
    let components_dir = ws
        .files(db)
        .iter()
        .filter(|f| f.origin(db) == FileOrigin::Source)
        .filter_map(|f| {
            let path = f.path(db).to_string_lossy().replace('\\', "/");
            let root = path.strip_suffix("/bootstrap/app.php")?.to_owned();
            Some(format!("{root}/app/View/Components/"))
        })
        .min_by_key(String::len);
    for &file in ws.files(db) {
        if file.origin(db) != FileOrigin::Source {
            continue;
        }
        let path = file.path(db).to_string_lossy().replace('\\', "/");
        if let Some(under) = components_dir
            .as_ref()
            .and_then(|dir| path.strip_prefix(dir.as_str()))
            .and_then(|under| under.strip_suffix(".php"))
        {
            let items = stir_hir::file_items(db, file);
            let base = under.rsplit('/').next().unwrap_or(under);
            let class = items.classes.iter().find(|c| c.name.short() == base);
            if let Some(class) = class {
                index
                    .classes
                    .insert(under.replace('/', "\\"), class.name.as_str().to_owned());
            }
        }
        for registration in &view_sites(db, ws, file).registrations {
            index.register(registration);
        }
    }
    index
}

impl ComponentIndex {
    fn register(&mut self, registration: &Registration) {
        match registration {
            Registration::Alias(tag, target) => {
                self.aliases.insert(tag.clone(), target.clone());
            }
            Registration::ClassNamespace(prefix, namespace) => {
                self.class_namespaces
                    .insert(prefix.clone(), namespace.clone());
            }
            Registration::ViewNamespace(prefix, directory) => {
                self.view_namespaces
                    .insert(prefix.clone(), directory.clone());
            }
        }
    }

    /// The first template there is for `name` in `directory`, in the order Laravel looks for them.
    fn view_in(&self, directory: &str, name: &str) -> Option<String> {
        let plain = format!("{directory}.{name}");
        let last = name.rsplit(['.', ':']).next().unwrap_or(name);
        [
            plain.clone(),
            format!("{plain}.index"),
            format!("{plain}.{last}"),
        ]
        .into_iter()
        .find(|candidate| self.views.contains(candidate))
    }

    /// `ComponentTagCompiler::componentClass`, up to what can be known without running anything.
    fn resolve(&self, tag: &str) -> ComponentTarget {
        if tag == "dynamic-component" && !self.aliases.contains_key(tag) {
            return ComponentTarget::Dynamic;
        }
        if let Some(target) = self.aliases.get(tag) {
            return target.clone();
        }
        if let Some((prefix, rest)) = tag.split_once("::") {
            if let Some(namespace) = self.class_namespaces.get(prefix) {
                // it may be there, by a package's autoloading: that's for who looks at the class
                return ComponentTarget::Class(format!(
                    "{namespace}\\{}",
                    component_class_path(rest)
                ));
            }
            if let Some(directory) = self.view_namespaces.get(prefix)
                && let Some(view) = self.view_in(directory, rest)
            {
                return ComponentTarget::View(view);
            }
            return ComponentTarget::Unknown;
        }
        let path = component_class_path(tag);
        let last = path.rsplit('\\').next().unwrap_or(&path);
        let class = self
            .classes
            .get(&path)
            .or_else(|| self.classes.get(&format!("{path}\\{last}")));
        if let Some(class) = class {
            return ComponentTarget::Class(class.clone());
        }
        match self.view_in("components", tag) {
            Some(view) => ComponentTarget::View(view),
            None => ComponentTarget::Unknown,
        }
    }

    /// What a tag can be written as, for completing it.
    pub fn tag_names(&self) -> Vec<String> {
        let mut out: Vec<String> = self.aliases.keys().cloned().collect();
        for path in self.classes.keys() {
            out.push(path.split('\\').map(kebab).collect::<Vec<_>>().join("."));
        }
        for view in &self.views {
            let Some(name) = view.strip_prefix("components.") else {
                continue;
            };
            let name = name.strip_suffix(".index").unwrap_or(name);
            let name = match name.rsplit_once('.') {
                Some((dir, last)) if dir.rsplit('.').next() == Some(last) => dir,
                _ => name,
            };
            out.push(name.to_owned());
        }
        out.sort();
        out.dedup();
        out
    }
}

/// Does the project tell the container how to make something by hand, as a parameter's value?
/// `when(X::class)->needs('$name')->give(..)`, and `Component::resolveComponentsUsing`.
#[salsa::tracked]
pub(crate) fn container_is_told(db: &dyn Db, ws: Workspace) -> bool {
    ws.files(db).iter().any(|file| {
        file.origin(db) == FileOrigin::Source && {
            let text = file.text(db);
            text.contains("->needs(") || text.contains("resolveComponentsUsing")
        }
    })
}

/// What the tag `tag` stands for.
pub fn resolve_component(db: &dyn Db, ws: Workspace, tag: &str) -> ComponentTarget {
    component_index(db, ws).resolve(tag)
}

/// See [`ViewIndex`].
#[salsa::tracked(returns(ref))]
pub fn view_index(db: &dyn Db, ws: Workspace) -> ViewIndex {
    let mut index = ViewIndex::default();
    let mut edges: FxHashMap<&str, Vec<String>> = FxHashMap::default();
    let mut template_names: Vec<(SourceFile, String)> = Vec::new();
    let components = component_index(db, ws);
    for &file in ws.files(db) {
        if file.origin(db) != FileOrigin::Source {
            continue;
        }
        // the templates, compiled: what they do is known from the templates themselves
        if (file.path(db).to_string_lossy().replace('\\', "/")).contains("/storage/framework/") {
            continue;
        }
        let sites = view_sites(db, ws, file);
        index.opens.extend(sites.opens.iter().cloned());
        index
            .custom_directives
            .extend(sites.custom_directives.iter().cloned());
        let own_name = stir_syntax::is_blade_path(file.path(db))
            .then(|| view_name_of(file.path(db)))
            .flatten();
        if let Some(name) = &own_name {
            template_names.push((file, name.clone()));
        }
        index.stray_slots.extend(sites.stray_slots.iter().cloned());
        for site in &sites.sites {
            for name in site_names(site, components) {
                let files = index.sources.entry(name).or_default();
                if files.last() != Some(&file) {
                    files.push(file);
                }
            }
            index.note_tag_slots(site, components);
        }
    }
    for (file, name) in &template_names {
        let sites = view_sites(db, ws, *file);
        let to = edges.entry(name.as_str()).or_default();
        for site in &sites.sites {
            to.extend(site_names(site, components).into_iter());
        }
    }
    index.recursive = on_cycles(&edges);
    let packages = package_views(db, ws);
    for (tag, slots) in &packages.tags {
        // whether it's rendered at all isn't known, nor what with
        match components.resolve(tag) {
            ComponentTarget::View(view) => {
                index
                    .opens
                    .push((Target::Name(view), OpenReason::RenderedByPackage));
            }
            ComponentTarget::Class(class) => {
                let entry = index
                    .class_slots
                    .entry(class.to_ascii_lowercase())
                    .or_default();
                for slot in slots {
                    match slot {
                        DataPart::Slot(name) => entry.0.push(name.clone()),
                        _ => entry.1 = true,
                    }
                }
            }
            ComponentTarget::Dynamic | ComponentTarget::Unknown => {}
        }
    }
    index.stray_slots.sort();
    index.stray_slots.dedup();
    index.opens.extend(packages.opens.iter().cloned());
    for (name, vars) in &packages.directives {
        index
            .custom_directives
            .entry(name.clone())
            .or_insert_with(|| Some(vars.clone()));
    }
    index.package_directives = (packages.unread_directives.iter())
        .filter(|name| !index.custom_directives.contains_key(*name))
        .cloned()
        .collect();
    index
}

/// The templates that lead back to themselves. Tarjan's algorithm, on what's a small graph, with
/// recursion only as deep as a chain of includes is.
fn on_cycles(edges: &FxHashMap<&str, Vec<String>>) -> FxHashSet<String> {
    struct State<'a> {
        edges: &'a FxHashMap<&'a str, Vec<String>>,
        index: FxHashMap<&'a str, u32>,
        low: FxHashMap<&'a str, u32>,
        on_stack: FxHashSet<&'a str>,
        stack: Vec<&'a str>,
        next: u32,
        found: FxHashSet<String>,
    }
    fn visit<'a>(state: &mut State<'a>, node: &'a str) {
        state.index.insert(node, state.next);
        state.low.insert(node, state.next);
        state.next += 1;
        state.stack.push(node);
        state.on_stack.insert(node);
        let mut low = state.low[node];
        let edges = state.edges;
        if let Some(to) = edges.get(node) {
            for next in to {
                let next = next.as_str();
                if !edges.contains_key(next) {
                    continue;
                }
                if let Some(&seen) = state.index.get(next) {
                    if state.on_stack.contains(next) {
                        low = low.min(seen);
                    }
                } else {
                    visit(state, next);
                    low = low.min(state.low[next]);
                }
            }
        }
        state.low.insert(node, low);
        if low == state.index[node] {
            let mut members = Vec::new();
            while let Some(member) = state.stack.pop() {
                state.on_stack.remove(member);
                members.push(member);
                if member == node {
                    break;
                }
            }
            let self_loop = edges
                .get(node)
                .is_some_and(|to| to.iter().any(|n| n == node));
            if members.len() > 1 || self_loop {
                state.found.extend(members.iter().map(|m| (*m).to_owned()));
            }
        }
    }
    let mut state = State {
        edges,
        index: FxHashMap::default(),
        low: FxHashMap::default(),
        on_stack: FxHashSet::default(),
        stack: Vec::new(),
        next: 0,
        found: FxHashSet::default(),
    };
    let mut nodes: Vec<&&str> = edges.keys().collect();
    nodes.sort();
    for node in nodes {
        if !state.index.contains_key(*node) {
            visit(&mut state, node);
        }
    }
    state.found
}

/// One render of some templates, with nothing about where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewCall {
    names: Vec<String>,
    open: Option<OpenReason>,
    vars: BTreeMap<SmolStr, ViewVar>,
    /// Those of `vars` that `@props` takes away, as they're in `$attributes`.
    in_attributes: Vec<SmolStr>,
    /// What this makes of what it renders.
    component: CallComponent,
    /// See [`ViewData::spread`].
    spread: bool,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
enum CallComponent {
    #[default]
    None,
    /// By a tag or `@component`, with nothing of a class.
    Anonymous,
    /// The view of this class, which is a component.
    Class(String),
}

impl ViewCall {
    fn of(names: Vec<String>) -> ViewCall {
        ViewCall {
            names,
            open: None,
            vars: BTreeMap::new(),
            in_attributes: Vec::new(),
            component: CallComponent::None,
            spread: false,
        }
    }
}

/// What's given to a template, in the form it can be used in, as the type goes: it's on its own
/// there, with nothing around it of the class it came from.
pub(crate) fn given_type(ty: &Ty) -> Ty {
    let ty = widen_literals(ty);
    let ty = match ty.kind() {
        TyKind::SelfType { class, .. } => class.clone(),
        _ => ty,
    };
    let mut usable = !matches!(ty.kind(), TyKind::Error | TyKind::Mixed);
    ty.walk(&mut |t| {
        if matches!(t.kind(), TyKind::Param { .. } | TyKind::SelfType { .. }) {
            usable = false;
        }
        usable
    });
    if usable { ty } else { Ty::untyped() }
}

/// The fields of a type that's an array of known keys: `None` for anything else.
fn fields_of(ty: &Ty) -> Option<Vec<(SmolStr, Ty, bool)>> {
    let mut out = Vec::new();
    for member in ty.members() {
        match member.kind() {
            TyKind::Shape(shape) if shape.sealed => {
                for field in &shape.fields {
                    if let stir_types::ShapeKey::Str(name) = &field.key {
                        out.push((name.clone(), field.ty.clone(), field.optional));
                    }
                }
            }
            TyKind::Null => {}
            TyKind::Array { .. } | TyKind::List { .. } if ty.is_union() => return None,
            _ => return None,
        }
    }
    // in a union, any of them may not be there
    if ty.is_union() {
        for field in &mut out {
            field.2 = true;
        }
    }
    Some(out)
}

/// Sets a variable to be what's given, over what it was.
fn give(vars: &mut BTreeMap<SmolStr, ViewVar>, name: &SmolStr, ty: &Ty, optional: bool) {
    let ty = given_type(ty);
    vars.insert(name.clone(), ViewVar { ty, optional });
}

/// A class's name, if that's what a type is.
fn class_of(ty: &Ty) -> Option<String> {
    let ty = match ty.kind() {
        TyKind::SelfType { class, .. } => class,
        _ => ty,
    };
    match ty.kind() {
        TyKind::Class { name, .. } => Some(name.to_string()),
        _ => None,
    }
}

const MAILABLE: &str = "Illuminate\\Mail\\Mailable";
const MAIL_MESSAGE: &str = "Illuminate\\Notifications\\Messages\\MailMessage";

/// What a notification's mail message has for its template, whichever that is.
const MAIL_MESSAGE_DATA: &[&str] = &[
    "level",
    "subject",
    "greeting",
    "salutation",
    "introLines",
    "outroLines",
    "actionText",
    "actionUrl",
    "displayableActionUrl",
];

/// The templates' variables for a mail: from the class, which goes by its public properties.
fn mail_variables(ctx: &Ctx<'_>, class: &str, vars: &mut BTreeMap<SmolStr, ViewVar>) -> Option<()> {
    let ty = Ty::class(class, Vec::new());
    if ctx.is_subclass(class, MAILABLE) {
        for member in model_api::members(ctx, &ty) {
            if member.is_static
                || member.visibility != stir_hir::Visibility::Public
                || member.class.eq_ignore_ascii_case(MAILABLE)
            {
                continue;
            }
            if let MemberTy::Property { ty, .. } = member.ty {
                give(vars, &member.name, &ty, false);
            }
        }
    } else if ctx.is_subclass(class, MAIL_MESSAGE) {
        for name in MAIL_MESSAGE_DATA {
            give(vars, &SmolStr::new(name), &Ty::untyped(), false);
        }
    } else {
        return None;
    }
    Some(())
}

pub(crate) const COMPONENT: &str = "Illuminate\\View\\Component";

fn slot_type(ctx: &Ctx<'_>) -> Ty {
    if ctx.class("illuminate\\view\\componentslot").is_some() {
        Ty::class("Illuminate\\View\\ComponentSlot", Vec::new())
    } else {
        Ty::untyped()
    }
}

/// What a component's class has for its template, as `Component::data()` gets it: its public
/// properties and methods, those of its own. A method's is a callable variable that's also the
/// value of calling it, for which there's no type. `None` if this isn't a component that does
/// that as usual.
fn component_variables(
    ctx: &Ctx<'_>,
    class: &str,
    vars: &mut BTreeMap<SmolStr, ViewVar>,
) -> Option<()> {
    if !ctx.is_subclass(class, COMPONENT) {
        return None;
    }
    const IGNORED: &[&str] = &[
        "data",
        "render",
        "resolve",
        "resolveview",
        "shouldrender",
        "view",
        "withname",
        "withattributes",
        "flushcache",
        "forgetfactory",
        "forgetcomponentsresolver",
        "resolvecomponentsusing",
    ];
    let ty = Ty::class(class, Vec::new());
    let component = COMPONENT.to_ascii_lowercase();
    for member in model_api::members(ctx, &ty) {
        let own = member.class.as_str() != component;
        let lower = member.name.to_ascii_lowercase();
        // another way of giving it anything, or another kind of template
        if own && matches!(lower.as_str(), "data" | "resolveview") {
            return None;
        }
        if !own
            || member.is_static
            || member.visibility != stir_hir::Visibility::Public
            || member.name.starts_with("__")
        {
            continue;
        }
        match member.ty {
            MemberTy::Property { ty, .. } => give(vars, &member.name, &ty, false),
            MemberTy::Method(_) if !IGNORED.contains(&lower.as_str()) => {
                give(vars, &member.name, &Ty::untyped(), false);
            }
            _ => {}
        }
    }
    Some(())
}

fn message_variable(ctx: &Ctx<'_>, vars: &mut BTreeMap<SmolStr, ViewVar>) {
    let ty = if ctx.class("illuminate\\mail\\message").is_some() {
        Ty::class("Illuminate\\Mail\\Message", Vec::new())
    } else {
        Ty::untyped()
    };
    give(vars, &SmolStr::new("message"), &ty, false);
}

/// What `file` renders, with types: see the top of this module.
#[salsa::tracked(returns(ref))]
pub fn view_calls(db: &dyn Db, ws: Workspace, file: SourceFile) -> Vec<ViewCall> {
    let sites = view_sites(db, ws, file);
    if sites.sites.is_empty() {
        return Vec::new();
    }
    let index = view_index(db, ws);
    let known = view_names(db, ws);
    let components = component_index(db, ws);
    let wanted: Vec<&Site> = sites
        .sites
        .iter()
        .filter(|s| site_names(s, components).iter().any(|n| known.contains(n)))
        .collect();
    if wanted.is_empty() {
        return Vec::new();
    }
    let typed = typed_file(db, ws, file);
    let own = stir_syntax::is_blade_path(file.path(db))
        .then_some(())
        .map(|()| view_data(db, ws, file));
    let ctx = Ctx::new(db, ws);
    let mut out: Vec<ViewCall> = Vec::new();
    for site in wanted {
        let mut call = ViewCall::of(
            site_names(site, components)
                .into_iter()
                .filter(|n| known.contains(n))
                .collect(),
        );
        // a template with a cycle in it has no need of this
        if call.names.iter().all(|n| index.recursive.contains(n)) {
            call.open = Some(OpenReason::Recursive);
            push_call(&mut out, call);
            continue;
        }
        if site.escapes {
            call.open = Some(OpenReason::SourceNotUnderstood);
            push_call(&mut out, call);
            continue;
        }
        match &site.kind {
            SiteKind::Plain { mail } => {
                if *mail {
                    message_variable(&ctx, &mut call.vars);
                }
            }
            SiteKind::Include { inherits: false } => {}
            SiteKind::Component { .. } => call.component = CallComponent::Anonymous,
            SiteKind::ComponentView { class, render } => {
                let is = |ancestor| class.as_ref().is_some_and(|c| ctx.is_subclass(c, ancestor));
                if is(COMPONENT) {
                    let read = class
                        .as_ref()
                        .and_then(|c| component_variables(&ctx, c, &mut call.vars));
                    match (read, class) {
                        (Some(()), Some(class)) => {
                            call.component = CallComponent::Class(class.clone());
                        }
                        _ => call.open = Some(OpenReason::Component),
                    }
                } else if *render || is("Livewire\\Component") {
                    // Livewire's, or anything else with its own idea of what a template is given
                    call.open = Some(OpenReason::Component);
                }
                if call.open.is_some() {
                    call.vars.clear();
                    push_call(&mut out, call);
                    continue;
                }
            }
            SiteKind::Include { inherits: true } => {
                let Some(own) = own else { continue };
                // Laravel gives an error page what it gives it, whatever's reading it
                if own.open == Some(OpenReason::ErrorPage) {
                    give(
                        &mut call.vars,
                        &SmolStr::new("exception"),
                        &Ty::untyped(),
                        true,
                    );
                } else if own.open.is_some() {
                    call.open = Some(OpenReason::ParentOpen);
                    push_call(&mut out, call);
                    continue;
                }
                for (name, var) in &own.vars {
                    call.vars.insert(name.clone(), var.clone());
                }
                match typed.view_flows.get(&site.range) {
                    Some(flow) if !flow.dynamic => {
                        for (name, ty, maybe) in &flow.scope {
                            give(&mut call.vars, name, ty, *maybe);
                        }
                    }
                    // not looked at, as in code that's never run, or anything may be there
                    _ => {
                        call.open = Some(OpenReason::SourceNotUnderstood);
                        push_call(&mut out, call);
                        continue;
                    }
                }
            }
            SiteKind::Each { variable } => {
                let flow = typed.view_flows.get(&site.range);
                match (variable, flow.and_then(|f| f.each.as_ref())) {
                    (Some(variable), Some((key, value))) => {
                        give(&mut call.vars, &SmolStr::new("key"), key, false);
                        give(&mut call.vars, variable, value, false);
                    }
                    _ => {
                        call.open = Some(OpenReason::SourceNotUnderstood);
                        push_call(&mut out, call);
                        continue;
                    }
                }
            }
            SiteKind::Mail { receiver, class } => {
                let class = match (class, receiver) {
                    (Some(class), _) => Some(class.clone()),
                    (None, Some(range)) => typed.types.get(range).and_then(class_of),
                    (None, None) => None,
                };
                let is_test_case = class.as_deref().is_some_and(|c| {
                    ctx.is_subclass(c, "Illuminate\\Foundation\\Testing\\TestCase")
                        || ctx.is_subclass(c, "PHPUnit\\Framework\\TestCase")
                });
                let Some(class) = class.filter(|c| ctx.class(c).is_some()) else {
                    call.open = Some(OpenReason::SourceNotUnderstood);
                    push_call(&mut out, call);
                    continue;
                };
                if !is_test_case {
                    if mail_variables(&ctx, &class, &mut call.vars).is_none() {
                        // `->view()` on something else, that could do anything with the name
                        call.open = Some(OpenReason::SourceNotUnderstood);
                        push_call(&mut out, call);
                        continue;
                    }
                    message_variable(&ctx, &mut call.vars);
                }
            }
        }
        for part in &site.data {
            match part {
                DataPart::Pair(name, at) => {
                    let ty = typed.types.get(at).cloned().unwrap_or_else(Ty::untyped);
                    give(&mut call.vars, name, &ty, false);
                }
                DataPart::Compact(name, at) => match typed.compact_vars.get(at) {
                    Some((ty, maybe)) => give(&mut call.vars, name, ty, *maybe),
                    None => give(&mut call.vars, name, &Ty::untyped(), true),
                },
                DataPart::FromType(at) => {
                    let fields = typed.types.get(at).and_then(fields_of);
                    match fields {
                        Some(fields) => {
                            for (name, ty, optional) in fields {
                                give(&mut call.vars, &name, &ty, optional);
                            }
                        }
                        None => call.open = Some(OpenReason::SourceNotUnderstood),
                    }
                }
                DataPart::NotUnderstood => call.open = Some(OpenReason::SourceNotUnderstood),
                DataPart::Attribute {
                    name,
                    value,
                    kept_by_props,
                } => {
                    let ty = match value {
                        AttributeValue::Php(at) => {
                            typed.types.get(at).cloned().unwrap_or_else(Ty::untyped)
                        }
                        AttributeValue::Text => Ty::string(),
                        AttributeValue::True => Ty::bool(),
                    };
                    give(&mut call.vars, name, &ty, false);
                    if !kept_by_props {
                        call.in_attributes.push(name.clone());
                    }
                }
                DataPart::Slot(name) => {
                    let ty = slot_type(&ctx);
                    give(&mut call.vars, name, &ty, false);
                    call.in_attributes.retain(|n| n != name);
                }
                DataPart::AttributeBag => call.spread = true,
            }
        }
        if call.open.is_some() {
            call.vars.clear();
        }
        push_call(&mut out, call);
    }
    out
}

fn push_call(out: &mut Vec<ViewCall>, call: ViewCall) {
    if !out.contains(&call) {
        out.push(call);
    }
}

/// What each directive the project and its packages define in a way that can be read declares, by
/// its name in lower case.
pub(crate) fn directive_variables(db: &dyn Db, ws: Workspace) -> FxHashMap<String, Vec<SmolStr>> {
    view_index(db, ws)
        .custom_directives
        .iter()
        .filter_map(|(name, vars)| {
            let vars = vars.as_ref().filter(|v| !v.is_empty())?;
            Some((name.clone(), vars.clone()))
        })
        .collect()
}

/// What's given to the template `file`: see the top of this module.
#[salsa::tracked(returns(ref))]
pub fn view_data(db: &dyn Db, ws: Workspace, file: SourceFile) -> ViewData {
    let path = file.path(db);
    let Some(name) = view_name_of(path) else {
        return ViewData::opened(OpenReason::ProjectNotKnown);
    };
    if !views_are_all_known(db, ws, file) {
        return ViewData::opened(OpenReason::ProjectNotKnown);
    }
    let sites = view_sites(db, ws, file);
    if name.starts_with("vendor.") {
        return ViewData::opened(OpenReason::PublishedByPackage);
    }
    if name.starts_with("errors.") || name.contains(".errors.") {
        return ViewData::opened(OpenReason::ErrorPage);
    }
    if sites.uses_this {
        return ViewData::opened(OpenReason::UsesThis);
    }
    let index = view_index(db, ws);
    if let Some(why) = index.open_reason(&name) {
        return ViewData::opened(why);
    }
    if index.recursive.contains(&name) {
        return ViewData::opened(OpenReason::Recursive);
    }
    if sites
        .at_names
        .iter()
        .any(|d| matches!(index.custom_directives.get(d), Some(None)))
        || sites
            .at_calls
            .iter()
            .any(|d| index.package_directives.contains(d))
    {
        return ViewData::opened(OpenReason::CustomDirective);
    }
    let sources = index.sources.get(&name).map_or(&[][..], |s| s.as_slice());
    let packages = package_views(db, ws);
    let by_packages = packages.calls.get(&name).map_or(&[][..], |c| c.as_slice());
    let mut merged: BTreeMap<SmolStr, (Vec<Ty>, usize, bool)> = BTreeMap::new();
    let mut calls = 0usize;
    let mut in_attributes: BTreeMap<SmolStr, usize> = BTreeMap::new();
    let (mut component, mut spread) = (false, false);
    let ctx = Ctx::new(db, ws);
    let slot_ty = slot_type(&ctx);
    let from_project = sources
        .iter()
        .flat_map(|&source| view_calls(db, ws, source));
    for call in from_project.chain(by_packages) {
        if !call.names.contains(&name) {
            continue;
        }
        if let Some(why) = call.open {
            return ViewData::opened(why);
        }
        calls += 1;
        spread |= call.spread;
        for var in &call.in_attributes {
            *in_attributes.entry(var.clone()).or_default() += 1;
        }
        match &call.component {
            CallComponent::None => {}
            CallComponent::Anonymous => component = true,
            CallComponent::Class(class) => {
                component = true;
                let slots = index.class_slots.get(&class.to_ascii_lowercase());
                if slots.is_some_and(|(_, worked_out)| *worked_out) {
                    return ViewData::opened(OpenReason::SourceNotUnderstood);
                }
                for slot in slots.iter().flat_map(|(names, _)| names) {
                    let entry = merged
                        .entry(slot.clone())
                        .or_insert_with(|| (Vec::new(), 0, true));
                    if !entry.0.contains(&slot_ty) {
                        entry.0.push(slot_ty.clone());
                    }
                    entry.2 = true;
                }
            }
        }
        for (var, given) in &call.vars {
            let entry = merged
                .entry(var.clone())
                .or_insert_with(|| (Vec::new(), 0, false));
            if !entry.0.contains(&given.ty) {
                entry.0.push(given.ty.clone());
            }
            entry.1 += 1;
            entry.2 |= given.optional;
        }
    }
    if calls == 0 {
        return ViewData::opened(OpenReason::NoSource);
    }
    if component && index.dynamic_components {
        return ViewData::opened(OpenReason::DynamicView);
    }
    // what's in `$attributes` is no variable once `@props` has been, and without it it's all there
    match (&sites.props, spread) {
        (None, true) => return ViewData::opened(OpenReason::SourceNotUnderstood),
        (None, false) => {}
        (Some(props), _) => {
            for (var, count) in &in_attributes {
                let given = merged.get(var).map_or(0, |entry| entry.1);
                if *count == given && !props.contains(var) {
                    merged.remove(var);
                }
            }
        }
    }
    if component {
        for slot in &index.stray_slots {
            let entry = merged
                .entry(slot.clone())
                .or_insert_with(|| (Vec::new(), 0, true));
            if !entry.0.contains(&Ty::untyped()) {
                entry.0.push(Ty::untyped());
            }
            entry.2 = true;
        }
    }
    let shared = packages
        .shared
        .iter()
        .map(|name| (name.clone(), (vec![Ty::untyped()], 0, true)));
    merged.extend(shared);
    let vars = merged
        .into_iter()
        .map(|(var, (types, count, optional))| {
            let ty = if types.len() > UNION_LIMIT {
                Ty::untyped()
            } else {
                let ty = Ty::union(types);
                if ty.members().len() > UNION_LIMIT {
                    Ty::untyped()
                } else {
                    ty
                }
            };
            // a component's made to be used in more ways than it is now
            let optional = optional || count < calls || component;
            (var, ViewVar { ty, optional })
        })
        .collect();
    ViewData {
        open: None,
        vars,
        component,
        spread,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_go_by_any_text() {
        assert!(matches_pattern("*", "a.b"));
        assert!(matches_pattern("a.*", "a.b"));
        assert!(!matches_pattern("a.*", "b.a"));
        assert!(matches_pattern("a.*.c", "a.b.c"));
        assert!(!matches_pattern("a.*.c", "a.b.d"));
        assert!(matches_pattern("a", "a"));
        assert!(!matches_pattern("a", "ab"));
        assert!(matches_pattern("*.index", "posts.index"));
    }

    #[test]
    fn with_a_name_is_that_variable() {
        assert_eq!(with_variable("withFooBar").as_deref(), Some("fooBar"));
        assert_eq!(with_variable("without"), None);
        assert_eq!(with_variable("with"), None);
    }
}
