use stir_neon::{Key, Value, parse};

fn render(cases: &[&str]) -> String {
    let mut out = String::new();
    for src in cases {
        let r = match parse(src) {
            Ok(v) => format!("{v}"),
            Err(e) => format!("ERROR: {e}"),
        };
        out.push_str(&format!("{src:?}\n  => {r}\n"));
    }
    out
}

#[test]
fn scalars() {
    insta::assert_snapshot!(render(&[
        "",
        "# only a comment",
        "null",
        "NULL",
        "true",
        "yes",
        "On",
        "off",
        "No",
        "FALSE",
        "123",
        "-123",
        "+5",
        "1.5",
        "-1.5e3",
        ".5",
        "1e3",
        "0x1F",
        "0o17",
        "0b101",
        "0777",
        "99999999999999999999",
        "hello",
        "hello world",
        "hello  # comment",
        "foo#bar",
        "a:b",
        "http://example.com/path?x=1",
        "%currentWorkingDirectory%/src",
        "*/tests/*",
        "@service",
        "2024-01-15",
        "2024-01-15 10:20:30",
        "-foo",
        "::bar",
        "'single'",
        "'it''s'",
        "\"double\\n\\t\\\"q\\\" \\u00e9 \\x41 \\uD83D\\uDE00 \\/ \\_\"",
        "'no \\n escapes'",
        "\"bad \\q escape\"",
        "'unterminated",
    ]));
}

#[test]
fn inline_arrays() {
    insta::assert_snapshot!(render(&[
        "[]",
        "{}",
        "()",
        "[1, 2, 3]",
        "[1, 2, 3,]",
        "{a: 1, b: 2}",
        "{a=1, b=2}",
        "[a, b: c, d]",
        "{1: a, b}",
        "{0: a, 1: b}",
        "[\n  1\n  2\n  3\n]",
        "{\n  a: 1,\n  b: [x, y]\n}",
        "[a: ]",
        "{a: 1, a: 2}",
        "{true: 1, null: 2, 123: x, '456': y, 1.5: z}",
        "[1 2]",
        "[1, 2",
        "{[a]: b}",
    ]));
}

#[test]
fn entities() {
    insta::assert_snapshot!(render(&[
        "Foo()",
        "Foo(a, b: c)",
        "Foo(1)::bar(2)",
        "@foo::bar(1)::baz",
        "Column(type: string, length: 255)",
        "key: Foo(a)",
        "- Foo(\n    bar: 1\n  )",
    ]));
}

#[test]
fn blocks() {
    insta::assert_snapshot!(render(&[
        "a: 1\nb: 2",
        "a: 1\nb:\nc: 3",
        "a:\n  b: 1\n  c:\n    d: 2\ne: 3",
        "- a\n- b\n- c",
        "list:\n  - a\n  - b",
        "list:\n- a\n- b\nnext: 1",
        "- a: 1\n  b: 2\n- c: 3",
        "- - a\n  - b\n- c",
        "-\n  a: 1",
        "- \n- x",
        "key: [1, 2]\nother: {a: b}",
        "a: 1 # comment\n# full line comment\n\nb: 2",
        "  indented: 1\n  same: 2",
        "a=1\nb=2",
        "a: 1\n  b: 2",
        "a:\n  b: 1\n c: 2",
        "a:\n\tb: 1\n  c: 2",
        "a: 1 2",
        "a: 1\nb",
        "a: 1\na: 2",
        "x: hello world\ny: 'quoted' ",
        "- 1\n-2",
        "empty:\n  # comment only\nnext: 1",
        "a: ]",
    ]));
}

#[test]
fn multiline_strings() {
    insta::assert_snapshot!(render(&[
        "a: '''\n    line one\n      indented\n    line three\n'''",
        "a: \"\"\"\n  tab\\there\n  second\n  \"\"\"",
        "'''\n'''",
        "x: '''\n\n    after blank\n    '''\nnext: 1",
        "'''\nunterminated",
    ]));
}

#[test]
fn crlf_and_bom() {
    let v = parse("\u{feff}a: 1\r\nb:\r\n  - x\r\n").unwrap();
    assert_eq!(v.to_string(), "{\"a\": 1, \"b\": [\"x\"]}");
}

#[test]
fn error_positions() {
    let e = parse("a: 1\nb: [1, 2\nc: 3").unwrap_err();
    assert_eq!((e.line, e.column), (3, 5));
    assert_eq!(e.message, "Unexpected end");
    let e = parse("a:\n  b: 1\n    c: 2").unwrap_err();
    assert_eq!(e.message, "Bad indentation");
    assert_eq!(e.line, 3);
}

#[test]
fn accessors() {
    let v = parse("a:\n  b: [x, y]\n  c: 5\n  0: zero").unwrap();
    let a = v.get("a").unwrap();
    assert_eq!(a.get("b").unwrap().as_sequence().unwrap().len(), 2);
    assert_eq!(a.get("c").and_then(Value::as_int), Some(5));
    assert_eq!(a.get(0).and_then(Value::as_str), Some("zero"));
    assert_eq!(Key::from("12"), Key::Int(12));
    assert_eq!(Key::from("012"), Key::Str("012".into()));
    assert_eq!(Key::from("-3"), Key::Int(-3));
}
