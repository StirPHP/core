//! Where a name written as a string leads, in a framework that has names for things: WordPress's
//! hooks to where they run, and Laravel's views to their templates and routes to where they're
//! named. For going to a definition from the string.

use stir_db::{Db, FileOrigin, SourceFile, Workspace};
use stir_syntax::SyntaxKind::*;
use stir_syntax::{SyntaxNode, TextRange, TextSize};

use super::wordpress::{arguments, string_value, wordpress_hooks};

/// The arguments of a Blade directive that name templates, by position, in lower case without the
/// `@`. A list of names counts too: `@includeFirst(['a', 'b'])`.
fn view_arguments(directive: &str) -> &'static [usize] {
    match directive {
        "include" | "includeif" | "includefirst" | "includeisolated" | "extends"
        | "extendsfirst" | "component" | "componentfirst" => &[0],
        "includewhen" | "includeunless" => &[1],
        // the fourth is what's shown for none, and may be text instead: `raw|None yet`
        "each" => &[0, 3],
        _ => &[],
    }
}

/// Is this a string in a Blade directive that names a template, if it's a name at all?
fn names_a_view(literal: &SyntaxNode) -> bool {
    let arg = literal.parent().and_then(|p| match p.kind() {
        ARG => Some(p),
        ARRAY_ELEM => p.parent()?.parent().filter(|a| a.kind() == ARG),
        _ => None,
    });
    let Some(arg) = arg else { return false };
    let Some(directive) = arg
        .parent()
        .and_then(|l| l.parent())
        .filter(|d| d.kind() == BLADE_DIRECTIVE)
    else {
        return false;
    };
    let Some(name) = directive
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| t.kind() == BLADE_DIRECTIVE_NAME)
    else {
        return false;
    };
    let name = name.text().trim_start_matches('@').to_ascii_lowercase();
    let Some(position) = arg.parent().map(|l| {
        l.children()
            .filter(|c| c.kind() == ARG)
            .position(|c| c == arg)
    }) else {
        return false;
    };
    position.is_some_and(|p| view_arguments(&name).contains(&p))
}

/// The string at `offset` in a template, if it's in a Blade directive where a template is named:
/// its content's range and what's written of it before `offset`.
pub fn view_string_at(root: &SyntaxNode, offset: TextSize) -> Option<(TextRange, String)> {
    let token = root.token_at_offset(offset).left_biased()?;
    if !matches!(token.kind(), STRING_SINGLE | STRING_DOUBLE) {
        return None;
    }
    let range = token.text_range();
    if offset <= range.start() || offset >= range.end() {
        return None;
    }
    let literal = token.parent()?;
    if literal.kind() != STRING_LITERAL || !names_a_view(&literal) {
        return None;
    }
    let inner = TextRange::new(
        range.start() + TextSize::of('\''),
        range.end() - TextSize::of('\''),
    );
    let written = token.text()[1..usize::from(offset - range.start())].to_owned();
    Some((inner, written))
}

/// The names of the templates a Blade directive can't do without: not `@includeIf` and those
/// that take a list, and only what can be looked for, so not a package's (`pkg::name`), one worked
/// out when it runs, or a class (`@component('App\View\Alert')`).
pub(crate) fn required_views(directive: &str, args: &[SyntaxNode]) -> Vec<(String, TextRange)> {
    if matches!(
        directive,
        "includeif" | "includefirst" | "extendsfirst" | "componentfirst"
    ) {
        return Vec::new();
    }
    view_arguments(directive)
        .iter()
        .filter_map(|&i| args.get(i))
        .filter_map(|arg| arg.children().find(|c| c.kind() == STRING_LITERAL))
        .filter_map(|literal| string_value(&literal))
        .filter(|(name, _)| {
            !name.is_empty()
                && !name.contains("::")
                && !name.contains('\\')
                && !name.contains('|')
                && !name.contains(['$', ' '])
        })
        .collect()
}

