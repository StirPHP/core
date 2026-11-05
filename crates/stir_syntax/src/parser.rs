//! Event-based parser core (in the style of rust-analyzer).
//!
//! The grammar functions drive a [`Parser`] over the non-trivia tokens and
//! record [`Event`]s. [`build_tree`] then replays the events against the full
//! token stream (trivia included) to build a lossless rowan green tree.

use rowan::{GreenNode, GreenNodeBuilder, TextRange, TextSize};

use crate::{Lexed, SyntaxKind, SyntaxKind::*};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxError {
    pub range: TextRange,
    pub message: String,
}

/// A set of token kinds, for lookahead and error recovery.
#[derive(Clone, Copy)]
pub(crate) struct TokenSet([u64; 8]);

impl TokenSet {
    pub(crate) const fn new(kinds: &[SyntaxKind]) -> TokenSet {
        let mut bits = [0u64; 8];
        let mut i = 0;
        while i < kinds.len() {
            let k = kinds[i] as usize;
            bits[k / 64] |= 1 << (k % 64);
            i += 1;
        }
        TokenSet(bits)
    }

    pub(crate) const fn contains(&self, kind: SyntaxKind) -> bool {
        let k = kind as usize;
        k < 512 && self.0[k / 64] & (1 << (k % 64)) != 0
    }
}

/// A non-trivia token as seen by the grammar.
#[derive(Debug, Clone, Copy)]
struct InputToken {
    kind: SyntaxKind,
    start: u32,
    len: u32,
}

#[derive(Debug)]
pub(crate) enum Event {
    Start {
        kind: SyntaxKind,
        forward_parent: Option<u32>,
    },
    Finish,
    Token {
        kind: SyntaxKind,
    },
    Error {
        message: String,
    },
}

pub(crate) struct Parser<'s> {
    src: &'s str,
    tokens: Vec<InputToken>,
    pos: usize,
    events: Vec<Event>,
    fuel: std::cell::Cell<u32>,
}

impl<'s> Parser<'s> {
    pub(crate) fn new(src: &'s str, lexed: &Lexed) -> Parser<'s> {
        let mut tokens = Vec::new();
        let mut offset = 0u32;
        for tok in &lexed.tokens {
            if !tok.kind.is_trivia() {
                tokens.push(InputToken {
                    kind: tok.kind,
                    start: offset,
                    len: tok.len,
                });
            }
            offset += tok.len;
        }
        Parser {
            src,
            tokens,
            pos: 0,
            events: Vec::new(),
            fuel: std::cell::Cell::new(256),
        }
    }

    pub(crate) fn finish(self) -> Vec<Event> {
        self.events
    }

    fn raw(&self, n: usize) -> Option<&InputToken> {
        self.tokens.get(self.pos + n)
    }

    /// Kind of the `n`th upcoming token.
    pub(crate) fn nth(&self, n: usize) -> SyntaxKind {
        assert!(self.fuel.get() > 0, "parser is stuck");
        self.fuel.set(self.fuel.get() - 1);
        self.raw(n).map_or(EOF, |t| t.kind)
    }

    pub(crate) fn current(&self) -> SyntaxKind {
        self.nth(0)
    }

    pub(crate) fn at(&self, kind: SyntaxKind) -> bool {
        self.nth(0) == kind
    }

    pub(crate) fn at_ts(&self, set: TokenSet) -> bool {
        set.contains(self.current())
    }

    pub(crate) fn nth_text(&self, n: usize) -> &'s str {
        match self.raw(n) {
            Some(t) => &self.src[t.start as usize..(t.start + t.len) as usize],
            None => "",
        }
    }

    /// Is the current token the contextual keyword `kw` (an `IDENT` with that text)?
    pub(crate) fn at_contextual(&self, kw: &str) -> bool {
        self.nth_at_contextual(0, kw)
    }

    pub(crate) fn nth_at_contextual(&self, n: usize, kw: &str) -> bool {
        self.nth(n) == IDENT && self.nth_text(n) == kw
    }

    fn do_bump(&mut self, kind: SyntaxKind) {
        self.pos += 1;
        self.fuel.set(256);
        self.events.push(Event::Token { kind });
    }

    /// Consumes the current token, which must be a `kind`.
    pub(crate) fn bump(&mut self, kind: SyntaxKind) {
        assert!(self.at(kind), "bump({kind:?}) at {:?}", self.current());
        self.do_bump(kind);
    }

