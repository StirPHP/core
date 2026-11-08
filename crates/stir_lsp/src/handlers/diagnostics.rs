//! What's published as a file's diagnostics, and what's been published already.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ls_types::{
    CodeDescription, DiagnosticRelatedInformation, DiagnosticSeverity, DiagnosticTag, Location,
    NumberOrString, Uri,
};
use rustc_hash::{FxHashMap, FxHashSet};
use stir_db::{Diagnostic, RootDatabase, Severity, SourceFile, Workspace};

use crate::convert::{Encoding, LineMap};

/// Where PHPStan says what an identifier of its means.
fn phpstan_docs(identifier: &str) -> Option<CodeDescription> {
    let href = format!("https://phpstan.org/error-identifiers/{identifier}");
    Some(CodeDescription {
        href: href.parse().ok()?,
    })
}

pub fn lsp_diagnostic(d: &Diagnostic, map: &LineMap, uri: &Uri) -> ls_types::Diagnostic {
    let mut message = d.message.clone();
    for note in &d.notes {
        message.push('\n');
        message.push_str(note);
    }
    let related = (!d.labels.is_empty()).then(|| {
        d.labels
            .iter()
            .map(|(r, l)| DiagnosticRelatedInformation {
                location: Location::new(uri.clone(), map.range(*r)),
                message: l.clone(),
            })
            .collect()
    });
    ls_types::Diagnostic {
        range: map.range(d.range),
        severity: Some(match d.severity {
            Severity::Error => DiagnosticSeverity::ERROR,
            Severity::Warning => DiagnosticSeverity::WARNING,
            Severity::Hint => DiagnosticSeverity::HINT,
        }),
        // a report of PHPStan's may have no identifier
        code: (!d.code.is_empty()).then(|| NumberOrString::String(d.code.to_string())),
        code_description: (d.from_phpstan && !d.code.is_empty())
            .then(|| phpstan_docs(&d.code))
            .flatten(),
        source: Some(if d.from_phpstan { "phpstan" } else { "stirphp" }.into()),
        message,
        related_information: related,
        tags: if d.code == stir_checker::codes::UNUSED_IMPORT {
            Some(vec![DiagnosticTag::UNNECESSARY])
        } else {
            stir_checker::deprecations::is_deprecation(&d.code)
                .then(|| vec![DiagnosticTag::DEPRECATED])
        },
        ..Default::default()
    }
}

/// `diags` of the file at `uri` with `text`, as the protocol has them.
pub fn to_lsp(
    diags: &[Diagnostic],
    text: &str,
    enc: Encoding,
    uri: &Uri,
) -> Vec<ls_types::Diagnostic> {
    let map = LineMap::new(text, enc);
    diags.iter().map(|d| lsp_diagnostic(d, &map, uri)).collect()
}

/// What we say of a project source: what checking it finds, what's deprecated (struck through,
/// whatever the project says of reporting it) and imports nothing uses.
pub fn of_source(db: &RootDatabase, ws: Workspace, file: SourceFile) -> Vec<Diagnostic> {
    let mut diags = stir_checker::check_file(db, ws, file).as_ref().clone();
    if !ws.settings(db).deprecations {
        let marks = stir_checker::deprecations::deprecations(db, ws, file);
        diags.extend(
            marks
                .iter()
                .cloned()
                .map(|d| d.with_severity(Severity::Hint)),
        );
    }
    diags.extend(crate::actions::unused_import_hints(db, file));
    diags
}

/// What's wrong with the scripts that ran on `sources`, by script: its path, its text and that.
pub fn of_scripts(
    db: &RootDatabase,
    ws: Workspace,
    sources: &[SourceFile],
) -> Vec<(PathBuf, String, Vec<Diagnostic>)> {
    let mut by_script: BTreeMap<PathBuf, Vec<Diagnostic>> = BTreeMap::new();
    for d in stir_checker::script_diagnostics(db, ws, sources) {
        by_script.entry(d.file.clone()).or_default().push(d);
    }
    by_script
        .into_iter()
        .map(|(path, diags)| {
            let script = ws.scripts(db).iter().find(|s| s.path(db) == &path);
            let text = script
                .and_then(|s| s.text(db).as_ref().ok())
                .map_or("", String::as_str)
                .to_owned();
            (path, text, diags)
        })
        .collect()
}

/// What's been published, so that only what changes is published again.
#[derive(Default)]
pub struct Published {
    files: FxHashMap<PathBuf, Vec<Diagnostic>>,
}

impl Published {
    /// Are `diags` news for `path`? They're taken as published if they are.
    pub fn is_news(&mut self, path: &Path, diags: &[Diagnostic]) -> bool {
        if self.files.get(path).is_some_and(|d| d == diags) {
            return false;
        }
        self.files.insert(path.to_owned(), diags.to_vec());
        true
    }

    /// What was published and isn't among `seen` any more: its diagnostics are to be cleared.
    pub fn gone(&mut self, seen: &FxHashSet<PathBuf>) -> Vec<PathBuf> {
        let gone: Vec<PathBuf> = self
            .files
            .keys()
            .filter(|p| !seen.contains(*p))
            .cloned()
            .collect();
        for path in &gone {
            self.files.remove(path);
        }
        gone
    }
}
