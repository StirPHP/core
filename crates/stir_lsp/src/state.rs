//! What a server knows of a workspace: the salsa database, its files and the documents open in
//! the editor. The same whatever the client speaks through.

use std::path::{Path, PathBuf};

use ls_types::TextDocumentContentChangeEvent;
use rustc_hash::FxHashMap;
use salsa::Setter as _;
use stir_db::{FileOrigin, RootDatabase, ScriptFile, Settings, SourceFile, Workspace};
use stir_project::workspace::builtin_files;

use crate::convert::{Encoding, LineMap};

pub struct State {
    pub db: RootDatabase,
    pub ws: Workspace,
    /// Files with a path (project sources and vendor), by path.
    pub files: FxHashMap<PathBuf, SourceFile>,
    /// Documents open in the editor, with their version.
    pub open: FxHashMap<PathBuf, i32>,
    /// The project root (where its configuration or `composer.json` is, else the workspace folder).
    pub root: Option<PathBuf>,
    /// Directories whose files are not project sources (generated output,
    /// `vendor/`): opened files there are dependencies, never checked.
    pub foreign: Vec<PathBuf>,
    /// The project's scripts, whether they're allowed to run or not. Those that are in the
    /// workspace's `scripts`, and those alone, run.
    pub rules: Vec<ScriptFile>,
    /// How the project says to indent, over what the editor says.
    pub indent: Option<stir_neon::phpstan::FormatIndent>,
}

impl State {
    pub fn new(
        db: RootDatabase,
        ws: Workspace,
        root: Option<PathBuf>,
        foreign: Vec<PathBuf>,
    ) -> Self {
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
            foreign,
            root,
            rules: Vec::new(),
            indent: None,
        }
    }

    /// A workspace with only the builtin stubs.
    pub fn empty(foreign: Vec<PathBuf>) -> Self {
        let db = RootDatabase::default();
        let files = builtin_files(&db);
        let ws = Workspace::new(&db, files, Settings::default());
        State::new(db, ws, None, foreign)
    }

    /// Lets the project's scripts run, or stops them.
    pub fn set_rules_running(&mut self, running: bool) {
        let scripts = if running {
            self.rules.clone()
        } else {
            Vec::new()
        };
        if self.ws.scripts(&self.db) != &scripts {
            self.ws.set_scripts(&mut self.db).to(scripts);
        }
    }

    /// Sets the text of `path`, adding it as a project source if it is new.
    pub fn set_text(&mut self, path: &Path, text: String) -> SourceFile {
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

    pub fn remove(&mut self, path: &Path) {
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

    /// A document opened in the editor.
    pub fn open(&mut self, path: PathBuf, version: i32, text: String) {
        self.set_text(&path, text);
        self.open.insert(path, version);
    }

    /// Applies the editor's changes to the open document at `path`. Older versions than what's
    /// there are ignored.
    pub fn change(
        &mut self,
        path: PathBuf,
        version: i32,
        changes: Vec<TextDocumentContentChangeEvent>,
        enc: Encoding,
    ) {
        if self.open.get(&path).is_some_and(|v| *v > version) {
            return;
        }
        let Some(&file) = self.files.get(&path) else {
            return;
        };
        let mut text = file.text(&self.db).clone();
        for change in changes {
            match change.range {
                Some(range) => {
                    let map = LineMap::new(&text, enc);
                    let r = map.text_range(range);
                    text.replace_range(usize::from(r.start())..usize::from(r.end()), &change.text);
                }
                None => text = change.text,
            }
        }
        self.set_text(&path, text);
        self.open.insert(path, version);
    }

    /// A document closed in the editor: back to `stored`, the file's text where it's kept, for a
    /// file of the project; otherwise it's gone.
    pub fn close(&mut self, path: &Path, stored: Option<String>) {
        self.open.remove(path);
        let in_project = self.is_project_path(path);
        match stored {
            Some(text) if in_project => {
                self.set_text(path, text);
            }
            _ => self.remove(path),
        }
    }

    /// Takes in files that changed where they're kept (`None`: deleted). Open documents keep the
    /// editor's text.
    pub fn stored_changed(&mut self, updates: Vec<(PathBuf, Option<String>)>) {
        for (path, text) in updates {
            if self.open.contains_key(&path) {
                continue;
            }
            let in_project = self.is_project_path(&path);
            match text {
                Some(text) if self.files.contains_key(&path) || in_project => {
                    self.set_text(&path, text);
                }
                Some(_) => {}
                None => self.remove(&path),
            }
        }
    }

    /// Replaces this with `state`, a project loaded again, keeping the documents open in the editor
    /// with their text.
    pub fn reload(&mut self, mut state: State) {
        let open: Vec<(PathBuf, i32, String)> = self
            .open
            .iter()
            .filter_map(|(p, v)| {
                self.files
                    .get(p)
                    .map(|f| (p.clone(), *v, f.text(&self.db).clone()))
            })
            .collect();
        for (path, version, text) in open {
            state.open(path, version, text);
        }
        *self = state;
    }

    /// Is `path` a project source location (in the project, not generated or vendored)?
    pub fn is_project_path(&self, path: &Path) -> bool {
        self.root.as_ref().is_some_and(|r| path.starts_with(r))
            && !self.foreign.iter().any(|d| path.starts_with(d))
    }

    pub fn sources(&self) -> Vec<SourceFile> {
        self.ws
            .files(&self.db)
            .iter()
            .copied()
            .filter(|f| f.origin(&self.db) == FileOrigin::Source)
            .collect()
    }
}
