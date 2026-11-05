//! The LSP server: document state, the salsa database and request handlers.
//!
//! Reads run on a snapshot (a clone) of the database in `spawn_blocking`; an
//! edit cancels outstanding snapshots, whose requests then fail with
//! `ContentModified` and are retried by the client.

use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rustc_hash::{FxHashMap, FxHashSet};
use salsa::Setter as _;
use stir_db::{
    Db, Diagnostic, FileOrigin, RootDatabase, Settings, Severity, SourceFile, Workspace,
};
use stir_project::Project;
use stir_project::workspace::{LoadOptions, builtin_files, load_workspace};
use stir_syntax::{TextRange, TextSize};
use tokio::sync::Notify;
use tower_lsp_server::jsonrpc::{Error, ErrorCode, Result};
use tower_lsp_server::ls_types::*;
use tower_lsp_server::{Client, LanguageServer};

use crate::convert::{Encoding, LineMap};
use crate::{complete, hints, hover, nav, symbols, tokens};

/// Delay between an edit and the diagnostics run it triggers.
const DIAGNOSTICS_DELAY: Duration = Duration::from_millis(150);

struct State {
    db: RootDatabase,
    ws: Workspace,
    /// Files with a path on disk (project sources and vendor), by canonical path.
    files: FxHashMap<PathBuf, SourceFile>,
    /// Documents open in the editor, with their version.
    open: FxHashMap<PathBuf, i32>,
    /// The project root (directory of `sphp.toml`, else the workspace folder).
    root: Option<PathBuf>,
    /// Directories whose files are not project sources (generated output,
    /// `vendor/`): opened files there are dependencies, never checked.
    foreign: Vec<PathBuf>,
}

impl State {
    fn new(db: RootDatabase, ws: Workspace, root: Option<PathBuf>) -> Self {
        let files = ws
            .files(&db)
            .iter()
            .filter(|f| f.origin(&db) != FileOrigin::Stub)
            .map(|f| (f.path(&db).clone(), *f))
            .collect();
        State {
            db,
            ws,
            files,
            open: FxHashMap::default(),
            foreign: root.iter().map(|r| r.join("vendor")).collect(),
            root,
        }
    }

    /// A workspace with only the builtin stubs.
    fn empty() -> Self {
        let db = RootDatabase::default();
        let files = builtin_files(&db);
        let ws = Workspace::new(&db, files, Settings::default());
        State::new(db, ws, None)
    }

    /// Sets the text of `path`, adding it as a project source if it is new.
    fn set_text(&mut self, path: &Path, text: String) -> SourceFile {
        if let Some(&file) = self.files.get(path) {
            if file.text(&self.db) != &text {
                file.set_text(&mut self.db).to(text);
            }
            return file;
        }
        let origin = if self.foreign.iter().any(|d| path.starts_with(d)) {
            FileOrigin::Vendor
        } else {
            FileOrigin::Source
        };
        let file = SourceFile::new(&self.db, path.to_owned(), text, origin);
        let mut files = self.ws.files(&self.db).clone();
        files.push(file);
        self.ws.set_files(&mut self.db).to(files);
        self.files.insert(path.to_owned(), file);
        file
    }

    fn remove(&mut self, path: &Path) {
        if let Some(file) = self.files.remove(path) {
            let files = self
                .ws
                .files(&self.db)
                .iter()
                .copied()
                .filter(|f| *f != file)
                .collect();
            self.ws.set_files(&mut self.db).to(files);
        }
    }

    /// Is `path` a project source location (in the project, not generated or vendored)?
    fn is_project_path(&self, path: &Path) -> bool {
        self.root.as_ref().is_some_and(|r| path.starts_with(r))
            && !self.foreign.iter().any(|d| path.starts_with(d))
    }

    fn sources(&self) -> Vec<SourceFile> {
        self.ws
            .files(&self.db)
            .iter()
            .copied()
            .filter(|f| f.origin(&self.db) == FileOrigin::Source)
            .collect()
    }
}

fn is_source_path(path: &Path) -> bool {
    stir_syntax::is_php_path(path)
}

/// The canonical path of a `file://` URI.
fn uri_path(uri: &Uri) -> Option<PathBuf> {
    let path = uri.to_file_path()?.into_owned();
    Some(std::fs::canonicalize(&path).unwrap_or(path))
}

/// Builtin files are written to a cache directory so editors can open them.
fn stub_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("sphp").join(env!("CARGO_PKG_VERSION"))
}

