// Shared between `build.rs` (via `include!`) and the library: keep this file a
// plain list with no crate-specific items.

/// The phpstorm-stubs extension directories embedded into the binary, in the
/// order [`stub_files`](crate::stub_files) yields them. Each entry is a
/// directory name under `phpstorm-stubs/`; edit this list to add or drop an
/// extension (a rebuild picks up the change).
pub const EXTENSIONS: &[&str] = &[
    // Always compiled in.
    "Core",
    "standard",
    "date",
    "json",
    "pcre",
    "SPL",
    "Reflection",
    "random",
    "hash",
    // Bundled and enabled by default in most distributions.
    "ctype",
    "filter",
    "tokenizer",
    "session",
    "fileinfo",
    "libxml",
    "dom",
    "SimpleXML",
    "xml",
    "xmlreader",
    "xmlwriter",
    "PDO",
    "sqlite3",
    "Phar",
    "posix",
    "iconv",
    "mbstring",
    "zlib",
    "openssl",
    "curl",
    "sodium",
    "Zend OPcache",
    "readline",
    // Common optional extensions.
    "intl",
    "bcmath",
    "gmp",
    "pcntl",
    "sockets",
    "exif",
    "gd",
    "imagick",
    "calendar",
    "ftp",
    "gettext",
    "mysqli",
    "zip",
    "bz2",
    "xsl",
    "soap",
    "FFI",
];
