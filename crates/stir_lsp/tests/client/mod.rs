//! A client speaking to the server over JSON-RPC, as an editor would.
#![allow(dead_code)]

use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};
use stir_lsp::server::TrustOptions;
use stir_lsp::trust::TrustStore;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, DuplexStream};
use tokio::sync::mpsc;
use tower_lsp_server::ls_types::Uri;
use tower_lsp_server::{LspService, Server};

pub struct Client {
    tx: tokio::io::WriteHalf<DuplexStream>,
    rx: mpsc::UnboundedReceiver<Value>,
    /// Messages received while waiting for something else.
    pub backlog: Vec<Value>,
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
    /// With a store of answers of its own, not the user's.
    pub fn start() -> Client {
        let store = tempfile::TempDir::new().unwrap();
        let path = store.keep().join("trust.json");
        Client::start_with(TrustOptions {
            store: TrustStore::at(path),
            answer_wait: Duration::from_secs(60),
        })
    }

    pub fn start_with(trust: TrustOptions) -> Client {
        let (client_side, server_side) = tokio::io::duplex(1 << 20);
        let (sr, sw) = tokio::io::split(server_side);
        let (service, socket) =
            LspService::new(move |client| stir_lsp::server::Server::with_trust(client, trust));
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

    pub async fn send(&mut self, msg: Value) {
        let body = msg.to_string();
        let frame = format!("Content-Length: {}\r\n\r\n{body}", body.len());
        self.tx.write_all(frame.as_bytes()).await.unwrap();
    }

    pub async fn notify(&mut self, method: &str, params: Value) {
        self.send(json!({"jsonrpc": "2.0", "method": method, "params": params}))
            .await;
    }

    pub async fn next(&mut self) -> Value {
        tokio::time::timeout(Duration::from_secs(60), self.rx.recv())
            .await
            .expect("timed out waiting for the server")
            .expect("server closed the connection")
    }

    pub async fn request(&mut self, method: &str, params: Value) -> Value {
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

    /// The next message with this method, from what came in meanwhile first.
    pub async fn message(&mut self, method: &str) -> Value {
        if let Some(i) = self.backlog.iter().position(|m| m["method"] == method) {
            return self.backlog.remove(i);
        }
        loop {
            let m = self.next().await;
            if m["method"] == method {
                return m;
            }
            self.backlog.push(m);
        }
    }

    /// Takes in whatever comes in for this long.
    pub async fn pump(&mut self, how_long: Duration) {
        let end = tokio::time::Instant::now() + how_long;
        while let Ok(Some(message)) = tokio::time::timeout_at(end, self.rx.recv()).await {
            self.backlog.push(message);
        }
    }

    /// Answers a request from the server.
    pub async fn respond(&mut self, request: &Value, result: Value) {
        self.send(json!({"jsonrpc": "2.0", "id": request["id"], "result": result}))
            .await;
    }

    /// The next `publishDiagnostics` for `uri`.
    pub async fn diagnostics(&mut self, uri: &str) -> Vec<Value> {
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

pub fn uri(path: &Path) -> String {
    Uri::from_file_path(path).unwrap().to_string()
}

pub fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}
