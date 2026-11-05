//! The server spoken to over JSON-RPC, as an editor would.

use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, DuplexStream};
use tokio::sync::mpsc;
use tower_lsp_server::ls_types::Uri;
use tower_lsp_server::{LspService, Server};

struct Client {
    tx: tokio::io::WriteHalf<DuplexStream>,
    rx: mpsc::UnboundedReceiver<Value>,
    /// Messages received while waiting for something else.
    backlog: Vec<Value>,
    next_id: i64,
}

async fn read_message(r: &mut BufReader<tokio::io::ReadHalf<DuplexStream>>) -> Option<Value> {
    let mut len = 0usize;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).await.ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(v) = line.strip_prefix("Content-Length: ") {
            len = v.parse().ok()?;
        }
    }
    let mut buf = vec![0; len];
    r.read_exact(&mut buf).await.ok()?;
    serde_json::from_slice(&buf).ok()
}

impl Client {
    fn start() -> Client {
        let (client_side, server_side) = tokio::io::duplex(1 << 20);
        let (sr, sw) = tokio::io::split(server_side);
        let (service, socket) = LspService::new(stir_lsp::server::Server::new);
        tokio::spawn(Server::new(sr, sw, socket).serve(service));
        let (cr, cw) = tokio::io::split(client_side);
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut r = BufReader::new(cr);
            while let Some(msg) = read_message(&mut r).await {
                if tx.send(msg).is_err() {
                    break;
                }
            }
        });
        Client {
            tx: cw,
            rx,
            backlog: Vec::new(),
            next_id: 1,
        }
    }

    async fn send(&mut self, msg: Value) {
        let body = msg.to_string();
        let frame = format!("Content-Length: {}\r\n\r\n{body}", body.len());
        self.tx.write_all(frame.as_bytes()).await.unwrap();
    }

    async fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({"jsonrpc": "2.0", "method": method, "params": params}))
            .await;
    }

    async fn next(&mut self) -> Value {
        tokio::time::timeout(Duration::from_secs(60), self.rx.recv())
            .await
            .expect("timed out waiting for the server")
            .expect("server closed the connection")
    }

    async fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await;
        loop {
            let msg = self.next().await;
            if msg.get("id") == Some(&json!(id)) && msg.get("method").is_none() {
                if let Some(err) = msg.get("error") {
                    panic!("{method} failed: {err}");
                }
                return msg["result"].clone();
            }
            self.backlog.push(msg);
        }
    }

    /// The next `publishDiagnostics` for `uri`.
    async fn diagnostics(&mut self, uri: &str) -> Vec<Value> {
        let matches = |m: &Value| {
            m["method"] == "textDocument/publishDiagnostics" && m["params"]["uri"] == uri
        };
        if let Some(i) = self.backlog.iter().position(matches) {
            let m = self.backlog.remove(i);
            return m["params"]["diagnostics"].as_array().unwrap().clone();
        }
        loop {
            let m = self.next().await;
            if matches(&m) {
                return m["params"]["diagnostics"].as_array().unwrap().clone();
            }
            self.backlog.push(m);
        }
    }
}

