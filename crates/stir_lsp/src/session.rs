//! The language server one message at a time, with no threads, clock or disk of its own: what
//! runs in the browser, in a Web Worker. It answers with [`crate::handlers`], as the native server
//! does.
//!
//! The host hands it JSON-RPC messages ([`Session::push`]) and lets it work in small steps
//! ([`Session::step`]), saying what time it is. A step handles the notifications that came in and at
//! most one request, or checks one file for diagnostics, so that whatever comes in meanwhile (an
//! edit, `$/cancelRequest`) is seen soon. Edits are applied as they come, and diagnostics wait until
//! they've stopped for [`DIAGNOSTICS_DELAY`]. What's to be sent is taken with
//! [`Session::take_outgoing`].
//!
//! There's no disk: the project's files come from the host, in `initializationOptions.files` and
//! then the `stirphp/setFiles` notification, both a map from a path or `file://` URI to its text
//! (`null` deletes it). The project is loaded from them as from a disk (configuration, Composer's
//! files, `vendor/`, frameworks). The builtin stubs have `stirphp:///stubs/...` URIs, and the text of
//! any file the server has is there to ask for with the `stirphp/fileText` request.
//!
//! The project's Rhai rules run unless `initializationOptions.workspaceTrusted` is `false`: the page
//! chose to hand them over, and they run in the sandbox `stirphp check` runs them in, with no file
//! or network access and capped operations. PHPStan and file watching aren't there.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ls_types::*;
use rustc_hash::{FxHashMap, FxHashSet};
use salsa::Setter as _;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use stir_db::{Diagnostic, ScriptFile, SourceFile};
use stir_project::workspace::{LoadOptions, load_workspace_from};
use stir_project::{Fs, Memory, Project};

use crate::convert::{Encoding, LineMap};
use crate::handlers::actions::{self, ActionSupport, Resolved, ToResolve};
use crate::handlers::diagnostics::{self, Published};
use crate::handlers::{self, Failure, Snapshot};
use crate::links::{Links, Virtual};
use crate::state::State;

/// How long edits have to have stopped before diagnostics are worked out, in milliseconds.
pub const DIAGNOSTICS_DELAY: f64 = 150.0;

/// JSON-RPC's error codes, and the protocol's.
mod code {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const REQUEST_CANCELLED: i64 = -32800;
    pub const CONTENT_MODIFIED: i64 = -32801;
}

/// What the host is to do after a step.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Next {
    /// Step again, after letting what's come in arrive.
    Now,
    /// Step again after this many milliseconds, unless a message comes in first.
    After(f64),
    /// Nothing to do until a message comes in.
    Idle,
}

/// A request or notification that came in.
struct Incoming {
    id: Option<Value>,
    method: String,
    params: Value,
}

/// A round of diagnostics: the files left to check.
struct Round {
    files: VecDeque<SourceFile>,
    seen: FxHashSet<PathBuf>,
}

pub struct Session {
    state: State,
    /// The files the host handed over.
    fs: Arc<Memory>,
    enc: Encoding,
    support: ActionSupport,
    folder: Option<PathBuf>,
    /// May the project's scripts run?
    trusted: bool,
    type_hierarchy_dynamic: bool,
    queue: VecDeque<Incoming>,
    outgoing: Vec<Value>,
    /// When diagnostics are due, after the last edit.
    due: Option<f64>,
    /// Something changed since diagnostics were last worked out, and the time isn't set yet.
    edited: bool,
    round: Option<Round>,
    published: Published,
    /// Documents edited most recently last: they're checked first.
    recent: Vec<PathBuf>,
    notices: Vec<String>,
    next_id: i64,
    shut_down: bool,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    pub fn new() -> Session {
        Session {
            state: State::empty(Vec::new()),
            fs: Arc::new(Memory::new()),
            enc: Encoding::Utf16,
            support: ActionSupport::default(),
            folder: None,
            trusted: true,
            type_hierarchy_dynamic: false,
            queue: VecDeque::new(),
            outgoing: Vec::new(),
            due: None,
            edited: false,
            round: None,
            published: Published::default(),
            recent: Vec::new(),
            notices: Vec::new(),
            next_id: 1,
            shut_down: false,
        }
    }

