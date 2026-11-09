//! Tokenizer for PHP source.
//!
//! The lexer is a small mode-stack state machine. Fixed-shape tokens in code
//! mode (operators, names, numbers, casts) are recognised by a [`logos`]
//! automaton; everything context-sensitive (inline HTML, comments ending at
//! `?>`, strings with interpolation, heredocs) is scanned by hand.
//!
//! A Blade template differs only in what its HTML holds. [`lex_blade`] finds echos and directives
//! there as Laravel's compiler does, and hands what they contain to the same code mode, stopped
//! at their end. They use the tokens of what they compile to: `{{` is an `OPEN_TAG_WITH_ECHO` and
//! `}}` a `CLOSE_TAG`, `@php` an `OPEN_TAG` and `@endphp` a `CLOSE_TAG`, `@foreach` a `FOREACH_KW`.

use logos::Logos;

use crate::{
    SyntaxKind::{self, *},
    blade::{self, Args},
};

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
    #[token(">=")]
    GtEq,
    #[token(">>")]
    Shr,
    #[token(">>=")]
    ShrEq,
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
            Raw::GtEq => GTEQ,
            Raw::Shr => SHR,
            Raw::ShrEq => SHR_EQ,
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
    /// The HTML of a Blade template.
    BladeHtml,
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
    modes: Vec<Mode>,
    out: Lexed,
    halted: bool,
    /// Where the input stops for now: the end of `src`, or that of the Blade region being lexed.
    limit: usize,
    blade: bool,
    in_blade_region: bool,
    /// The `{{ ... }}` of an `@{{ ... }}`, and the like: text up to here.
    no_echo_until: usize,
    /// The arguments of a directive that's left as text: what's in them isn't compiled.
    no_directive_until: usize,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str, mode: Mode) -> Lexer<'a> {
        Lexer {
            src,
            pos: 0,
            blade: matches!(mode, Mode::BladeHtml),
            modes: vec![mode],
            out: Lexed::default(),
            halted: false,
            limit: src.len(),
            in_blade_region: false,
            no_echo_until: 0,
            no_directive_until: 0,
        }
    }
}

/// Tokenizes PHP source. The token lengths always sum to `src.len()`.
pub fn lex(src: &str) -> Lexed {
    run_lexer(src, Mode::Html)
}

/// Tokenizes a Blade template. The token lengths always sum to `src.len()`.
pub fn lex_blade(src: &str) -> Lexed {
    run_lexer(src, Mode::BladeHtml)
}

