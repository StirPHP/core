//! Where a name written as a string leads, in a framework that has names for things: WordPress's
//! hooks to where they run, and Laravel's views to their templates and routes to where they're
//! named. For going to a definition from the string.

use stir_db::{Db, FileOrigin, SourceFile, Workspace};
use stir_syntax::SyntaxKind::*;
use stir_syntax::{SyntaxNode, TextRange, TextSize};

use super::wordpress::{arguments, string_value, wordpress_hooks};

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

/// What the string at `offset` in `file` names, as places in the workspace: none where it's no
/// string, or one that names nothing known.
pub fn string_targets(
    db: &dyn Db,
    ws: Workspace,
    file: SourceFile,
    offset: TextSize,
) -> Vec<(SourceFile, TextRange)> {
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
