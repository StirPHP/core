//! Every embedded stub must parse losslessly and without syntax errors.

use stir_syntax::{Dialect, parse};

fn line_of(text: &str, offset: usize) -> usize {
    text[..offset.min(text.len())].matches('\n').count() + 1
}

#[test]
fn all_stubs_parse_cleanly() {
    let mut lossy = Vec::new();
    let mut errors = Vec::new();
    let mut files = 0;
    for stub in stir_stubs::stub_files() {
        files += 1;
        let parsed = parse(stub.text, Dialect::Php);
        if parsed.syntax().to_string() != stub.text {
            lossy.push(stub.path);
        }
        for err in parsed.errors() {
            let offset = u32::from(err.range.start()) as usize;
            errors.push(format!(
                "{}:{}: {}",
                stub.path,
                line_of(stub.text, offset),
                err.message
            ));
        }
    }
    if !lossy.is_empty() || !errors.is_empty() {
        let mut report = format!(
            "{files} stub files; {} not lossless, {} syntax errors\n",
            lossy.len(),
            errors.len()
        );
        for path in &lossy {
            report.push_str(&format!("not lossless: {path}\n"));
        }
        for e in &errors {
            report.push_str(e);
            report.push('\n');
        }
        panic!("{report}");
    }
}

#[test]
fn well_known_symbols_are_present() {
    let ext_contains = |exts: &[&str], needle: &str| {
        stir_stubs::stub_files()
            .filter(|f| exts.contains(&f.extension))
            .any(|f| f.text.contains(needle))
    };
    assert!(ext_contains(&["Core", "standard"], "function strlen("));
    assert!(ext_contains(&["standard"], "function array_map("));
    assert!(ext_contains(&["SPL"], "class ArrayObject"));
    assert!(ext_contains(&["json"], "function json_encode("));
    assert!(ext_contains(&["Core"], "interface Traversable"));
    assert!(ext_contains(&["date"], "class DateTimeImmutable"));
    assert!(ext_contains(&["pcre"], "function preg_match("));
    assert!(ext_contains(&["mbstring"], "function mb_strlen("));
    assert!(ext_contains(&["PDO"], "class PDO"));
}

#[test]
fn parser_reports_errors_on_broken_input() {
    // Guards against the parse-all test passing vacuously.
    assert!(!parse("<?php function (", Dialect::Php).errors().is_empty());
}
