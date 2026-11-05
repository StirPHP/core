//! Parses every `.php` file under a directory and reports files with errors.
//! Usage: cargo run --release --example corpus -- <dir> [max-reported]
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let max: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(20);
    let (mut files, mut bad, mut bytes) = (0usize, 0usize, 0usize);
    let start = std::time::Instant::now();
    for entry in walkdir::WalkDir::new(&args[1])
        .into_iter()
        .filter_map(Result::ok)
    {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "php") {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(path) else {
            continue;
        };
        files += 1;
        bytes += src.len();
        let parse = std::panic::catch_unwind(|| stir_syntax::parse(&src));
        let parse = match parse {
            Ok(p) => p,
            Err(_) => {
                bad += 1;
                println!("PANIC {}", path.display());
                continue;
            }
        };
        assert_eq!(
            parse.syntax().to_string(),
            src,
            "lossless: {}",
            path.display()
        );
        if let Some(err) = parse.errors().first() {
            bad += 1;
            if bad <= max {
                let off = u32::from(err.range.start()) as usize;
                let line = src[..off.min(src.len())].matches('\n').count() + 1;
                println!("{}:{}: {}", path.display(), line, err.message);
            }
        }
    }
    println!(
        "{files} files, {} MB, {bad} with errors, {:.2?}",
        bytes / 1_000_000,
        start.elapsed()
    );
}
