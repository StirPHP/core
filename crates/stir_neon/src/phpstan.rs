//! Typed loader for `phpstan.neon` / `phpstan.dist.neon`, and for `stirphp.neon`, which has the
//! same shape. Our own settings are in `parameters.stirphp` ([`StirphpParameters`]); the
//! `parametersSchema` section that real PHPStan needs to accept those is read as any other
//! section nothing looks at.
//!
//! Semantics follow Nette DI's config loader as used by PHPStan:
//! - `includes:` are loaded first (recursively, relative to the including file); each later
//!   include overrides earlier ones and the including file overrides all of its includes.
//! - Merging: mappings merge per key (recursively), integer-keyed (list) items are appended,
//!   scalars are replaced. A `!` suffix on a key (`paths!:`) replaces instead of merging.
//! - Path-valued parameters are resolved relative to the file that declared them.
//! - `%param%` references are expanded from the caller-supplied map (e.g.
//!   `currentWorkingDirectory`, `rootDir`) and from scalar `parameters`; `%%` is a literal `%`.

use std::path::{Component, Path, PathBuf};

use indexmap::IndexMap;

use crate::{Key, Value};

/// Errors from [`PhpstanConfig::load`].
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read {}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{}: {source}", path.display())]
    Parse {
        path: PathBuf,
        #[source]
        source: crate::Error,
    },
    #[error("include cycle: {}", chain.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(" -> "))]
    IncludeCycle { chain: Vec<PathBuf> },
    #[error("{}: invalid `{key}`: {message}", path.display())]
    Invalid {
        path: PathBuf,
        key: String,
        message: String,
    },
}

/// `level:` value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// 0–10
    Level(u8),
    Max,
}

/// `excludePaths:` — a plain list means `analyseAndScan`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExcludePaths {
    pub analyse: Vec<PathBuf>,
    pub analyse_and_scan: Vec<PathBuf>,
}

/// One message pattern of an `ignoreErrors` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IgnoreMessage {
    /// `message`/`messages`: a PCRE pattern such as `#^Foo#`.
    Regex(String),
    /// `rawMessage`/`rawMessages`: an exact message.
    Raw(String),
}

/// An `ignoreErrors` entry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IgnoreError {
    pub messages: Vec<IgnoreMessage>,
    /// `identifier`, plus any `identifiers`.
    pub identifiers: Vec<String>,
    /// `path` and `paths` (resolved; may contain fnmatch globs).
    pub paths: Vec<PathBuf>,
    pub count: Option<u32>,
    pub report_unmatched: Option<bool>,
}

/// `parameters.stirphp`: StirPHP's own settings. Paths are resolved against the file declaring them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StirphpParameters {
    /// `srcDir`: where the project's sources are, if not in the project folder.
    pub src_dir: Option<PathBuf>,
    /// `include`: globs of the source files, relative to `srcDir`.
    pub include: Option<Vec<String>>,
    /// `exclude`: globs left out of them, relative to `srcDir` or the project folder.
    pub exclude: Option<Vec<String>>,
    /// `composer`: the path of `composer.json`, if not in the project folder.
    pub composer: Option<PathBuf>,
    /// `baseline`: the file `stirphp check --generate-baseline` writes. PHPStan never sees it,
    /// which is why it isn't in `includes`.
    pub baseline: Option<PathBuf>,
    /// `rules`: the project's own rules, as scripts.
    pub rules: Vec<PathBuf>,
}

/// Options for [`PhpstanConfig::load_with`].
#[derive(Debug, Clone, Default)]
pub struct LoadOptions {
    /// Known `%name%` parameters (e.g. `currentWorkingDirectory`, `rootDir`).
    /// `currentWorkingDirectory` defaults to the process working directory.
    pub params: IndexMap<String, String>,
    /// Configs merged in before the root file's includes, so that everything overrides them:
    /// the ones PHPStan's extension installer would add.
    pub extensions: Vec<PathBuf>,
}

impl LoadOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn param(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.params.insert(name.into(), value.into());
        self
    }

    pub fn extensions(mut self, paths: Vec<PathBuf>) -> Self {
        self.extensions = paths;
        self
    }
}

