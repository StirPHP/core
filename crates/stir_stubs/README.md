# stir_stubs

Embedded signatures of PHP's builtin functions, classes and constants, used by
the StirPHP type checker. See the crate docs (`src/lib.rs`) for the API and for
how to interpret the stubs' version attributes.

## Attribution

The stub files come from [JetBrains/phpstorm-stubs](https://github.com/JetBrains/phpstorm-stubs),
Copyright JetBrains s.r.o. and contributors, licensed under the
[Apache License 2.0](phpstorm-stubs/LICENSE). They are vendored unmodified as a
git submodule in `phpstorm-stubs/`; only the extensions listed in
`src/extensions.rs` are embedded into the binary.

`phpstan-stubs/` has PHPStan's stubs for PHP's classes, copied unmodified from
[phpstan/phpstan-src](https://github.com/phpstan/phpstan-src)'s `stubs/`, Copyright Ondřej
Mirtes and PHPStan s.r.o., licensed under the [MIT license](phpstan-stubs/LICENSE).
`src/overrides.php` has some return types from its function map too.

After cloning, fetch them with:

```sh
git submodule update --init --depth 1 crates/stir_stubs/phpstorm-stubs
```