/// A URI for `file`: its path, or (for builtins) a copy in the cache directory.
fn file_uri(db: &dyn Db, file: SourceFile) -> Option<Uri> {
    let path = file.path(db);
    let s = path.to_string_lossy();
    if let Some(rest) = s.strip_prefix("stubs://") {
        let target = stub_dir().join("stubs").join(rest);
        let text = file.text(db);
        let fresh = std::fs::read_to_string(&target).is_ok_and(|t| &t == text);
        if !fresh {
            std::fs::create_dir_all(target.parent()?).ok()?;
            std::fs::write(&target, text).ok()?;
        }
        return Uri::from_file_path(target);
    }
    Uri::from_file_path(path)
}

fn location(db: &dyn Db, file: SourceFile, range: TextRange, enc: Encoding) -> Option<Location> {
    let map = LineMap::new(file.text(db), enc);
    Some(Location::new(file_uri(db, file)?, map.range(range)))
}

fn lsp_diagnostic(
    d: &Diagnostic,
    map: &LineMap,
    uri: &Uri,
) -> tower_lsp_server::ls_types::Diagnostic {
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
    tower_lsp_server::ls_types::Diagnostic {
        range: map.range(d.range),
        severity: Some(match d.severity {
            Severity::Error => DiagnosticSeverity::ERROR,
            Severity::Warning => DiagnosticSeverity::WARNING,
            Severity::Hint => DiagnosticSeverity::HINT,
        }),
        code: Some(NumberOrString::String(d.code.to_owned())),
        source: Some("sphp".into()),
        message,
        related_information: related,
        ..Default::default()
    }
}

fn content_modified() -> Error {
    Error {
        code: ErrorCode::ServerError(-32801),
        message: "content modified".into(),
        data: None,
    }
}

pub struct Server {
    client: Client,
    state: Arc<Mutex<State>>,
    encoding: Arc<Mutex<Encoding>>,
    /// Set once the project is loaded.
    ready: Arc<tokio::sync::watch::Sender<bool>>,
    diagnostics: Arc<Notify>,
    workspace_folder: Mutex<Option<PathBuf>>,
    watch_dynamic: Mutex<bool>,
}

impl Server {
    pub fn new(client: Client) -> Self {
        let server = Server {
            client,
            state: Arc::new(Mutex::new(State::empty())),
            encoding: Arc::new(Mutex::new(Encoding::Utf16)),
            ready: Arc::new(tokio::sync::watch::channel(false).0),
            diagnostics: Arc::new(Notify::new()),
            workspace_folder: Mutex::new(None),
            watch_dynamic: Mutex::new(false),
        };
        tokio::spawn(diagnostics_loop(
            server.client.clone(),
            server.state.clone(),
            server.encoding.clone(),
            server.diagnostics.clone(),
        ));
        server
    }

    fn encoding(&self) -> Encoding {
        *self.encoding.lock().unwrap()
    }

    async fn wait_ready(&self) {
        let mut rx = self.ready.subscribe();
        let _ = rx.wait_for(|r| *r).await;
    }

    /// Runs `f` on a snapshot of the database for the document `uri`.
    async fn read<T, F>(&self, uri: &Uri, f: F) -> Result<Option<T>>
    where
        T: Send + 'static,
        F: FnOnce(&RootDatabase, Workspace, SourceFile, &LineMap) -> Option<T> + Send + 'static,
    {
        self.wait_ready().await;
        let Some(path) = uri_path(uri) else {
            return Ok(None);
        };
        let enc = self.encoding();
        let snapshot = {
            let s = self.state.lock().unwrap();
            s.files.get(&path).map(|&f| (s.db.clone(), s.ws, f))
        };
        let Some((db, ws, file)) = snapshot else {
            return Ok(None);
        };
        self.run(move || {
            let map = LineMap::new(file.text(&db), enc);
            f(&db, ws, file, &map)
        })
        .await
    }

    /// Runs `f` on a snapshot of the whole workspace.
    async fn read_ws<T, F>(&self, f: F) -> Result<Option<T>>
    where
        T: Send + 'static,
        F: FnOnce(&RootDatabase, Workspace, Encoding) -> Option<T> + Send + 'static,
    {
        self.wait_ready().await;
        let enc = self.encoding();
        let (db, ws) = {
            let s = self.state.lock().unwrap();
            (s.db.clone(), s.ws)
        };
        self.run(move || f(&db, ws, enc)).await
    }

