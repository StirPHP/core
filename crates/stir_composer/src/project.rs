//! A project as Composer sees it.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::Value;
use stir_db::PhpVersion;

use crate::autoload::{Autoload, normalize};
use crate::constraint::lowest_php_version;

/// An installed Composer package read from `installed.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledPackage {
    pub name: String,
    /// Absolute install directory of the package.
    pub install_path: PathBuf,
}

/// An installed package, and what it says about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    pub name: String,
    pub version: Option<String>,
    /// Installed only for development (`dev-package-names`).
    pub dev: bool,
    /// Absolute install directory of the package.
    pub install_path: PathBuf,
    /// `extra.phpstan.includes`: the PHPStan configs it brings, absolute.
    pub phpstan_includes: Vec<PathBuf>,
}

/// Whether `vendor/` holds what the project asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallState {
    /// As declared, or nothing has to be installed.
    Installed,
    /// Packages are declared, but `installed.json` isn't there: `composer install` hasn't run.
    Missing,
    /// It's there, but disagrees with `composer.json` or `composer.lock`. Says how.
    Stale(String),
}

/// A framework a project is built on, for behaviour specific to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Framework {
    CakePhp,
    CodeIgniter,
    Drupal,
    Laminas,
    Laravel,
    Magento,
    Mockery,
    Pest,
    Slim,
    Symfony,
    WordPress,
    Yii,
}

/// The packages that mean a framework. A root package being one of these counts too.
const FRAMEWORK_PACKAGES: &[(&str, Framework)] = &[
    ("cakephp/cakephp", Framework::CakePhp),
    ("codeigniter4/framework", Framework::CodeIgniter),
    ("drupal/core", Framework::Drupal),
    ("drupal/core-recommended", Framework::Drupal),
    ("johnpbloch/wordpress", Framework::WordPress),
    ("johnpbloch/wordpress-core", Framework::WordPress),
    ("laminas/laminas-mvc", Framework::Laminas),
    ("laravel/framework", Framework::Laravel),
    ("magento/product-community-edition", Framework::Magento),
    ("magento/framework", Framework::Magento),
    // what a plugin or theme is checked against
    ("mockery/mockery", Framework::Mockery),
    ("pestphp/pest", Framework::Pest),
    ("php-stubs/wordpress-stubs", Framework::WordPress),
    ("roots/wordpress", Framework::WordPress),
    ("slim/slim", Framework::Slim),
    ("symfony/framework-bundle", Framework::Symfony),
    ("symfony/symfony", Framework::Symfony),
    ("yiisoft/yii2", Framework::Yii),
];

/// What Composer's files say about a project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Composer {
    /// The directory of `composer.json`.
    pub root: PathBuf,
    /// `config.vendor-dir`, absolute.
    pub vendor_dir: PathBuf,
    /// The root package's name.
    pub name: Option<String>,
    /// The root package's `autoload` and `autoload-dev` rules.
    pub root_autoload: Autoload,
    /// The `autoload` rules of the installed packages.
    pub vendor_autoload: Autoload,
    pub packages: Vec<Package>,
    pub install: InstallState,
    /// `config.platform.php`, else `require.php`: the lowest version they allow.
    pub php_version: Option<PhpVersion>,
    /// The `ext-*` that the root package or an installed one requires or suggests, without
    /// `ext-`, lowercase.
    pub extensions: BTreeSet<String>,
    /// Names from the root package's `require` and `require-dev`.
    required: Vec<String>,
}

impl Composer {
    /// Reads the project of the `composer.json` at `composer_json`, asking `read` for the files
    /// it needs. `read` gives `None` for a file that isn't there. Errors if the manifest itself
    /// isn't there or isn't JSON; a broken lock or `installed.json` shows as a stale install.
    pub fn load_with(
        composer_json: &Path,
        mut read: impl FnMut(&Path) -> Option<String>,
    ) -> Result<Self, ComposerError> {
        let Some(manifest) = read(composer_json) else {
            return Err(ComposerError::NoManifest);
        };
        let manifest: Value =
            serde_json::from_str(&manifest).map_err(ComposerError::InvalidManifest)?;
        let root = composer_json
            .parent()
            .unwrap_or(Path::new(""))
            .to_path_buf();
        let vendor_dir = vendor_dir(&manifest, &root);
        let installed = read(&vendor_dir.join("composer").join("installed.json"));
        let lock = read(&root.join("composer.lock"));
        Ok(Self::from_contents(
            root,
            vendor_dir,
            &manifest,
            installed.as_deref(),
            lock.as_deref(),
        ))
    }

