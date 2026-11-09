# StirPHP for VS Code

Type checking and code navigation for PHP files and Laravel's Blade templates, from the StirPHP language server. It
starts when you open a PHP file or a template, and a Composer project needs no configuration.

- diagnostics, updated as you type, and PHPStan's on save for a project that has it
- hover with types and doc comments
- completion, signature help and inlay hints for inferred types
- go to definition and type definition, including into packages and PHP's own stubs
- find references, highlights and rename
- quick fixes and refactorings in the light bulb and **Refactor...** menus, and **Organize Imports**
- semantic highlighting on top of VS Code's PHP grammar, outline, folding and selection ranges
- a `blade` language for `*.blade.php`, with a grammar of its own for directives, `{{ }}`, `{!! !!}`, `{{-- --}}`, `@php`
  and `<x-...>` tags, and everything above for the PHP inside a template

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

## Other Blade extensions

Another Blade extension can stay enabled. If it declares the same `blade` language, VS Code uses one of the two grammars, by
the order extensions load, so the colours may be that extension's. StirPHP's diagnostics, hover and
completion work either way. If another extension gives `.blade.php` files a language of its own, StirPHP still attaches
to them once a PHP file has started it.

## The language server

The extension downloads `stirphp` itself: on first start it fetches the build for your platform from
[GitHub Releases](https://github.com/StirPHP/core/releases), checks it against the release's `SHA256SUMS` and keeps it in
the extension's storage. Every few hours it looks for a newer build and installs it. You're offered a server restart, and
the next start uses it anyway.

To use your own `stirphp` instead (a local build, say), set `stirphp.server.path`. If the download fails and there's no
earlier build, the extension falls back to `stirphp` on the `PATH`.

## Custom rules and workspace trust

A project can configure scripts in `parameters.stirphp.rules` (see the
[main README](https://github.com/StirPHP/core#custom-rules)). Those are code in the repository, so nothing runs them when
you open it:

- In a workspace VS Code doesn't trust (`Workspaces: Manage Workspace Trust`), they never run, and you aren't asked. The
  server restarts when you trust the workspace.
- In a trusted one, the first time you open it you're asked: **Run the N custom rules this workspace configures?**
  Allow or Don't allow. Until you answer they don't run.
- Your answer is kept for that folder and those exact scripts, in `trusted-rules.json` under your config directory
  (`~/.config/stirphp` on Linux). When a script changes they stop and you're asked again.

Scripts have no access to files, the network or other programs, can't tell the time, and are stopped when they take too
long. `stirphp check` on the command line runs them without asking, since running it in a project is your choice.

## Frameworks

What StirPHP knows of Laravel, Symfony and WordPress is part of it, not code from your project, so it runs without asking,
in trusted workspaces and others alike: completion, hover and going to definitions work through models' columns, relations
and scopes, facades, container services and the like, and from a hook's, view's or route's name to where it's defined. See
the [main README](https://github.com/StirPHP/core#frameworks) for what's covered and how to turn it off.

## PHPStan

If a project has PHPStan's configuration (`phpstan.neon` and so on) and PHPStan among its Composer packages, the extension can
show PHPStan's errors with StirPHP's, without a second extension and without the same error twice. See the
[main README](https://github.com/StirPHP/core#running-phpstan) for what counts as the same.

Running it means running code from the project, so:

- In a workspace VS Code doesn't trust, it never runs, and you aren't asked.
- In a trusted one, you're asked first, and told the command. This is a separate question from custom rules, because PHPStan
  has no sandbox: it's PHP, and it loads the project's bootstrap files and extensions. Until you answer it doesn't run.
- Your answer is kept for that folder, that binary and that configuration file, in `trusted-phpstan.json` next to
  `trusted-rules.json`. You're asked again when either file changes, but not when what's in `vendor/` or what the configuration
  includes changes.

It runs when the workspace opens and when you save a PHP file, on the whole project and in the background, with progress in the
status bar. While it's running, a save waits for the run and then there's one more. Errors from it have the source `phpstan`, link
to PHPStan's page for the identifier (there's none for a third party's identifiers), and cover the whole line. Once you type in a
file, PHPStan's errors in that file go away until the next save's run has finished, because its line numbers are for the saved
text. If it can't run, you get one notice saying why. Changes made outside VS Code aren't noticed until the next save.

Set `stirphp.phpstan.enable` to `off` if you don't want it, or `on` to be told if it can't be found.

## Code actions

The light bulb has StirPHP's quick fixes for what's under the cursor, such as importing an unknown class, adding a type
that's left out, implementing what a class has to, or ignoring an error on its line. **Refactor...** (`Ctrl+Shift+R`) has
extract and inline variable, extract method, changes of signature, and the rest; see the
[main README](https://github.com/StirPHP/core#code-actions) for the list and for what each refuses to do. What can't be
done where you are is listed greyed out, with the reason. **Organize Imports** (`Shift+Alt+O`) sorts the `use` lines and
removes what's unused, which is greyed out in the editor.

Two things are particular to VS Code:

- After **Extract method** and **Extract to a variable**, the new name is selected for renaming, so you can type the name
  you want and have it changed in both places.
- **Add parameter...** asks what the parameter is, such as `int $limit = 10`, and if it has no default, what existing calls
  should pass. Left empty, they pass `TODO_NEW_ARGUMENT`, which is an error at each call until it's replaced.

To have imports organized when you save, add this to your settings:

```json
{
  "[php]": {
    "editor.codeActionsOnSave": { "source.organizeImports": "explicit" }
  }
}
```

## Settings

| Setting | Default | |
|---|---|---|
| `stirphp.server.path` | (empty) | the `stirphp` to run instead of the downloaded one: a path or a command on the `PATH` |
| `stirphp.server.channel` | `stable` | download the latest release, or `nightly` (built from every commit on `main`) |
| `stirphp.server.autoUpdate` | `true` | install newer builds of the channel |
| `stirphp.phpstan.enable` | `auto` | run PHPStan on save: `auto` if the project has it set up, `on`, or `off` |
| `stirphp.phpstan.path` | (empty) | the `phpstan` to run instead of `vendor/bin/phpstan`: a path, or a command on the `PATH` |
| `stirphp.phpstan.configuration` | (empty) | PHPStan's configuration, instead of the one it finds by itself |
| `stirphp.phpstan.level` | (empty) | a level or `max`, over the one in its configuration |
| `stirphp.phpstan.memoryLimit` | (empty) | PHP's memory limit per process, as in `--memory-limit`; empty means `2G` |
| `stirphp.phpstan.timeout` | `0` | seconds a run gets; `0` means 600 |
| `stirphp.trace.server` | `off` | how much LSP traffic to log: `verbose` adds message contents. Traffic is logged once the **StirPHP** output channel's log level is Trace |

Commands: **StirPHP: Restart Language Server**, **StirPHP: Check for Updates**. The **StirPHP** output channel logs which
`stirphp` is used.

## Development

```sh
npm ci
npm run check      # type-check extension.js
npm run package    # build stirphp-<version>.vsix
code --install-extension stirphp-0.3.0.vsix
```
