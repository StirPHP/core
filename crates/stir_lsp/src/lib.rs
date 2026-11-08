//! The StirPHP language server.
//!
//! Editor features are plain functions over the salsa database (`hover`,
//! `nav`, `complete`, ...), testable without a client. [`handlers`] answers the
//! Language Server Protocol's requests with them, for both ways of speaking it:
//! [`server`] over stdio (tokio and threads), and [`session`] one message at a
//! time with no threads or clock of its own, which is what runs in the browser.

pub mod actions;
pub mod complete;
pub mod convert;
pub mod handlers;
pub mod hierarchy;
pub mod hints;
pub mod hover;
pub mod imports;
pub mod links;
pub mod nav;
pub mod render;
#[cfg(feature = "server")]
pub mod server;
pub mod session;
pub mod state;
pub mod symbols;
pub mod tokens;
#[cfg(feature = "server")]
pub mod trust;

/// Serves the language server over stdin/stdout until the client exits.
#[cfg(feature = "server")]
pub async fn run_stdio() {
    use tower_lsp_server::{LspService, Server};
    let (service, socket) = LspService::new(server::Server::new);
    let serve = Server::new(tokio::io::stdin(), tokio::io::stdout(), socket).serve(service);
    tokio::select! {
        () = serve => stir_phpstan::kill_all(),
        () = stir_phpstan::signalled() => {
            stir_phpstan::kill_all();
            // not by returning: the runtime would wait for what's reading stdin, forever
            std::process::exit(0);
        }
    }
}
