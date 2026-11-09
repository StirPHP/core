# StirPHP in the browser

StirPHP's language server compiled to WebAssembly, running in a Web Worker and speaking the Language Server Protocol, so
any LSP client in a page gets what `stirphp lsp` offers: diagnostics, hover, completion with `use` lines, definitions,
references, rename, inlay hints, semantic tokens, signature help, formatting, code actions, hierarchies, symbols, folding
and selection ranges, project rules and framework support. It's the same server as the native one, answering from the same
code: only how messages arrive, and where files come from, differ.

## Build

```sh
just web   # wasm-pack build into pkg/, then the Node test
```

It needs `wasm-pack` and the `wasm32-unknown-unknown` target. `pkg/` isn't committed.

## Use

```js
import { startWorker } from 'stirphp-web';
import { BrowserMessageReader, BrowserMessageWriter } from 'vscode-jsonrpc/browser';

const worker = startWorker();
const reader = new BrowserMessageReader(worker);
const writer = new BrowserMessageWriter(worker);
```

Hand `reader` and `writer` to a client. With `monaco-languageclient`, its `LcWorker` connection does the same given the URL
of `worker.js` and `type: 'module'`; that's how it was tried, in Firefox. The `.wasm` is loaded from
next to `pkg/stir_wasm.js`; `worker.js?wasm=<url>` says where else it is.

## Files

There's no disk. The project's files go in `initializationOptions.files`, a map from an absolute path (or a `file://` URI) to
its text, and the workspace folder is the root URI as usual:

```js
initializationOptions: {
  files: {
    '/project/composer.json': '{"require": {"php": "^8.2"}}',
    '/project/stirphp.neon': 'parameters:\n    stirphp:\n        rules: [rules/no_var_dump.rhai]\n',
    '/project/rules/no_var_dump.rhai': '...',
    '/project/src/User.php': '<?php ...',
  },
},
```

They're loaded as a project on disk is: `stirphp.neon` or `phpstan.neon` and what they include, Composer's files and the
`vendor/` they describe, the PHP version, baselines, frameworks. Workspace-wide answers (references, rename, workspace
symbols, implementations, changing a signature) see all of them, open or not.

Afterwards, the `stirphp/setFiles` notification changes them, with the same map; `null` deletes a file. A change to a
configuration or Composer's files loads the project again. A document open in the editor keeps the editor's text, and goes
back to the page's when it's closed.

Two requests are this server's own:

- `stirphp/fileText` with `{uri}` answers the text of any file the server has, for opening a definition in `vendor/` or in
  PHP's own stubs, whose URIs are `stirphp:///stubs/...`.
- `stirphp/memory` answers `{bytes}`, the size of the WebAssembly memory.

## Rules

A project's Rhai rules run without asking: the page chose to load them, and they run in the same sandbox as in
`stirphp check`, with no file or network access and a cap on operations. `initializationOptions.workspaceTrusted: false`
keeps them off.

## Not here

PHPStan doesn't run, files aren't watched (the page says what changed), and nothing is read from or written to a disk.

## Typing stays smooth

The worker has one thread, so the server works in small steps: the notifications that came in and at most one request, or
the diagnostics of one file, with the thread let go in between. Edits are applied as they arrive; diagnostics wait until
they've stopped for 150 ms, the file being edited first. A `$/cancelRequest` for a request still waiting answers it as
cancelled. A panic stops the server: it says so with `window/showMessage`, and later requests get an error.

## Size and speed

Measured on this machine, with a 54-file project (Firefox, localhost):

| | |
|---|---|
| `stir_wasm_bg.wasm` | 15.0 MB, 3.5 MB gzipped, 2.5 MB with brotli (10 MB of it is PHP's stubs) |
| page load to first diagnostics | about 1.0 s (1.7 s on the first visit) |
| an edit to its diagnostics | 155 to 165 ms, the 150 ms wait included |
| completion | 1 to 10 ms |
| WebAssembly memory | about 90 MB |

## Test

```sh
node --test 'test/*.test.mjs'
```

loads the real `.wasm` in a worker thread and edits a small project: diagnostics, completion with a `use` line, rename across
files.
