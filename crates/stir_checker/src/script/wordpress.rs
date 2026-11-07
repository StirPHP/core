//! WordPress's hooks, as the project's code runs them: each `do_action('name', ..)` and
//! `apply_filters('name', ..)` with a name written out, and what it passes. For scripts to check
//! what's hooked onto them (`wordpress_hook(name)`), and for editors to go from a hook's name to
//! where it runs.

use std::collections::BTreeMap;

use stir_db::{Db, FileOrigin, SourceFile, Workspace};
use stir_script::rhai::{Array, Dynamic, Engine, INT, Map};
use stir_syntax::SyntaxKind::*;
use stir_syntax::{SyntaxNode, TextRange};

/// Where a hook runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookRun {
    pub file: SourceFile,
    /// The hook's name, as written.
    pub range: TextRange,
    pub filter: bool,
    /// How many arguments its callbacks get, if that's known: none for `do_action_ref_array`.
    pub args: Option<u32>,
}

/// Every hook run with a written name, by name.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Hooks {
    pub runs: BTreeMap<String, Vec<HookRun>>,
}

/// The functions that run hooks: whether it's a filter, and whether what's passed is one array.
const RUNNERS: &[(&str, bool, bool)] = &[
    ("do_action", false, false),
    ("do_action_ref_array", false, true),
    ("do_action_deprecated", false, true),
    ("apply_filters", true, false),
    ("apply_filters_ref_array", true, true),
    ("apply_filters_deprecated", true, true),
];

/// See [`Hooks`]: the project's files and its packages', WordPress itself among them when it's
/// there as source. Only files that mention a runner are read.
#[salsa::tracked(returns(ref))]
pub fn wordpress_hooks(db: &dyn Db, ws: Workspace) -> Hooks {
    let mut hooks = Hooks::default();
    for &file in ws.files(db) {
        if file.origin(db) == FileOrigin::Stub {
            continue;
        }
        let text = file.text(db);
        if !text.contains("do_action") && !text.contains("apply_filters") {
            continue;
        }
        let root = stir_db::parse(db, file).syntax();
        for call in root.descendants().filter(|n| n.kind() == CALL_EXPR) {
            let Some((runner, filter, by_array)) = runner_of(&call) else {
                continue;
            };
            let args = arguments(&call);
            let Some(name) = args.first().and_then(string_value) else {
                continue;
            };
            let passed = (args.len() as u32).saturating_sub(1);
            let args = if by_array {
                // `do_action_ref_array('x', [$a, $b])`: what's in the array, if it's written out
                args.get(1)
                    .filter(|a| a.kind() == ARRAY_EXPR && runner.ends_with("ref_array"))
                    .map(|a| a.children().filter(|e| e.kind() == ARRAY_ELEM).count() as u32)
            } else {
                Some(passed)
            };
            hooks.runs.entry(name.0).or_default().push(HookRun {
                file,
                range: name.1,
                filter,
                args,
            });
        }
    }
    hooks
}

fn runner_of(call: &SyntaxNode) -> Option<(&'static str, bool, bool)> {
    let callee = call.children().find(|c| c.kind() == NAME_EXPR)?;
    let name = callee.text().to_string();
    let name = name.trim().trim_start_matches('\\');
    RUNNERS
        .iter()
        .find(|(n, _, _)| n.eq_ignore_ascii_case(name))
        .copied()
}

/// The expressions passed to a call, in order.
pub(crate) fn arguments(call: &SyntaxNode) -> Vec<SyntaxNode> {
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

/// A string literal's value and range.
pub(crate) fn string_value(node: &SyntaxNode) -> Option<(String, TextRange)> {
    if node.kind() != STRING_LITERAL {
        return None;
    }
    let text = node.text().to_string();
    let trimmed = text.trim();
    if trimmed.starts_with('"') && trimmed.contains('$') {
        return None;
    }
    Some((stir_hir::type_ref::unquote(trimmed), node.text_range()))
}

/// `wordpress_hook(name)`: where it runs, as `#{ filter, args }`, `args` being `()` where it isn't
/// known. None for a hook nothing here runs, as WordPress's own hooks aren't, when only its stubs
/// are there.
fn hook(name: &str) -> Array {
    stir_db::attached(|db| {
        let Some(ws) = Workspace::try_get(db) else {
            return Array::new();
        };
        let Some(runs) = wordpress_hooks(db, ws).runs.get(name) else {
            return Array::new();
        };
        runs.iter()
            .map(|r| {
                let mut run = Map::new();
                run.insert("filter".into(), r.filter.into());
                run.insert(
                    "args".into(),
                    r.args.map_or(Dynamic::UNIT, |n| Dynamic::from(n as INT)),
                );
                Dynamic::from(run)
            })
            .collect()
    })
    .unwrap_or_default()
}

pub(super) fn register(engine: &mut Engine) {
    engine.register_fn("wordpress_hook", hook);
}
