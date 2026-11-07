//! A project's scripts: its own rules, return types and magic members, in Rhai.
//!
//! [`stir_script`] is the sandbox. Here's what scripts see of a project, and how they fit into the
//! queries. A script's text is a salsa input and each script is compiled once, so changing one
//! re-runs what ran it. Everything a script reads through its facades happens within the query
//! calling it, on its thread, which makes that a tracked dependency like any other.
//!
//! A script failing never stops the analysis: it's left out, for that file, and
//! [`script_diagnostics`] says so once, at the script.

use std::path::PathBuf;
use std::sync::{Arc, LazyLock};

use rustc_hash::{FxHashMap, FxHashSet};
use smol_str::SmolStr;
use stir_db::{Db, Diagnostic, ScriptFile, SourceFile, Workspace};
use stir_script::ScriptError;
use stir_script::rhai::{AST, Array, Engine, Map};
use stir_syntax::{SyntaxKind, TextRange, TextSize};

use crate::codes;

mod facade;
pub(crate) mod hooks;
pub(crate) mod magic;
mod members;
mod node;
mod reflect;
mod rules;

pub(crate) use facade::ScriptCtx;
pub use magic::Origin;
pub use members::magic_origin;
pub(crate) use node::Node;
pub use rules::{RuleCheck, rule_check};

/// The engine all scripts share: it holds nothing of any one of them.
pub(crate) static ENGINE: LazyLock<Engine> = LazyLock::new(|| {
    let mut engine = stir_script::sandbox();
    // an edit stops what it makes pointless, also in the middle of a call
    engine.on_progress(|operations| {
        if operations.trailing_zeros() >= 12 {
            stir_db::attached(|db| db.unwind_if_revision_cancelled());
        }
        None
    });
    node::register(&mut engine);
    facade::register(&mut engine);
    reflect::register(&mut engine);
    engine
});

/// Prefix of the functions rules subscribe to node kinds with.
const VISIT: &str = "visit_";
pub(crate) const RETURN_TYPE_OF_FUNCTION: &str = "return_type_of_function";
pub(crate) const RETURN_TYPE_OF_METHOD: &str = "return_type_of_method";
pub(crate) const RETURN_TYPE_OF_STATIC_METHOD: &str = "return_type_of_static_method";
pub(crate) const MAGIC_MEMBERS: &str = "magic_members";
pub(crate) const MAGIC_METHOD: &str = "magic_method";
pub(crate) const MAGIC_PROPERTY: &str = "magic_property";
/// What a script answers return types for: its `return_type_of_*` only get called for these.
const RETURN_TYPE_TARGETS: &str = "return_type_targets";

/// A script, ready to run.
pub struct Compiled {
    pub(crate) ast: Option<Arc<AST>>,
    /// Why it can't be used.
    error: Option<ScriptError>,
    /// The functions rules are in, with the kinds of node they visit.
    visits: Vec<(&'static [SyntaxKind], SmolStr)>,
    targets: Targets,
    magic: bool,
    magic_method: bool,
    magic_property: bool,
}

/// What `return_type_targets()` says.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Targets {
    functions: Vec<SmolStr>,
    /// `(class, method)`, lowercase.
    methods: Vec<(SmolStr, SmolStr)>,
    static_methods: Vec<(SmolStr, SmolStr)>,
}

/// Is `script` one of ours, rather than the project's?
pub fn is_bundled(db: &dyn Db, script: ScriptFile) -> bool {
    script
        .path(db)
        .to_str()
        .is_some_and(|p| p.starts_with(stir_db::BUNDLED_PREFIX))
}

fn script_name(path: &std::path::Path) -> String {
    path.file_name()
        .map_or_else(|| path.display().to_string(), |n| n.display().to_string())
}

/// Compiles `script` and reads what it declares. Only happens again when its text changes.
#[salsa::tracked(returns(ref), no_eq)]
pub fn compiled_script(db: &dyn Db, script: ScriptFile) -> Compiled {
    let refuse = |error| Compiled {
        ast: None,
        error: Some(error),
        visits: Vec::new(),
        targets: Targets::default(),
        magic: false,
        magic_method: false,
        magic_property: false,
    };
    let text = match script.text(db) {
        Ok(text) => text,
        Err(why) => return refuse(ScriptError::new(why.clone())),
    };
    let name = script_name(script.path(db));
    let ast = match stir_script::compile(&ENGINE, &name, text) {
        Ok(ast) => ast,
        Err(error) => return refuse(error),
    };
    match read_declarations(&ast) {
        Ok(d) => Compiled {
            ast: Some(ast),
            error: None,
            visits: d.visits,
            targets: d.targets,
            magic: d.magic,
            magic_method: d.magic_method,
            magic_property: d.magic_property,
        },
        Err(error) => refuse(error),
    }
}

