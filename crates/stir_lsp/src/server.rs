//! The LSP server: document state, the salsa database and request handlers.
//!
//! Reads run on a snapshot (a clone) of the database in `spawn_blocking`; an
//! edit cancels outstanding snapshots, whose requests then fail with
//! `ContentModified` and are retried by the client.

use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rustc_hash::FxHashSet;
use salsa::Setter as _;
use stir_db::{Db, Diagnostic, RootDatabase, ScriptFile, SourceFile, Workspace};
use stir_project::Project;
use stir_project::workspace::{LoadOptions, load_workspace};
use tokio::sync::Notify;
use tower_lsp_server::jsonrpc::{Error, ErrorCode, Result};
use tower_lsp_server::ls_types::*;
use tower_lsp_server::{Client, LanguageServer};

use crate::convert::{Encoding, LineMap};
use crate::handlers::diagnostics::{self, Published};
use crate::handlers::{self, Failure, Snapshot};
use crate::links::Links;
use crate::state::State;
use crate::trust::{self, TrustStore};

mod actions;
mod phpstan;

/// Delay between an edit and the diagnostics run it triggers.
const DIAGNOSTICS_DELAY: Duration = Duration::from_millis(150);
/// How long a question about a workspace's scripts goes unanswered before the person is told, for
/// a client that shows no such questions.
const TRUST_ANSWER_WAIT: Duration = Duration::from_secs(30);

impl State {
    /// What identifies the project's scripts as they are now.
    fn rules_hash(&self) -> String {
        let root = self.root.clone().unwrap_or_default();
        let scripts: Vec<_> = self
            .rules
            .iter()
            .map(|s| {
                let text = s.text(&self.db).as_ref();
                (
                    s.path(&self.db).as_path(),
                    text.map(String::as_str).map_err(String::as_str),
                )
            })
            .collect();
        trust::scripts_hash(&root, &scripts)
    }
}

/// A workspace with only the builtin stubs.
fn empty_state() -> State {
    // a stub opened from the cache directory stays a dependency
    State::empty(vec![stub_dir()])
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
    base.join("stirphp").join(env!("CARGO_PKG_VERSION"))
}

/// Files on disk: a URI is a file's path, and a builtin's is a copy in the cache directory.
#[derive(Debug, Clone, Copy)]
struct OnDisk;

