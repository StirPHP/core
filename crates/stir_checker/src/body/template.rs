//! What's particular to checking a Blade template.
//!
//! A template is top-level code whose variables mostly come from whoever renders it, which isn't
//! known. So a variable nothing declares isn't an error: it's unknown, which says nothing of what
//! can be done with it. What a directive or Laravel declares is typed, as far as the classes are
//! there to say what with.
//!
//! The conditions that stay flat in the tree (`@auth ... @endauth`, and the project's own, from
//! `Blade::if`) still make what's inside them conditional: they're followed here, in the order of
//! the statements.

use rustc_hash::FxHashSet;
use stir_syntax::{BladeFlatPart, NodeOrToken, SyntaxElement, TextSize};

use super::*;
use crate::typed::ViewFlow;
use crate::views::{ViewData, given_type};

/// A flat condition that's open at the current point.
struct Flat {
    /// The state where it was entered: nothing says the condition holds.
    entry: FlowState,
    entry_dead: bool,
    /// How its branches before the current one ended.
    ends: Vec<FlowState>,
    /// A variable it declares for its body, and what that was before.
    shadowed: Option<(SmolStr, Option<Local>)>,
}

pub(super) struct Template {
    /// Under `components/`, or has `@props`: `$slot` and `$attributes` are what Laravel gives a
    /// component. Elsewhere they're names like any other, that may be anything.
    is_component: bool,
    /// Under `errors/`: `$exception` is the one being shown.
    is_error_page: bool,
    /// Where each variable nothing declares is first seen: the one place to go to for it, whichever
    /// branch it's seen on.
    first_seen: FxHashMap<SmolStr, TextRange>,
    flats: Vec<Flat>,
    /// The names of the file's unknown directives with an `@end` to match: the project's own
    /// conditions and blocks. `Blade::if` makes those `@name`, `@unlessname`, `@elsename` and
    /// `@endname`.
    custom_blocks: FxHashSet<String>,
    /// The comment `@var` tags were last read from.
    pub(super) last_doc: Option<SyntaxToken>,
    /// What it's given by what renders it.
    view: ViewData,
    /// `$$name`, which can define anything.
    variable_variables: bool,
    /// What custom directives declare, by their names in lower case.
    directive_variables: FxHashMap<String, Vec<SmolStr>>,
    /// The `@extends` seen so far: what they pass on is what's defined at the end of the file.
    extends: Vec<TextRange>,
}

impl Template {
    pub(super) fn of(checker: &Checker<'_, '_>, root: &SyntaxNode) -> Option<Template> {
        if !stir_syntax::is_blade_path(&checker.path) {
            return None;
        }
        let (db, ws) = (checker.ctx.db, checker.ctx.ws);
        let view = crate::views::view_data(db, ws, checker.file).clone();
        let path = checker.path.to_string_lossy().replace('\\', "/");
        let is_component = view.component
            || path.contains("/components/")
            || root
                .descendants()
                .filter(|n| n.kind() == BLADE_DIRECTIVE)
                .filter_map(|n| directive_name(&n))
                .any(|n| n == "props" || n == "aware");
        let mut custom_blocks = FxHashSet::default();
        for html in root.descendants_with_tokens() {
            if let SyntaxElement::Token(t) = html
                && t.kind() == INLINE_HTML
            {
                for (_, name) in at_names(t.text()) {
                    if let Some(block) = name.strip_prefix("end")
                        && !stir_syntax::is_blade_directive(&name)
                    {
                        custom_blocks.insert(block.to_owned());
                    }
                }
            }
        }
        // which only matters where something's made of what's defined here
        let directive_variables =
            if view.is_closed() || view.open == Some(crate::views::OpenReason::ErrorPage) {
                crate::views::directive_variables(db, ws)
            } else {
                FxHashMap::default()
            };
        Some(Template {
            is_component,
            is_error_page: path.contains("/errors/"),
            first_seen: FxHashMap::default(),
            flats: Vec::new(),
            custom_blocks,
            last_doc: None,
            view,
            variable_variables: root.descendants_with_tokens().any(|e| e.kind() == DOLLAR),
            extends: Vec::new(),
            directive_variables,
        })
    }
}

