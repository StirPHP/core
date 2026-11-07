//! StirPHP in the browser: checks, hovers and completes one PHP file.
//!
//! The database lives as long as the [`Playground`], so each call after an
//! edit only recomputes what the edit affected. Positions are 1-based lines
//! and UTF-16 columns (what Monaco uses); results are JSON strings.

use salsa::Setter as _;
use serde::Serialize;
use stir_db::{FileOrigin, RootDatabase, ScriptFile, Settings, Severity, SourceFile, Workspace};
use stir_lsp::convert::{Encoding, LineMap};
use stir_syntax::{TextRange, TextSize};
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
    code: String,
    message: String,
    /// The rule script this is about, if it's not about the source.
    #[serde(skip_serializing_if = "Option::is_none")]
    script: Option<String>,
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

/// A single-file workspace.
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
            "/playground/main.php".into(),
            String::new(),
            FileOrigin::Source,
        );
        let mut files = vec![file];
        files.extend(stir_stubs::stub_files().map(|s| {
            SourceFile::new(
                &db,
                format!("stubs://{}", s.path).into(),
                s.text.to_owned(),
                FileOrigin::Stub,
            )
        }));
        let ws = Workspace::new(&db, files, Settings::default());
        Playground { db, ws, file }
    }

    /// Adds a rule script, in the sandbox `stirphp check` runs them in. A page should run only
    /// what its visitor chose to.
    #[wasm_bindgen(js_name = addRule)]
    pub fn add_rule(&mut self, name: &str, source: &str) {
        // a visitor's script is never taken for one of ours
        let name = name.trim_start_matches(stir_db::BUNDLED_PREFIX);
        let script = ScriptFile::new(&self.db, name.into(), Ok(source.to_owned()));
        let mut scripts = self.ws.scripts(&self.db).clone();
        scripts.push(script);
        let _ = self.ws.set_scripts(&mut self.db).to(scripts);
    }

    /// Turns on what StirPHP ships for a framework (`laravel`, `symfony` or `wordpress`), as a
    /// project using it would have: its bundled script and stubs. False for one there's none for.
    #[wasm_bindgen(js_name = useFramework)]
    pub fn use_framework(&mut self, name: &str) -> bool {
        let Some(framework) = stir_stubs::frameworks::framework(name) else {
            return false;
        };
        let path = format!("{}{}", stir_db::BUNDLED_PREFIX, framework.script_name);
        let mut scripts = self.ws.bundled_scripts(&self.db).clone();
        if scripts
            .iter()
            .any(|s| s.path(&self.db).to_str() == Some(path.as_str()))
        {
            return true;
        }
        scripts.push(ScriptFile::new(
            &self.db,
            path.into(),
            Ok(framework.script.to_owned()),
        ));
        let mut files = self.ws.files(&self.db).clone();
        files.extend(framework.stubs.iter().map(|(path, text)| {
            SourceFile::new(
                &self.db,
                format!("stubs://{path}").into(),
                (*text).to_owned(),
                FileOrigin::Stub,
            )
        }));
        let _ = self.ws.set_bundled_scripts(&mut self.db).to(scripts);
        let _ = self.ws.set_files(&mut self.db).to(files);
        true
    }

    /// Removes all rule scripts.
    #[wasm_bindgen(js_name = clearRules)]
    pub fn clear_rules(&mut self) {
        let _ = self.ws.set_scripts(&mut self.db).to(Vec::new());
    }

    /// The diagnostics of `source`, and what's wrong with the rule scripts.
    pub fn check(&mut self, source: &str) -> String {
        self.set(source);
        let map = self.map();
        let found = stir_checker::check_file(&self.db, self.ws, self.file);
        let mut diags: Vec<Diagnostic> = found.iter().map(|d| diagnostic(d, &map, None)).collect();
        for d in stir_checker::script_diagnostics(&self.db, self.ws, &[self.file]) {
            let name = d.file.display().to_string();
            let script = self
                .ws
                .scripts(&self.db)
                .iter()
                .find(|s| s.path(&self.db) == &d.file);
            let text = script.and_then(|s| s.text(&self.db).as_ref().ok());
            let map = LineMap::new(text.map_or("", String::as_str), Encoding::Utf16);
            diags.push(diagnostic(&d, &map, Some(name)));
        }
        json(&diags)
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

fn diagnostic(d: &stir_db::Diagnostic, map: &LineMap, script: Option<String>) -> Diagnostic {
    Diagnostic {
        span: span(map, d.range),
        severity: match d.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Hint => "hint",
        },
        code: d.code.to_string(),
        message: d.message.clone(),
        script,
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
        K::Const => "constant",
        K::EnumCase => "enumMember",
        K::Class => "class",
        K::Interface => "interface",
        K::Enum => "enum",
        K::Keyword => "keyword",
    }
}

fn json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_hovers_and_completes() {
        let mut p = Playground::new();
        let bad = "<?php\necho missing();\n";
        let diags: serde_json::Value = serde_json::from_str(&p.check(bad)).unwrap();
        assert_eq!(diags[0]["code"], "function.notFound");
        assert_eq!(diags[0]["startLine"], 2);
        assert_eq!(diags[0]["startColumn"], 6);

        let ok = "<?php\n$s = strlen('abc');\necho $s;\n";
        let hover: serde_json::Value = serde_json::from_str(&p.hover(ok, 3, 7)).unwrap();
        assert!(
            hover["markdown"].as_str().unwrap().contains("int"),
            "{hover}"
        );

        let partial =
            "<?php\nclass Box { public function open(): void {} }\n$b = new Box();\n$b->\n";
        let done: serde_json::Value = serde_json::from_str(&p.complete(partial, 4, 5)).unwrap();
        let labels: Vec<&str> = done["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["label"].as_str().unwrap())
            .collect();
        assert!(labels.contains(&"open"), "{labels:?}");
    }

    #[test]
    fn runs_rules_and_says_what_is_wrong_with_them() {
        let mut p = Playground::new();
        let source = "<?php\nvar_dump(1);\n";
        let check = |p: &mut Playground| -> serde_json::Value {
            serde_json::from_str(&p.check(source)).unwrap()
        };
        assert_eq!(check(&mut p).as_array().unwrap().len(), 0);

        p.add_rule(
            "no_var_dump.rhai",
            r#"fn visit_func_call(node, ctx) {
                if node.name == "var_dump" { ctx.error("no var_dump", "app.noVarDump"); }
            }"#,
        );
        let diags = check(&mut p);
        assert_eq!(diags.as_array().unwrap().len(), 1, "{diags}");
        assert_eq!(diags[0]["code"], "app.noVarDump");
        assert_eq!(diags[0]["startLine"], 2);
        assert!(diags[0].get("script").is_none());

        p.clear_rules();
        p.add_rule("bad.rhai", "fn visit_func_call(node, ctx) {\n  let = 1;\n}");
        let diags = check(&mut p);
        assert_eq!(diags[0]["code"], "SPHP0071", "{diags}");
        assert_eq!(diags[0]["script"], "bad.rhai");
        assert_eq!(diags[0]["startLine"], 2);

        p.clear_rules();
        p.add_rule("slow.rhai", "fn visit_func_call(node, ctx) { loop {} }");
        let diags = check(&mut p);
        assert_eq!(diags[0]["code"], "SPHP0070", "{diags}");

        p.clear_rules();
        assert_eq!(check(&mut p).as_array().unwrap().len(), 0);
    }
}