    /// Has the client said `exit`?
    pub fn exited(&self) -> bool {
        self.shut_down && self.queue.is_empty()
    }

    /// Takes in a message: a JSON-RPC request, notification or response, as text.
    pub fn push_text(&mut self, text: &str) {
        match serde_json::from_str::<Value>(text) {
            Ok(message) => self.push(message),
            Err(error) => self.error(Value::Null, code::PARSE_ERROR, error.to_string()),
        }
    }

    /// Takes in a message. `$/cancelRequest` takes effect at once; everything else waits for a
    /// step, in order.
    pub fn push(&mut self, message: Value) {
        if let Value::Array(batch) = message {
            for m in batch {
                self.push(m);
            }
            return;
        }
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            // a response to a request of ours: nothing waits for one
            if message.get("id").is_none() {
                self.error(Value::Null, code::INVALID_REQUEST, "not a message".into());
            }
            return;
        };
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        if method == "$/cancelRequest" {
            let id = params.get("id").cloned().unwrap_or(Value::Null);
            if let Some(i) = self.queue.iter().position(|m| m.id.as_ref() == Some(&id)) {
                self.queue.remove(i);
                self.error(id, code::REQUEST_CANCELLED, "cancelled".into());
            }
            return;
        }
        self.queue.push_back(Incoming {
            id: message.get("id").cloned(),
            method: method.to_owned(),
            params,
        });
    }

    /// The messages to send, in order.
    pub fn take_outgoing(&mut self) -> Vec<Value> {
        std::mem::take(&mut self.outgoing)
    }

    /// Does a little of what there is to do. `now` is the time in milliseconds, from any start
    /// that stays the same.
    pub fn step(&mut self, now: f64) -> Next {
        if !self.queue.is_empty() {
            // the notifications are cheap, and an edit is better seen before what follows it
            while let Some(message) = self.queue.pop_front() {
                let is_request = message.id.is_some();
                self.handle(message);
                if is_request {
                    break;
                }
            }
            if self.edited {
                self.edited = false;
                self.round = None;
                self.due = Some(now + DIAGNOSTICS_DELAY);
            }
            return if self.queue.is_empty() {
                self.next(now)
            } else {
                Next::Now
            };
        }
        match self.due {
            Some(due) if now >= due => {
                self.diagnose();
                self.next(now)
            }
            _ => self.next(now),
        }
    }

    /// Steps until there's nothing left to do, with time going by as needed: what a test (or a
    /// host that doesn't mind waiting) does.
    pub fn run_until_idle(&mut self) {
        let mut now = 0.0;
        loop {
            match self.step(now) {
                Next::Now => {}
                Next::After(ms) => now += ms,
                Next::Idle => break,
            }
        }
    }

    fn next(&self, now: f64) -> Next {
        if !self.queue.is_empty() {
            return Next::Now;
        }
        match self.due {
            Some(due) if due > now => Next::After(due - now),
            Some(_) => Next::Now,
            None => Next::Idle,
        }
    }

    fn send(&mut self, message: Value) {
        self.outgoing.push(message);
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    /// A request of ours to the client. Its answer isn't waited for.
    fn ask(&mut self, method: &str, params: Value) {
        let id = format!("stirphp-{}", self.next_id);
        self.next_id += 1;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
    }

    fn respond(&mut self, id: Value, result: Value) {
        self.send(json!({"jsonrpc": "2.0", "id": id, "result": result}));
    }

    fn error(&mut self, id: Value, code: i64, message: String) {
        self.send(json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}));
    }

    fn show(&mut self, kind: MessageType, message: String) {
        self.notify(
            "window/showMessage",
            json!(ShowMessageParams { typ: kind, message }),
        );
    }

    fn handle(&mut self, message: Incoming) {
        let Incoming { id, method, params } = message;
        match id {
            Some(id) => match self.request(&method, params) {
                Ok(result) => self.respond(id, result),
                Err((code, message)) => self.error(id, code, message),
            },
            None => self.notification(&method, params),
        }
    }

    fn notification(&mut self, method: &str, params: Value) {
        match method {
            "initialized" => self.initialized(),
            "exit" => self.shut_down = true,
            "textDocument/didOpen" => {
                let Ok(p) = parse::<DidOpenTextDocumentParams>(params) else {
                    return;
                };
                let Some(path) = Virtual.path(&p.text_document.uri) else {
                    return;
                };
                self.touched(&path);
                let doc = p.text_document;
                self.state.open(path, doc.version, doc.text);
                self.edited = true;
            }
            "textDocument/didChange" => {
                let Ok(p) = parse::<DidChangeTextDocumentParams>(params) else {
                    return;
                };
                let Some(path) = Virtual.path(&p.text_document.uri) else {
                    return;
                };
                self.touched(&path);
                let enc = self.enc;
                let version = p.text_document.version;
                self.state.change(path, version, p.content_changes, enc);
                self.edited = true;
            }
            "textDocument/didClose" => {
                let Ok(p) = parse::<DidCloseTextDocumentParams>(params) else {
                    return;
                };
                let Some(path) = Virtual.path(&p.text_document.uri) else {
                    return;
                };
                // back to what the host handed over
                let stored = self.fs.get(&path).map(|t| t.to_string());
                self.state.close(&path, stored);
                self.edited = true;
            }
            "stirphp/setFiles" => {
                let files = params.get("files").cloned().unwrap_or(Value::Null);
                self.set_files(&files);
            }
            _ => {}
        }
    }

    fn touched(&mut self, path: &Path) {
        self.recent.retain(|p| p != path);
        self.recent.push(path.to_owned());
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, (i64, String)> {
        if self.shut_down {
            return Err((code::INVALID_REQUEST, "the server has exited".into()));
        }
        match method {
            "initialize" => self.initialize(parse(params)?),
            "shutdown" => Ok(Value::Null),
            "textDocument/hover" => self.doc(params, handlers::hover, |p: &HoverParams| {
                &p.text_document_position_params.text_document.uri
            }),
            "textDocument/definition" => {
                self.doc(params, handlers::definition, |p: &GotoDefinitionParams| {
                    &p.text_document_position_params.text_document.uri
                })
            }
            "textDocument/typeDefinition" => self.doc(
                params,
                handlers::type_definition,
                |p: &GotoDefinitionParams| &p.text_document_position_params.text_document.uri,
            ),
            "textDocument/declaration" => self.doc(
                params,
                handlers::declaration,
                |p: &request::GotoDeclarationParams| {
                    &p.text_document_position_params.text_document.uri
                },
            ),
            "textDocument/implementation" => self.doc(
                params,
                handlers::implementation,
                |p: &request::GotoImplementationParams| {
                    &p.text_document_position_params.text_document.uri
                },
            ),
            "textDocument/prepareCallHierarchy" => self.doc(
                params,
                handlers::prepare_call_hierarchy,
                |p: &CallHierarchyPrepareParams| &p.text_document_position_params.text_document.uri,
            ),
            "callHierarchy/incomingCalls" => self.ws(params, handlers::incoming_calls),
            "callHierarchy/outgoingCalls" => self.ws(params, handlers::outgoing_calls),
            "textDocument/prepareTypeHierarchy" => self.doc(
                params,
                handlers::prepare_type_hierarchy,
                |p: &TypeHierarchyPrepareParams| &p.text_document_position_params.text_document.uri,
            ),
            "typeHierarchy/supertypes" => self.ws(params, handlers::supertypes),
            "typeHierarchy/subtypes" => self.ws(params, handlers::subtypes),
            "textDocument/references" => {
                self.doc(params, handlers::references, |p: &ReferenceParams| {
                    &p.text_document_position.text_document.uri
                })
            }
            "textDocument/documentHighlight" => self.doc(
                params,
                handlers::document_highlight,
                |p: &DocumentHighlightParams| &p.text_document_position_params.text_document.uri,
            ),
            "textDocument/prepareRename" => {
                let p: TextDocumentPositionParams = parse(params)?;
                let uri = p.text_document.uri.clone();
                self.doc_result(&uri, |s, f, m| handlers::prepare_rename(s, f, m, p))
            }
            "textDocument/rename" => {
                let p: RenameParams = parse(params)?;
                handlers::check_new_name(&p).map_err(failure)?;
                let uri = p.text_document_position.text_document.uri.clone();
                self.doc_result(&uri, |s, f, m| handlers::rename(s, f, m, p))
            }
            "textDocument/completion" => {
                self.doc(params, handlers::completion, |p: &CompletionParams| {
                    &p.text_document_position.text_document.uri
                })
            }
            "textDocument/signatureHelp" => self.doc(
                params,
                handlers::signature_help,
                |p: &SignatureHelpParams| &p.text_document_position_params.text_document.uri,
            ),
            "textDocument/formatting" => {
                let p: DocumentFormattingParams = parse(params)?;
                let indent = self.state.indent;
                let uri = p.text_document.uri.clone();
                self.doc_value(&uri, |s, f, m| handlers::formatting(s, f, m, p, indent))
            }
            "textDocument/rangeFormatting" => {
                let p: DocumentRangeFormattingParams = parse(params)?;
                let indent = self.state.indent;
                let uri = p.text_document.uri.clone();
                self.doc_value(&uri, |s, f, m| {
                    handlers::range_formatting(s, f, m, p, indent)
                })
            }
            "textDocument/documentSymbol" => self.doc(
                params,
                handlers::document_symbol_response,
                |p: &DocumentSymbolParams| &p.text_document.uri,
            ),
            "workspace/symbol" => self.ws(params, handlers::workspace_symbols),
            "textDocument/semanticTokens/full" => self.doc(
                params,
                handlers::semantic_tokens,
                |p: &SemanticTokensParams| &p.text_document.uri,
            ),
            "textDocument/inlayHint" => {
                self.doc(params, handlers::inlay_hints, |p: &InlayHintParams| {
                    &p.text_document.uri
                })
            }
            "textDocument/foldingRange" => self.doc(
                params,
                handlers::folding_ranges,
                |p: &FoldingRangeParams| &p.text_document.uri,
            ),
            "textDocument/selectionRange" => self.doc(
                params,
                handlers::selection_ranges,
                |p: &SelectionRangeParams| &p.text_document.uri,
            ),
            "textDocument/codeAction" => {
                let p: CodeActionParams = parse(params)?;
                let support = self.support;
                let versions = self.state.open.clone();
                let uri = p.text_document.uri.clone();
                self.doc_value(&uri, |s, f, m| {
                    actions::code_actions(s, f, m, p, support, &versions)
                })
            }
            "codeAction/resolve" => self.resolve_action(parse(params)?),
            "workspace/executeCommand" => self.execute_command(parse(params)?),
            "stirphp/fileText" => {
                let uri: Uri = params
                    .get("uri")
                    .and_then(Value::as_str)
                    .and_then(|u| u.parse().ok())
                    .ok_or((code::INVALID_PARAMS, "expected a `uri`".to_owned()))?;
                Ok(self.file_text(&uri).map_or(Value::Null, Value::String))
            }
            _ => Err((code::METHOD_NOT_FOUND, format!("no method `{method}`"))),
        }
    }

    fn snapshot(&self) -> Snapshot<'_> {
        Snapshot {
            db: &self.state.db,
            ws: self.state.ws,
            enc: self.enc,
            links: &Virtual,
        }
    }

    /// The document `uri` is, with its lines.
    fn document(&self, uri: &Uri) -> Option<(SourceFile, LineMap)> {
        let path = Virtual.path(uri)?;
        let file = match self.state.files.get(&path) {
            Some(&file) => file,
            // a builtin stub the client opened
            None => self.snapshot().file_at(&path)?,
        };
        let map = LineMap::new(file.text(&self.state.db), self.enc);
        Some((file, map))
    }

    fn doc_result<T: Serialize>(
        &self,
        uri: &Uri,
        f: impl FnOnce(&Snapshot, SourceFile, &LineMap) -> Result<Option<T>, Failure>,
    ) -> Result<Value, (i64, String)> {
        let Some((file, map)) = self.document(uri) else {
            return Ok(Value::Null);
        };
        let result = f(&self.snapshot(), file, &map).map_err(failure)?;
        Ok(json!(result))
    }

    fn doc_value<T: Serialize>(
        &self,
        uri: &Uri,
        f: impl FnOnce(&Snapshot, SourceFile, &LineMap) -> Option<T>,
    ) -> Result<Value, (i64, String)> {
        self.doc_result(uri, |s, file, map| Ok(f(s, file, map)))
    }

    fn doc<P: DeserializeOwned, T: Serialize>(
        &self,
        params: Value,
        f: fn(&Snapshot, SourceFile, &LineMap, P) -> Option<T>,
        uri: fn(&P) -> &Uri,
    ) -> Result<Value, (i64, String)> {
        let p: P = parse(params)?;
        let uri = uri(&p).clone();
        self.doc_value(&uri, |s, file, map| f(s, file, map, p))
    }

    fn ws<P: DeserializeOwned, T: Serialize>(
        &self,
        params: Value,
        f: fn(&Snapshot, P) -> Option<T>,
    ) -> Result<Value, (i64, String)> {
        let p: P = parse(params)?;
        Ok(json!(f(&self.snapshot(), p)))
    }

    fn initialize(&mut self, params: InitializeParams) -> Result<Value, (i64, String)> {
        let utf8 = handlers::wants_utf8(&params);
        self.enc = if utf8 {
            Encoding::Utf8
        } else {
            Encoding::Utf16
        };
        self.support = ActionSupport::read(&params);
        self.type_hierarchy_dynamic = handlers::type_hierarchy_dynamic(&params);
        self.folder = handlers::workspace_folder(&params).and_then(|u| Virtual.path(&u));
        let options = params.initialization_options.unwrap_or(Value::Null);
        self.trusted = options.get("workspaceTrusted").and_then(Value::as_bool) != Some(false);
        if let Some(files) = options.get("files") {
            let fs = Arc::make_mut(&mut self.fs);
            for (path, text) in files_of(files) {
                if let Some(text) = text {
                    fs.insert(&path, text);
                }
            }
        }
        Ok(json!(handlers::initialize_result(utf8)))
    }

    fn initialized(&mut self) {
        self.load();
        if self.type_hierarchy_dynamic {
            let registration = handlers::type_hierarchy_registration();
            self.ask(
                "client/registerCapability",
                json!(RegistrationParams {
                    registrations: vec![registration]
                }),
            );
        }
    }

    /// Loads the project from the files handed over, keeping what's open in the editor.
    fn load(&mut self) {
        let fs: Arc<dyn Fs> = self.fs.clone();
        let folder = self.folder.clone();
        let loaded = folder.as_ref().and_then(|dir| {
            let project = Project::find(&fs, dir).ok()?;
            let loaded = load_workspace_from(&fs, &project, LoadOptions::default()).ok()?;
            Some((project, loaded))
        });
        let mut notices = Vec::new();
        let state = match loaded {
            Some((project, l)) => {
                notices = l.notices.clone();
                let foreign = [l.vendor_dir]
                    .into_iter()
                    .chain(project.config.phpstan_tmp_dir.clone())
                    .collect();
                let mut state = State::new(l.db, l.workspace, Some(project.root.clone()), foreign);
                state.rules = l.rules;
                state.indent = project.config.indent;
                state
            }
            None => {
                let mut state = State::empty(Vec::new());
                state.root = folder;
                state
            }
        };
        self.state.reload(state);
        let trusted = self.trusted;
        self.state.set_rules_running(trusted);
        self.edited = true;
        let fresh: Vec<String> = notices
            .iter()
            .filter(|n| !self.notices.contains(n))
            .cloned()
            .collect();
        self.notices = notices;
        for notice in fresh {
            self.show(MessageType::WARNING, notice);
        }
    }

    /// Takes in what the host says of its files.
    fn set_files(&mut self, files: &Value) {
        let changes = files_of(files);
        let mut reload = false;
        let mut rules = false;
        let mut updates = Vec::new();
        {
            let fs = Arc::make_mut(&mut self.fs);
            for (path, text) in &changes {
                match text {
                    Some(text) => fs.insert(path, text.as_str()),
                    None => fs.remove(path),
                }
            }
        }
        for (path, text) in changes {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            if matches!(name, "composer.json" | "composer.lock" | "installed.json")
                || name.contains(".neon")
            {
                reload = true;
            } else if name.ends_with(".rhai") {
                rules |= self
                    .state
                    .rules
                    .iter()
                    .any(|r| r.path(&self.state.db) == &path);
            } else if stir_syntax::is_php_path(&path) {
                updates.push((path, text));
            }
        }
        if reload {
            self.load();
            return;
        }
        if rules {
            let scripts: Vec<ScriptFile> = self.state.rules.clone();
            for script in scripts {
                let path = script.path(&self.state.db).clone();
                let text = self
                    .fs
                    .get(&path)
                    .map(|t| t.to_string())
                    .ok_or_else(|| "it can't be read: it isn't there".to_owned());
                if script.text(&self.state.db) != &text {
                    let _ = script.set_text(&mut self.state.db).to(text);
                }
            }
        }
        self.state.stored_changed(updates);
        self.edited = true;
    }

    /// The text of a file the server has, by its URI.
    fn file_text(&self, uri: &Uri) -> Option<String> {
        let path = Virtual.path(uri)?;
        if let Some(file) = self.snapshot().file_at(&path) {
            return Some(file.text(&self.state.db).clone());
        }
        let overlay = stir_db::overlay_path(&path);
        if let Some(file) = self.snapshot().file_at(&overlay) {
            return Some(file.text(&self.state.db).clone());
        }
        self.fs.get(&path).map(|t| t.to_string())
    }

    fn resolve_action(&mut self, mut action: CodeAction) -> Result<Value, (i64, String)> {
        let Some(what) = ToResolve::of(&action) else {
            return Ok(json!(action));
        };
        let support = self.support;
        let Some((file, _)) = self.document(&what.uri) else {
            return Ok(json!(action));
        };
        let resolved = actions::resolve(&self.snapshot(), file, what, support, &self.state.open);
        match resolved {
            Resolved::Done(edit, command) => {
                action.edit = edit;
                action.command = command;
            }
            Resolved::Stale => {
                return Err((code::CONTENT_MODIFIED, "content modified".into()));
            }
            Resolved::Refused(reason) => {
                self.show(MessageType::WARNING, format!("StirPHP: {reason}"));
            }
        }
        Ok(json!(action))
    }

    fn execute_command(&mut self, params: ExecuteCommandParams) -> Result<Value, (i64, String)> {
        let Some((uri, request)) = actions::command_request(&params) else {
            return Ok(Value::Null);
        };
        let Some((file, _)) = self.document(&uri) else {
            return Ok(Value::Null);
        };
        let support = self.support;
        let result =
            actions::add_parameter(&self.snapshot(), file, &request, support, &self.state.open);
        match result {
            Some(Ok(Some(edit))) => self.ask(
                "workspace/applyEdit",
                json!(ApplyWorkspaceEditParams { label: None, edit }),
            ),
            Some(Err(reason)) => self.show(MessageType::WARNING, format!("StirPHP: {reason}")),
            _ => {}
        }
        Ok(Value::Null)
    }

    /// Checks the next file of the round of diagnostics, starting one if there's none; publishes
    /// what's new.
    fn diagnose(&mut self) {
        let round = self.round.get_or_insert_with(|| {
            let sources = self.state.sources();
            // what's being edited first
            let mut files: Vec<SourceFile> = self
                .recent
                .iter()
                .rev()
                .filter_map(|p| self.state.files.get(p).copied())
                .filter(|f| sources.contains(f))
                .collect();
            let rest: Vec<SourceFile> = sources
                .iter()
                .filter(|f| !files.contains(f))
                .copied()
                .collect();
            files.extend(rest);
            Round {
                files: files.into(),
                seen: FxHashSet::default(),
            }
        });
        let Some(file) = round.files.pop_front() else {
            self.finish_round();
            return;
        };
        let db = &self.state.db;
        let path = file.path(db).clone();
        round.seen.insert(path.clone());
        let diags = diagnostics::of_source(db, self.state.ws, file);
        let text = file.text(db).clone();
        self.publish(&path, &text, &diags);
    }

    /// The scripts' diagnostics, and clearing those of files that are gone.
    fn finish_round(&mut self) {
        let Some(mut round) = self.round.take() else {
            return;
        };
        self.due = None;
        let sources = self.state.sources();
        for (path, text, diags) in diagnostics::of_scripts(&self.state.db, self.state.ws, &sources)
        {
            round.seen.insert(path.clone());
            self.publish(&path, &text, &diags);
        }
        for path in self.published.gone(&round.seen) {
            if let Some(uri) = Virtual.path_uri(&path) {
                self.notify(
                    "textDocument/publishDiagnostics",
                    json!(PublishDiagnosticsParams::new(uri, Vec::new(), None)),
                );
            }
        }
    }

    fn publish(&mut self, path: &Path, text: &str, diags: &[Diagnostic]) {
        let Some(uri) = Virtual.path_uri(path) else {
            return;
        };
        if !self.published.is_news(path, diags) {
            return;
        }
        let lsp = diagnostics::to_lsp(diags, text, self.enc, &uri);
        self.notify(
            "textDocument/publishDiagnostics",
            json!(PublishDiagnosticsParams::new(uri, lsp, None)),
        );
    }

    /// The files open in the editor, with their versions.
    pub fn open_documents(&self) -> &FxHashMap<PathBuf, i32> {
        &self.state.open
    }
}

fn parse<P: DeserializeOwned>(params: Value) -> Result<P, (i64, String)> {
    serde_json::from_value(params).map_err(|e| (code::INVALID_PARAMS, e.to_string()))
}

fn failure(f: Failure) -> (i64, String) {
    match f {
        Failure::InvalidParams(message) => (code::INVALID_PARAMS, message),
        Failure::ContentModified => (code::CONTENT_MODIFIED, "content modified".into()),
    }
}

/// The files of a `files` map: path (or `file://` URI) to text, `null` for one that's gone.
fn files_of(files: &Value) -> Vec<(PathBuf, Option<String>)> {
    let Some(files) = files.as_object() else {
        return Vec::new();
    };
    files
        .iter()
        .filter_map(|(key, text)| {
            let path = if key.contains("://") {
                Virtual.path(&key.parse().ok()?)?
            } else {
                stir_vfs::normalize(Path::new(key))
            };
            path.is_absolute()
                .then(|| (path, text.as_str().map(str::to_owned)))
        })
        .collect()
}