/// The name of a directive, in lower case and without the `@`.
pub(crate) fn directive_name(node: &SyntaxNode) -> Option<String> {
    let token = node.token(BLADE_DIRECTIVE_NAME)?;
    Some(token.text().trim_start_matches('@').to_ascii_lowercase())
}

/// Every `@name` that could be a directive in `html`, in lower case.
pub(crate) fn at_names(html: &str) -> Vec<(usize, String)> {
    let bytes = html.as_bytes();
    let word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut out = Vec::new();
    for (at, _) in html.match_indices('@') {
        if at > 0 && (word(bytes[at - 1]) || bytes[at - 1] == b'@') {
            continue;
        }
        let len = bytes[at + 1..].iter().take_while(|b| word(**b)).count();
        if len > 0 {
            out.push((at, html[at + 1..at + 1 + len].to_ascii_lowercase()));
        }
    }
    out
}

/// The string a literal holds, unless it has anything in it that's worked out.
fn plain_string(node: &SyntaxNode) -> Option<String> {
    let text = text_of(node);
    (node.kind() == STRING_LITERAL && !text.contains('$')).then(|| unquote(&text))
}

impl<'a, 'db> Checker<'a, 'db> {
    /// Can a variable nothing declares be read here?
    pub(super) fn open_vars(&self) -> bool {
        self.fns.last().is_some_and(|f| f.open_vars)
    }

    /// A class of the project or of what it depends on, if it's there.
    fn class_if_known(&self, name: &str) -> Option<Ty> {
        self.ctx
            .class(&name.to_ascii_lowercase())
            .map(|_| Ty::class(name, Vec::new()))
    }

    /// What Laravel makes available in a template without anything saying so.
    fn implicit_variable(&self, name: &str) -> Option<Ty> {
        let template = self.template.as_ref()?;
        let known = |class: &str| Some(self.class_if_known(class).unwrap_or_else(Ty::untyped));
        match name {
            "errors" => known("Illuminate\\Support\\ViewErrorBag"),
            "__env" => known("Illuminate\\View\\Factory"),
            "slot" if template.is_component => known("Illuminate\\View\\ComponentSlot"),
            "attributes" if template.is_component => {
                known("Illuminate\\View\\ComponentAttributeBag")
            }
            // a class-based component's own class, whichever that is
            "component" if template.is_component => Some(Ty::untyped()),
            "exception" if template.is_error_page => {
                known("Symfony\\Component\\HttpKernel\\Exception\\HttpExceptionInterface")
            }
            _ => None,
        }
    }

    /// Is `name` one that some of what renders this template gives, and not all? Then to ask if
    /// it's there may well be answered no.
    pub(super) fn given_if_at_all(&self, name: &str) -> bool {
        let Some(template) = &self.template else {
            return false;
        };
        template.view.vars.get(name).is_some_and(|v| v.optional)
    }

    /// Would reading `name`, which nothing here declares and nothing gives, be an error? All that
    /// renders the template has to be known, and nothing here can have defined it another way.
    fn undefined_in_template(&self, name: &str) -> bool {
        let Some(template) = &self.template else {
            return false;
        };
        template.view.is_closed()
            && !template.variable_variables
            && self.dynamic_vars_fn != Some(self.fns.len())
            // a component's, what Laravel's factory shares with all templates, and its own
            && !matches!(name, "this" | "errors" | "slot" | "app" | "component")
            && !name.starts_with("__")
    }

    /// A variable read at `range` that nothing here has declared.
    pub(super) fn open_variable(&mut self, name: &str, range: TextRange) -> Ty {
        let implicit = self.implicit_variable(name);
        let given = self
            .template
            .as_ref()
            .and_then(|t| t.view.vars.get(name))
            .cloned();
        if implicit.is_none() && given.is_none() && self.undefined_in_template(name) {
            // `isset($x)` and `$x ?? ''` are how to read what may not be there
            if self.probing == 0 {
                self.error(
                    range,
                    codes::VARIABLE_UNDEFINED,
                    format!(
                        "undefined variable `${name}`: what renders this template doesn't give it"
                    ),
                );
                return Ty::error();
            }
        }
        let optional = given.as_ref().is_some_and(|g| g.optional);
        let kind = if implicit.is_some() {
            LocalKind::Implicit
        } else {
            LocalKind::Open
        };
        let ty = implicit.or(given.map(|g| g.ty)).unwrap_or_else(Ty::untyped);
        let Some(template) = &mut self.template else {
            return ty;
        };
        let decl = *template
            .first_seen
            .entry(SmolStr::new(name))
            .or_insert(range);
        self.def(range, Def::Local { decl });
        self.locals.insert(decl, (ty.clone(), kind));
        self.frames.last_mut().unwrap().insert(
            SmolStr::new(name),
            Local {
                ty: ty.clone(),
                declared: ty.clone(),
                kind,
                range: decl,
                maybe_undefined: false,
            },
        );
        if optional && self.probing > 0 {
            return ty.nullable();
        }
        ty
    }

