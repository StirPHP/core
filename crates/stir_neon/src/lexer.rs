//! NEON lexer (patterns follow nette/neon v3).

use crate::Error;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum TokenKind {
    /// Decoded quoted string.
    String(String),
    /// Raw literal text (bare word, number, `yes`, ...).
    Literal(String),
    /// One of `, : = [ ] { } ( ) -`
    Char(u8),
    /// Newline followed by the indentation of the next line.
    Newline(String),
}

#[derive(Debug, Clone)]
pub(crate) struct Token {
    pub kind: TokenKind,
    pub offset: usize,
}

const SPECIAL: &[u8] = b",:=[]{}()";

fn is_ws(b: u8) -> bool {
    b == b' ' || b == b'\t'
}

pub(crate) fn lex(src: &str) -> Result<Vec<Token>, Error> {
    let s = src.as_bytes();
    let mut out = Vec::new();
    // Virtual newline so the first line's indentation is known.
    let mut p = 0;
    while p < s.len() && is_ws(s[p]) {
        p += 1;
    }
    out.push(Token {
        kind: TokenKind::Newline(src[..p].to_string()),
        offset: 0,
    });

    while p < s.len() {
        let c = s[p];
        let next = s.get(p + 1).copied();
        let start = p;
        match c {
            b'\n' => {
                p += 1;
                let is = p;
                while p < s.len() && is_ws(s[p]) {
                    p += 1;
                }
                out.push(Token {
                    kind: TokenKind::Newline(src[is..p].to_string()),
                    offset: start,
                });
            }
            b' ' | b'\t' => p += 1,
            b'#' => {
                while p < s.len() && s[p] != b'\n' {
                    p += 1;
                }
            }
            b'\'' | b'"' => {
                let (value, end) = string(src, p)?;
                out.push(Token {
                    kind: TokenKind::String(value),
                    offset: start,
                });
                p = end;
            }
            b':' | b'-'
                if next
                    .is_some_and(|n| !n.is_ascii_whitespace() && !b"\"',=[]{}()".contains(&n)) =>
            {
                p = literal(s, p + 2);
                out.push(Token {
                    kind: TokenKind::Literal(src[start..p].to_string()),
                    offset: start,
                });
            }
            b'-' => {
                p += 1;
                out.push(Token {
                    kind: TokenKind::Char(c),
                    offset: start,
                });
            }
            c if SPECIAL.contains(&c) => {
                p += 1;
                out.push(Token {
                    kind: TokenKind::Char(c),
                    offset: start,
                });
            }
            b'`' | b'\r' | 0x0b | 0x0c => {
                return Err(Error::at(src, p, format!("Unexpected '{}'", c as char)));
            }
            _ => {
                p = literal(s, p + 1);
                out.push(Token {
                    kind: TokenKind::Literal(src[start..p].to_string()),
                    offset: start,
                });
            }
        }
    }
    Ok(out)
}

/// Continues a literal starting at `p` (the first char(s) already consumed).
fn literal(s: &[u8], mut p: usize) -> usize {
    const STOP: &[u8] = b",:=]})(\n\t ";
    loop {
        if p >= s.len() {
            return p;
        }
        let c = s[p];
        if !STOP.contains(&c) {
            p += 1;
        } else if c == b':' {
            match s.get(p + 1) {
                Some(&n) if !n.is_ascii_whitespace() && !b",]})".contains(&n) => p += 1,
                _ => return p,
            }
        } else if is_ws(c) {
            let mut q = p;
            while q < s.len() && is_ws(s[q]) {
                q += 1;
            }
            match s.get(q) {
                Some(&n) if !b"#,:=]})(\n\t ".contains(&n) => p = q + 1,
                _ => return p,
            }
        } else {
            return p;
        }
    }
}

