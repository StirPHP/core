# StirPHP for VS Code

Language support for StirPHP (`.sphp` files), a strictly typed superset of PHP:

- syntax highlighting, refined by the language server's semantic tokens
  (immutable bindings are `readonly`, mutable ones carry the `mutable` modifier)
- diagnostics, hover, go to definition, references, rename, completion,
  signature help, inlay type hints, outline and folding — all from `sphp lsp`

## Requirements

The `sphp` binary on your `PATH` (or set `sphp.server.path`):

```sh
cargo install --path crates/stir_cli
```

VS Code 1.100 or newer.

## Settings

| Setting | Default | |
|---|---|---|
| `sphp.server.path` | `sphp` | the binary; started as `sphp lsp` |
| `sphp.php.enable` | `false` | also serve `.php` files |
| `sphp.trace.server` | `off` | log LSP traffic to the output channel |

Command: **StirPHP: Restart Language Server**.

## Development

```sh
npm install
npm run check      # type-check extension.js
npm run package    # build stirphp-<version>.vsix
code --install-extension stirphp-0.1.0.vsix
```

Or open this folder in VS Code and press F5 to start an Extension Development Host.