    /// `compact('a', ['b'])`: what's in those variables here, for what it's passed to.
    pub(super) fn note_compact(&mut self, call: &SyntaxNode) {
        fn strings(node: &SyntaxNode, out: &mut Vec<SyntaxNode>) {
            for arg in node.children() {
                if !matches!(arg.kind(), ARG | ARRAY_ELEM) {
                    continue;
                }
                for value in exprs_of(&arg) {
                    match value.kind() {
                        STRING_LITERAL => out.push(value),
                        ARRAY_EXPR => strings(&value, out),
                        _ => {}
                    }
                }
            }
        }
        let Some(args) = call.child(ARG_LIST) else {
            return;
        };
        let mut found = Vec::new();
        strings(&args, &mut found);
        for literal in found {
            let Some(name) = plain_string(&literal) else {
                continue;
            };
            if let Some(local) = self.lookup(&name) {
                let seen = (local.ty.clone(), local.maybe_undefined);
                self.compact_vars.insert(literal.text_range(), seen);
            }
        }
    }

    /// What `@include` or `@extends` would pass on from here, by `get_defined_vars()`.
    fn flow_here(&self) -> ViewFlow {
        let mut scope: Vec<(SmolStr, Ty, bool)> = self
            .frames
            .last()
            .into_iter()
            .flatten()
            .filter(|(name, _)| {
                !matches!(name.as_str(), "this" | "errors" | "app") && !name.starts_with("__")
            })
            .map(|(name, local)| {
                let ty = match local.ty.kind() {
                    // no further back than that: it would only go on growing
                    TyKind::Class { name: class, .. } if name == "loop" => {
                        Ty::class(class.clone(), vec![Ty::untyped()])
                    }
                    _ => given_type(&local.ty),
                };
                (name.clone(), ty, local.maybe_undefined)
            })
            .collect();
        // what a component has, as nothing has to have read them yet
        if self.template.as_ref().is_some_and(|t| t.is_component) {
            for name in ["slot", "attributes", "component"] {
                if !scope.iter().any(|(n, ..)| n == name) {
                    let ty = self.implicit_variable(name).unwrap_or_else(Ty::untyped);
                    scope.push((SmolStr::new(name), given_type(&ty), false));
                }
            }
        }
        scope.sort_by(|a, b| a.0.cmp(&b.0));
        ViewFlow {
            scope,
            dynamic: self.dynamic_vars_fn == Some(self.fns.len()),
            each: None,
        }
    }

    /// Takes note of what a directive that renders a template passes on.
    fn note_view_flow(&mut self, node: &SyntaxNode, name: &str, values: &[SyntaxNode]) {
        let closed = self.template.as_ref().is_some_and(|t| {
            t.view.is_closed() || t.view.open == Some(crate::views::OpenReason::ErrorPage)
        });
        match name {
            "include" | "includeif" | "includefirst" | "includewhen" | "includeunless"
                if closed =>
            {
                let flow = self.flow_here();
                self.view_flows.insert(node.text_range(), flow);
            }
            "extends" | "extendsfirst" if closed => {
                if let Some(template) = &mut self.template {
                    template.extends.push(node.text_range());
                }
            }
            "each" => {
                let Some(items) = values.get(1) else { return };
                let Some(ty) = self.types.get(&items.text_range()).cloned() else {
                    return;
                };
                let before = self.diags.len();
                let (key, value) = self.iter_types(items.text_range(), &ty);
                self.diags.truncate(before);
                let flow = ViewFlow {
                    each: Some((key, value)),
                    ..ViewFlow::default()
                };
                self.view_flows.insert(node.text_range(), flow);
            }
            _ => {}
        }
    }