    fn from_contents(
        root: PathBuf,
        vendor_dir: PathBuf,
        manifest: &Value,
        installed: Option<&str>,
        lock: Option<&str>,
    ) -> Self {
        let mut root_autoload = Autoload::default();
        for section in ["autoload", "autoload-dev"] {
            if let Some(rules) = manifest.get(section) {
                root_autoload.add_rules(rules, &root);
            }
        }

        let installed_value: Option<Value> = installed.and_then(|t| serde_json::from_str(t).ok());
        let mut vendor_autoload = Autoload::default();
        let mut packages = Vec::new();
        let mut extensions = BTreeSet::new();
        declared_extensions(manifest, true, &mut extensions);
        if let Some(value) = &installed_value {
            let dev_names = string_set(value.get("dev-package-names"));
            for pkg in package_list(value) {
                let Some((package, rules)) = parse_package(pkg, &vendor_dir, &dev_names) else {
                    continue;
                };
                if let Some(rules) = rules {
                    vendor_autoload.add_rules(rules, &package.install_path);
                }
                declared_extensions(pkg, false, &mut extensions);
                packages.push(package);
            }
        }

        let install = install_state(manifest, installed, installed_value.as_ref(), lock);
        let required = ["require", "require-dev"]
            .iter()
            .filter_map(|s| manifest.get(*s).and_then(Value::as_object))
            .flat_map(|m| m.keys().cloned())
            .collect();
        Composer {
            root,
            vendor_dir,
            name: manifest
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_owned),
            root_autoload,
            vendor_autoload,
            packages,
            install,
            php_version: declared_php_version(manifest),
            extensions,
            required,
        }
    }

    /// The rules of the root package and of the installed ones.
    pub fn autoload(&self) -> Autoload {
        let mut out = self.root_autoload.clone();
        out.merge(self.vendor_autoload.clone());
        out
    }

    /// One notice about the state of `vendor/`, if something's off.
    pub fn install_notice(&self) -> Option<String> {
        match &self.install {
            InstallState::Installed => None,
            InstallState::Missing => Some(format!(
                "Composer's packages aren't installed: {} doesn't exist. Run `composer install`; \
                 until then, errors about what they'd declare are held back.",
                self.vendor_dir.join("composer/installed.json").display()
            )),
            InstallState::Stale(why) => Some(format!(
                "Composer's packages aren't up to date: {why}. Run `composer install`; until \
                 then, errors about what they'd declare are held back."
            )),
        }
    }

    /// Is the package installed, or required by the root package?
    pub fn has_package(&self, name: &str) -> bool {
        self.packages.iter().any(|p| p.name == name)
            || self.required.iter().any(|r| r == name)
            || self.name.as_deref() == Some(name)
    }

    /// Does the root package itself require `name`, in `require` or `require-dev`?
    pub fn requires(&self, name: &str) -> bool {
        self.required.iter().any(|r| r == name)
    }

    /// The frameworks the project uses, sorted.
    pub fn frameworks(&self) -> Vec<Framework> {
        let mut out: Vec<Framework> = FRAMEWORK_PACKAGES
            .iter()
            .filter(|(name, _)| self.has_package(name))
            .map(|(_, f)| *f)
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// The PHPStan configs that `phpstan/extension-installer` would load: extensions apply on
    /// their own only with that package installed, as PHPStan does it.
    pub fn phpstan_includes(&self) -> Vec<PathBuf> {
        if !self
            .packages
            .iter()
            .any(|p| p.name == "phpstan/extension-installer")
        {
            return Vec::new();
        }
        self.packages
            .iter()
            .flat_map(|p| p.phpstan_includes.iter().cloned())
            .collect()
    }

    /// The namespaces of the project's own code, lowercase: a name in one of these can't be
    /// waiting for a package.
    pub fn own_namespaces(&self) -> Vec<String> {
        self.root_autoload
            .psr4
            .iter()
            .chain(&self.root_autoload.psr0)
            .map(|(prefix, _)| prefix.to_ascii_lowercase())
            .filter(|prefix| !prefix.is_empty())
            .collect()
    }
}

/// Why a `composer.json` can't be used.
#[derive(Debug)]
pub enum ComposerError {
    NoManifest,
    InvalidManifest(serde_json::Error),
}