/// What a script declares by the functions it has.
struct Declarations {
    visits: Vec<(&'static [SyntaxKind], SmolStr)>,
    targets: Targets,
    magic: bool,
    magic_method: bool,
    magic_property: bool,
}

fn read_declarations(ast: &AST) -> Result<Declarations, ScriptError> {
    let mut visits = Vec::new();
    let mut magic = false;
    let (mut magic_method, mut magic_property) = (false, false);
    let mut has_targets = false;
    let mut needs_targets = None;
    let takes = |name: &str, found: usize, wanted: usize| {
        if found == wanted {
            return Ok(());
        }
        Err(ScriptError::new(format!(
            "`{name}` takes {wanted} parameter(s) when it's called, not {found}"
        )))
    };
    let mut functions: Vec<_> = ast.iter_functions().collect();
    functions.sort_by_key(|f| f.name);
    for f in functions {
        let found = f.params.len();
        if let Some(kind) = f.name.strip_prefix(VISIT) {
            let Some(kinds) = node::syntax_kinds(kind) else {
                let known: Vec<_> = node::kind_names().collect();
                return Err(ScriptError::new(format!(
                    "there are no `{kind}` nodes for `{}` to visit: there's {}",
                    f.name,
                    known.join(", ")
                )));
            };
            takes(f.name, found, 2)?;
            visits.push((kinds, SmolStr::new(f.name)));
            continue;
        }
        match f.name {
            RETURN_TYPE_OF_FUNCTION | RETURN_TYPE_OF_METHOD | RETURN_TYPE_OF_STATIC_METHOD => {
                takes(f.name, found, 2)?;
                needs_targets = needs_targets.or(Some(f.name));
            }
            RETURN_TYPE_TARGETS => {
                takes(f.name, found, 0)?;
                has_targets = true;
            }
            MAGIC_MEMBERS => {
                takes(f.name, found, 1)?;
                magic = true;
            }
            MAGIC_METHOD => {
                takes(f.name, found, 3)?;
                magic_method = true;
            }
            MAGIC_PROPERTY => {
                takes(f.name, found, 2)?;
                magic_property = true;
            }
            _ => {}
        }
    }
    let targets = if has_targets {
        read_targets(ast)?
    } else if let Some(name) = needs_targets {
        return Err(ScriptError::new(format!(
            "`{name}` needs a `{RETURN_TYPE_TARGETS}()` to say what it answers for"
        )));
    } else {
        Targets::default()
    };
    Ok(Declarations {
        visits,
        targets,
        magic,
        magic_method,
        magic_property,
    })
}

fn read_targets(ast: &AST) -> Result<Targets, ScriptError> {
    let answer = stir_script::call(&ENGINE, ast, RETURN_TYPE_TARGETS, Vec::new())?;
    let wrong = |what: &str| {
        ScriptError::new(format!(
            "`{RETURN_TYPE_TARGETS}()` should answer `#{{ functions: [..], methods: [\"Class::method\", ..], static_methods: [..] }}`: {what}"
        ))
    };
    let Some(map) = answer.try_cast::<Map>() else {
        return Err(wrong("that isn't a map"));
    };
    let list = |key: &str| -> Result<Vec<String>, ScriptError> {
        let Some(value) = map.get(key) else {
            return Ok(Vec::new());
        };
        let Some(items) = value.clone().try_cast::<Array>() else {
            return Err(wrong(&format!("`{key}` isn't an array")));
        };
        items
            .into_iter()
            .map(|item| {
                item.into_string()
                    .map_err(|_| wrong(&format!("`{key}` has something else than strings")))
            })
            .collect()
    };
    let member = |name: String| {
        let name = name.trim_start_matches('\\').to_ascii_lowercase();
        let (class, method) = name
            .rsplit_once("::")
            .ok_or_else(|| wrong(&format!("`{name}` isn't `Class::method`")))?;
        Ok::<_, ScriptError>((SmolStr::new(class), SmolStr::new(method)))
    };
    Ok(Targets {
        functions: list("functions")?
            .into_iter()
            .map(|f| SmolStr::new(f.trim_start_matches('\\').to_ascii_lowercase()))
            .collect(),
        methods: list("methods")?
            .into_iter()
            .map(member)
            .collect::<Result<_, _>>()?,
        static_methods: list("static_methods")?
            .into_iter()
            .map(member)
            .collect::<Result<_, _>>()?,
    })
}

/// What all of a workspace's scripts subscribe to. Equal unless what they declare changes, so that
/// editing a rule's body leaves type checks alone.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ScriptIndex {
    /// By syntax kind: the scripts with a rule for it, and the rule's function.
    pub(crate) visits: FxHashMap<SyntaxKind, Vec<(ScriptFile, SmolStr)>>,
    /// By lowercase method name: the class it's on, and the scripts answering for it.
    pub(crate) methods: FxHashMap<SmolStr, Vec<(SmolStr, ScriptFile)>>,
    pub(crate) static_methods: FxHashMap<SmolStr, Vec<(SmolStr, ScriptFile)>>,
    /// By lowercase function name.
    pub(crate) functions: FxHashMap<SmolStr, Vec<ScriptFile>>,
    pub(crate) magic: Vec<ScriptFile>,
    /// The scripts with a `magic_method`, and those with a `magic_property`.
    pub(crate) magic_methods: Vec<ScriptFile>,
    pub(crate) magic_properties: Vec<ScriptFile>,
}

