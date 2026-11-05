//! What `phpstan analyse --error-format=json` says.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// One error of PHPStan's about a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reported {
    /// From 1. Some errors are about a file as a whole, such as an ignore rule nothing matched.
    pub line: Option<u32>,
    pub message: String,
    /// Older PHPStans, and some of its errors, have none. Those can't be recognized as the same
    /// error as anything.
    pub identifier: Option<String>,
    pub tip: Option<String>,
    pub ignorable: bool,
}

/// A run's output.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub files: BTreeMap<PathBuf, Vec<Reported>>,
    /// What's wrong with the run or the configuration as a whole, and isn't about a file.
    pub errors: Vec<String>,
}

/// Finds PHPStan's JSON in what it wrote to stdout. PHP warnings and whatever bootstrap files
/// `echo` can be on either side of it, so the output as a whole needn't be JSON. Relative paths are
/// relative to `cwd`.
pub fn parse(stdout: &str, cwd: &Path) -> Option<Report> {
    // no JSON string can have this in it unescaped, so it's the start of an object
    let mut search = stdout.len();
    while let Some(key) = stdout[..search].rfind("\"totals\"") {
        search = key;
        let Some(start) = stdout[..key].rfind('{') else {
            break;
        };
        if !stdout[start + 1..key].trim().is_empty() {
            continue;
        }
        let parsed = serde_json::Deserializer::from_str(&stdout[start..])
            .into_iter::<Value>()
            .next();
        if let Some(Ok(value)) = parsed
            && let Some(report) = from_value(&value, cwd)
        {
            return Some(report);
        }
    }
    None
}

fn from_value(value: &Value, cwd: &Path) -> Option<Report> {
    let mut report = Report::default();
    for error in value.get("errors")?.as_array()? {
        report.errors.push(error.as_str()?.to_owned());
    }
    for (key, file) in value.get("files")?.as_object()? {
        let (path, context) = split_context(key);
        let messages = file.get("messages")?.as_array()?;
        let reported = report.files.entry(cwd.join(path)).or_default();
        for message in messages {
            reported.push(reported_error(message, context)?);
        }
    }
    Some(report)
}

/// What's reported of a trait's code is under `path (in context of class Foo)`, once for each class
/// using it.
fn split_context(key: &str) -> (&str, Option<&str>) {
    match key.rsplit_once(" (in context of ") {
        Some((path, rest)) if rest.ends_with(')') => (path, rest.strip_suffix(')')),
        _ => (key, None),
    }
}

fn reported_error(value: &Value, context: Option<&str>) -> Option<Reported> {
    let mut message = value.get("message")?.as_str()?.to_owned();
    if let Some(context) = context {
        message.push_str(&format!(" (in context of {context})"));
    }
    let text = |key| value.get(key).and_then(Value::as_str).map(str::to_owned);
    Some(Reported {
        line: value
            .get("line")
            .and_then(Value::as_u64)
            .and_then(|l| u32::try_from(l).ok()),
        message,
        identifier: text("identifier").filter(|i| !i.is_empty()),
        tip: text("tip").map(|t| plain_tip(&t)),
        ignorable: value.get("ignorable").and_then(Value::as_bool) == Some(true),
    })
}

/// Tips are written for PHPStan's terminal: `<fg=cyan>this</>`, and a placeholder of what it's
/// running with.
fn plain_tip(tip: &str) -> String {
    let mut out = String::with_capacity(tip.len());
    let mut rest = tip;
    while let Some(open) = rest.find('<') {
        let (before, after) = rest.split_at(open);
        out.push_str(before);
        let tag = after
            .find('>')
            .filter(|_| after.starts_with("</>") || after.starts_with("<fg="));
        match tag {
            Some(end) => rest = &after[end + 1..],
            None => {
                out.push('<');
                rest = &after[1..];
            }
        }
    }
    out.push_str(rest);
    out.replace("%configurationFile%", "your configuration")
}

/// Did what's in a report's `errors` stop PHPStan from analysing everything, as opposed to being a
/// complaint about the configuration? Then what's missing from the report says nothing.
pub fn is_incomplete(error: &str) -> bool {
    error.starts_with("Child process error") || error.starts_with("Internal error")
}

/// Did PHP run out of memory, by what's said?
pub fn is_memory(output: &str) -> bool {
    output.contains("Allowed memory size of") || output.contains("configured PHP memory limit")
}

#[cfg(test)]
mod tests {
    use super::*;

    const CWD: &str = "/project";

    fn parsed(stdout: &str) -> Report {
        parse(stdout, Path::new(CWD)).unwrap()
    }

    /// PHPStan 2.3.1's output for a project with an error in a file, a tip, and a broken file.
    const REAL: &str = r#"{"totals":{"errors":0,"file_errors":3},"files":{"/project/src/Foo.php":{"errors":2,"messages":[{"message":"Method App\\Foo::a() should return int but returns string.","line":11,"ignorable":true,"identifier":"return.type"},{"message":"Function undefined_fn not found.","line":16,"ignorable":true,"tip":"Learn more at https://phpstan.org/user-guide/discovering-symbols","identifier":"function.notFound"}]},"/project/src/Bad.php":{"errors":1,"messages":[{"message":"Syntax error, unexpected '{', expecting T_VARIABLE on line 2","line":2,"ignorable":false,"identifier":"phpstan.parse"}]}},"errors":[]}"#;

