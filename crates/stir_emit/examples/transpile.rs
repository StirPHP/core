//! Transpiles one `.sphp` file (with stubs + stdlib loaded) and prints the PHP.
//! Usage: cargo run -p stir_emit --example transpile -- <file.sphp> [--inline] [--runtime]
use stir_db::{FileOrigin, RootDatabase, SourceFile, Workspace};
use stir_syntax::Dialect;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = std::path::PathBuf::from(&args[1]);
    let inline = args.iter().any(|a| a == "--inline");
    let runtime = args.iter().any(|a| a == "--runtime");
    let db = RootDatabase::default();
    let text = std::fs::read_to_string(&path).unwrap();
    let file = SourceFile::new(&db, path.clone(), text, Dialect::Stir, FileOrigin::Source);
    let mut files = vec![file];
    files.extend(stir_stubs::stub_files().map(|s| {
        SourceFile::new(
            &db,
            format!("stubs://{}", s.path).into(),
            s.text.to_owned(),
            Dialect::Php,
            FileOrigin::Stub,
        )
    }));
    files.extend(stir_stubs::stdlib_files().map(|s| {
        SourceFile::new(
            &db,
            format!("stdlib://{}", s.path).into(),
            s.text.to_owned(),
            Dialect::Stir,
            FileOrigin::Stub,
        )
    }));
    let ws = Workspace::new(&db, files, stir_db::Settings::default());
    for d in stir_checker::check_file(&db, ws, file).iter() {
        eprintln!("diagnostic {}: {} at {:?}", d.code, d.message, d.range);
    }
    let opts = stir_emit::EmitOptions {
        inline_extensions: inline,
        runtime_require: Some("__DIR__ . '/stir-runtime.php'".into()),
        ..Default::default()
    };
    if runtime {
        print!("{}", stir_emit::runtime(&db, ws, &opts).code);
    } else {
        print!("{}", stir_emit::transpile(&db, ws, file, &opts).code);
    }
}