impl std::fmt::Display for ComposerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ComposerError::NoManifest => f.write_str("there's no composer.json"),
            ComposerError::InvalidManifest(e) => write!(f, "composer.json isn't valid: {e}"),
        }
    }
}

impl std::error::Error for ComposerError {}

/// `config.vendor-dir`, or `vendor`, against the project's directory.
pub(crate) fn vendor_dir(manifest: &Value, root: &Path) -> PathBuf {
    let dir = manifest
        .pointer("/config/vendor-dir")
        .and_then(Value::as_str)
        .unwrap_or("vendor");
    normalize(&root.join(dir))
}

fn declared_php_version(manifest: &Value) -> Option<PhpVersion> {
    // `config.platform.php` can also be `false`, for "the running one"
    manifest
        .pointer("/config/platform/php")
        .and_then(Value::as_str)
        .and_then(lowest_php_version)
        .or_else(|| {
            manifest
                .pointer("/require/php")
                .and_then(Value::as_str)
                .and_then(lowest_php_version)
        })
}

fn declared_extensions(package: &Value, root: bool, out: &mut BTreeSet<String>) {
    let mut sections = vec!["require", "suggest"];
    if root {
        sections.push("require-dev");
    }
    for section in sections {
        let Some(map) = package.get(section).and_then(Value::as_object) else {
            continue;
        };
        for name in map.keys() {
            if let Some(ext) = name.strip_prefix("ext-") {
                out.insert(ext.to_ascii_lowercase());
            }
        }
    }
}

fn string_set(value: Option<&Value>) -> HashSet<&str> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}

/// `{"packages": [...]}` (Composer 2) or a plain array (older ones).
fn package_list(value: &Value) -> &[Value] {
    match value {
        Value::Array(items) => items.as_slice(),
        Value::Object(obj) => obj
            .get("packages")
            .and_then(Value::as_array)
            .map_or(&[][..], |v| v),
        _ => &[],
    }
}

fn install_path(pkg: &Value, name: &str, vendor: &Path) -> PathBuf {
    match pkg.get("install-path").and_then(Value::as_str) {
        Some(p) => normalize(&vendor.join("composer").join(p)),
        None => normalize(&vendor.join(name)),
    }
}

fn parse_package<'a>(
    pkg: &'a Value,
    vendor: &Path,
    dev_names: &HashSet<&str>,
) -> Option<(Package, Option<&'a Value>)> {
    let name = pkg.get("name")?.as_str()?.to_owned();
    let install_path = install_path(pkg, &name, vendor);
    let phpstan_includes = pkg
        .pointer("/extra/phpstan/includes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|p| normalize(&install_path.join(p)))
        .collect();
    let package = Package {
        dev: dev_names.contains(name.as_str()),
        version: pkg
            .get("version")
            .and_then(Value::as_str)
            .map(str::to_owned),
        name,
        install_path,
        phpstan_includes,
    };
    Some((package, pkg.get("autoload")))
}

/// Parses `installed.json` in either the Composer 2 format (`{"packages": [...]}`) or the
/// legacy plain-array format. Each package's install path is `install-path` resolved against
/// `<vendor>/composer` when present, otherwise `<vendor>/<name>`. Returns each package with
/// its raw `autoload` value, if any.
pub fn parse_installed<'a>(
    value: &'a Value,
    vendor: &Path,
) -> Vec<(InstalledPackage, Option<&'a Value>)> {
    package_list(value)
        .iter()
        .filter_map(|pkg| {
            let name = pkg.get("name")?.as_str()?.to_owned();
            let install_path = install_path(pkg, &name, vendor);
            Some((InstalledPackage { name, install_path }, pkg.get("autoload")))
        })
        .collect()
}

/// Not a package to install: PHP itself, its extensions and libraries, Composer's own APIs.
fn is_platform_package(name: &str) -> bool {
    name == "php"
        || name == "php-64bit"
        || name == "hhvm"
        || name.starts_with("ext-")
        || name.starts_with("lib-")
        || name.starts_with("composer-")
        || name == "composer"
}

