//! PHPStan in the background, for a project that has it. See `stir_phpstan`.
//!
//! A save asks for a run of the whole project, since a change in one file can cause errors in
//! others. A save during a run waits for it to end, and then there's another: a run that's killed
//! leaves no result cache, so with saves coming faster than a first run takes, there'd never be
//! any results. Only what makes a run pointless kills it: settings that change, and shutting down.
//!
//! PHPStan reads files from disk, so its lines are only right for the text it read. What's kept of
//! a run says which text that was, and a file with other text now shows nothing of PHPStan's until
//! a run has seen it.

use std::hash::BuildHasher as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rustc_hash::{FxBuildHasher, FxHashMap, FxHashSet};
use stir_neon::phpstan::{PhpstanRun, valid_memory_limit, valid_phpstan_level};
use stir_phpstan::{Failure, Invocation, Located, Report, Reported, Wanted};
use tokio::sync::{Notify, watch};
use tower_lsp_server::Client;
use tower_lsp_server::ls_types::*;

use super::{RulesTrust, State};
use crate::trust::{self, TrustStore};

/// Saving everything at once is one save.
const SAVE_DELAY: Duration = Duration::from_millis(200);
/// How long a client gets to answer whether it can show progress. Runs don't wait for it.
const PROGRESS_WAIT: Duration = Duration::from_secs(2);
/// How long shutting down waits for a run to be gone. Killing doesn't take long.
const STOP_WAIT: Duration = Duration::from_secs(5);

/// What a text is known by, to tell whether it's still the one PHPStan read.
pub(super) fn text_hash(text: &str) -> u64 {
    FxBuildHasher.hash_one(text)
}

/// What PHPStan said of a file.
#[derive(Debug)]
pub(super) struct FileResult {
    /// The text PHPStan read.
    pub text: u64,
    pub reported: Vec<Reported>,
}

/// What the last run said.
pub(super) type Results = FxHashMap<PathBuf, FileResult>;

/// The editor's settings for PHPStan, from `initializationOptions`. Empty ones aren't set.
pub(super) fn editor_settings(options: Option<&serde_json::Value>) -> PhpstanRun {
    let Some(options) = options.and_then(|o| o.get("phpstan")) else {
        return PhpstanRun::default();
    };
    let text = |key: &str| {
        let value = options.get(key)?;
        let text = value
            .as_str()
            .map(str::to_owned)
            .or_else(|| value.as_i64().map(|n| n.to_string()))?;
        (!text.is_empty()).then_some(text)
    };
    let enabled = match options.get("enable") {
        Some(serde_json::Value::Bool(enable)) => Some(*enable),
        Some(serde_json::Value::String(enable)) if enable == "on" => Some(true),
        Some(serde_json::Value::String(enable)) if enable == "off" => Some(false),
        _ => None,
    };
    PhpstanRun {
        enabled,
        binary: text("path").map(PathBuf::from),
        configuration: text("configuration").map(PathBuf::from),
        level: text("level").filter(|l| valid_phpstan_level(l)),
        memory_limit: text("memoryLimit").filter(|m| valid_memory_limit(m)),
        timeout: options
            .get("timeout")
            .and_then(serde_json::Value::as_u64)
            .filter(|t| *t > 0),
    }
}

/// Everything about running PHPStan for a server. It's a set of handles, one of them with the task
/// doing the running.
#[derive(Clone)]
pub(super) struct Phpstan {
    client: Client,
    state: Arc<Mutex<State>>,
    diagnostics: Arc<Notify>,
    /// What the diagnostics are merged with.
    pub results: Arc<Mutex<Arc<Results>>>,
    settings: Arc<Mutex<PhpstanRun>>,
    /// What to run, if it's allowed and there's anything to run.
    plan: Arc<Mutex<Option<Invocation>>>,
    /// There's a reason for another run.
    wake: Arc<Notify>,
    /// Kills what's running when it changes.
    stop: Arc<watch::Sender<u64>>,
    /// Is a run going on?
    busy: Arc<watch::Sender<bool>>,
    shut_down: Arc<AtomicBool>,
    can_show_progress: Arc<AtomicBool>,
    progress_tokens: Arc<AtomicU64>,
    /// What the person's been told since it last worked, so it's only told once.
    told: Arc<Mutex<FxHashSet<String>>>,
    store: TrustStore,
    /// What's being decided: the hash of what's being or has been asked about.
    current: Arc<Mutex<Option<String>>>,
    asking: Arc<Mutex<Option<String>>>,
    rules: RulesTrust,
}

