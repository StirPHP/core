//! StirPHP's language server in the browser, speaking the Language Server Protocol: the
//! [`Session`] of `stir_lsp`, with the same answers as the native server, for a Web Worker to
//! run. `editors/web` has the worker script that drives it.
//!
//! The worker hands it each message that comes in ([`LanguageServer::push`]), and calls
//! [`LanguageServer::step`] until it says there's nothing to do; what's to be sent goes to the
//! callback given to the constructor, one message (a plain object) at a time.

use stir_lsp::session::{Next, Session};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console)]
    fn error(message: &str);
}

#[wasm_bindgen]
pub struct LanguageServer {
    session: Session,
    send: js_sys::Function,
}

#[wasm_bindgen]
impl LanguageServer {
    /// `send` is called with each message for the client.
    #[wasm_bindgen(constructor)]
    pub fn new(send: js_sys::Function) -> LanguageServer {
        // a panic stops the server for good: the worker says so, and the console has why
        std::panic::set_hook(Box::new(|info| error(&format!("StirPHP: {info}"))));
        LanguageServer {
            session: Session::new(),
            send,
        }
    }

    /// Takes in a JSON-RPC message: its text, or the object `JSON.parse` would make of it.
    pub fn push(&mut self, message: JsValue) {
        let text = match message.as_string() {
            Some(text) => text,
            None => js_sys::JSON::stringify(&message)
                .ok()
                .and_then(|t| t.as_string())
                .unwrap_or_default(),
        };
        self.session.push_text(&text);
    }

    /// Does a little of what there is to do, and sends what's to be sent. `now` is the time in
    /// milliseconds (`performance.now()`). Returns when to step again: 0 at once (after letting
    /// messages in), a number of milliseconds from now, or -1 when a message comes in.
    pub fn step(&mut self, now: f64) -> f64 {
        let next = self.session.step(now);
        for message in self.session.take_outgoing() {
            let Ok(value) = js_sys::JSON::parse(&message.to_string()) else {
                continue;
            };
            if let Err(e) = self.send.call1(&JsValue::NULL, &value) {
                error(&format!("StirPHP: can't send a message: {e:?}"));
            }
        }
        match next {
            Next::Now => 0.0,
            Next::After(ms) => ms.max(1.0),
            Next::Idle => -1.0,
        }
    }

    /// Has the client said `exit`?
    pub fn exited(&self) -> bool {
        self.session.exited()
    }
}
