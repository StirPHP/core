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