    /// `@extends` runs at the end of a file, with what's defined by then.
    pub(super) fn finish_template_flows(&mut self) {
        let Some(template) = &mut self.template else {
            return;
        };
        let extends = std::mem::take(&mut template.extends);
        if extends.is_empty() {
            return;
        }
        let flow = self.flow_here();
        for range in extends {
            self.view_flows.insert(range, flow.clone());
        }
    }

    /// `/** @var Post $post */` about a variable nothing has declared.
    pub(super) fn stated_variable(&mut self, name: &str, ty: Ty) {
        let doc = self.template.as_ref().and_then(|t| t.last_doc.clone());
        let Some(doc) = doc else { return };
        let spelled = format!("${name}");
        let decl = match doc.text().find(&spelled) {
            Some(at) => TextRange::at(
                doc.text_range().start() + TextSize::from(at as u32),
                TextSize::of(spelled.as_str()),
            ),
            None => doc.text_range(),
        };
        if let Some(template) = &mut self.template {
            template
                .first_seen
                .entry(SmolStr::new(name))
                .or_insert(decl);
        }
        self.def(decl, Def::Local { decl });
        self.locals.insert(decl, (ty.clone(), LocalKind::Open));
        self.local_names.insert(decl, SmolStr::new(name));
        self.frames.last_mut().unwrap().insert(
            SmolStr::new(name),
            Local {
                ty: ty.clone(),
                declared: ty,
                kind: LocalKind::Open,
                range: decl,
                maybe_undefined: false,
            },
        );
    }

    /// A variable a directive declares. `at` is where: not its name, so it isn't a reference to
    /// the variable for hovering over.
    pub(super) fn declare_implicit(&mut self, name: &str, ty: Ty, at: TextRange) {
        self.locals.insert(at, (ty.clone(), LocalKind::Implicit));
        self.local_names.insert(at, SmolStr::new(name));
        self.frames.last_mut().unwrap().insert(
            SmolStr::new(name),
            Local {
                ty: ty.clone(),
                declared: ty,
                kind: LocalKind::Implicit,
                range: at,
                maybe_undefined: false,
            },
        );
    }

    /// The statements of a template. Nothing is said of code that can't be reached: what looks
    /// like it may be in a condition of the project's own.
    pub(super) fn run_template_stmts(&mut self, parent: &SyntaxNode) {
        let depth = self.template.as_ref().map_or(0, |t| t.flats.len());
        for element in parent.children_with_tokens() {
            match element {
                NodeOrToken::Node(n) if is_stmt_kind(n.kind()) => {
                    if n.kind() == INLINE_HTML_STMT {
                        self.custom_conditions(&n);
                        self.custom_directive_variables(&n);
                    }
                    self.stmt(&n);
                }
                // `<?php /** @var Post $post */ ?>`, with no statement to be about
                NodeOrToken::Token(t) if t.kind() == DOC_COMMENT => {
                    let tags = self.var_tags_of(&t);
                    let named: Vec<_> = tags.into_iter().filter(|(n, _)| n.is_some()).collect();
                    self.apply_var_tags(&named, None);
                }
                _ => {}
            }
        }
        // as in the parser: a flat condition not closed by the end of a block stays in it
        while self
            .template
            .as_ref()
            .is_some_and(|t| t.flats.len() > depth)
        {
            self.close_flat();
        }
    }

    /// What the directives of the project's own declare, such as `@title('..')` being
    /// `<?php $title = '..' ?>`, for what comes after. What they're given isn't read.
    fn custom_directive_variables(&mut self, html: &SyntaxNode) {
        let Some(token) = html.token(INLINE_HTML) else {
            return;
        };
        let Some(template) = &self.template else {
            return;
        };
        if template.directive_variables.is_empty() {
            return;
        }
        let mut found = Vec::new();
        for (at, name) in at_names(token.text()) {
            if let Some(vars) = template.directive_variables.get(&name) {
                let range = TextRange::at(
                    token.text_range().start() + TextSize::from(at as u32),
                    TextSize::from(1 + name.len() as u32),
                );
                found.extend(vars.iter().map(|v| (v.clone(), range)));
            }
        }
        for (variable, at) in found {
            self.declare_implicit(&variable, Ty::untyped(), at);
        }
    }

