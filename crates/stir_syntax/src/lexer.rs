//! Tokenizer for PHP and StirPHP source.
//!
//! The lexer is a small mode-stack state machine. Fixed-shape tokens in code
//! mode (operators, names, numbers, casts) are recognised by a [`logos`]
//! automaton; everything context-sensitive (inline HTML, comments ending at
//! `?>`, strings with interpolation, heredocs) is scanned by hand.

use logos::Logos;

use crate::{Dialect, SyntaxKind, SyntaxKind::*};

/// A lexed token: its kind and its length in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub kind: SyntaxKind,
    pub len: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexError {
    pub offset: u32,
    pub message: String,
}

#[derive(Debug, Default)]
pub struct Lexed {
    pub tokens: Vec<Token>,
    pub errors: Vec<LexError>,
}

/// Fixed-shape tokens of code mode.
#[derive(Logos, Debug, Clone, Copy, PartialEq, Eq)]
enum Raw {
    #[regex(r"[a-zA-Z_\u{80}-\u{10FFFF}][a-zA-Z0-9_\u{80}-\u{10FFFF}]*")]
    Ident,
    #[regex(r"[a-zA-Z_\u{80}-\u{10FFFF}][a-zA-Z0-9_\u{80}-\u{10FFFF}]*(\\[a-zA-Z_\u{80}-\u{10FFFF}][a-zA-Z0-9_\u{80}-\u{10FFFF}]*)+")]
    NameQualified,
    #[regex(r"(\\[a-zA-Z_\u{80}-\u{10FFFF}][a-zA-Z0-9_\u{80}-\u{10FFFF}]*)+")]
    NameFullyQualified,
    #[regex(r"[0-9]+(_[0-9]+)*")]
    #[regex(r"0[xX][0-9a-fA-F]+(_[0-9a-fA-F]+)*")]
    #[regex(r"0[bB][01]+(_[01]+)*")]
    #[regex(r"0[oO][0-7]+(_[0-7]+)*")]
    Int,
    #[regex(r"(\.[0-9]+(_[0-9]+)*|[0-9]+(_[0-9]+)*\.([0-9]+(_[0-9]+)*)?)([eE][+-]?[0-9]+(_[0-9]+)*)?|[0-9]+(_[0-9]+)*[eE][+-]?[0-9]+(_[0-9]+)*")]
    Float,

    #[regex(r"\([ \t]*(int|integer)[ \t]*\)", ignore(case))]
    IntCast,
    #[regex(r"\([ \t]*(float|double|real)[ \t]*\)", ignore(case))]
    FloatCast,
    #[regex(r"\([ \t]*(string|binary)[ \t]*\)", ignore(case))]
    StringCast,
    #[regex(r"\([ \t]*(bool|boolean)[ \t]*\)", ignore(case))]
    BoolCast,
    #[regex(r"\([ \t]*array[ \t]*\)", ignore(case))]
    ArrayCast,
    #[regex(r"\([ \t]*object[ \t]*\)", ignore(case))]
    ObjectCast,
    #[regex(r"\([ \t]*unset[ \t]*\)", ignore(case))]
    UnsetCast,
    #[regex(r"\([ \t]*void[ \t]*\)", ignore(case))]
    VoidCast,

