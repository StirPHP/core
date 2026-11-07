//! Building the salsa database for a project: project sources, builtin stubs and
//! vendor PHP files reachable through Composer autoload rules. Composer itself is never run.
//!
//! Shared by `stirphp check` and the language server.

#[cfg(feature = "tokio")]
use crate::config::disk;
use crate::{Autoload, Composer, Project};
use aho_corasick::AhoCorasick;
use anyhow::Context as _;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use stir_composer::{Exclusions, InstallState};
use stir_db::{
    FileOrigin, IgnoreRule, IgnoreRules, PhpVersion, PhpVersions, RootDatabase, ScriptFile,
    Settings, SourceFile, Workspace,
};
use stir_vfs::{Fs, Kind};

/// Options for [`load_workspace`].
#[derive(Debug, Clone, Copy)]
pub struct LoadOptions {
    /// Load vendor PHP files reachable through Composer autoload rules.
    pub vendor: bool,
    /// The rule level, over the one in the PHPStan config.
    pub level: Option<u8>,
    /// Apply the baseline in `parameters.stirphp.baseline`: generating one starts without.
    pub baseline: bool,
    /// Let the scripts in `parameters.stirphp.rules` run. Only for someone who chose to.
    pub rules: bool,
}

impl Default for LoadOptions {
    fn default() -> Self {
        Self {
            vendor: true,
            level: None,
            baseline: true,
            rules: false,
        }
    }
}

/// A loaded workspace: the database, the workspace input and the project's own files.
pub struct Loaded {
    pub db: RootDatabase,
    pub workspace: Workspace,
    /// Project source files (origin [`FileOrigin::Source`]), sorted by path.
    pub sources: Vec<SourceFile>,
    pub stub_count: usize,
    pub vendor_count: usize,
    /// What Composer's files say, if the project has any.
    pub composer: Option<Composer>,
    /// Where dependencies live: files there are never the project's own.
    pub vendor_dir: PathBuf,
    /// What a person should be told about the project, once.
    pub notices: Vec<String>,
    /// The project's scripts, whether they're allowed to run or not.
    pub rules: Vec<ScriptFile>,
    /// The frameworks there's support for in the workspace, by name.
    pub frameworks: Vec<&'static str>,
}

/// Builds the database for `project`: discovered sources, all builtin stubs and
/// (unless disabled) vendor files from the root `composer.json` autoload rules.
#[cfg(feature = "tokio")]
pub async fn load_workspace(project: &Project, options: LoadOptions) -> anyhow::Result<Loaded> {
    let start = std::time::Instant::now();
    let project = project.clone();
    let loaded =
        tokio::task::spawn_blocking(move || load_workspace_from(&disk(), &project, options))
            .await??;
    tracing::debug!(
        sources = loaded.sources.len(),
        stubs = loaded.stub_count,
        vendor = loaded.vendor_count,
        elapsed = ?start.elapsed(),
        "workspace loaded"
    );
    Ok(loaded)
}