/// The dotted names of the project's templates under `resources/views/`.
#[salsa::tracked(returns(ref))]
pub fn view_names(db: &dyn Db, ws: Workspace) -> rustc_hash::FxHashSet<String> {
    let mut names = rustc_hash::FxHashSet::default();
    for file in ws.files(db) {
        if file.origin(db) != FileOrigin::Source {
            continue;
        }
        let path = file.path(db).to_string_lossy().replace('\\', "/");
        let Some((_, relative)) = path.rsplit_once("/resources/views/") else {
            continue;
        };
        let name = relative
            .strip_suffix(".blade.php")
            .or_else(|| relative.strip_suffix(".php"));
        if let Some(name) = name {
            names.insert(name.replace('/', "."));
        }
    }
    names
}

/// Can a template missing from [`view_names`] be said not to exist? Only in a Laravel application
/// (a package's templates can include those of whichever application it's in) that keeps its
/// templates where it's expected to, all of them being looked at.
#[salsa::tracked]
pub(crate) fn views_are_all_known(db: &dyn Db, ws: Workspace, file: SourceFile) -> bool {
    let in_views = |f: SourceFile| {
        f.path(db)
            .to_string_lossy()
            .replace('\\', "/")
            .contains("/resources/views/")
    };
    let settings = ws.settings(db);
    in_views(file)
        && supports(db, ws, "laravel")
        && settings.php_paths.is_empty()
        && settings.php_excludes.is_empty()
        && !settings.files_left_out
        && *project_finds_views_as_usual(db, ws)
}

#[salsa::tracked]
fn project_finds_views_as_usual(db: &dyn Db, ws: Workspace) -> bool {
    let mut is_application = false;
    for file in ws.files(db) {
        if file.origin(db) != FileOrigin::Source {
            continue;
        }
        let path = file.path(db).to_string_lossy().replace('\\', "/");
        let text = file.text(db);
        if path.ends_with("/bootstrap/app.php") {
            is_application = true;
        }
        if path.ends_with("/config/view.php") && !default_view_paths(text) {
            return false;
        }
        if !stir_syntax::is_blade_path(file.path(db))
            && [
                "addLocation(",
                "prependLocation(",
                "addExtension('",
                "addExtension(\"",
            ]
            .iter()
            .any(|call| text.contains(call))
        {
            return false;
        }
    }
    is_application
}

/// Does `config/view.php` keep to `resource_path('views')`?
fn default_view_paths(config: &str) -> bool {
    let Some((_, after)) = config.split_once("'paths'") else {
        return true;
    };
    let list = after.split(']').next().unwrap_or(after);
    list.matches('(').count() == 1 && list.contains("resource_path('views')")
}

/// Is the support for `framework` on in `ws`?
fn supports(db: &dyn Db, ws: Workspace, framework: &str) -> bool {
    let name = format!("{}{framework}.rhai", stir_db::BUNDLED_PREFIX);
    ws.bundled_scripts(db)
        .iter()
        .any(|s| s.path(db).to_str() == Some(name.as_str()))
}

/// The functions whose first argument names a hook.
const HOOK_FUNCTIONS: &[&str] = &[
    "add_action",
    "add_filter",
    "remove_action",
    "remove_filter",
    "has_action",
    "has_filter",
    "did_action",
    "doing_action",
    "remove_all_actions",
    "remove_all_filters",
];

/// A part of a component's tag that's a name for something.
enum TagPart {
    /// The name of the component, in its tag or its end.
    Name(stir_syntax::SyntaxToken),
    /// An attribute, by its key, with where its name is.
    Attribute(String, TextRange),
}

/// What of a component's tag is at `offset`.
fn tag_part_at(root: &SyntaxNode, offset: TextSize) -> Option<(SyntaxNode, TagPart)> {
    let at = root.token_at_offset(offset);
    let token = [at.clone().right_biased(), at.left_biased()]
        .into_iter()
        .flatten()
        .find(|t| matches!(t.kind(), BLADE_TAG_NAME | BLADE_ATTR_NAME))?;
    if token.kind() == BLADE_TAG_NAME {
        let tag = token.parent()?;
        if tag.kind() == BLADE_TAG && crate::views::tag_is_slot(&tag) {
            return None;
        }
        return Some((tag, TagPart::Name(token)));
    }
    let tag = token.parent()?.parent().filter(|n| n.kind() == BLADE_TAG)?;
    if crate::views::tag_is_slot(&tag) {
        return None;
    }
    let attribute = crate::views::tag_attributes(&tag)
        .into_iter()
        .find(|a| a.name_range == token.text_range())?;
    Some((tag, TagPart::Attribute(attribute.key, token.text_range())))
}