    #[token(";")]
    Semi,
    #[token(",")]
    Comma,
    #[token("(")]
    LParen,
    #[token(")")]
    RParen,
    #[token("[")]
    LBrack,
    #[token("]")]
    RBrack,
    #[token("{")]
    LCurly,
    #[token("}")]
    RCurly,
    #[token(":")]
    Colon,
    #[token("::")]
    Colon2,
    #[token("?")]
    Question,
    #[token("??")]
    Question2,
    #[token("??=")]
    Question2Eq,
    #[token("?->")]
    NullsafeArrow,
    #[token("->")]
    Arrow,
    #[token("=>")]
    FatArrow,
    #[token("...")]
    Ellipsis,
    #[token(".")]
    Dot,
    #[token(".=")]
    DotEq,
    #[token("=")]
    Eq,
    #[token("==")]
    Eq2,
    #[token("===")]
    Eq3,
    #[token("!=")]
    #[token("<>")]
    Neq,
    #[token("!==")]
    Neq2,
    #[token("<")]
    LAngle,
    #[token(">")]
    RAngle,
    #[token("<=")]
    LtEq,
    #[token("<=>")]
    Spaceship,
    #[token("+")]
    Plus,
    #[token("-")]
    Minus,
    #[token("*")]
    Star,
    #[token("/")]
    Slash,
    #[token("%")]
    Percent,
    #[token("**")]
    Pow,
    #[token("+=")]
    PlusEq,
    #[token("-=")]
    MinusEq,
    #[token("*=")]
    StarEq,
    #[token("/=")]
    SlashEq,
    #[token("%=")]
    PercentEq,
    #[token("**=")]
    PowEq,
    #[token("++")]
    Inc,
    #[token("--")]
    Dec,
    #[token("&")]
    Amp,
    #[token("|")]
    Pipe,
    #[token("^")]
    Caret,
    #[token("~")]
    Tilde,
    #[token("<<")]
    Shl,
    #[token("&=")]
    AmpEq,
    #[token("|=")]
    PipeEq,
    #[token("^=")]
    CaretEq,
    #[token("<<=")]
    ShlEq,
    #[token("&&")]
    Amp2,
    #[token("||")]
    Pipe2,
    #[token("|>")]
    PipeGt,
    #[token("!")]
    Bang,
    #[token("@")]
    At,
    #[token("#[")]
    AttrOpen,
    #[token("\\")]
    Backslash,
}

impl Raw {
    fn kind(self) -> SyntaxKind {
        match self {
            Raw::Ident => IDENT,
            Raw::NameQualified => NAME_QUALIFIED,
            Raw::NameFullyQualified => NAME_FULLY_QUALIFIED,
            Raw::Int => INT_NUMBER,
            Raw::Float => FLOAT_NUMBER,
            Raw::IntCast => INT_CAST,
            Raw::FloatCast => FLOAT_CAST,
            Raw::StringCast => STRING_CAST,
            Raw::BoolCast => BOOL_CAST,
            Raw::ArrayCast => ARRAY_CAST,
            Raw::ObjectCast => OBJECT_CAST,
            Raw::UnsetCast => UNSET_CAST,
            Raw::VoidCast => VOID_CAST,
            Raw::Semi => SEMI,
            Raw::Comma => COMMA,
            Raw::LParen => L_PAREN,
            Raw::RParen => R_PAREN,
            Raw::LBrack => L_BRACK,
            Raw::RBrack => R_BRACK,
            Raw::LCurly => L_CURLY,
            Raw::RCurly => R_CURLY,
            Raw::Colon => COLON,
            Raw::Colon2 => COLON2,
            Raw::Question => QUESTION,
            Raw::Question2 => QUESTION2,
            Raw::Question2Eq => QUESTION2_EQ,
            Raw::NullsafeArrow => NULLSAFE_ARROW,
            Raw::Arrow => ARROW,
            Raw::FatArrow => FAT_ARROW,
            Raw::Ellipsis => ELLIPSIS,
            Raw::Dot => DOT,
            Raw::DotEq => DOT_EQ,
            Raw::Eq => EQ,
            Raw::Eq2 => EQ2,
            Raw::Eq3 => EQ3,
            Raw::Neq => NEQ,
            Raw::Neq2 => NEQ2,
            Raw::LAngle => L_ANGLE,
            Raw::RAngle => R_ANGLE,
            Raw::LtEq => LTEQ,
            Raw::Spaceship => SPACESHIP,
            Raw::Plus => PLUS,
            Raw::Minus => MINUS,
            Raw::Star => STAR,
            Raw::Slash => SLASH,
            Raw::Percent => PERCENT,
            Raw::Pow => POW,
            Raw::PlusEq => PLUS_EQ,
            Raw::MinusEq => MINUS_EQ,
            Raw::StarEq => STAR_EQ,
            Raw::SlashEq => SLASH_EQ,
            Raw::PercentEq => PERCENT_EQ,
            Raw::PowEq => POW_EQ,
            Raw::Inc => INC,
            Raw::Dec => DEC,
            Raw::Amp => AMP,
            Raw::Pipe => PIPE,
            Raw::Caret => CARET,
            Raw::Tilde => TILDE,
            Raw::Shl => SHL,
            Raw::AmpEq => AMP_EQ,
            Raw::PipeEq => PIPE_EQ,
            Raw::CaretEq => CARET_EQ,
            Raw::ShlEq => SHL_EQ,
            Raw::Amp2 => AMP2,
            Raw::Pipe2 => PIPE2,
            Raw::PipeGt => PIPE_GT,
            Raw::Bang => BANG,
            Raw::At => AT,
            Raw::AttrOpen => ATTR_OPEN,
            Raw::Backslash => BACKSLASH,
        }
    }
}

