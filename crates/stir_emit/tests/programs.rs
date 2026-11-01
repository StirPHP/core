//! End-to-end programs: every `tests/programs/*.sphp` must type-check, is
//! transpiled with and without extension inlining (both snapshotted), and —
//! when `SPHP_TEST_PHP` names a docker image (e.g. `php:8.4-cli`) — is run with
//! real PHP and its stdout compared with `<name>.out`.

use std::path::{Path, PathBuf};
use std::process::Command;

use libtest_mimic::{Arguments, Failed, Trial};
use stir_db::{FileOrigin, RootDatabase, Settings, Severity, SourceFile, Workspace};
use stir_syntax::Dialect;

fn workspace(db: &RootDatabase, file: SourceFile) -> Workspace {
    let mut files = vec![file];
    files.extend(stir_stubs::stub_files().map(|s| {
        SourceFile::new(
            db,
            format!("stubs://{}", s.path).into(),
            s.text.to_owned(),
            Dialect::Php,
            FileOrigin::Stub,
        )
    }));
    files.extend(stir_stubs::stdlib_files().map(|s| {
        SourceFile::new(
            db,
            format!("stdlib://{}", s.path).into(),
            s.text.to_owned(),
            Dialect::Stir,
            FileOrigin::Stub,
        )
    }));
    Workspace::new(db, files, Settings::default())
}

fn run(path: &Path) -> Result<(), Failed> {
    let name = path.file_stem().unwrap().to_str().unwrap().to_owned();
    let db = RootDatabase::default();
    let text = std::fs::read_to_string(path).unwrap();
    let file = SourceFile::new(
        &db,
        path.to_path_buf(),
        text.clone(),
        Dialect::Stir,
        FileOrigin::Source,
    );
    let ws = workspace(&db, file);
    let errors: Vec<String> = stir_checker::check_file(&db, ws, file)
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| {
            let line = text[..u32::from(d.range.start()) as usize]
                .matches('\n')
                .count()
                + 1;
            format!("{line}: {} {}", d.code, d.message)
        })
        .collect();
    if !errors.is_empty() {
        return Err(format!("type errors:\n{}", errors.join("\n")).into());
    }
    let image = std::env::var("SPHP_TEST_PHP").ok();
    for inline in [false, true] {
        let opts = stir_emit::EmitOptions {
            inline_extensions: inline,
            runtime_require: Some("__DIR__ . '/stir-runtime.php'".into()),
            ..Default::default()
        };
        let out = stir_emit::transpile(&db, ws, file, &opts);
        let suffix = if inline { "inline" } else { "call" };
        insta::with_settings!({ snapshot_suffix => format!("{name}_{suffix}"), prepend_module_to_snapshot => false }, {
            insta::assert_snapshot!(out.code);
        });
        if let Some(image) = &image {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("main.php"), &out.code).unwrap();
            std::fs::write(
                dir.path().join("stir-runtime.php"),
                stir_emit::runtime(&db, ws, &opts).code,
            )
            .unwrap();
            let o = Command::new("docker")
                .args([
                    "run",
                    "--rm",
                    "-v",
                    &format!("{}:/w", dir.path().display()),
                    "-w",
                    "/w",
                    image,
                    "php",
                    "-d",
                    "error_reporting=-1",
                    "-d",
                    "display_errors=stderr",
                    "main.php",
                ])
                .output()
                .map_err(|e| format!("docker: {e}"))?;
            let stdout = String::from_utf8_lossy(&o.stdout);
            let stderr = String::from_utf8_lossy(&o.stderr);
            let expected = std::fs::read_to_string(path.with_extension("out")).unwrap_or_default();
            if !o.status.success() || stdout != expected || !stderr.trim().is_empty() {
                return Err(format!(
                    "[{suffix}] php run differs (exit {:?})\n--- stdout\n{stdout}\n--- expected\n{expected}\n--- stderr\n{stderr}\n--- php\n{}",
                    o.status.code(),
                    out.code
                )
                .into());
            }
        }
    }
    Ok(())
}

fn main() {
    let args = Arguments::from_args();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/programs");
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "sphp"))
        .collect();
    paths.sort();
    let trials = paths
        .into_iter()
        .map(|p| {
            Trial::test(
                p.file_stem().unwrap().to_string_lossy().into_owned(),
                move || run(&p),
            )
        })
        .collect();
    libtest_mimic::run(&args, trials).exit();
}
