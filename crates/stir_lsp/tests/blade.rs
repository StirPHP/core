//! A template spoken about over JSON-RPC, as an editor would, with text before the cursor that takes
//! more bytes than it does UTF-16 units.

mod client;

use std::path::{Path, PathBuf};

use client::{Client, uri, write};
use serde_json::{Value, json};

const TEMPLATE: &str = "<h1>Grüße, 世界 🎉</h1>
@use('App\\Models\\Post')
<!-- ñandú 🎉 --> {{ $title }} {{ $title }}
<p>🎉</p> {{ $given->anything }}
@foreach ($posts as $post)
    <p>日本語</p> {{ $loop->iteration }} {{ $post->anything }}
    <p>日本語</p> @foreach ($post->comments as $comment) {{ $loop->parent->depth }} @endforeach
@endforeach
@php
    /** @var \\App\\Models\\Post $article */
    $article->publish();
    $local = 1;
    $local2 = $local + 1;
@endphp
<p>日本語</p> {{ $local2 }} {{ $article->title }}
<p>日本語</p> {{ new Post }} {{ helper() }}
<p>日本語</p> @include('partials.card', ['card' => $article->title])
<p>日本語</p> @error('title') {{ $message }} @enderror
{{ $errors->any() }}
";

/// The position of the `delta`-th character of the first `needle` in `text`, as UTF-16 positions.
fn position(text: &str, needle: &str, delta: usize) -> Value {
    let at = text.find(needle).unwrap_or_else(|| panic!("{needle}"));
    let at = at
        + needle
            .char_indices()
            .nth(delta)
            .map_or(needle.len(), |(i, _)| i);
    let before = &text[..at];
    let line = before.matches('\n').count();
    let start = before.rfind('\n').map_or(0, |i| i + 1);
    json!({"line": line, "character": before[start..].encode_utf16().count()})
}

struct Session {
    c: Client,
    root: PathBuf,
    path: PathBuf,
    uri: String,
    text: String,
    _tmp: tempfile::TempDir,
}

impl Session {
    async fn start(text: &str) -> Session {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        write(
            &root.join("stirphp.neon"),
            "parameters:\n    stirphp:\n        frameworks:\n            laravel: true\n",
        );
        write(
            &root.join("app/Models/Post.php"),
            "<?php\nnamespace App\\Models;\nclass Post {\n    public string $title = '';\n    public function publish(): void {}\n}\nfunction helper(): int { return 1; }\n",
        );
        write(
            &root.join("app/Factory.php"),
            "<?php\nnamespace Illuminate\\View;\nclass Factory {}\n",
        );
        write(
            &root.join("resources/views/partials/card.blade.php"),
            "{{ $card }}\n",
        );
        write(
            &root.join("resources/views/partials/badge.blade.php"),
            "{{ $badge }}\n",
        );
        let path = root.join("resources/views/show.blade.php");
        write(&path, text);
        let doc = uri(&path);
        let mut c = Client::start();
        c.request(
            "initialize",
            json!({"processId": null, "rootUri": uri(&root), "capabilities": {},
                   "initializationOptions": {"workspaceTrusted": true}}),
        )
        .await;
        c.notify("initialized", json!({})).await;
        c.notify(
            "textDocument/didOpen",
            json!({"textDocument": {"uri": doc, "languageId": "php", "version": 1, "text": text}}),
        )
        .await;
        Session {
            c,
            root,
            path,
            uri: doc,
            text: text.to_owned(),
            _tmp: tmp,
        }
    }

    async fn at(&mut self, method: &str, needle: &str, delta: usize) -> Value {
        let position = position(&self.text, needle, delta);
        self.c
            .request(
                method,
                json!({"textDocument": {"uri": self.uri}, "position": position}),
            )
            .await
    }

    async fn hover(&mut self, needle: &str, delta: usize) -> String {
        let got = self.at("textDocument/hover", needle, delta).await;
        got["contents"]["value"].as_str().unwrap_or("").to_owned()
    }

    async fn labels(&mut self, needle: &str, delta: usize) -> Vec<String> {
        let got = self.at("textDocument/completion", needle, delta).await;
        got["items"]
            .as_array()
            .cloned()
            .or(got.as_array().cloned())
            .unwrap_or_default()
            .iter()
            .map(|i| i["label"].as_str().unwrap().to_owned())
            .collect()
    }

    /// The text at a location's range.
    fn range_text(&self, range: &Value) -> String {
        slice(&self.text, range)
    }
}

fn slice(text: &str, range: &Value) -> String {
    let offset = |p: &Value| {
        let line = p["line"].as_u64().unwrap() as usize;
        let character = p["character"].as_u64().unwrap() as usize;
        let mut at = 0;
        for l in text.split_inclusive('\n').take(line) {
            at += l.len();
        }
        let mut units = 0;
        for (i, c) in text[at..].char_indices() {
            if units == character {
                return at + i;
            }
            units += c.len_utf16();
        }
        text.len()
    };
    text[offset(&range["start"])..offset(&range["end"])].to_owned()
}

fn is_under(uri_text: &Value, root: &Path, relative: &str) -> bool {
    uri_text == &json!(uri(&root.join(relative)))
}