#[derive(Debug, Clone)]
enum Mode {
    Html,
    Code,
    /// Code inside `{$ ... }` / `${ ... }` of a string; pops at the matching `}`.
    InterpCode {
        depth: u32,
    },
    DoubleQuote,
    Backtick,
    Heredoc {
        label: String,
    },
}

struct Lexer<'a> {
    src: &'a str,
    pos: usize,
    dialect: Dialect,
    modes: Vec<Mode>,
    out: Lexed,
    halted: bool,
}

/// Tokenizes `src`. The token lengths always sum to `src.len()`.
pub fn lex(src: &str, dialect: Dialect) -> Lexed {
    let start = match dialect {
        Dialect::Php => Mode::Html,
        // StirPHP files are code from the first byte; a leading `<?php` is
        // accepted for familiarity.
        Dialect::Stir => Mode::Code,
    };
    let mut lexer = Lexer {
        src,
        pos: 0,
        dialect,
        modes: vec![start],
        out: Lexed::default(),
        halted: false,
    };
    lexer.run();
    debug_assert_eq!(
        lexer
            .out
            .tokens
            .iter()
            .map(|t| t.len as usize)
            .sum::<usize>(),
        src.len()
    );
    lexer.out
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b >= 0x80
}

fn is_ident_continue(b: u8) -> bool {
    is_ident_start(b) || b.is_ascii_digit()
}

