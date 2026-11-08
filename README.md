# StirPHP

StirPHP is a type checker and language server for PHP. It reads your `.php` files, their PHPDoc and your Composer
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
stirphp check --with-phpstan        # run the project's PHPStan too; see Running PHPStan
```

The levels follow PHPStan's, 0 to 10, and `level: max` in a configuration means 10.

`--generate-baseline` writes `identifier`, `path` and `count` entries to `stirphp-baseline.neon` next to the configuration,
or to the path you give it, and makes `parameters.stirphp.baseline` name the file. It creates `stirphp.neon` if there's no
configuration. From then on an error is reported once a file has more of that identifier than its count, and a warning
says which entries have fewer than their count and can go. Errors PHP itself can't run past, such as syntax errors, can't
be put in a baseline or ignored.

A file with `// @stirphp-nocheck` in its leading comments isn't checked for types, though its declarations are still known.

`stirphp lsp` runs the language server, and `stirphp init` writes a configuration, as below.

## `stirphp fmt`

```sh
stirphp fmt                  # the project's sources
stirphp fmt src tests/A.php  # these files and folders
stirphp fmt --check          # change nothing, list what would change, and fail if anything would
stirphp fmt --indent tab     # or a number of spaces, over `parameters.stirphp.indent`
stirphp fmt -                # from standard input to standard output
```

It formats in the style of PER-CS, and it only changes whitespace: every token stays as it is, comments and trailing commas
included, and strings, heredocs and inline HTML aren't touched. A file with syntax errors is left alone. It normalizes spaces,
indentation, blank lines, brace placement and one statement to a line. It doesn't wrap long lines: line breaks inside
expressions stay where they are, and lines continuing an expression keep their indentation relative to its start. Alignment
before `=` and `=>` stays, as PER-CS allows it. In files with HTML around their PHP, only the spaces within lines change.

Run over everything in `corpus/` (about 54,000 files, some of them deliberately odd), tokens didn't change, a second run
changed nothing, and nothing that parsed stopped parsing. It isn't configurable beyond the indentation, and isn't
PhpStorm-compatible; PER-CS is the style.

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
        rules: [rules/no_var_dump.rhai]  # see Custom rules
        indent: 4                   # or `tab`, for `stirphp fmt`; in an editor, the default is the editor's
        deprecations: true          # report the use of what's deprecated; see PHPStan compatibility
        phpstan: true               # or `false`, or settings; see Running PHPStan
        frameworks: {laravel: false}  # turn a framework's support off, or on; see Frameworks
```

An unknown key is an error.

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

StirPHP is its own engine, and it doesn't need PHP installed or PHPStan to work. If a project has PHPStan, it can
[run alongside](#running-phpstan). What's shared:

- **Configuration:** `level`, `paths`, `excludePaths`, `ignoreErrors`, `phpVersion`, `stubFiles` and `includes` are read
  from NEON. Includes written in PHP are skipped, because reading them would run your code.
- **Stubs:** what a stub file says of a class is laid over the class, method by method, and a function in one replaces the
  function. PHPStan's stubs for PHP's own classes are built in, so `FilterIterator<K, V, I>` and the like are generic.
- **Deprecations:** with `parameters.stirphp.deprecations: true`, or `phpstan-deprecation-rules` among the configured
  extensions, what's marked `@deprecated` or `#[Deprecated]` is a warning with that package's identifiers
  (`function.deprecated`, `method.deprecated`, `new.deprecatedClass` and so on). Without that, `stirphp check` says nothing
  about it, and an editor strikes it through either way.
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

