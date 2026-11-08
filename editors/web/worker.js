// The Web Worker StirPHP's language server runs in. Start it as a module worker
// (`new Worker(url, {type: 'module'})`, or `startWorker()` from `index.js`), then speak LSP to it
// with `postMessage`: vscode-jsonrpc's `BrowserMessageReader` and `BrowserMessageWriter` do that
// over the `Worker` as it is.
//
// `?wasm=<url>` on the worker's URL says where the `.wasm` is, if it isn't next to
// `pkg/stir_wasm.js`.

import { serve } from './serve.js';

const wasm = new URL(self.location.href).searchParams.get('wasm') ?? undefined;
serve(self, { wasm });