    /// Consumes the current token whatever it is.
    pub(crate) fn bump_any(&mut self) {
        let kind = self.current();
        if kind == EOF {
            return;
        }
        self.do_bump(kind);
    }

    /// Consumes the current token, re-tagging it as `kind`.
    pub(crate) fn bump_remap(&mut self, kind: SyntaxKind) {
        if self.current() == EOF {
            return;
        }
        self.do_bump(kind);
    }

    pub(crate) fn eat(&mut self, kind: SyntaxKind) -> bool {
        if self.at(kind) {
            self.bump(kind);
            true
        } else {
            false
        }
    }

    pub(crate) fn expect(&mut self, kind: SyntaxKind) -> bool {
        if self.eat(kind) {
            return true;
        }
        self.error(format!("expected {}", describe(kind)));
        false
    }

    pub(crate) fn error(&mut self, message: impl Into<String>) {
        self.events.push(Event::Error {
            message: message.into(),
        });
    }

    /// Reports an error and wraps the current token in an `ERROR` node.
    pub(crate) fn err_and_bump(&mut self, message: impl Into<String>) {
        let m = self.start();
        self.error(message);
        self.bump_any();
        m.complete(self, ERROR);
    }

    /// Reports an error, consuming the current token unless it is in `recovery`.
    pub(crate) fn err_recover(&mut self, message: impl Into<String>, recovery: TokenSet) {
        if self.at_ts(recovery) || self.at(EOF) || self.at(R_CURLY) {
            self.error(message);
        } else {
            self.err_and_bump(message);
        }
    }

    pub(crate) fn start(&mut self) -> Marker {
        let pos = self.events.len() as u32;
        self.events.push(Event::Start {
            kind: TOMBSTONE,
            forward_parent: None,
        });
        Marker {
            pos,
            done: drop_bomb::DropBomb::new("marker must be completed or abandoned"),
        }
    }
}

/// Placeholder kind for abandoned or not-yet-completed `Start` events.
const TOMBSTONE: SyntaxKind = SyntaxKind::__LAST;

pub(crate) struct Marker {
    pos: u32,
    done: drop_bomb::DropBomb,
}

impl Marker {
    pub(crate) fn complete(mut self, p: &mut Parser<'_>, kind: SyntaxKind) -> CompletedMarker {
        self.done.defuse();
        match &mut p.events[self.pos as usize] {
            Event::Start { kind: slot, .. } => *slot = kind,
            _ => unreachable!(),
        }
        p.events.push(Event::Finish);
        CompletedMarker {
            pos: self.pos,
            kind,
        }
    }