impl Links for OnDisk {
    fn uri(&self, db: &dyn Db, file: SourceFile) -> Option<Uri> {
        let path = file.path(db);
        let s = path.to_string_lossy();
        // a project's stub file is a file of the project
        if let Some(real) = s.strip_prefix(stir_db::OVERLAY_PREFIX) {
            return Uri::from_file_path(real);
        }
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

    fn path(&self, uri: &Uri) -> Option<PathBuf> {
        uri_path(uri)
    }
}

fn content_modified() -> Error {
    Error {
        code: ErrorCode::ServerError(-32801),
        message: "content modified".into(),
        data: None,
    }
}

fn failure(f: Failure) -> Error {
    match f {
        Failure::InvalidParams(msg) => Error::invalid_params(msg),
        Failure::ContentModified => content_modified(),
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
    /// The client can have a type hierarchy registered, which is the only way to offer one: the
    /// server capabilities of the protocol's types have no field for it.
    type_hierarchy_dynamic: Mutex<bool>,
    /// What the user was last told about the project, so that a reload doesn't repeat it.
    notices: Mutex<Vec<String>>,
    rules: RulesTrust,
    phpstan: phpstan::Phpstan,
    action_support: Mutex<handlers::actions::ActionSupport>,
}

/// What it takes to decide whether a workspace's scripts run. See [`trust`].
#[derive(Clone)]
struct RulesTrust {
    store: TrustStore,
    /// The client said this workspace isn't trusted (`workspaceTrusted`): scripts never run.
    never: Arc<Mutex<bool>>,
    /// Can the client show a question at all?
    can_ask: Arc<Mutex<bool>>,
    /// The scripts a question is out about.
    asking: Arc<Mutex<Option<String>>>,
    /// Told the person they're off, for lack of an answer: once is enough.
    told: Arc<std::sync::atomic::AtomicBool>,
    answer_wait: Duration,
}

/// How a server decides about scripts: in tests, not the user's own answers.
#[derive(Debug, Clone)]
pub struct TrustOptions {
    pub store: TrustStore,
    pub answer_wait: Duration,
}

impl Default for TrustOptions {
    fn default() -> Self {
        TrustOptions {
            store: TrustStore::in_config_dir(),
            answer_wait: TRUST_ANSWER_WAIT,
        }
    }
}

impl Server {
    pub fn new(client: Client) -> Self {
        Server::with_trust(client, TrustOptions::default())
    }

    pub fn with_trust(client: Client, trust: TrustOptions) -> Self {
        let state = Arc::new(Mutex::new(empty_state()));
        let diagnostics = Arc::new(Notify::new());
        let rules = RulesTrust {
            store: trust.store,
            never: Arc::default(),
            can_ask: Arc::default(),
            asking: Arc::default(),
            told: Arc::default(),
            answer_wait: trust.answer_wait,
        };
        let phpstan = phpstan::Phpstan::new(
            client.clone(),
            state.clone(),
            diagnostics.clone(),
            rules.clone(),
        );
        let server = Server {
            client,
            state,
            encoding: Arc::new(Mutex::new(Encoding::Utf16)),
            ready: Arc::new(tokio::sync::watch::channel(false).0),
            diagnostics,
            workspace_folder: Mutex::new(None),
            watch_dynamic: Mutex::new(false),
            type_hierarchy_dynamic: Mutex::new(false),
            notices: Mutex::new(Vec::new()),
            rules,
            phpstan,
            action_support: Mutex::new(handlers::actions::ActionSupport::default()),
        };
        tokio::spawn(diagnostics_loop(
            server.client.clone(),
            server.state.clone(),
            server.encoding.clone(),
            server.diagnostics.clone(),
            server.phpstan.results.clone(),
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
        F: FnOnce(&Snapshot, SourceFile, &LineMap) -> Option<T> + Send + 'static,
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
            let snapshot = Snapshot {
                db: &db,
                ws,
                enc,
                links: &OnDisk,
            };
            f(&snapshot, file, &map)
        })
        .await
    }

    /// Runs `f` on a snapshot of the whole workspace.
    async fn read_ws<T, F>(&self, f: F) -> Result<Option<T>>
    where
        T: Send + 'static,
        F: FnOnce(&Snapshot) -> Option<T> + Send + 'static,
    {
        self.wait_ready().await;
        let enc = self.encoding();
        let (db, ws) = {
            let s = self.state.lock().unwrap();
            (s.db.clone(), s.ws)
        };
        self.run(move || {
            f(&Snapshot {
                db: &db,
                ws,
                enc,
                links: &OnDisk,
            })
        })
        .await
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
        let notices = loaded.as_ref().map_or_else(Vec::new, |l| l.notices.clone());
        let rules = loaded.as_ref().map(|l| l.rules.clone()).unwrap_or_default();
        let indent = loaded.as_ref().and_then(|l| l.indent);
        let phpstan = loaded
            .as_ref()
            .map(|l| (l.root.clone(), l.vendor_dir.clone(), l.phpstan.clone()));
        {
            let mut state = match loaded {
                Some(l) => {
                    // a stub opened from the cache directory stays a dependency, and PHPStan's
                    // caches are `.php` files, written while we watch for those
                    let foreign = [l.vendor_dir, stub_dir()]
                        .into_iter()
                        .chain(l.phpstan_tmp_dir)
                        .collect();
                    State::new(l.db, l.ws, Some(l.root), foreign)
                }
                None => {
                    let mut st = empty_state();
                    st.root = folder.clone();
                    st
                }
            };
            state.rules = rules;
            state.indent = indent;
            // documents opened meanwhile keep their editor text
            self.state.lock().unwrap().reload(state);
        }
        self.decide_rules();
        self.phpstan.decide(
            phpstan
                .as_ref()
                .map(|(root, vendor, run)| (root.as_path(), vendor.as_path(), run)),
        );
        self.ready.send_replace(true);
        self.diagnostics.notify_one();
        self.show_notices(notices).await;
    }

    /// Runs the project's scripts if that's been allowed, and asks if it hasn't been asked.
    fn decide_rules(&self) {
        let (root, hash, count) = {
            let s = self.state.lock().unwrap();
            let root = s.root.clone().unwrap_or_default();
            (root, s.rules_hash(), s.rules.len())
        };
        let trust = &self.rules;
        let allowed = count > 0
            && !*trust.never.lock().unwrap()
            && trust.store.decision(&root, &hash) == Some(true);
        self.state.lock().unwrap().set_rules_running(allowed);
        if count == 0 || allowed || *trust.never.lock().unwrap() {
            return;
        }
        if trust.store.decision(&root, &hash).is_some() {
            return;
        }
        {
            let mut asking = trust.asking.lock().unwrap();
            if asking.as_ref() == Some(&hash) {
                return;
            }
            *asking = Some(hash.clone());
        }
        tokio::spawn(ask_about_rules(
            self.client.clone(),
            trust.clone(),
            self.state.clone(),
            self.diagnostics.clone(),
            root,
            hash,
            count,
        ));
    }

    /// Reads the project's scripts again, after one of them changed.
    async fn reload_rules(&self) {
        let scripts: Vec<(ScriptFile, PathBuf)> = {
            let s = self.state.lock().unwrap();
            s.rules
                .iter()
                .map(|r| (*r, r.path(&s.db).clone()))
                .collect()
        };
        let mut texts = Vec::new();
        for (script, path) in scripts {
            let text = tokio::fs::read_to_string(&path)
                .await
                .map_err(|e| format!("it can't be read: {e}"));
            texts.push((script, text));
        }
        self.write(|s| {
            // what's changed hasn't been allowed: it mustn't run before that's decided
            if texts
                .iter()
                .any(|(script, text)| script.text(&s.db) != text)
            {
                s.set_rules_running(false);
            }
            for (script, text) in texts {
                if script.text(&s.db) != &text {
                    let _ = script.set_text(&mut s.db).to(text);
                }
            }
        });
        self.decide_rules();
        self.diagnostics.notify_one();
    }

    /// Tells the user what's new among `notices`.
    async fn show_notices(&self, notices: Vec<String>) {
        let fresh: Vec<String> = {
            let mut shown = self.notices.lock().unwrap();
            let fresh = notices
                .iter()
                .filter(|n| !shown.contains(n))
                .cloned()
                .collect();
            *shown = notices;
            fresh
        };
        for notice in fresh {
            self.client.show_message(MessageType::WARNING, notice).await;
        }
    }
}

/// Asks whether a workspace's scripts may run, and acts on the answer. Until there is one they
/// don't run. A client that doesn't answer gets told that, once.
async fn ask_about_rules(
    client: Client,
    trust: RulesTrust,
    state: Arc<Mutex<State>>,
    diagnostics: Arc<Notify>,
    root: PathBuf,
    hash: String,
    count: usize,
) {
    use std::sync::atomic::Ordering;

    const ALLOW: &str = "Allow";
    const DENY: &str = "Don't allow";
    let tell = |client: Client, told: Arc<std::sync::atomic::AtomicBool>| async move {
        if !told.swap(true, Ordering::Relaxed) {
            let notice = "Custom rules in this workspace (`parameters.stirphp.rules`) aren't running, because nobody was asked whether to allow them. `stirphp check` runs them.";
            client.show_message(MessageType::INFO, notice).await;
        }
    };
    if !*trust.can_ask.lock().unwrap() {
        tell(client, trust.told.clone()).await;
        return;
    }
    let actions = vec![
        MessageActionItem {
            title: ALLOW.into(),
            properties: Default::default(),
        },
        MessageActionItem {
            title: DENY.into(),
            properties: Default::default(),
        },
    ];
    let rules = if count == 1 {
        "the 1 custom rule".to_owned()
    } else {
        format!("the {count} custom rules")
    };
    let message = format!(
        "Run {rules} this workspace configures? Scripts can't touch files or the network, but allow them only if you trust this project."
    );
    let question = client.show_message_request(MessageType::WARNING, message, Some(actions));
    let mut question = std::pin::pin!(question);
    let answer = tokio::select! {
        answer = &mut question => answer,
        _ = tokio::time::sleep(trust.answer_wait) => {
            tell(client.clone(), trust.told.clone()).await;
            question.await
        }
    };
    {
        let mut asking = trust.asking.lock().unwrap();
        if asking.as_ref() == Some(&hash) {
            *asking = None;
        }
    }
    let allow = match answer {
        Ok(Some(action)) => action.title == ALLOW,
        // put away without an answer: the question comes back next time
        Ok(None) => return,
        Err(error) => {
            tracing::warn!(%error, "no answer about the workspace's rules");
            tell(client.clone(), trust.told.clone()).await;
            return;
        }
    };
    if let Err(error) = trust.store.remember(&root, &hash, allow) {
        tracing::warn!(%error, "can't remember the answer about the workspace's rules");
    }
    // only what was asked about: the scripts may have changed, and that's asked separately
    let mut s = state.lock().unwrap();
    if s.rules_hash() == hash && s.root.as_deref().unwrap_or(Path::new("")) == root {
        s.set_rules_running(allow);
        drop(s);
        diagnostics.notify_one();
    }
}

/// A project read from disk, for [`State`].
struct LoadedProject {
    db: RootDatabase,
    ws: Workspace,
    root: PathBuf,
    vendor_dir: PathBuf,
    notices: Vec<String>,
    rules: Vec<ScriptFile>,
    indent: Option<stir_neon::phpstan::FormatIndent>,
    phpstan: stir_neon::phpstan::PhpstanRun,
    phpstan_tmp_dir: Option<PathBuf>,
}

async fn load_project(dir: &Path) -> Option<LoadedProject> {
    let project = match Project::load_or_default(dir).await {
        Ok(p) => p,
        Err(error) => {
            tracing::error!(%error, "failed to load the project");
            return None;
        }
    };
    match load_workspace(&project, LoadOptions::default()).await {
        Ok(l) => Some(LoadedProject {
            db: l.db,
            ws: l.workspace,
            root: project.root.clone(),
            vendor_dir: l.vendor_dir,
            notices: l.notices,
            rules: l.rules,
            indent: project.config.indent,
            phpstan: project.config.phpstan.clone(),
            phpstan_tmp_dir: project.config.phpstan_tmp_dir.clone(),
        }),
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
    phpstan: Arc<Mutex<Arc<phpstan::Results>>>,
) {
    let mut published = Published::default();
    loop {
        notify.notified().await;
        tokio::time::sleep(DIAGNOSTICS_DELAY).await;
        let phpstan = phpstan.lock().unwrap().clone();
        let (db, ws, sources, elsewhere) = {
            let s = state.lock().unwrap();
            let sources = s.sources();
            // what PHPStan reports isn't only about what we check
            let elsewhere: Vec<_> = phpstan
                .keys()
                .filter(|p| !s.files.get(*p).is_some_and(|f| sources.contains(f)))
                .map(|p| (p.clone(), s.files.get(p).copied()))
                .collect();
            (s.db.clone(), s.ws, sources, elsewhere)
        };
        let enc = *encoding.lock().unwrap();
        // everything that needs the snapshot happens here, so it is dropped
        // before awaiting the client (an edit waits for snapshots to go away)
        let result = tokio::task::spawn_blocking(move || {
            salsa::Cancelled::catch(AssertUnwindSafe(|| {
                let to_lsp = |path: &Path, text: &str, diags: &[Diagnostic]| {
                    OnDisk
                        .path_uri(path)
                        .map(|uri| (diagnostics::to_lsp(diags, text, enc, &uri), uri))
                        .map(|(lsp, uri)| (uri, lsp))
                };
                // what PHPStan said of a file, if that's still the text it read
                let from_phpstan = |path: &Path, text: &str, ours: &[Diagnostic]| {
                    let Some(result) = phpstan.get(path) else {
                        return Vec::new();
                    };
                    if result.text != phpstan::text_hash(text) {
                        return Vec::new();
                    }
                    let theirs = stir_phpstan::diagnostics(path, text, &result.reported);
                    stir_phpstan::merge(ours, theirs, text)
                };
                let mut results = sources
                    .iter()
                    .map(|&f| {
                        let path = f.path(&db).clone();
                        let mut diags = diagnostics::of_source(&db, ws, f);
                        let theirs = from_phpstan(&path, f.text(&db), &diags);
                        diags.extend(theirs);
                        let lsp = to_lsp(&path, f.text(&db), &diags);
                        (path, diags, lsp)
                    })
                    .collect::<Vec<_>>();
                // what's wrong with a script is at the script
                for (path, text, diags) in diagnostics::of_scripts(&db, ws, &sources) {
                    let lsp = to_lsp(&path, &text, &diags);
                    results.push((path, diags, lsp));
                }
                for (path, file) in &elsewhere {
                    let text = match file {
                        Some(file) => Some(file.text(&db).clone()),
                        None => std::fs::read_to_string(path).ok(),
                    };
                    let Some(text) = text else { continue };
                    let diags = from_phpstan(path, &text, &[]);
                    let lsp = to_lsp(path, &text, &diags);
                    results.push((path.clone(), diags, lsp));
                }
                results
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
            let Some((uri, lsp)) = lsp else { continue };
            if !published.is_news(&path, &diags) {
                continue;
            }
            client.publish_diagnostics(uri, lsp, None).await;
        }
        // removed files
        for path in published.gone(&seen) {
            if let Some(uri) = Uri::from_file_path(&path) {
                client.publish_diagnostics(uri, Vec::new(), None).await;
            }
        }
    }
}

impl LanguageServer for Server {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        let utf8 = handlers::wants_utf8(&params);
        *self.action_support.lock().unwrap() = handlers::actions::ActionSupport::read(&params);
        *self.encoding.lock().unwrap() = if utf8 {
            Encoding::Utf8
        } else {
            Encoding::Utf16
        };
        let folder = handlers::workspace_folder(&params).and_then(|u| uri_path(&u));
        *self.workspace_folder.lock().unwrap() = folder;
        // an editor that knows this workspace isn't trusted says so: its scripts don't get asked about
        let never = params
            .initialization_options
            .as_ref()
            .and_then(|o| o.get("workspaceTrusted"))
            .and_then(serde_json::Value::as_bool)
            == Some(false);
        *self.rules.never.lock().unwrap() = never;
        *self.rules.can_ask.lock().unwrap() = params
            .capabilities
            .window
            .as_ref()
            .is_some_and(|w| w.show_message.is_some());
        self.phpstan.set_settings(
            phpstan::editor_settings(params.initialization_options.as_ref()),
            params
                .capabilities
                .window
                .as_ref()
                .and_then(|w| w.work_done_progress)
                == Some(true),
        );
        *self.watch_dynamic.lock().unwrap() = params
            .capabilities
            .workspace
            .as_ref()
            .and_then(|w| w.did_change_watched_files.as_ref())
            .and_then(|d| d.dynamic_registration)
            .unwrap_or(false);
        *self.type_hierarchy_dynamic.lock().unwrap() = handlers::type_hierarchy_dynamic(&params);
        Ok(handlers::initialize_result(utf8))
    }

    async fn initialized(&self, _: InitializedParams) {
        self.load().await;
        if *self.type_hierarchy_dynamic.lock().unwrap() {
            let registration = handlers::type_hierarchy_registration();
            if let Err(error) = self.client.register_capability(vec![registration]).await {
                tracing::warn!(%error, "cannot offer a type hierarchy");
            }
        }
        if *self.watch_dynamic.lock().unwrap() {
            let watchers = [
                "**/*.php",
                // configurations, what they include and baselines, by whatever name
                "**/*.neon*",
                "**/*.rhai",
                "**/composer.json",
                "**/composer.lock",
                "**/composer/installed.json",
            ]
            .into_iter()
            .map(|g| FileSystemWatcher {
                glob_pattern: GlobPattern::String(g.into()),
                kind: None,
            })
            .collect();
            let options = DidChangeWatchedFilesRegistrationOptions { watchers };
            let registration = Registration {
                id: "stirphp-watch".into(),
                method: "workspace/didChangeWatchedFiles".into(),
                register_options: serde_json::to_value(options).ok(),
            };
            if let Err(error) = self.client.register_capability(vec![registration]).await {
                tracing::warn!(%error, "cannot watch files");
            }
        }
    }

    async fn shutdown(&self) -> Result<()> {
        self.phpstan.shut_down().await;
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let Some(path) = uri_path(&params.text_document.uri) else {
            return;
        };
        let doc = params.text_document;
        self.write(|s| s.open(path, doc.version, doc.text));
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let Some(path) = uri_path(&params.text_document.uri) else {
            return;
        };
        let enc = self.encoding();
        let version = params.text_document.version;
        self.write(|s| s.change(path, version, params.content_changes, enc));
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let Some(path) = uri_path(&params.text_document.uri) else {
            return;
        };
        let in_project = self.state.lock().unwrap().is_project_path(&path);
        if in_project && is_source_path(&path) {
            self.phpstan.request();
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let Some(path) = uri_path(&params.text_document.uri) else {
            return;
        };
        // back to what is on disk
        let disk = tokio::fs::read_to_string(&path).await.ok();
        self.write(|s| s.close(&path, disk));
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        let mut reload = false;
        let mut rules_changed = false;
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
            if matches!(name, "composer.json" | "composer.lock" | "installed.json")
                || name.contains(".neon")
            {
                reload = true;
                continue;
            }
            if name.ends_with(".rhai") {
                let s = self.state.lock().unwrap();
                rules_changed |= s.rules.iter().any(|r| r.path(&s.db) == &path);
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
        if rules_changed {
            self.reload_rules().await;
        }
        self.write(|s| s.stored_changed(updates));
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = params
            .text_document_position_params
            .text_document
            .uri
            .clone();
        self.read(&uri, move |s, f, m| handlers::hover(s, f, m, params))
            .await
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = params
            .text_document_position_params
            .text_document
            .uri
            .clone();
        self.read(&uri, move |s, f, m| handlers::definition(s, f, m, params))
            .await
    }

    async fn goto_type_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let uri = params
            .text_document_position_params
            .text_document
            .uri
            .clone();
        self.read(&uri, move |s, f, m| {
            handlers::type_definition(s, f, m, params)
        })
        .await
    }

    async fn goto_declaration(
        &self,
        params: request::GotoDeclarationParams,
    ) -> Result<Option<request::GotoDeclarationResponse>> {
        let uri = params
            .text_document_position_params
            .text_document
            .uri
            .clone();
        self.read(&uri, move |s, f, m| handlers::declaration(s, f, m, params))
            .await
    }

    async fn goto_implementation(
        &self,
        params: request::GotoImplementationParams,
    ) -> Result<Option<request::GotoImplementationResponse>> {
        let uri = params
            .text_document_position_params
            .text_document
            .uri
            .clone();
        self.read(&uri, move |s, f, m| {
            handlers::implementation(s, f, m, params)
        })
        .await
    }

    async fn prepare_call_hierarchy(
        &self,
        params: CallHierarchyPrepareParams,
    ) -> Result<Option<Vec<CallHierarchyItem>>> {
        let uri = params
            .text_document_position_params
            .text_document
            .uri
            .clone();
        self.read(&uri, move |s, f, m| {
            handlers::prepare_call_hierarchy(s, f, m, params)
        })
        .await
    }

    async fn incoming_calls(
        &self,
        params: CallHierarchyIncomingCallsParams,
    ) -> Result<Option<Vec<CallHierarchyIncomingCall>>> {
        self.read_ws(move |s| handlers::incoming_calls(s, params))
            .await
    }

    async fn outgoing_calls(
        &self,
        params: CallHierarchyOutgoingCallsParams,
    ) -> Result<Option<Vec<CallHierarchyOutgoingCall>>> {
        self.read_ws(move |s| handlers::outgoing_calls(s, params))
            .await
    }

    async fn prepare_type_hierarchy(
        &self,
        params: TypeHierarchyPrepareParams,
    ) -> Result<Option<Vec<TypeHierarchyItem>>> {
        let uri = params
            .text_document_position_params
            .text_document
            .uri
            .clone();
        self.read(&uri, move |s, f, m| {
            handlers::prepare_type_hierarchy(s, f, m, params)
        })
        .await
    }

    async fn supertypes(
        &self,
        params: TypeHierarchySupertypesParams,
    ) -> Result<Option<Vec<TypeHierarchyItem>>> {
        self.read_ws(move |s| handlers::supertypes(s, params)).await
    }

    async fn subtypes(
        &self,
        params: TypeHierarchySubtypesParams,
    ) -> Result<Option<Vec<TypeHierarchyItem>>> {
        self.read_ws(move |s| handlers::subtypes(s, params)).await
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let uri = params.text_document_position.text_document.uri.clone();
        self.read(&uri, move |s, f, m| handlers::references(s, f, m, params))
            .await
    }

    async fn document_highlight(
        &self,
        params: DocumentHighlightParams,
    ) -> Result<Option<Vec<DocumentHighlight>>> {
        let uri = params
            .text_document_position_params
            .text_document
            .uri
            .clone();
        self.read(&uri, move |s, f, m| {
            handlers::document_highlight(s, f, m, params)
        })
        .await
    }

    async fn prepare_rename(
        &self,
        params: TextDocumentPositionParams,
    ) -> Result<Option<PrepareRenameResponse>> {
        let uri = params.text_document.uri.clone();
        let result = self
            .read(&uri, move |s, f, m| {
                Some(handlers::prepare_rename(s, f, m, params))
            })
            .await?;
        result.transpose().map(Option::flatten).map_err(failure)
    }

    async fn rename(&self, params: RenameParams) -> Result<Option<WorkspaceEdit>> {
        handlers::check_new_name(&params).map_err(failure)?;
        let uri = params.text_document_position.text_document.uri.clone();
        let result = self
            .read(&uri, move |s, f, m| Some(handlers::rename(s, f, m, params)))
            .await?;
        result.transpose().map(Option::flatten).map_err(failure)
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri.clone();
        self.read(&uri, move |s, f, m| handlers::completion(s, f, m, params))
            .await
    }

    async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        self.actions(params).await
    }

    async fn code_action_resolve(&self, action: CodeAction) -> Result<CodeAction> {
        self.resolve_action(action).await
    }

    async fn execute_command(&self, params: ExecuteCommandParams) -> Result<Option<LSPAny>> {
        self.run_command(params).await
    }

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        let indent = self.state.lock().unwrap().indent;
        let uri = params.text_document.uri.clone();
        self.read(&uri, move |s, f, m| {
            handlers::formatting(s, f, m, params, indent)
        })
        .await
    }

    async fn range_formatting(
        &self,
        params: DocumentRangeFormattingParams,
    ) -> Result<Option<Vec<TextEdit>>> {
        let indent = self.state.lock().unwrap().indent;
        let uri = params.text_document.uri.clone();
        self.read(&uri, move |s, f, m| {
            handlers::range_formatting(s, f, m, params, indent)
        })
        .await
    }

    async fn signature_help(&self, params: SignatureHelpParams) -> Result<Option<SignatureHelp>> {
        let uri = params
            .text_document_position_params
            .text_document
            .uri
            .clone();
        self.read(&uri, move |s, f, m| {
            handlers::signature_help(s, f, m, params)
        })
        .await
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        let uri = params.text_document.uri.clone();
        self.read(&uri, move |s, f, m| {
            handlers::document_symbol_response(s, f, m, params)
        })
        .await
    }

    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<WorkspaceSymbolResponse>> {
        self.read_ws(move |s| handlers::workspace_symbols(s, params))
            .await
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>> {
        let uri = params.text_document.uri.clone();
        self.read(&uri, move |s, f, m| {
            handlers::semantic_tokens(s, f, m, params)
        })
        .await
    }

    async fn inlay_hint(&self, params: InlayHintParams) -> Result<Option<Vec<InlayHint>>> {
        let uri = params.text_document.uri.clone();
        self.read(&uri, move |s, f, m| handlers::inlay_hints(s, f, m, params))
            .await
    }

    async fn folding_range(&self, params: FoldingRangeParams) -> Result<Option<Vec<FoldingRange>>> {
        let uri = params.text_document.uri.clone();
        self.read(&uri, move |s, f, m| {
            handlers::folding_ranges(s, f, m, params)
        })
        .await
    }

    async fn selection_range(
        &self,
        params: SelectionRangeParams,
    ) -> Result<Option<Vec<SelectionRange>>> {
        let uri = params.text_document.uri.clone();
        self.read(&uri, move |s, f, m| {
            handlers::selection_ranges(s, f, m, params)
        })
        .await
    }
}
