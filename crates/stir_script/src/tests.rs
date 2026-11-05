use super::*;

fn run(text: &str) -> Result<Dynamic, ScriptError> {
    let engine = sandbox();
    let ast = compile(&engine, "test.rhai", text)?;
    call(&engine, &ast, "main", Vec::new())
}

fn message(text: &str) -> String {
    run(text).unwrap_err().message
}

#[test]
fn runs_functions_with_what_they_are_given() {
    let engine = sandbox();
    let ast = compile(&engine, "t", "fn add(a, b) { a + b }").unwrap();
    let got = call(&engine, &ast, "add", vec![1_i64.into(), 2_i64.into()]).unwrap();
    assert_eq!(got.as_int(), Ok(3));
}

#[test]
fn a_loop_that_never_ends_fails_the_call() {
    let m = message("fn main() { loop {} }");
    assert!(m.contains("operations"), "{m}");
}

#[test]
fn recursion_without_end_fails_the_call() {
    let error = run("fn main() { f(1) } fn f(n) { f(n + 1) }").unwrap_err();
    assert!(error.message.contains("too deep"), "{error:?}");
}

/// The most stack a script can ask for: every call as far down in expressions as it can be.
fn deepest_script() -> String {
    let engine = sandbox();
    (1..MAX_FN_EXPR_DEPTH)
        .rev()
        .map(|depth| {
            let open = "1 + (".repeat(depth);
            let close = ")".repeat(depth);
            format!("fn main() {{ f(1) }} fn f(n) {{ {open} f(n + 1) {close} }}")
        })
        .find(|text| compile(&engine, "t", text).is_ok())
        .unwrap()
}

#[test]
fn the_deepest_recursion_fits_the_stack_of_a_small_thread() {
    let text = deepest_script();
    let run_small = move || {
        let error = run(&text).unwrap_err();
        assert!(error.message.contains("too deep"), "{error:?}");
    };
    std::thread::Builder::new()
        .stack_size(256 * 1024)
        .spawn(run_small)
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn the_stack_a_script_is_sure_of_is_enough() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static LOWEST: AtomicUsize = AtomicUsize::new(usize::MAX);
    let mut engine = sandbox();
    engine.register_fn("probe", || {
        LOWEST.fetch_min(stacker::remaining_stack().unwrap(), Ordering::Relaxed);
    });
    let deepest = |shape: &dyn Fn(&str, &str) -> String| {
        (1..MAX_FN_EXPR_DEPTH)
            .rev()
            .map(|depth| shape(&"1 + (".repeat(depth), &")".repeat(depth)))
            .find(|text| compile(&engine, "t", text).is_ok())
            .unwrap()
    };
    let last = MAX_CALL_LEVELS as i64 - 3;
    let direct = |open: &str, close: &str| {
        format!(
            "fn main() {{ f(1) }} fn f(n) {{ if n > {last} {{ probe(); 0 }} else {{ {open} f(n + 1) {close} }} }}"
        )
    };
    let callbacks = |open: &str, close: &str| {
        format!(
            "fn main() {{ f(1) }} fn f(n) {{ if n > {} {{ probe(); [0] }} else {{ [{open} n {close}].map(|x| f(x)) }} }}",
            last / 2
        )
    };
    for text in [deepest(&direct), deepest(&callbacks)] {
        let ast = compile(&engine, "t", &text).unwrap();
        with_stack(|| {
            LOWEST.store(usize::MAX, Ordering::Relaxed);
            let start = stacker::remaining_stack().unwrap();
            let _ = call(&engine, &ast, "main", Vec::new()).unwrap();
            let used = start - LOWEST.load(Ordering::Relaxed);
            assert!(used < SCRIPT_STACK / 2, "{} KiB: {text}", used / 1024);
        });
    }
}

#[test]
fn recursion_through_callbacks_counts_too() {
    let text = "fn main() { f(1) } fn f(n) { [n].map(|x| f(x + 1)) }";
    let error = run(text).unwrap_err();
    assert!(error.message.contains("too deep"), "{error:?}");
}