    async fn run<T: Send + 'static>(
        &self,
        f: impl FnOnce() -> Option<T> + Send + 'static,
    ) -> Result<Option<T>> {
        let result =
            tokio::task::spawn_blocking(move || salsa::Cancelled::catch(AssertUnwindSafe(f)))
                .await
                .map_err(|e| {
                    tracing::error!(error = %e, "request panicked");
                    Error::internal_error()
                })?;
        result.map_err(|_| content_modified())
    }

    /// Applies an edit to the database.
    fn write(&self, f: impl FnOnce(&mut State)) {
        let mut s = self.state.lock().unwrap();
        f(&mut s);
        drop(s);
        self.diagnostics.notify_one();
    }

    async fn load(&self) {
        let folder = self.workspace_folder.lock().unwrap().clone();
        let loaded = match &folder {
            Some(dir) => load_project(dir).await,
            None => None,
        };
        {
            let mut s = self.state.lock().unwrap();
            // documents opened meanwhile keep their editor text
            let open: Vec<(PathBuf, i32, String)> = s
                .open
                .iter()
                .filter_map(|(p, v)| {
                    s.files
                        .get(p)
                        .map(|f| (p.clone(), *v, f.text(&s.db).clone()))
                })
                .collect();
            let mut state = match loaded {
                Some((db, ws, root)) => State::new(db, ws, Some(root)),
                None => {
                    let mut st = State::empty();
                    st.root = folder.clone();
                    st
                }
            };
            for (path, version, text) in open {
                state.set_text(&path, text);
                state.open.insert(path, version);
            }
            *s = state;
        }
        self.ready.send_replace(true);
        self.diagnostics.notify_one();
    }
}

async fn load_project(dir: &Path) -> Option<(RootDatabase, Workspace, PathBuf)> {
    let project = match Project::load_or_default(dir).await {
        Ok(p) => p,
        Err(error) => {
            tracing::error!(%error, "failed to load the project");
            return None;
        }
    };
    match load_workspace(&project, LoadOptions::default()).await {
        Ok(l) => Some((l.db, l.workspace, project.root.clone())),
        Err(error) => {
            tracing::error!(%error, "failed to load the project");
            None
        }
    }
}

/// Publishes diagnostics of every project source after edits settle.
async fn diagnostics_loop(
    client: Client,
    state: Arc<Mutex<State>>,
    encoding: Arc<Mutex<Encoding>>,
    notify: Arc<Notify>,
) {
    let mut published: FxHashMap<PathBuf, Vec<Diagnostic>> = FxHashMap::default();
    loop {
        notify.notified().await;
        tokio::time::sleep(DIAGNOSTICS_DELAY).await;
        let (db, ws, sources) = {
            let s = state.lock().unwrap();
            (s.db.clone(), s.ws, s.sources())
        };
        let enc = *encoding.lock().unwrap();
        // everything that needs the snapshot happens here, so it is dropped
        // before awaiting the client (an edit waits for snapshots to go away)
        let result = tokio::task::spawn_blocking(move || {
            salsa::Cancelled::catch(AssertUnwindSafe(|| {
                sources
                    .iter()
                    .map(|&f| {
                        let path = f.path(&db).clone();
                        let diags = stir_checker::check_file(&db, ws, f).as_ref().clone();
                        let lsp = Uri::from_file_path(&path).map(|uri| {
                            let map = LineMap::new(f.text(&db), enc);
                            let lsp = diags
                                .iter()
                                .map(|d| lsp_diagnostic(d, &map, &uri))
                                .collect();
                            (uri, lsp)
                        });
                        (path, diags, lsp)
                    })
                    .collect::<Vec<_>>()
            }))
        })
        .await;
        let Ok(Ok(results)) = result else {
            // cancelled by an edit: that edit triggers another run
            continue;
        };
        let mut seen = FxHashSet::default();
        for (path, diags, lsp) in results {
            seen.insert(path.clone());
            if published.get(&path) == Some(&diags) {
                continue;
            }
            let Some((uri, lsp)) = lsp else { continue };
            client.publish_diagnostics(uri, lsp, None).await;
            published.insert(path, diags);
        }
        // removed files
        let gone: Vec<PathBuf> = published
            .keys()
            .filter(|p| !seen.contains(*p))
            .cloned()
            .collect();
        for path in gone {
            published.remove(&path);
            if let Some(uri) = Uri::from_file_path(&path) {
                client.publish_diagnostics(uri, Vec::new(), None).await;
            }
        }
    }
}

