//! A code formatter for PHP, in the style of PER-CS.
//!
//! It only ever changes whitespace: what's between tokens, and the indentation of the lines of a
//! block comment. Every token comes out as it went in, in the same order; no comma, brace or
//! parenthesis is added or taken away. Strings, heredocs and inline HTML are not touched at all, and
//! [`format`] checks all of that against a lexer before answering, so a mistake here is an error
//! rather than a changed program.
//!
//! What it normalizes: spaces around tokens, indentation, how many blank lines there are, where
//! braces go, and one statement to a line. It keeps the line breaks it finds inside expressions and
//! never wraps a long line, so what's run through it twice comes out the same; and it keeps how
//! lines continuing an expression are indented relative to its start, as projects lay out fluent
//! calls with meaning. Files with HTML around their PHP keep their lines as they are, and only get
//! spaces within lines normalized.
//!
//! PER-CS allows some things this leaves as found: alignment before `=`, `=>` and trailing comments,
//! and blank lines inside bodies, up to one.

mod check;
mod layout;
mod rules;

use stir_syntax::{TextRange, TextSize};

pub use check::same_tokens;

/// How a level of indentation is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Indent {
    Spaces(u32),
    Tabs,
}

impl Default for Indent {
    fn default() -> Self {
        Indent::Spaces(4)
    }
}

impl Indent {
    /// How many columns a level takes. Also what a tab is taken for when reading a file.
    pub(crate) fn width(self) -> u32 {
        match self {
            Indent::Spaces(n) => n.max(1),
            Indent::Tabs => 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Options {
    pub indent: Indent,
}

/// A replacement of some whitespace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub range: TextRange,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The file has syntax errors: nobody can tell where its tokens belong.
    Syntax,
    /// Formatting would have changed a token. A bug, and nothing's been changed.
    TokensChanged,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Error::Syntax => "the file has syntax errors",
            Error::TokensChanged => "formatting would have changed a token, so it was left alone",
        })
    }
}

impl std::error::Error for Error {}

/// `source`, formatted.
pub fn format(source: &str, options: &Options) -> Result<String, Error> {
    let edits = edits(source, options)?;
    Ok(apply(source, &edits))
}

/// What formatting `source` changes, in order.
pub fn edits(source: &str, options: &Options) -> Result<Vec<Edit>, Error> {
    let parse = stir_syntax::parse(source);
    if !parse.errors().is_empty() {
        return Err(Error::Syntax);
    }
    let edits = layout::layout(&parse.syntax(), source, options);
    if !same_tokens(source, &apply(source, &edits)) {
        return Err(Error::TokensChanged);
    }
    Ok(edits)
}

/// What formatting `source` changes that touches `range`: the part of a whole format inside it.
pub fn range_edits(source: &str, range: TextRange, options: &Options) -> Result<Vec<Edit>, Error> {
    Ok(edits(source, options)?
        .into_iter()
        .filter(|e| touches(e.range, range))
        .collect())
}

fn touches(a: TextRange, b: TextRange) -> bool {
    a.start() <= b.end() && b.start() <= a.end()
}

/// `edits` applied to `source`.
pub fn apply(source: &str, edits: &[Edit]) -> String {
    let mut out = String::with_capacity(source.len() + source.len() / 8);
    let mut at = TextSize::new(0);
    for edit in edits {
        out.push_str(&source[usize::from(at)..usize::from(edit.range.start())]);
        out.push_str(&edit.text);
        at = edit.range.end();
    }
    out.push_str(&source[usize::from(at)..]);
    out
}
