//! A client speaking to the server over JSON-RPC, as an editor would.
#![allow(dead_code)]

use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};
use stir_lsp::server::TrustOptions;
use stir_lsp::session::{Next, Session};
use stir_lsp::trust::TrustStore;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, DuplexStream};
use tokio::sync::mpsc;
use tower_lsp_server::ls_types::Uri;
use tower_lsp_server::{LspService, Server};

/// How messages get to the server.
enum Transport {
    /// The native server, over a byte stream as stdio would be.
    Native(tokio::io::WriteHalf<DuplexStream>),
    /// The thread-free session, as in the browser, stepped on a task of its own.
    Session(mpsc::UnboundedSender<Value>),
}

pub struct Client {
    tx: Transport,
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
            tx: Transport::Native(cw),
            rx,
            backlog: Vec::new(),
            next_id: 1,
        }
    }

    /// The server as it runs in the browser: a [`Session`] stepped on a task, with the clock going
    /// by as it would. It has no disk, so it's handed the files of the workspace folder at
    /// `initialize`, and those that `workspace/didChangeWatchedFiles` names after that, as a page
    /// would.
    pub fn start_session() -> Client {
        let (tx, mut incoming) = mpsc::unbounded_channel::<Value>();
        let (out, rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut session = Session::new();
            let start = tokio::time::Instant::now();
            let now = || start.elapsed().as_secs_f64() * 1000.0;
            loop {
                let next = session.step(now());
                for m in session.take_outgoing() {
                    if out.send(m).is_err() {
                        return;
                    }
                }
                let message = match next {
                    Next::Now => {
                        tokio::task::yield_now().await;
                        incoming.try_recv().ok()
                    }
                    Next::After(ms) => {
                        let wait = Duration::from_secs_f64(ms / 1000.0);
                        tokio::time::timeout(wait, incoming.recv())
                            .await
                            .ok()
                            .flatten()
                    }
                    Next::Idle => match incoming.recv().await {
                        Some(m) => Some(m),
                        None => return,
                    },
                };
                if let Some(m) = message {
                    session.push(as_a_page_would(m));
                }
                while let Ok(m) = incoming.try_recv() {
                    session.push(as_a_page_would(m));
                }
            }
        });
        Client {
            tx: Transport::Session(tx),
            rx,
            backlog: Vec::new(),
            next_id: 1,
        }
    }

    pub async fn send(&mut self, msg: Value) {
        match &mut self.tx {
            Transport::Native(tx) => {
                let body = msg.to_string();
                let frame = format!("Content-Length: {}\r\n\r\n{body}", body.len());
                tx.write_all(frame.as_bytes()).await.unwrap();
            }
            Transport::Session(tx) => tx.send(msg).unwrap(),
        }
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

/// `message`, with the files a page hands the session where the native server reads them from disk.
fn as_a_page_would(mut message: Value) -> Value {
    match message["method"].as_str() {
        Some("initialize") => {
            let root = message["params"]["rootUri"]
                .as_str()
                .and_then(|u| u.parse::<Uri>().ok())
                .and_then(|u| u.to_file_path().map(|p| p.into_owned()));
            let mut files = serde_json::Map::new();
            let mut dirs: Vec<_> = root.into_iter().collect();
            while let Some(dir) = dirs.pop() {
                for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        dirs.push(path);
                    } else if let Ok(text) = std::fs::read_to_string(&path) {
                        files.insert(path.display().to_string(), json!(text));
                    }
                }
            }
            let params = &mut message["params"];
            if !params["initializationOptions"].is_object() {
                params["initializationOptions"] = json!({});
            }
            params["initializationOptions"]["files"] = Value::Object(files);
            message
        }
        Some("workspace/didChangeWatchedFiles") => {
            let mut files = serde_json::Map::new();
            for change in message["params"]["changes"]
                .as_array()
                .into_iter()
                .flatten()
            {
                let Some(path) = change["uri"]
                    .as_str()
                    .and_then(|u| u.parse::<Uri>().ok())
                    .and_then(|u| u.to_file_path().map(|p| p.into_owned()))
                else {
                    continue;
                };
                let text = std::fs::read_to_string(&path).map_or(Value::Null, Value::String);
                files.insert(path.display().to_string(), text);
            }
            json!({"jsonrpc": "2.0", "method": "stirphp/setFiles", "params": {"files": files}})
        }
        _ => message,
    }
}

pub fn uri(path: &Path) -> String {
    Uri::from_file_path(path).unwrap().to_string()
}

pub fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}