    pub(crate) fn abandon(mut self, p: &mut Parser<'_>) {
        self.done.defuse();
        let idx = self.pos as usize;
        if idx == p.events.len() - 1 {
            p.events.pop();
        }
        // Otherwise it stays a TOMBSTONE start, which the builder skips.
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct CompletedMarker {
    pos: u32,
    kind: SyntaxKind,
}

impl CompletedMarker {
    pub(crate) fn kind(&self) -> SyntaxKind {
        self.kind
    }

    /// Starts a new node that will become the parent of this one.
    pub(crate) fn precede(self, p: &mut Parser<'_>) -> Marker {
        let m = p.start();
        match &mut p.events[self.pos as usize] {
            Event::Start { forward_parent, .. } => *forward_parent = Some(m.pos - self.pos),
            _ => unreachable!(),
        }
        m
    }
}

pub(crate) fn describe(kind: SyntaxKind) -> String {
    let s = match kind {
        SEMI => "`;`",
        COMMA => "`,`",
        L_PAREN => "`(`",
        R_PAREN => "`)`",
        L_BRACK => "`[`",
        R_BRACK => "`]`",
        L_CURLY => "`{`",
        R_CURLY => "`}`",
        COLON => "`:`",
        EQ => "`=`",
        ARROW => "`->`",
        FAT_ARROW => "`=>`",
        VARIABLE => "a variable",
        IDENT => "an identifier",
        DQUOTE => "`\"`",
        _ => return format!("{kind:?}"),
    };
    s.to_owned()
}

/// Kinds whose immediately preceding doc comment is attached inside the node.
fn attaches_doc_comment(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        FUNCTION_DECL
            | CLASS_DECL
            | INTERFACE_DECL
            | TRAIT_DECL
            | ENUM_DECL
            | METHOD_DECL
            | PROPERTY_DECL
            | CLASS_CONST_DECL
            | CONST_DECL
            | ENUM_CASE
            | PARAM
            | EXPR_STMT
            | RETURN_STMT
            | FOREACH_STMT
            | STATIC_VAR_STMT
            | GLOBAL_STMT
    )
}

/// Replays parser events over the full token stream into a green tree.
pub(crate) fn build_tree(
    src: &str,
    lexed: &Lexed,
    mut events: Vec<Event>,
) -> (GreenNode, Vec<SyntaxError>) {
    let mut builder = GreenNodeBuilder::new();
    let mut errors = Vec::new();
    let tokens = &lexed.tokens;
    let mut tok_idx = 0usize;
    let mut offset = 0usize;
    let mut depth = 0usize;

    let emit_raw = |builder: &mut GreenNodeBuilder, tok_idx: &mut usize, offset: &mut usize| {
        let t = tokens[*tok_idx];
        let len = t.len as usize;
        builder.token(t.kind.into(), &src[*offset..*offset + len]);
        *offset += len;
        *tok_idx += 1;
    };

    let mut forward_parents = Vec::new();
    let mut i = 0;
    while i < events.len() {
        match std::mem::replace(&mut events[i], Event::Finish) {
            Event::Start {
                kind,
                forward_parent,
            } => {
                if kind == TOMBSTONE && forward_parent.is_none() {
                    // Abandoned marker: emits nothing, and its Finish was never pushed.
                    events[i] = Event::Error {
                        message: String::new(),
                    };
                    i += 1;
                    continue;
                }
                forward_parents.clear();
                forward_parents.push(kind);
                let mut idx = i;
                let mut fp = forward_parent;
                while let Some(delta) = fp {
                    idx += delta as usize;
                    fp = match std::mem::replace(&mut events[idx], Event::Finish) {
                        Event::Start {
                            kind,
                            forward_parent,
                        } => {
                            forward_parents.push(kind);
                            // The replaced event must not emit a Finish of its own;
                            // mark it as a no-op error with empty message.
                            events[idx] = Event::Error {
                                message: String::new(),
                            };
                            forward_parent
                        }
                        _ => unreachable!(),
                    };
                }
                for (n, kind) in forward_parents.drain(..).rev().enumerate() {
                    if kind == TOMBSTONE {
                        continue;
                    }
                    if depth > 0 {
                        // Leading trivia goes before the node, except a doc comment
                        // (and what follows it) which belongs inside.
                        let mut end = tok_idx;
                        while end < tokens.len() && tokens[end].kind.is_trivia() {
                            end += 1;
                        }
                        let mut attach_from = end;
                        if n == 0 && attaches_doc_comment(kind) {
                            let mut j = end;
                            while j > tok_idx {
                                j -= 1;
                                match tokens[j].kind {
                                    DOC_COMMENT => {
                                        attach_from = j;
                                        break;
                                    }
                                    WHITESPACE => continue,
                                    _ => break,
                                }
                            }
                        }
                        while tok_idx < attach_from {
                            emit_raw(&mut builder, &mut tok_idx, &mut offset);
                        }
                    }
                    builder.start_node(kind.into());
                    depth += 1;
                }
            }
            Event::Finish => {
                if depth == 1 {
                    // Root: flush everything that's left.
                    while tok_idx < tokens.len() {
                        emit_raw(&mut builder, &mut tok_idx, &mut offset);
                    }
                }
                builder.finish_node();
                depth -= 1;
            }
            Event::Token { kind } => {
                while tokens[tok_idx].kind.is_trivia() {
                    emit_raw(&mut builder, &mut tok_idx, &mut offset);
                }
                let len = tokens[tok_idx].len as usize;
                builder.token(kind.into(), &src[offset..offset + len]);
                offset += len;
                tok_idx += 1;
            }
            Event::Error { message } => {
                if !message.is_empty() {
                    let mut j = tok_idx;
                    let mut at = offset;
                    while j < tokens.len() && tokens[j].kind.is_trivia() {
                        at += tokens[j].len as usize;
                        j += 1;
                    }
                    let end = if j < tokens.len() {
                        at + tokens[j].len as usize
                    } else {
                        at
                    };
                    errors.push(SyntaxError {
                        range: TextRange::new(TextSize::new(at as u32), TextSize::new(end as u32)),
                        message,
                    });
                }
            }
        }
        i += 1;
    }
    (builder.finish(), errors)
}