    /// The directives of the project's own conditions in some HTML, which is what they are to us.
    fn custom_conditions(&mut self, html: &SyntaxNode) {
        let Some(template) = &self.template else {
            return;
        };
        if template.custom_blocks.is_empty() {
            return;
        }
        let Some(token) = html.token(INLINE_HTML) else {
            return;
        };
        let names = at_names(token.text());
        for (_, name) in names {
            let blocks = self
                .template
                .as_ref()
                .map(|t| &t.custom_blocks)
                .expect("a template");
            let of = |prefix: &str| {
                name.strip_prefix(prefix)
                    .is_some_and(|rest| blocks.contains(rest))
            };
            if of("end") {
                self.close_flat();
            } else if blocks.contains(name.as_str()) || of("unless") {
                self.open_flat(None);
            } else if of("else") {
                self.flat_else();
            }
        }
    }

    fn open_flat(&mut self, shadowed: Option<(SmolStr, Option<Local>)>) {
        let entry = self.snapshot();
        let entry_dead = self.dead;
        if let Some(template) = &mut self.template {
            template.flats.push(Flat {
                entry,
                entry_dead,
                ends: Vec::new(),
                shadowed,
            });
        }
    }

    fn flat_else(&mut self) {
        let end = (!self.dead).then(|| self.snapshot());
        let Some(flat) = self.template.as_mut().and_then(|t| t.flats.last_mut()) else {
            return;
        };
        flat.ends.extend(end);
        let (entry, entry_dead) = (flat.entry.clone(), flat.entry_dead);
        self.restore(&entry);
        self.dead = entry_dead;
    }

    fn close_flat(&mut self) {
        let Some(flat) = self.template.as_mut().and_then(|t| t.flats.pop()) else {
            return;
        };
        let mut states = flat.ends;
        if !self.dead {
            states.push(self.snapshot());
        }
        states.push(flat.entry);
        self.continue_from(states);
        self.dead = flat.entry_dead;
        if let Some((name, before)) = flat.shadowed {
            let frame = self.frames.last_mut().unwrap();
            match before {
                Some(local) => frame.insert(name, local),
                None => frame.remove(&name),
            };
        }
    }

    pub(super) fn blade_directive(&mut self, node: &SyntaxNode) {
        let Some(name) = directive_name(node) else {
            return;
        };
        let args: Vec<SyntaxNode> = node
            .child(ARG_LIST)
            .map(|l| l.children_of(ARG))
            .unwrap_or_default();
        let values: Vec<SyntaxNode> = args.iter().flat_map(exprs_of).collect();
        let part = stir_syntax::blade_flat_part(&name);
        match part {
            Some(BladeFlatPart::Else) => self.flat_else(),
            Some(BladeFlatPart::Closes) => self.close_flat(),
            _ => {}
        }
        match name.as_str() {
            "unset" => return self.unset(&values),
            "props" => self.declare_named_by_array(&values, true),
            "aware" => self.declare_named_by_array(&values, false),
            "inject" => self.inject(&values),
            _ => {
                for value in &values {
                    self.expr(value, None);
                }
                self.check_view_names(&name, &args);
                self.note_view_flow(node, &name, &values);
            }
        }
        if part == Some(BladeFlatPart::Opens) {
            let at = node
                .token(BLADE_DIRECTIVE_NAME)
                .map_or_else(|| node.text_range(), |t| t.text_range());
            let declared = match name.as_str() {
                "error" => Some(("message", Ty::string())),
                "session" | "context" => Some(("value", Ty::untyped())),
                _ => None,
            };
            let shadowed = declared
                .as_ref()
                .map(|(n, _)| (SmolStr::new(*n), self.lookup(n).cloned()));
            self.open_flat(shadowed);
            if let Some((variable, ty)) = declared {
                self.declare_implicit(variable, ty, at);
            }
        }
    }