impl Phpstan {
    pub fn new(
        client: Client,
        state: Arc<Mutex<State>>,
        diagnostics: Arc<Notify>,
        rules: RulesTrust,
    ) -> Phpstan {
        let phpstan = Phpstan {
            client,
            state,
            diagnostics,
            results: Arc::default(),
            settings: Arc::default(),
            plan: Arc::default(),
            wake: Arc::default(),
            stop: Arc::new(watch::channel(0).0),
            busy: Arc::new(watch::channel(false).0),
            shut_down: Arc::default(),
            can_show_progress: Arc::default(),
            progress_tokens: Arc::default(),
            told: Arc::default(),
            store: rules.store.phpstan(),
            current: Arc::default(),
            asking: Arc::default(),
            rules,
        };
        tokio::spawn(phpstan.clone().run_loop());
        phpstan
    }

    pub fn set_settings(&self, settings: PhpstanRun, can_show_progress: bool) {
        *self.settings.lock().unwrap() = settings;
        self.can_show_progress
            .store(can_show_progress, Ordering::Relaxed);
    }

    /// Asks for a run, after a save.
    pub fn request(&self) {
        self.wake.notify_one();
    }

    /// Stops what's running, and everything from now on, for shutting down.
    pub async fn shut_down(&self) {
        self.shut_down.store(true, Ordering::Relaxed);
        self.stop.send_modify(|n| *n += 1);
        self.wake.notify_one();
        let mut busy = self.busy.subscribe();
        let gone = busy.wait_for(|busy| !busy);
        let _ = tokio::time::timeout(STOP_WAIT, gone).await;
    }

    /// Decides what to do about PHPStan for the project in `root`, as it now is.
    pub fn decide(&self, project: Option<(&Path, &Path, &PhpstanRun)>) {
        let Some((root, vendor_dir, neon)) = project else {
            self.set_plan(None);
            return;
        };
        // not even whether it's there: nothing about this workspace has been agreed to
        if *self.rules.never.lock().unwrap() {
            self.set_plan(None);
            return;
        }
        let run = self.settings.lock().unwrap().clone().or(neon);
        let invocation = match stir_phpstan::locate(root, vendor_dir, &run, Wanted::IfSetUp) {
            Located::Off => {
                self.set_plan(None);
                return;
            }
            Located::Missing(why) => {
                self.set_plan(None);
                self.tell("missing", why);
                return;
            }
            Located::Found(invocation) => invocation,
        };
        let hash = trust::phpstan_hash(root, &invocation);
        *self.current.lock().unwrap() = Some(hash.clone());
        match self.store.decision(root, &hash) {
            Some(true) => self.set_plan(Some(invocation)),
            Some(false) => self.set_plan(None),
            None => {
                self.set_plan(None);
                let mut asking = self.asking.lock().unwrap();
                if asking.as_ref() != Some(&hash) {
                    *asking = Some(hash.clone());
                    tokio::spawn(self.clone().ask(root.to_owned(), hash, invocation));
                }
            }
        }
    }

    /// From now on, this is what runs. What's running of anything else stops.
    fn set_plan(&self, plan: Option<Invocation>) {
        let run_again = plan.is_some();
        let cleared = plan.is_none();
        let changed = {
            let mut current = self.plan.lock().unwrap();
            let changed = *current != plan;
            *current = plan;
            changed
        };
        if changed {
            self.stop.send_modify(|n| *n += 1);
        }
        if cleared {
            let had_results = {
                let mut results = self.results.lock().unwrap();
                let had = !results.is_empty();
                *results = Arc::default();
                had
            };
            if had_results {
                self.diagnostics.notify_one();
            }
        }
        if run_again {
            self.request();
        }
    }