/// [`load_workspace`], reading from `fs`.
pub fn load_workspace_from(
    fs: &Arc<dyn Fs>,
    project: &Project,
    options: LoadOptions,
) -> anyhow::Result<Loaded> {
    let mut notices = Vec::new();
    // a `composer.json` that's being edited shouldn't take the rest of the project down
    let composer = match crate::composer::load_composer_in(fs.as_ref(), project) {
        Ok(composer) => composer,
        Err(error) => {
            notices.push(error.to_string());
            None
        }
    };
    let vendor_dir = composer
        .as_ref()
        .map_or_else(|| project.root.join("vendor"), |c| c.vendor_dir.clone());
    notices.extend(composer.as_ref().and_then(Composer::install_notice));

    let discovered =
        crate::discover::discover_in(fs.as_ref(), &with_vendor_excluded(project, &vendor_dir))?;
    let mut source_paths: Vec<PathBuf> = discovered.into_iter().map(|f| f.path).collect();
    if let Some(composer) = &composer {
        source_paths.extend(sources_elsewhere(fs.as_ref(), project, composer));
    }

    let read = fs.read_all(&source_paths);
    let source_texts: Vec<(PathBuf, Arc<str>)> = source_paths
        .into_iter()
        .zip(read)
        .map(|(path, text)| {
            let text = text.with_context(|| format!("failed to read `{}`", path.display()))?;
            anyhow::Ok((path, Arc::from(stir_vfs::lossy(text))))
        })
        .collect::<anyhow::Result<_>>()?;

    let vendor_texts = if options.vendor {
        let mut autoload = composer
            .as_ref()
            .map(Composer::autoload)
            .unwrap_or_default();
        // a `vendor/` nothing describes: all of it
        if fs.is_dir(&vendor_dir) && !fs.is_file(&vendor_dir.join("composer/installed.json")) {
            autoload.classmap_paths.push(vendor_dir.clone());
        }
        let paths = vendor_paths(fs.as_ref(), autoload, &source_texts);
        read_vendor(fs.as_ref(), paths)
    } else {
        Vec::new()
    };

    let rules = read_rules_from(fs.as_ref(), project);
    let frameworks =
        crate::frameworks::frameworks_of(fs.as_ref(), project, composer.as_ref(), &source_texts);

    let db = RootDatabase::default();
    let mut files = Vec::new();
    let mut sources = Vec::new();
    for (path, text) in source_texts {
        let file = SourceFile::new(&db, path, text.to_string(), FileOrigin::Source);
        sources.push(file);
        files.push(file);
    }

    let builtins = builtin_files(&db);
    let stub_count = builtins
        .iter()
        .filter(|f| f.path(&db).to_string_lossy().starts_with("stubs://"))
        .count();
    files.extend(builtins);
    let (bundled, framework_stubs) = crate::frameworks::bundled_inputs(&db, &frameworks);
    files.extend(framework_stubs);

    if fs.is_dir(&vendor_dir.join("phpstan/phpstan")) {
        files.push(SourceFile::new(
            &db,
            PathBuf::from("stubs://phpstan/helpers.php"),
            stir_stubs::PHPSTAN_HELPERS.to_owned(),
            FileOrigin::Stub,
        ));
    }

    let vendor_count = vendor_texts.len();
    for (path, text) in vendor_texts {
        files.push(SourceFile::new(&db, path, text, FileOrigin::Vendor));
    }

    let config = load_neon_config(fs, project, composer.as_ref())?;
    if let Some(config) = &config {
        // laid over the stubs of our own, and so after them
        for (path, text) in read_vendor(fs.as_ref(), config.stub_files.clone()) {
            files.push(SourceFile::new(
                &db,
                stir_db::overlay_path(&path),
                text,
                FileOrigin::Stub,
            ));
        }
        let own: BTreeSet<PathBuf> = sources
            .iter()
            .filter_map(|f| fs.canonicalize(f.path(&db)).ok())
            .collect();
        for (path, text) in read_vendor(fs.as_ref(), scanned_paths(fs.as_ref(), config, &own)) {
            files.push(SourceFile::new(&db, path, text, FileOrigin::Vendor));
        }
    }
    // WordPress's declarations, where only its stubs are installed: they're no package's autoload
    if options.vendor && frameworks.iter().any(|f| f.name == "wordpress") {
        let stubs = vendor_dir.join("php-stubs/wordpress-stubs/wordpress-stubs.php");
        let known = files.iter().any(|f| f.path(&db) == &stubs);
        if !known && let Ok(text) = fs.read_to_string(&stubs) {
            files.push(SourceFile::new(&db, stubs, text, FileOrigin::Vendor));
        }
    }
    let mut data_files = Vec::new();
    if frameworks.iter().any(|f| f.name == "symfony") {
        let configured = config
            .as_ref()
            .and_then(|c| c.param("symfony"))
            .and_then(|s| s.get("containerXmlPath"))
            .and_then(|p| p.as_str())
            .map(|p| {
                let base = project.config_file.as_ref().and_then(|f| f.parent());
                base.unwrap_or(&project.root).join(p)
            });
        let found = crate::frameworks::container_xml(fs.as_ref(), &project.root, configured);
        if let Some((path, text)) = found {
            tracing::info!(path = %path.display(), "reading Symfony's container");
            data_files.push(stir_db::DataFile::new(&db, path, text));
        }
    }
    let (mut settings, ignore_notice) =
        settings_from(fs, project, config, composer.as_ref(), options.baseline);
    notices.extend(ignore_notice);
    if let Some(level) = options.level {
        settings.php_level = level;
    }
    let rules: Vec<ScriptFile> = rules
        .into_iter()
        .map(|(path, text)| ScriptFile::new(&db, path, text))
        .collect();
    let allowed = if options.rules {
        rules.clone()
    } else {
        Vec::new()
    };
    for rule in &allowed {
        tracing::info!(path = %rule.path(&db).display(), "running rules");
    }
    let frameworks: Vec<&'static str> = frameworks.iter().map(|f| f.name).collect();
    if !frameworks.is_empty() {
        tracing::info!(?frameworks, "framework support");
    }
    let workspace = Workspace::builder(files, settings)
        .scripts(allowed)
        .bundled_scripts(bundled)
        .data_files(data_files)
        .new(&db);
    Ok(Loaded {
        db,
        workspace,
        sources,
        stub_count,
        vendor_count,
        composer,
        vendor_dir,
        notices,
        rules,
        frameworks,
    })
}

/// The scripts of `parameters.stirphp.rules` and their texts, in order. Those that can't be read
/// are kept, for what uses them to say so at the script.
#[cfg(feature = "tokio")]
pub async fn read_rules(project: &Project) -> Vec<(PathBuf, Result<String, String>)> {
    let project = project.clone();
    tokio::task::spawn_blocking(move || read_rules_from(&stir_vfs::Disk, &project))
        .await
        .unwrap_or_default()
}

/// [`read_rules`], from `fs`.
pub fn read_rules_from(fs: &dyn Fs, project: &Project) -> Vec<(PathBuf, Result<String, String>)> {
    let mut seen = BTreeSet::new();
    let mut scripts = Vec::new();
    for path in &project.config.rules {
        let path = stir_composer::normalize(&project.root.join(path));
        if seen.insert(path.clone()) {
            let text = fs
                .read_to_string(&path)
                .map_err(|e| format!("it can't be read: {e}"));
            scripts.push((path, text));
        }
    }
    scripts
}

/// `project`, with a custom `vendor-dir` left out of its sources like `vendor/` is.
fn with_vendor_excluded(project: &Project, vendor_dir: &Path) -> Project {
    let mut project = project.clone();
    if let Ok(rel) = vendor_dir.strip_prefix(&project.root) {
        let glob = format!("{}/**", globset::escape(&rel.to_string_lossy()));
        if !project.config.exclude.contains(&glob) {
            project.config.exclude.push(glob);
        }
    }
    project
}

