//! PHP string literal escaping.

use std::fmt::Write as _;

/// Characters that cannot appear readably in a single-quoted literal.
fn needs_escape(c: char) -> bool {
    c.is_control() || c == '\u{2028}' || c == '\u{2029}' || c == '\u{feff}'
}

/// Prints `s` as a PHP string literal: single-quoted unless it contains control characters,
/// in which case a double-quoted literal with escapes is produced.
pub fn string_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    if s.chars().any(needs_escape) {
        out.push('"');
        double_quoted_body(s, &mut out);
        out.push('"');
    } else {
        out.push('\'');
        for c in s.chars() {
            if c == '\'' || c == '\\' {
                out.push('\\');
            }
            out.push(c);
        }
        out.push('\'');
    }
    out
}

/// Escapes `s` for the inside of a double-quoted PHP string (also used for the literal parts of
/// interpolated strings). `$` is always escaped, so no accidental interpolation can occur
/// (`{$` becomes `{\$`, which PHP prints as `{$`).
pub fn double_quoted_body(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '$' => out.push_str("\\$"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{0b}' => out.push_str("\\v"),
            '\u{0c}' => out.push_str("\\f"),
            '\u{1b}' => out.push_str("\\e"),
            c if (c as u32) < 0x80 && needs_escape(c) => {
                let _ = write!(out, "\\x{:02X}", c as u32);
            }
            c if needs_escape(c) => {
                let _ = write!(out, "\\u{{{:X}}}", c as u32);
            }
            c => out.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_quoted() {
        assert_eq!(string_literal(""), "''");
        assert_eq!(string_literal("abc"), "'abc'");
        assert_eq!(string_literal("it's"), r"'it\'s'");
        assert_eq!(string_literal(r"a\b"), r"'a\\b'");
        assert_eq!(string_literal(r"a\"), r"'a\\'");
        assert_eq!(string_literal("$x {$y}"), "'$x {$y}'");
        assert_eq!(string_literal("\"q\""), "'\"q\"'");
        assert_eq!(string_literal("héllo ✓ 日本"), "'héllo ✓ 日本'");
    }

    #[test]
    fn double_quoted() {
        assert_eq!(string_literal("a\nb"), r#""a\nb""#);
        assert_eq!(string_literal("\t$x\"\\"), r#""\t\$x\"\\""#);
        assert_eq!(string_literal("\0"), r#""\x00""#);
        assert_eq!(string_literal("\x01\x7f\x1b"), r#""\x01\x7F\e""#);
        assert_eq!(string_literal("{$a}\r"), r#""{\$a}\r""#);
        assert_eq!(
            string_literal("\u{85}\u{2028}é\n"),
            r#""\u{85}\u{2028}é\n""#
        );
        assert_eq!(string_literal("\x0b\x0c"), r#""\v\f""#);
    }
}
