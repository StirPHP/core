//! Signatures of PHP's builtin functions, classes and constants, as PHP stub
//! source embedded from [JetBrains/phpstorm-stubs] (Apache-2.0, vendored as a
//! git submodule at `crates/stir_stubs/phpstorm-stubs`).
//!
//! Only the extensions listed in [`EXTENSIONS`] are embedded. Consumers parse
//! the stub text (it is ordinary PHP: declarations with empty bodies plus
//! PHPDoc) and build their symbol tables from it.
//!
//! # Embedding
//!
//! `build.rs` walks each listed extension directory and generates one
//! `include_str!` entry per `.php` file, sorted by path. This was chosen over
//! the `include_dir` crate because `include_dir!` needs one literal path per
//! directory (so the extension list could not be a single `const`), embedding
//! the whole submodule instead would pull in hundreds of irrelevant
//! extensions plus tests, and the generated table is a plain `&'static [_]`
//! with no runtime work or dependencies. The embedded text is roughly 10 MB.
//!
//! # Interpreting phpstorm-stubs attributes
//!
//! The stubs describe every PHP version at once and use attributes from
//! `JetBrains\PhpStorm` to say which declaration applies where. For a PHP
//! 8.3+ target a consumer should treat them as follows. Resolve attribute
//! names through the file's `use` imports: a few files alias them, e.g.
//! `use JetBrains\PhpStorm\Internal\PhpStormStubsElementAvailable as
//! ElementAvailable;`, `LanguageLevelTypeAware as LanguageAware`,
//! `ExpectedValues as EV`.
//!
//! - `#[PhpStormStubsElementAvailable(from: 'X.Y', to: 'X.Y')]` (also the
//!   positional form `('8.0')`, meaning `from`): the function, method,
//!   parameter, or class constant exists only for versions in the inclusive
//!   range. Both bounds are optional. The same function may be declared twice
//!   with disjoint ranges, or have two parameters with the same name. Drop
//!   any element not available on the target, *before* checking for duplicates.
//! - `#[LanguageLevelTypeAware(['8.0' => 'T1', '8.1' => 'T2'], default: 'T0')]`
//!   on a parameter, return, or property: the native type is the entry with
//!   the greatest version `<=` target, else `default`. An empty string means
//!   "no native type" (fall back to the PHPDoc type). The declared native type
//!   next to the attribute, if any, should be ignored in favour of this.
//! - `#[TentativeType]` on a method: the return type is tentative (8.1+);
//!   overriding methods without a compatible return type get a deprecation,
//!   not an error. Treat the declared type as the return type.
//! - `#[Deprecated(reason: ..., replacement: ..., since: 'X.Y')]`: deprecated
//!   from `since` (or always, without `since`). Report usage as a warning.
//! - `#[Pure]` / `#[Pure(true)]`: no side effects (`true`: may depend on
//!   global state). Useful for "unused result" diagnostics only.
//! - `#[ArrayShape(['key' => 'type', ...])]`: the array's keys and value
//!   types; equivalent to a PHPDoc `array{key: type}` shape.
//! - `#[ExpectedValues([...])]` / `#[ExpectedValues(flags: [...])]`: the set of
//!   allowed values (or bit flags); purely advisory.
//! - `#[Immutable]`: properties are not written after construction (akin to
//!   `readonly`). `#[Language]`, `#[NoReturn]` (return type `never`),
//!   `#[ReturnTypeContract]`, `#[TypeContract]` and `#[FileReference]` are
//!   IDE hints and may be ignored or mapped where convenient.
//!
//! PHPDoc tags (`@return`, `@param`, ...) carry the richer types. `@since X.Y` and
//! `@removed X.Y` say which PHP versions have an element in the extensions for which
//! [`ships_with_php`] holds: elsewhere those numbers are the extension's own versions.
//!
//! [JetBrains/phpstorm-stubs]: https://github.com/JetBrains/phpstorm-stubs

mod extensions;
pub mod frameworks;

pub use extensions::{EXTENSIONS, PHP_EXTENSIONS};

/// The functions the `phpstan/phpstan` package declares inside its PHAR, where no source
/// scan finds them: for projects that have that package installed.
pub const PHPSTAN_HELPERS: &str = include_str!("phpstan_helpers.php");

/// One embedded stub file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StubFile {
    /// The extension directory it comes from, an entry of [`EXTENSIONS`].
    pub extension: &'static str,
    /// Path relative to the phpstorm-stubs root, e.g. `"standard/standard_0.php"`.
    pub path: &'static str,
    /// The file's full source text.
    pub text: &'static str,
}

include!(concat!(env!("OUT_DIR"), "/stubs.rs"));

/// All embedded stub files, grouped by extension in [`EXTENSIONS`] order and
/// sorted by path within each extension.
pub fn stub_files() -> impl Iterator<Item = StubFile> {
    let ours = PHPSTAN_STUBS
        .iter()
        .map(|(path, text)| StubFile {
            extension: "override",
            path,
            text,
        })
        .chain(std::iter::once(StubFile {
            extension: "override",
            path: "override/functions.php",
            text: include_str!("overrides.php"),
        }));
    FILES.iter().copied().chain(ours)
}