fn run_lexer(src: &str, mode: Mode) -> Lexed {
    let mut lexer = Lexer::new(src, mode);
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

/// `\w` of a regular expression without the `u` flag, which is what Blade's are.
fn is_blade_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Where `needle` is in `hay`, looking from `from` on.
fn find_from(hay: &str, needle: &str, from: usize) -> Option<usize> {
    let from = (from..=hay.len()).find(|&i| hay.is_char_boundary(i))?;
    hay[from..].find(needle).map(|i| from + i)
}

/// The open tag at the start of `rest`, if there's one: its length and kind.
fn open_tag_at(rest: &str) -> Option<(usize, SyntaxKind)> {
    let after = rest.strip_prefix("<?")?;
    if after.starts_with('=') {
        return Some((3, OPEN_TAG_WITH_ECHO));
    }
    if after.len() >= 3 && after.as_bytes()[..3].eq_ignore_ascii_case(b"php") {
        match after.as_bytes().get(3) {
            None => return Some((5, OPEN_TAG)),
            Some(b' ' | b'\t' | b'\n') => return Some((6, OPEN_TAG)),
            Some(b'\r') => {
                let len = if after.as_bytes().get(4) == Some(&b'\n') {
                    7
                } else {
                    6
                };
                return Some((len, OPEN_TAG));
            }
            _ => {}
        }
    }
    None
}

/// What's at the `(` of a directive.
enum ArgsEnd {
    /// The offset after its `)`.
    Balanced(usize),
    /// It has a `)` but no balanced one, which leaves the whole directive as text.
    Unbalanced,
    /// No `)` at all, which leaves the `(` as text after a directive without arguments.
    Unclosed,
}

/// Finds the `)` that closes the `(` at `open`, by tokens, so not in a string or a comment. This
/// is Blade's `hasEvenNumberOfParentheses`: it uses PHP's tokenizer for this too.
fn args_end(src: &str, open: usize) -> ArgsEnd {
    let start = open + 1;
    let mut lexer = Lexer::new(&src[start..], Mode::Code);
    let (mut depth, mut seen, mut offset) = (1u32, 0, start);
    while lexer.pos < lexer.limit {
        lexer.step();
        for token in &lexer.out.tokens[seen..] {
            offset += token.len as usize;
            match token.kind {
                L_PAREN => depth += 1,
                R_PAREN => {
                    depth -= 1;
                    if depth == 0 {
                        return ArgsEnd::Balanced(offset);
                    }
                }
                _ => {}
            }
        }
        seen = lexer.out.tokens.len();
    }
    if src[start..].contains(')') {
        ArgsEnd::Unbalanced
    } else {
        ArgsEnd::Unclosed
    }
}

/// Does an interpolation start at the beginning of `b`?
fn interpolation_at(b: &[u8]) -> Option<SyntaxKind> {
    match (b.first(), b.get(1)) {
        (Some(b'$'), Some(b'{')) => Some(DOLLAR_CURLY_OPEN),
        (Some(b'$'), Some(c)) if is_ident_start(*c) => Some(VARIABLE),
        (Some(b'{'), Some(b'$')) => Some(CURLY_OPEN),
        _ => None,
    }
}

impl<'a> Lexer<'a> {
    fn rest(&self) -> &'a str {
        &self.src[self.pos..self.limit]
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
            self.step();
        }
    }

    fn step(&mut self) {
        if self.halted {
            let len = self.limit - self.pos;
            self.push(HALT_DATA, len);
            return;
        }
        match self
            .modes
            .last()
            .cloned()
            .expect("mode stack is never empty")
        {
            Mode::Html => self.lex_html(),
            Mode::BladeHtml => self.lex_blade_html(),
            Mode::Code | Mode::InterpCode { .. } => self.lex_code(),
            Mode::DoubleQuote => self.lex_string_part(b'"'),
            Mode::Backtick => self.lex_string_part(b'`'),
            Mode::Heredoc { label } => self.lex_heredoc_part(&label),
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
                    if let Some((len, kind)) = open_tag_at(&rest[at..]) {
                        break Some((at, len, kind));
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

    /// Lexes `self.pos..end` as code, then goes back to the HTML of the template.
    fn lex_blade_region(&mut self, end: usize) {
        let depth = self.modes.len();
        self.modes.push(Mode::Code);
        let outer = self.limit;
        self.limit = end;
        self.in_blade_region = true;
        while self.pos < end {
            self.step();
        }
        self.in_blade_region = false;
        self.limit = outer;
        self.modes.truncate(depth);
    }

    /// Lexes HTML up to the next thing Blade compiles, and that thing.
    fn lex_blade_html(&mut self) {
        let rest = self.rest();
        let bytes = rest.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'<' => {
                    if let Some((len, kind)) = open_tag_at(&rest[i..]) {
                        self.flush_html(i);
                        self.push(kind, len);
                        *self.modes.last_mut().unwrap() = Mode::Code;
                        return;
                    }
                    if let Some(tag) = blade::tag_at(&rest[i..]) {
                        self.flush_html(i);
                        self.lex_blade_tag(tag);
                        return;
                    }
                    i += 1;
                }
                b'{' => match self.lex_blade_brace(i) {
                    Some(next) => i = next,
                    None => return,
                },
                b'@' => match self.lex_blade_at(i) {
                    Some(next) => i = next,
                    None => return,
                },
                _ => i += 1,
            }
        }
        self.flush_html(i);
    }

    /// Lexes a component's tag, which starts here. The attributes are read as Laravel reads them
    /// once it knows it's a tag, which is not quite as it finds that out.
    fn lex_blade_tag(&mut self, tag: blade::Tag) {
        let start = self.pos;
        self.push(BLADE_TAG_START, tag.start);
        if tag.name_end > tag.start {
            self.push(BLADE_TAG_NAME, tag.name_end - tag.start);
        }
        let attrs_end = start + tag.attrs_end;
        if tag.closes {
            if attrs_end > self.pos {
                let text = self.src[self.pos..attrs_end].trim();
                let kind = if text.is_empty() {
                    WHITESPACE
                } else {
                    BLADE_ATTR_TEXT
                };
                self.push(kind, attrs_end - self.pos);
            }
        } else {
            self.lex_blade_attrs(attrs_end);
        }
        self.push(BLADE_TAG_END, tag.end - tag.attrs_end);
    }

    fn lex_blade_attrs(&mut self, end: usize) {
        let outer = self.limit;
        self.limit = end;
        while self.pos < end {
            let b = self.bytes();
            if b[0].is_ascii_whitespace() || b[0] == 0x0b {
                let len = b
                    .iter()
                    .take_while(|c| c.is_ascii_whitespace() || **c == 0x0b)
                    .count();
                self.push(WHITESPACE, len);
            } else if let Some(close) = blade::comment_end(b, 0) {
                self.push(BLADE_COMMENT, close);
            } else if let Some(len) = blade::class_or_style_end(b, 0) {
                self.push(BLADE_DIRECTIVE_NAME, 6);
                self.push(L_PAREN, 1);
                self.lex_blade_region_within(self.pos + len - 7);
            } else if let Some(len) = blade::attribute_bag_end(b, 0) {
                self.push(OPEN_TAG_WITH_ECHO, 2);
                self.lex_blade_region_within(self.pos + len - 4);
                self.push(CLOSE_TAG, 2);
            } else if let Some(len) = blade::short_attr_end(b, 0) {
                self.push(BLADE_ATTR_NAME, 1);
                self.lex_blade_region_within(self.pos + len - 1);
            } else {
                self.lex_blade_attr();
            }
        }
        self.limit = outer;
    }

    /// `name`, `name=value`, or something that's skipped between them.
    fn lex_blade_attr(&mut self) {
        let b = self.bytes();
        let name = b
            .iter()
            .take_while(|c| is_blade_word(**c) || matches!(c, b'-' | b':' | b'.' | b'@' | b'%'))
            .count();
        if name == 0 {
            let len = b
                .iter()
                .take_while(|c| {
                    !(c.is_ascii_whitespace()
                        || is_blade_word(**c)
                        || matches!(c, b'-' | b':' | b'.' | b'@' | b'%' | b'{'))
                })
                .count()
                .max(1);
            let len = (len..=b.len())
                .find(|&l| self.rest().is_char_boundary(l))
                .unwrap_or(b.len());
            self.push(BLADE_ATTR_TEXT, len);
            return;
        }
        let bound = blade::attr_is_bound(&self.rest()[..name]);
        self.push(BLADE_ATTR_NAME, name);
        if self.peek(0) != Some(b'=') {
            return;
        }
        self.push(EQ, 1);
        let b = self.bytes();
        let quoted = match b.first() {
            // Laravel's `"[^"]+"`: `""` is the empty value anyway
            Some(q @ (b'"' | b'\'')) => b[1..].iter().position(|c| c == q).map(|len| (1, len)),
            _ => None,
        };
        let (open, len) = quoted.unwrap_or((
            0,
            b.iter()
                .take_while(|c| !(c.is_ascii_whitespace() || **c == b'>'))
                .count(),
        ));
        if open > 0 {
            self.push(BLADE_ATTR_QUOTE, 1);
        }
        let value_end = self.pos + len;
        if bound {
            self.lex_blade_region_within(value_end);
        } else {
            self.lex_blade_attr_text(value_end);
        }
        if open > 0 {
            self.push(BLADE_ATTR_QUOTE, 1);
        }
    }

    /// What's in a tag that's PHP, if there's anything to it.
    fn lex_blade_region_within(&mut self, end: usize) {
        if self.pos < end {
            self.lex_blade_region(end);
        }
    }

    /// The value of an attribute that's text, in which echos are still compiled.
    fn lex_blade_attr_text(&mut self, end: usize) {
        let limit = self.limit;
        self.limit = end;
        while self.pos < end {
            let rest = self.rest();
            let mut i = 0;
            let mut echo = None;
            while i < rest.len() {
                match rest.as_bytes()[i] {
                    b'@' if rest.as_bytes().get(i + 1) == Some(&b'{') => {
                        if let Some((_, close, at)) = self.blade_echo_at(i + 1) {
                            i += 1 + at + close.len();
                            continue;
                        }
                    }
                    b'{' => {
                        if let Some(close) = blade::comment_end(rest.as_bytes(), i) {
                            echo = Some((i, None, close));
                            break;
                        }
                        if let Some((open, close, at)) = self.blade_echo_at(i) {
                            echo = Some((i, Some((open, close)), at));
                            break;
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            let text = echo.map_or(rest.len(), |(at, ..)| at);
            if text > 0 {
                self.push(BLADE_ATTR_TEXT, text);
            }
            match echo {
                None => {}
                Some((at, None, close)) => self.push(BLADE_COMMENT, close - at),
                Some((_, Some((open, close)), at)) => {
                    self.push(OPEN_TAG_WITH_ECHO, open);
                    self.lex_blade_region_within(self.pos + at - open);
                    self.push(CLOSE_TAG, close.len());
                }
            }
        }
        self.limit = limit;
    }

    fn flush_html(&mut self, len: usize) {
        if len > 0 {
            self.push(INLINE_HTML, len);
        }
    }

    /// The echo that opens at `i`: its opening's length, its closing and where that is.
    fn blade_echo_at(&self, i: usize) -> Option<(usize, &'static str, usize)> {
        let rest = &self.rest()[i..];
        // a regular expression's `.+?` wants a character, whatever it is
        let close_of = |open: usize, close: &str| find_from(rest, close, open + 1);
        if rest.starts_with("{!!") {
            return close_of(3, "!!}").map(|at| (3, "!!}", at));
        }
        if rest.starts_with("{{{")
            && let Some(at) = close_of(3, "}}}")
        {
            return Some((3, "}}}", at));
        }
        if rest.starts_with("{{") {
            return close_of(2, "}}").map(|at| (2, "}}", at));
        }
        None
    }

    /// Lexes what opens with the `{` at `i` of the rest, after the HTML before it, and returns
    /// `None`. If nothing does, returns where to go on looking.
    fn lex_blade_brace(&mut self, i: usize) -> Option<usize> {
        let rest = self.rest();
        if rest[i..].starts_with("{{--")
            && let Some(close) = find_from(rest, "--}}", i + 4)
        {
            self.flush_html(i);
            self.push(BLADE_COMMENT, close + 4 - i);
            return None;
        }
        if self.pos + i < self.no_echo_until {
            return Some(i + 1);
        }
        let Some((open, close, at)) = self.blade_echo_at(i) else {
            return Some(i + 1);
        };
        self.flush_html(i);
        self.push(OPEN_TAG_WITH_ECHO, open);
        let end = self.pos + at - open;
        self.lex_blade_region(end);
        self.push(CLOSE_TAG, close.len());
        None
    }

    /// Lexes what starts with the `@` at `i` of the rest, after the HTML before it, and returns
    /// `None`. If it's text, returns where to go on looking.
    fn lex_blade_at(&mut self, i: usize) -> Option<usize> {
        let rest = self.rest();
        let at = self.pos + i;
        let after = &rest[i + 1..];
        // Comments go first in Blade, leaving the `@` of `@{{-- a --}}` by itself.
        let comment = after.starts_with("{{--") && find_from(after, "--}}", 4).is_some();
        if after.starts_with('{') && !comment {
            if let Some((open, close, end)) = self.blade_echo_at(i + 1) {
                self.no_echo_until = at + 1 + end + close.len();
                return Some(i + 1 + open);
            }
            return Some(i + 1);
        }
        let previous = self.src.as_bytes()[..at].last().copied();
        if previous != Some(b'@') {
            if after.starts_with("verbatim")
                && let Some(close) = find_from(after, "@endverbatim", 8)
            {
                self.flush_html(i);
                self.push(BLADE_DIRECTIVE_NAME, 9);
                self.flush_html(close - 8);
                self.push(BLADE_DIRECTIVE_NAME, "@endverbatim".len());
                return None;
            }
            if after.starts_with("php")
                && !after.as_bytes().get(3).copied().is_some_and(is_blade_word)
                && let Some(close) = find_from(after, "@endphp", 3)
            {
                self.flush_html(i);
                self.push(OPEN_TAG, 4);
                let end = self.pos + close - 3;
                self.lex_blade_region(end);
                self.push(CLOSE_TAG, "@endphp".len());
                return None;
            }
        }
        // `\B@(@?\w+(?:::\w+)?)`
        if previous.is_some_and(is_blade_word) || at < self.no_directive_until {
            return Some(i + 1);
        }
        let bytes = after.as_bytes();
        let escaped = bytes.first() == Some(&b'@');
        let name_start = usize::from(escaped);
        let word = |from: usize| {
            bytes[from..]
                .iter()
                .take_while(|b| is_blade_word(**b))
                .count()
        };
        let mut name_end = name_start + word(name_start);
        if name_end == name_start {
            return Some(i + 1);
        }
        if after[name_end..].starts_with("::") && word(name_end + 2) > 0 {
            name_end += 2 + word(name_end + 2);
        }
        let name = &after[name_start..name_end];
        // what comes after the name: `[ \t]*`, and then maybe the arguments
        let name_end_abs = at + 1 + name_end;
        let spaces = bytes[name_end..]
            .iter()
            .take_while(|b| matches!(b, b' ' | b'\t'))
            .count();
        let open = name_end_abs + spaces;
        let args = (bytes.get(name_end + spaces) == Some(&b'('))
            .then(|| args_end(&self.src[..self.limit], open));
        let text_from = i + 1 + name_end;
        let directive = blade::directive(name).filter(|_| !escaped);
        let Some(directive) = directive else {
            // `@@if (...)`, and any `@name (...)` Blade doesn't know
            if let Some(ArgsEnd::Balanced(end)) = args {
                self.no_directive_until = end;
            }
            return Some(text_from);
        };
        if matches!(args, Some(ArgsEnd::Unbalanced)) {
            return Some(text_from);
        }
        let args = match args {
            Some(ArgsEnd::Balanced(end)) => Some(end),
            _ => None,
        };
        // `@php` without arguments or an `@endphp` compiles to itself
        if directive.name == "php" && args.is_none() {
            return Some(text_from);
        }
        let kind = match directive.kind {
            EMPTY_KW if args.is_none() => ENDFOREACH_KW,
            kind => kind,
        };
        self.flush_html(i);
        self.push(kind, 1 + name_end);
        let end = args?;
        if spaces > 0 {
            self.push(WHITESPACE, spaces);
        }
        match directive.args {
            Args::Php => {
                // not left to the code mode, which reads `(string)` as a cast
                self.push(L_PAREN, 1);
                self.lex_blade_region(end);
            }
            Args::Ignored => self.push(BLADE_IGNORED_ARGS, end - open),
            Args::Raw => self.push(BLADE_RAW_ARGS, end - open),
        }
        None
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

        // close tag
        if c == b'?' && b.get(1) == Some(&b'>') {
            let mut len = 2;
            if b.get(2) == Some(&b'\n') {
                len = 3;
            } else if b.get(2) == Some(&b'\r') {
                len = if b.get(3) == Some(&b'\n') { 4 } else { 3 };
            }
            self.push(CLOSE_TAG, len);
            *self.modes.last_mut().unwrap() = if self.blade && !self.in_blade_region {
                Mode::BladeHtml
            } else {
                Mode::Html
            };
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
                let len = raw.span().end;
                let mut kind = tok.kind();
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
                // `?>` ends a line comment
                b'?' if b.get(i + 1) == Some(&b'>') => break,
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
                _ if interpolation_at(&b[i..]).is_some() => break,
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
            if b[i] == quote || interpolation_at(&b[i..]).is_some() {
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
        match interpolation_at(b) {
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
                    if len > 0 {
                        self.push(STRING_CONTENT, len);
                    }
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
                _ if interpolation_at(&b[i..]).is_some() => break,
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