#[tokio::test(flavor = "multi_thread")]
async fn hover_in_a_template() {
    let mut s = Session::start(TEMPLATE).await;
    // nothing says what it is, and it says so
    let open = s.hover("$title }}", 1).await;
    assert!(open.contains("$title"), "{open}");
    assert!(
        open.contains("Nothing in this template declares it"),
        "{open}"
    );
    assert!(!open.contains("mixed"), "{open}");

    // through a multi-byte `<p>`
    let in_loop = s.hover("$loop->iteration", 1).await;
    assert!(in_loop.contains("BladeLoop"), "{in_loop}");
    let member = s.hover("iteration }}", 1).await;
    assert!(member.contains("int $iteration"), "{member}");
    let depth = s.hover("depth }}", 1).await;
    assert!(depth.contains("int $depth"), "{depth}");

    // the `$post` of the `@foreach`
    let post = s.hover("$post)", 1).await;
    assert!(post.contains("$post"), "{post}");

    let stated = s.hover("$article->title", 1).await;
    assert!(stated.contains("Post $article"), "{stated}");
    let title = s.hover("title }}\n<p>日本語</p> {{ new", 1).await;
    assert!(title.contains("string $title"), "{title}");

    let message = s.hover("$message", 2).await;
    assert!(message.contains("string $message"), "{message}");
    assert!(message.contains("@error"), "{message}");

    // the `@foreach` isn't `$loop`
    assert_eq!(s.hover("@foreach ($posts", 2).await, "");

    let class = s.hover("Post }}", 1).await;
    assert!(class.contains("class Post"), "{class}");
    let used = s.hover("App\\Models\\Post')", 11).await;
    assert!(used.contains("class Post"), "{used}");
}

#[tokio::test(flavor = "multi_thread")]
async fn going_to_a_definition_in_a_template() {
    let mut s = Session::start(TEMPLATE).await;
    let def = s.at("textDocument/definition", "Post }}", 1).await;
    assert!(
        is_under(&def["uri"], &s.root, "app/Models/Post.php"),
        "{def}"
    );
    let def = s
        .at("textDocument/definition", "App\\Models\\Post')", 11)
        .await;
    assert!(
        is_under(&def["uri"], &s.root, "app/Models/Post.php"),
        "{def}"
    );

    // a variable goes to where it's first seen
    let def = s.at("textDocument/definition", "$title }}\n", 1).await;
    assert_eq!(def["range"]["start"]["character"], 21, "{def}");
    assert_eq!(s.range_text(&def["range"]), "$title", "{def}");

    let def = s.at("textDocument/definition", "$local + 1", 1).await;
    assert_eq!(s.range_text(&def["range"]), "$local", "{def}");
    assert_eq!(def["range"]["start"]["line"], 11, "{def}");

    // what `@foreach` declares goes to it
    let def = s.at("textDocument/definition", "$loop->iteration", 1).await;
    assert_eq!(s.range_text(&def["range"]), "@foreach", "{def}");
    let def = s.at("textDocument/definition", "$message", 1).await;
    assert_eq!(s.range_text(&def["range"]), "@error", "{def}");

    // to the comment that states it
    let def = s
        .at("textDocument/definition", "$article->publish", 1)
        .await;
    assert_eq!(s.range_text(&def["range"]), "$article", "{def}");

    // the name of a template
    let def = s.at("textDocument/definition", "partials.card", 3).await;
    assert!(
        is_under(
            &def["uri"],
            &s.root,
            "resources/views/partials/card.blade.php"
        ),
        "{def}"
    );
    let def = s
        .at("textDocument/typeDefinition", "$article->title", 1)
        .await;
    assert!(
        is_under(&def["uri"], &s.root, "app/Models/Post.php"),
        "{def}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn references_and_highlights_in_a_template() {
    let mut s = Session::start(TEMPLATE).await;
    let position = position(&s.text, "$title }}", 1);
    let refs =
        s.c.request(
            "textDocument/references",
            json!({"textDocument": {"uri": s.uri}, "position": position,
                   "context": {"includeDeclaration": true}}),
        )
        .await;
    let refs = refs.as_array().unwrap();
    assert_eq!(refs.len(), 2, "{refs:?}");
    for r in refs {
        assert_eq!(s.range_text(&r["range"]), "$title");
    }

    // a class is referred to from the template, by `@use` too
    let php = std::fs::read_to_string(s.root.join("app/Models/Post.php")).unwrap();
    let lib = uri(&s.root.join("app/Models/Post.php"));
    let at = position_of_class(&php);
    let refs =
        s.c.request(
            "textDocument/references",
            json!({"textDocument": {"uri": lib}, "position": at,
                   "context": {"includeDeclaration": false}}),
        )
        .await;
    let here: Vec<String> = refs
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["uri"] == json!(s.uri))
        .map(|r| s.range_text(&r["range"]))
        .collect();
    assert!(here.contains(&"App\\Models\\Post".to_owned()), "{here:?}");

    let highlights = s
        .at("textDocument/documentHighlight", "$local + 1", 1)
        .await;
    assert_eq!(highlights.as_array().unwrap().len(), 2, "{highlights}");
}

fn position_of_class(php: &str) -> Value {
    position(php, "Post {", 1)
}

