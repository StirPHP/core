//! Hand-written lexer mirroring `phpstan/phpdoc-parser`'s token set.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TokenKind {
    OpenPhpdoc,
    ClosePhpdoc,
    /// Newline plus the following `*` line prefix.
    Eol,
    /// Horizontal whitespace.
    Ws,
    Tag,
    This,
    Variable,
    Identifier,
    Int,
    Float,
    SingleQuoted,
    DoubleQuoted,
    OpenParen,
    CloseParen,
    OpenAngle,
    CloseAngle,
    OpenSquare,
    CloseSquare,
    OpenCurly,
    CloseCurly,
    Comma,
    Variadic,
    DoubleColon,
    Colon,
    Equal,
    Nullable,
    Negated,
    Union,
    Intersection,
    Wildcard,
    Arrow,
    Other,
}

impl TokenKind {
    pub(crate) fn is_trivia(self) -> bool {
        matches!(self, TokenKind::Ws | TokenKind::Eol)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Token {
    pub kind: TokenKind,
    pub start: u32,
    pub end: u32,
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b >= 0x80
}

fn is_var_cont(b: u8) -> bool {
    is_ident_start(b) || b.is_ascii_digit()
}

fn is_ident_cont(b: u8) -> bool {
    is_var_cont(b) || b == b'-'
}

/// `d+(_d+)*`
fn digits(s: &[u8], mut p: usize, f: fn(u8) -> bool) -> Option<usize> {
    if p >= s.len() || !f(s[p]) {
        return None;
    }
    while p < s.len() && f(s[p]) {
        p += 1;
    }
    while p + 1 < s.len() && s[p] == b'_' && f(s[p + 1]) {
        p += 1;
        while p < s.len() && f(s[p]) {
            p += 1;
        }
    }
    Some(p)
}

fn dec(b: u8) -> bool {
    b.is_ascii_digit()
}
fn hex(b: u8) -> bool {
    b.is_ascii_hexdigit()
}
fn oct(b: u8) -> bool {
    (b'0'..=b'7').contains(&b)
}
fn bin(b: u8) -> bool {
    b == b'0' || b == b'1'
}

fn exponent(s: &[u8], p: usize) -> Option<usize> {
    if p < s.len() && (s[p] | 0x20) == b'e' {
        let mut q = p + 1;
        if q < s.len() && (s[q] == b'+' || s[q] == b'-') {
            q += 1;
        }
        return digits(s, q, dec);
    }
    None
}

fn number(s: &[u8], start: usize) -> Option<(TokenKind, usize)> {
    let mut p = start;
    if s[p] == b'+' || s[p] == b'-' {
        p += 1;
    }
    let int_part = digits(s, p, dec);
    let q = int_part.unwrap_or(p);
    if q < s.len() && s[q] == b'.' {
        let frac = digits(s, q + 1, dec);
        if int_part.is_some() || frac.is_some() {
            let e = frac.unwrap_or(q + 1);
            return Some((TokenKind::Float, exponent(s, e).unwrap_or(e)));
        }
    }
    if let Some(ip) = int_part
        && let Some(e) = exponent(s, ip)
    {
        return Some((TokenKind::Float, e));
    }
    if p + 1 < s.len() && s[p] == b'0' {
        let r = match s[p + 1] | 0x20 {
            b'x' => digits(s, p + 2, hex),
            b'o' => digits(s, p + 2, oct),
            b'b' => digits(s, p + 2, bin),
            _ => None,
        };
        if let Some(e) = r {
            return Some((TokenKind::Int, e));
        }
    }
    int_part.map(|e| (TokenKind::Int, e))
}

fn quoted(s: &[u8], start: usize, q: u8) -> Option<usize> {
    let mut p = start + 1;
    while p < s.len() {
        match s[p] {
            b'\\' => {
                if p + 1 < s.len() && s[p + 1] != b'\n' && s[p + 1] != b'\r' {
                    p += 2;
                } else {
                    return None;
                }
            }
            b'\n' | b'\r' => return None,
            c if c == q => return Some(p + 1),
            _ => p += 1,
        }
    }
    None
}

fn identifier(s: &[u8], start: usize) -> Option<usize> {
    let mut p = start;
    let mut any = false;
    loop {
        let save = p;
        if p < s.len() && s[p] == b'\\' {
            p += 1;
        }
        if p >= s.len() || !is_ident_start(s[p]) {
            p = save;
            break;
        }
        p += 1;
        while p < s.len() && is_ident_cont(s[p]) {
            p += 1;
        }
        any = true;
    }
    any.then_some(p)
}

fn tag(s: &[u8], start: usize) -> Option<usize> {
    // @(?:[a-z][a-z0-9-\\]+:)?[a-z][a-z0-9-\\]*
    let word = |mut p: usize| -> Option<usize> {
        if p >= s.len() || !s[p].is_ascii_alphabetic() {
            return None;
        }
        p += 1;
        while p < s.len() && (s[p].is_ascii_alphanumeric() || s[p] == b'-' || s[p] == b'\\') {
            p += 1;
        }
        Some(p)
    };
    let p = word(start + 1)?;
    if p < s.len()
        && s[p] == b':'
        && let Some(q) = word(p + 1)
    {
        return Some(q);
    }
    Some(p)
}

pub(crate) fn lex(src: &str) -> Vec<Token> {
    use TokenKind as K;
    let s = src.as_bytes();
    let mut out = Vec::new();
    let mut p = 0usize;
    while p < s.len() {
        let start = p;
        let c = s[p];
        let next = s.get(p + 1).copied();
        let (kind, end) = match c {
            b'\r' if next == Some(b'\n') => (K::Eol, eol(s, p + 1)),
            b'\n' => (K::Eol, eol(s, p)),
            b' ' | b'\t' | b'\r' | 0x0b | 0x0c => {
                let mut e = p + 1;
                while e < s.len() && matches!(s[e], b' ' | b'\t' | 0x0b | 0x0c) {
                    e += 1;
                }
                (K::Ws, e)
            }
            b'/' if s[p..].starts_with(b"/**")
                && s.get(p + 3).is_some_and(|b| b.is_ascii_whitespace()) =>
            {
                let e = if s[p + 3] == b' ' { p + 4 } else { p + 3 };
                (K::OpenPhpdoc, e)
            }
            b'*' if next == Some(b'/') => (K::ClosePhpdoc, p + 2),
            b'*' => (K::Wildcard, p + 1),
            b'@' if next.is_some_and(|b| b.is_ascii_alphabetic()) => {
                (K::Tag, tag(s, p).unwrap_or(p + 1))
            }
            b'$' if s[p..].starts_with(b"$this")
                && !s.get(p + 5).is_some_and(|&b| is_var_cont(b)) =>
            {
                (K::This, p + 5)
            }
            b'$' if next.is_some_and(is_ident_start) => {
                let mut e = p + 2;
                while e < s.len() && is_var_cont(s[e]) {
                    e += 1;
                }
                (K::Variable, e)
            }
            b'.' if s[p..].starts_with(b"...") => (K::Variadic, p + 3),
            b':' if next == Some(b':') => (K::DoubleColon, p + 2),
            b'-' if next == Some(b'>') => (K::Arrow, p + 2),
            b'(' => (K::OpenParen, p + 1),
            b')' => (K::CloseParen, p + 1),
            b'<' => (K::OpenAngle, p + 1),
            b'>' => (K::CloseAngle, p + 1),
            b'[' => (K::OpenSquare, p + 1),
            b']' => (K::CloseSquare, p + 1),
            b'{' => (K::OpenCurly, p + 1),
            b'}' => (K::CloseCurly, p + 1),
            b',' => (K::Comma, p + 1),
            b':' => (K::Colon, p + 1),
            b'=' => (K::Equal, p + 1),
            b'?' => (K::Nullable, p + 1),
            b'!' => (K::Negated, p + 1),
            b'|' => (K::Union, p + 1),
            b'&' => (K::Intersection, p + 1),
            b'\'' | b'"' => match quoted(s, p, c) {
                Some(e) => (
                    if c == b'\'' {
                        K::SingleQuoted
                    } else {
                        K::DoubleQuoted
                    },
                    e,
                ),
                None => (K::Other, other(s, p)),
            },
            _ => {
                let numeric_start = c.is_ascii_digit()
                    || ((c == b'+' || c == b'-' || c == b'.')
                        && next.is_some_and(|b| b.is_ascii_digit() || b == b'.'));
                if let Some(r) = numeric_start.then(|| number(s, p)).flatten() {
                    r
                } else if let Some(e) = identifier(s, p) {
                    (K::Identifier, e)
                } else {
                    (K::Other, other(s, p))
                }
            }
        };
        debug_assert!(end > start);
        out.push(Token {
            kind,
            start: start as u32,
            end: end as u32,
        });
        p = end;
    }
    out
}

/// `\n[\t ]*(?:\*(?!/) ?)?` starting at the `\n`.
fn eol(s: &[u8], nl: usize) -> usize {
    let mut e = nl + 1;
    while e < s.len() && (s[e] == b' ' || s[e] == b'\t') {
        e += 1;
    }
    if e < s.len() && s[e] == b'*' && s.get(e + 1) != Some(&b'/') {
        e += 1;
        if e < s.len() && s[e] == b' ' {
            e += 1;
        }
    }
    e
}

/// A run of non-whitespace not containing `*/`.
fn other(s: &[u8], start: usize) -> usize {
    let mut e = start + 1;
    while e < s.len()
        && !s[e].is_ascii_whitespace()
        && !(s[e] == b'*' && s.get(e + 1) == Some(&b'/'))
    {
        e += 1;
    }
    e
}
