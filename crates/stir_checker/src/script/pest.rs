//! What a script can ask about a Pest test file, read from the code as written: nothing is run.
//! Pest writes a class for each test file at runtime, out of what `uses()` calls name, and binds the
//! file's closures to it. Those calls are in the file itself, and in `tests/Pest.php` with `->in()`
//! saying which folders they're for.

use std::path::{Component, Path, PathBuf};

use stir_db::{Db, FileOrigin, SourceFile, Workspace};
use stir_script::rhai::{Array, Dynamic, Engine};
use stir_syntax::SyntaxKind::*;
use stir_syntax::SyntaxNode;

use super::ScriptCtx;

/// A call of `uses()`, or of `pest()` with what comes after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PestUses {
    /// The classes and traits, in order.
    classes: Vec<String>,
    /// What `->in()` says, absolute: the file it's in, unless it says.
    targets: Vec<PathBuf>,
}

/// The expressions passed to a call, in order.
fn arguments(call: &SyntaxNode) -> Vec<SyntaxNode> {
    call.children()
        .find(|c| c.kind() == ARG_LIST)
        .map(|l| {
            l.children()
                .filter(|a| matches!(a.kind(), ARG | NAMED_ARG))
                .filter_map(|a| a.children().last())
                .collect()
        })
        .unwrap_or_default()
}

fn name_in(node: &SyntaxNode) -> Option<String> {
    node.children()
        .find(|c| matches!(c.kind(), NAME | NAME_REF))
        .map(|n| n.text().to_string().trim().to_owned())
}

/// What's said of test cases in the statement `stmt`, if it's about them.
fn read_uses(stmt: &SyntaxNode, resolve: &dyn Fn(&str, &SyntaxNode) -> String) -> Option<PestUses> {
    // the calls from the outermost in, down to `uses()` or `pest()`
    let mut chain = Vec::new();
    let mut at = stmt.children().next()?;
    loop {
        match at.kind() {
            METHOD_CALL_EXPR => {
                let fetch = at.children().next()?;
                chain.push((name_in(&fetch)?.to_ascii_lowercase(), arguments(&at)));
                at = fetch.children().next()?;
            }
            CALL_EXPR => break,
            _ => return None,
        }
    }
    let callee = at.children().next()?.text().to_string();
    let first = match callee.trim().trim_start_matches('\\') {
        "uses" => arguments(&at),
        "pest" => Vec::new(),
        _ => return None,
    };
    let mut uses = PestUses {
        classes: Vec::new(),
        targets: Vec::new(),
    };
    let class_of = |arg: &SyntaxNode| -> Option<String> {
        let text = arg.text().to_string();
        let name = text.trim().strip_suffix("::class")?;
        Some(resolve(name, arg))
    };
    uses.classes.extend(first.iter().filter_map(class_of));
    let mut targets = Vec::new();
    for (name, args) in chain.iter().rev() {
        match name.as_str() {
            "extend" | "extends" | "use" | "uses" => {
                uses.classes.extend(args.iter().filter_map(class_of));
            }
            "in" => targets.extend(args.iter().filter_map(target_of)),
            _ => {}
        }
    }
    uses.targets = targets;
    Some(uses)
}

/// A path as `->in()` is given it, relative to the folder of its file: `'Feature'`, `__DIR__`, or
/// `__DIR__.'/Feature'`.
fn target_of(arg: &SyntaxNode) -> Option<PathBuf> {
    let text = arg.text().to_string();
    let text = text.trim();
    let unquoted = |s: &str| stir_hir::type_ref::unquote(s.trim());
    if text == "__DIR__" {
        return Some(PathBuf::from("."));
    }
    if let Some(rest) = text.strip_prefix("__DIR__") {
        let rest = rest.trim_start().strip_prefix('.')?;
        return Some(PathBuf::from(unquoted(rest).trim_start_matches('/')));
    }
    if arg.kind() == STRING_LITERAL {
        return Some(PathBuf::from(unquoted(text)));
    }
    None
}