/// What the root package's autoload rules name that walking the project folder doesn't find:
/// directories outside it, and files listed by name whatever their extension. With a source
/// directory set in the config, that's what the sources are.
fn sources_elsewhere(fs: &dyn Fs, project: &Project, composer: &Composer) -> Vec<PathBuf> {
    if project.config.src_dir != Path::new(".") {
        return Vec::new();
    }
    let root = &project.root;
    let autoload = &composer.root_autoload;
    let vendor_dir = &composer.vendor_dir;
    let is_outside = |p: &Path| !p.starts_with(root);
    let mut found = BTreeSet::new();
    let mut seen = BTreeSet::new();
    for (_, dir) in autoload.psr4.iter().chain(&autoload.psr0) {
        if is_outside(dir) {
            walk(
                fs,
                dir,
                Extensions::Psr,
                None,
                &mut seen,
                &mut found,
                &mut Vec::new(),
            );
        }
    }
    for path in &autoload.classmap_paths {
        if is_outside(path) {
            walk(
                fs,
                path,
                Extensions::Classmap,
                None,
                &mut seen,
                &mut found,
                &mut Vec::new(),
            );
        }
    }
    for file in &autoload.files {
        let php = file.extension().is_some_and(|e| e == "php");
        if (is_outside(file) || !php) && fs.is_file(file) {
            found.insert(file.clone());
        }
    }
    found
        .into_iter()
        .filter(|p| !p.starts_with(vendor_dir))
        .collect()
}

/// The builtin PHP stubs (`stubs://...`).
pub fn builtin_files(db: &RootDatabase) -> Vec<SourceFile> {
    stir_stubs::stub_files()
        .map(|stub| {
            SourceFile::new(
                db,
                PathBuf::from(format!("stubs://{}", stub.path)),
                stub.text.to_owned(),
                FileOrigin::Stub,
            )
        })
        .collect()
}

/// Check settings: the rule level and `ignoreErrors` of the configuration, if there is one.
#[cfg(feature = "tokio")]
pub async fn load_settings(project: &Project) -> anyhow::Result<Settings> {
    let project = project.clone();
    tokio::task::spawn_blocking(move || {
        let fs = disk();
        let composer = crate::composer::load_composer_in(fs.as_ref(), &project)
            .ok()
            .flatten();
        let config = load_neon_config(&fs, &project, composer.as_ref())?;
        Ok(settings_from(&fs, &project, config, composer.as_ref(), true).0)
    })
    .await?
}

fn load_neon_config(
    fs: &Arc<dyn Fs>,
    project: &Project,
    composer: Option<&Composer>,
) -> anyhow::Result<Option<stir_neon::phpstan::PhpstanConfig>> {
    let Some(path) = project.config_file.clone() else {
        return Ok(None);
    };
    let root = &project.root;
    let extensions = composer.map(Composer::phpstan_includes).unwrap_or_default();
    let config = {
        let opts = stir_neon::phpstan::LoadOptions::new()
            .param("currentWorkingDirectory", root.to_string_lossy())
            .fs(fs.clone());
        let with_extensions = opts.clone().extensions(extensions);
        // an extension's config that can't be read isn't the project's problem
        stir_neon::phpstan::PhpstanConfig::load_with(&path, &with_extensions).or_else(|error| {
            tracing::warn!(%error, "ignoring the configs of PHPStan extensions");
            stir_neon::phpstan::PhpstanConfig::load_with(&path, &opts)
        })
    }
    .context("failed to load the configuration")?;
    for skipped in config.skipped_includes.iter().filter(|p| !is_php(p)) {
        tracing::warn!("skipping include {}: not installed", skipped.display());
    }
    Ok(Some(config))
}

/// The rules in `ignoreErrors` entries, which never match on messages. An entry with a message
/// applies by its identifier and path alone, if it has both; otherwise it can't apply, and it's
/// counted in what's returned.
pub fn ignore_rules(entries: &[stir_neon::phpstan::IgnoreError]) -> (Vec<IgnoreRule>, usize) {
    let mut rules = Vec::new();
    let mut skipped = 0;
    for e in entries {
        let has_message = !e.messages.is_empty();
        if has_message && (e.identifiers.is_empty() || e.paths.is_empty()) {
            skipped += 1;
            continue;
        }
        // PHPStan makes an entry of each message and each identifier, each with the count
        let count = e
            .count
            .map(|n| n.saturating_mul(e.messages.len().max(1) as u32));
        let rule = |identifiers: Vec<String>| IgnoreRule {
            identifiers,
            paths: e.paths.clone(),
            count,
            report_unmatched: e.report_unmatched.unwrap_or(true),
        };
        if e.identifiers.is_empty() {
            rules.push(rule(Vec::new()));
        }
        rules.extend(e.identifiers.iter().map(|i| rule(vec![i.clone()])));
    }
    (rules, skipped)
}

/// The rules of the baseline at `path`, and what's wrong with it if it can't be applied.
fn baseline_rules(
    fs: &Arc<dyn Fs>,
    path: PathBuf,
    root: &Path,
) -> (Vec<IgnoreRule>, usize, Vec<String>) {
    if path.extension().is_some_and(|e| e == "php") {
        return (Vec::new(), 0, vec![display(&path, root)]);
    }
    let opts = stir_neon::phpstan::LoadOptions::new()
        .param("currentWorkingDirectory", root.to_string_lossy())
        .fs(fs.clone());
    match stir_neon::phpstan::PhpstanConfig::load_with(&path, &opts) {
        Ok(config) => {
            let (rules, skipped) = ignore_rules(&config.ignore_errors);
            let php = config.skipped_includes.iter().filter(|p| is_php(p));
            let php: Vec<String> = php.map(|p| display(p, root)).collect();
            (rules, skipped, php)
        }
        Err(error) => {
            tracing::warn!(%error, "ignoring the baseline");
            (Vec::new(), 0, Vec::new())
        }
    }
}

fn is_php(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "php")
}

