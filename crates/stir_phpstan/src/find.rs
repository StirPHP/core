//! Which PHPStan to run, if any, without running anything or installing anything. A global
//! `phpstan` is only used if someone names it: what's on a machine isn't necessarily what a project
//! is analysed with.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use stir_neon::phpstan::PhpstanRun;
use stir_project::PHPSTAN_CONFIG_FILES;

/// What a run gets unless it's told otherwise. A cold run took 6 seconds on Laravel's framework, so
/// this is only for what's stuck, and for very large projects: killed, a run leaves no result cache
/// behind, so one too short never gets anywhere.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);
/// PHP's memory limit, per process. PHP's own default of 128M isn't enough for a project of any
/// size, and `-1` lets a background process take all there is, unnoticed. The largest process on
/// Laravel's framework took about 420M.
const DEFAULT_MEMORY_LIMIT: &str = "2G";

/// How much someone wants PHPStan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wanted {
    /// If the project has it set up, and the settings don't say otherwise.
    IfSetUp,
    /// Asked for by name: what's missing is an error, whatever the settings say.
    Required,
}

/// A command that runs PHPStan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub program: PathBuf,
    /// Comes before everything else, when `program` is `php` and this is what it runs.
    pub script: Option<PathBuf>,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// The configuration PHPStan will read, whether it's told to or finds it itself.
    pub config_file: Option<PathBuf>,
    pub timeout: Duration,
}

impl Invocation {
    /// All the arguments.
    pub(crate) fn argv(&self) -> Vec<OsString> {
        let script = self.script.iter().map(|s| s.as_os_str().to_owned());
        script.chain(self.args.iter().map(OsString::from)).collect()
    }

    /// The file that gets executed: for asking whether that's all right.
    pub fn executed_file(&self) -> &Path {
        self.script.as_deref().unwrap_or(&self.program)
    }

    /// The command as a person would type it, in the project's folder.
    pub fn shown(&self) -> String {
        let short = |p: &Path| p.strip_prefix(&self.cwd).unwrap_or(p).display().to_string();
        let mut parts = vec![short(&self.program)];
        parts.extend(self.script.iter().map(|s| short(s)));
        parts.extend(
            self.args
                .iter()
                .map(|arg| match self.config_file.as_deref() {
                    Some(file) if arg == &file.display().to_string() => short(file),
                    _ => arg.clone(),
                }),
        );
        parts.join(" ")
    }
}

/// What came of looking for PHPStan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Located {
    /// There's nothing to run, and nobody has asked for it.
    Off,
    /// It was asked for and can't be run, for this reason.
    Missing(String),
    Found(Invocation),
}

/// Looks for PHPStan for the project in `root`, whose Composer packages are in `vendor_dir`.
pub fn locate(root: &Path, vendor_dir: &Path, run: &PhpstanRun, wanted: Wanted) -> Located {
    let required = wanted == Wanted::Required;
    if run.enabled == Some(false) && !required {
        return Located::Off;
    }
    let named = run.enabled == Some(true) || run.binary.is_some();
    let required = required || named;
    let missing = |why: String| {
        if required {
            Located::Missing(why)
        } else {
            Located::Off
        }
    };

    let config_file = match &run.configuration {
        Some(file) => {
            let file = root.join(file);
            if !file.is_file() {
                return Located::Missing(format!(
                    "PHPStan's configuration `{}` doesn't exist.",
                    file.display()
                ));
            }
            Some(file)
        }
        None => PHPSTAN_CONFIG_FILES
            .iter()
            .map(|name| root.join(name))
            .find(|p| p.is_file()),
    };
    let Some(config_file) = config_file else {
        return missing(format!(
            "PHPStan has no configuration to run with: `{}` has no `phpstan.neon`, and none is set.",
            root.display()
        ));
    };

    let (program, script) = match &run.binary {
        Some(binary) => match named_binary(root, binary) {
            Ok(found) => found,
            Err(why) => return Located::Missing(why),
        },
        None => match project_binary(root, vendor_dir) {
            Some(binary) => runnable(binary),
            None => {
                return missing(format!(
                    "PHPStan isn't installed in `{}`: there's no `{}`. Run `composer require --dev phpstan/phpstan`, or name a binary in the settings.",
                    root.display(),
                    vendor_dir.join("bin/phpstan").display()
                ));
            }
        },
    };

    let mut args = vec!["analyse".to_owned()];
    // only if it's been named: otherwise PHPStan finds what it'd find when run by hand
    if run.configuration.is_some() {
        args.push("--configuration".to_owned());
        args.push(config_file.display().to_string());
    }
    if let Some(level) = &run.level {
        args.push(format!("--level={level}"));
    }
    let memory = run.memory_limit.as_deref().unwrap_or(DEFAULT_MEMORY_LIMIT);
    args.push(format!("--memory-limit={memory}"));
    args.push("--error-format=json".to_owned());
    args.push("--no-progress".to_owned());
    Located::Found(Invocation {
        program,
        script,
        args,
        cwd: root.to_owned(),
        config_file: Some(config_file),
        timeout: run.timeout.map_or(DEFAULT_TIMEOUT, Duration::from_secs),
    })
}