fn symbol_kind(kind: symbols::SymbolKind) -> SymbolKind {
    use symbols::SymbolKind as K;
    match kind {
        K::Function => SymbolKind::FUNCTION,
        K::Class => SymbolKind::CLASS,
        K::Interface => SymbolKind::INTERFACE,
        K::Enum => SymbolKind::ENUM,
        K::Method => SymbolKind::METHOD,
        K::Property => SymbolKind::PROPERTY,
        K::Constant => SymbolKind::CONSTANT,
        K::EnumMember => SymbolKind::ENUM_MEMBER,
    }
}

#[allow(deprecated)]
fn document_symbol(s: symbols::Symbol, map: &LineMap) -> DocumentSymbol {
    DocumentSymbol {
        name: s.name,
        detail: s.detail,
        kind: symbol_kind(s.kind),
        tags: None,
        deprecated: None,
        range: map.range(s.full_range),
        selection_range: map.range(s.focus_range),
        children: Some(
            s.children
                .into_iter()
                .map(|c| document_symbol(c, map))
                .collect(),
        ),
    }
}

fn completion_kind(kind: complete::ItemKind) -> CompletionItemKind {
    use complete::ItemKind as K;
    match kind {
        K::Local => CompletionItemKind::VARIABLE,
        K::Param => CompletionItemKind::VARIABLE,
        K::Function => CompletionItemKind::FUNCTION,
        K::Method => CompletionItemKind::METHOD,
        K::Property => CompletionItemKind::PROPERTY,
        K::Const => CompletionItemKind::CONSTANT,
        K::EnumCase => CompletionItemKind::ENUM_MEMBER,
        K::Class => CompletionItemKind::CLASS,
        K::Interface => CompletionItemKind::INTERFACE,
        K::Enum => CompletionItemKind::ENUM,
        K::Keyword => CompletionItemKind::KEYWORD,
    }
}

/// The rename edit for one occurrence: keeps the occurrence's `$` (mutable
/// bindings and PHP properties are written with it, others without).
fn renamed(old: &str, new: &str) -> String {
    let bare = new.trim_start_matches('$');
    if old.starts_with('$') {
        format!("${bare}")
    } else {
        bare.to_owned()
    }
}

fn valid_identifier(name: &str) -> bool {
    let bare = name.strip_prefix('$').unwrap_or(name);
    let mut chars = bare.chars();
    chars
        .next()
        .is_some_and(|c| c.is_alphabetic() || c == '_' || !c.is_ascii())
        && chars.all(|c| c.is_alphanumeric() || c == '_' || !c.is_ascii())
}

impl LanguageServer for Server {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        let utf8 = params
            .capabilities
            .general
            .as_ref()
            .and_then(|g| g.position_encodings.as_ref())
            .is_some_and(|e| e.contains(&PositionEncodingKind::UTF8));
        *self.encoding.lock().unwrap() = if utf8 {
            Encoding::Utf8
        } else {
            Encoding::Utf16
        };
        #[allow(deprecated)]
        let folder = params
            .workspace_folders
            .as_ref()
            .and_then(|f| f.first())
            .map(|f| f.uri.clone())
            .or(params.root_uri.clone())
            .and_then(|u| uri_path(&u));
        *self.workspace_folder.lock().unwrap() = folder;
        *self.watch_dynamic.lock().unwrap() = params
            .capabilities
            .workspace
            .as_ref()
            .and_then(|w| w.did_change_watched_files.as_ref())
            .and_then(|d| d.dynamic_registration)
            .unwrap_or(false);

