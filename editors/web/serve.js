// Runs StirPHP's language server on one thread (a Web Worker's), speaking JSON-RPC messages as
// objects through `port`: anything with `postMessage` and `message` events, such as a worker's
// global scope or a MessagePort.
//
// Messages are taken in as they come and handed to the server, which works in small steps: the
// notifications that came in and at most one request, or the diagnostics of one file. Between
// steps the thread is let go, so an edit or a `$/cancelRequest` is seen soon. Diagnostics wait for
// edits to stop for a moment; the server says how long, and a timer waits for it.

import init, { LanguageServer } from './pkg/stir_wasm.js';

/**
 * @param {{postMessage(message: unknown): void, addEventListener(type: 'message', listener: (event: {data: unknown}) => void): void}} port
 * @param {{wasm?: unknown}} [options] where the `.wasm` is, if not next to `pkg/stir_wasm.js`: a
 *   URL, a `Response`, bytes or a `WebAssembly.Module`
 * @returns {Promise<void>} once the server is ready
 */
export async function serve(port, options = {}) {
  /** @type {unknown[]} */
  const queue = [];
  /** @type {LanguageServer | null} */
  let server = null;
  /** @type {string | null} */
  let stopped = null;
  /** @type {ReturnType<typeof setTimeout> | null} */
  let timer = null;
  let soon = false;

  // a hop through the event loop that, unlike `setTimeout(0)`, isn't held back
  const channel = new MessageChannel();
  channel.port1.onmessage = () => {
    soon = false;
    pump();
  };

  /** @param {number} ms */
  function schedule(ms) {
    if (ms < 0) return;
    if (timer !== null) {
      clearTimeout(timer);
      timer = null;
    }
    if (ms === 0) {
      if (!soon) {
        soon = true;
        channel.port2.postMessage(null);
      }
      return;
    }
    timer = setTimeout(() => {
      timer = null;
      pump();
    }, ms);
  }

  function pump() {
    if (stopped !== null) {
      refuse();
      return;
    }
    if (server === null) return;
    try {
      while (queue.length > 0) server.push(queue.shift());
      schedule(server.step(performance.now()));
    } catch (error) {
      stopped = String(error);
      port.postMessage({
        jsonrpc: '2.0',
        method: 'window/showMessage',
        params: { type: 1, message: `StirPHP stopped working: ${stopped}` },
      });
      refuse();
    }
  }

  // with the server gone, requests get an answer rather than none
  function refuse() {
    for (const message of queue.splice(0)) {
      const m = typeof message === 'string' ? JSON.parse(message) : message;
      if (m && m.id !== undefined && m.method !== undefined) {
        port.postMessage({
          jsonrpc: '2.0',
          id: m.id,
          error: { code: -32603, message: `StirPHP stopped working: ${stopped}` },
        });
      }
    }
  }

  port.addEventListener('message', (event) => {
    queue.push(event.data);
    schedule(0);
  });
  // a worker's own scope needs nothing more; a MessagePort has to be started
  if (typeof (/** @type {any} */ (port).start) === 'function') {
    /** @type {any} */ (port).start();
  }

  await init(options.wasm === undefined ? undefined : { module_or_path: options.wasm });
  server = new LanguageServer((/** @type {unknown} */ message) => port.postMessage(message));
  schedule(0);
}