/// A loaded and merged PHPStan configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct PhpstanConfig {
    /// The root config file.
    pub file: PathBuf,
    /// Every included file, in load order.
    pub included_files: Vec<PathBuf>,
    /// Includes that can't be read: those under `vendor/` that don't exist yet
    /// (`composer install` hasn't run), so a project can be checked without its dev
    /// dependencies, and `.php` configs, because running them would run the project's code.
    pub skipped_includes: Vec<PathBuf>,
    pub level: Option<Level>,
    pub paths: Vec<PathBuf>,
    pub exclude_paths: ExcludePaths,
    pub ignore_errors: Vec<IgnoreError>,
    /// `typeAliases`: name → PHPStan type string.
    pub type_aliases: IndexMap<String, String>,
    pub bootstrap_files: Vec<PathBuf>,
    pub scan_files: Vec<PathBuf>,
    pub scan_directories: Vec<PathBuf>,
    pub treat_phpdoc_types_as_certain: Option<bool>,
    /// `phpVersion`, as `PHP_VERSION_ID`-style numbers: `(min, max)`, the same for a single one.
    pub php_version: Option<(u32, u32)>,
    /// `parameters.stirphp`.
    pub stirphp: StirphpParameters,
    /// The full merged `parameters` mapping (after path resolution and expansion).
    pub parameters: IndexMap<Key, Value>,
    /// The whole merged config (all sections, `includes` removed).
    pub raw: Value,
}

/// Parameters whose values are lists of paths.
const PATH_LIST_PARAMS: &[&str] = &[
    "paths",
    "bootstrapFiles",
    "scanFiles",
    "scanDirectories",
    "stubFiles",
    "excludes_analyse",
    "excludePaths",
    "autoload_files",
    "autoload_directories",
];
/// Parameters whose values are single paths.
const PATH_SCALAR_PARAMS: &[&str] = &["tmpDir", "bootstrap"];
/// Keys of `parameters.stirphp` whose values are paths.
const STIRPHP_PATH_PARAMS: &[&str] = &["srcDir", "composer", "baseline", "rules"];