/// The templates a component's name in a tag could be for.
fn view_file(db: &dyn Db, ws: Workspace, view: &str) -> Option<SourceFile> {
    views(db, ws, view).first().map(|(file, _)| *file)
}

/// Where a variable `@props` declares is declared, in the template `file`: the string for it.
fn prop_declared(db: &dyn Db, file: SourceFile, key: &str) -> Option<TextRange> {
    let root = stir_db::parse(db, file).syntax();
    let camel = stir_syntax::blade_camel(key);
    root.descendants()
        .filter(|n| n.kind() == BLADE_DIRECTIVE)
        .filter(|n| crate::body::template::directive_name(n).as_deref() == Some("props"))
        .flat_map(|n| n.descendants().collect::<Vec<_>>())
        .filter(|n| n.kind() == ARRAY_ELEM)
        .filter_map(|elem| elem.children().find(|c| c.kind() == STRING_LITERAL))
        .find_map(|literal| {
            let (value, range) = string_value(&literal)?;
            (value == key || value == camel).then_some(range)
        })
}

/// The constructor's parameter an attribute of a component is for.
fn constructor_param(
    ctx: &crate::Ctx<'_>,
    class: &str,
    key: &str,
) -> Option<(SourceFile, crate::model::ParamSig)> {
    let model = ctx.class(class)?;
    let (owner, method, _) = ctx.find_method(&model.self_ty(), "__construct")?;
    let sig = ctx.method_sig(&owner, method);
    let camel = stir_syntax::blade_camel(key);
    let param = sig.params.into_iter().find(|p| p.name == camel.as_str())?;
    Some((owner.file, param))
}

/// Where the name of a component in its tag, or an attribute in it, leads: its class or template,
/// and the parameter of the constructor or the variable of `@props`.
pub fn tag_targets(
    db: &dyn Db,
    ws: Workspace,
    file: SourceFile,
    offset: TextSize,
) -> Vec<(SourceFile, TextRange)> {
    use crate::views::ComponentTarget;
    let root = stir_db::parse(db, file).syntax();
    let Some((tag, part)) = tag_part_at(&root, offset) else {
        return Vec::new();
    };
    let target = crate::views::resolve_component(db, ws, &crate::views::tag_name(&tag));
    let ctx = crate::Ctx::new(db, ws);
    match (target, part) {
        (ComponentTarget::Class(class), TagPart::Name(_)) => ctx
            .class(&class)
            .map(|model| vec![(model.file, model.def.name_range)])
            .unwrap_or_default(),
        (ComponentTarget::View(view), TagPart::Name(_)) => views(db, ws, &view),
        (ComponentTarget::Class(class), TagPart::Attribute(key, _)) => {
            constructor_param(&ctx, &class, &key)
                .map(|(owner, param)| vec![(owner, param.range)])
                .unwrap_or_default()
        }
        (ComponentTarget::View(view), TagPart::Attribute(key, _)) => view_file(db, ws, &view)
            .and_then(|template| Some((template, prop_declared(db, template, &key)?)))
            .into_iter()
            .collect(),
        _ => Vec::new(),
    }
}

/// What to say of a part of a tag on hovering over it.
pub struct TagHover {
    pub range: TextRange,
    /// The declaration: code, if there's any.
    pub code: Option<String>,
    pub note: String,
}

