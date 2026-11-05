# StirPHP for VS Code

Type checking and code navigation for plain PHP files, from the StirPHP language server. It starts when you open a PHP
file, and a Composer project needs no configuration.

- diagnostics, updated as you type
- hover with types and doc comments
- completion, signature help and inlay hints for inferred types
- go to definition and type definition, including into packages and PHP's own stubs
- find references, highlights and rename
- semantic highlighting on top of VS Code's PHP grammar, outline, folding and selection ranges

If a project declares Composer packages that aren't installed, a notice tells you to run `composer install`. Until you
do, errors about what those packages would declare are held back.

See the [main README](https://github.com/StirPHP/core#readme) for `stirphp.neon`, PHPStan compatibility, and how accurate
it is at the moment.

Needs VS Code 1.100 or newer. Install the `stirphp-vscode-<version>.vsix` from
[GitHub Releases](https://github.com/StirPHP/core/releases) with **Extensions: Install from VSIX...**, or:

```sh
code --install-extension stirphp-vscode-<version>.vsix
```

## VS Code's built-in PHP features

VS Code ships its own basic PHP support, which runs next to this extension. The extension doesn't change your settings,
so with the defaults you'll see:

- `php.suggest.basic`: VS Code's completions next to StirPHP's, so `strlen` is listed twice, and names of functions in the
  file are offered after `$object->`. Hover shows two descriptions of built-in functions.
- `php.validate.enable`: VS Code runs `php -l` on save, so a syntax error is reported twice. It also needs PHP on your
  machine, which StirPHP doesn't.

To leave these to StirPHP, add this to your settings, for the workspace or for yourself:

```json
{
  "php.suggest.basic": false,
  "php.validate.enable": false
}
```

Any other PHP language extension you have enabled also adds its own results.

## The language server

The extension downloads `stirphp` itself: on first start it fetches the build for your platform from
[GitHub Releases](https://github.com/StirPHP/core/releases), checks it against the release's `SHA256SUMS` and keeps it in
the extension's storage. Every few hours it looks for a newer build and installs it. You're offered a server restart, and
the next start uses it anyway.

To use your own `stirphp` instead (a local build, say), set `stirphp.server.path`. If the download fails and there's no
earlier build, the extension falls back to `stirphp` on the `PATH`.

## Settings

| Setting | Default | |
|---|---|---|
| `stirphp.server.path` | (empty) | the `stirphp` to run instead of the downloaded one: a path or a command on the `PATH` |
| `stirphp.server.channel` | `stable` | download the latest release, or `nightly` (built from every commit on `main`) |
| `stirphp.server.autoUpdate` | `true` | install newer builds of the channel |
| `stirphp.trace.server` | `off` | how much LSP traffic to log: `verbose` adds message contents. Traffic is logged once the **StirPHP** output channel's log level is Trace |

Commands: **StirPHP: Restart Language Server**, **StirPHP: Check for Updates**. The **StirPHP** output channel logs which
`stirphp` is used.

## Development

```sh
npm ci
npm run check      # type-check extension.js
npm run package    # build stirphp-<version>.vsix
code --install-extension stirphp-0.1.21.vsix
```
