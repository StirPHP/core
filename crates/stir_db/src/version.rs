//! PHP versions: what a project targets, and what a declaration is available on.

use std::fmt;

/// A PHP `major.minor` version. Patch levels never change the language or its library's
/// declarations in a way this tool tracks, so they aren't kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PhpVersion {
    pub major: u8,
    pub minor: u8,
}

impl PhpVersion {
    /// The newest version this tool knows the language of: the target when a project doesn't
    /// declare one.
    pub const LATEST: PhpVersion = PhpVersion::new(8, 5);

    pub const fn new(major: u8, minor: u8) -> Self {
        PhpVersion { major, minor }
    }

    /// `8`, `8.1` or `8.1.2`, the last part ignored, as a version must start at digits.
    /// Trailing text in a part (`8.1-dev`, `0.4.1(-1)`) is ignored too.
    pub fn parse(text: &str) -> Option<Self> {
        let mut parts = text.trim().split('.');
        let number = |part: &str| {
            let end = part
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(part.len());
            part[..end].parse::<u8>().ok()
        };
        let major = number(parts.next()?)?;
        let minor = match parts.next() {
            Some(part) => number(part)?,
            None => 0,
        };
        Some(PhpVersion { major, minor })
    }

    /// From a `PHP_VERSION_ID`-style number, such as PHPStan's `phpVersion: 80103`.
    pub const fn from_id(id: u32) -> Self {
        PhpVersion::new((id / 10_000) as u8, ((id / 100) % 100) as u8)
    }

    /// The first release after this one: PHP 5.6 became 7.0, and 7.4 became 8.0.
    pub const fn next(self) -> Self {
        match (self.major, self.minor) {
            (5, 6) => PhpVersion::new(7, 0),
            (7, 4) => PhpVersion::new(8, 0),
            (major, minor) => PhpVersion::new(major, minor + 1),
        }
    }
}

impl fmt::Display for PhpVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// The PHP versions a project has to run on, inclusive. Composer's constraints give a single
/// version, the lowest allowed; PHPStan's `phpVersion` can give a real range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PhpVersions {
    pub min: PhpVersion,
    pub max: PhpVersion,
}

impl PhpVersions {
    pub const fn exactly(version: PhpVersion) -> Self {
        PhpVersions {
            min: version,
            max: version,
        }
    }

    /// The version the language and the stubs are read for: code has to work on the lowest.
    pub const fn target(self) -> PhpVersion {
        self.min
    }

    pub fn is_single(self) -> bool {
        self.min == self.max
    }
}

impl Default for PhpVersions {
    fn default() -> Self {
        PhpVersions::exactly(PhpVersion::LATEST)
    }
}

impl fmt::Display for PhpVersions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_single() {
            write!(f, "{}", self.min)
        } else {
            write!(f, "{} to {}", self.min, self.max)
        }
    }
}

/// Which versions of PHP have a declaration of the library.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Availability {
    /// The first version with it. `None`: every version.
    pub since: Option<PhpVersion>,
    /// The first version without it again. `None`: it's still there.
    pub removed: Option<PhpVersion>,
}

impl Availability {
    pub fn includes(self, version: PhpVersion) -> bool {
        self.since.is_none_or(|v| version >= v) && self.removed.is_none_or(|v| version < v)
    }

    /// Can code rely on it on every version of `versions`?
    pub fn covers(self, versions: PhpVersions) -> bool {
        self.includes(versions.min) && self.includes(versions.max)
    }

    /// The versions having either of two declarations of one name.
    pub fn either(self, other: Availability) -> Availability {
        Availability {
            since: self.since.zip(other.since).map(|(a, b)| a.min(b)),
            removed: self.removed.zip(other.removed).map(|(a, b)| a.max(b)),
        }
    }

    pub fn is_always(self) -> bool {
        self.since.is_none() && self.removed.is_none()
    }
}

/// The newer-than-target wording shared by the version diagnostics.
impl PhpVersions {
    /// `targets PHP 8.1` or `supports PHP 8.1 to 8.3`, for messages.
    pub fn described(self) -> String {
        if self.is_single() {
            format!("targets PHP {}", self.min)
        } else {
            format!("supports PHP {}", self)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const V74: PhpVersion = PhpVersion::new(7, 4);
    const V80: PhpVersion = PhpVersion::new(8, 0);
    const V81: PhpVersion = PhpVersion::new(8, 1);

    #[test]
    fn versions_order_and_print() {
        assert!(V74 < V80 && V80 < V81);
        assert_eq!(V81.to_string(), "8.1");
        assert_eq!(PhpVersion::from_id(80103), V81);
        assert_eq!(PhpVersion::from_id(70400), V74);
    }

    #[test]
    fn versions_parse() {
        assert_eq!(PhpVersion::parse("8.1"), Some(V81));
        assert_eq!(PhpVersion::parse("8.1.27"), Some(V81));
        assert_eq!(PhpVersion::parse("8"), Some(V80));
        assert_eq!(PhpVersion::parse(" 7.4 "), Some(V74));
        assert_eq!(PhpVersion::parse("8.1-dev"), Some(V81));
        assert_eq!(PhpVersion::parse("garbage"), None);
        assert_eq!(PhpVersion::parse(""), None);
    }

    #[test]
    fn the_release_after() {
        assert_eq!(V74.next(), V80);
        assert_eq!(V80.next(), V81);
        assert_eq!(PhpVersion::new(5, 6).next(), PhpVersion::new(7, 0));
    }

    #[test]
    fn availability() {
        let added = Availability {
            since: Some(V80),
            removed: None,
        };
        let removed = Availability {
            since: None,
            removed: Some(V80),
        };
        assert!(!added.includes(V74) && added.includes(V80));
        assert!(removed.includes(V74) && !removed.includes(V80));
        let range = PhpVersions { min: V74, max: V81 };
        assert!(!added.covers(range));
        assert!(!removed.covers(range));
        assert!(added.either(removed).is_always());
    }
}