/// A binary someone named: a path, or a bare name to look for on the `PATH`.
fn named_binary(root: &Path, binary: &Path) -> Result<(PathBuf, Option<PathBuf>), String> {
    let is_command = binary.components().count() == 1 && !root.join(binary).exists();
    if is_command {
        return Ok((binary.to_owned(), None));
    }
    let path = root.join(binary);
    if path.is_file() {
        Ok(runnable(path))
    } else {
        Err(format!(
            "The PHPStan binary `{}` doesn't exist.",
            path.display()
        ))
    }
}

/// Composer's link to PHPStan, or the package's own script if links are off.
fn project_binary(root: &Path, vendor_dir: &Path) -> Option<PathBuf> {
    let bin_dir = composer_bin_dir(root).unwrap_or_else(|| vendor_dir.join("bin"));
    [
        bin_dir.join("phpstan"),
        vendor_dir.join("phpstan/phpstan/phpstan"),
    ]
    .into_iter()
    .find(|p| p.is_file())
}

/// `config.bin-dir` of `composer.json`.
fn composer_bin_dir(root: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(root.join("composer.json")).ok()?;
    let manifest: serde_json::Value = serde_json::from_str(&text).ok()?;
    let dir = manifest.get("config")?.get("bin-dir")?.as_str()?;
    Some(root.join(dir))
}

/// How to run a script. A `.phar` or a checkout without the executable bit goes through `php`.
#[cfg(unix)]
fn runnable(path: PathBuf) -> (PathBuf, Option<PathBuf>) {
    use std::os::unix::fs::PermissionsExt as _;
    let executable = path
        .metadata()
        .is_ok_and(|m| m.permissions().mode() & 0o111 != 0);
    if executable {
        (path, None)
    } else {
        (PathBuf::from("php"), Some(path))
    }
}