impl ScriptIndex {
    pub(crate) fn has_return_hooks(&self) -> bool {
        !self.methods.is_empty() || !self.static_methods.is_empty() || !self.functions.is_empty()
    }
}

/// See [`ScriptIndex`].
#[salsa::tracked(returns(ref))]
pub fn script_index(db: &dyn Db, ws: Workspace) -> ScriptIndex {
    let mut index = ScriptIndex::default();
    // the project's first: what they say counts over what ours say
    for &script in ws.scripts(db).iter().chain(ws.bundled_scripts(db)) {
        let c = compiled_script(db, script);
        for (kinds, function) in &c.visits {
            for kind in *kinds {
                let rules = index.visits.entry(*kind).or_default();
                rules.push((script, function.clone()));
            }
        }
        for f in &c.targets.functions {
            index.functions.entry(f.clone()).or_default().push(script);
        }
        for (class, method) in &c.targets.methods {
            let hooks = index.methods.entry(method.clone()).or_default();
            hooks.push((class.clone(), script));
        }
        for (class, method) in &c.targets.static_methods {
            let hooks = index.static_methods.entry(method.clone()).or_default();
            hooks.push((class.clone(), script));
        }
        if c.magic {
            index.magic.push(script);
        }
        if c.magic_method {
            index.magic_methods.push(script);
        }
        if c.magic_property {
            index.magic_properties.push(script);
        }
    }
    index
}

/// A call of a script that failed.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ScriptFailure {
    pub script: ScriptFile,
    /// The function that was called.
    pub function: SmolStr,
    pub message: String,
    /// Where in the script, as far as it's known.
    pub line: Option<u32>,
    pub column: Option<u32>,
    /// The PHP file being checked at the time.
    pub file: Option<PathBuf>,
}

impl ScriptFailure {
    pub(crate) fn new(
        script: ScriptFile,
        function: &str,
        error: ScriptError,
        file: Option<PathBuf>,
    ) -> Self {
        ScriptFailure {
            script,
            function: SmolStr::new(function),
            message: error.message,
            line: error.line,
            column: error.column,
            file,
        }
    }
}

/// A range for what's wrong at `error`'s place in `script`.
fn script_range(db: &dyn Db, script: ScriptFile, error: &ScriptError) -> TextRange {
    let Ok(text) = script.text(db) else {
        return TextRange::empty(TextSize::new(0));
    };
    let start = error.offset_in(text).min(text.len() as u32);
    let end = (start + 1).min(text.len() as u32);
    TextRange::new(TextSize::new(start), TextSize::new(end))
}

/// What's wrong with `ws`'s scripts as far as `files` show: those that can't be used, and for
/// each function that failed, the first failure of those files (the first of them in order). One
/// diagnostic, at the script, however many files it failed for.
pub fn script_diagnostics(db: &dyn Db, ws: Workspace, files: &[SourceFile]) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for &script in ws.bundled_scripts(db) {
        if let Some(error) = &compiled_script(db, script).error {
            tracing::error!(script = %script.path(db).display(), %error, "a bundled script can't be used");
        }
    }
    for &script in ws.scripts(db) {
        if let Some(error) = &compiled_script(db, script).error {
            let name = script_name(script.path(db));
            out.push(Diagnostic::custom(
                script.path(db).clone(),
                script_range(db, script, error),
                stir_db::Severity::Error,
                codes::SCRIPT_NOT_USABLE,
                format!("the script `{name}` can't be used: {error}"),
            ));
        }
    }
    let mut failed: FxHashMap<(ScriptFile, SmolStr), (&ScriptFailure, FxHashSet<&PathBuf>)> =
        FxHashMap::default();
    let mut order = Vec::new();
    for &file in files {
        for failure in &crate::checked_file(db, ws, file).script_failures {
            // ours failing is our mistake, not the project's: it's logged, and that's all
            if is_bundled(db, failure.script) {
                tracing::warn!(
                    script = %failure.script.path(db).display(),
                    function = %failure.function,
                    file = ?failure.file,
                    message = %failure.message,
                    "a bundled script failed"
                );
                continue;
            }
            let key = (failure.script, failure.function.clone());
            let entry = failed.entry(key.clone()).or_insert_with(|| {
                order.push(key);
                (failure, FxHashSet::default())
            });
            entry.1.extend(failure.file.as_ref());
        }
    }
    for key in order {
        let (failure, in_files) = &failed[&key];
        let script = failure.script;
        let error = ScriptError {
            message: failure.message.clone(),
            line: failure.line,
            column: failure.column,
        };
        let name = script_name(script.path(db));
        let mut message = format!(
            "the script `{name}` failed in `{}`: {}",
            failure.function, failure.message
        );
        if let Some(file) = &failure.file {
            message.push_str(&format!(", while checking `{}`", file.display()));
        }
        let mut d = Diagnostic::custom(
            script.path(db).clone(),
            script_range(db, script, &error),
            stir_db::Severity::Error,
            codes::SCRIPT_FAILED,
            message,
        );
        if in_files.len() > 1 {
            d = d.with_note(format!(
                "it failed for {} files, and it's left out of those",
                in_files.len()
            ));
        }
        out.push(d);
    }
    out
}