fn install_state(
    manifest: &Value,
    installed_text: Option<&str>,
    installed: Option<&Value>,
    lock: Option<&str>,
) -> InstallState {
    let section = |name: &str| -> Vec<&str> {
        manifest
            .get(name)
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|m| m.keys())
            .map(String::as_str)
            .filter(|n| !is_platform_package(n))
            .collect()
    };
    let (require, require_dev) = (section("require"), section("require-dev"));
    if installed_text.is_none() {
        return if require.is_empty() && require_dev.is_empty() {
            InstallState::Installed
        } else {
            InstallState::Missing
        };
    }
    let Some(installed) = installed else {
        return InstallState::Stale("installed.json can't be read".to_owned());
    };

    // `composer install --no-dev` leaves out the development packages, and says so
    let with_dev = installed
        .get("dev")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let mut versions: HashMap<&str, &str> = HashMap::new();
    let mut names: HashSet<String> = HashSet::new();
    for pkg in package_list(installed) {
        let Some(name) = pkg.get("name").and_then(Value::as_str) else {
            continue;
        };
        names.insert(name.to_ascii_lowercase());
        versions.insert(
            name,
            pkg.get("version").and_then(Value::as_str).unwrap_or(""),
        );
        for section in ["replace", "provide"] {
            if let Some(map) = pkg.get(section).and_then(Value::as_object) {
                names.extend(map.keys().map(|k| k.to_ascii_lowercase()));
            }
        }
    }
    for section in ["replace", "provide"] {
        if let Some(map) = manifest.get(section).and_then(Value::as_object) {
            names.extend(map.keys().map(|k| k.to_ascii_lowercase()));
        }
    }

    let wanted = require.iter().chain(if with_dev {
        require_dev.iter()
    } else {
        [].iter()
    });
    let absent: Vec<&&str> = wanted
        .filter(|n| !names.contains(&n.to_ascii_lowercase()))
        .collect();
    if let Some(first) = absent.first() {
        return InstallState::Stale(describe(
            &format!("composer.json requires {first}, which isn't installed"),
            absent.len(),
        ));
    }

    let Some(lock) = lock else {
        return InstallState::Installed;
    };
    let Ok(lock) = serde_json::from_str::<Value>(lock) else {
        return InstallState::Stale("composer.lock can't be read".to_owned());
    };
    let mut locked: Vec<&Value> = Vec::new();
    for section in ["packages", "packages-dev"] {
        if section == "packages-dev" && !with_dev {
            continue;
        }
        if let Some(items) = lock.get(section).and_then(Value::as_array) {
            locked.extend(items);
        }
    }
    let mut differences = Vec::new();
    for pkg in locked {
        let Some(name) = pkg.get("name").and_then(Value::as_str) else {
            continue;
        };
        let version = pkg.get("version").and_then(Value::as_str).unwrap_or("");
        match versions.get(name) {
            None => differences.push(format!(
                "composer.lock has {name} {version}, which isn't installed"
            )),
            Some(have) if *have != version => differences.push(format!(
                "composer.lock has {name} {version}, but {have} is installed"
            )),
            Some(_) => {}
        }
    }
    match differences.first() {
        Some(first) => InstallState::Stale(describe(first, differences.len())),
        None => InstallState::Installed,
    }
}