    /// Asks whether this may run, and acts on the answer. Allowing a workspace's scripts says
    /// nothing about this: those are sandboxed, and this isn't.
    async fn ask(self, root: PathBuf, hash: String, invocation: Invocation) {
        const ALLOW: &str = "Allow";
        const DENY: &str = "Don't allow";
        let nobody = "PHPStan is set up in this workspace but isn't running, because nobody was asked whether to allow it. `stirphp check --with-phpstan` runs it.";
        if !*self.rules.can_ask.lock().unwrap() {
            self.tell("no answer", nobody.to_owned());
            return;
        }
        let actions = [ALLOW, DENY]
            .map(|title| MessageActionItem {
                title: title.into(),
                properties: Default::default(),
            })
            .to_vec();
        let message = format!(
            "Run PHPStan in this workspace whenever you save? That runs `{}`, which is PHP executing code from the project: PHPStan as installed there, and the bootstrap files and extensions its configuration loads. Unlike custom rules, none of it is sandboxed. Allow it only if you trust this project.",
            invocation.shown()
        );
        let question =
            self.client
                .show_message_request(MessageType::WARNING, message, Some(actions));
        let mut question = std::pin::pin!(question);
        let answer = tokio::select! {
            answer = &mut question => answer,
            () = tokio::time::sleep(self.rules.answer_wait) => {
                self.tell("no answer", nobody.to_owned());
                question.await
            }
        };
        {
            let mut asking = self.asking.lock().unwrap();
            if asking.as_ref() == Some(&hash) {
                *asking = None;
            }
        }
        let allow = match answer {
            Ok(Some(action)) => action.title == ALLOW,
            // put away without an answer: the question comes back next time
            Ok(None) => return,
            Err(error) => {
                tracing::warn!(%error, "no answer about running PHPStan");
                self.tell("no answer", nobody.to_owned());
                return;
            }
        };
        if let Err(error) = self.store.remember(&root, &hash, allow) {
            tracing::warn!(%error, "can't remember the answer about running PHPStan");
        }
        // only what was asked about: it may have changed, and that's asked separately
        if allow && self.current.lock().unwrap().as_ref() == Some(&hash) {
            self.set_plan(Some(invocation));
        }
    }

    /// Tells the person something once, until it next works.
    fn tell(&self, kind: &str, message: String) {
        if !self.told.lock().unwrap().insert(kind.to_owned()) {
            return;
        }
        let client = self.client.clone();
        tokio::spawn(async move {
            let message = format!("PHPStan: {message} StirPHP's own diagnostics aren't affected.");
            client.show_message(MessageType::WARNING, message).await;
        });
    }

    async fn run_loop(self) {
        loop {
            self.wake.notified().await;
            if self.shut_down.load(Ordering::Relaxed) {
                return;
            }
            tokio::time::sleep(SAVE_DELAY).await;
            // what came in meanwhile is seen by this run
            let _ = tokio::time::timeout(Duration::ZERO, self.wake.notified()).await;
            let Some(invocation) = self.plan.lock().unwrap().clone() else {
                continue;
            };

            let mut stop = self.stop.subscribe();
            self.busy.send_replace(true);
            let texts = self.texts_on_disk().await;
            let progress = self.begin_progress().await;
            let start = Instant::now();
            let stopped = async move {
                let _ = stop.changed().await;
            };
            let result = stir_phpstan::run(&invocation, stopped).await;
            self.busy.send_replace(false);
            if let Some(progress) = progress {
                progress.finish().await;
            }
            match result {
                Ok(report) => {
                    self.finished(&invocation, report, texts, start.elapsed())
                        .await
                }
                Err(Failure::Cancelled) => {}
                Err(failure) => {
                    if let Some(output) = failure.output() {
                        self.client
                            .log_message(MessageType::WARNING, format!("PHPStan said:\n{output}"))
                            .await;
                    }
                    self.tell(failure.kind(), failure.describe(&invocation));
                }
            }
        }
    }