fn uri(path: &Path) -> String {
    Uri::from_file_path(path).unwrap().to_string()
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn editing_a_php_file() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    write(&root.join("sphp.toml"), "srcDir = \"src\"\n");
    let lib = root.join("src/lib.php");
    write(
        &lib,
        "<?php\nnamespace App;\n\nfunction greet(string $name): string { return $name; }\n",
    );
    let main = root.join("src/main.php");
    let main_text =
        "<?php\nnamespace App;\n\n$msg = greet('x');\necho missing_fn();\necho strlen($msg);\n";
    write(&main, main_text);
    let main_uri = uri(&main);

    let mut c = Client::start();
    c.request(
        "initialize",
        json!({"processId": null, "rootUri": uri(&root), "capabilities": {}}),
    )
    .await;
    c.notify("initialized", json!({})).await;
    c.notify(
        "textDocument/didOpen",
        json!({"textDocument": {"uri": main_uri, "languageId": "php", "version": 1, "text": main_text}}),
    )
    .await;

    let diags = c.diagnostics(&main_uri).await;
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0]["code"], "SPHP0007");
    assert_eq!(diags[0]["range"]["start"]["line"], 4);

    let hover = c
        .request(
            "textDocument/hover",
            json!({"textDocument": {"uri": main_uri}, "position": {"line": 3, "character": 8}}),
        )
        .await;
    let text = hover["contents"]["value"].as_str().unwrap();
    assert!(text.contains("function greet"), "{text}");

    let def = c
        .request(
            "textDocument/definition",
            json!({"textDocument": {"uri": main_uri}, "position": {"line": 3, "character": 8}}),
        )
        .await;
    assert_eq!(def["uri"], uri(&lib));

    // rename a local
    let edit = c
        .request(
            "textDocument/rename",
            json!({"textDocument": {"uri": main_uri}, "position": {"line": 3, "character": 1},
                   "newName": "$renamed"}),
        )
        .await;
    let changes = edit["changes"][&main_uri].as_array().unwrap();
    assert_eq!(changes.len(), 2, "{changes:?}");

    // rename `greet` everywhere
    let edit = c
        .request(
            "textDocument/rename",
            json!({"textDocument": {"uri": main_uri}, "position": {"line": 3, "character": 8}, "newName": "welcome"}),
        )
        .await;
    let changes = edit["changes"].as_object().unwrap();
    assert_eq!(changes.len(), 2, "{changes:?}");
    assert_eq!(changes[&main_uri][0]["newText"], "welcome");

    // fix the error with an incremental edit: `missing_fn` → `greet`
    c.notify(
        "textDocument/didChange",
        json!({
            "textDocument": {"uri": main_uri, "version": 2},
            "contentChanges": [{
                "range": {"start": {"line": 4, "character": 5}, "end": {"line": 4, "character": 17}},
                "text": "greet('y')"
            }]
        }),
    )
    .await;
    let diags = c.diagnostics(&main_uri).await;
    assert!(diags.is_empty(), "{diags:?}");

    // completion after `->`
    c.notify(
        "textDocument/didChange",
        json!({
            "textDocument": {"uri": main_uri, "version": 3},
            "contentChanges": [{"text": "<?php\nnamespace App;\n\nclass Box { public function open(): void {} }\n$b = new Box();\n$b->\n"}]
        }),
    )
    .await;
    let items = c
        .request(
            "textDocument/completion",
            json!({"textDocument": {"uri": main_uri}, "position": {"line": 5, "character": 4}}),
        )
        .await;
    let labels: Vec<&str> = items["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["label"].as_str().unwrap())
        .collect();
    assert!(labels.contains(&"open"), "{labels:?}");

    // semantic tokens are produced
    let tokens = c
        .request(
            "textDocument/semanticTokens/full",
            json!({"textDocument": {"uri": main_uri}}),
        )
        .await;
    assert!(!tokens["data"].as_array().unwrap().is_empty());

    let _ = c.request("shutdown", Value::Null).await;
}

impl Client {
    /// The `window/showMessage` notifications seen so far, without waiting for more.
    fn shown_messages(&self) -> Vec<String> {
        self.backlog
            .iter()
            .filter(|m| m["method"] == "window/showMessage")
            .map(|m| m["params"]["message"].as_str().unwrap().to_owned())
            .collect()
    }

