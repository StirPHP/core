//! Running rules over a file: a walk of its tree, calling what's subscribed to each node.

use std::sync::Arc;

use rustc_hash::FxHashSet;
use stir_db::{Db, Diagnostic, FileOrigin, ScriptFile, SourceFile, Workspace};
use stir_script::rhai::Dynamic;

use super::{ENGINE, Node, ScriptCtx, ScriptFailure, compiled_script, node, script_index};

/// What rules found in a file.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RuleCheck {
    pub diags: Arc<Vec<Diagnostic>>,
    pub failures: Vec<ScriptFailure>,
}

/// Runs the rules of `ws` over `file`. A query of its own: an edit runs them for what it affects,
/// which unless they ask for types is only the file edited, and editing a rule runs just rules.
#[salsa::tracked(returns(ref))]
pub fn rule_check(db: &dyn Db, ws: Workspace, file: SourceFile) -> RuleCheck {
    let index = script_index(db, ws);
    if index.visits.is_empty() || file.origin(db) != FileOrigin::Source {
        return RuleCheck::default();
    }
    stir_script::with_stack(|| run_rules(db, ws, file))
}

fn run_rules(db: &dyn Db, ws: Workspace, file: SourceFile) -> RuleCheck {
    let index = script_index(db, ws);
    let parse = stir_db::parse(db, file);
    let ctx = ScriptCtx::for_rules(ws, file);
    let mut failures = Vec::new();
    // a script that failed stays out for the rest of the file: it would likely fail again, and
    // take as long about it, on every node
    let mut failed: FxHashSet<ScriptFile> = FxHashSet::default();
    for syntax in parse.syntax().descendants() {
        let Some(rules) = index.visits.get(&syntax.kind()) else {
            continue;
        };
        if node::php_kind(&syntax).is_none() {
            continue;
        }
        let Some(visited) = Node::new(parse.green(), file, &syntax) else {
            continue;
        };
        for (script, function) in rules {
            if failed.contains(script) {
                continue;
            }
            // nothing holds an edit up for long: it stops whatever's between calls
            db.unwind_if_revision_cancelled();
            let Some(ast) = &compiled_script(db, *script).ast else {
                continue;
            };
            let args = vec![
                Dynamic::from(visited.clone()),
                Dynamic::from(ctx.at(visited.range())),
            ];
            if let Err(error) = stir_script::call(&ENGINE, ast, function, args) {
                let path = Some(file.path(db).clone());
                failures.push(ScriptFailure::new(*script, function, error, path));
                failed.insert(*script);
            }
        }
    }
    RuleCheck {
        diags: Arc::new(ctx.take_reports()),
        failures,
    }
}
