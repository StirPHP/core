/** Something messages go through: a worker's global scope, a `MessagePort`. */
export interface Port {
  postMessage(message: unknown): void;
  addEventListener(type: 'message', listener: (event: { data: unknown }) => void): void;
}

/**
 * Runs the language server on this thread, speaking JSON-RPC messages (objects) through `port`.
 * `wasm` is where the `.wasm` is, if not next to `pkg/stir_wasm.js`: a URL, a `Response`, bytes
 * or a `WebAssembly.Module`. Resolves once the server is ready.
 */
export function serve(port: Port, options?: { wasm?: unknown }): Promise<void>;

/** Starts the language server in a module Web Worker. */
export function startWorker(options?: { name?: string }): Worker;
