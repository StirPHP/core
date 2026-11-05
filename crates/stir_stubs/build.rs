//! Generates `$OUT_DIR/stubs.rs`: one `include_str!` entry per `.php` file in
//! each embedded extension directory, sorted for deterministic output.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

include!("src/extensions.rs");

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("cannot read stub directory {}: {e}", dir.display()));
    for entry in entries {
        let path = entry.expect("read_dir entry").path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "php") {
            out.push(path);
        }
    }
}

fn main() {
    let manifest =
        PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let root = manifest.join("phpstorm-stubs");
    println!("cargo::rerun-if-changed=src/extensions.rs");
    if !root.join("Core").is_dir() {
        panic!(
            "{} is empty; run `git submodule update --init --depth 1 crates/stir_stubs/phpstorm-stubs`",
            root.display()
        );
    }

    let mut code = String::from("static FILES: &[StubFile] = &[\n");
    for ext in EXTENSIONS {
        let dir = root.join(ext);
        println!("cargo::rerun-if-changed={}", dir.display());
        let mut files = Vec::new();
        collect(&dir, &mut files);
        files.sort();
        for file in files {
            println!("cargo::rerun-if-changed={}", file.display());
            let rel = file
                .strip_prefix(&root)
                .expect("stub under root")
                .to_str()
                .expect("UTF-8 stub path")
                .replace('\\', "/");
            let abs = file.to_str().expect("UTF-8 stub path");
            writeln!(
                code,
                "    StubFile {{ extension: {ext:?}, path: {rel:?}, text: include_str!({abs:?}) }},"
            )
            .unwrap();
        }
    }
    code.push_str("];\n");

    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR")).join("stubs.rs");
    std::fs::write(out, code).expect("write stubs.rs");
}
