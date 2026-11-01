# StirPHP

StirPHP is to PHP what TypeScript is to JavaScript: a strictly typed superset
of PHP that is type-checked and transpiled to plain PHP (with PHPStan-compatible
PHPDoc). Plain `.php` files typed with PHPStan PHPDoc are first-class, the way
TypeScript treats `.js` + JSDoc.

The toolchain is written in Rust; the binary is `sphp`.

## Install

Linux and macOS:

```sh
curl -fsSL https://stirphp.github.io/core/install.sh | sh
```

Windows (PowerShell):

```powershell
irm https://stirphp.github.io/core/install.ps1 | iex
```

This installs the latest release for your platform, after checking it against the
release's `SHA256SUMS`, and needs no admin rights. Set `SPHP_VERSION` to a version
or to `nightly`, and `SPHP_INSTALL_DIR` to choose where it goes. The scripts are
[`install.sh`](install.sh) and [`install.ps1`](install.ps1); archives for every
platform are on the [releases page](https://github.com/StirPHP/core/releases).

To build from source:

```sh
git submodule update --init --depth 1 crates/stir_stubs/phpstorm-stubs
cargo install --path crates/stir_cli
```

## Usage

```sh
sphp init                  # create sphp.toml and src/main.sphp (interactive; --yes for defaults)
sphp check                 # type-check the project (--format json, --max-errors N)
sphp build                 # transpile src/ to build/ (+ source maps)
sphp build --watch         # rebuild on changes
sphp build --inline-extensions
sphp lsp                   # language server over stdin/stdout
```

`sphp.toml`:

```toml
srcDir = "src"
outDir = "build"
allowPhp = true          # load .php files from srcDir
checkPhp = false         # also check them (or opt in per file with `// @sphp-check`)
inlineExtensions = false
# phpstan = "phpstan.neon"   # level and ignoreErrors apply to .php files
```

Generated code targets PHP 8.4. Non-inlined calls into the standard library
`require_once` a generated `build/stir-runtime.php`.

## Editor support

`sphp lsp` is a language server for `.sphp` and `.php` files. It finds the
project from the workspace folder's `sphp.toml` (or uses defaults), and offers:

- diagnostics for every project file, updated as you type
- hover with types (narrowed by smart casts), signatures and doc comments
- go to definition / type definition, including into the stdlib and PHP stubs
- find references, highlights and rename (keeping `$` on mutable names)
- completion of members after `->`/`::` (extension functions included), locals,
  functions, classes and keywords
- signature help, inlay type hints for `val`/`var` and lambda parameters
- semantic highlighting (immutable bindings are `readonly`, mutable ones carry
  a `mutable` modifier), document/workspace symbols, folding and selection ranges

VS Code: the extension in [`editors/vscode`](editors/vscode) registers `.sphp`
files and starts `sphp lsp` (see its README to build and install it).

Neovim (0.11+):

```lua
vim.filetype.add({ extension = { sphp = "sphp" } })
vim.lsp.config("sphp", {
  cmd = { "sphp", "lsp" },
  filetypes = { "sphp", "php" },
  root_markers = { "sphp.toml", ".git" },
})
vim.lsp.enable("sphp")
```

Helix (`languages.toml`):

```toml
[language-server.sphp]
command = "sphp"
args = ["lsp"]

[[language]]
name = "sphp"
scope = "source.sphp"
file-types = ["sphp"]
roots = ["sphp.toml"]
language-servers = ["sphp"]
```

Any other LSP client works the same way: run `sphp lsp` for `.sphp` files.

## The language in one screen

```
namespace App

val name: String = "world"          // immutable: no `$`
var $count = 0                       // mutable: always written with `$`
$count += 1

function greet(name: String, mut times: Int = 1): String {   // params are immutable unless `mut`
    var $out = ""
    while (times > 0) {
        $out = $out . "Hello, ${name}! "   // templates: only `${...}`, no `$` inside
        times -= 1
    }
    return $out
}

function double(x: Int): Int = x * 2      // expression bodies

// StirPHP types (`Int`, `String`, `List<T>`, ...) never coerce and have methods;
// PHP types (`int`, `string`, ...) keep PHP semantics. Int → int is fine, int → Int is not.
val n: Int = strlen(name)

// Null safety; untyped PHP values are `Any!` and must be narrowed
val maybe: String? = null
echo maybe?->length() ?: 0
val raw = json_decode("{}")         // Any!
if (raw is Map<String, Int>) { echo raw->size() }

// Generics with bounds and variance; no top type (`mixed`/`Any` are not allowed)
interface Source<out T> { function next(): T }
interface Shape { function area(): Float }
function <T of Shape> biggest(a: T, b: T): T = if (a->area() > b->area()) a else b

// Records are C-like structs (structural, copied by value) and lower to arrays
record Point { x: Int; y: Int }
var $p = Point { x = 1, y = 2 }
$p->x = 10

// Classes: primary constructors, final by default, `open` to extend
open class Base(val id: Int)
data class User(val id: Int, val name: String)
sealed interface Result<out T>
data class Ok<T>(val value: T) : Result<T>
data class Err(val msg: String) : Result<Nothing>

function show(r: Result<Int>): String = when (r) {   // exhaustive over sealed types
    is Ok -> "ok ${r->value}"
    is Err -> "err ${r->msg}"
}

// Lambdas, trailing lambdas, `it`, extension functions (inlinable by construction)
val doubled = listOf(1, 2, 3)->map { it * 2 }
function String.shout(): String = this->uppercase() . "!"
val pair = "a" to 1
```

See `stdlib/` for the standard library (methods on `String`, `Int`, `List`, `Map`, ...),
all written in StirPHP.

## Layout

| Crate | Purpose |
|---|---|
| `stir_syntax` | lexer (logos) and error-tolerant parser → lossless rowan tree, PHP 8.5 + StirPHP |
| `stir_phpdoc` | PHPStan PHPDoc parser |
| `stir_neon` | NEON / `phpstan.neon` loader |
| `stir_types` | type representation and relations (assignability, comparability, narrowing) |
| `stir_db` | salsa database: file inputs, parsing, diagnostics, settings |
| `stir_hir` | declarations, name resolution, PHPDoc and phpstorm-stubs attribute lowering |
| `stir_stubs` | embedded phpstorm-stubs and the StirPHP stdlib |
| `stir_checker` | the type checker |
| `stir_emit` | PHP output AST, printer and the StirPHP → PHP lowering |
| `stir_project` | `sphp.toml`, discovery, composer autoload, `init`, workspace loading |
| `stir_lsp` | the language server (tower-lsp-server; the editor features build without it) |
| `stir_wasm` | the checker and compiler for the browser (WebAssembly), for the docs playground |
| `stir_cli` | the `sphp` binary |

## Tests

```sh
cargo test --workspace
SPHP_TEST_PHP=php:8.4-cli cargo test -p stir_emit --test programs   # run transpiled programs with PHP in docker
scripts/parse-corpus.sh                                              # parse Laravel/Symfony/PHPStan (git submodules)
```

Checker fixtures live in `crates/stir_checker/tests/fixtures/` and mark expected
diagnostics inline: `$x = "no" // error: SPHP0005`.

## License

StirPHP is licensed under the [MIT license](LICENSE).

The `sphp` binary embeds JetBrains'
[phpstorm-stubs](https://github.com/JetBrains/phpstorm-stubs) (a git submodule
of `crates/stir_stubs`), which are licensed under the
[Apache License 2.0](crates/stir_stubs/phpstorm-stubs/LICENSE).