fn display(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

/// The one notice about what can't be applied of `ignoreErrors` and baselines.
fn ignore_notice(skipped: usize, php: &[String]) -> Option<String> {
    let entries = (skipped > 0).then(|| {
        let (entry, them) = if skipped == 1 {
            ("entry", "it matches")
        } else {
            ("entries", "they match")
        };
        format!(
            "Skipped {skipped} `ignoreErrors` {entry}: {them} on a message without both an identifier and a path, and messages are never matched."
        )
    });
    let php = (!php.is_empty()).then(|| {
        format!(
            "Skipped PHP configs and baselines, because reading them would mean running them: {}.",
            php.join(", ")
        )
    });
    match (entries, php) {
        (Some(a), Some(b)) => Some(format!("{a} {b}")),
        (a, b) => a.or(b),
    }
}

/// The files PHPStan reads for their symbols (`bootstrapFiles`, `scanFiles` and
/// `scanDirectories`), other than the project's own.
fn scanned_paths(
    fs: &dyn Fs,
    config: &stir_neon::phpstan::PhpstanConfig,
    own: &BTreeSet<PathBuf>,
) -> Vec<PathBuf> {
    let files = config.bootstrap_files.iter().chain(&config.scan_files);
    let mut out = BTreeSet::new();
    for file in files.filter(|f| fs.is_file(f)) {
        out.insert(file.clone());
    }
    for dir in &config.scan_directories {
        for path in fs.all_below(dir) {
            if path.extension().is_some_and(|e| e == "php") {
                out.insert(path);
            }
        }
    }
    out.into_iter()
        .filter(|p| fs.canonicalize(p).map_or(true, |c| !own.contains(&c)))
        .collect()
}

/// PHPStan's `phpVersion`, then what Composer's files say, then the latest.
fn php_versions(
    config: Option<&stir_neon::phpstan::PhpstanConfig>,
    composer: Option<&Composer>,
) -> PhpVersions {
    if let Some((min, max)) = config.and_then(|c| c.php_version) {
        return PhpVersions {
            min: PhpVersion::from_id(min),
            max: PhpVersion::from_id(max),
        };
    }
    composer
        .and_then(|c| c.php_version)
        .map(PhpVersions::exactly)
        .unwrap_or_default()
}

fn settings_from(
    fs: &Arc<dyn Fs>,
    project: &Project,
    config: Option<stir_neon::phpstan::PhpstanConfig>,
    composer: Option<&Composer>,
    with_baseline: bool,
) -> (Settings, Option<String>) {
    let mut settings = Settings {
        php_version: php_versions(config.as_ref(), composer),
        ..Settings::default()
    };
    if let Some(composer) = composer {
        settings.declared_extensions = Some(composer.extensions.iter().cloned().collect());
        if composer.install != InstallState::Installed {
            settings.pending_install = Some(composer.own_namespaces());
        }
    }
    let mut skipped = 0;
    let mut php = Vec::new();
    if let Some(baseline) = project.config.baseline.clone().filter(|_| with_baseline) {
        let (rules, count, files) = baseline_rules(fs, baseline, &project.root);
        settings.baseline = IgnoreRules::new(rules);
        skipped += count;
        php.extend(files);
    }
    settings.deprecations = project.config.deprecations;
    let Some(config) = config else {
        return (settings, ignore_notice(skipped, &php));
    };
    settings.deprecations = project.config.deprecations || config.deprecation_rules;
    settings.treat_phpdoc_types_as_certain = config.treat_phpdoc_types_as_certain != Some(false);
    settings.php_paths = config.paths.clone();
    settings.php_excludes = config
        .exclude_paths
        .analyse
        .iter()
        .chain(&config.exclude_paths.analyse_and_scan)
        .cloned()
        .collect();
    if let Some(level) = config.level {
        settings.php_level = match level {
            stir_neon::phpstan::Level::Level(n) => n.min(stir_db::MAX_PHP_LEVEL),
            stir_neon::phpstan::Level::Max => stir_db::MAX_PHP_LEVEL,
        };
    }
    let (rules, count) = ignore_rules(&config.ignore_errors);
    settings.ignore_errors = IgnoreRules::new(rules);
    skipped += count;
    php.extend(
        config
            .skipped_includes
            .iter()
            .filter(|p| is_php(p))
            .map(|p| display(p, &project.root)),
    );
    (settings, ignore_notice(skipped, &php))
}

/// Collects every file reachable from the autoload rules, canonicalised and deduplicated,
/// excluding the project's own sources (`own`, with their text). What `exclude-from-classmap`
/// leaves out is skipped, unless autoloading would find it as a class the project names.
fn vendor_paths(fs: &dyn Fs, autoload: Autoload, own: &[(PathBuf, Arc<str>)]) -> Vec<PathBuf> {
    let exclusions = autoload.exclusions();
    let mut found = BTreeSet::new();
    let mut excluded = Vec::new();
    // the two look at different files, so each has to see a directory they share
    let mut seen_dirs = BTreeSet::new();
    for (_, dir) in autoload.psr4.iter().chain(&autoload.psr0) {
        walk(
            fs,
            dir,
            Extensions::Psr,
            exclusions.as_ref(),
            &mut seen_dirs,
            &mut found,
            &mut excluded,
        );
    }
    let mut seen_dirs = BTreeSet::new();
    for path in &autoload.classmap_paths {
        walk(
            fs,
            path,
            Extensions::Classmap,
            exclusions.as_ref(),
            &mut seen_dirs,
            &mut found,
            &mut excluded,
        );
    }
    // listed by name: whatever the extension, and PHP includes it however it's excluded
    for file in &autoload.files {
        if let Ok(path) = fs.canonicalize(file)
            && fs.is_file(&path)
        {
            found.insert(path);
        }
    }

    let own_paths: BTreeSet<&Path> = own.iter().map(|(p, _)| p.as_path()).collect();
    excluded.retain(|p| !own_paths.contains(p.as_path()));
    if !excluded.is_empty() {
        let classes: Vec<(String, &PathBuf)> = excluded
            .iter()
            .filter_map(|p| Some((autoload.class_of_file(p)?, p)))
            .collect();
        for path in named_in(&classes, own) {
            if let Ok(path) = fs.canonicalize(path) {
                found.insert(path);
            }
        }
    }

    let own: BTreeSet<PathBuf> = own
        .iter()
        .map(|(p, _)| fs.canonicalize(p).unwrap_or_else(|_| p.clone()))
        .collect();
    found.into_iter().filter(|p| !own.contains(p)).collect()
}

/// Those of `classes` whose names are in the text of one of the files `own`. All searched at once:
/// a project's sources can be large.
fn named_in<'a>(
    classes: &[(String, &'a PathBuf)],
    own: &[(PathBuf, Arc<str>)],
) -> Vec<&'a PathBuf> {
    if classes.is_empty() {
        return Vec::new();
    }
    let Ok(searcher) = AhoCorasick::new(classes.iter().map(|(class, _)| class)) else {
        return Vec::new();
    };
    let mut found = vec![false; classes.len()];
    for (_, text) in own {
        for m in searcher.find_overlapping_iter(&**text) {
            found[m.pattern().as_usize()] = true;
        }
    }
    classes
        .iter()
        .zip(found)
        .filter(|(_, named)| *named)
        .map(|((_, path), _)| *path)
        .collect()
}

/// Which files in a directory a kind of autoload rule looks at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Extensions {
    /// PSR-0 and PSR-4 only ever map to `.php`.
    Psr,
    /// What Composer's classmap generator scans.
    Classmap,
}