impl PhpstanConfig {
    /// Loads `path` with default options (`%currentWorkingDirectory%` = process cwd).
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        Self::load_with(path, &LoadOptions::default())
    }

    pub fn load_with(path: impl AsRef<Path>, opts: &LoadOptions) -> Result<Self, ConfigError> {
        let path = path.as_ref();
        let mut opts = opts.clone();
        if !opts.params.contains_key("currentWorkingDirectory")
            && let Ok(cwd) = std::env::current_dir()
        {
            opts.params.insert(
                "currentWorkingDirectory".into(),
                cwd.to_string_lossy().into_owned(),
            );
        }
        let mut loader = Loader {
            opts: &opts,
            stack: Vec::new(),
            included: Vec::new(),
            skipped: Vec::new(),
        };
        let merged = loader.load_file(path)?;
        let included_files = loader.included;
        let skipped_includes = loader.skipped;

        // Expand %params% across the whole config.
        let params_for_expansion = merged
            .get("parameters")
            .and_then(Value::as_mapping)
            .cloned()
            .unwrap_or_default();
        let raw = expand_value(merged, &opts.params, &params_for_expansion);
        let parameters = raw
            .get("parameters")
            .and_then(Value::entries)
            .map(|e| e.into_iter().map(|(k, v)| (k, v.clone())).collect())
            .unwrap_or_default();

        let mut cfg = PhpstanConfig {
            file: path.to_path_buf(),
            included_files,
            skipped_includes,
            level: None,
            paths: Vec::new(),
            exclude_paths: ExcludePaths::default(),
            ignore_errors: Vec::new(),
            type_aliases: IndexMap::new(),
            bootstrap_files: Vec::new(),
            scan_files: Vec::new(),
            scan_directories: Vec::new(),
            treat_phpdoc_types_as_certain: None,
            php_version: None,
            stirphp: StirphpParameters::default(),
            parameters,
            raw: Value::Null,
        };
        cfg.extract()?;
        cfg.raw = raw;
        Ok(cfg)
    }

    fn invalid(&self, key: &str, message: impl Into<String>) -> ConfigError {
        ConfigError::Invalid {
            path: self.file.clone(),
            key: key.into(),
            message: message.into(),
        }
    }

    fn param(&self, key: &str) -> Option<&Value> {
        self.parameters
            .get(&Key::from(key))
            .filter(|v| !v.is_null())
    }

    fn path_list(&self, key: &str) -> Result<Vec<PathBuf>, ConfigError> {
        match self.param(key) {
            None => Ok(Vec::new()),
            Some(v) => path_list(v).ok_or_else(|| self.invalid(key, "expected a list of paths")),
        }
    }

    /// `phpVersion: 80100` or `phpVersion: {min: 80100, max: 80399}`.
    fn php_version(&self) -> Result<Option<(u32, u32)>, ConfigError> {
        let number = |v: &Value| match v {
            Value::Int(i) => u32::try_from(*i).ok(),
            Value::String(s) => s.parse().ok(),
            _ => None,
        };
        let wrong = || self.invalid("phpVersion", "expected a version id, or min and max");
        match self.param("phpVersion") {
            None => Ok(None),
            Some(Value::Mapping(m)) => {
                let bound = |name: &str| m.get(&Key::from(name)).and_then(number);
                match (bound("min"), bound("max")) {
                    (Some(min), Some(max)) => Ok(Some((min, max))),
                    _ => Err(wrong()),
                }
            }
            Some(v) => number(v).map(|id| Some((id, id))).ok_or_else(wrong),
        }
    }

    fn extract(&mut self) -> Result<(), ConfigError> {
        self.level = match self.param("level") {
            None => None,
            Some(Value::Int(i)) if (0..=10).contains(i) => Some(Level::Level(*i as u8)),
            Some(Value::String(s)) if s == "max" => Some(Level::Max),
            Some(Value::String(s)) => match s.parse::<u8>() {
                Ok(i) if i <= 10 => Some(Level::Level(i)),
                _ => return Err(self.invalid("level", format!("expected 0-10 or max, got {s:?}"))),
            },
            Some(v) => return Err(self.invalid("level", format!("expected 0-10 or max, got {v}"))),
        };
        self.paths = self.path_list("paths")?;
        self.bootstrap_files = self.path_list("bootstrapFiles")?;
        if let Some(Value::String(b)) = self.param("bootstrap") {
            self.bootstrap_files.push(PathBuf::from(b));
        }
        self.scan_files = self.path_list("scanFiles")?;
        self.scan_directories = self.path_list("scanDirectories")?;
        self.php_version = self.php_version()?;

        let mut ex = ExcludePaths::default();
        match self.param("excludePaths") {
            None => {}
            Some(v @ Value::Sequence(_)) => {
                ex.analyse_and_scan =
                    path_list(v).ok_or_else(|| self.invalid("excludePaths", "expected paths"))?;
            }
            Some(Value::Mapping(m)) => {
                for (k, v) in m {
                    let list = if v.is_null() {
                        Vec::new()
                    } else {
                        path_list(v)
                            .ok_or_else(|| self.invalid("excludePaths", "expected paths"))?
                    };
                    match k.as_str() {
                        Some("analyse") => ex.analyse.extend(list),
                        Some("analyseAndScan") => ex.analyse_and_scan.extend(list),
                        _ => {
                            return Err(self.invalid(
                                "excludePaths",
                                format!("unknown key {k}, expected analyse or analyseAndScan"),
                            ));
                        }
                    }
                }
            }
            Some(_) => return Err(self.invalid("excludePaths", "expected a list or mapping")),
        }
        ex.analyse.extend(self.path_list("excludes_analyse")?);
        self.exclude_paths = ex;

        if let Some(v) = self.param("ignoreErrors") {
            let items = v
                .values()
                .ok_or_else(|| self.invalid("ignoreErrors", "expected a list"))?;
            let mut out = Vec::new();
            for item in items {
                out.push(self.ignore_error(item)?);
            }
            self.ignore_errors = out;
        }

        if let Some(v) = self.param("typeAliases") {
            let m = v
                .as_mapping()
                .ok_or_else(|| self.invalid("typeAliases", "expected a mapping"))?;
            let mut aliases = IndexMap::new();
            for (k, v) in m {
                let Some(s) = v.as_str() else {
                    return Err(self.invalid("typeAliases", format!("alias {k} must be a string")));
                };
                aliases.insert(key_string(k), s.to_string());
            }
            self.type_aliases = aliases;
        }

        self.treat_phpdoc_types_as_certain = match self.param("treatPhpDocTypesAsCertain") {
            None => None,
            Some(Value::Bool(b)) => Some(*b),
            Some(_) => return Err(self.invalid("treatPhpDocTypesAsCertain", "expected a bool")),
        };
        self.stirphp = self.stirphp_parameters()?;
        Ok(())
    }

    fn stirphp_parameters(&self) -> Result<StirphpParameters, ConfigError> {
        const KEY: &str = "stirphp";
        let mut out = StirphpParameters::default();
        let Some(section) = self.param(KEY) else {
            return Ok(out);
        };
        let Some(entries) = section.entries() else {
            return Err(self.invalid(KEY, "expected a mapping"));
        };
        let path = |v: &Value, what: &str| -> Result<PathBuf, ConfigError> {
            v.as_str()
                .map(PathBuf::from)
                .ok_or_else(|| self.invalid(KEY, format!("`{what}` must be a path")))
        };
        let strings = |v: &Value, what: &str| -> Result<Vec<String>, ConfigError> {
            v.values()
                .and_then(|vs| vs.iter().map(|v| v.as_str().map(str::to_string)).collect())
                .ok_or_else(|| self.invalid(KEY, format!("`{what}` must be a list of strings")))
        };
        for (k, v) in entries {
            let k = key_string(&k);
            match k.as_str() {
                "srcDir" => out.src_dir = Some(path(v, &k)?),
                "include" => out.include = Some(strings(v, &k)?),
                "exclude" => out.exclude = Some(strings(v, &k)?),
                "composer" => out.composer = Some(path(v, &k)?),
                "baseline" => out.baseline = Some(path(v, &k)?),
                "rules" => out.rules = strings(v, &k)?.into_iter().map(PathBuf::from).collect(),
                other => return Err(self.invalid(KEY, format!("unknown key `{other}`"))),
            }
        }
        Ok(out)
    }

    fn ignore_error(&self, item: &Value) -> Result<IgnoreError, ConfigError> {
        const KEY: &str = "ignoreErrors";
        let mut e = IgnoreError::default();
        match item {
            Value::String(s) => e.messages.push(IgnoreMessage::Regex(s.clone())),
            Value::Mapping(m) => {
                let string = |v: &Value, what: &str| -> Result<String, ConfigError> {
                    v.as_str()
                        .map(str::to_string)
                        .ok_or_else(|| self.invalid(KEY, format!("`{what}` must be a string")))
                };
                let strings = |v: &Value, what: &str| -> Result<Vec<String>, ConfigError> {
                    v.values()
                        .and_then(|vs| vs.iter().map(|v| v.as_str().map(str::to_string)).collect())
                        .ok_or_else(|| {
                            self.invalid(KEY, format!("`{what}` must be a list of strings"))
                        })
                };
                for (k, v) in m {
                    let k = key_string(k);
                    match k.as_str() {
                        "message" => e.messages.push(IgnoreMessage::Regex(string(v, &k)?)),
                        "messages" => e
                            .messages
                            .extend(strings(v, &k)?.into_iter().map(IgnoreMessage::Regex)),
                        "rawMessage" => e.messages.push(IgnoreMessage::Raw(string(v, &k)?)),
                        "rawMessages" => e
                            .messages
                            .extend(strings(v, &k)?.into_iter().map(IgnoreMessage::Raw)),
                        "identifier" => e.identifiers.push(string(v, &k)?),
                        "identifiers" => e.identifiers.extend(strings(v, &k)?),
                        "path" => e.paths.push(PathBuf::from(string(v, &k)?)),
                        "paths" => e
                            .paths
                            .extend(strings(v, &k)?.into_iter().map(PathBuf::from)),
                        "count" => {
                            e.count =
                                Some(v.as_int().and_then(|i| u32::try_from(i).ok()).ok_or_else(
                                    || self.invalid(KEY, "`count` must be a non-negative int"),
                                )?)
                        }
                        "reportUnmatched" => {
                            e.report_unmatched = Some(v.as_bool().ok_or_else(|| {
                                self.invalid(KEY, "`reportUnmatched` must be a bool")
                            })?)
                        }
                        other => {
                            return Err(self.invalid(KEY, format!("unknown key `{other}`")));
                        }
                    }
                }
                if e.messages.is_empty() && e.identifiers.is_empty() && e.paths.is_empty() {
                    return Err(self.invalid(KEY, "entry needs a message, an identifier or a path"));
                }
            }
            _ => return Err(self.invalid(KEY, format!("unexpected entry {item}"))),
        }
        Ok(e)
    }
}

