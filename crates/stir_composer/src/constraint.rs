//! Composer's version constraints, as far as a project's PHP version needs them.

use stir_db::PhpVersion;

/// The lowest PHP version `constraint` allows, such as `8.1` for `>=8.1 <8.5`, `^8.1`, `8.1.*`,
/// `~8.1.0` or `8.1 || 8.3`. `None` if it sets no lower bound, like `*` or `<8.0`.
pub fn lowest_php_version(constraint: &str) -> Option<PhpVersion> {
    constraint
        .split('|')
        .filter(|alternative| !alternative.trim().is_empty())
        .map(lowest_of_alternative)
        .collect::<Option<Vec<_>>>()?
        .into_iter()
        .min()
}

/// One side of `||`: the highest lower bound of what must all hold.
fn lowest_of_alternative(alternative: &str) -> Option<PhpVersion> {
    // a hyphen range: `8.1 - 8.3`
    if let Some((low, _)) = alternative.split_once(" - ") {
        return plain_version(low.trim());
    }
    let alternative = spread_operators(alternative);
    alternative
        .split([',', ' '])
        .filter(|atom| !atom.is_empty())
        .filter_map(lower_bound)
        .max()
}

/// `>= 8.1` has the same meaning as `>=8.1`: joins an operator to what follows.
fn spread_operators(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut after_operator = false;
    for c in text.chars() {
        if c == ' ' && after_operator {
            continue;
        }
        after_operator = matches!(c, '>' | '<' | '=' | '!' | '^' | '~');
        out.push(c);
    }
    out
}

fn lower_bound(atom: &str) -> Option<PhpVersion> {
    // `<8.0` and `!=8.1` set no lower bound; `>8.1` means a later patch of it, unless that's
    // the last one
    if atom.starts_with('<') || atom.starts_with('!') {
        return None;
    }
    if let Some(rest) = atom.strip_prefix('>').filter(|r| !r.starts_with('=')) {
        let version = plain_version(rest)?;
        let last_patch = rest.trim_start_matches('v').split('.').nth(2) == Some("99");
        return Some(if last_patch { version.next() } else { version });
    }
    plain_version(atom.trim_start_matches(['>', '=', '^', '~']))
}

/// `8.1`, `8.1.2`, `v8.1`, `8.1.*` or `8.1.0-dev@dev`.
fn plain_version(text: &str) -> Option<PhpVersion> {
    let text = text.trim_start_matches(['v', 'V']);
    if !text.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    PhpVersion::parse(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lowest(c: &str) -> Option<String> {
        lowest_php_version(c).map(|v| v.to_string())
    }

    #[test]
    fn composer_constraints() {
        for (constraint, want) in [
            ("^8.2", "8.2"),
            ("^7.4 || ^8.0", "7.4"),
            ("^7.4|^8.0", "7.4"),
            ("8.2 || 8.3", "8.2"),
            ("8.3 || 8.2", "8.2"),
            (">=8.1 <8.5", "8.1"),
            (">=8.1,<8.5", "8.1"),
            (">= 8.1, < 8.5", "8.1"),
            (">=8.4.1", "8.4"),
            ("8.1.*", "8.1"),
            ("8.*", "8.0"),
            ("~8.2.0", "8.2"),
            ("~8.2", "8.2"),
            ("8.2.99", "8.2"),
            ("v8.1", "8.1"),
            ("=8.1", "8.1"),
            ("==8.1.2", "8.1"),
            (">8.1", "8.1"),
            (">8.1.99", "8.2"),
            ("8.1 - 8.3", "8.1"),
            (">=8.0 !=8.1.0", "8.0"),
            ("^8.2@dev", "8.2"),
            ("8.3.0-dev", "8.3"),
            (">=5.3.3", "5.3"),
        ] {
            assert_eq!(lowest(constraint).as_deref(), Some(want), "{constraint}");
        }
    }

    #[test]
    fn constraints_without_a_lower_bound() {
        for constraint in ["*", "<8.0", "!=8.1", "", "latest", "<8.0 || ^8.1"] {
            assert_eq!(lowest(constraint), None, "{constraint}");
        }
    }
}
