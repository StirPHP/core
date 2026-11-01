//! The standard library itself must type-check cleanly.

use stir_db::{FileOrigin, RootDatabase, SourceFile, Workspace};
use stir_syntax::Dialect;

#[test]
#[ignore = "stdlib is being brought in line with the checker"]
fn stdlib_checks_cleanly() {
    let db = RootDatabase::default();
    let mut files: Vec<SourceFile> = stir_stubs::stub_files()
        .map(|s| {
            SourceFile::new(
                &db,
                format!("stubs://{}", s.path).into(),
                s.text.to_owned(),
                Dialect::Php,
                FileOrigin::Stub,
            )
        })
        .collect();
    let lib: Vec<SourceFile> = stir_stubs::stdlib_files()
        .map(|f| {
            SourceFile::new(
                &db,
                format!("stdlib/{}", f.path).into(),
                f.text.to_owned(),
                Dialect::Stir,
                FileOrigin::Source,
            )
        })
        .collect();
    files.extend(lib.iter().copied());
    let ws = Workspace::new(&db, files);
    let mut report = String::new();
    let mut count = 0;
    for f in &lib {
        let text = f.text(&db);
        for d in stir_checker::check_file(&db, ws, *f).iter() {
            count += 1;
            let line = text[..u32::from(d.range.start()) as usize]
                .matches('\n')
                .count()
                + 1;
            report.push_str(&format!(
                "{}:{line}: {} {}\n",
                f.path(&db).display(),
                d.code,
                d.message
            ));
        }
    }
    assert!(count == 0, "{count} diagnostics in the stdlib:\n{report}");
}
