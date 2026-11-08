//! Which URI a workspace file has for the client, and which file a URI is. On a disk that's its
//! path; in the browser, where there's no disk, files are wherever the page says and the builtin
//! stubs have URIs of their own.

use std::path::{Path, PathBuf};

use ls_types::Uri;
use stir_db::{Db, SourceFile};

pub trait Links: Send + Sync {
    /// The URI of `file`, for the client to open it.
    fn uri(&self, db: &dyn Db, file: SourceFile) -> Option<Uri>;

    /// The workspace path of what the client calls `uri`.
    fn path(&self, uri: &Uri) -> Option<PathBuf>;

    /// The URI of a path that isn't necessarily a workspace file (a script, what PHPStan
    /// reported).
    fn path_uri(&self, path: &Path) -> Option<Uri> {
        Uri::from_file_path(path)
    }
}

/// The scheme of the builtin stubs' URIs in [`Virtual`]: `stirphp:///stubs/Core/Core.php`.
pub const STUB_SCHEME: &str = "stirphp";

/// Files with no disk under them: a URI is its path, and a builtin stub is `stirphp:///stubs/...`,
/// whose text the client can ask for (`stirphp/fileText`).
#[derive(Debug, Clone, Copy, Default)]
pub struct Virtual;

impl Links for Virtual {
    fn uri(&self, db: &dyn Db, file: SourceFile) -> Option<Uri> {
        let path = file.path(db);
        let s = path.to_string_lossy();
        if let Some(real) = s.strip_prefix(stir_db::OVERLAY_PREFIX) {
            return file_uri(Path::new(real));
        }
        if let Some(rest) = s.strip_prefix("stubs://") {
            return format!("{STUB_SCHEME}:///stubs/{}", encode(rest))
                .parse()
                .ok();
        }
        file_uri(path)
    }

    fn path_uri(&self, path: &Path) -> Option<Uri> {
        file_uri(path)
    }

    fn path(&self, uri: &Uri) -> Option<PathBuf> {
        let text = uri.as_str();
        if let Some(rest) = text
            .strip_prefix(STUB_SCHEME)
            .and_then(|r| r.strip_prefix(":///stubs/"))
        {
            let rest = percent_decode(rest);
            return Some(PathBuf::from(format!("stubs://{rest}")));
        }
        Some(stir_vfs::normalize(&uri.to_file_path()?))
    }
}

/// The `file://` URI of `path`, which must start at the root. Not `Uri::from_file_path`: with no
/// disk (WebAssembly), no path counts as absolute to the standard library, and it would look for
/// the file.
fn file_uri(path: &Path) -> Option<Uri> {
    let text = path.to_str()?;
    if !text.starts_with('/') {
        return None;
    }
    format!("file://{}", encode(text)).parse().ok()
}

/// `text` percent-encoded as `Uri::from_file_path` does it, for the same URIs as on a disk.
fn encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = text.get(i + 1..i + 3)
            && let Ok(b) = u8::from_str_radix(hex, 16)
        {
            out.push(b);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