/// Lexes a quoted string at `p`, returning the decoded value and end offset.
fn string(src: &str, p: usize) -> Result<(String, usize), Error> {
    let s = src.as_bytes();
    let q = s[p];
    let triple = s[p..].starts_with(&[q, q, q, b'\n']);
    if triple {
        let content_start = p + 4;
        let mut line = content_start;
        loop {
            let mut i = line;
            while i < s.len() && is_ws(s[i]) {
                i += 1;
            }
            if s[i..].starts_with(&[q, q, q]) {
                let raw = &src[p + 3..i]; // "\n...\n   "
                let end = i + 3;
                let value = decode_multiline(raw);
                let value = if q == b'"' {
                    unescape(src, p, &value)?
                } else {
                    value
                };
                return Ok((value, end));
            }
            match src[line..].find('\n') {
                Some(nl) => line += nl + 1,
                None => return Err(Error::at(src, p, "Unterminated multi-line string".into())),
            }
        }
    }

    let mut i = p + 1;
    while i < s.len() {
        match s[i] {
            b'\n' => break,
            b'\\' if q == b'"' => {
                i += 2;
                continue;
            }
            c if c == q => {
                if q == b'\'' && s.get(i + 1) == Some(&b'\'') {
                    i += 2;
                    continue;
                }
                let inner = &src[p + 1..i];
                let value = if q == b'\'' {
                    inner.replace("''", "'")
                } else {
                    unescape(src, p, inner)?
                };
                return Ok((value, i + 1));
            }
            _ => {}
        }
        i += 1;
    }
    Err(Error::at(src, p, "Unterminated string".into()))
}

/// `raw` is everything between the opening delimiter and the closing one, starting with `\n`.
fn decode_multiline(raw: &str) -> String {
    // Indentation of the first non-empty line is removed from every line.
    let after = raw.trim_start_matches('\n');
    let indent_len = after.bytes().take_while(|&b| is_ws(b)).count();
    let indent = &after[..indent_len];
    let mut res = if indent.is_empty() {
        raw.to_string()
    } else {
        raw.replace(&format!("\n{indent}"), "\n")
    };
    if res.starts_with('\n') {
        res.remove(0);
    }
    // strip trailing "\n[\t ]*"
    let trimmed = res.trim_end_matches([' ', '\t']);
    if let Some(t) = trimmed.strip_suffix('\n') {
        t.to_string()
    } else if trimmed.is_empty() {
        String::new()
    } else {
        res
    }
}

fn unescape(src: &str, at: usize, s: &str) -> Result<String, Error> {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    let hex = |chars: &mut std::iter::Peekable<std::str::Chars<'_>>, n: usize| -> Option<u32> {
        let mut h = String::new();
        for _ in 0..n {
            let c = chars.peek().copied().filter(|c| c.is_ascii_hexdigit())?;
            chars.next();
            h.push(c);
        }
        u32::from_str_radix(&h, 16).ok()
    };
    let bad = || Error::at(src, at, "Invalid escape sequence".into());
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let e = chars.next().ok_or_else(bad)?;
        match e {
            't' => out.push('\t'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            'f' => out.push('\x0c'),
            'b' => out.push('\x08'),
            '"' => out.push('"'),
            '\\' => out.push('\\'),
            '/' => out.push('/'),
            '_' => out.push('\u{a0}'),
            'x' => {
                let v = hex(&mut chars, 2).ok_or_else(bad)?;
                out.push(char::from_u32(v).ok_or_else(bad)?);
            }
            'u' => {
                let hi = hex(&mut chars, 4).ok_or_else(bad)?;
                let cp = if (0xD800..0xDC00).contains(&hi) {
                    // surrogate pair: 😀
                    if chars.next() != Some('\\') || chars.next() != Some('u') {
                        return Err(bad());
                    }
                    let lo = hex(&mut chars, 4).ok_or_else(bad)?;
                    if !(0xDC00..0xE000).contains(&lo) {
                        return Err(bad());
                    }
                    0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00)
                } else {
                    hi
                };
                out.push(char::from_u32(cp).ok_or_else(bad)?);
            }
            _ => return Err(bad()),
        }
    }
    Ok(out)
}
