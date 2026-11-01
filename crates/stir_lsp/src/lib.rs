//! The StirPHP language server.
//!
//! Editor features are plain functions over the salsa database (`hover`,
//! `nav`, `complete`, ...), testable without a client; [`server`] wires them
//! to the Language Server Protocol.

pub mod complete;
pub mod convert;
pub mod hints;
pub mod hover;
pub mod nav;
pub mod render;
pub mod server;
pub mod symbols;
pub mod tokens;

use tower_lsp_server::{LspService, Server};

/// Serves the language server over stdin/stdout until the client exits.
pub async fn run_stdio() {
    let (service, socket) = LspService::new(server::Server::new);
    Server::new(tokio::io::stdin(), tokio::io::stdout(), socket)
        .serve(service)
        .await;
}
