# StirPHP

StirPHP is a type checker and language server for plain PHP. It reads your `.php` files, their PHPDoc and your Composer
setup as they are. There's nothing to migrate to and no required configuration.

The goal is the best open source language server for PHP, as good as PhpStorm's engine. Type safety shouldn't depend on
being able to pay for a licence, and it should work live in the editor. It isn't there yet: see [Current state](#current-state).

It's written in Rust, and the command is `stirphp`.

## Install

Linux and macOS, to `~/.local/bin/stirphp`:

```sh
curl -fsSL https://stirphp.github.io/core/install.sh | sh
```

Windows (PowerShell), to `%LOCALAPPDATA%\Programs\stirphp\bin\stirphp.exe`:

```powershell
irm https://stirphp.github.io/core/install.ps1 | iex
```

These install the latest release for your platform, after checking it against the release's `SHA256SUMS`, and need no
admin rights. Set `STIRPHP_VERSION` to a version or to `nightly`, and `STIRPHP_INSTALL_DIR` to choose where it goes. On
Windows the script adds that folder to your user `PATH`, unless `STIRPHP_NO_MODIFY_PATH` is set. The scripts are
[`install.sh`](install.sh) and [`install.ps1`](install.ps1). Archives for every platform are on the
[releases page](https://github.com/StirPHP/core/releases).

To build from source (Rust 1.96 or newer):

```sh
git submodule update --init --depth 1 crates/stir_stubs/phpstorm-stubs
cargo install --path crates/stir_cli
```

For VS Code, see [`editors/vscode`](editors/vscode). The extension downloads `stirphp` itself.

## Quick start

From a project's folder, with no configuration:

```sh
composer install    # if it's a Composer project; StirPHP reads the result and never runs Composer
stirphp check
```

```text
error[return.type]: type mismatch in return value: 'not an int' is not assignable to int
    ╭─[ src/main.php:16:12 ]
    │
 16 │     return 'not an int';
    │            ──────┬─────
    │                  ╰─────── type mismatch in return value: 'not an int' is not assignable to int
────╯
1 error, 0 warnings in 1 file (checked 2 files)
```

Without configuration it checks every `.php` file under the folder except `vendor/`, at level 5. It exits with 1 if it
reports errors, and with 0 otherwise.

## `stirphp check`

```sh
stirphp check                       # the whole project
stirphp check src/Foo.php tests     # report only for these paths
stirphp check --level 8             # rule level, 0 to 10, over the one in the configuration
stirphp check -c ci.neon            # use this configuration
stirphp check --generate-baseline   # accept what's reported now
stirphp check --format json         # one JSON array of diagnostics
stirphp check --max-errors 20       # show at most 20; all are still counted
stirphp check --no-vendor           # don't load Composer's packages
```

The levels follow PHPStan's, 0 to 10, and `level: max` in a configuration means 10.

`--generate-baseline` writes `identifier`, `path` and `count` entries to `stirphp-baseline.neon` next to the configuration,
or to the path you give it, and makes `parameters.stirphp.baseline` name the file. It creates `stirphp.neon` if there's no
configuration. From then on an error is reported once a file has more of that identifier than its count, and a warning
says which entries have fewer than their count and can go. Errors PHP itself can't run past, such as syntax errors, can't
be put in a baseline or ignored.

A file with `// @stirphp-nocheck` in its leading comments isn't checked for types, though its declarations are still known.

`stirphp lsp` runs the language server, and `stirphp init` writes a configuration, as below.

## Configuration

All of it is optional. StirPHP looks in the current folder and its parents for `stirphp.neon`, then for PHPStan's
`phpstan.neon`, `phpstan.neon.dist` and `phpstan.dist.neon`, and reads the first it finds.

Its own settings go under `parameters.stirphp`, and all of them are optional:

```neon
parameters:
    stirphp:
        srcDir: src                 # where the sources are; default: the project folder
        include: ['**/*.php']       # globs of source files, relative to srcDir
        exclude: ['vendor/**']      # globs left out
        composer: ../composer.json  # default: composer.json in the project folder
        baseline: stirphp-baseline.neon
```

An unknown key is an error. `rules` is accepted as well, and `stirphp init` writes an empty one, but nothing runs those
rules yet.

PHPStan's parameters work in `stirphp.neon` too, so a project can keep its settings there and include PHPStan's:

```neon
includes:
    - phpstan.neon

parameters:
    stirphp:
        srcDir: src
```

Or keep everything in `phpstan.neon`. PHPStan rejects parameters it doesn't know, so declare this one in
`parametersSchema`. `stirphp init` does that for you when the project has a PHPStan configuration, and
`stirphp init --phpstan` creates `phpstan.neon` if it doesn't:

```neon
parameters:
    level: 5
    paths:
        - src
    stirphp:
        baseline: stirphp-baseline.neon

parametersSchema:
    ?stirphp: mixed()
```

Real PHPStan accepts this. Don't put StirPHP's baseline in PHPStan's `includes`: PHPStan would apply its entries too, and
they'd hide its own errors.

## PHPStan compatibility

StirPHP is its own engine. It doesn't run PHPStan, and it doesn't need PHP installed. What's shared:

- **Configuration:** `level`, `paths`, `excludePaths`, `ignoreErrors`, `phpVersion` and `includes` are read from NEON.
  Includes written in PHP are skipped, because reading them would run your code.
- **PHPDoc:** PHPStan's types and tags, such as generics, array shapes, `@template` and `@phpstan-*` tags.
- **Identifiers:** a check that means the same as a PHPStan rule reports PHPStan's identifier, such as `return.type` or
  `function.notFound`. Checks only StirPHP has report codes such as `SPHP0061`. Rule levels come from these.
- **Ignoring:** `@phpstan-ignore return.type` on the line or the line before, `@phpstan-ignore-line` and
  `@phpstan-ignore-next-line` all work. As in PHPStan, an identifier that doesn't match doesn't silence the line.

Ignore rules and baselines match on identifier, path and count, and never on the message, because StirPHP's messages
differ from PHPStan's. So:

- An `ignoreErrors` entry with `identifier`, `path` and `count` works, and so does one with only an `identifier`.
- A PHPStan baseline applies: its entries have all of these, and the message is ignored.
- An entry with only a `message`, or a `message` and an `identifier` but no `path`, is skipped, and a notice gives the
  number skipped.

The limits:

- PHPStan extensions and custom rules are PHP code, which can't run in this engine. A project that relies on them, such
  as framework extensions or strict rules, should run PHPStan alongside StirPHP. Until the rules that describe a
  framework's magic have an equivalent here, expect errors PHPStan's extensions would have explained away.
- The checks don't all exist yet, and those that exist don't always agree with PHPStan.

## PHP versions

The project's PHP version comes from the first of these that's set:

1. PHPStan's `phpVersion`: `80100`, or a range such as `{min: 80100, max: 80400}`
2. Composer's `config.platform.php`
3. Composer's `require.php`, using the lowest version it allows
4. Otherwise, the latest

Syntax and library functions are checked against it:

```text
error[classConstant.nativeTypeNotSupported]: PHP 8.3 added typed class constants, but this project targets PHP 8.2
error[SPHP0061]: PHP 8.4 added `array_find()`, but this project targets PHP 8.2
```

## Composer

StirPHP reads `composer.json` and what `composer install` leaves in `vendor/`, including autoload rules and the list of
installed packages. It never runs Composer.

If packages are declared but not installed, or aren't up to date, it says so and tells you to run `composer install`.
Until then, errors about what those packages would declare are held back, so you don't get an error for each missing
class. Other errors are still reported. `stirphp check` prints this notice and the number held back, and the language
server shows it in the editor.

## Editors

`stirphp lsp` is a language server over stdin and stdout. It offers:

- diagnostics for the project's files, updated as you type
- hover with types and doc comments
- completion of members, variables, functions, classes and keywords
- signature help and inlay hints for inferred types
- go to definition and type definition, including into packages and PHP's own stubs
- find references, highlights and rename
- semantic highlighting, document and workspace symbols, folding and selection ranges

The [VS Code extension](editors/vscode) is the only client this has been tested with. Any editor that can start an LSP
server for PHP files should work with the command `stirphp lsp`, but that's untested.

## Current state

This is early, and accuracy against PHPStan is still being improved. On Laravel's framework, Symfony and PHPStan's own
source, each with its own configuration, it reports about 8500, 1300 and 700 problems, in about 4 to 5 seconds each. Those
include false alarms, at a rate nobody has measured yet. Reducing them is the current work. If `stirphp check` is wrong
about your code, please [open an issue](https://github.com/StirPHP/core/issues) with a small example.

Not there yet: formatting, quick fixes and refactoring beyond rename, framework support, and custom rules.

## Layout

| Crate | Purpose |
|---|---|
| `stir_syntax` | lexer and error-tolerant parser, producing a lossless syntax tree |
| `stir_phpdoc` | PHPDoc parser |
| `stir_neon` | NEON and PHPStan configuration loader |
| `stir_types` | type representation and relations |
| `stir_db` | salsa database: files, parsing, diagnostics, settings |
| `stir_hir` | declarations, name resolution, and PHPDoc and stub attribute lowering |
| `stir_stubs` | embedded phpstorm-stubs |
| `stir_checker` | the type checker |
| `stir_composer` | reads Composer's files |
| `stir_project` | configuration, discovery, baselines, `init` and workspace loading |
| `stir_lsp` | the language server |
| `stir_wasm` | the checker, hover and completion for the browser (WebAssembly) |
| `stir_cli` | the `stirphp` binary |

## Tests

```sh
cargo test --workspace
scripts/parse-corpus.sh                 # parse Laravel, Symfony and PHPStan (git submodules)
scripts/check-corpus.sh <label>         # check them and write a summary to .corpus-results/<label>
scripts/diff-corpus.py <label> <label>  # compare two summaries
```

Checker fixtures live in `crates/stir_checker/tests/fixtures/` and mark expected diagnostics inline:
`return 1; // error: return.type`.

`STIRPHP_LOG` sets the log level, on stderr, as in `STIRPHP_LOG=debug stirphp check`.

## License

StirPHP is licensed under the [MIT license](LICENSE).

The `stirphp` binary embeds JetBrains' [phpstorm-stubs](https://github.com/JetBrains/phpstorm-stubs) (a git submodule of
`crates/stir_stubs`), which are licensed under the [Apache License 2.0](crates/stir_stubs/phpstorm-stubs/LICENSE).