#[tokio::test(flavor = "multi_thread")]
async fn renaming_in_a_template() {
    let mut s = Session::start(TEMPLATE).await;
    // what comes from outside isn't this file's to rename, nor is what a directive declares
    for (needle, delta) in [("$title }}", 1), ("$loop->iteration", 1), ("$message", 1)] {
        let position = position(&s.text, needle, delta);
        let params =
            json!({"textDocument": {"uri": s.uri}, "position": position, "newName": "$other"});
        let got =
            s.c.send_request_for_error("textDocument/rename", params.clone())
                .await;
        assert!(got.is_some(), "{needle}");
        let got =
            s.c.send_request_for_error("textDocument/prepareRename", params)
                .await;
        assert!(got.is_some(), "{needle}");
    }
    // nor, here, what `@include` passes on by its name
    let position_local = position(&s.text, "$local + 1", 1);
    let got = s
        .c
        .send_request_for_error(
            "textDocument/rename",
            json!({"textDocument": {"uri": s.uri}, "position": position_local, "newName": "$other"}),
        )
        .await;
    assert!(got.is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn renaming_after_multi_byte_text() {
    let text = "<h1>Grüße 🎉</h1>\n@php\n    $local = 1;\n@endphp\n<p>🎉 日本語</p> {{ $local }} {{ fn ($x) => $x + $local }}\n";
    let mut s = Session::start(text).await;
    let position = position(text, "$local }}", 2);
    let edit =
        s.c.request(
            "textDocument/rename",
            json!({"textDocument": {"uri": s.uri}, "position": position, "newName": "$other"}),
        )
        .await;
    let edits = edit["changes"][&s.uri].as_array().unwrap();
    assert_eq!(edits.len(), 3, "{edits:?}");
    for e in edits {
        assert_eq!(s.range_text(&e["range"]), "$local", "{e}");
    }
    let prepared = s.at("textDocument/prepareRename", "$local }}", 2).await;
    assert_eq!(s.range_text(&prepared), "$local", "{prepared}");

    // a class named by `@use` is renamed there, and only in the name
    let text = "<p>🎉</p>\n@use('App\\Models\\Post')\n<p>日本語</p> {{ new Post }}\n";
    let mut s = Session::start(text).await;
    let lib = uri(&s.root.join("app/Models/Post.php"));
    let php = std::fs::read_to_string(s.root.join("app/Models/Post.php")).unwrap();
    let edit =
        s.c.request(
            "textDocument/rename",
            json!({"textDocument": {"uri": lib}, "position": position_of_class(&php),
                   "newName": "Article"}),
        )
        .await;
    let edits = edit["changes"][&s.uri].as_array().unwrap();
    assert_eq!(edits.len(), 2, "{edits:?}");
    for e in edits {
        assert_eq!(s.range_text(&e["range"]), "Post", "{e}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn completing_in_a_template() {
    let text = "<p>🎉 日本語</p> @inc
<p>日本語</p> @
<p>日本語</p> @include('par')
<p>日本語</p> @include('')
<p>日本語</p> @includeWhen(true, 'partials.ca')
<p>日本語</p> @section('par')
<p>日本語</p> {{ $ }}
@foreach ($xs as $x)
    <p>日本語</p> {{ $ }}
    {{ $errors-> }}
@endforeach
{{ $a->b }} foo@exa
{{ @fil }}
";
    let mut s = Session::start(text).await;
    let got = s.labels("@inc\n", 4).await;
    assert!(got.contains(&"@include".to_owned()), "{got:?}");
    assert!(got.contains(&"@includeIf".to_owned()), "{got:?}");
    assert!(!got.contains(&"@foreach".to_owned()), "{got:?}");

    let got = s.labels("@\n", 1).await;
    assert!(got.contains(&"@foreach".to_owned()), "{got:?}");

    // the whole of what's being typed is replaced, after multi-byte text
    let raw = s.at("textDocument/completion", "@inc\n", 4).await;
    let edit = &raw["items"][0]["textEdit"];
    assert_eq!(s.range_text(&edit["range"]), "@inc", "{edit}");

    let got = s.labels("par')\n<p>日本語</p> @include(''", 3).await;
    assert_eq!(got, ["partials.badge", "partials.card"]);
    let raw = s
        .at(
            "textDocument/completion",
            "par')\n<p>日本語</p> @include(''",
            3,
        )
        .await;
    let edit = &raw["items"][0]["textEdit"];
    assert_eq!(s.range_text(&edit["range"]), "par", "{edit}");
    assert_eq!(edit["newText"], "partials.badge", "{edit}");

    let got = s.labels("')\n<p>日本語</p> @includeWhen", 0).await;
    assert_eq!(got, ["partials.badge", "partials.card", "show"]);
    let got = s.labels("partials.ca')", 11).await;
    assert_eq!(got, ["partials.card"]);

    // what's inserted for a class goes with a `@use`
    let imported = "<p>🎉</p>\n{{ new Pos }}\n";
    let mut with_import = Session::start(imported).await;
    let raw = with_import.at("textDocument/completion", "Pos }}", 3).await;
    let items = raw["items"].as_array().unwrap();
    let post = items.iter().find(|i| i["label"] == "Post").unwrap();
    let added = &post["additionalTextEdits"][0];
    assert_eq!(added["newText"], "@use('App\\Models\\Post')\n", "{post}");
    assert_eq!(added["range"]["start"]["line"], 0, "{post}");
    assert_eq!(post["textEdit"]["newText"], "Post", "{post}");

    // nothing in the HTML, a comment or right after an echo
    let quiet = "<p>a</p> {{ $a }} {{-- $b --}}\n";
    let mut quiet_session = Session::start(quiet).await;
    let got = quiet_session.labels(" {{-- $b", 8).await;
    assert_eq!(got, Vec::<String>::new());
    let got = quiet_session.labels("}} {{--", 2).await;
    assert_eq!(got, Vec::<String>::new());

    // not every string is a template's name
    let got = s.labels("par')\n<p>日本語</p> {{", 3).await;
    assert_eq!(got, Vec::<String>::new());

    let got = s.labels("$ }}\n@foreach", 1).await;
    assert!(got.contains(&"$errors".to_owned()), "{got:?}");
    assert!(!got.contains(&"$loop".to_owned()), "{got:?}");
    let got = s.labels("$ }}\n    {{ $errors", 1).await;
    assert!(got.contains(&"$errors".to_owned()), "{got:?}");
    assert!(got.contains(&"$loop".to_owned()), "{got:?}");
    assert!(got.contains(&"$x".to_owned()), "{got:?}");

    // an address isn't a directive, and `@` in PHP doesn't start one
    let got = s.labels("foo@exa", 7).await;
    assert!(!got.iter().any(|l| l.starts_with('@')), "{got:?}");
    let got = s.labels("@fil }}", 4).await;
    assert!(!got.iter().any(|l| l.starts_with('@')), "{got:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn trigger_characters_of_a_template_stay_there() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    write(&root.join("stirphp.neon"), "parameters:\n    stirphp: {}\n");
    let php = root.join("main.php");
    let text = "<?php\n$x = 'a';\n$y = @foo();\n";
    write(&php, text);
    let doc = uri(&php);
    let mut c = Client::start();
    c.request(
        "initialize",
        json!({"processId": null, "rootUri": uri(&root), "capabilities": {}}),
    )
    .await;
    c.notify("initialized", json!({})).await;
    c.notify(
        "textDocument/didOpen",
        json!({"textDocument": {"uri": doc, "languageId": "php", "version": 1, "text": text}}),
    )
    .await;
    let got = c
        .request(
            "textDocument/completion",
            json!({"textDocument": {"uri": doc}, "position": {"line": 1, "character": 8},
                   "context": {"triggerKind": 2, "triggerCharacter": "'"}}),
        )
        .await;
    assert!(got.is_null(), "{got}");
}

/// The semantic token at `needle`: its type and length.
fn token_at(data: &[u64], text: &str, needle: &str) -> Option<(u64, u64)> {
    let want = position(text, needle, 0);
    let (want_line, want_col) = (want["line"].as_u64()?, want["character"].as_u64()?);
    let (mut line, mut col) = (0, 0);
    for t in data.chunks(5) {
        if t[0] > 0 {
            col = 0;
        }
        line += t[0];
        col += t[1];
        if (line, col) == (want_line, want_col) {
            return Some((t[3], t[2]));
        }
    }
    None
}

#[tokio::test(flavor = "multi_thread")]
async fn highlighting_a_template() {
    let text = "<p>🎉 日本語</p> {{-- a --}} {{ $a }} {!! $b !!} {{{ $c }}}
@if ($a)
@unless ($b)
@foreach ($xs as $x) @endforeach
@endunless
@endif
@include('card')
@use('App\\Post')
@forelse ($xs as $x) @empty @endforelse
@php $y = 1; @endphp
<?php $z = 1; ?>
";
    let mut s = Session::start(text).await;
    let got =
        s.c.request(
            "textDocument/semanticTokens/full",
            json!({"textDocument": {"uri": s.uri}}),
        )
        .await;
    let data: Vec<u64> = got["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap())
        .collect();
    const KEYWORD: u64 = 12;
    const COMMENT: u64 = 13;
    const STRING: u64 = 14;
    const VARIABLE: u64 = 7;
    for (needle, ty, len) in [
        ("{{-- a --}}", COMMENT, 11),
        ("{{ $a", KEYWORD, 2),
        ("$a }}", VARIABLE, 2),
        ("}} {!!", KEYWORD, 2),
        ("{!! $b", KEYWORD, 3),
        ("!!} {{{", KEYWORD, 3),
        ("{{{ $c", KEYWORD, 3),
        ("}}}", KEYWORD, 3),
        ("@if", KEYWORD, 3),
        ("@unless", KEYWORD, 7),
        ("@foreach", KEYWORD, 8),
        ("@endforeach", KEYWORD, 11),
        ("@endunless", KEYWORD, 10),
        ("@endif", KEYWORD, 6),
        ("@include", KEYWORD, 8),
        ("'card'", STRING, 6),
        ("@use", KEYWORD, 4),
        ("('App", STRING, 12),
        ("@forelse", KEYWORD, 8),
        ("@empty", KEYWORD, 6),
        ("@endforelse", KEYWORD, 11),
        ("@php", KEYWORD, 4),
        ("@endphp", KEYWORD, 7),
    ] {
        assert_eq!(token_at(&data, text, needle), Some((ty, len)), "{needle}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn formatting_leaves_a_template_alone() {
    let text = "<p>🎉</p>\n@php\n$a    =   1;\n@endphp\n{{    $a   }}\n<?php   $b=1;   ?>\n";
    let mut s = Session::start(text).await;
    let got = s
        .c
        .request(
            "textDocument/formatting",
            json!({"textDocument": {"uri": s.uri}, "options": {"tabSize": 4, "insertSpaces": true}}),
        )
        .await;
    assert_eq!(got, json!([]));
    let got = s
        .c
        .request(
            "textDocument/rangeFormatting",
            json!({"textDocument": {"uri": s.uri},
                   "range": {"start": {"line": 0, "character": 0}, "end": {"line": 5, "character": 0}},
                   "options": {"tabSize": 4, "insertSpaces": true}}),
        )
        .await;
    assert_eq!(got, json!([]));
}

#[tokio::test(flavor = "multi_thread")]
async fn inlay_hints_in_a_template() {
    let text = "<p>🎉 日本語</p>\n@php\n    $n = 1;\n@endphp\n@foreach (['a'] as $item)\n    {{ $item }} {{ $given }}\n@endforeach\n";
    let mut s = Session::start(text).await;
    let got = s
        .c
        .request(
            "textDocument/inlayHint",
            json!({"textDocument": {"uri": s.uri},
                   "range": {"start": {"line": 0, "character": 0}, "end": {"line": 9, "character": 0}}}),
        )
        .await;
    let labels: Vec<(u64, u64, String)> = got
        .as_array()
        .unwrap()
        .iter()
        .map(|h| {
            (
                h["position"]["line"].as_u64().unwrap(),
                h["position"]["character"].as_u64().unwrap(),
                h["label"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        labels,
        [(2, 6, ": int".to_owned()), (4, 24, ": string".to_owned()),]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn symbols_and_structure_of_a_template() {
    let text = "<p>🎉 日本語</p>
@php
    function helper_here(): int { return 1; }
    class HereToo {
        public function run(): int { return helper_here(); }
    }
@endphp
{{ (new HereToo)->run() }} {{ helper_here() }}
";
    let mut s = Session::start(text).await;
    let symbols =
        s.c.request(
            "textDocument/documentSymbol",
            json!({"textDocument": {"uri": s.uri}}),
        )
        .await;
    let names: Vec<&str> = symbols
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["helper_here", "HereToo"], "{symbols}");

    // who calls it, from a `{{ }}`
    let position_decl = position(text, "helper_here(): int", 1);
    let prepared =
        s.c.request(
            "textDocument/prepareCallHierarchy",
            json!({"textDocument": {"uri": s.uri}, "position": position_decl}),
        )
        .await;
    let item = prepared[0].clone();
    assert_eq!(item["name"], "helper_here", "{prepared}");
    let calls =
        s.c.request("callHierarchy/incomingCalls", json!({"item": item}))
            .await;
    let ranges: Vec<String> = calls
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|c| c["fromRanges"].as_array().unwrap().clone())
        .map(|r| slice(text, &r))
        .collect();
    assert!(ranges.iter().all(|r| r == "helper_here"), "{ranges:?}");
    assert_eq!(ranges.len(), 2, "{ranges:?}");

    let hover = s.hover("run() }}", 1).await;
    assert!(hover.contains("function run"), "{hover}");

    let selection =
        s.c.request(
            "textDocument/selectionRange",
            json!({"textDocument": {"uri": s.uri},
                   "positions": [position(text, "helper_here() }}", 2)]}),
        )
        .await;
    assert_eq!(
        slice(text, &selection[0]["range"]),
        "helper_here",
        "{selection}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_template_has_no_false_alarms_in_the_editor() {
    let mut s = Session::start(TEMPLATE).await;
    let uri = s.uri.clone();
    let diags = s.c.diagnostics(&uri).await;
    let codes: Vec<&str> = diags.iter().map(|d| d["code"].as_str().unwrap()).collect();
    // `helper` isn't imported, and that's all
    assert_eq!(codes, ["function.notFound"], "{diags:?}");
    assert_eq!(s.path.file_name().unwrap(), "show.blade.php");
}

#[tokio::test(flavor = "multi_thread")]
async fn laravels_names_without_laravel() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    write(&root.join("stirphp.neon"), "parameters:\n    stirphp: {}\n");
    let path = root.join("page.blade.php");
    let text = "{{ route('home') }} {{ Str::upper($a) }} {{ $errors->any() }} {{ $a->b }}\n@foreach ($a as $b) {{ $loop->first }} @endforeach\n";
    write(&path, text);
    let doc = uri(&path);
    let mut c = Client::start();
    c.request(
        "initialize",
        json!({"processId": null, "rootUri": uri(&root), "capabilities": {}}),
    )
    .await;
    c.notify("initialized", json!({})).await;
    c.notify(
        "textDocument/didOpen",
        json!({"textDocument": {"uri": doc, "languageId": "php", "version": 1, "text": text}}),
    )
    .await;
    assert_eq!(c.diagnostics(&doc).await, Vec::<Value>::new());
}

async fn action_titles(s: &mut Session, needle: &str, delta: usize) -> Vec<Value> {
    let at = position(&s.text, needle, delta);
    let got =
        s.c.request(
            "textDocument/codeAction",
            json!({"textDocument": {"uri": s.uri}, "range": {"start": at, "end": at},
                   "context": {"diagnostics": []}}),
        )
        .await;
    got.as_array().cloned().unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread")]
async fn code_actions_in_a_template() {
    let text = "<p>🎉 Grüße 日本語</p>
<p>🎉</p> {{ new Post }} {{ strlen($given) }}
@foreach (new Post as $x)
    <p>日本語</p> {{ 1 + 2 }}
@endforeach
@php
    $sum = 1 + 2;
@endphp
";
    let mut s = Session::start(text).await;
    let uri_of = s.uri.clone();
    s.c.diagnostics(&uri_of).await;

    for (needle, delta) in [("Post }}", 1), ("Post as", 1)] {
        let actions = action_titles(&mut s, needle, delta).await;
        let import = actions
            .iter()
            .find(|a| a["title"] == "Import `App\\Models\\Post`")
            .unwrap_or_else(|| panic!("{actions:?}"));
        let edits = import["edit"]["changes"][&uri_of].as_array().unwrap();
        let inserted = edits
            .iter()
            .find(|e| {
                e["newText"]
                    .as_str()
                    .unwrap()
                    .contains("@use('App\\Models\\Post')")
            })
            .unwrap_or_else(|| panic!("{edits:?}"));
        assert_eq!(
            inserted["range"]["start"],
            json!({"line": 0, "character": 0})
        );
        // nothing that takes a statement, nor a comment on the line above
        for a in &actions {
            let kind = a["kind"].as_str().unwrap_or("");
            assert!(!kind.starts_with("refactor"), "{a}");
            assert!(!a["title"].as_str().unwrap().starts_with("Ignore"), "{a}");
        }
    }
    for (needle, delta) in [("strlen($given)", 1), ("1 + 2 }}", 0)] {
        let actions = action_titles(&mut s, needle, delta).await;
        for a in &actions {
            assert!(
                !a["kind"].as_str().unwrap_or("").starts_with("refactor"),
                "{a}"
            );
        }
    }
    // in a block of PHP, there's a place for one
    let at = position(text, "1 + 2;", 0);
    let end = position(text, "1 + 2;", 5);
    let got =
        s.c.request(
            "textDocument/codeAction",
            json!({"textDocument": {"uri": uri_of}, "range": {"start": at, "end": end},
                   "context": {"diagnostics": []}}),
        )
        .await;
    let actions = got.as_array().unwrap();
    assert!(
        actions
            .iter()
            .any(|a| a["title"] == "Extract to a variable"),
        "{actions:?}"
    );
}

/// A Laravel application's template and the controller that renders it, both open.
struct Rendered {
    c: Client,
    controller: String,
    template: String,
    _tmp: tempfile::TempDir,
}

const CONTROLLER: &str = "<?php
namespace App;
use App\\Models\\Post;
class Show
{
    public function __invoke(Post $post)
    {
        return view('show', ['post' => $post]);
    }
}
";

const SHOWN: &str = "<p>日本語</p> {{ $post->title }} {{ $post->wrong() }}\n{{ $missing }}\n";

async fn rendered() -> Rendered {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    write(
        &root.join("stirphp.neon"),
        "parameters:\n    stirphp:\n        frameworks:\n            laravel: true\n",
    );
    write(&root.join("bootstrap/app.php"), "<?php\n");
    write(
        &root.join("app/Models/Post.php"),
        "<?php\nnamespace App\\Models;\nclass Post {\n    public string $title = '';\n}\n",
    );
    write(
        &root.join("app/Models/Page.php"),
        "<?php\nnamespace App\\Models;\nclass Page {\n    public string $title = '';\n    public function wrong(): int { return 1; }\n}\n",
    );
    write(
        &root.join("app/Factory.php"),
        "<?php\nnamespace Illuminate\\View;\nclass Factory {}\n",
    );
    write(
        &root.join("app/helpers.php"),
        "<?php\nfunction view(string $name, array $data = []): string { return ''; }\n",
    );
    let controller_path = root.join("app/Show.php");
    write(&controller_path, CONTROLLER);
    let template_path = root.join("resources/views/show.blade.php");
    write(&template_path, SHOWN);
    let (controller, template) = (uri(&controller_path), uri(&template_path));
    let mut c = Client::start();
    c.request(
        "initialize",
        json!({"processId": null, "rootUri": uri(&root), "capabilities": {},
               "initializationOptions": {"workspaceTrusted": true}}),
    )
    .await;
    c.notify("initialized", json!({})).await;
    for (doc, text) in [(&controller, CONTROLLER), (&template, SHOWN)] {
        c.notify(
            "textDocument/didOpen",
            json!({"textDocument": {"uri": doc, "languageId": "php", "version": 1, "text": text}}),
        )
        .await;
    }
    Rendered {
        c,
        controller,
        template,
        _tmp: tmp,
    }
}

impl Rendered {
    /// The template's diagnostics, once they come to these codes.
    async fn template_reports(&mut self, wanted: &[&str]) {
        let uri = self.template.clone();
        let mut last = Vec::new();
        let waited = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                let diags = self.c.diagnostics(&uri).await;
                let mut codes: Vec<String> = diags
                    .iter()
                    .map(|d| d["code"].as_str().unwrap().to_owned())
                    .collect();
                codes.sort();
                if codes == wanted {
                    return;
                }
                last = codes;
            }
        })
        .await;
        assert!(waited.is_ok(), "wanted {wanted:?}, last got {last:?}");
    }

    async fn change_controller(&mut self, version: u32, text: &str) {
        let doc = self.controller.clone();
        self.c
            .notify(
                "textDocument/didChange",
                json!({"textDocument": {"uri": doc, "version": version},
                       "contentChanges": [{"text": text}]}),
            )
            .await;
    }
}

const WITH_COMPONENTS: &str = "<p>🎉 日本語</p> <x-alert type=\"a\" :limit=\"$given->count\" other=\"{{ $title }}\" :$title>
    <x-slot:heading>🎉 {{ $title }}</x-slot:heading>
</x-alert>
<p>日本語</p> <x-card :kind=\"$title\" not-a-prop=\"b\" /> <x-bare a=\"b\" /> <x-pkg::thing c=\"d\" />
<p>日本語</p> <x-tabs /><x-forms.input />
<x-
<x-fo
<x:ca
<div class=\"<x-a\"></div> {{ '<x-' }}
";

/// A template with components of each kind around it.
async fn with_components() -> Session {
    let mut s = Session::start(WITH_COMPONENTS).await;
    let root = s.root.clone();
    let files = [
        ("bootstrap/app.php", "<?php\n"),
        (
            "app/View/Component.php",
            "<?php\nnamespace Illuminate\\View;\nabstract class Component {\n    public $attributes;\n    public static function resolve($data) { return new static; }\n    abstract public function render();\n}\n",
        ),
        (
            "app/View/Components/Alert.php",
            "<?php\nnamespace App\\View\\Components;\nuse Illuminate\\View\\Component;\nclass Alert extends Component {\n    public function __construct(public string $type, public int $limit = 0) {}\n    public function render() { return view('components.alert'); }\n}\n",
        ),
        (
            "app/View/Components/Forms/Input.php",
            "<?php\nnamespace App\\View\\Components\\Forms;\nclass Input extends \\Illuminate\\View\\Component {\n    public function render() { return view('components.forms.input'); }\n}\n",
        ),
        (
            "resources/views/components/alert.blade.php",
            "{{ $type }}\n",
        ),
        ("resources/views/components/forms/input.blade.php", "a\n"),
        (
            "resources/views/components/card.blade.php",
            "@props(['title', 'kind' => 'info'])\n{{ $kind }}\n",
        ),
        ("resources/views/components/bare.blade.php", "{{ $a }}\n"),
        ("resources/views/components/tabs/index.blade.php", "a\n"),
    ];
    for (path, text) in files {
        write(&root.join(path), text);
    }
    // what's been written since it started is seen by the server restarted on them
    let mut c = Client::start();
    c.request(
        "initialize",
        json!({"processId": null, "rootUri": uri(&root), "capabilities": {},
               "initializationOptions": {"workspaceTrusted": true}}),
    )
    .await;
    c.notify("initialized", json!({})).await;
    c.notify(
        "textDocument/didOpen",
        json!({"textDocument": {"uri": s.uri, "languageId": "php", "version": 1, "text": WITH_COMPONENTS}}),
    )
    .await;
    s.c = c;
    s
}

#[tokio::test(flavor = "multi_thread")]
async fn hovering_in_a_components_tag() {
    let mut s = with_components().await;
    let got = s.hover("alert type", 2).await;
    assert!(got.contains("App\\View\\Components\\Alert"), "{got}");
    assert!(got.contains("string $type"), "{got}");
    let got = s.hover("forms.input", 8).await;
    assert!(got.contains("Forms\\Input"), "{got}");
    let got = s.hover("card :kind", 1).await;
    assert!(got.contains("@props(['title', 'kind' => 'info'])"), "{got}");
    assert!(got.contains("components.card"), "{got}");
    let got = s.hover("tabs />", 1).await;
    assert!(got.contains("components.tabs.index"), "{got}");
    let got = s.hover("pkg::thing", 1).await;
    assert!(got.contains("package"), "{got}");

    let got = s.hover(":limit", 3).await;
    assert!(got.contains("int $limit"), "{got}");
    let got = s.hover("other=", 2).await;
    assert!(got.contains("$attributes"), "{got}");
    let got = s.hover("kind=", 1).await;
    assert!(got.contains("$kind"), "{got}");
    assert!(got.contains("@props"), "{got}");
    let got = s.hover("not-a-prop", 3).await;
    assert!(got.contains("$attributes"), "{got}");

    // the variables in it are what they are anywhere, after multi-byte text
    let raw = s.at("textDocument/hover", "$given->count", 2).await;
    assert_eq!(s.range_text(&raw["range"]), "$given", "{raw}");
    let raw = s.at("textDocument/hover", "$title }}\" :$title", 2).await;
    assert_eq!(s.range_text(&raw["range"]), "$title", "{raw}");
    let raw = s.at("textDocument/hover", ":$title>", 2).await;
    assert_eq!(s.range_text(&raw["range"]), "$title", "{raw}");
}

#[tokio::test(flavor = "multi_thread")]
async fn going_to_a_definition_from_a_components_tag() {
    let mut s = with_components().await;
    let root = s.root.clone();
    let got = s.at("textDocument/definition", "alert type", 2).await;
    assert!(
        is_under(&got["uri"], &root, "app/View/Components/Alert.php"),
        "{got}"
    );
    let got = s.at("textDocument/definition", "forms.input", 8).await;
    assert!(
        is_under(&got["uri"], &root, "app/View/Components/Forms/Input.php"),
        "{got}"
    );
    let got = s.at("textDocument/definition", "card :kind", 1).await;
    assert!(
        is_under(
            &got["uri"],
            &root,
            "resources/views/components/card.blade.php"
        ),
        "{got}"
    );
    let got = s.at("textDocument/definition", "tabs />", 1).await;
    assert!(
        is_under(
            &got["uri"],
            &root,
            "resources/views/components/tabs/index.blade.php"
        ),
        "{got}"
    );
    // the end of a tag goes to the same place
    let got = s.at("textDocument/definition", "alert>\n<p>", 2).await;
    assert!(
        is_under(&got["uri"], &root, "app/View/Components/Alert.php"),
        "{got}"
    );
    // a parameter of the constructor and a variable of `@props`
    let got = s.at("textDocument/definition", ":limit", 3).await;
    assert!(
        is_under(&got["uri"], &root, "app/View/Components/Alert.php"),
        "{got}"
    );
    assert_eq!(got["range"]["start"]["line"], 4, "{got}");
    let got = s.at("textDocument/definition", ":kind", 3).await;
    assert!(
        is_under(
            &got["uri"],
            &root,
            "resources/views/components/card.blade.php"
        ),
        "{got}"
    );
    assert_eq!(got["range"]["start"]["line"], 0, "{got}");
    // nothing for what's only in `$attributes`, or a package's
    let got = s.at("textDocument/definition", "not-a-prop", 3).await;
    assert!(got.is_null(), "{got}");
    let got = s.at("textDocument/definition", "pkg::thing", 1).await;
    assert!(got.is_null(), "{got}");
    // a variable in an attribute is one of this template's
    let got = s
        .at("textDocument/definition", "$title }}\" :$title", 2)
        .await;
    assert!(got["uri"] == json!(s.uri), "{got}");
    let got = s.at("textDocument/definition", ":$title>", 2).await;
    assert!(got["uri"] == json!(s.uri), "{got}");
    assert_eq!(s.range_text(&got["range"]), "$title", "{got}");
}

#[tokio::test(flavor = "multi_thread")]
async fn completing_a_components_name() {
    let mut s = with_components().await;
    let got = s.labels("<x-\n", 3).await;
    assert_eq!(
        got,
        ["alert", "bare", "card", "forms.input", "tabs"],
        "{got:?}"
    );
    let got = s.labels("<x-fo\n", 5).await;
    assert_eq!(got, ["forms.input"], "{got:?}");
    let raw = s.at("textDocument/completion", "<x-fo\n", 5).await;
    let edit = &raw["items"][0]["textEdit"];
    assert_eq!(s.range_text(&edit["range"]), "fo", "{edit}");
    assert_eq!(edit["newText"], "forms.input", "{edit}");
    let got = s.labels("<x:ca\n", 5).await;
    assert_eq!(got, ["card"], "{got:?}");
    // the name that's there, and not in a string
    let got = s.labels("x-tabs />", 4).await;
    assert_eq!(got, ["tabs"], "{got:?}");
    let got = s.labels("'<x-' }}", 4).await;
    assert!(!got.contains(&"alert".to_owned()), "{got:?}");

    // typing the `-` of `<x-` asks for them, and it doesn't anywhere else
    let at = position(&s.text, "<x-\n", 3);
    let got = s
        .c
        .request(
            "textDocument/completion",
            json!({"textDocument": {"uri": s.uri}, "position": at,
                   "context": {"triggerKind": 2, "triggerCharacter": "-"}}),
        )
        .await;
    let first = got["items"][0]["label"].as_str();
    assert_eq!(first, Some("alert"), "{got}");
    let at = position(&s.text, "<x-fo\n", 4);
    let got = s
        .c
        .request(
            "textDocument/completion",
            json!({"textDocument": {"uri": s.uri}, "position": at,
                   "context": {"triggerKind": 2, "triggerCharacter": "-"}}),
        )
        .await;
    assert!(got.is_null(), "{got}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_template_with_components_has_no_false_alarms_in_the_editor() {
    let mut s = with_components().await;
    let uri = s.uri.clone();
    let diags = s.c.diagnostics(&uri).await;
    assert_eq!(diags, Vec::<Value>::new());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_template_of_a_component_has_what_its_class_gives() {
    let mut s = with_components().await;
    let root = s.root.clone();
    let path = root.join("resources/views/components/alert.blade.php");
    let doc = uri(&path);
    s.c.notify(
        "textDocument/didOpen",
        json!({"textDocument": {"uri": doc, "languageId": "php", "version": 1, "text": "{{ $type }}\n"}}),
    )
    .await;
    let got =
        s.c.request(
            "textDocument/hover",
            json!({"textDocument": {"uri": doc}, "position": {"line": 0, "character": 4}}),
        )
        .await;
    let shown = got["contents"]["value"].as_str().unwrap_or("");
    assert!(shown.contains("string"), "{got}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_templates_variable_has_the_type_of_what_renders_it() {
    let mut r = rendered().await;
    r.template_reports(&["method.notFound", "variable.undefined"])
        .await;
    let uri = r.template.clone();
    let got =
        r.c.request(
            "textDocument/hover",
            json!({"textDocument": {"uri": uri}, "position": position(SHOWN, "$post->title", 1)}),
        )
        .await;
    let shown = got["contents"]["value"].as_str().unwrap_or("").to_owned();
    assert!(shown.contains("App\\Models\\Post"), "{shown}");
    assert!(shown.contains("Given to this template"), "{shown}");
    // renaming stays refused: its name is in the controller as well
    let refused =
        r.c.send_request_for_error(
            "textDocument/prepareRename",
            json!({"textDocument": {"uri": uri}, "position": position(SHOWN, "$post->title", 1)}),
        )
        .await;
    assert!(refused.is_some(), "{refused:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn editing_what_renders_a_template_changes_what_it_reports() {
    let mut r = rendered().await;
    r.template_reports(&["method.notFound", "variable.undefined"])
        .await;
    // what's given has that method
    let page = CONTROLLER.replace("Post", "Page");
    r.change_controller(2, &page).await;
    r.template_reports(&["variable.undefined"]).await;
    // and now the variable that wasn't there
    let more = page.replace("['post' => $post]", "['post' => $post, 'missing' => 1]");
    r.change_controller(3, &more).await;
    r.template_reports(&[]).await;
    r.change_controller(4, CONTROLLER).await;
    r.template_reports(&["method.notFound", "variable.undefined"])
        .await;
    // what's given can't be read, so nothing's known of the template
    let elsewhere = CONTROLLER.replace("['post' => $post]", "$post->toArray()");
    r.change_controller(5, &elsewhere).await;
    r.template_reports(&[]).await;
}
