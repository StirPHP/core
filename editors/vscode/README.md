# StirPHP for VS Code

Language support for StirPHP (`.sphp` files), a strictly typed superset of PHP:

- syntax highlighting, refined by the language server's semantic tokens
  (immutable bindings are `readonly`, mutable ones carry the `mutable` modifier)
- diagnostics, hover, go to definition, references, rename, completion,
  signature help, inlay type hints, outline and folding — all from `sphp lsp`

## The language server

The extension downloads `sphp` itself: on first start it fetches the build for
your platform from [GitHub Releases](https://github.com/StirPHP/core/releases),
checks it against the release's `SHA256SUMS` and keeps it in the extension's
storage. Every few hours it looks for a newer build and installs it; you're
offered a server restart, and the next start uses it anyway.

To use your own `sphp` instead (a local build, say), set `sphp.server.path`.
Without network and without a downloaded build, the extension falls back to
`sphp` on the `PATH`.

VS Code 1.100 or newer.

## Settings

| Setting | Default | |
|---|---|---|
| `sphp.server.path` | (empty) | the `sphp` to run instead of the downloaded one: a path or a command on the `PATH` |
| `sphp.server.channel` | `stable` | download the latest release, or `nightly` (built from every commit on `main`) |
| `sphp.server.autoUpdate` | `true` | install newer builds of the channel |
| `sphp.php.enable` | `false` | also serve `.php` files |
| `sphp.trace.server` | `off` | log LSP traffic to the output channel |

Commands: **StirPHP: Restart Language Server**, **StirPHP: Check for Updates**.
The **StirPHP** output channel logs which `sphp` is used.

## Development

```sh
npm install
npm run check      # type-check extension.js
npm run package    # build stirphp-<version>.vsix
code --install-extension stirphp-0.1.19.vsix
```

Or open this folder in VS Code and press F5 to start an Extension Development Host.