impl Extensions {
    fn matches(self, path: &Path) -> bool {
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            return false;
        };
        match self {
            Extensions::Psr => ext == "php",
            Extensions::Classmap => matches!(ext, "php" | "inc" | "hh"),
        }
    }
}

/// Adds `root`, or the files below it that `extensions` covers, to `found`, canonicalised. A
/// file named directly is added whatever its extension. Missing paths are ignored; directories
/// are visited once (by canonical path). Files matched by `exclusions` go into `excluded`, under
/// their path as the rules spell it.
fn walk(
    fs: &dyn Fs,
    root: &Path,
    extensions: Extensions,
    exclusions: Option<&Exclusions>,
    seen_dirs: &mut BTreeSet<PathBuf>,
    found: &mut BTreeSet<PathBuf>,
    excluded: &mut Vec<PathBuf>,
) {
    if fs.is_file(root) {
        if exclusions.is_some_and(|e| e.contains(root)) {
            excluded.push(root.to_path_buf());
        } else if let Ok(path) = fs.canonicalize(root) {
            found.insert(path);
        }
        return;
    }
    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        let Ok(canonical) = fs.canonicalize(&path) else {
            continue;
        };
        let Some(kind) = fs.kind(&canonical) else {
            continue;
        };
        let is_excluded = exclusions.is_some_and(|e| e.contains(&path));
        if let Kind::File { .. } = kind {
            if extensions.matches(&path) {
                if is_excluded {
                    excluded.push(path);
                } else {
                    found.insert(canonical);
                }
            }
            continue;
        }
        if !seen_dirs.insert(canonical.clone()) {
            continue;
        }
        match fs.read_dir(&canonical) {
            Ok(entries) => stack.extend(
                entries
                    .iter()
                    .filter_map(|e| e.file_name())
                    .map(|name| path.join(name)),
            ),
            Err(error) => {
                tracing::warn!(path = %canonical.display(), %error, "cannot read directory");
            }
        }
    }
}

