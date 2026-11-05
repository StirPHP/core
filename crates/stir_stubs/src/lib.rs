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
//! with no runtime work or dependencies. The embedded text is roughly 3.3 MB.
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
//!   any element for which [`is_available`] returns `false`, *before* checking
//!   for duplicates.
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
//! PHPDoc tags (`@return`, `@param`, `@since`, `@removed X.Y`, ...) carry the
//! richer types; `@removed` should be treated like `to:` one minor version
//! below the given one.
//!
//! [JetBrains/phpstorm-stubs]: https://github.com/JetBrains/phpstorm-stubs

mod extensions;

pub use extensions::EXTENSIONS;

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
    FILES.iter().copied()
}

/// The embedded extension directory names.
pub fn extensions() -> &'static [&'static str] {
    EXTENSIONS
}

/// Looks up a stub file by its path relative to the phpstorm-stubs root.
pub fn stub_file(path: &str) -> Option<StubFile> {
    FILES.iter().find(|f| f.path == path).copied()
}

/// Whether an element annotated with
/// `#[PhpStormStubsElementAvailable(from: ..., to: ...)]` exists on PHP
/// version `target` (`(major, minor)`). Both bounds are inclusive and
/// optional; a bound that is not a `major.minor` version is ignored.
pub fn is_available(from: Option<&str>, to: Option<&str>, target: (u8, u8)) -> bool {
    let from = from.and_then(parse_version);
    let to = to.and_then(parse_version);
    from.is_none_or(|v| target >= v) && to.is_none_or(|v| target <= v)
}

fn parse_version(s: &str) -> Option<(u8, u8)> {
    let s = s.trim().trim_matches(|c| c == '\'' || c == '"');
    let mut parts = s.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().map_or(Some(0), |m| m.parse().ok())?;
    Some((major, minor))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn availability_ranges() {
        assert!(is_available(None, None, (8, 3)));
        assert!(is_available(Some("8.0"), None, (8, 3)));
        assert!(!is_available(Some("8.4"), None, (8, 3)));
        assert!(is_available(Some("8.3"), Some("8.3"), (8, 3)));
        assert!(!is_available(Some("5.3"), Some("7.4"), (8, 3)));
        assert!(is_available(None, Some("8.5"), (8, 3)));
        assert!(!is_available(None, Some("8.2"), (8, 3)));
        assert!(is_available(Some("'8.0'"), None, (8, 3)));
        assert!(is_available(Some("8"), None, (8, 0)));
        assert!(is_available(Some("garbage"), None, (8, 3)));
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
