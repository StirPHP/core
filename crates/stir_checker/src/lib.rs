//! The StirPHP type checker.
//!
//! Entry point: [`check_file`], a salsa query returning every diagnostic of a
//! file (syntax errors included).
//!
//! Project `.php` files are checked unless they opt out with a leading
//! `// @stirphp-nocheck` comment; their diagnostics are filtered by the PHPStan
//! rule level and `ignoreErrors` (see [`levels`]).

#![allow(clippy::nonminimal_bool)]

use std::sync::Arc;

use stir_db::{Db, Diagnostic, FileOrigin, IgnoreRule, SourceFile, Workspace};

mod body;
pub mod codes;
mod infer;
pub mod levels;
mod model;
pub mod model_api;
pub mod php_version;
pub mod script;
mod typed;

pub use body::{Checker, LocalKind};
pub use infer::widen_literals;
pub use model::Ctx;
pub use script::{ScriptFailure, script_diagnostics};
pub use typed::{Def, MemberKind, TypedFile, typed_file};

/// What checking a file comes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileCheck {
    pub diags: Arc<Vec<Diagnostic>>,
    /// How many diagnostics each rule of the baseline hid here, by the rule's index.
    pub baseline_hits: Vec<(u32, u32)>,
    /// What went wrong with the project's scripts on this file. Not its fault, so not its problem:
    /// see [`script_diagnostics`].
    pub script_failures: Vec<ScriptFailure>,
}

/// All diagnostics of `file` within `ws`.
pub fn check_file(db: &dyn Db, ws: Workspace, file: SourceFile) -> &Arc<Vec<Diagnostic>> {
    &checked_file(db, ws, file).diags
}

/// [`check_file`], and what it's known from.
#[salsa::tracked(returns(ref))]
pub fn checked_file(db: &dyn Db, ws: Workspace, file: SourceFile) -> FileCheck {
    let settings = ws.settings(db);
    if file.origin(db) == FileOrigin::Source
        && (levels::is_nocheck(file.text(db)) || !levels::is_analysed(file.path(db), settings))
    {
        return FileCheck {
            diags: Arc::new(Vec::new()),
            baseline_hits: Vec::new(),
            script_failures: Vec::new(),
        };
    }
    let mut diags = stir_db::syntax_diagnostics(db, file).as_ref().clone();
    let mut script_failures = Vec::new();
    // Semantic errors on a broken tree are mostly noise: fix syntax first.
    if file.origin(db) == FileOrigin::Source && diags.is_empty() {
        diags.extend(
            php_version::syntax_version_diagnostics(db, ws, file)
                .iter()
                .cloned(),
        );
        let typed = typed_file(db, ws, file);
        diags.extend(typed.diags.iter().cloned());
        script_failures.extend(typed.script_failures.iter().cloned());
        // what rules report goes the way of everything else from here on
        let rules = script::rule_check(db, ws, file);
        diags.extend(rules.diags.iter().cloned());
        script_failures.extend(rules.failures.iter().cloned());
    }
    levels::filter_php(&mut diags, settings);
    diags.sort_by_key(|d| (d.range.start(), d.range.end()));
    diags.dedup();
    // in PHPStan's order, so that a comment doesn't take away from a `count`
    levels::apply_phpstan_ignores(
        &mut diags,
        &stir_db::parse(db, file).syntax(),
        file.text(db),
        file.path(db),
    );
    let path = file.path(db);
    levels::apply_ignore_rules(&mut diags, path, &settings.ignore_errors);
    let baseline_hits = levels::apply_ignore_rules(&mut diags, path, &settings.baseline);
    diags.sort_by_key(|d| (d.range.start(), d.range.end()));
    FileCheck {
        diags: Arc::new(diags),
        baseline_hits,
        script_failures,
    }
}

/// An entry of the baseline that hides fewer diagnostics than it says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnmatchedRule {
    pub rule: IgnoreRule,
    /// How many it did hide.
    pub hidden: u32,
}

/// The entries of the baseline with less to hide than they say in `files`, the files that were
/// checked: PHPStan's `reportUnmatched`. Only for the baseline, not `ignoreErrors`: PHPStan's
/// configuration is written for what PHPStan reports, which is not all this checker's. A file
/// with syntax errors says nothing, because it only has those until they're fixed; nor does
/// a project whose packages aren't installed.
pub fn unmatched_baseline(db: &dyn Db, ws: Workspace, files: &[SourceFile]) -> Vec<UnmatchedRule> {
    let settings = ws.settings(db);
    let rules = settings.baseline.rules();
    if rules.is_empty() || settings.pending_install.is_some() {
        return Vec::new();
    }
    let mut hidden = vec![0u32; rules.len()];
    let mut unknown = vec![false; rules.len()];
    for &file in files {
        for &(rule, hits) in &checked_file(db, ws, file).baseline_hits {
            hidden[rule as usize] += hits;
        }
        if !stir_db::syntax_diagnostics(db, file).is_empty() {
            let path = file.path(db);
            for (i, _) in settings.baseline.for_file(path, &levels::path_matches) {
                unknown[i as usize] = true;
            }
        }
    }
    rules
        .iter()
        .enumerate()
        .filter(|(i, r)| {
            let enough = r.count.map_or(hidden[*i] > 0, |n| hidden[*i] >= n);
            r.report_unmatched && !enough && !unknown[*i]
        })
        .map(|(i, r)| UnmatchedRule {
            rule: r.clone(),
            hidden: hidden[i],
        })
        .collect()
}

/// How many errors about unknown names [`check_file`] leaves out of `file`, because Composer's
/// packages aren't installed as declared.
pub fn held_back_count(db: &dyn Db, ws: Workspace, file: SourceFile) -> usize {
    let settings = ws.settings(db);
    if settings.pending_install.is_none() || file.origin(db) != FileOrigin::Source {
        return 0;
    }
    if !stir_db::syntax_diagnostics(db, file).is_empty()
        || levels::is_nocheck(file.text(db))
        || !levels::is_analysed(file.path(db), settings)
    {
        return 0;
    }
    typed_file(db, ws, file)
        .diags
        .iter()
        .filter(|d| levels::is_held_back(d, settings) && levels::min_level(d) <= settings.level())
        .count()
}

#[cfg(test)]
mod interop_tests;