/// How to run a script. Composer leaves a `.bat` next to its link; anything else goes through `php`.
#[cfg(not(unix))]
fn runnable(path: PathBuf) -> (PathBuf, Option<PathBuf>) {
    let bat = path.with_extension("bat");
    if path
        .extension()
        .is_some_and(|e| e == "bat" || e == "cmd" || e == "exe")
    {
        (path, None)
    } else if bat.is_file() {
        (bat, None)
    } else {
        (PathBuf::from("php"), Some(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().to_owned();
        let vendor = root.join("vendor");
        (tmp, root, vendor)
    }

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    fn found(located: Located) -> Invocation {
        match located {
            Located::Found(invocation) => invocation,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn it_takes_both_a_configuration_and_a_binary_to_turn_itself_on() {
        let (_tmp, root, vendor) = project();
        let run = PhpstanRun::default();
        assert_eq!(locate(&root, &vendor, &run, Wanted::IfSetUp), Located::Off);
        touch(&vendor.join("bin/phpstan"));
        assert_eq!(locate(&root, &vendor, &run, Wanted::IfSetUp), Located::Off);
        touch(&root.join("phpstan.neon.dist"));
        let invocation = found(locate(&root, &vendor, &run, Wanted::IfSetUp));
        assert_eq!(invocation.program, vendor.join("bin/phpstan"));
        assert_eq!(invocation.cwd, root);
        assert_eq!(
            invocation.config_file.as_deref(),
            Some(root.join("phpstan.neon.dist").as_path())
        );
        // PHPStan finds its own configuration, as it would by hand
        assert_eq!(
            invocation.args,
            [
                "analyse",
                "--memory-limit=2G",
                "--error-format=json",
                "--no-progress"
            ]
        );
        // a configuration without a binary does nothing either
        std::fs::remove_file(vendor.join("bin/phpstan")).unwrap();
        assert_eq!(locate(&root, &vendor, &run, Wanted::IfSetUp), Located::Off);
    }

    #[test]
    fn what_is_asked_for_and_missing_says_so() {
        let (_tmp, root, vendor) = project();
        let on = PhpstanRun {
            enabled: Some(true),
            ..Default::default()
        };
        assert!(matches!(
            locate(&root, &vendor, &on, Wanted::IfSetUp),
            Located::Missing(_)
        ));
        touch(&root.join("phpstan.neon"));
        let Located::Missing(why) = locate(&root, &vendor, &on, Wanted::IfSetUp) else {
            panic!();
        };
        assert!(why.contains("isn't installed"), "{why}");
        // the command line asks by name, whatever the settings
        let off = PhpstanRun {
            enabled: Some(false),
            ..Default::default()
        };
        assert_eq!(locate(&root, &vendor, &off, Wanted::IfSetUp), Located::Off);
        assert!(matches!(
            locate(&root, &vendor, &off, Wanted::Required),
            Located::Missing(_)
        ));
        // never anything global, though one may well be on the PATH
        assert!(matches!(
            locate(&root, &vendor, &on, Wanted::Required),
            Located::Missing(_)
        ));
    }

    #[test]
    fn off_is_off() {
        let (_tmp, root, vendor) = project();
        touch(&vendor.join("bin/phpstan"));
        touch(&root.join("phpstan.neon"));
        let off = PhpstanRun {
            enabled: Some(false),
            ..Default::default()
        };
        assert_eq!(locate(&root, &vendor, &off, Wanted::IfSetUp), Located::Off);
    }

    #[test]
    fn settings_come_first() {
        let (_tmp, root, vendor) = project();
        touch(&vendor.join("bin/phpstan"));
        touch(&root.join("tools/phpstan"));
        touch(&root.join("ci/phpstan.neon"));
        let run = PhpstanRun {
            binary: Some(root.join("tools/phpstan")),
            configuration: Some("ci/phpstan.neon".into()),
            level: Some("max".into()),
            memory_limit: Some("1G".into()),
            timeout: Some(5),
            ..Default::default()
        };
        let invocation = found(locate(&root, &vendor, &run, Wanted::IfSetUp));
        assert_eq!(invocation.program, root.join("tools/phpstan"));
        assert_eq!(invocation.timeout, Duration::from_secs(5));
        let config = root.join("ci/phpstan.neon").display().to_string();
        assert_eq!(
            invocation.args,
            [
                "analyse",
                "--configuration",
                &config,
                "--level=max",
                "--memory-limit=1G",
                "--error-format=json",
                "--no-progress"
            ]
        );
        assert_eq!(
            invocation.shown(),
            "tools/phpstan analyse --configuration ci/phpstan.neon --level=max --memory-limit=1G --error-format=json --no-progress"
        );
    }

    #[test]
    fn a_bare_name_is_a_command_and_only_if_nothing_there_has_that_name() {
        let (_tmp, root, vendor) = project();
        touch(&root.join("phpstan.neon"));
        let run = PhpstanRun {
            binary: Some("phpstan".into()),
            ..Default::default()
        };
        assert_eq!(
            found(locate(&root, &vendor, &run, Wanted::IfSetUp)).program,
            Path::new("phpstan")
        );
        touch(&root.join("phpstan"));
        assert_eq!(
            found(locate(&root, &vendor, &run, Wanted::IfSetUp)).program,
            root.join("phpstan")
        );
        let gone = PhpstanRun {
            binary: Some("tools/gone".into()),
            ..Default::default()
        };
        assert!(matches!(
            locate(&root, &vendor, &gone, Wanted::IfSetUp),
            Located::Missing(_)
        ));
    }

    #[test]
    fn composer_can_put_its_links_elsewhere() {
        let (_tmp, root, vendor) = project();
        touch(&root.join("phpstan.neon"));
        touch(&root.join("bin/phpstan"));
        std::fs::write(
            root.join("composer.json"),
            r#"{"config": {"bin-dir": "bin"}}"#,
        )
        .unwrap();
        let run = PhpstanRun::default();
        assert_eq!(
            found(locate(&root, &vendor, &run, Wanted::IfSetUp)).program,
            root.join("bin/phpstan")
        );
    }

    #[test]
    fn links_can_be_off() {
        let (_tmp, root, vendor) = project();
        touch(&root.join("phpstan.neon"));
        touch(&vendor.join("phpstan/phpstan/phpstan"));
        let run = PhpstanRun::default();
        assert_eq!(
            found(locate(&root, &vendor, &run, Wanted::IfSetUp)).program,
            vendor.join("phpstan/phpstan/phpstan")
        );
    }

    #[cfg(unix)]
    #[test]
    fn what_is_not_executable_goes_through_php() {
        use std::os::unix::fs::PermissionsExt as _;
        let (_tmp, root, vendor) = project();
        touch(&root.join("phpstan.neon"));
        touch(&vendor.join("bin/phpstan"));
        std::fs::set_permissions(
            vendor.join("bin/phpstan"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        let invocation = found(locate(
            &root,
            &vendor,
            &PhpstanRun::default(),
            Wanted::IfSetUp,
        ));
        assert_eq!(invocation.program, Path::new("php"));
        assert_eq!(invocation.executed_file(), vendor.join("bin/phpstan"));
        assert_eq!(invocation.argv()[0], vendor.join("bin/phpstan").as_os_str());
    }
}