fn key_string(k: &Key) -> String {
    match k {
        Key::Int(i) => i.to_string(),
        Key::Str(s) => s.clone(),
    }
}

fn path_list(v: &Value) -> Option<Vec<PathBuf>> {
    v.values()?
        .into_iter()
        .map(|v| v.as_str().map(PathBuf::from))
        .collect()
}

fn is_in_vendor(path: &Path) -> bool {
    path.components()
        .any(|c| matches!(c, Component::Normal(n) if n == "vendor"))
}

struct Loader<'a> {
    opts: &'a LoadOptions,
    stack: Vec<PathBuf>,
    included: Vec<PathBuf>,
    skipped: Vec<PathBuf>,
}

impl Loader<'_> {
    fn load_file(&mut self, path: &Path) -> Result<Value, ConfigError> {
        let io = |source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        };
        let canon = std::fs::canonicalize(path).map_err(io)?;
        if let Some(i) = self.stack.iter().position(|p| *p == canon) {
            let mut chain = self.stack[i..].to_vec();
            chain.push(canon);
            return Err(ConfigError::IncludeCycle { chain });
        }
        let src = std::fs::read_to_string(path).map_err(io)?;
        let value = crate::parse(&src).map_err(|source| ConfigError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
        let invalid = |key: &str, message: &str| ConfigError::Invalid {
            path: path.to_path_buf(),
            key: key.into(),
            message: message.into(),
        };
        let mut data: IndexMap<Key, Value> = match value {
            Value::Null => IndexMap::new(),
            Value::Mapping(m) => m,
            Value::Sequence(s) if s.is_empty() => IndexMap::new(),
            _ => return Err(invalid("", "config root must be a mapping")),
        };
        let dir = canon.parent().map(Path::to_path_buf).unwrap_or_default();

        let is_root = self.stack.is_empty();
        self.stack.push(canon);
        let mut res = Value::Mapping(IndexMap::new());
        if is_root {
            let extensions = self.opts.extensions.clone();
            for path in extensions.iter().filter(|p| p.is_file()) {
                self.included.push(path.clone());
                let sub = self.load_file(path)?;
                res = merge(sub, res);
            }
        }
        if let Some(includes) = data.shift_remove(&Key::from("includes"))
            && !includes.is_null()
        {
            let list = includes
                .values()
                .ok_or_else(|| invalid("includes", "expected a list of files"))?;
            for inc in list {
                let inc = inc
                    .as_str()
                    .ok_or_else(|| invalid("includes", "expected a list of files"))?;
                let inc_path = resolve_path(&expand_known(inc, &self.opts.params), &dir);
                let is_php = inc_path.extension().is_some_and(|e| e == "php");
                if is_php || (!inc_path.exists() && is_in_vendor(&inc_path)) {
                    self.skipped.push(inc_path);
                    continue;
                }
                self.included.push(inc_path.clone());
                let sub = self.load_file(&inc_path)?;
                res = merge(sub, res);
            }
        }
        self.stack.pop();

        if let Some(params) = data.get_mut(&Key::from("parameters"))
            && let Value::Mapping(m) = params
        {
            for (k, v) in m.iter_mut() {
                let Some(name) = k.as_str() else { continue };
                let name = name.strip_suffix('!').unwrap_or(name);
                if PATH_LIST_PARAMS.contains(&name) || PATH_SCALAR_PARAMS.contains(&name) {
                    resolve_strings(v, &dir, &self.opts.params);
                } else if name == "stirphp"
                    && let Value::Mapping(section) = v
                {
                    for (key, value) in section.iter_mut() {
                        let key = key.as_str().unwrap_or_default();
                        if STIRPHP_PATH_PARAMS.contains(&key.strip_suffix('!').unwrap_or(key)) {
                            resolve_strings(value, &dir, &self.opts.params);
                        }
                    }
                } else if name == "ignoreErrors"
                    && let Some(items) = values_mut(v)
                {
                    for item in items {
                        if let Value::Mapping(entry) = item {
                            for (ek, ev) in entry.iter_mut() {
                                if matches!(ek.as_str(), Some("path" | "paths")) {
                                    resolve_strings(ev, &dir, &self.opts.params);
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(merge(Value::Mapping(data), res))
    }
}

fn values_mut(v: &mut Value) -> Option<Vec<&mut Value>> {
    match v {
        Value::Sequence(s) => Some(s.iter_mut().collect()),
        Value::Mapping(m) => Some(m.values_mut().collect()),
        _ => None,
    }
}

fn resolve_strings(v: &mut Value, dir: &Path, params: &IndexMap<String, String>) {
    match v {
        Value::String(s) => {
            let expanded = expand_known(s, params);
            *s = if expanded.starts_with('%') || expanded.starts_with('*') {
                expanded
            } else {
                resolve_path(&expanded, dir).to_string_lossy().into_owned()
            };
        }
        Value::Sequence(items) => items
            .iter_mut()
            .for_each(|i| resolve_strings(i, dir, params)),
        Value::Mapping(m) => m.values_mut().for_each(|i| resolve_strings(i, dir, params)),
        _ => {}
    }
}

/// Joins a relative path onto `dir` and normalises `.`/`..` lexically.
fn resolve_path(p: &str, dir: &Path) -> PathBuf {
    let path = Path::new(p);
    let joined = if path.is_absolute() || p.contains("://") {
        path.to_path_buf()
    } else {
        dir.join(path)
    };
    let mut out = PathBuf::new();
    for c in joined.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            c => out.push(c.as_os_str()),
        }
    }
    out
}

fn strip_bangs(v: Value) -> Value {
    match v {
        Value::Mapping(m) => {
            let entries = m
                .into_iter()
                .map(|(k, v)| (strip_bang_key(k), strip_bangs(v)))
                .collect();
            Value::from_entries(entries, true)
        }
        Value::Sequence(s) => Value::Sequence(s.into_iter().map(strip_bangs).collect()),
        v => v,
    }
}

fn strip_bang_key(k: Key) -> Key {
    match k {
        Key::Str(s) if s.ends_with('!') => Key::from_php_string(&s[..s.len() - 1]),
        k => k,
    }
}

fn into_entries(v: Value) -> IndexMap<Key, Value> {
    match v {
        Value::Sequence(s) => s
            .into_iter()
            .enumerate()
            .map(|(i, v)| (Key::Int(i as i64), v))
            .collect(),
        Value::Mapping(m) => m,
        _ => IndexMap::new(),
    }
}

/// Nette `Helpers::merge($left, $base)`: `left` (later) takes priority over `base`.
pub fn merge(left: Value, base: Value) -> Value {
    match (left, base) {
        (left, base) if left.is_array() && base.is_array() => {
            let mapping_hint =
                matches!(left, Value::Mapping(_)) || matches!(base, Value::Mapping(_));
            let mut out = into_entries(base);
            let mut next = out
                .keys()
                .filter_map(Key::as_int)
                .max()
                .map_or(0, |m| m.saturating_add(1));
            for (k, v) in into_entries(left) {
                match k {
                    Key::Int(_) => {
                        out.insert(Key::Int(next), strip_bangs(v));
                        next = next.saturating_add(1);
                    }
                    Key::Str(ref s) if s.ends_with('!') => {
                        out.insert(strip_bang_key(k), strip_bangs(v));
                    }
                    k => {
                        let merged = match out.shift_remove_full(&k) {
                            Some((idx, _, existing)) => {
                                let m = merge(v, existing);
                                out.shift_insert(idx, k, m);
                                continue;
                            }
                            None => strip_bangs(v),
                        };
                        out.insert(k, merged);
                    }
                }
            }
            Value::from_entries(out, mapping_hint)
        }
        (Value::Null, base) if base.is_array() => base,
        (left, _) => strip_bangs(left),
    }
}

fn is_param_name(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'-')
}

/// Expands `%name%` from `known` only, leaving everything else (incl. `%%`) untouched.
fn expand_known(s: &str, known: &IndexMap<String, String>) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find('%') {
            Some(j) if is_param_name(&after[..j]) && known.contains_key(&after[..j]) => {
                out.push_str(&known[&after[..j]]);
                rest = &after[j + 1..];
            }
            Some(0) => {
                out.push_str("%%");
                rest = &after[1..];
            }
            _ => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn lookup_param<'a>(name: &str, params: &'a IndexMap<Key, Value>) -> Option<&'a Value> {
    let mut parts = name.split('.');
    let mut cur = params.get(&Key::from(parts.next()?))?;
    for p in parts {
        cur = cur.get(p)?;
    }
    Some(cur)
}

fn scalar_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Int(i) => Some(i.to_string()),
        Value::Float(f) => Some(f.to_string()),
        Value::Bool(true) => Some("1".into()),
        Value::Bool(false) | Value::Null => Some(String::new()),
        _ => None,
    }
}