/// `a/../b` is `b`.
fn normal(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// A part of a path against a part of a glob with `*` in it.
fn part_matches(glob: &str, part: &str) -> bool {
    let mut pieces = glob.split('*');
    let first = pieces.next().unwrap_or_default();
    let Some(mut rest) = part.strip_prefix(first) else {
        return false;
    };
    let pieces: Vec<&str> = pieces.collect();
    let Some((last, middle)) = pieces.split_last() else {
        return rest.is_empty();
    };
    for piece in middle {
        match rest.find(piece) {
            Some(at) => rest = &rest[at + piece.len()..],
            None => return false,
        }
    }
    rest.ends_with(last)
}

/// Is `file` at or under `target`, which can have `*` in it as for `glob()`?
fn is_within(file: &Path, target: &Path) -> bool {
    let mut parts = file.components();
    target.components().all(|t| {
        parts.next().is_some_and(|p| {
            part_matches(
                &t.as_os_str().to_string_lossy(),
                &p.as_os_str().to_string_lossy(),
            )
        })
    })
}

fn statements(root: &SyntaxNode) -> Vec<SyntaxNode> {
    let mut out = Vec::new();
    for child in root.children() {
        match child.kind() {
            EXPR_STMT => out.push(child),
            NAMESPACE_DECL => {
                let block = child.children().find(|c| c.kind() == BLOCK);
                let within = block.as_ref().unwrap_or(&child);
                out.extend(within.children().filter(|c| c.kind() == EXPR_STMT));
            }
            _ => {}
        }
    }
    out
}

/// What `file` itself says of test cases.
#[salsa::tracked(returns(ref))]
pub fn pest_uses_in(db: &dyn Db, file: SourceFile) -> Vec<PestUses> {
    let root = stir_db::parse(db, file).syntax();
    let items = stir_hir::file_items(db, file);
    let resolve = |name: &str, at: &SyntaxNode| {
        items
            .scope_at(at.text_range().start())
            .resolve_class(name)
            .as_str()
            .to_owned()
    };
    let dir = file
        .path(db)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    statements(&root)
        .iter()
        .filter_map(|stmt| read_uses(stmt, &resolve))
        .map(|mut uses| {
            for target in &mut uses.targets {
                *target = normal(&dir.join(&*target));
            }
            uses
        })
        .collect()
}

/// The configuration files of a project's tests.
#[salsa::tracked(returns(ref))]
pub fn pest_files(db: &dyn Db, ws: Workspace) -> Vec<SourceFile> {
    ws.files(db)
        .iter()
        .copied()
        .filter(|f| f.origin(db) == FileOrigin::Source)
        .filter(|f| f.path(db).file_name().is_some_and(|n| n == "Pest.php"))
        .collect()
}

/// The properties `$this` gets in `file`, by `$this->name = ..`.
#[salsa::tracked(returns(ref))]
pub fn pest_properties_in(db: &dyn Db, file: SourceFile) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if !file.text(db).contains("$this->") {
        return out;
    }
    let root = stir_db::parse(db, file).syntax();
    for assign in root.descendants().filter(|n| n.kind() == ASSIGN_EXPR) {
        let Some(mut target) = assign.children().next() else {
            continue;
        };
        // `$this->rows['a'] = ..`
        while target.kind() == INDEX_EXPR {
            let Some(base) = target.children().next() else {
                break;
            };
            target = base;
        }
        if target.kind() != PROPERTY_FETCH_EXPR {
            continue;
        }
        let on_this = target
            .children()
            .next()
            .is_some_and(|base| base.text().to_string().trim() == "$this");
        if !on_this {
            continue;
        }
        if let Some(name) = name_in(&target)
            && !out.contains(&name)
        {
            out.push(name);
        }
    }
    out
}

/// The classes and traits the test case of `file` is made of, with the last class being its base.
fn uses_of(ctx: &mut ScriptCtx) -> Array {
    let (ws, file) = ctx.place();
    stir_db::attached(|db| {
        let path = normal(file.path(db));
        let mut classes: Vec<&String> = Vec::new();
        for &config in pest_files(db, ws) {
            if config == file {
                continue;
            }
            for uses in pest_uses_in(db, config) {
                if uses.targets.iter().any(|t| is_within(&path, t)) {
                    classes.extend(&uses.classes);
                }
            }
        }
        for uses in pest_uses_in(db, file) {
            if uses.targets.is_empty() || uses.targets.iter().any(|t| is_within(&path, t)) {
                classes.extend(&uses.classes);
            }
        }
        classes
            .iter()
            .map(|c| Dynamic::from((*c).clone()))
            .collect()
    })
    .unwrap_or_default()
}

/// What's set on `$this` in the file's hooks and tests, and in the configuration's hooks.
fn properties_of(ctx: &mut ScriptCtx) -> Array {
    let (ws, file) = ctx.place();
    stir_db::attached(|db| {
        let configs = pest_files(db, ws).iter().filter(|f| **f != file);
        let mut names: Vec<&String> = pest_properties_in(db, file).iter().collect();
        for &config in configs {
            names.extend(pest_properties_in(db, config));
        }
        names.iter().map(|n| Dynamic::from((*n).clone())).collect()
    })
    .unwrap_or_default()
}

pub(super) fn register(engine: &mut Engine) {
    engine
        .register_fn("pest_uses", uses_of)
        .register_fn("pest_properties", properties_of);
}
