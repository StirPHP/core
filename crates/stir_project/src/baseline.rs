//! Our own baseline: `sphp check --generate-baseline` writes what's reported now to a NEON file
//! of `ignoreErrors` entries, by identifier, path and count, never by message.

use std::collections::BTreeMap;
use std::path::Path;

/// Where a baseline goes by default: next to the configuration.
pub const DEFAULT_NAME: &str = "stirphp-baseline.neon";

/// The baseline for how many times each `(identifier, path)` is reported. Paths are relative to
/// the baseline's folder. The order of the map makes the same errors give the same bytes.
pub fn render(entries: &BTreeMap<(String, String), u32>) -> String {
    if entries.is_empty() {
        return "parameters:\n\tignoreErrors: []\n".to_owned();
    }
    let mut out = String::from("parameters:\n\tignoreErrors:\n");
    for ((identifier, path), count) in entries {
        out.push_str(&format!(
            "\t\t-\n\t\t\tidentifier: {}\n\t\t\tcount: {count}\n\t\t\tpath: {}\n",
            neon_string(identifier),
            neon_string(path),
        ));
    }
    out
}

/// A string as NEON reads it back, with `%` not read as a parameter.
pub fn neon_string(s: &str) -> String {
    let s = s.replace('%', "%%");
    let plain = s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '-' | '%'))
        && s.starts_with(|c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/'))
        && s.parse::<f64>().is_err()
        && !matches!(
            s.to_ascii_lowercase().as_str(),
            "true" | "false" | "yes" | "no" | "on" | "off" | "null"
        );
    if plain {
        s
    } else {
        format!("'{}'", s.replace('\'', "''"))
    }
}

/// `path` as seen from `dir`, both absolute, with `/` as the separator.
pub fn relative_to(path: &Path, dir: &Path) -> String {
    let relative = pathdiff::diff_paths(path, dir).unwrap_or_else(|| path.to_path_buf());
    if relative.is_absolute() {
        return relative.to_string_lossy().into_owned();
    }
    relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_what_neon_reads_back() {
        let entries = BTreeMap::from([
            (("a.b".to_owned(), "src/a b.php".to_owned()), 2),
            (("a.b".to_owned(), "../it's/100%.php".to_owned()), 1),
            (("SPHP0060".to_owned(), "1.5".to_owned()), 3),
            (("a.b".to_owned(), "null".to_owned()), 1),
        ]);
        let text = render(&entries);
        let tmp = tempfile::TempDir::new().unwrap();
        let file = tmp.path().join("b.neon");
        std::fs::write(&file, &text).unwrap();
        let config = stir_neon::phpstan::PhpstanConfig::load(&file).unwrap();
        let read: BTreeMap<(String, String), u32> = config
            .ignore_errors
            .iter()
            .map(|e| {
                let path = e.paths[0]
                    .strip_prefix(tmp.path().canonicalize().unwrap())
                    .map_or_else(
                        |_| e.paths[0].display().to_string(),
                        |p| p.display().to_string(),
                    );
                ((e.identifiers[0].clone(), path), e.count.unwrap())
            })
            .collect();
        assert_eq!(read.len(), 4, "{text}");
        assert_eq!(read[&("a.b".to_owned(), "src/a b.php".to_owned())], 2);
        assert_eq!(read[&("SPHP0060".to_owned(), "1.5".to_owned())], 3);
        assert_eq!(read[&("a.b".to_owned(), "null".to_owned())], 1);
        assert!(text.ends_with('\n'));
    }

    #[test]
    fn nothing_to_hide_is_still_a_config() {
        assert_eq!(
            render(&BTreeMap::new()),
            "parameters:\n\tignoreErrors: []\n"
        );
    }

    #[test]
    fn paths_are_relative_to_the_baseline() {
        assert_eq!(
            relative_to(Path::new("/p/src/a.php"), Path::new("/p")),
            "src/a.php"
        );
        assert_eq!(
            relative_to(Path::new("/p/src/a.php"), Path::new("/p/conf")),
            "../src/a.php"
        );
    }
}