- PHPStan extensions and custom rules are PHP code, which can't run in this engine. StirPHP has its own support for
  [Laravel, Symfony and WordPress](#frameworks), covering part of what larastan, phpstan-symfony, phpstan-doctrine and
  phpstan-wordpress do. For the rest, and for other extensions such as strict rules, [run PHPStan alongside
  StirPHP](#running-phpstan), or write what a project needs as [scripts](#custom-rules).
- The checks don't all exist yet, and those that exist don't always agree with PHPStan. Conditions and comparisons that can
  only go one way are reported only where that's certain, so far fewer of them are found. A project with
  `treatPhpDocTypesAsCertain: false` gets none of them, because types here don't say which came from PHPDoc.
  `missingType.generics`, `missingType.callable` and `missingType.checkedException` aren't reported at all.

## Running PHPStan

A project that has PHPStan can get its full answer in the editor, or from `stirphp check`, with StirPHP's. This runs the
PHPStan the project has installed, with the project's configuration and level. It's there for what StirPHP doesn't have, such
as extensions and checks it hasn't got yet, and the same error isn't shown twice.

**In an editor** it's on for a project with both PHPStan's configuration (`phpstan.neon`, `phpstan.neon.dist` or
`phpstan.dist.neon`, in the project's folder) and PHPStan among its Composer packages, once you've said it can run. That's
because running it means running code from the project: the `vendor/bin/phpstan` that's there, and the bootstrap files and
extensions its configuration loads. So you're asked first, showing the command, separately from
[custom rules](#custom-rules), as this has no sandbox. The answer is kept for that workspace, for that binary and that
configuration file as they are, and you're asked again when either changes. What's in bootstrap files, extensions, included
files and `vendor/` isn't part of it, so a change there doesn't ask again. In a workspace the editor doesn't trust
(`"workspaceTrusted": false` in `initializationOptions`), it never runs and nothing's asked. A client that can't ask gets one
notice, and it doesn't run.

**On the command line**, `stirphp check --with-phpstan` runs it, which is your choice, as with running PHPStan by hand.
`stirphp check` alone never does. If it can't give an answer, what StirPHP found is still shown, the reason goes to stderr, and
the exit code is 1. With `--format json`, what PHPStan reported has `"source": "phpstan"`. `--level` and `-c` are passed on,
except that a `stirphp.neon` isn't, since PHPStan refuses `parameters.stirphp` unless that's declared. It can't be combined with
`--generate-baseline`.

Which PHPStan: `parameters.stirphp.phpstan.binary` or an editor's setting (which wins), then Composer's `vendor/bin/phpstan`, or
`bin-dir`, then the package's own script. Never one that's only on your `PATH`, unless you name it, and StirPHP never installs
anything. All of these are optional:

```neon
parameters:
    stirphp:
        phpstan:
            enabled: true                # say so if it can't be found; `false` never runs it, in an editor
            binary: tools/phpstan.phar   # relative to this file; without the executable bit it's run by `php`
            configuration: phpstan.ci.neon
            level: max                   # over the one in the configuration
            memoryLimit: 1G              # for each of its processes; default 2G, -1 for none
            timeout: 300                 # seconds for a run; default 600
```

What happens in an editor:

- **When it runs:** when the project is opened, and whenever a PHP file of the project is saved in the editor, never as you
  type. A change made elsewhere, such as `git pull`, is noticed at the next save. It analyses what PHPStan would with
  `phpstan analyse`, not only the saved file, since a change in one file can cause errors in others.
- **One at a time:** a save during a run waits for it, and then there's one more run, however many saves there were. A run
  isn't killed for a save because a killed run leaves no result cache, and with a slow first run there'd never be any results.
  It's killed with all its worker processes when it takes too long, when the settings or configuration change, and when the
  server stops. If the server itself is killed with `SIGKILL`, the run can only end by itself.
- **Stale lines:** PHPStan reads files from disk and only gives line numbers. So what it said of a file is only shown while the
  editor has the text it read: type in a file and PHPStan's errors in it go away, until the next save's run is done. StirPHP's
  own stay and keep up.
- **Failures:** if it's not there, crashes, runs out of memory or time, has a broken configuration or a PHP it can't run on, you
  get one notice, and no more of that kind until it's worked again. PHP's warnings around the JSON don't matter. What it
  said before stays until it says something else, and StirPHP's diagnostics carry on regardless.
- **Results:** errors have the source `phpstan` and link to [PHPStan's page for the identifier](https://phpstan.org/error-identifiers),
  which doesn't exist for a third party's identifiers. They cover the line, as that's all PHPStan gives. Errors about the run as
  a whole, such as an ignore pattern that matched nothing, show up at the top of its configuration file.

Duplicates:

- Two reports are the same if they have the same file, line and identifier. Messages play no part. StirPHP's stays, because it
  has a real range and keeps up as you type. For what's wrong in a doc comment, either the comment's line or the line of what
  it documents counts, because PHPStan reports the latter. A syntax error counts as the same as PHPStan's `phpstan.parse`
  anywhere in the file.
- One of StirPHP's removes one of PHPStan's. Two on a line with one identifier may be different errors.
- A report with no identifier is never a duplicate. Neither is a report of StirPHP's own, such as `SPHP0061`, so PHPStan's
  error about the same thing under another identifier is shown too. Nor are the same error on different lines, as with a call
  over several lines: on Laravel's framework at level 5, 1593 of PHPStan's 2884 errors were removed, and 97 of the other 1291 had
  one of StirPHP's with its identifier within 3 lines, some of which really are other errors.
- What StirPHP reports and PHPStan doesn't stays. PHPStan's silence doesn't take anything away, and the checks don't all agree yet.

Baselines and ignore rules don't cross over. PHPStan applies its own to what it reports, and StirPHP's apply to what StirPHP
reports. So an error StirPHP's baseline hides still shows if PHPStan reports it, unless PHPStan's baseline has it too, and
`// @stirphp-nocheck` only quiets StirPHP.

On Laravel's framework with its own configuration (level 1), on 16 cores: 6 seconds for a first run, with 17 processes of up to
420 MB each, and under a second with PHPStan's result cache when nothing or a little changed. A change to `Collection`, which
much depends on, took under 4 seconds.

## Custom rules

Rules and framework knowledge are scripts in [Rhai](https://rhai.rs), listed in the configuration. Paths are relative to
the file that lists them:

```neon
parameters:
    stirphp:
        rules:
            - examples/rules/no_var_dump.rhai
```

A rule is a function named after the kind of node it wants to see, and only those nodes are visited:

```rhai
// Runs on every function call, and on nothing else.
fn visit_func_call(node, ctx) {
    if node.name.to_lower() == "var_dump" {
        ctx.error("Don't leave var_dump in", "app.noVarDump");
    }
}
```

Reports go the way of any other diagnostic, under the identifier you give, so `@phpstan-ignore app.noVarDump`,
`ignoreErrors` and baselines all apply. A report counts at every level unless it says otherwise, as in
`ctx.error(message, identifier, #{ level: 6 })`, and `#{ node: other }` reports somewhere else than the node visited.
`ctx.warning(..)` is the same at a lower severity.

- **Kinds of node:** `func_call`, `method_call`, `static_call`, `new`, `property_fetch`, `static_property_fetch`,
  `class_const_fetch`, `const_fetch`, `class`, `interface`, `trait`, `enum`, `function`, `method`, `property`, `variable`,
  `assign`, `binary_op`, `echo`, `return`, `foreach` and so on. A wrong one is an error that lists them all.
- **Nodes:** `node.kind`, `.name`, `.text`, `.range` (`start`, `end`, `line`, `column`), `.arguments`, `.children`,
  `.receiver`.
- **Types:** `ctx.get_type(node)` gives `to_string()`, `is_null()`, `is_nullable()`, `is_unknown()`, `is_string()`,
  `is_int()`, `is_float()`, `is_bool()`, `is_array()`, `is_object()`, `is_subtype_of("Foo")`, `without_null()`,
  `class_name`, `class_string`, `literal`, `members()`, `has_method("x")` and `has_property("x")`. Rhai reserves `type_of`
  and `call`, so that's `get_type`, and a parameter can't be named `call`. Its map keys can't be `static` either.

A hook tells the checker what it can't know from the code. These are PHPStan's dynamic return type extensions, answering
with a PHPDoc type as a string, such as `"list<App\\User>"`. Names in it are complete, as no file's `use` applies:

```rhai
// `$container->get(Foo::class)` returns a `Foo`, and a subclass's `get` counts too.
fn return_type_targets() {
    #{ methods: ["App\\Container::get"] }
}

fn return_type_of_method(node, ctx) {
    // nothing, for anything else: the declared return type stands
    ctx.get_type(node.arguments[0]).class_string
}
```

`return_type_targets()` can also list `static_methods` and `functions`, for `return_type_of_static_method` and
`return_type_of_function`. `ctx.get_type` knows the receiver and the arguments of the call, and `ctx.declared_return_type`
is what the signature says.

Magic members are PHPStan's class reflection extensions: lookups, hover and completion see them on a class and its
subclasses, as if `@property` and `@method` tags said so. Anything PHP itself declares comes first. A property's value is
a type or `#{ type, read_only, write, declared_at }`, a method's a return type or
`#{ returns, params, is_static, declared_at }`. `write` is what a property takes when it's set, if that's not its type, and
`declared_at` (`"App\\Post::author"`, or `"App\\Post::$title"` for a property) is where going to the definition leads:

```rhai
// What `__get` and `__call` make available on a model and its subclasses.
fn magic_members(class_name) {
    if class_name == "App\\Model" {
        #{
            properties: #{ id: #{ type: "int", read_only: true } },
            methods: #{ find: #{ returns: "?static", params: "int $id", is_static: true } },
        }
    }
}
```

Besides `properties` and `methods`, the answer can say `forward_calls: "App\\Query<static>"`: calls of methods the class
hasn't got, on an object or statically, are made on a value of that type instead, and give what they give there, as with
a model forwarding to its query builder. `static` in it is the class the call is made on. Completion offers that type's
public methods too.

What depends on a type's arguments, or on the name itself, is answered one name at a time. These run only where nothing
else has the member, `@mixin` types included, just before `__call` and `__get` would be reported:

```rhai
// `$query->whereEmail($email)`, on a query of any model
fn magic_method(type, name, is_static) {
    if type.class_name == "App\\Query" && name.starts_with("where") { return "$this"; }
}

// what a proxy has is what the value it's a proxy for has
fn magic_property(type, name) {
    if type.class_name == "App\\Proxy" { return #{ forward_to: type.type_args[0].to_string() }; }
}
```

`magic_method` answers like a method in `magic_members`, and `magic_property` like a property, or either with
`#{ forward_to, fluent }`: the member of that name on that type. With `fluent: true`, a call that gives back the value it
was forwarded to gives back the original one, as a relation does with its query. `global_type(name)` gives the type of a
global variable, for `global $name;`.

Scripts see the project's classes with `class("App\\Post")`, which is `()` for one that isn't there. A class has `name`,
`kind`, `parent`, `type`, `type_params`, `is_abstract`, `is_subtype_of("..")` (traits count), `ancestors()`, `methods()`,
`method("..")`, `properties()`, `property("..")`, `constant("..")` and `attributes()`, as `#{ name, args }` with arguments
by position (`"0"`) and by name. A method has `name`, `class`, `is_static`, `is_magic`, `visibility`, `return_type`,
`params`, `params_text(skip)` (as a `@method` tag writes them), `attributes()`, `returned_value()` and `returned_call()`,
the first call on `$this` of what it returns, such as `#{ name: "hasMany", args: #{ "0": "App\\Comment" } }`. A property
has `name`, `class`, `type`, `is_static`, `visibility` and `default_value`. Values are only what's written out literally:
strings, numbers, booleans, `null`, arrays of them, `Foo::class` as the class's name, and constants with such values.
Nothing's inferred from code, so that a class's members never depend on checking the code that uses them. Asking about many
methods is much cheaper all at once, with `method_details(names)`, or `methods_below("App\\Model")` for those a class has
that its ancestor hasn't. Types also have `type_args`, `types()` (a union's members), `method_type("..")` and
`callable_params()` (`#{ required, max }`, `max` being `-1` for any number).

Scripts are sandboxed. They can't reach files, the network or other programs, have no clock, randomness, `import` or
`eval`, and keep nothing between calls, so only their functions run and the same input gets the same answer. A call that
takes over 1,000,000 operations, goes deeper than 64 calls or builds huge values fails. So does any other error in one:
that's one `SPHP0070` at the script, and the script sits out that file while the analysis goes on. A script that can't be
read or doesn't compile is `SPHP0071`. `print` goes to the log (`STIRPHP_LOG=stirphp::script=debug`), and
`STIRPHP_LOG=info` says which scripts run.

`stirphp check` runs what's configured, as you chose to run it in that project. An editor asks first: see
[the extension's README](editors/vscode/README.md#custom-rules-and-workspace-trust). Other editors get the same question
through `window/showMessageRequest`, and without an answer the scripts stay off. An editor that knows a workspace isn't
trusted passes `"workspaceTrusted": false` in `initializationOptions`, and then they never run.

On Laravel's framework, Symfony and PHPStan's source, one rule on every function call and one return type hook add under
3% to the time of a run, and a rule call costs 14 to 34 microseconds of CPU time. So the editor runs them as part of its
diagnostics, per file, without a pass of their own.

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

## Frameworks

StirPHP ships support for Laravel, Symfony and WordPress: scripts like the ones above, inside the binary, with native code
where reading many files would be slow in a script. They're ours, so they run without anyone being asked, in editors too,
and they're never read from disk, so no file of a project can stand in for one. A project's own scripts come first and can
override what they say.

Each is on when the project uses that framework: `laravel/framework`, `symfony/framework-bundle`, `symfony/symfony` or
`symfony/dependency-injection` (unless the project is built on Laravel or Drupal), and for WordPress
`johnpbloch/wordpress-core`, `roots/wordpress`, `php-stubs/wordpress-stubs`, a `wp-config.php` or `wp-content` folder, or
a plugin's `Plugin Name:` header. `parameters.stirphp.frameworks` turns one off or on: `{laravel: false, wordpress: true}`.

**Laravel**, after what larastan does:

- Models get properties for the columns their migrations in `database/migrations` create (by `$table`, or the class's
  name in plural, as Laravel has it), their `$casts` and `casts()`, accessors (`getFullNameAttribute()` and
  `fullName(): Attribute`) and relations, typed as what the relation gives. They accept anything when set, as Laravel
  converts it. An attribute nothing declares is `mixed` rather than an error, as migrations aren't always the whole story.
- Scopes (`scopeActive()` and `#[Scope]`) are on the model and its queries, with their parameters. Calls a model hasn't got,
  statically or not, go to `Builder<Model>`, and a relation's go to a query of its related model; `where{Column}()` works
  on both, and so do query methods a model declares, such as `withTrashed()` from `SoftDeletes`.
- Facades forward to what their accessor names, by class or by the container's names for Laravel's services, and contracts
  such as `Illuminate\Contracts\Auth\Factory` to the class Laravel binds them to. `auth()->user()`, `Auth::user()` and
  `$request->user()` give the user model `config/auth.php` names. Laravel's class aliases, such as `\DB`, are there.
- Macros the project adds with `Foo::macro('name', ..)` aren't reported, though what they return isn't known.
- In a closure given to `Artisan::command()` or the console kernel's `command()`, `$this` is the `ClosureCommand` Laravel
  binds it to, so `$this->comment()` and `$this->argument()` resolve; not in a static closure, which PHP can't bind.
- In an editor, these complete, hover and lead to where they're declared: a relation's property to its method, a scope to its
  `scopeX` method. Going to the definition of `route('name')` finds `->name('name')` in `routes/`, and of `view('a.b')`
  the template in `resources/views`.

**Symfony and Doctrine**, after phpstan-symfony and phpstan-doctrine: services and parameters come from the compiled
container, `parameters.symfony.containerXmlPath` or else one in `var/cache/`, the `dev` environment's first, so
`$container->get('app.mailer')` and `$this->getParameter('kernel.debug')` have types. A container that hasn't been
compiled isn't compiled; then a `get(Foo::class)` is still a `Foo`. `getRepository(Product::class)` gives the repository
`#[ORM\Entity(repositoryClass: ..)]` names, and repositories have `findByX()`, `findOneByX()` and `countByX()`.

**WordPress**, after phpstan-wordpress: `php-stubs/wordpress-stubs` is loaded when it's installed, `global $wpdb;` and the
other globals WordPress sets up have their types, and `add_action()` and `add_filter()` are checked: a callback that isn't
a function, one that needs more arguments than `$accepted_args`, or takes fewer than it says, and `$accepted_args` above what
any `do_action()` or `apply_filters()` of that hook in the project passes (`wordpress.hookArguments`). Going to the
definition of a hook's name finds where it runs.

What's measured, with PHPStan and the extension at the same level:

- Laravel's framework repository, with no configuration: 8993 reported before, 4937 now. 4074 went, 3577 of which PHPStan
  without larastan reports on the same line with the same identifier: calls and properties only Laravel's magic has.
  18 are new, errors that the ones before stopped short of.
- [BookStack](https://github.com/BookStackApp/BookStack), an MIT-licensed Laravel application, at its own level 5: 149 before,
  36 now. PHPStan with larastan reports none. What's left isn't about Laravel's magic: argument types the checker gets wrong,
  such as Collection where an array is taken.
- A new Laravel application with models, relations, scopes, casts and a controller: 20 before, 3 now. Larastan reports 6.
  Both find the 3 mistakes written in on purpose. Larastan also reports an `Attribute` accessor without type arguments, a
  property of a relation without them, and a macro added outside a service provider, which are all fine.
- A new Symfony application with an entity, its repository and a controller, and a container: 5 before, 1 now, the same as
  PHPStan's.
- A WordPress plugin with hooks: none of its 3 mistakes before, all of them now, plus a hook's arguments PHPStan doesn't check.

On the framework repository, with thousands of models in its tests, a run takes about a fifth more CPU time, most of it in
the models' scripts and the extra checking of what used to be unknown. On BookStack it's within the noise, as it is on
Symfony's and PHPStan's repositories.

Not there yet: config keys of `config()`, the results of `$collection->map->name` (it's the property's type, not a
collection of them), relation names in `with()` and `has()`, route names in groups, `Request` input as more than `mixed`,
Doctrine's `@ORM` annotations in PHPDoc, Symfony forms, routes as a reason a method is used, and the return types of
`apply_filters()` from the hook's PHPDoc.

## Composer

StirPHP reads `composer.json` and what `composer install` leaves in `vendor/`, including autoload rules and the list of
installed packages. It never runs Composer.

If packages are declared but not installed, or aren't up to date, it says so and tells you to run `composer install`.
Until then, errors about what those packages would declare are held back, so you don't get an error for each missing
class. Other errors are still reported. `stirphp check` prints this notice and the number held back, and the language
server shows it in the editor.

## Editors

`stirphp lsp` is a language server over stdin and stdout. It offers:

- diagnostics for the project's files, updated as you type, with [PHPStan's](#running-phpstan) on save for a project that has it
- hover with types and doc comments
- completion of members, variables and keywords, and of functions, classes and constants from the whole workspace by fuzzy
  match, adding the `use` line a name needs in the right place
- signature help and inlay hints for inferred types
- go to definition and type definition, including into packages and PHP's own stubs
- go to implementation and declaration, and call and type hierarchies
- find references, highlights and rename
- formatting of a document or a range
- quick fixes and refactorings: see [Code actions](#code-actions)
- what's deprecated is marked in diagnostics, hover, completion and symbols
- semantic highlighting, document and workspace symbols, folding and selection ranges

The [VS Code extension](editors/vscode) is the only client this has been tested with. Any editor that can start an LSP
server for PHP files should work with the command `stirphp lsp`, but that's untested.

The same server runs in the browser, compiled to WebAssembly in a Web Worker: see [editors/web](editors/web). It answers
the same, from files the page hands it instead of a disk, but without PHPStan. It's been tried with Monaco and
`monaco-languageclient` in Firefox.

## Code actions

Quick fixes are offered on a diagnostic, and marked as preferred where one of them is clearly the fix:

- **Unknown names:** a class, function or constant that isn't known can be imported, one action for each that has that
  name in the workspace: the project's own first, then its dependencies', then PHP's and its declared extensions', and
  among those the nearest namespace first, with the `use` line put where completion puts it. Names that look like a typo
  get **Change to ...**, in the same order, for classes, functions, constants, methods (only those that could take the
  arguments), properties and variables, but not for members of a class whose `__call` or `__get` takes any name.
- **Imports:** an import nothing uses is greyed out in the editor (as a hint, `SPHP0080`, which `stirphp check` doesn't
  report) and can be removed, alone or with all the others. **Organize imports** (`source.organizeImports`) removes them
  and sorts what's left as completion's insertion does: classes, then functions, then constants, a blank line between.
  A comment stays with the line it's on or above, and lists and groups (`use A\{B, C}`) keep their form. An import counts
  as used if its name appears in the code in any way, or in a doc comment or a comment with a tag, so one may be kept that
  isn't used, but never the other way round.
- **Types left out:** a return type from what every `return` gives, when every path ends in one (or `void`, for none), a
  parameter's type from its PHPDoc or from what it overrides, and a property's from its PHPDoc or, for a private one, from
  everything assigned to it in its class. Only what the file's PHP version can say natively, and nothing a subclass or
  what's overridden would stop compiling over. What PHPDoc can say better, such as `list<string>`, goes in a doc comment,
  and so does a type only PHPDoc can say. These are also refactorings with the cursor on the name, for levels below 6.
- **What a class has to have:** a class that doesn't implement the abstract or interface methods it has to is now reported
  (`SPHP0062`, as PHP won't load it), and the fix writes them all, with the signatures and visibility they need, the
  imports for their types, and a body that throws a `LogicException` saying it's not implemented. A call of a method or
  property of the class itself that doesn't exist (`$this->foo()`, `self::bar()`) can create it, private, with
  parameters typed from the arguments where that's known.
- **Nullable types:** `T $x = null` (`parameter.implicitlyNullable`) as `?T`, `return null;` in something that doesn't
  allow it, and a property whose default is `null`.
- **Other:** `return null;` at the end of a function missing a return, if its type allows it (there's no value to make up
  for other types); removing code that can't be reached, unless something in it is declared or is a `goto` label; removing
  a variable that's assigned something without effects and never read.
- **Ignoring:** `// @phpstan-ignore <identifier>` on the line, added to one that's there, or on the line above when the
  line is in a heredoc or HTML. It's offered for every identifier that can be ignored, StirPHP's own codes too. Not for
  PHP's syntax errors, which can't be.
- **PHPStan's errors** get the same fixes where their identifier is one of these and their line can only be about one
  thing, as they have nothing more precise than a line; and **Ignore** always.
- **`source.fixAll`**, when an editor asks for it, as on save: removes unused imports and makes implicitly nullable
  parameters nullable, which can't change what the code does.

Refactorings, with the cursor on something or a selection:

- **Extract to a variable**, and the same value again in the statement if nothing in it can change it; **inline a
  variable** assigned once (anything if what it's assigned is constant, otherwise only into the next statement, first
  thing). Both refuse what would run something in another order, or less often, and say why.
- **Extract method** (or function, outside classes) on whole statements or an expression. See below.
- **Change signature:** remove a parameter nothing uses, move one left or right, add one, for a function or a method and
  everything it overrides or is overridden by, updating every call.
- **Classes:** generate a constructor from the properties without a value, promote properties to the constructor and
  back, getters and setters, override a parent's method, and add `#[\Override]` on PHP 8.3.
- **Flow:** flip `if` and `else`, turn an `if` at the end of a function or loop into an early `return` or `continue`,
  drop the `else` of an `if` that always leaves, and `switch` to `match` where that means the same: the subject and every
  `case` have to be ints, strings that can't be read as numbers, or enums, so that `==` and `===` agree, every case gives
  a value in the same way, without falling through to a body, and there has to be a `default`, as `match` throws where
  `switch` would do nothing.
- **Import a name** written in full, wherever it's written so in the namespace.

**Extract method** works out what the selection reads and writes in the order things run. What it reads before it's
certainly written, and that's certainly set before, becomes a parameter; what it writes that's read afterwards (or before,
around a loop) is returned, as an array that's destructured for more than one. A parameter gets a type only if that's
certain where the selection starts: the declared type of a parameter of the function that isn't reassigned, or what one
unconditional assignment before it gives. What a condition narrowed a variable to is left out, since a narrower type than
the value could have would throw. The method is private, and static if it doesn't need `$this`, and its name is one the
class and every class related to it don't have. `return` is taken if every path through the selection returns, and then the
call is returned. It refuses `break` and `continue` out of it, `yield`, `goto`, `global` and `static` variables,
references (`&`) to what it uses, `compact`, `extract`, `$$x`, `eval` and other ways of reaching variables by name,
`__FUNCTION__` and `__LINE__`, a selection in a closure (whose `$this` may be bound elsewhere), and a variable that might not
be set, or that carries over from one time round a loop to the next. A refused action is shown greyed out with the reason,
in an editor that can show that. In VS Code the name is selected for renaming straight after; elsewhere it's `extracted`.

**Change signature** has no dialog in the protocol, so each change is its own action. All calls are found through what the
checker resolved, across the project, including `parent::`, `new`, `new static`, attributes and a subclass's `new` that
uses the parent's constructor. If any can't be updated, nothing is changed and you're told where: a spread argument that
covers the parameters concerned, an argument with effects that removing it would drop, a call where both arguments being
swapped have effects, a first-class callable (`foo(...)`), a callable string or `[$x, 'foo']` given to PHP's functions,
and a call of a method of that name whose receiver's class isn't known. `$x->$name()` can't be seen at all. Named arguments
are updated by name. A new parameter with a default leaves the calls alone. Without one it's put before any optional
parameters, and calls pass `TODO_NEW_ARGUMENT`, a constant that doesn't exist, so each call is an error until it's given a
real value, rather than quietly running with a wrong one; an override passing its arguments on to `parent::` passes the new
one. VS Code asks for the parameter (`int $limit = 10`) and what calls should pass; other editors add `$newParameter`.

Every edit is checked to leave each file it touches parsing, or it isn't made. Edits use `documentChanges` with the
version of each open file when the editor can take that, and the file's line endings and indentation. Nothing is offered in
a file that has syntax errors.

Measured on Laravel's framework, Symfony and PHPStan's source, in memory, at diagnostics and a fixed sample of places and
selections in 400 files of each (`crates/stir_lsp/tests/actions_corpus.rs`): about 5,000 actions applied, every result
parsed, and about 25 left a new error in the file, which were all the checker seeing the moved code differently rather than
the code doing something else: what a condition had narrowed being lost in an extracted method or variable, or a typo's
correction being a method with another signature. Working out what's offered took 0.1 ms at the median and 15 ms at the
95th percentile, extracting a method 3 to 6 ms (under 60 ms at the 95th), and a change of signature, which looks at the
whole project, 40 to 400 ms once the project's been checked. Most of the changes of signature tried there were refused,
because a call of a method of that name was on something whose class isn't known, or a callable named it.

Not offered: adding `declare(strict_types=1)`, which isn't reported and changes what the code does; moving a class to the
file PSR-4 says, which would mean creating and renaming files, so it's left out; adding to a baseline; extracting from a
closure; and inlining methods.

## Current state

This is early, and accuracy against PHPStan is still being improved. On Laravel's framework, Symfony and PHPStan's own
source, each with its own configuration, it reports about 4900, 1300 and 1300 problems, in about 4 to 6 seconds each. Those
include false alarms, at a rate nobody has measured overall yet. Reducing them is the current work. If `stirphp check` is wrong
about your code, please [open an issue](https://github.com/StirPHP/core/issues) with a small example.

Not there yet: wrapping long lines when formatting, and the rest of what [frameworks](#frameworks) need.

## Layout

| Crate | Purpose |
|---|---|
| `stir_syntax` | lexer and error-tolerant parser, producing a lossless syntax tree |
| `stir_phpdoc` | PHPDoc parser |
| `stir_neon` | NEON and PHPStan configuration loader |
| `stir_types` | type representation and relations |
| `stir_db` | salsa database: files, parsing, diagnostics, settings |
| `stir_hir` | declarations, name resolution, and PHPDoc and stub attribute lowering |
| `stir_stubs` | embedded phpstorm-stubs, PHPStan's stubs for PHP's classes, and our scripts and stubs for frameworks |
| `stir_checker` | the type checker, and what scripts see of a project |
| `stir_script` | the sandbox scripts run in |
| `stir_composer` | reads Composer's files |
| `stir_phpstan` | runs a project's PHPStan and merges what it reports with ours (not in the browser) |
| `stir_fmt` | the formatter |
| `stir_vfs` | where a project's files are read from: the disk, or files in memory |
| `stir_project` | configuration, discovery, baselines, `init` and workspace loading |
| `stir_lsp` | the language server and its code actions, over stdio or one message at a time (the browser's) |
| `stir_wasm` | the language server for the browser (WebAssembly); [editors/web](editors/web) runs it in a Web Worker |
| `stir_cli` | the `stirphp` binary |

## Tests

```sh
cargo test --workspace
scripts/parse-corpus.sh                 # parse Laravel, Symfony and PHPStan (git submodules)
scripts/check-corpus.sh <label>         # check them and write a summary to .corpus-results/<label>
scripts/diff-corpus.py <label> <label>  # compare two summaries
cargo test --release -p stir_fmt --test corpus -- --ignored --nocapture  # format all of them, in memory
just web                                # build the browser's language server and test it in Node
```

Checker fixtures live in `crates/stir_checker/tests/fixtures/` and mark expected diagnostics inline:
`return 1; // error: return.type`. `// @framework laravel` in one turns on what we ship for that framework.

`STIRPHP_LOG` sets the log level, on stderr, as in `STIRPHP_LOG=debug stirphp check`.

## License

StirPHP is licensed under the [MIT license](LICENSE).

The `stirphp` binary embeds JetBrains' [phpstorm-stubs](https://github.com/JetBrains/phpstorm-stubs) (a git submodule of
`crates/stir_stubs`), which are licensed under the [Apache License 2.0](crates/stir_stubs/phpstorm-stubs/LICENSE).
