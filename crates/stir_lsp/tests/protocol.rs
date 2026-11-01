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
async fn editing_session() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    write(
        &root.join("sphp.toml"),
        "srcDir = \"src\"\noutDir = \"build\"\n",
    );
    write(
        &root.join("src/lib.sphp"),
        "namespace App\n\nfunction greet(name: String): String = \"Hello, ${name}!\"\n",
    );
    let main = root.join("src/main.sphp");
    let main_text = "namespace App\n\nval msg = greet(\"x\")\nval n: Int = msg\n";
    write(&main, main_text);
    let main_uri = uri(&main);

    let mut c = Client::start();
    let init = c
        .request(
            "initialize",
            json!({"processId": null, "rootUri": uri(&root), "capabilities": {}}),
        )
        .await;
    assert_eq!(init["capabilities"]["hoverProvider"], true);
    c.notify("initialized", json!({})).await;
    c.notify(
        "textDocument/didOpen",
        json!({"textDocument": {"uri": main_uri, "languageId": "sphp", "version": 1, "text": main_text}}),
    )
    .await;

    // the type error in main.sphp is published
    let diags = c.diagnostics(&main_uri).await;
    assert_eq!(diags.len(), 1, "{diags:?}");
    assert_eq!(diags[0]["code"], "SPHP0005");
    assert_eq!(diags[0]["range"]["start"]["line"], 3);

    // hover and go to definition across files
    let hover = c
        .request(
            "textDocument/hover",
            json!({"textDocument": {"uri": main_uri}, "position": {"line": 2, "character": 11}}),
        )
        .await;
    let text = hover["contents"]["value"].as_str().unwrap();
    assert!(
        text.contains("function greet(name: String): String"),
        "{text}"
    );
    let def = c
        .request(
            "textDocument/definition",
            json!({"textDocument": {"uri": main_uri}, "position": {"line": 2, "character": 11}}),
        )
        .await;
    assert_eq!(def["uri"], uri(&root.join("src/lib.sphp")));
    assert_eq!(def["range"]["start"], json!({"line": 2, "character": 9}));

    // fix the error with an incremental edit: `Int` → `String`
    c.notify(
        "textDocument/didChange",
        json!({
            "textDocument": {"uri": main_uri, "version": 2},
            "contentChanges": [{
                "range": {"start": {"line": 3, "character": 7}, "end": {"line": 3, "character": 10}},
                "text": "String"
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
            "contentChanges": [{"text": "namespace App\n\nval msg = greet(\"x\")\nval k = msg->\n"}]
        }),
    )
    .await;
    let items = c
        .request(
            "textDocument/completion",
            json!({"textDocument": {"uri": main_uri}, "position": {"line": 3, "character": 13}}),
        )
        .await;
    let labels: Vec<&str> = items["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["label"].as_str().unwrap())
        .collect();
    assert!(labels.contains(&"length"), "{labels:?}");

    // rename `greet` everywhere
    let edit = c
        .request(
            "textDocument/rename",
            json!({"textDocument": {"uri": main_uri}, "position": {"line": 2, "character": 11}, "newName": "welcome"}),
        )
        .await;
    let changes = edit["changes"].as_object().unwrap();
    assert_eq!(changes.len(), 2, "{changes:?}");
    assert_eq!(changes[&main_uri][0]["newText"], "welcome");

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

/// Opening generated output (the runtime: the stdlib as PHP) must not replace
/// the stdlib's declarations, even with `checkPhp` on.
#[tokio::test(flavor = "multi_thread")]
async fn opened_build_output_does_not_shadow_the_stdlib() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    write(
        &root.join("sphp.toml"),
        "srcDir = \"src\"\noutDir = \"build\"\ncheckPhp = true\n",
    );
    let main = root.join("src/main.sphp");
    let fixed = "class Registry {\n    private static var $all: MutableMap<String, Registry> = mutableMapOf()\n}\n";
    let broken = format!("{fixed}val n: Int = \"x\"\n");
    write(&main, &broken);
    let runtime = root.join("build/stir-runtime.php");
    let runtime_text = "<?php\n\nnamespace Stir;\n\n/**\n * @template K of int|string\n * @template V\n * @return array<K, V>\n */\nfunction mutableMapOf(array ...$pairs): array\n{\n    return \\array_column($pairs, 'second', 'first');\n}\n";
    write(&runtime, runtime_text);
    let (main_uri, runtime_uri) = (uri(&main), uri(&runtime));

    let mut c = Client::start();
    c.request(
        "initialize",
        json!({"processId": null, "rootUri": uri(&root), "capabilities": {}}),
    )
    .await;
    c.notify("initialized", json!({})).await;
    c.notify(
        "textDocument/didOpen",
        json!({"textDocument": {"uri": main_uri, "languageId": "sphp", "version": 1, "text": broken}}),
    )
    .await;
    assert_eq!(c.diagnostics(&main_uri).await.len(), 1);

    c.notify(
        "textDocument/didOpen",
        json!({"textDocument": {"uri": runtime_uri, "languageId": "php", "version": 1, "text": runtime_text}}),
    )
    .await;
    c.notify(
        "textDocument/didChange",
        json!({"textDocument": {"uri": main_uri, "version": 2}, "contentChanges": [{"text": fixed}]}),
    )
    .await;
    let diags = c.diagnostics(&main_uri).await;
    assert!(diags.is_empty(), "{diags:?}");
    // generated code is not checked
    assert!(
        !c.backlog
            .iter()
            .any(|m| m["params"]["uri"] == runtime_uri.as_str()
                && !m["params"]["diagnostics"].as_array().unwrap().is_empty())
    );
    let _ = c.request("shutdown", Value::Null).await;
}