    /// Everything until a round trip, which the server answers after it's done loading.
    async fn drain(&mut self, uri: &str) {
        let _ = self
            .request(
                "textDocument/documentSymbol",
                json!({"textDocument": {"uri": uri}}),
            )
            .await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn composer_install_state() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    write(
        &root.join("composer.json"),
        r#"{"require": {"acme/lib": "^1"}, "autoload": {"psr-4": {"App\\": "src/"}}}"#,
    );
    let main = root.join("src/main.php");
    let main_text = "<?php\nnamespace App;\n\nuse Acme\\Foo;\n\nnew Foo();\nnew Missing();\n";
    write(&main, main_text);
    let main_uri = uri(&main);

    let mut c = Client::start();
    c.request(
        "initialize",
        json!({"processId": null, "rootUri": uri(&root), "capabilities": {
            "workspace": {"didChangeWatchedFiles": {"dynamicRegistration": true}}
        }}),
    )
    .await;
    c.notify("initialized", json!({})).await;

    // the server asks to watch what says whether packages are installed
    let registration = loop {
        let m = c.next().await;
        if m["method"] == "client/registerCapability" {
            break m;
        }
        c.backlog.push(m);
    };
    let globs =
        registration["params"]["registrations"][0]["registerOptions"]["watchers"].to_string();
    assert!(globs.contains("composer.lock"), "{globs}");
    assert!(globs.contains("composer/installed.json"), "{globs}");
    c.send(json!({"jsonrpc": "2.0", "id": registration["id"], "result": null}))
        .await;

    // one notice, and no error about the package's class
    let diags = c.diagnostics(&main_uri).await;
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0]["code"], "SPHP0008");
    assert_eq!(diags[0]["range"]["start"]["line"], 6);
    c.drain(&main_uri).await;
    let shown = c.shown_messages();
    assert_eq!(shown.len(), 1, "{shown:?}");
    assert!(shown[0].contains("composer install"), "{shown:?}");

    // once it's installed, what the package doesn't declare is an error
    write(
        &root.join("vendor/composer/installed.json"),
        r#"{"packages": [{"name": "acme/lib", "version": "1.0.0", "install-path": "../acme/lib",
            "autoload": {"psr-4": {"Acme\\": "src/"}}}], "dev": true}"#,
    );
    write(
        &root.join("vendor/acme/lib/src/Other.php"),
        "<?php\nnamespace Acme;\nclass Other {}\n",
    );
    c.notify(
        "workspace/didChangeWatchedFiles",
        json!({"changes": [{"uri": uri(&root.join("vendor/composer/installed.json")), "type": 1}]}),
    )
    .await;
    let diags = c.diagnostics(&main_uri).await;
    assert_eq!(diags.len(), 2, "{diags:?}");
    c.drain(&main_uri).await;
    assert_eq!(c.shown_messages().len(), 1);

    // a lock that disagrees says so, once more
    write(
        &root.join("composer.lock"),
        r#"{"packages": [{"name": "acme/lib", "version": "1.1.0"}]}"#,
    );
    c.notify(
        "workspace/didChangeWatchedFiles",
        json!({"changes": [{"uri": uri(&root.join("composer.lock")), "type": 1}]}),
    )
    .await;
    let diags = c.diagnostics(&main_uri).await;
    assert_eq!(diags.len(), 1, "{diags:?}");
    c.drain(&main_uri).await;
    let shown = c.shown_messages();
    assert_eq!(shown.len(), 2, "{shown:?}");
    assert!(shown[1].contains("1.1.0"), "{shown:?}");

    let _ = c.request("shutdown", Value::Null).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_editor_follows_the_php_version() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    write(
        &root.join("composer.json"),
        r#"{"require": {"php": "^8.0"}}"#,
    );
    let main = root.join("main.php");
    let main_text = "<?php\necho str_contains('a', 'b');\necho array_is_list([]);\n";
    write(&main, main_text);
    let main_uri = uri(&main);

    let mut c = Client::start();
    c.request(
        "initialize",
        json!({"processId": null, "rootUri": uri(&root), "capabilities": {}}),
    )
    .await;
    c.notify("initialized", json!({})).await;
    let diags = c.diagnostics(&main_uri).await;
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0]["code"], "SPHP0061");
    assert!(diags[0]["message"].as_str().unwrap().contains("PHP 8.1"));

    let hover = c
        .request(
            "textDocument/hover",
            json!({"textDocument": {"uri": main_uri}, "position": {"line": 1, "character": 8}}),
        )
        .await;
    assert!(
        hover["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("str_contains")
    );
    let hover = c
        .request(
            "textDocument/hover",
            json!({"textDocument": {"uri": main_uri}, "position": {"line": 2, "character": 8}}),
        )
        .await;
    assert!(hover.is_null(), "{hover}");

    c.notify(
        "textDocument/didOpen",
        json!({"textDocument": {"uri": main_uri, "languageId": "php", "version": 1,
                                "text": "<?php\necho array_is\n"}}),
    )
    .await;
    let items = c
        .request(
            "textDocument/completion",
            json!({"textDocument": {"uri": main_uri}, "position": {"line": 1, "character": 13}}),
        )
        .await;
    let labels: Vec<&str> = items["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["label"].as_str().unwrap())
        .collect();
    assert!(!labels.contains(&"array_is_list"), "{labels:?}");

    let _ = c.request("shutdown", Value::Null).await;
}