    /// `@props(['title', 'kind' => 'info'])`: each is a variable, of what its component is given
    /// where that's known, and of its default if there may be nothing given. `@aware` is the same
    /// for what's given to a parent, which isn't looked at: that's anything.
    fn declare_named_by_array(&mut self, values: &[SyntaxNode], typed: bool) {
        for value in values {
            if value.kind() != ARRAY_EXPR {
                self.expr(value, None);
                continue;
            }
            for elem in value.children_of(ARRAY_ELEM) {
                let parts = exprs_of(&elem);
                let Some(name) = parts.first() else { continue };
                let default: Vec<Ty> = parts[1..]
                    .iter()
                    .map(|default| self.expr(default, None))
                    .collect();
                let Some(variable) = plain_string(name) else {
                    self.expr(name, None);
                    continue;
                };
                let ty = if typed {
                    self.prop_type(&variable, default.first())
                } else {
                    Ty::untyped()
                };
                self.declare_implicit(&variable, ty, name.text_range());
            }
        }
    }

    /// What a variable of `@props` holds: `$name = $name ?? $default`, as Laravel compiles it. A
    /// default of `null`, which is to say it may not be given, isn't made part of it: the same is
    /// said for all that's given to a component if it's asked, and no more.
    fn prop_type(&self, name: &str, default: Option<&Ty>) -> Ty {
        let Some(template) = &self.template else {
            return Ty::untyped();
        };
        if !template.view.is_closed() || template.view.spread {
            return Ty::untyped();
        }
        let default = default
            .map(given_type)
            .filter(|d| !d.is_null() && !d.is_untyped());
        match (template.view.vars.get(name), default) {
            (Some(given), None) => given.ty.clone(),
            (Some(given), Some(default)) => {
                if given.ty.is_untyped() {
                    return Ty::untyped();
                }
                let without_null = stir_types::without_null(&given.ty);
                let may_be_left_out = given.optional || without_null != given.ty;
                if may_be_left_out {
                    Ty::union(vec![without_null, default])
                } else {
                    given.ty.clone()
                }
            }
            (None, Some(default)) => default,
            (None, None) => Ty::untyped(),
        }
    }

    /// `<x-name ...>`: what's in it is checked where it is, and what's in between it and its end
    /// is in the same scope as it, so there's nothing to do for that.
    pub(super) fn blade_tag(&mut self, tag: &SyntaxNode) {
        let name = crate::views::tag_name(tag);
        for echo in tag.children_of(BLADE_ATTR_ECHO) {
            for e in exprs_of(&echo) {
                self.expr(&e, None);
            }
        }
        for directive in tag.children_of(BLADE_DIRECTIVE) {
            self.blade_directive(&directive);
        }
        let is_slot = crate::views::tag_is_slot(tag);
        let class = if is_slot {
            None
        } else {
            self.project_component_class(&name)
        };
        let signature = class.as_deref().and_then(|class| {
            let model = self.ctx.class(class)?;
            let (owner, method, _) = self.ctx.find_method(&model.self_ty(), "__construct")?;
            Some(self.ctx.method_sig(&owner, method))
        });
        for attr in tag.children_of(BLADE_ATTR) {
            for echo in attr.children_of(BLADE_ATTR_ECHO) {
                for e in exprs_of(&echo) {
                    self.expr(&e, None);
                }
            }
        }
        let attributes = crate::views::tag_attributes(tag);
        for attr in &attributes {
            let key = stir_syntax::blade_camel(&attr.key);
            let param = signature
                .as_ref()
                .and_then(|sig| sig.params.iter().find(|p| p.name == key.as_str()));
            let given = match &attr.php {
                Some(php) => {
                    let ty = self.expr(php, param.map(|p| &p.ty));
                    // the rest of them, in the rare case of there being some
                    if let Some(parent) = php.parent() {
                        for rest in exprs_of(&parent) {
                            if &rest != php {
                                self.expr(&rest, None);
                            }
                        }
                    }
                    Some((ty, php.text_range()))
                }
                None if attr.bare => Some((Ty::bool(), attr.name_range)),
                None => attr
                    .text
                    .as_ref()
                    .map(|words| {
                        (
                            Ty::literal(LitValue::String(SmolStr::new(words))),
                            attr.name_range,
                        )
                    })
                    .or_else(|| attr.echoes.then(|| (Ty::string(), attr.name_range))),
            };
            if let (Some((ty, range)), Some(param)) = (given, param) {
                self.check_tag_attribute(&name, &key, range, &ty, &param.ty);
            }
        }
        if let (Some(class), Some(signature)) = (&class, &signature) {
            self.check_required_of_component(tag, &name, class, signature, &attributes);
        }
    }

