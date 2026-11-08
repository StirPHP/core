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

export { serve } from './serve.js';

/**
 * Starts the language server in a module Web Worker.
 * @param {{name?: string}} [options]
 * @returns {Worker}
 */
export function startWorker(options = {}) {
  // written out like this so that bundlers (Vite, webpack) see the worker and the `.wasm` it loads
  return new Worker(new URL('./worker.js', import.meta.url), {
    type: 'module',
    name: options.name ?? 'stirphp',
  });
}
