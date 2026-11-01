//! StirPHP in the browser: checks, compiles, hovers and completes one `.sphp`
//! file, for the documentation's playground.
//!
//! The database lives as long as the [`Playground`], so each call after an
//! edit only recomputes what the edit affected. Positions are 1-based lines
//! and UTF-16 columns (what Monaco uses); results are JSON strings.

use salsa::Setter as _;
use serde::Serialize;
use stir_db::{FileOrigin, RootDatabase, Settings, Severity, SourceFile, Workspace};
use stir_lsp::convert::{Encoding, LineMap};
use stir_syntax::{Dialect, TextRange, TextSize};
use wasm_bindgen::prelude::*;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Span {
    start_line: u32,
    start_column: u32,
    end_line: u32,
    end_column: u32,
}

#[derive(Serialize)]
struct Diagnostic {
    #[serde(flatten)]
    span: Span,
    severity: &'static str,
    code: &'static str,
    message: String,
}

#[derive(Serialize)]
struct Compiled {
    php: String,
    errors: usize,
}

#[derive(Serialize)]
struct Hover {
    #[serde(flatten)]
    span: Span,
    markdown: String,
}

#[derive(Serialize)]
struct Completion {
    label: String,
    kind: &'static str,
    detail: Option<String>,
    insert: String,
    sort: String,
}

#[derive(Serialize)]
struct Completions {
    #[serde(flatten)]
    span: Span,
    items: Vec<Completion>,
}

/// A single-file StirPHP workspace.
#[wasm_bindgen]
pub struct Playground {
    db: RootDatabase,
    ws: Workspace,
    file: SourceFile,
}

impl Default for Playground {
    fn default() -> Self {
        Self::new()
    }
}

#[wasm_bindgen]
impl Playground {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Playground {
        let db = RootDatabase::default();
        let file = SourceFile::new(
            &db,
            "/playground/main.sphp".into(),
            String::new(),
            Dialect::Stir,
            FileOrigin::Source,
        );
        let mut files = vec![file];
        files.extend(stir_stubs::stub_files().map(|s| {
            SourceFile::new(
                &db,
                format!("stubs://{}", s.path).into(),
                s.text.to_owned(),
                Dialect::Php,
                FileOrigin::Stub,
            )
        }));
        files.extend(stir_stubs::stdlib_files().map(|s| {
            SourceFile::new(
                &db,
                format!("stdlib://{}", s.path).into(),
                s.text.to_owned(),
                Dialect::Stir,
                FileOrigin::Stub,
            )
        }));
        let ws = Workspace::new(&db, files, Settings::default());
        Playground { db, ws, file }
    }

    /// The diagnostics of `source`.
    pub fn check(&mut self, source: &str) -> String {
        self.set(source);
        let map = self.map();
        let diags: Vec<Diagnostic> = stir_checker::check_file(&self.db, self.ws, self.file)
            .iter()
            .map(|d| Diagnostic {
                span: span(&map, d.range),
                severity: match d.severity {
                    Severity::Error => "error",
                    Severity::Warning => "warning",
                    Severity::Hint => "hint",
                },
                code: d.code,
                message: d.message.clone(),
            })
            .collect();
        json(&diags)
    }

    /// `source` transpiled to PHP (also when it has errors), and its error count.
    pub fn compile(&mut self, source: &str, inline_extensions: bool) -> String {
        self.set(source);
        let errors = stir_checker::check_file(&self.db, self.ws, self.file)
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count();
        let opts = stir_emit::EmitOptions {
            inline_extensions,
            runtime_require: Some("__DIR__ . '/stir-runtime.php'".into()),
            ..Default::default()
        };
        let php = stir_emit::transpile(&self.db, self.ws, self.file, &opts).code;
        json(&Compiled { php, errors })
    }

    /// Hover at a position: markdown and the range it is about, or "null".
    pub fn hover(&mut self, source: &str, line: u32, column: u32) -> String {
        self.set(source);
        let map = self.map();
        let offset = offset(&map, line, column);
        match stir_lsp::hover::hover(&self.db, self.ws, self.file, offset) {
            Some(h) => json(&Hover {
                span: span(&map, h.range),
                markdown: h.markdown,
            }),
            None => "null".into(),
        }
    }

    /// Completions at a position and the range they replace.
    pub fn complete(&mut self, source: &str, line: u32, column: u32) -> String {
        self.set(source);
        let map = self.map();
        let offset = offset(&map, line, column);
        let c = stir_lsp::complete::complete(&self.db, self.ws, self.file, offset);
        let items = c
            .items
            .into_iter()
            .map(|i| Completion {
                sort: format!("{}{}", i.rank, i.label),
                kind: kind(i.kind),
                label: i.label,
                detail: i.detail,
                insert: i.insert,
            })
            .collect();
        json(&Completions {
            span: span(&map, c.replace),
            items,
        })
    }
}

impl Playground {
    fn set(&mut self, source: &str) {
        if self.file.text(&self.db) != source {
            self.file.set_text(&mut self.db).to(source.to_owned());
        }
    }

    fn map(&self) -> LineMap {
        LineMap::new(self.file.text(&self.db), Encoding::Utf16)
    }
}

fn span(map: &LineMap, range: TextRange) -> Span {
    let r = map.range(range);
    Span {
        start_line: r.start.line + 1,
        start_column: r.start.character + 1,
        end_line: r.end.line + 1,
        end_column: r.end.character + 1,
    }
}

fn offset(map: &LineMap, line: u32, column: u32) -> TextSize {
    map.offset(stir_lsp::convert::position(
        line.saturating_sub(1),
        column.saturating_sub(1),
    ))
}

fn kind(k: stir_lsp::complete::ItemKind) -> &'static str {
    use stir_lsp::complete::ItemKind as K;
    match k {
        K::Local | K::Param => "variable",
        K::Function => "function",
        K::Method => "method",
        K::Property => "property",
        K::Field => "field",
        K::Const => "constant",
        K::EnumCase => "enumMember",
        K::Class => "class",
        K::Interface => "interface",
        K::Enum => "enum",
        K::Struct => "struct",
        K::Keyword => "keyword",
        K::Type => "class",
    }
}

fn json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_compiles_hovers_and_completes() {
        let mut p = Playground::new();
        let bad = "val n: Int = \"x\"\n";
        let diags: serde_json::Value = serde_json::from_str(&p.check(bad)).unwrap();
        assert_eq!(diags[0]["code"], "SPHP0005");
        assert_eq!(diags[0]["startLine"], 1);
        assert_eq!(diags[0]["startColumn"], 14);

        let ok = "val s = \"abc\"->uppercase()\necho s\n";
        let out: serde_json::Value = serde_json::from_str(&p.compile(ok, true)).unwrap();
        assert_eq!(out["errors"], 0);
        assert!(out["php"].as_str().unwrap().contains("mb_strtoupper"));

        let hover: serde_json::Value = serde_json::from_str(&p.hover(ok, 2, 7)).unwrap();
        assert!(
            hover["markdown"]
                .as_str()
                .unwrap()
                .contains("val s: String")
        );

        let partial = "val s = \"abc\"\necho s->\n";
        let done: serde_json::Value = serde_json::from_str(&p.complete(partial, 2, 9)).unwrap();
        let labels: Vec<&str> = done["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["label"].as_str().unwrap())
            .collect();
        assert!(labels.contains(&"uppercase"), "{labels:?}");
    }
}