    /// A hash of the text PHPStan is going to read of each source file. For a document open in the
    /// editor that's not necessarily what's in the editor.
    async fn texts_on_disk(&self) -> FxHashMap<PathBuf, u64> {
        let (mut texts, open) = {
            let s = self.state.lock().unwrap();
            let mut texts = FxHashMap::default();
            let mut open = Vec::new();
            for file in s.sources() {
                let path = file.path(&s.db);
                if s.open.contains_key(path) {
                    open.push(path.clone());
                } else {
                    texts.insert(path.clone(), text_hash(file.text(&s.db)));
                }
            }
            (texts, open)
        };
        for path in open {
            if let Ok(text) = tokio::fs::read_to_string(&path).await {
                texts.insert(path, text_hash(&text));
            }
        }
        texts
    }

    /// Takes in what a run reported.
    async fn finished(
        &self,
        invocation: &Invocation,
        mut report: Report,
        texts: FxHashMap<PathBuf, u64>,
        took: Duration,
    ) {
        let (stopped, about_config): (Vec<_>, Vec<_>) = std::mem::take(&mut report.errors)
            .into_iter()
            .partition(|e| stir_phpstan::is_incomplete(e));
        if let Some(first) = stopped.first() {
            let advice = if stir_phpstan::is_memory(first) {
                " A higher `memoryLimit` can be set in `parameters.stirphp.phpstan` or the editor's settings."
            } else {
                ""
            };
            self.tell(
                "incomplete",
                format!("the run didn't finish: {}.{advice}", first.trim()),
            );
            // nothing can be concluded from what isn't there
            if report.files.is_empty() {
                return;
            }
        } else {
            self.told.lock().unwrap().clear();
        }
        match &invocation.config_file {
            // these outlast any notice, like the configuration's errors would in the editor
            Some(config) => {
                let reported = report.files.entry(config.clone()).or_default();
                reported.extend(about_config.into_iter().map(|message| Reported {
                    line: None,
                    message,
                    identifier: None,
                    tip: None,
                    ignorable: false,
                }));
            }
            None => {
                for error in about_config {
                    self.tell("configuration", error);
                }
            }
        }
        let count: usize = report.files.values().map(Vec::len).sum();
        let mut results = Results::default();
        for (path, reported) in report.files {
            if reported.is_empty() {
                continue;
            }
            let path = tokio::fs::canonicalize(&path).await.unwrap_or(path);
            let text = match texts.get(&path) {
                Some(text) => *text,
                // not one of our sources: as it is now is the best there is
                None => match tokio::fs::read_to_string(&path).await {
                    Ok(text) => text_hash(&text),
                    Err(_) => continue,
                },
            };
            match results.entry(path) {
                std::collections::hash_map::Entry::Occupied(mut same) => {
                    let same: &mut FileResult = same.get_mut();
                    same.reported.extend(reported);
                }
                std::collections::hash_map::Entry::Vacant(new) => {
                    new.insert(FileResult { text, reported });
                }
            }
        }
        // what it said about something that's been switched off in the meantime doesn't count
        if self.plan.lock().unwrap().as_ref() != Some(invocation) {
            return;
        }
        *self.results.lock().unwrap() = Arc::new(results);
        self.diagnostics.notify_one();
        self.client
            .log_message(
                MessageType::INFO,
                format!(
                    "PHPStan reported {count} errors in {:.1} s",
                    took.as_secs_f32()
                ),
            )
            .await;
    }

    /// Shows that something's running, on a client that can. Whatever goes wrong with that, the
    /// run doesn't depend on it.
    async fn begin_progress(
        &self,
    ) -> Option<
        tower_lsp_server::OngoingProgress<
            tower_lsp_server::Unbounded,
            tower_lsp_server::NotCancellable,
        >,
    > {
        if !self.can_show_progress.load(Ordering::Relaxed) {
            return None;
        }
        let n = self.progress_tokens.fetch_add(1, Ordering::Relaxed);
        let token = NumberOrString::String(format!("stirphp/phpstan/{n}"));
        let create = self.client.send_request::<request::WorkDoneProgressCreate>(
            WorkDoneProgressCreateParams {
                token: token.clone(),
            },
        );
        match tokio::time::timeout(PROGRESS_WAIT, create).await {
            Ok(Ok(())) => Some(self.client.progress(token, "PHPStan").begin().await),
            _ => None,
        }
    }
}