/// Reads vendor files, keeping their order: which of two declarations of a name wins depends
/// on it, so results must not vary between runs. PHP source isn't always UTF-8, so invalid bytes
/// are replaced. Unreadable files are skipped with a warning.
fn read_vendor(fs: &dyn Fs, paths: Vec<PathBuf>) -> Vec<(PathBuf, String)> {
    let read = fs.read_all(&paths);
    paths
        .into_iter()
        .zip(read)
        .filter_map(|(path, text)| match text {
            Ok(text) => Some((path, stir_vfs::lossy(text))),
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "cannot read vendor file");
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use stir_db::PhpVersion;

    use super::*;

    const SRC_DIR: &str = "parameters:\n    stirphp:\n        srcDir: src\n";

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[tokio::test]
    async fn loads_sources_stubs_and_vendor() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        write(&root.join("stirphp.neon"), SRC_DIR);
        write(&root.join("src/main.php"), "<?php\n$x = 1;\n");
        write(&root.join("src/Legacy.php"), "<?php\nclass Legacy {}\n");
        write(
            &root.join("composer.json"),
            r#"{"autoload": {"psr-4": {"App\\": "src/"}}}"#,
        );
        write(
            &root.join("vendor/composer/installed.json"),
            r#"{"packages": [{"name": "acme/lib", "install-path": "../acme/lib",
                "autoload": {"psr-4": {"Acme\\": "src/"}, "files": ["helpers.php"]}}]}"#,
        );
        write(
            &root.join("vendor/acme/lib/src/Foo.php"),
            "<?php\nnamespace Acme;\nclass Foo {}\n",
        );
        write(
            &root.join("vendor/acme/lib/src/Sub/Bar.php"),
            "<?php\nnamespace Acme\\Sub;\nclass Bar {}\n",
        );
        write(&root.join("vendor/acme/lib/src/README.md"), "not php");
        write(
            &root.join("vendor/acme/lib/helpers.php"),
            "<?php\nfunction h() {}\n",
        );

        let project = Project::load_or_default(root).await.unwrap();
        let loaded = load_workspace(&project, LoadOptions::default())
            .await
            .unwrap();
        let db = &loaded.db;
        let files = loaded.workspace.files(db);
        let count = |origin| files.iter().filter(|f| f.origin(db) == origin).count();

        assert_eq!(loaded.sources.len(), 2);
        assert_eq!(count(FileOrigin::Source), 2);
        // `src/Legacy.php` is reachable through the root PSR-4 rule but is a project
        // source, so it is not loaded twice.
        assert_eq!(count(FileOrigin::Vendor), 3);
        assert_eq!(loaded.vendor_count, 3);
        assert_eq!(count(FileOrigin::Stub), loaded.stub_count);
        assert!(loaded.stub_count > 0);
        assert!(
            files
                .iter()
                .filter(|f| f.origin(db) == FileOrigin::Stub)
                .all(|f| f.path(db).to_string_lossy().starts_with("stubs://"))
        );

        let without = load_workspace(
            &project,
            LoadOptions {
                vendor: false,
                ..LoadOptions::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(without.vendor_count, 0);
    }

    #[tokio::test]
    async fn a_source_that_is_not_utf8_still_loads() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        write(&root.join("stirphp.neon"), SRC_DIR);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/latin1.php"), b"<?php\n// caf\xe9\n").unwrap();

        let project = Project::load_or_default(root).await.unwrap();
        let loaded = load_workspace(
            &project,
            LoadOptions {
                vendor: false,
                ..LoadOptions::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(loaded.sources.len(), 1);
        assert!(loaded.sources[0].text(&loaded.db).contains("caf\u{fffd}"));
    }

    #[tokio::test]
    async fn reads_phpstan_settings() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        write(
            &root.join("phpstan.neon.dist"),
            "parameters:\n    level: 5\n    ignoreErrors:\n        - '#^unknown function#'\n        -\n            identifier: function.notFound\n            path: src/legacy.php\n            count: 3\n",
        );
        let project = Project::load_or_default(root).await.unwrap();
        let settings = load_settings(&project).await.unwrap();
        assert_eq!(settings.php_level, 5);
        let rules = settings.ignore_errors.rules();
        assert_eq!(rules.len(), 1, "a message alone can't be matched");
        assert_eq!(rules[0].identifiers, ["function.notFound"]);
        assert!(rules[0].paths[0].ends_with("src/legacy.php"));
        assert_eq!(rules[0].count, Some(3));

        // `stirphp.neon` wins, and can include a PHPStan config
        write(
            &root.join("stirphp.neon"),
            "includes:\n    - conf/custom.neon\n",
        );
        write(
            &root.join("conf/custom.neon"),
            "parameters:\n    level: max\n",
        );
        let project = Project::load_or_default(root).await.unwrap();
        let settings = load_settings(&project).await.unwrap();
        assert_eq!(settings.php_level, stir_db::MAX_PHP_LEVEL);
    }

    #[tokio::test]
    async fn knows_phpstans_helpers_only_with_phpstan_installed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        write(&root.join("stirphp.neon"), SRC_DIR);
        write(&root.join("src/a.php"), "<?php\n");
        let has_helpers = |loaded: &Loaded| {
            loaded
                .workspace
                .files(&loaded.db)
                .iter()
                .any(|f| f.path(&loaded.db).ends_with("phpstan/helpers.php"))
        };
        let project = Project::load_or_default(root).await.unwrap();
        let loaded = load_workspace(&project, LoadOptions::default())
            .await
            .unwrap();
        assert!(!has_helpers(&loaded));

        std::fs::create_dir_all(root.join("vendor/phpstan/phpstan")).unwrap();
        let loaded = load_workspace(&project, LoadOptions::default())
            .await
            .unwrap();
        assert!(has_helpers(&loaded));
    }

    #[tokio::test]
    async fn finds_every_default_phpstan_config_name() {
        for name in ["phpstan.neon", "phpstan.neon.dist", "phpstan.dist.neon"] {
            let tmp = tempfile::TempDir::new().unwrap();
            let root = tmp.path();
            write(&root.join(name), "parameters:\n    level: 3\n");
            let project = Project::load_or_default(root).await.unwrap();
            let settings = load_settings(&project).await.unwrap();
            assert_eq!(settings.php_level, 3, "{name}");
        }
    }

    #[tokio::test]
    async fn loads_phpstan_paths_and_scanned_files() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        write(&root.join("src/a.php"), "<?php\necho BOOTED;\n");
        write(
            &root.join("tools/boot.php"),
            "<?php\ndefine('BOOTED', 1);\n",
        );
        write(
            &root.join("phpstan.neon"),
            "parameters:\n    stirphp:\n        srcDir: src\n    paths:\n        - src\n    excludePaths:\n        analyse:\n            - src/gen\n    bootstrapFiles:\n        - tools/boot.php\n    scanFiles:\n        - src/a.php\n",
        );
        let project = Project::load_or_default(&root).await.unwrap();
        let loaded = load_workspace(
            &project,
            LoadOptions {
                vendor: false,
                ..LoadOptions::default()
            },
        )
        .await
        .unwrap();
        let settings = loaded.workspace.settings(&loaded.db);
        assert_eq!(settings.php_paths, [root.join("src")]);
        assert_eq!(settings.php_excludes, [root.join("src/gen")]);
        assert_eq!(loaded.sources.len(), 1);
        let paths: Vec<_> = loaded
            .workspace
            .files(&loaded.db)
            .iter()
            .map(|f| f.path(&loaded.db).clone())
            .collect();
        assert_eq!(
            paths
                .iter()
                .filter(|p| p.ends_with("tools/boot.php"))
                .count(),
            1
        );
        assert_eq!(paths.iter().filter(|p| p.ends_with("src/a.php")).count(), 1);
    }

    /// The paths of `loaded`'s files of an origin, relative to `root`.
    fn paths_of(loaded: &Loaded, root: &Path, origin: FileOrigin) -> Vec<String> {
        let db = &loaded.db;
        let mut out: Vec<String> = loaded
            .workspace
            .files(db)
            .iter()
            .filter(|f| f.origin(db) == origin)
            .map(|f| {
                let path = f.path(db);
                path.strip_prefix(root)
                    .map_or_else(|_| path.display().to_string(), |p| p.display().to_string())
            })
            .collect();
        out.sort();
        out
    }

    #[tokio::test]
    async fn source_paths_are_plain() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        write(&root.join("a.php"), "<?php\n");
        let loaded = load(&root).await;
        // an editor gets this as a URI: `/./` in it is another file
        let path = loaded.sources[0].path(&loaded.db);
        assert_eq!(path.to_string_lossy(), root.join("a.php").to_string_lossy());
    }

    async fn load(root: &Path) -> Loaded {
        let project = Project::load_or_default(root).await.unwrap();
        load_workspace(&project, LoadOptions::default())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn without_composer_the_folder_is_the_source_root() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        write(&root.join("a.php"), "<?php\n");
        write(&root.join("routes/web.php"), "<?php\n");
        let loaded = load(&root).await;
        assert_eq!(
            paths_of(&loaded, &root, FileOrigin::Source),
            ["a.php", "routes/web.php"]
        );
        assert!(paths_of(&loaded, &root, FileOrigin::Vendor).is_empty());
        assert!(loaded.composer.is_none());
        assert!(loaded.notices.is_empty());

        // `vendor/` is scanned if it's there, and stays vendor
        write(&root.join("vendor/acme/lib.php"), "<?php\nclass Lib {}\n");
        let loaded = load(&root).await;
        assert_eq!(
            paths_of(&loaded, &root, FileOrigin::Source),
            ["a.php", "routes/web.php"]
        );
        assert_eq!(
            paths_of(&loaded, &root, FileOrigin::Vendor),
            ["vendor/acme/lib.php"]
        );
        assert!(loaded.notices.is_empty());
    }

    #[tokio::test]
    async fn sources_with_composer_are_more_than_its_autoload_paths() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let project_dir = root.join("app");
        write(
            &project_dir.join("composer.json"),
            r#"{"autoload": {
                "psr-4": {"App\\": "src/", "Shared\\": "../shared/src/"},
                "classmap": ["../legacy/"],
                "files": ["helpers.inc"]
            }}"#,
        );
        write(&project_dir.join("src/A.php"), "<?php\n");
        write(&project_dir.join("routes/web.php"), "<?php\n");
        write(
            &project_dir.join("helpers.inc"),
            "<?php\nfunction helper() {}\n",
        );
        write(&root.join("shared/src/S.php"), "<?php\n");
        write(&root.join("legacy/L.inc"), "<?php\n");
        write(&root.join("legacy/notes.txt"), "not php");
        let loaded = load(&project_dir).await;
        assert_eq!(
            paths_of(&loaded, &project_dir, FileOrigin::Source),
            [
                root.join("legacy/L.inc").display().to_string(),
                root.join("shared/src/S.php").display().to_string(),
                "helpers.inc".to_owned(),
                "routes/web.php".to_owned(),
                "src/A.php".to_owned(),
            ]
        );
        assert!(paths_of(&loaded, &project_dir, FileOrigin::Vendor).is_empty());
    }

    #[tokio::test]
    async fn a_custom_vendor_dir_is_vendor() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        write(
            &root.join("composer.json"),
            r#"{"config": {"vendor-dir": "lib/deps"}, "require": {"acme/lib": "*"}}"#,
        );
        write(&root.join("a.php"), "<?php\n");
        write(
            &root.join("lib/deps/composer/installed.json"),
            r#"{"packages": [{"name": "acme/lib", "install-path": "../acme/lib",
                "autoload": {"psr-4": {"Acme\\": "src/"}}}]}"#,
        );
        write(&root.join("lib/deps/acme/lib/src/Foo.php"), "<?php\n");
        write(&root.join("lib/deps/phpstan/phpstan/x"), "");
        let loaded = load(&root).await;
        assert_eq!(paths_of(&loaded, &root, FileOrigin::Source), ["a.php"]);
        assert_eq!(
            paths_of(&loaded, &root, FileOrigin::Vendor),
            ["lib/deps/acme/lib/src/Foo.php"]
        );
        assert_eq!(loaded.vendor_dir, root.join("lib/deps"));
        assert!(loaded.notices.is_empty(), "{:?}", loaded.notices);
        assert!(
            loaded
                .workspace
                .files(&loaded.db)
                .iter()
                .any(|f| f.path(&loaded.db).ends_with("phpstan/helpers.php"))
        );
    }

    #[tokio::test]
    async fn autoload_accuracy() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        write(
            &root.join("composer.json"),
            r#"{"require": {"acme/lib": "*"}}"#,
        );
        // the project names a class in an excluded directory: autoloading would find it
        write(&root.join("a.php"), "<?php\nnew \\Acme\\Tests\\Used();\n");
        write(
            &root.join("vendor/composer/installed.json"),
            r#"{"packages": [{"name": "acme/lib", "install-path": "../acme/lib", "autoload": {
                "psr-4": {"Acme\\": ""},
                "classmap": ["maps/", "single.phtml"],
                "files": ["functions.inc", "gone.php"],
                "exclude-from-classmap": ["/Tests/", "maps/skipped/", "src/data/"]
            }}]}"#,
        );
        let lib = root.join("vendor/acme/lib");
        write(&lib.join("Foo.php"), "<?php\n");
        write(&lib.join("README.md"), "not php");
        write(&lib.join("Tests/Used.php"), "<?php\n");
        write(&lib.join("Tests/Unused.php"), "<?php\n");
        write(&lib.join("src/data/a-b.json.php"), "<?php\n");
        write(&lib.join("maps/Old.inc"), "<?php\n");
        write(&lib.join("maps/notes.txt"), "not php");
        write(&lib.join("maps/skipped/Skipped.php"), "<?php\n");
        write(&lib.join("single.phtml"), "<?php\n");
        write(&lib.join("functions.inc"), "<?php\n");
        let loaded = load(&root).await;
        assert_eq!(
            paths_of(&loaded, &root, FileOrigin::Vendor),
            [
                "vendor/acme/lib/Foo.php",
                "vendor/acme/lib/Tests/Used.php",
                "vendor/acme/lib/functions.inc",
                "vendor/acme/lib/maps/Old.inc",
                "vendor/acme/lib/single.phtml",
            ]
        );
    }

    #[tokio::test]
    async fn the_php_version_comes_from_phpstan_then_composer() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        write(&root.join("a.php"), "<?php\n");
        let versions = || async {
            let project = Project::load_or_default(&root).await.unwrap();
            load_settings(&project)
                .await
                .unwrap()
                .php_version
                .to_string()
        };
        assert_eq!(versions().await, PhpVersion::LATEST.to_string());

        write(
            &root.join("composer.json"),
            r#"{"require": {"php": ">=8.1 <8.4"}}"#,
        );
        assert_eq!(versions().await, "8.1");

        write(
            &root.join("composer.json"),
            r#"{"require": {"php": "^8.1"}, "config": {"platform": {"php": "8.3.5"}}}"#,
        );
        assert_eq!(versions().await, "8.3");

        write(
            &root.join("phpstan.neon"),
            "parameters:\n    phpVersion: 80200\n",
        );
        assert_eq!(versions().await, "8.2");

        write(
            &root.join("phpstan.neon"),
            "parameters:\n    phpVersion:\n        min: 80100\n        max: 80399\n",
        );
        assert_eq!(versions().await, "8.1 to 8.3");
    }

    #[tokio::test]
    async fn phpstan_extensions_add_what_they_scan() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        write(&root.join("composer.json"), "{}");
        write(&root.join("phpstan.neon"), "parameters:\n    level: 3\n");
        write(&root.join("a.php"), "<?php\necho EXT_CONST;\n");
        write(
            &root.join("vendor/composer/installed.json"),
            r#"{"packages": [
                {"name": "phpstan/extension-installer"},
                {"name": "acme/ext", "install-path": "../acme/ext",
                 "extra": {"phpstan": {"includes": ["extension.neon"]}}}
            ]}"#,
        );
        write(
            &root.join("vendor/acme/ext/extension.neon"),
            "parameters:\n    level: 8\n    scanFiles:\n        - stubs.php\n",
        );
        write(
            &root.join("vendor/acme/ext/stubs.php"),
            "<?php\nconst EXT_CONST = 1;\n",
        );
        let loaded = load(&root).await;
        let settings = loaded.workspace.settings(&loaded.db);
        assert_eq!(settings.php_level, 3, "the project's config comes last");
        assert_eq!(
            paths_of(&loaded, &root, FileOrigin::Vendor),
            ["vendor/acme/ext/stubs.php"]
        );
        // a broken one doesn't take the project down
        write(
            &root.join("vendor/acme/ext/extension.neon"),
            "parameters:\n  level: [\n",
        );
        assert_eq!(
            load(&root).await.workspace.settings(&loaded.db).php_level,
            3
        );
    }

    #[tokio::test]
    async fn one_notice_while_composer_has_not_installed_what_is_declared() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        write(
            &root.join("composer.json"),
            r#"{"require": {"acme/lib": "^1"}, "autoload": {"psr-4": {"App\\": "src/"}}}"#,
        );
        write(&root.join("src/A.php"), "<?php\n");
        let loaded = load(&root).await;
        assert_eq!(loaded.notices.len(), 1, "{:?}", loaded.notices);
        assert!(loaded.notices[0].contains("composer install"));
        let settings = loaded.workspace.settings(&loaded.db);
        assert_eq!(
            settings.pending_install.as_deref(),
            Some(&["app\\".to_owned()][..])
        );

        write(
            &root.join("vendor/composer/installed.json"),
            r#"{"packages": [{"name": "acme/lib", "version": "1.0.0"}], "dev": true}"#,
        );
        let loaded = load(&root).await;
        assert!(loaded.notices.is_empty(), "{:?}", loaded.notices);
        assert!(
            loaded
                .workspace
                .settings(&loaded.db)
                .pending_install
                .is_none()
        );

        write(
            &root.join("composer.lock"),
            r#"{"packages": [{"name": "acme/lib", "version": "1.1.0"}]}"#,
        );
        let loaded = load(&root).await;
        assert_eq!(loaded.notices.len(), 1, "{:?}", loaded.notices);
        assert!(loaded.notices[0].contains("1.1.0"), "{:?}", loaded.notices);
        assert!(
            loaded
                .workspace
                .settings(&loaded.db)
                .pending_install
                .is_some()
        );
    }

    #[tokio::test]
    async fn a_broken_composer_json_is_a_notice() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        write(&root.join("composer.json"), "{");
        write(&root.join("a.php"), "<?php\n");
        let loaded = load(&root).await;
        assert_eq!(loaded.sources.len(), 1);
        assert_eq!(loaded.notices.len(), 1);
        assert!(
            loaded.notices[0].contains("composer.json"),
            "{:?}",
            loaded.notices
        );
    }
}
