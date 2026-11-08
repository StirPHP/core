// The worker of `worker.js`, in Node: `parentPort` for the worker's own scope, and the `.wasm`
// read from disk, since Node can't fetch a `file:` URL.
import { readFileSync } from 'node:fs';
import { parentPort } from 'node:worker_threads';

import { serve } from '../serve.js';

const wasm = readFileSync(new URL('../pkg/stir_wasm_bg.wasm', import.meta.url));
serve(
  {
    postMessage: (message) => parentPort.postMessage(message),
    // `on` keeps the thread alive while it listens; `addEventListener` doesn't
    addEventListener: (_, listener) => parentPort.on('message', (data) => listener({ data })),
  },
  { wasm },
);