fn describe(first: &str, count: usize) -> String {
    match count {
        1 => first.to_owned(),
        n => format!("{first}, among {n} differences"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A project read from in-memory files, as a browser would supply them.
    fn composer(files: &[(&str, &str)]) -> Composer {
        let files: HashMap<PathBuf, String> = files
            .iter()
            .map(|(p, t)| (PathBuf::from("/p").join(p), (*t).to_owned()))
            .collect();
        Composer::load_with(Path::new("/p/composer.json"), |p| files.get(p).cloned()).unwrap()
    }

    const REQUIRES: &str = r#"{"require": {"php": "^8.2", "ext-redis": "*", "acme/lib": "^1"}}"#;
    const INSTALLED: &str = r#"{"packages": [{"name": "acme/lib", "version": "1.2.0",
        "install-path": "../acme/lib"}], "dev": true, "dev-package-names": []}"#;

    #[test]
    fn nothing_to_install_is_not_a_missing_install() {
        let c = composer(&[(
            "composer.json",
            r#"{"require": {"php": "^8.2", "ext-json": "*", "composer-runtime-api": "^2"},
                "autoload": {"psr-4": {"App\\": "src/"}}}"#,
        )]);
        assert_eq!(c.install, InstallState::Installed);
        assert_eq!(c.install_notice(), None);
    }

    #[test]
    fn missing_install() {
        let c = composer(&[("composer.json", REQUIRES)]);
        assert_eq!(c.install, InstallState::Missing);
        assert!(c.install_notice().unwrap().contains("composer install"));
        let dev_only = composer(&[("composer.json", r#"{"require-dev": {"a/b": "*"}}"#)]);
        assert_eq!(dev_only.install, InstallState::Missing);
    }

    #[test]
    fn up_to_date_install() {
        let c = composer(&[
            ("composer.json", REQUIRES),
            ("vendor/composer/installed.json", INSTALLED),
            (
                "composer.lock",
                r#"{"packages": [{"name": "acme/lib", "version": "1.2.0"}], "packages-dev": []}"#,
            ),
        ]);
        assert_eq!(c.install, InstallState::Installed);
    }

    #[test]
    fn a_lock_that_disagrees() {
        let c = composer(&[
            ("composer.json", REQUIRES),
            ("vendor/composer/installed.json", INSTALLED),
            (
                "composer.lock",
                r#"{"packages": [{"name": "acme/lib", "version": "1.3.0"},
                                 {"name": "acme/other", "version": "2.0.0"}]}"#,
            ),
        ]);
        let InstallState::Stale(why) = &c.install else {
            panic!("{:?}", c.install);
        };
        assert!(why.contains("acme/lib 1.3.0, but 1.2.0"), "{why}");
        assert!(why.contains("2 differences"), "{why}");
    }

    #[test]
    fn a_requirement_not_installed() {
        let c = composer(&[
            (
                "composer.json",
                r#"{"require": {"acme/lib": "^1", "acme/new": "^1"}}"#,
            ),
            ("vendor/composer/installed.json", INSTALLED),
        ]);
        let InstallState::Stale(why) = &c.install else {
            panic!("{:?}", c.install);
        };
        assert!(why.contains("acme/new"), "{why}");
    }

    #[test]
    fn what_a_package_replaces_or_provides_counts_as_installed() {
        let c = composer(&[
            (
                "composer.json",
                r#"{"require": {"psr/log-implementation": "*", "acme/old": "*"},
                    "replace": {"acme/old": "*"}}"#,
            ),
            (
                "vendor/composer/installed.json",
                r#"{"packages": [{"name": "monolog/monolog", "version": "3.0.0",
                    "provide": {"psr/log-implementation": "3.0"}}]}"#,
            ),
        ]);
        assert_eq!(c.install, InstallState::Installed);
    }

    #[test]
    fn an_install_without_dev_packages_is_complete() {
        let installed = r#"{"packages": [{"name": "acme/lib", "version": "1.2.0"}],
            "dev": false, "dev-package-names": []}"#;
        let c = composer(&[
            (
                "composer.json",
                r#"{"require": {"acme/lib": "^1"}, "require-dev": {"acme/dev": "^1"}}"#,
            ),
            ("vendor/composer/installed.json", installed),
            (
                "composer.lock",
                r#"{"packages": [{"name": "acme/lib", "version": "1.2.0"}],
                    "packages-dev": [{"name": "acme/dev", "version": "1.0.0"}]}"#,
            ),
        ]);
        assert_eq!(c.install, InstallState::Installed);
    }

    #[test]
    fn broken_files_are_a_stale_install() {
        let c = composer(&[
            ("composer.json", REQUIRES),
            ("vendor/composer/installed.json", "{"),
        ]);
        assert!(matches!(c.install, InstallState::Stale(_)));
    }

    #[test]
    fn a_broken_manifest_is_an_error() {
        let read = |_: &Path| Some("{".to_owned());
        assert!(Composer::load_with(Path::new("/p/composer.json"), read).is_err());
    }

    #[test]
    fn custom_vendor_dir() {
        let c = composer(&[
            (
                "composer.json",
                r#"{"config": {"vendor-dir": "lib/deps"}, "require": {"acme/lib": "^1"}}"#,
            ),
            ("lib/deps/composer/installed.json", INSTALLED),
        ]);
        assert_eq!(c.vendor_dir, Path::new("/p/lib/deps"));
        assert_eq!(c.install, InstallState::Installed);
        assert_eq!(
            c.packages[0].install_path,
            Path::new("/p/lib/deps/acme/lib")
        );
    }

    #[test]
    fn php_version_from_platform_then_require() {
        let version = |json: &str| {
            composer(&[("composer.json", json)])
                .php_version
                .map(|v| v.to_string())
        };
        assert_eq!(
            version(r#"{"require": {"php": "^8.3"}, "config": {"platform": {"php": "8.2.99"}}}"#)
                .as_deref(),
            Some("8.2")
        );
        assert_eq!(
            version(r#"{"require": {"php": ">=8.1 <8.5"}}"#).as_deref(),
            Some("8.1")
        );
        assert_eq!(
            version(r#"{"require": {"php": "^8.3"}, "config": {"platform": {"php": false}}}"#)
                .as_deref(),
            Some("8.3")
        );
        assert_eq!(version("{}"), None);
    }

    #[test]
    fn extensions_from_require_and_suggest() {
        let c = composer(&[
            (
                "composer.json",
                r#"{"require": {"ext-Redis": "*"}, "require-dev": {"ext-xdebug": "*"},
                    "suggest": {"ext-imagick": "for images"}}"#,
            ),
            (
                "vendor/composer/installed.json",
                r#"{"packages": [{"name": "a/b", "require": {"ext-swoole": "*"},
                    "require-dev": {"ext-ignored": "*"}, "suggest": {"ext-apcu": "cache"}}]}"#,
            ),
        ]);
        let want: Vec<&str> = vec!["apcu", "imagick", "redis", "swoole", "xdebug"];
        assert_eq!(
            c.extensions.iter().map(String::as_str).collect::<Vec<_>>(),
            want
        );
    }

    #[test]
    fn dev_packages() {
        let c = composer(&[
            ("composer.json", "{}"),
            (
                "vendor/composer/installed.json",
                r#"{"packages": [{"name": "a/run"}, {"name": "a/dev"}],
                    "dev-package-names": ["a/dev"]}"#,
            ),
        ]);
        let dev: Vec<_> = c
            .packages
            .iter()
            .map(|p| (p.name.as_str(), p.dev))
            .collect();
        assert_eq!(dev, [("a/run", false), ("a/dev", true)]);
    }

    #[test]
    fn frameworks_from_what_is_required_or_installed() {
        let frameworks = |json: &str| composer(&[("composer.json", json)]).frameworks();
        assert_eq!(
            frameworks(r#"{"require": {"laravel/framework": "^11"}}"#),
            [Framework::Laravel]
        );
        assert_eq!(
            frameworks(r#"{"name": "symfony/symfony"}"#),
            [Framework::Symfony]
        );
        assert_eq!(frameworks(r#"{"require": {"acme/lib": "1"}}"#), []);
        let c = composer(&[
            ("composer.json", "{}"),
            (
                "vendor/composer/installed.json",
                r#"{"packages": [{"name": "symfony/framework-bundle"}]}"#,
            ),
        ]);
        assert_eq!(c.frameworks(), [Framework::Symfony]);
        assert!(c.has_package("symfony/framework-bundle"));
    }

    #[test]
    fn phpstan_extensions_load_through_the_installer() {
        let packages = r#"{"packages": [{"name": "a/ext", "install-path": "../a/ext",
            "extra": {"phpstan": {"includes": ["extension.neon", "./rules.neon"]}}}]}"#;
        let without = composer(&[
            ("composer.json", "{}"),
            ("vendor/composer/installed.json", packages),
        ]);
        assert!(without.phpstan_includes().is_empty());
        assert_eq!(without.packages[0].phpstan_includes.len(), 2);
        let with_installer = packages.replace(
            r#"{"packages": ["#,
            r#"{"packages": [{"name": "phpstan/extension-installer"}, "#,
        );
        let with = composer(&[
            ("composer.json", "{}"),
            ("vendor/composer/installed.json", &with_installer),
        ]);
        assert_eq!(
            with.phpstan_includes(),
            [
                PathBuf::from("/p/vendor/a/ext/extension.neon"),
                PathBuf::from("/p/vendor/a/ext/rules.neon"),
            ]
        );
    }

    #[test]
    fn own_namespaces_leave_out_fallback_directories() {
        let c = composer(&[(
            "composer.json",
            r#"{"autoload": {"psr-4": {"App\\": "src/", "": "lib/"}},
                "autoload-dev": {"psr-4": {"Tests\\": "tests/"}, "psr-0": {"Old_": "old/"}}}"#,
        )]);
        let mut own = c.own_namespaces();
        own.sort();
        assert_eq!(own, ["app\\", "old_", "tests\\"]);
    }
}