    #[test]
    fn real_output() {
        let report = parsed(REAL);
        assert!(report.errors.is_empty());
        let foo = &report.files[Path::new("/project/src/Foo.php")];
        assert_eq!(foo.len(), 2);
        assert_eq!(foo[0].line, Some(11));
        assert_eq!(foo[0].identifier.as_deref(), Some("return.type"));
        assert!(foo[0].ignorable);
        assert_eq!(
            foo[1].tip.as_deref(),
            Some("Learn more at https://phpstan.org/user-guide/discovering-symbols")
        );
        let bad = &report.files[Path::new("/project/src/Bad.php")];
        assert!(!bad[0].ignorable);
    }

    #[test]
    fn errors_about_a_whole_file_and_the_whole_run() {
        // an ignore rule that matched nothing, as PHPStan 2.3.1 reports them
        let report = parsed(
            r##"{"totals":{"errors":1,"file_errors":1},"files":{"/project/src/Foo.php":{"errors":1,"messages":[{"message":"Ignored error pattern nope.nope in path /project/src/Foo.php was not matched in reported errors.","line":null,"ignorable":false,"identifier":"ignore.unmatched"}]}},"errors":["Ignored error pattern #this never matches# was not matched in reported errors."]}"##,
        );
        assert_eq!(
            report.files[Path::new("/project/src/Foo.php")][0].line,
            None
        );
        assert_eq!(report.errors.len(), 1);
        assert!(!is_incomplete(&report.errors[0]));
    }

    #[test]
    fn a_crashed_worker_is_reported_in_json() {
        let report = parsed(
            r#"{"totals":{"errors":1,"file_errors":0},"files":{},"errors":["Child process error: PHPStan process crashed because it reached configured PHP memory limit: 40M\nIncrease your memory limit in php.ini or run PHPStan with --memory-limit CLI option.\n while running parallel worker"]}"#,
        );
        assert!(is_incomplete(&report.errors[0]));
        assert!(is_memory(&report.errors[0]));
    }

    #[test]
    fn traits_are_reported_per_class_using_them() {
        let report = parsed(
            r#"{"totals":{"errors":0,"file_errors":2},"files":{
                "/project/src/Foo.php (in context of class App\\UsesT)":{"errors":1,"messages":[{"message":"Access to an undefined property App\\UsesT::$x.","line":21,"ignorable":true,"identifier":"property.notFound"}]},
                "/project/src/Foo.php (in context of class App\\UsesT2)":{"errors":1,"messages":[{"message":"Access to an undefined property App\\UsesT2::$x.","line":21,"ignorable":true,"identifier":"property.notFound"}]}
            },"errors":[]}"#,
        );
        assert_eq!(report.files.len(), 1);
        let foo = &report.files[Path::new("/project/src/Foo.php")];
        assert_eq!(foo.len(), 2);
        assert!(
            foo[0].message.ends_with("(in context of class App\\UsesT)"),
            "{}",
            foo[0].message
        );
    }

    #[test]
    fn garbage_around_the_json() {
        let garbage = format!(
            "PHP Warning:  Module already loaded in Unknown on line 0\nHELLO {{\"not\": 1}}\n{REAL}\nWarning: something after it {{\n"
        );
        assert_eq!(parsed(&garbage), parsed(REAL));
    }

    #[test]
    fn a_false_start_before_the_real_one() {
        let garbage = format!("{{\"totals\": oops\n{REAL}");
        assert_eq!(parsed(&garbage), parsed(REAL));
        // and the other way round: broken output after the real one
        let garbage = format!("{REAL}\n{{\"totals\":{{\"errors\"");
        assert_eq!(parsed(&garbage), parsed(REAL));
    }

    #[test]
    fn what_is_not_a_report() {
        for stdout in [
            "",
            "Level config file conf/config.level99.neon was not found.",
            "PHP Fatal error:  Allowed memory size of 26214400 bytes exhausted",
            r#"{"totals":{"errors":0}}"#,
            r#"{"totals":{},"files":[],"errors":[]}"#,
            r#"{"totals":{},"files":{},"errors":[1]}"#,
            r#"{"totals":{},"files":{"a.php":{"messages":[{"line":1}]}},"errors":[]}"#,
        ] {
            assert_eq!(parse(stdout, Path::new(CWD)), None, "{stdout}");
        }
    }

    #[test]
    fn no_identifier_and_a_relative_path() {
        let report = parsed(
            r#"{"totals":{},"files":{"src/A.php":{"messages":[{"message":"Old.","line":3,"ignorable":true}]}},"errors":[]}"#,
        );
        let a = &report.files[Path::new("/project/src/A.php")];
        assert_eq!(a[0].identifier, None);
        assert_eq!(a[0].tip, None);
    }

    #[test]
    fn tips_lose_what_is_only_for_a_terminal() {
        assert_eq!(
            plain_tip("Use <fg=cyan>a < b</> in %configurationFile%"),
            "Use a < b in your configuration"
        );
    }
}
