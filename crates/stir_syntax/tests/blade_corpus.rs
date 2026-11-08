//! Blade templates of real projects, read from `corpus/`, the submodules that may not be there.
//! Those are only read: their code is never run.

use std::path::{Path, PathBuf};

use stir_syntax::{blade_directive_names, is_blade_path, parse_blade};

fn corpus(name: &str) -> Option<PathBuf> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus")
        .join(name);
    if path.read_dir().is_ok_and(|mut dir| dir.next().is_some()) {
        Some(path)
    } else {
        eprintln!("skipped: corpus/{name} isn't there");
        None
    }
}

fn templates(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(Result::ok)
        .map(walkdir::DirEntry::into_path)
        .filter(|path| is_blade_path(path) && path.is_file())
        .collect();
    found.sort();
    found
}

/// The templates with syntax errors, and those of each, having checked that nothing got lost.
fn errors_of(files: &[PathBuf]) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    for path in files {
        let src = std::fs::read_to_string(path).unwrap();
        let parse = parse_blade(&src);
        assert_eq!(parse.syntax().to_string(), src, "{}", path.display());
        if !parse.errors().is_empty() {
            out.push((path.clone(), parse.debug_tree()));
        }
    }
    out
}

#[test]
fn laravels_own_templates_parse() {
    let Some(framework) = corpus("framework") else {
        return;
    };
    let files = templates(&framework);
    assert!(files.len() >= 100, "{} templates", files.len());
    let (malformed, regular): (Vec<_>, Vec<_>) = files
        .into_iter()
        .partition(|path| path.to_string_lossy().contains("MalformedErrorViews"));
    let errors = errors_of(&regular);
    if let Some((path, tree)) = errors.first() {
        panic!(
            "{} of {} with errors, first {}:\n{tree}",
            errors.len(),
            regular.len(),
            path.display()
        );
    }
    assert!(!malformed.is_empty());
    errors_of(&malformed);
}

/// What the tests can't see for themselves: this reports, and doesn't decide.
#[test]
fn report_on_an_application() {
    let Some(app) = corpus("laravel-io") else {
        return;
    };
    for dir in ["resources/views", "vendor"] {
        let files = templates(&app.join(dir));
        let errors = errors_of(&files);
        eprintln!(
            "corpus/laravel-io/{dir}: {} templates, {} with syntax errors",
            files.len(),
            errors.len()
        );
        for (path, tree) in &errors {
            let first = tree.lines().find(|l| l.starts_with("error"));
            eprintln!("  {}: {}", path.display(), first.unwrap_or_default());
        }
    }
}

/// Every `compile*` method of Laravel's traits is in our table, or here to say why it isn't.
#[test]
fn every_directive_of_the_compiler_is_known() {
    /// What these compile isn't a directive: they're steps of the compiler that `method_exists`
    /// finds too, so `@comments` calls one by accident. As it's nothing anyone writes on purpose,
    /// such a name stays text here, and `@string` in a `<style>` isn't reported.
    const NOT_DIRECTIVES: &[&str] = &[
        "comments",
        "echos",
        "rawechos",
        "regularechos",
        "escapedechos",
        "classcomponentopening",
    ];
    let Some(framework) = corpus("framework") else {
        return;
    };
    let dir = framework.join("src/Illuminate/View/Compilers/Concerns");
    let known: Vec<&str> = blade_directive_names().collect();
    let mut seen = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let source = std::fs::read_to_string(entry.unwrap().path()).unwrap();
        for piece in source.split("function compile").skip(1) {
            let name: String = piece
                .chars()
                .take_while(char::is_ascii_alphanumeric)
                .collect::<String>()
                .to_ascii_lowercase();
            seen += 1;
            assert!(
                known.contains(&name.as_str()) || NOT_DIRECTIVES.contains(&name.as_str()),
                "@{name} is missing"
            );
        }
    }
    assert!(seen > 100, "{seen} methods");
}