        let legend = SemanticTokensLegend {
            token_types: tokens::TOKEN_TYPES
                .iter()
                .map(|t| SemanticTokenType::new(t))
                .collect(),
            token_modifiers: tokens::TOKEN_MODIFIERS
                .iter()
                .map(|m| SemanticTokenModifier::new(m))
                .collect(),
        };
        Ok(InitializeResult {
            offset_encoding: None,
            server_info: Some(ServerInfo {
                name: "sphp".into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
            capabilities: ServerCapabilities {
                position_encoding: Some(if utf8 {
                    PositionEncodingKind::UTF8
                } else {
                    PositionEncodingKind::UTF16
                }),
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::INCREMENTAL),
                        save: Some(TextDocumentSyncSaveOptions::Supported(false)),
                        ..Default::default()
                    },
                )),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                definition_provider: Some(OneOf::Left(true)),
                type_definition_provider: Some(TypeDefinitionProviderCapability::Simple(true)),
                references_provider: Some(OneOf::Left(true)),
                document_highlight_provider: Some(OneOf::Left(true)),
                rename_provider: Some(OneOf::Right(RenameOptions {
                    prepare_provider: Some(true),
                    work_done_progress_options: Default::default(),
                })),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec![">".into(), ":".into(), "$".into()]),
                    ..Default::default()
                }),
                signature_help_provider: Some(SignatureHelpOptions {
                    trigger_characters: Some(vec!["(".into(), ",".into()]),
                    ..Default::default()
                }),
                document_symbol_provider: Some(OneOf::Left(true)),
                workspace_symbol_provider: Some(OneOf::Left(true)),
                semantic_tokens_provider: Some(
                    SemanticTokensServerCapabilities::SemanticTokensOptions(
                        SemanticTokensOptions {
                            legend,
                            full: Some(SemanticTokensFullOptions::Bool(true)),
                            range: Some(false),
                            ..Default::default()
                        },
                    ),
                ),
                inlay_hint_provider: Some(OneOf::Left(true)),
                folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
                selection_range_provider: Some(SelectionRangeProviderCapability::Simple(true)),
                ..Default::default()
            },
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.load().await;
        if *self.watch_dynamic.lock().unwrap() {
            let watchers = [
                "**/*.php",
                "**/sphp.toml",
                "**/phpstan.neon*",
                "**/composer.json",
            ]
            .into_iter()
            .map(|g| FileSystemWatcher {
                glob_pattern: GlobPattern::String(g.into()),
                kind: None,
            })
            .collect();
            let options = DidChangeWatchedFilesRegistrationOptions { watchers };
            let registration = Registration {
                id: "sphp-watch".into(),
                method: "workspace/didChangeWatchedFiles".into(),
                register_options: serde_json::to_value(options).ok(),
            };
            if let Err(error) = self.client.register_capability(vec![registration]).await {
                tracing::warn!(%error, "cannot watch files");
            }
        }
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let Some(path) = uri_path(&params.text_document.uri) else {
            return;
        };
        let doc = params.text_document;
        self.write(|s| {
            s.set_text(&path, doc.text);
            s.open.insert(path, doc.version);
        });
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let Some(path) = uri_path(&params.text_document.uri) else {
            return;
        };
        let enc = self.encoding();
        let version = params.text_document.version;
        self.write(|s| {
            if s.open.get(&path).is_some_and(|v| *v > version) {
                return;
            }
            let Some(&file) = s.files.get(&path) else {
                return;
            };
            let mut text = file.text(&s.db).clone();
            for change in params.content_changes {
                match change.range {
                    Some(range) => {
                        let map = LineMap::new(&text, enc);
                        let r = map.text_range(range);
                        text.replace_range(
                            usize::from(r.start())..usize::from(r.end()),
                            &change.text,
                        );
                    }
                    None => text = change.text,
                }
            }
            s.set_text(&path, text);
            s.open.insert(path, version);
        });
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let Some(path) = uri_path(&params.text_document.uri) else {
            return;
        };
        // back to what is on disk
        let disk = tokio::fs::read_to_string(&path).await.ok();
        self.write(|s| {
            s.open.remove(&path);
            let in_project = s.is_project_path(&path);
            match disk {
                Some(text) if in_project => {
                    s.set_text(&path, text);
                }
                _ => s.remove(&path),
            }
        });
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        let mut reload = false;
        let mut updates = Vec::new();
        for change in params.changes {
            let Some(path) =
                uri_path(&change.uri).or_else(|| change.uri.to_file_path().map(|p| p.into_owned()))
            else {
                continue;
            };
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            if name == "sphp.toml" || name.starts_with("phpstan.neon") || name == "composer.json" {
                reload = true;
                continue;
            }
            if !is_source_path(&path) {
                continue;
            }
            let text = if change.typ == FileChangeType::DELETED {
                None
            } else {
                tokio::fs::read_to_string(&path).await.ok()
            };
            updates.push((path, text));
        }
        if reload {
            self.load().await;
            return;
        }
        self.write(|s| {
            for (path, text) in updates {
                if s.open.contains_key(&path) {
                    continue;
                }
                let in_project = s.is_project_path(&path);
                match text {
                    Some(text) if s.files.contains_key(&path) || in_project => {
                        s.set_text(&path, text);
                    }
                    Some(_) => {}
                    None => s.remove(&path),
                }
            }
        });
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let pos = params.text_document_position_params.position;
        self.read(
            &params.text_document_position_params.text_document.uri,
            move |db, ws, file, map| {
                let h = hover::hover(db, ws, file, map.offset(pos))?;
                Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: h.markdown,
                    }),
                    range: Some(map.range(h.range)),
                })
            },
        )
        .await
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let pos = params.text_document_position_params.position;
        let enc = self.encoding();
        self.read(
            &params.text_document_position_params.text_document.uri,
            move |db, ws, file, map| {
                let (_, def) = nav::def_at(db, ws, file, map.offset(pos))?;
                let t = nav::target(db, ws, file, &def)?;
                let loc = location(db, t.file, t.focus_range, enc)?;
                Some(GotoDefinitionResponse::Scalar(loc))
            },
        )
        .await
    }

    async fn goto_type_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let pos = params.text_document_position_params.position;
        let enc = self.encoding();
        self.read(
            &params.text_document_position_params.text_document.uri,
            move |db, ws, file, map| {
                let offset = map.offset(pos);
                let typed = stir_checker::typed_file(db, ws, file);
                let ty = match nav::def_at(db, ws, file, offset) {
                    Some((range, stir_checker::Def::Local { decl })) => typed
                        .types
                        .get(&range)
                        .cloned()
                        .or_else(|| typed.locals.get(&decl).map(|l| l.0.clone()))?,
                    Some((range, _)) => typed.types.get(&range).cloned()?,
                    None => typed
                        .types
                        .iter()
                        .filter(|(r, _)| r.contains(offset))
                        .min_by_key(|(r, _)| r.len())?
                        .1
                        .clone(),
                };
                let ty = stir_types::without_null(&ty);
                let key = match ty.kind() {
                    stir_types::TyKind::Class { name, .. } => name.to_ascii_lowercase(),
                    _ => return None,
                };
                let def = stir_checker::Def::Class(key.trim_start_matches('\\').into());
                let t = nav::target(db, ws, file, &def)?;
                Some(GotoDefinitionResponse::Scalar(location(
                    db,
                    t.file,
                    t.focus_range,
                    enc,
                )?))
            },
        )
        .await
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let pos = params.text_document_position.position;
        let include_decl = params.context.include_declaration;
        let enc = self.encoding();
        self.read(
            &params.text_document_position.text_document.uri,
            move |db, ws, file, map| {
                let (_, def) = nav::def_at(db, ws, file, map.offset(pos))?;
                let decl = nav::target(db, ws, file, &def);
                let refs = nav::references(db, ws, file, &def);
                Some(
                    refs.into_iter()
                        .filter(|(f, r)| {
                            include_decl
                                || decl
                                    .as_ref()
                                    .is_none_or(|d| !(d.file == *f && d.focus_range == *r))
                        })
                        .filter_map(|(f, r)| location(db, f, r, enc))
                        .collect(),
                )
            },
        )
        .await
    }

    async fn document_highlight(
        &self,
        params: DocumentHighlightParams,
    ) -> Result<Option<Vec<DocumentHighlight>>> {
        let pos = params.text_document_position_params.position;
        self.read(
            &params.text_document_position_params.text_document.uri,
            move |db, ws, file, map| {
                let (_, def) = nav::def_at(db, ws, file, map.offset(pos))?;
                let decl = nav::target(db, ws, file, &def);
                Some(
                    nav::file_refs(db, ws, file)
                        .iter()
                        .filter(|(_, d)| *d == def)
                        .map(|(r, _)| DocumentHighlight {
                            range: map.range(*r),
                            kind: Some(
                                if decl
                                    .as_ref()
                                    .is_some_and(|d| d.file == file && d.focus_range == *r)
                                {
                                    DocumentHighlightKind::WRITE
                                } else {
                                    DocumentHighlightKind::READ
                                },
                            ),
                        })
                        .collect(),
                )
            },
        )
        .await
    }

    async fn prepare_rename(
        &self,
        params: TextDocumentPositionParams,
    ) -> Result<Option<PrepareRenameResponse>> {
        let pos = params.position;
        let result = self
            .read(&params.text_document.uri, move |db, ws, file, map| {
                let (range, def) = nav::def_at(db, ws, file, map.offset(pos))?;
                if !hover::is_project_def(db, ws, file, &def) {
                    return Some(Err("can't rename a declaration outside the project"));
                }
                // the name token itself (not `\Qualified\Name`)
                let text = &file.text(db)[usize::from(range.start())..usize::from(range.end())];
                let short = text.rsplit('\\').next().unwrap_or(text);
                let start = range.end() - TextSize::of(short);
                Some(Ok(PrepareRenameResponse::Range(
                    map.range(TextRange::new(start, range.end())),
                )))
            })
            .await?;
        match result {
            Some(Ok(r)) => Ok(Some(r)),
            Some(Err(msg)) => Err(Error::invalid_params(msg)),
            None => Ok(None),
        }
    }

    async fn rename(&self, params: RenameParams) -> Result<Option<WorkspaceEdit>> {
        let pos = params.text_document_position.position;
        let new_name = params.new_name;
        if !valid_identifier(&new_name) {
            return Err(Error::invalid_params(format!(
                "`{new_name}` is not a valid name"
            )));
        }
        let enc = self.encoding();
        let result = self
            .read(
                &params.text_document_position.text_document.uri,
                move |db, ws, file, map| {
                    let (_, def) = nav::def_at(db, ws, file, map.offset(pos))?;
                    if !hover::is_project_def(db, ws, file, &def) {
                        return Some(Err("can't rename a declaration outside the project"));
                    }
                    let _ = map;
                    let mut changes: std::collections::HashMap<Uri, Vec<TextEdit>> =
                        Default::default();
                    for (f, range) in nav::references(db, ws, file, &def) {
                        let text = f.text(db);
                        let old = &text[usize::from(range.start())..usize::from(range.end())];
                        // only the last segment of a qualified name changes
                        let short = old.rsplit('\\').next().unwrap_or(old);
                        let range = TextRange::new(range.end() - TextSize::of(short), range.end());
                        let Some(uri) = file_uri(db, f) else { continue };
                        let fmap = LineMap::new(text, enc);
                        changes.entry(uri).or_default().push(TextEdit {
                            range: fmap.range(range),
                            new_text: renamed(short, &new_name),
                        });
                    }
                    Some(Ok(WorkspaceEdit {
                        changes: Some(changes),
                        ..Default::default()
                    }))
                },
            )
            .await?;
        match result {
            Some(Ok(e)) => Ok(Some(e)),
            Some(Err(msg)) => Err(Error::invalid_params(msg)),
            None => Ok(None),
        }
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let pos = params.text_document_position.position;
        self.read(
            &params.text_document_position.text_document.uri,
            move |db, ws, file, map| {
                let c = complete::complete(db, ws, file, map.offset(pos));
                let range = map.range(c.replace);
                let items = c
                    .items
                    .into_iter()
                    .map(|i| CompletionItem {
                        filter_text: Some(i.label.clone()),
                        sort_text: Some(format!("{}{}", i.rank, i.label)),
                        kind: Some(completion_kind(i.kind)),
                        detail: i.detail,
                        text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                            range,
                            new_text: i.insert,
                        })),
                        label: i.label,
                        ..Default::default()
                    })
                    .collect();
                Some(CompletionResponse::List(CompletionList {
                    is_incomplete: c.incomplete,
                    items,
                }))
            },
        )
        .await
    }

    async fn signature_help(&self, params: SignatureHelpParams) -> Result<Option<SignatureHelp>> {
        let pos = params.text_document_position_params.position;
        self.read(
            &params.text_document_position_params.text_document.uri,
            move |db, ws, file, map| {
                let h = hints::signature_help(db, ws, file, map.offset(pos))?;
                let parameters = h
                    .params
                    .iter()
                    .map(|&(s, e)| {
                        let (s, e) = (s as usize, e as usize);
                        ParameterInformation {
                            label: ParameterLabel::LabelOffsets([
                                map.measure(&h.label[..s]),
                                map.measure(&h.label[..e]),
                            ]),
                            documentation: None,
                        }
                    })
                    .collect();
                Some(SignatureHelp {
                    signatures: vec![SignatureInformation {
                        label: h.label,
                        documentation: h.doc.map(|d| {
                            Documentation::MarkupContent(MarkupContent {
                                kind: MarkupKind::Markdown,
                                value: d,
                            })
                        }),
                        parameters: Some(parameters),
                        active_parameter: Some(h.active_param),
                    }],
                    active_signature: Some(0),
                    active_parameter: Some(h.active_param),
                })
            },
        )
        .await
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        self.read(&params.text_document.uri, move |db, _, file, map| {
            let syms = symbols::document_symbols(db, file);
            Some(DocumentSymbolResponse::Nested(
                syms.into_iter().map(|s| document_symbol(s, map)).collect(),
            ))
        })
        .await
    }

    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<WorkspaceSymbolResponse>> {
        let query = params.query;
        self.read_ws(move |db, ws, enc| {
            #[allow(deprecated)]
            let out = symbols::workspace_symbols(db, ws, &query)
                .into_iter()
                .filter_map(|s| {
                    Some(SymbolInformation {
                        name: s.name,
                        kind: symbol_kind(s.kind),
                        tags: None,
                        deprecated: None,
                        location: location(db, s.file, s.range, enc)?,
                        container_name: (!s.container.is_empty()).then_some(s.container),
                    })
                })
                .collect();
            Some(WorkspaceSymbolResponse::Flat(out))
        })
        .await
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>> {
        self.read(&params.text_document.uri, move |db, ws, file, map| {
            let text = file.text(db);
            let mut data = Vec::new();
            let (mut prev_line, mut prev_col) = (0u32, 0u32);
            for t in tokens::semantic_tokens(db, ws, file) {
                // tokens can't span lines
                for line in map.lines(t.range) {
                    let piece = line.intersect(t.range).unwrap_or(line);
                    let s = &text[usize::from(piece.start())..usize::from(piece.end())];
                    let s = s.trim_end_matches(['\n', '\r']);
                    if s.is_empty() {
                        continue;
                    }
                    let pos = map.position(piece.start());
                    let delta_line = pos.line - prev_line;
                    let delta_start = if delta_line == 0 {
                        pos.character - prev_col
                    } else {
                        pos.character
                    };
                    data.push(SemanticToken {
                        delta_line,
                        delta_start,
                        length: map.measure(s),
                        token_type: t.ty as u32,
                        token_modifiers_bitset: t.modifiers,
                    });
                    prev_line = pos.line;
                    prev_col = pos.character;
                }
            }
            Some(SemanticTokensResult::Tokens(SemanticTokens {
                result_id: None,
                data,
            }))
        })
        .await
    }

    async fn inlay_hint(&self, params: InlayHintParams) -> Result<Option<Vec<InlayHint>>> {
        let range = params.range;
        self.read(&params.text_document.uri, move |db, ws, file, map| {
            let r = map.text_range(range);
            Some(
                hints::inlay_hints(db, ws, file, r)
                    .into_iter()
                    .map(|h| InlayHint {
                        position: map.position(h.offset),
                        label: InlayHintLabel::String(h.label),
                        kind: Some(InlayHintKind::TYPE),
                        text_edits: None,
                        tooltip: None,
                        padding_left: None,
                        padding_right: None,
                        data: None,
                    })
                    .collect(),
            )
        })
        .await
    }

    async fn folding_range(&self, params: FoldingRangeParams) -> Result<Option<Vec<FoldingRange>>> {
        self.read(&params.text_document.uri, move |db, _, file, map| {
            Some(
                symbols::folding_ranges(db, file)
                    .into_iter()
                    .filter_map(|(r, kind)| {
                        let start = map.position(r.start());
                        let end = map.position(r.end());
                        (end.line > start.line).then(|| FoldingRange {
                            start_line: start.line,
                            end_line: end.line,
                            kind: Some(match kind {
                                symbols::FoldKind::Region => FoldingRangeKind::Region,
                                symbols::FoldKind::Comment => FoldingRangeKind::Comment,
                                symbols::FoldKind::Imports => FoldingRangeKind::Imports,
                            }),
                            ..Default::default()
                        })
                    })
                    .collect(),
            )
        })
        .await
    }

    async fn selection_range(
        &self,
        params: SelectionRangeParams,
    ) -> Result<Option<Vec<SelectionRange>>> {
        let positions = params.positions;
        self.read(&params.text_document.uri, move |db, _, file, map| {
            Some(
                positions
                    .iter()
                    .map(|p| {
                        let ranges = symbols::selection_ranges(db, file, map.offset(*p));
                        let mut sel: Option<SelectionRange> = None;
                        for r in ranges.into_iter().rev() {
                            sel = Some(SelectionRange {
                                range: map.range(r),
                                parent: sel.map(Box::new),
                            });
                        }
                        sel.unwrap_or(SelectionRange {
                            range: Range::new(*p, *p),
                            parent: None,
                        })
                    })
                    .collect(),
            )
        })
        .await
    }
}