impl<'a> Lexer<'a> {
    fn rest(&self) -> &'a str {
        &self.src[self.pos..]
    }

    fn bytes(&self) -> &'a [u8] {
        self.rest().as_bytes()
    }

    fn peek(&self, n: usize) -> Option<u8> {
        self.bytes().get(n).copied()
    }

    fn push(&mut self, kind: SyntaxKind, len: usize) {
        debug_assert!(len > 0);
        self.out.tokens.push(Token {
            kind,
            len: len as u32,
        });
        self.pos += len;
    }

    fn error(&mut self, message: impl Into<String>) {
        self.out.errors.push(LexError {
            offset: self.pos as u32,
            message: message.into(),
        });
    }

    fn run(&mut self) {
        while self.pos < self.src.len() {
            if self.halted {
                let len = self.src.len() - self.pos;
                self.push(HALT_DATA, len);
                break;
            }
            match self
                .modes
                .last()
                .cloned()
                .expect("mode stack is never empty")
            {
                Mode::Html => self.lex_html(),
                Mode::Code | Mode::InterpCode { .. } => self.lex_code(),
                Mode::DoubleQuote => self.lex_string_part(b'"'),
                Mode::Backtick => self.lex_string_part(b'`'),
                Mode::Heredoc { label } => self.lex_heredoc_part(&label),
            }
        }
    }

    fn lex_html(&mut self) {
        let rest = self.rest();
        let mut search = 0;
        let open = loop {
            match rest[search..].find("<?") {
                None => break None,
                Some(i) => {
                    let at = search + i;
                    let after = &rest[at + 2..];
                    if after.starts_with('=') {
                        break Some((at, 3, OPEN_TAG_WITH_ECHO));
                    }
                    if after.len() >= 3 && after[..3].eq_ignore_ascii_case("php") {
                        let tail = after.as_bytes().get(3).copied();
                        match tail {
                            None => break Some((at, 5, OPEN_TAG)),
                            Some(b' ' | b'\t') => break Some((at, 6, OPEN_TAG)),
                            Some(b'\n') => break Some((at, 6, OPEN_TAG)),
                            Some(b'\r') => {
                                let len = if after.as_bytes().get(4) == Some(&b'\n') {
                                    7
                                } else {
                                    6
                                };
                                break Some((at, len, OPEN_TAG));
                            }
                            _ => {}
                        }
                    }
                    search = at + 2;
                }
            }
        };
        match open {
            None => {
                let len = rest.len();
                self.push(INLINE_HTML, len);
            }
            Some((at, len, kind)) => {
                if at > 0 {
                    self.push(INLINE_HTML, at);
                }
                self.push(kind, len);
                *self.modes.last_mut().unwrap() = Mode::Code;
            }
        }
    }

    fn lex_code(&mut self) {
        let b = self.bytes();
        let c = b[0];

        // whitespace
        if c.is_ascii_whitespace() {
            let len = b.iter().take_while(|b| b.is_ascii_whitespace()).count();
            self.push(WHITESPACE, len);
            return;
        }

        // close tag (StirPHP files have no inline HTML: `List<T?>` is a type)
        if c == b'?' && b.get(1) == Some(&b'>') && self.dialect == Dialect::Php {
            let mut len = 2;
            if b.get(2) == Some(&b'\n') {
                len = 3;
            } else if b.get(2) == Some(&b'\r') {
                len = if b.get(3) == Some(&b'\n') { 4 } else { 3 };
            }
            self.push(CLOSE_TAG, len);
            *self.modes.last_mut().unwrap() = Mode::Html;
            return;
        }

        // `<?php` at the very start of a StirPHP file
        if self.dialect == Dialect::Stir
            && self.out.tokens.is_empty()
            && self.rest().starts_with("<?php")
        {
            self.push(OPEN_TAG, 5);
            return;
        }

        // comments
        if c == b'#' && b.get(1) != Some(&b'[') {
            self.lex_line_comment(1);
            return;
        }
        if c == b'/' && b.get(1) == Some(&b'/') {
            self.lex_line_comment(2);
            return;
        }
        if c == b'/' && b.get(1) == Some(&b'*') {
            let is_doc =
                b.get(2) == Some(&b'*') && b.get(3).is_some_and(|b| b.is_ascii_whitespace());
            match self.rest()[2..].find("*/") {
                Some(i) => self.push(if is_doc { DOC_COMMENT } else { COMMENT }, i + 4),
                None => {
                    self.error("unterminated comment");
                    let len = b.len();
                    self.push(if is_doc { DOC_COMMENT } else { COMMENT }, len);
                }
            }
            return;
        }

        // variables
        if c == b'$' {
            if b.get(1).copied().is_some_and(is_ident_start) {
                let len = 1 + b[1..].iter().take_while(|b| is_ident_continue(**b)).count();
                self.push(VARIABLE, len);
            } else {
                self.push(DOLLAR, 1);
            }
            return;
        }

        // strings
        if c == b'\'' {
            self.lex_single_quoted(1);
            return;
        }
        if (c == b'b' || c == b'B') && b.get(1) == Some(&b'\'') {
            self.lex_single_quoted(2);
            return;
        }
        if c == b'"' {
            self.lex_double_quoted(1);
            return;
        }
        if (c == b'b' || c == b'B') && b.get(1) == Some(&b'"') {
            self.lex_double_quoted(2);
            return;
        }
        if c == b'`' {
            self.push(BACKTICK, 1);
            self.modes.push(Mode::Backtick);
            return;
        }
        if self.rest().starts_with("<<<") && self.try_heredoc_start() {
            return;
        }

        // braces inside interpolation track nesting
        if c == b'{' {
            if let Some(Mode::InterpCode { depth }) = self.modes.last_mut() {
                *depth += 1;
            }
            self.push(L_CURLY, 1);
            return;
        }
        if c == b'}' {
            if let Some(Mode::InterpCode { depth }) = self.modes.last_mut() {
                if *depth == 0 {
                    self.modes.pop();
                } else {
                    *depth -= 1;
                }
            }
            self.push(R_CURLY, 1);
            return;
        }

        // everything else via logos
        let mut raw = Raw::lexer(self.rest());
        match raw.next() {
            Some(Ok(tok)) => {
                let mut len = raw.span().end;
                let mut kind = tok.kind();
                // StirPHP has no C-style casts (`as` instead), so `(Int) -> Int`
                // must lex as a parenthesis.
                if self.dialect == Dialect::Stir
                    && matches!(
                        kind,
                        INT_CAST
                            | FLOAT_CAST
                            | STRING_CAST
                            | BOOL_CAST
                            | ARRAY_CAST
                            | OBJECT_CAST
                            | UNSET_CAST
                            | VOID_CAST
                    )
                {
                    kind = L_PAREN;
                    len = 1;
                }
                if kind == IDENT {
                    let text = &self.rest()[..len];
                    if let Some(kw) = SyntaxKind::from_keyword(text) {
                        kind = kw;
                    }
                } else if kind == NAME_QUALIFIED {
                    let text = &self.rest()[..len];
                    if text.len() > 10 && text[..10].eq_ignore_ascii_case("namespace\\") {
                        kind = NAME_RELATIVE;
                    }
                }
                self.push(kind, len);
                if kind == HALT_COMPILER_KW {
                    self.lex_halt_compiler_tail();
                }
            }
            _ => {
                let len = self.rest().chars().next().map_or(1, char::len_utf8);
                self.error(format!("unexpected character `{}`", &self.rest()[..len]));
                self.push(ERROR_TOKEN, len);
            }
        }
    }

    /// After `__halt_compiler`, lex `(`, `)` and `;` (or `?>`), then the rest is raw data.
    fn lex_halt_compiler_tail(&mut self) {
        let expected = [b'(', b')'];
        for want in expected {
            self.skip_trivia_tokens();
            if self.peek(0) == Some(want) {
                self.push(if want == b'(' { L_PAREN } else { R_PAREN }, 1);
            } else {
                return;
            }
        }
        self.skip_trivia_tokens();
        if self.peek(0) == Some(b';') {
            self.push(SEMI, 1);
        } else if self.rest().starts_with("?>") {
            self.push(CLOSE_TAG, 2);
        } else {
            return;
        }
        self.halted = true;
    }

    fn skip_trivia_tokens(&mut self) {
        let len = self
            .bytes()
            .iter()
            .take_while(|b| b.is_ascii_whitespace())
            .count();
        if len > 0 {
            self.push(WHITESPACE, len);
        }
    }

    fn lex_line_comment(&mut self, prefix: usize) {
        let b = self.bytes();
        let mut i = prefix;
        while i < b.len() {
            match b[i] {
                b'\n' | b'\r' => break,
                // `?>` ends a line comment in PHP mode
                b'?' if b.get(i + 1) == Some(&b'>') && self.dialect == Dialect::Php => break,
                _ => i += 1,
            }
        }
        self.push(COMMENT, i);
    }

    fn lex_single_quoted(&mut self, prefix: usize) {
        let b = self.bytes();
        let mut i = prefix;
        while i < b.len() {
            match b[i] {
                b'\\' => i += 2,
                b'\'' => {
                    self.push(STRING_SINGLE, i + 1);
                    return;
                }
                _ => i += 1,
            }
        }
        self.error("unterminated string");
        let len = b.len();
        self.push(STRING_SINGLE, len);
    }

    /// Scans a double-quoted string. Without interpolation it becomes a single
    /// `STRING_DOUBLE` token; otherwise the lexer enters string mode.
    fn lex_double_quoted(&mut self, prefix: usize) {
        let b = self.bytes();
        let mut i = prefix;
        while i < b.len() {
            match b[i] {
                b'\\' => i += 2,
                b'"' => {
                    self.push(STRING_DOUBLE, i + 1);
                    return;
                }
                _ if self.interpolation_at(&b[i..]).is_some() => break,
                _ => i += 1,
            }
        }
        if i >= b.len() {
            self.error("unterminated string");
            let len = b.len();
            self.push(STRING_DOUBLE, len);
            return;
        }
        self.push(DQUOTE, prefix);
        self.modes.push(Mode::DoubleQuote);
    }

    /// Does an interpolation start at the beginning of `b`?
    fn interpolation_at(&self, b: &[u8]) -> Option<SyntaxKind> {
        match (b.first(), b.get(1)) {
            (Some(b'$'), Some(b'{')) => Some(DOLLAR_CURLY_OPEN),
            (Some(b'$'), Some(c)) if is_ident_start(*c) && self.dialect == Dialect::Php => {
                Some(VARIABLE)
            }
            (Some(b'{'), Some(b'$')) if self.dialect == Dialect::Php => Some(CURLY_OPEN),
            _ => None,
        }
    }

    /// Lexes either literal content or one interpolation inside `"..."` / `` `...` ``.
    fn lex_string_part(&mut self, quote: u8) {
        let b = self.bytes();
        if b[0] == quote {
            self.push(if quote == b'"' { DQUOTE } else { BACKTICK }, 1);
            self.modes.pop();
            return;
        }
        if self.lex_interpolation() {
            return;
        }
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'\\' {
                i += 2;
                continue;
            }
            if b[i] == quote || self.interpolation_at(&b[i..]).is_some() {
                break;
            }
            i += 1;
        }
        let i = i.min(b.len());
        if i == b.len() {
            self.error("unterminated string");
        }
        self.push(STRING_CONTENT, i);
    }

    /// Lexes an interpolation if one starts here. Returns whether it did.
    fn lex_interpolation(&mut self) -> bool {
        let b = self.bytes();
        match self.interpolation_at(b) {
            Some(DOLLAR_CURLY_OPEN) => {
                self.push(DOLLAR_CURLY_OPEN, 2);
                self.modes.push(Mode::InterpCode { depth: 0 });
                true
            }
            Some(CURLY_OPEN) => {
                self.push(CURLY_OPEN, 1);
                self.modes.push(Mode::InterpCode { depth: 0 });
                true
            }
            Some(VARIABLE) => {
                let len = 1 + b[1..].iter().take_while(|b| is_ident_continue(**b)).count();
                self.push(VARIABLE, len);
                self.lex_simple_interp_tail();
                true
            }
            _ => false,
        }
    }

    /// `"$a[0]"`, `"$a[key]"`, `"$a[$i]"`, `"$a->b"`, `"$a?->b"`.
    fn lex_simple_interp_tail(&mut self) {
        let b = self.bytes();
        if b.first() == Some(&b'[') {
            let mut j = 1;
            let mut inner: Option<(SyntaxKind, usize)> = None;
            if b.get(j) == Some(&b'-') {
                j += 1;
            }
            if b.get(j).is_some_and(|c| c.is_ascii_digit()) {
                let n = b[j..]
                    .iter()
                    .take_while(|c| c.is_ascii_alphanumeric() || **c == b'_')
                    .count();
                inner = Some((INT_NUMBER, j - 1 + n));
            } else if j == 1
                && b.get(1) == Some(&b'$')
                && b.get(2).copied().is_some_and(is_ident_start)
            {
                let n = 2 + b[2..].iter().take_while(|c| is_ident_continue(**c)).count();
                inner = Some((VARIABLE, n - 1));
            } else if j == 1 && b.get(1).copied().is_some_and(is_ident_start) {
                let n = 1 + b[2..].iter().take_while(|c| is_ident_continue(**c)).count();
                inner = Some((IDENT, n));
            }
            if let Some((kind, len)) = inner
                && b.get(1 + len) == Some(&b']')
            {
                self.push(L_BRACK, 1);
                if kind == INT_NUMBER && self.peek(0) == Some(b'-') {
                    self.push(MINUS, 1);
                    self.push(INT_NUMBER, len - 1);
                } else {
                    self.push(kind, len);
                }
                self.push(R_BRACK, 1);
            }
        } else if b.starts_with(b"->") && b.get(2).copied().is_some_and(is_ident_start) {
            self.push(ARROW, 2);
            let n = self
                .bytes()
                .iter()
                .take_while(|c| is_ident_continue(**c))
                .count();
            self.push(IDENT, n);
        } else if b.starts_with(b"?->") && b.get(3).copied().is_some_and(is_ident_start) {
            self.push(NULLSAFE_ARROW, 3);
            let n = self
                .bytes()
                .iter()
                .take_while(|c| is_ident_continue(**c))
                .count();
            self.push(IDENT, n);
        }
    }

    fn try_heredoc_start(&mut self) -> bool {
        let b = self.bytes();
        let mut i = 3;
        while matches!(b.get(i), Some(b' ' | b'\t')) {
            i += 1;
        }
        let quote = match b.get(i) {
            Some(q @ (b'\'' | b'"')) => {
                i += 1;
                Some(*q)
            }
            _ => None,
        };
        let label_start = i;
        if !b.get(i).copied().is_some_and(is_ident_start) {
            return false;
        }
        while b.get(i).copied().is_some_and(is_ident_continue) {
            i += 1;
        }
        let label = self.rest()[label_start..i].to_owned();
        if let Some(q) = quote {
            if b.get(i) != Some(&q) {
                return false;
            }
            i += 1;
        }
        match b.get(i) {
            Some(b'\n') => i += 1,
            Some(b'\r') => {
                i += 1;
                if b.get(i) == Some(&b'\n') {
                    i += 1;
                }
            }
            _ => return false,
        }
        self.push(HEREDOC_START, i);
        if quote == Some(b'\'') {
            self.lex_nowdoc_body(&label);
        } else {
            self.modes.push(Mode::Heredoc { label });
        }
        true
    }

    /// If a heredoc closing label starts at `b` (which must be at line start),
    /// returns the length of indentation + label.
    fn closing_label_len(b: &[u8], label: &str) -> Option<usize> {
        let indent = b.iter().take_while(|c| matches!(c, b' ' | b'\t')).count();
        let rest = &b[indent..];
        if rest.starts_with(label.as_bytes())
            && !rest
                .get(label.len())
                .copied()
                .is_some_and(is_ident_continue)
        {
            Some(indent + label.len())
        } else {
            None
        }
    }

    fn lex_nowdoc_body(&mut self, label: &str) {
        let b = self.bytes();
        let mut i = 0;
        loop {
            if let Some(len) = Self::closing_label_len(&b[i..], label) {
                if i > 0 {
                    self.push(STRING_CONTENT, i);
                }
                self.push(HEREDOC_END, len);
                return;
            }
            match b[i..].iter().position(|c| *c == b'\n') {
                Some(nl) => i += nl + 1,
                None => {
                    self.error("unterminated nowdoc");
                    let len = b.len();
                    self.push(STRING_CONTENT, len);
                    return;
                }
            }
        }
    }

    fn at_line_start(&self) -> bool {
        self.pos == 0 || matches!(self.src.as_bytes()[self.pos - 1], b'\n' | b'\r')
    }

    fn lex_heredoc_part(&mut self, label: &str) {
        if self.at_line_start()
            && let Some(len) = Self::closing_label_len(self.bytes(), label)
        {
            self.push(HEREDOC_END, len);
            self.modes.pop();
            return;
        }
        if self.lex_interpolation() {
            return;
        }
        let b = self.bytes();
        let mut i = 0;
        let mut line_start = self.at_line_start();
        while i < b.len() {
            if line_start && Self::closing_label_len(&b[i..], label).is_some() {
                break;
            }
            line_start = false;
            match b[i] {
                b'\\' => i += 2,
                b'\n' => {
                    i += 1;
                    line_start = true;
                }
                _ if self.interpolation_at(&b[i..]).is_some() => break,
                _ => i += 1,
            }
        }
        let i = i.min(b.len());
        if i == 0 {
            // Can only happen at EOF-adjacent garbage; avoid an infinite loop.
            self.error("unterminated heredoc");
            let len = b.len();
            self.push(STRING_CONTENT, len);
            return;
        }
        if i == b.len() {
            self.error("unterminated heredoc");
        }
        self.push(STRING_CONTENT, i);
    }
}

#[cfg(test)]
mod tests;