fn expand_str(
    s: &str,
    known: &IndexMap<String, String>,
    params: &IndexMap<Key, Value>,
    depth: u32,
) -> Value {
    // Whole-string reference to a non-scalar parameter substitutes the value itself.
    if let Some(name) = s.strip_prefix('%').and_then(|r| r.strip_suffix('%'))
        && is_param_name(name)
        && !known.contains_key(name)
        && let Some(v) = lookup_param(name, params)
        && !matches!(v, Value::String(_))
        && depth < 8
    {
        return expand_value_inner(v.clone(), known, params, depth + 1);
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find('%') {
            Some(0) => {
                out.push('%');
                rest = &after[1..];
            }
            Some(j) if is_param_name(&after[..j]) => {
                let name = &after[..j];
                if let Some(v) = known.get(name) {
                    out.push_str(v);
                } else if let Some(v) = lookup_param(name, params).and_then(scalar_string)
                    && depth < 8
                {
                    match expand_str(&v, known, params, depth + 1) {
                        Value::String(s) => out.push_str(&s),
                        other => out.push_str(&other.to_string()),
                    }
                } else {
                    out.push('%');
                    out.push_str(name);
                    out.push('%');
                }
                rest = &after[j + 1..];
            }
            _ => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    Value::String(out)
}

fn expand_value_inner(
    v: Value,
    known: &IndexMap<String, String>,
    params: &IndexMap<Key, Value>,
    depth: u32,
) -> Value {
    match v {
        Value::String(s) => expand_str(&s, known, params, depth),
        Value::Sequence(items) => Value::Sequence(
            items
                .into_iter()
                .map(|v| expand_value_inner(v, known, params, depth))
                .collect(),
        ),
        Value::Mapping(m) => Value::Mapping(
            m.into_iter()
                .map(|(k, v)| (k, expand_value_inner(v, known, params, depth)))
                .collect(),
        ),
        Value::Entity(mut e) => {
            e.attributes = e
                .attributes
                .into_iter()
                .map(|(k, v)| (k, expand_value_inner(v, known, params, depth)))
                .collect();
            Value::Entity(e)
        }
        v => v,
    }
}

fn expand_value(
    v: Value,
    known: &IndexMap<String, String>,
    params: &IndexMap<Key, Value>,
) -> Value {
    expand_value_inner(v, known, params, 0)
}