    /// The class a tag stands for, if it's the project's: a package's are left alone.
    fn project_component_class(&self, tag: &str) -> Option<String> {
        let (db, ws) = (self.ctx.db, self.ctx.ws);
        let crate::views::ComponentTarget::Class(class) =
            crate::views::resolve_component(db, ws, tag)
        else {
            return None;
        };
        let model = self.ctx.class(&class)?;
        (model.origin == stir_db::FileOrigin::Source).then_some(class)
    }

    /// A value for a parameter of a component's constructor. Laravel's call of it never has strict
    /// types, whatever the template or the class has, and what can be coerced is.
    fn check_tag_attribute(&mut self, tag: &str, key: &str, range: TextRange, ty: &Ty, want: &Ty) {
        let what = format!("attribute `{key}` of `<x-{tag}>`");
        if accepts_php_scalars(want) {
            let rest: Vec<Ty> = ty
                .members()
                .iter()
                .filter(|m| !coerces_to(m, want) && !self.is_stringable_for(m, want))
                .cloned()
                .collect();
            // what's only maybe wrong is no more than that of what a template is given
            if rest.len() < ty.members().len() {
                return;
            }
        }
        self.check_assign(range, ty, want, &what, codes::ARGUMENT_TYPE);
    }

    /// A parameter of the constructor nothing in the tag gives, which Laravel can't fill in. That
    /// comes down to `Container::resolvePrimitive`: a parameter that's a class is made by the
    /// container, and one with a default or `null` to fall back on gets that. A parent's
    /// `$attributes` also go to the constructor, so it has to be known there are none.
    fn check_required_of_component(
        &mut self,
        tag: &SyntaxNode,
        name: &str,
        class: &str,
        signature: &crate::model::FnSig,
        attributes: &[crate::views::TagAttribute],
    ) {
        let Some(template) = &self.template else {
            return;
        };
        if !template.view.is_closed()
            || template.is_component
            || template.view.vars.contains_key("attributes")
            || self.lookup("attributes").is_some()
        {
            return;
        }
        let (db, ws) = (self.ctx.db, self.ctx.ws);
        let Some(model) = self.ctx.class(class) else {
            return;
        };
        // its own way of being made
        let made_as_usual = self
            .ctx
            .find_method(&model.self_ty(), "resolve")
            .is_some_and(|(owner, ..)| owner.key == crate::views::COMPONENT.to_ascii_lowercase());
        if !made_as_usual || model.def.mods.is_abstract || *crate::views::container_is_told(db, ws)
        {
            return;
        }
        let Some((_, method, _)) = self.ctx.find_method(&model.self_ty(), "__construct") else {
            return;
        };
        let mut missing = Vec::new();
        for (param, native) in signature.params.iter().zip(&method.sig.params) {
            let given = attributes
                .iter()
                .any(|a| stir_syntax::blade_camel(&a.key) == param.name.as_str());
            if given || param.optional || param.variadic {
                continue;
            }
            let builtin_only = match &native.ty {
                None => true,
                Some(_) => {
                    !stir_types::is_nullable(self.ctx, &param.ty)
                        && !param.ty.is_untyped()
                        && param.ty.members().iter().all(|m| {
                            matches!(
                                m.kind(),
                                TyKind::Int
                                    | TyKind::Float
                                    | TyKind::String
                                    | TyKind::Bool
                                    | TyKind::Array { .. }
                                    | TyKind::List { .. }
                            )
                        })
                }
            };
            // `#[Config('..')]` and the like make it for itself
            if builtin_only && !native.has_attribute {
                missing.push(param.name.clone());
            }
        }
        if missing.is_empty() {
            return;
        }
        let range = tag
            .token(BLADE_TAG_NAME)
            .map_or_else(|| tag.text_range(), |t| t.text_range());
        for param in missing {
            self.error(
                range,
                codes::ARGUMENTS_COUNT,
                format!(
                    "`<x-{name}>` has nothing for `${param}` of the constructor of `{}`, which Laravel can't fill in itself",
                    model.name
                ),
            );
        }
    }