/// What's said of the name of a component in its tag, or an attribute in it.
pub fn tag_hover(
    db: &dyn Db,
    ws: Workspace,
    file: SourceFile,
    offset: TextSize,
) -> Option<TagHover> {
    use crate::views::ComponentTarget;
    let root = stir_db::parse(db, file).syntax();
    let (tag, part) = tag_part_at(&root, offset)?;
    let target = crate::views::resolve_component(db, ws, &crate::views::tag_name(&tag));
    let ctx = crate::Ctx::new(db, ws);
    let hover = |range, code, note: String| Some(TagHover { range, code, note });
    let unread = "A package's component, or one registered in a way that can't be read.";
    match part {
        TagPart::Name(token) => {
            let range = token.text_range();
            match target {
                ComponentTarget::Class(class) => {
                    let Some(model) = ctx.class(&class) else {
                        return hover(range, None, unread.to_owned());
                    };
                    let params = ctx
                        .find_method(&model.self_ty(), "__construct")
                        .map(|(owner, method, _)| ctx.method_sig(&owner, method).params)
                        .unwrap_or_default();
                    let shown: Vec<String> = params
                        .iter()
                        .map(|p| {
                            let default = if p.optional { " = ..." } else { "" };
                            format!("{} ${}{default}", p.ty, p.name)
                        })
                        .collect();
                    hover(
                        range,
                        Some(format!("class {}({})", model.name, shown.join(", "))),
                        "A component with a class.".to_owned(),
                    )
                }
                ComponentTarget::View(view) => {
                    let declared = view_file(db, ws, &view).and_then(|template| {
                        let root = stir_db::parse(db, template).syntax();
                        let props = root.descendants().find(|n| {
                            n.kind() == BLADE_DIRECTIVE
                                && crate::body::template::directive_name(n).as_deref()
                                    == Some("props")
                        })?;
                        Some(props.text().to_string().trim().to_owned())
                    });
                    hover(
                        range,
                        declared,
                        format!("An anonymous component, the template `{view}`."),
                    )
                }
                ComponentTarget::Dynamic => hover(
                    range,
                    None,
                    "The component it's given to render, which could be any.".to_owned(),
                ),
                ComponentTarget::Unknown => hover(range, None, unread.to_owned()),
            }
        }
        TagPart::Attribute(key, range) => match target {
            ComponentTarget::Class(class) => match constructor_param(&ctx, &class, &key) {
                Some((_, param)) => hover(
                    range,
                    Some(format!("{} ${}", param.ty, param.name)),
                    format!("A parameter of the constructor of `{class}`."),
                ),
                None => hover(
                    range,
                    None,
                    "Not a parameter of the constructor: it goes in `$attributes`.".to_owned(),
                ),
            },
            ComponentTarget::View(view) => {
                let template = view_file(db, ws, &view)?;
                if prop_declared(db, template, &key).is_some() {
                    let name = stir_syntax::blade_camel(&key);
                    let data = crate::views::view_data(db, ws, template);
                    let code = data
                        .vars
                        .get(name.as_str())
                        .map_or_else(|| format!("${name}"), |v| format!("{} ${name}", v.ty));
                    return hover(
                        range,
                        Some(code),
                        "Declared by the component's `@props`.".to_owned(),
                    );
                }
                let has_props = stir_db::parse(db, template)
                    .syntax()
                    .descendants()
                    .filter(|n| n.kind() == BLADE_DIRECTIVE)
                    .any(|n| crate::body::template::directive_name(&n).as_deref() == Some("props"));
                let note = if has_props {
                    "Not in the component's `@props`: it goes in `$attributes`."
                } else {
                    "A variable of the component, and also in its `$attributes`."
                };
                hover(range, None, note.to_owned())
            }
            _ => None,
        },
    }
}