/// PHPStan's stubs for PHP's classes (MIT, `phpstan-stubs/LICENSE`): what's generic, and what
/// phpstorm-stubs says less of. Each is laid over the phpstorm-stubs declaration, method by method.
const PHPSTAN_STUBS: &[(&str, &str)] = &[
    (
        "override/phpstan/ArrayObject.stub",
        include_str!("../phpstan-stubs/ArrayObject.stub"),
    ),
    (
        "override/phpstan/Countable.stub",
        include_str!("../phpstan-stubs/Countable.stub"),
    ),
    (
        "override/phpstan/Exception.stub",
        include_str!("../phpstan-stubs/Exception.stub"),
    ),
    (
        "override/phpstan/ImagickPixel.stub",
        include_str!("../phpstan-stubs/ImagickPixel.stub"),
    ),
    (
        "override/phpstan/Memcached.stub",
        include_str!("../phpstan-stubs/Memcached.stub"),
    ),
    (
        "override/phpstan/PDOStatement.stub",
        include_str!("../phpstan-stubs/PDOStatement.stub"),
    ),
    (
        "override/phpstan/Redis.stub",
        include_str!("../phpstan-stubs/Redis.stub"),
    ),
    (
        "override/phpstan/ReflectionAttribute.stub",
        include_str!("../phpstan-stubs/ReflectionAttribute.stub"),
    ),
    (
        "override/phpstan/ReflectionClass.stub",
        include_str!("../phpstan-stubs/ReflectionClass.stub"),
    ),
    (
        "override/phpstan/ReflectionClassConstant.stub",
        include_str!("../phpstan-stubs/ReflectionClassConstant.stub"),
    ),
    (
        "override/phpstan/ReflectionEnum.stub",
        include_str!("../phpstan-stubs/ReflectionEnum.stub"),
    ),
    (
        "override/phpstan/ReflectionFunctionAbstract.stub",
        include_str!("../phpstan-stubs/ReflectionFunctionAbstract.stub"),
    ),
    (
        "override/phpstan/ReflectionMethod.stub",
        include_str!("../phpstan-stubs/ReflectionMethod.stub"),
    ),
    (
        "override/phpstan/ReflectionObject.stub",
        include_str!("../phpstan-stubs/ReflectionObject.stub"),
    ),
    (
        "override/phpstan/ReflectionParameter.stub",
        include_str!("../phpstan-stubs/ReflectionParameter.stub"),
    ),
    (
        "override/phpstan/ReflectionProperty.stub",
        include_str!("../phpstan-stubs/ReflectionProperty.stub"),
    ),
    (
        "override/phpstan/ReflectionType.stub",
        include_str!("../phpstan-stubs/ReflectionType.stub"),
    ),
    (
        "override/phpstan/SensitiveParameterValue.stub",
        include_str!("../phpstan-stubs/SensitiveParameterValue.stub"),
    ),
    (
        "override/phpstan/SplObjectStorage.stub",
        include_str!("../phpstan-stubs/SplObjectStorage.stub"),
    ),
    (
        "override/phpstan/WeakReference.stub",
        include_str!("../phpstan-stubs/WeakReference.stub"),
    ),
    (
        "override/phpstan/bcmath.stub",
        include_str!("../phpstan-stubs/bcmath.stub"),
    ),
    (
        "override/phpstan/date.stub",
        include_str!("../phpstan-stubs/date.stub"),
    ),
    (
        "override/phpstan/dom.stub",
        include_str!("../phpstan-stubs/dom.stub"),
    ),
    (
        "override/phpstan/ext-ds.stub",
        include_str!("../phpstan-stubs/ext-ds.stub"),
    ),
    (
        "override/phpstan/iterable.stub",
        include_str!("../phpstan-stubs/iterable.stub"),
    ),
    (
        "override/phpstan/mysqli.stub",
        include_str!("../phpstan-stubs/mysqli.stub"),
    ),
    (
        "override/phpstan/spl.stub",
        include_str!("../phpstan-stubs/spl.stub"),
    ),
    (
        "override/phpstan/zip.stub",
        include_str!("../phpstan-stubs/zip.stub"),
    ),
];

/// The embedded extension directory names.
pub fn extensions() -> &'static [&'static str] {
    EXTENSIONS
}

/// Looks up a stub file by its path relative to the phpstorm-stubs root.
pub fn stub_file(path: &str) -> Option<StubFile> {
    FILES.iter().find(|f| f.path == path).copied()
}

/// Is `extension` developed and versioned with PHP itself?
pub fn ships_with_php(extension: &str) -> bool {
    PHP_EXTENSIONS.contains(&extension)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn php_extensions_are_embedded() {
        for ext in PHP_EXTENSIONS {
            assert!(EXTENSIONS.contains(ext), "{ext} isn't embedded");
        }
    }

    #[test]
    fn listing_is_consistent() {
        let files: Vec<_> = stub_files().collect();
        assert!(files.len() > 100);
        for ext in extensions() {
            assert!(
                files.iter().any(|f| f.extension == *ext),
                "no files for {ext}"
            );
        }
        for f in &files {
            assert!(f.path.starts_with(&format!("{}/", f.extension)));
        }
        let mut paths: Vec<_> = files.iter().map(|f| f.path).collect();
        paths.dedup();
        assert_eq!(paths.len(), files.len());
        assert_eq!(
            stub_file("standard/standard_0.php").unwrap().extension,
            "standard"
        );
        assert!(stub_file("nope.php").is_none());
    }
}