    /// `@inject('metrics', 'App\Services\Metrics')`, which is `$metrics = app(...)`.
    fn inject(&mut self, values: &[SyntaxNode]) {
        let [name, service, ..] = values else {
            for value in values {
                self.expr(value, None);
            }
            return;
        };
        let given = self.expr(service, None);
        let ty = match given.kind() {
            TyKind::Literal(LitValue::String(class)) => self.class_if_known(class),
            TyKind::ClassString(Some(inner)) => Some(inner.clone()),
            _ => None,
        };
        if let Some(variable) = plain_string(name) {
            self.declare_implicit(&variable, ty.unwrap_or_else(Ty::untyped), name.text_range());
        }
    }

    /// The `$loop` a statement gives what's in it, and where that's declared: for `@foreach` and
    /// `@forelse`, not a `foreach` in PHP, which doesn't keep track.
    pub(super) fn blade_loop(&self, node: &SyntaxNode) -> Option<(Ty, TextRange)> {
        self.template.as_ref()?;
        if node.kind() != FOREACH_STMT {
            return None;
        }
        let keyword = node.token(FOREACH_KW)?;
        if !keyword.text().starts_with('@') {
            return None;
        }
        let stub = "Illuminate\\View\\BladeLoop";
        let ty = if self.ctx.class(&stub.to_ascii_lowercase()).is_some() {
            // a template included in a loop has one around it that can't be seen from here, or
            // it hasn't: there's nothing to say of its parent
            let parent = self
                .lookup("loop")
                .filter(|l| l.kind == LocalKind::Implicit)
                .map_or_else(Ty::untyped, |l| l.ty.clone());
            Ty::class(stub, vec![parent])
        } else {
            Ty::untyped()
        };
        Some((ty, keyword.text_range()))
    }

    /// The `$loop` after a loop is that of the loop around it, if there's any.
    pub(super) fn leave_blade_loop(&mut self, outer: Option<Local>) {
        let frame = self.frames.last_mut().unwrap();
        match outer {
            Some(local) => frame.insert(SmolStr::new("loop"), local),
            None => frame.remove("loop"),
        };
    }

    /// With no Laravel to be seen, what a template names is mostly its: `route()`, `Str::`, or
    /// what's only there once the project's installed. Saying they don't exist helps no one.
    pub(super) fn forget_unknown_names_without_laravel(&mut self) {
        if self.template.is_some() && self.ctx.class("illuminate\\view\\factory").is_none() {
            self.diags.retain(|d| {
                !matches!(
                    &*d.code,
                    codes::CLASS_NOT_FOUND | codes::FUNCTION_NOT_FOUND | codes::CONSTANT_NOT_FOUND
                )
            });
        }
    }

    /// What a template can take for granted of an expression's type: that there's a user, in
    /// `Auth::user()` and `$request->user()`. Whether there is depends on the route the template is
    /// for or an `@auth` that may be in another, and there's no place in it to say so.
    pub(super) fn assumed_in_template(&self, node: &SyntaxNode, ty: Ty) -> Ty {
        if self.template.is_none()
            || !matches!(node.kind(), METHOD_CALL_EXPR | STATIC_CALL_EXPR)
            || !ty.is_union()
        {
            return ty;
        }
        let named = node
            .first_child()
            .and_then(|callee| callee.children().find(|c| c.kind() == NAME))
            .is_some_and(|n| text_of(&n).eq_ignore_ascii_case("user"));
        if named {
            stir_types::without_null(&ty)
        } else {
            ty
        }
    }

    /// `@include('no.such.template')`, if it's known what templates there are.
    fn check_view_names(&mut self, directive: &str, args: &[SyntaxNode]) {
        let names = crate::script::links::required_views(directive, args);
        if names.is_empty() {
            return;
        }
        let (db, ws) = (self.ctx.db, self.ctx.ws);
        if !crate::script::links::views_are_all_known(db, ws, self.file) {
            return;
        }
        let known = crate::script::links::view_names(db, ws);
        for (name, range) in names {
            if !known.contains(&name.replace('/', ".")) {
                self.error(
                    range,
                    codes::VIEW_NOT_FOUND,
                    format!("there's no template `{name}`"),
                );
            }
        }
    }
}
