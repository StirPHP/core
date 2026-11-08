// StirPHP's language server in the browser, for a page: it runs in a Web Worker.
//
//   import { startWorker } from 'stirphp-web';
//   import { BrowserMessageReader, BrowserMessageWriter } from 'vscode-jsonrpc/browser';
//
//   const worker = startWorker();
//   const reader = new BrowserMessageReader(worker);
//   const writer = new BrowserMessageWriter(worker);
//
// and hand `reader` and `writer` to a language client (monaco-languageclient's
// `MonacoLanguageClient`, say). The project's files go in `initializationOptions.files`; see the
// README.
//
// `serve`, which runs the server on the thread it's called on, is in `stirphp-web/serve.js`. It
// isn't exported from here, as it would bring the `.wasm` into the page's bundle as well as the
// worker's.

/**
 * Starts the language server in a module Web Worker.
 * @returns {Worker}
 */
export function startWorker() {
  // written out like this, with nothing but literals in the options, so that bundlers (Vite,
  // webpack) see the worker and the `.wasm` it loads: Vite refuses options it can't evaluate
  return new Worker(new URL('./worker.js', import.meta.url), { type: 'module', name: 'stirphp' });
}