/// What the string at `offset` in `file` names, as places in the workspace: none where it's no
/// string, or one that names nothing known.
pub fn string_targets(
    db: &dyn Db,
    ws: Workspace,
    file: SourceFile,
    offset: TextSize,
) -> Vec<(SourceFile, TextRange)> {
    if stir_syntax::is_blade_path(file.path(db)) {
        let tag = tag_targets(db, ws, file, offset);
        if !tag.is_empty() {
            return tag;
        }
    }
    let root = stir_db::parse(db, file).syntax();
    let Some(token) = root.token_at_offset(offset).right_biased() else {
        return Vec::new();
    };
    let Some(literal) = token
        .parent_ancestors()
        .find(|n| n.kind() == STRING_LITERAL)
    else {
        return Vec::new();
    };
    let Some((value, _)) = string_value(&literal) else {
        return Vec::new();
    };
    // the first argument of the call it's passed to
    if names_a_view(&literal) {
        return if supports(db, ws, "laravel") {
            views(db, ws, &value)
        } else {
            Vec::new()
        };
    }
    let Some(arg) = literal.parent().filter(|a| a.kind() == ARG) else {
        return Vec::new();
    };
    let Some(call) = arg.parent().and_then(|l| l.parent()) else {
        return Vec::new();
    };
    if arguments(&call).first() != Some(&literal) {
        return Vec::new();
    }
    let Some(callee) = callee_name(&call) else {
        return Vec::new();
    };
    let callee = callee.to_ascii_lowercase();
    if call.kind() == CALL_EXPR
        && HOOK_FUNCTIONS.contains(&callee.as_str())
        && supports(db, ws, "wordpress")
    {
        return wordpress_hooks(db, ws)
            .runs
            .get(&value)
            .map(|runs| runs.iter().map(|r| (r.file, r.range)).collect())
            .unwrap_or_default();
    }
    if !supports(db, ws, "laravel") {
        return Vec::new();
    }
    match callee.as_str() {
        // `view('posts.show')`, `View::make(..)`, `->view(..)`
        "view" | "make" if call.kind() != STATIC_CALL_EXPR || is_view_facade(&call) => {
            views(db, ws, &value)
        }
        "route" | "to_route" => routes(db, ws, &value),
        _ => Vec::new(),
    }
}

fn callee_name(call: &SyntaxNode) -> Option<String> {
    match call.kind() {
        CALL_EXPR => call.children().find(|c| c.kind() == NAME_EXPR).map(|n| {
            n.text()
                .to_string()
                .trim()
                .trim_start_matches('\\')
                .to_owned()
        }),
        METHOD_CALL_EXPR | NULLSAFE_METHOD_CALL_EXPR | STATIC_CALL_EXPR => call
            .first_child()?
            .children()
            .find(|c| c.kind() == NAME)
            .map(|n| n.text().to_string().trim().to_owned()),
        _ => None,
    }
}

fn is_view_facade(call: &SyntaxNode) -> bool {
    call.first_child()
        .and_then(|f| f.first_child())
        .is_some_and(|c| {
            c.text()
                .to_string()
                .trim()
                .trim_start_matches('\\')
                .ends_with("View")
        })
}

/// The templates under `resources/views/` a view's name stands for.
fn views(db: &dyn Db, ws: Workspace, name: &str) -> Vec<(SourceFile, TextRange)> {
    // `posts.show`, or a package's `mail::message`
    let name = name.rsplit("::").next().unwrap_or(name);
    let relative = name.replace('.', "/");
    // `raw|` before the fourth argument of `@each` makes it text
    if relative.contains('|') {
        return Vec::new();
    }
    let wanted = [
        format!("/resources/views/{relative}.blade.php"),
        format!("/resources/views/{relative}.php"),
    ];
    ws.files(db)
        .iter()
        .filter(|f| f.origin(db) == FileOrigin::Source)
        .filter(|f| {
            let path = f.path(db).to_string_lossy().replace('\\', "/");
            wanted.iter().any(|w| path.ends_with(w.as_str()))
        })
        .map(|&f| (f, TextRange::empty(TextSize::new(0))))
        .collect()
}

/// Where a route gets its name: `->name('posts.show')` in the files under `routes/`.
fn routes(db: &dyn Db, ws: Workspace, name: &str) -> Vec<(SourceFile, TextRange)> {
    let mut out = Vec::new();
    for &file in ws.files(db) {
        let path = file.path(db).to_string_lossy().replace('\\', "/");
        if file.origin(db) != FileOrigin::Source || !path.contains("/routes/") {
            continue;
        }
        let root = stir_db::parse(db, file).syntax();
        for call in root.descendants().filter(|n| n.kind() == METHOD_CALL_EXPR) {
            if callee_name(&call).as_deref() != Some("name") {
                continue;
            }
            if let Some((value, range)) = arguments(&call).first().and_then(string_value)
                && value == name
            {
                out.push((file, range));
            }
        }
    }
    out
}