#[test]
fn values_can_only_grow_so_far() {
    let m = message(r#"fn main() { let s = "x"; loop { s += s; } }"#);
    assert!(m.contains("too large"), "{m}");
    let m = message("fn main() { let a = [0]; loop { a += a; } }");
    assert!(m.contains("too large"), "{m}");
    let m =
        message(r#"fn main() { let m = #{}; let i = 0; loop { m[i.to_string()] = 1; i += 1; } }"#);
    assert!(m.contains("too large") || m.contains("operations"), "{m}");
    let m = message("fn main() { let a = []; a.pad(9999999999, 0); }");
    assert!(m.contains("too large"), "{m}");
    let m = message(r#"fn main() { "x".pad(9999999999, "y") }"#);
    assert!(m.contains("too large"), "{m}");
}

#[test]
fn nesting_is_limited_when_compiling() {
    let depth = MAX_FN_EXPR_DEPTH + 10;
    let text = format!(
        "fn main() {{ {}1{} }}",
        "1 + (".repeat(depth),
        ")".repeat(depth)
    );
    let error = run(&text).unwrap_err();
    assert!(error.message.contains("syntax error"), "{error:?}");
    // the parser mustn't overflow on what it's about to refuse
    let text = format!(
        "fn main() {{ {}1{} }}",
        "[".repeat(100_000),
        "]".repeat(100_000)
    );
    assert!(run(&text).is_err());
    let text = format!("fn main() {{}} {}", "x".repeat(MAX_SCRIPT_SIZE));
    assert!(message(&text).contains("larger"));
}

#[test]
fn a_syntax_error_says_where() {
    let error = run("fn main() {\n  let = 1;\n}").unwrap_err();
    assert!(error.message.contains("syntax error"), "{error:?}");
    assert_eq!(error.line, Some(2));
}

#[test]
fn no_way_to_load_or_evaluate_code() {
    for text in [
        r#"import "x" as y; fn main() {}"#,
        r#"fn main() { import "x" as y; }"#,
        r#"fn main() { eval("1 + 1") }"#,
        r#"export const X = 1; fn main() {}"#,
    ] {
        assert!(run(text).is_err(), "{text}");
    }
    for text in [
        r#"fn main() { Fn("eval").call("1 + 1") }"#,
        r#"fn main() { call(Fn("eval"), "1 + 1") }"#,
        r#"fn main() { "1 + 1".eval() }"#,
    ] {
        assert!(run(text).is_err(), "{text}");
    }
}

#[test]
fn no_clock_randomness_or_way_out() {
    for text in [
        "fn main() { timestamp() }",
        "fn main() { rand() }",
        "fn main() { open_file(\"/etc/passwd\") }",
        "fn main() { sleep(1) }",
        "fn main() { sleep(0.5) }",
        r#"fn main() { Fn("sleep").call(1) }"#,
        "fn main() { exit() }",
        "fn main() { exit(0) }",
        r#"fn main() { Fn("exit").call() }"#,
    ] {
        assert!(run(text).is_err(), "{text}");
    }
}

#[test]
fn printing_goes_nowhere_a_person_sees() {
    // the engine's own handlers would write to stdout, which the language server talks on
    assert!(run(r#"fn main() { print("x"); debug("y"); 1 }"#).is_ok());
}

#[test]
fn nothing_outlives_a_call() {
    // there's nothing to keep state in: functions see only their arguments
    assert!(run("let kept = 1; fn main() { kept }").is_err());
    assert!(run("fn main() { kept = 1; }").is_err());
    let engine = sandbox();
    let ast = compile(&engine, "t", "fn main(m) { m.x = 1; m.len() }").unwrap();
    for _ in 0..2 {
        let got = call(&engine, &ast, "main", vec![rhai::Map::new().into()]);
        assert_eq!(got.unwrap().as_int(), Ok(1));
    }
}

#[test]
fn the_top_level_never_runs() {
    let error = run("print(\"hi\");\nfn main() {}").unwrap_err();
    assert!(error.message.contains("only functions"), "{error:?}");
    let error = run("loop {}\nfn main() {}").unwrap_err();
    assert!(error.message.contains("only functions"), "{error:?}");
}

#[test]
fn an_engine_is_shared_between_threads() {
    let engine = Arc::new(sandbox());
    let ast = compile(&engine, "t", "fn f(n) { n * 2 }").unwrap();
    let threads: Vec<_> = (0..4_i64)
        .map(|i| {
            let (engine, ast) = (engine.clone(), ast.clone());
            std::thread::spawn(move || call(&engine, &ast, "f", vec![i.into()]).unwrap().as_int())
        })
        .collect();
    for (i, t) in threads.into_iter().enumerate() {
        assert_eq!(t.join().unwrap(), Ok(i as i64 * 2));
    }
}

#[test]
fn errors_know_their_offset() {
    let e = ScriptError {
        message: String::new(),
        line: Some(2),
        column: Some(3),
    };
    assert_eq!(e.offset_in("ab\ncd\n"), 5);
    assert_eq!(ScriptError::new("x").offset_in("ab\n"), 0);
}
