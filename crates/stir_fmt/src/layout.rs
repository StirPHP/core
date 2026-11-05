//! Where lines break and how far they're indented.

use stir_syntax::{SyntaxKind, SyntaxKind::*, SyntaxNode, TextRange, TextSize, WalkEvent};

use crate::rules::{Atom, Space, between, is_comment, is_line_comment, is_structural_brace};
use crate::{Edit, Options};

/// What's inside an open bracket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameKind {
    /// Statements or members.
    Statements,
    /// Statements under `case`s.
    Switch,
    /// Items, separated by commas.
    List,
}

#[derive(Debug)]
struct Frame {
    kind: FrameKind,
    /// The column of what's inside.
    inner: u32,
    /// The column of the closing bracket, on a line of its own.
    closer: u32,
    /// Is the next token the start of a statement or an item?
    at_start: bool,
    /// The line the current statement or item started on: its column here, and where it was.
    item_new: u32,
    item_old: u32,
    /// Everything's on one line, and stays there.
    inline: bool,
    /// For `:` blocks, which have no closing bracket: where they end.
    end: Option<usize>,
    /// The first item is on the bracket's own line: what continues it may be aligned with that.
    hanging: bool,
    /// The current item has gone over a line.
    continued: bool,
}

fn atoms(root: &SyntaxNode) -> Vec<Atom> {
    let mut out = Vec::new();
    let mut walk = root.preorder_with_tokens();
    while let Some(event) = walk.next() {
        let WalkEvent::Enter(element) = event else {
            continue;
        };
        match element {
            stir_syntax::NodeOrToken::Node(node) => {
                if matches!(node.kind(), INTERP_STRING | HEREDOC | SHELL_EXEC) {
                    let range = node.text_range();
                    out.push(Atom {
                        kind: node.kind(),
                        parent: node.parent().map_or(SOURCE_FILE, |p| p.kind()),
                        start: usize::from(range.start()),
                        end: usize::from(range.end()),
                        token: None,
                    });
                    walk.skip_subtree();
                }
            }
            stir_syntax::NodeOrToken::Token(token) => {
                if token.kind() == WHITESPACE {
                    continue;
                }
                let range = token.text_range();
                out.push(Atom {
                    kind: token.kind(),
                    parent: token.parent().map_or(SOURCE_FILE, |p| p.kind()),
                    start: usize::from(range.start()),
                    end: usize::from(range.end()),
                    token: Some(token),
                });
            }
        }
    }
    out
}

/// PHP mixed into HTML: its line structure is the template's.
fn is_template(atoms: &[Atom]) -> bool {
    let mut closed = false;
    for atom in atoms {
        match atom.kind {
            CLOSE_TAG => closed = true,
            INLINE_HTML if closed => return true,
            OPEN_TAG_WITH_ECHO => return true,
            _ => {}
        }
    }
    false
}

/// The node that `atom` is the first token of, at the outermost: `kind` if that's among them.
fn starts(atom: &Atom, kind: SyntaxKind) -> Option<SyntaxNode> {
    let token = atom.token.as_ref()?;
    token
        .parent_ancestors()
        .take_while(|n| usize::from(n.text_range().start()) == atom.start)
        .find(|n| n.kind() == kind)
}

fn ends(atom: &Atom, kind: SyntaxKind) -> bool {
    atom.token.as_ref().is_some_and(|t| {
        t.parent_ancestors()
            .take_while(|n| usize::from(n.text_range().end()) == atom.end)
            .any(|n| n.kind() == kind)
    })
}

fn use_kind(node: &SyntaxNode) -> SyntaxKind {
    node.children_with_tokens()
        .map(|e| e.kind())
        .find(|k| matches!(k, FUNCTION_KW | CONST_KW))
        .unwrap_or(USE_KW)
}

fn use_decl_ending(atom: &Atom) -> Option<SyntaxNode> {
    atom.token
        .as_ref()?
        .parent_ancestors()
        .take_while(|n| usize::from(n.text_range().end()) == atom.end)
        .find(|n| n.kind() == USE_DECL)
}

/// Must a blank line go between these? PER-CS's blocks at the top of a file.
fn wants_blank(prev: &Atom, next: &Atom) -> bool {
    if is_comment(prev.kind) || is_comment(next.kind) {
        return false;
    }
    if starts(next, NAMESPACE_DECL).is_some() || ends(prev, NAMESPACE_DECL) && prev.kind != R_CURLY
    {
        return true;
    }
    if prev.kind == OPEN_TAG
        && (starts(next, USE_DECL).is_some() || starts(next, DECLARE_STMT).is_some())
    {
        return true;
    }
    if prev.kind == SEMI && prev.parent == DECLARE_STMT && starts(next, DECLARE_STMT).is_none() {
        return true;
    }
    match (use_decl_ending(prev), starts(next, USE_DECL)) {
        (Some(a), Some(b)) => use_kind(&a) != use_kind(&b),
        (Some(_), None) => next.kind != CLOSE_TAG && next.kind != R_CURLY,
        (None, Some(_)) => prev.kind != L_CURLY,
        (None, None) => false,
    }
}

/// Does the closing `}` of `atom` end a statement, or is more of an expression to come?
fn ends_statement(atom: &Atom) -> bool {
    let Some(parent) = atom.token.as_ref().and_then(|t| t.parent()) else {
        return false;
    };
    match atom.parent {
        BLOCK | SWITCH_STMT | PROPERTY_HOOKS | TRAIT_ADAPTATIONS => !parent
            .parent()
            .is_some_and(|g| matches!(g.kind(), CLOSURE_EXPR | ARROW_FN_EXPR)),
        CLASS_BODY => parent.parent().is_none_or(|g| g.kind() != ANON_CLASS),
        _ => false,
    }
}

/// Is this the `{` of a statement's or a declaration's body, rather than of an expression's?
fn is_statement_brace(atom: &Atom) -> bool {
    if !is_structural_brace(atom) || atom.parent == MATCH_EXPR {
        return false;
    }
    let Some(parent) = atom.token.as_ref().and_then(|t| t.parent()) else {
        return false;
    };
    match atom.parent {
        BLOCK => !parent
            .parent()
            .is_some_and(|g| matches!(g.kind(), CLOSURE_EXPR | ARROW_FN_EXPR)),
        CLASS_BODY => parent.parent().is_none_or(|g| g.kind() != ANON_CLASS),
        _ => true,
    }
}

/// Can what's between these braces stay on one line?
fn may_stay_inline(atom: &Atom) -> bool {
    let Some(parent) = atom.token.as_ref().and_then(|t| t.parent()) else {
        return false;
    };
    let grand = parent.parent().map(|g| g.kind());
    let allowed = match atom.parent {
        BLOCK => matches!(grand, Some(CLOSURE_EXPR | PROPERTY_HOOK)),
        CLASS_BODY => grand == Some(ANON_CLASS),
        MATCH_EXPR | PROPERTY_HOOKS | TRAIT_ADAPTATIONS => true,
        _ => false,
    };
    allowed && !parent.text().contains_char('\n')
}

/// Where the `{` of a declaration goes.
#[derive(PartialEq, Eq)]
enum Brace {
    /// `{` on a line of its own.
    OwnLine,
    /// After what's before it.
    SameLine,
    /// Wherever it is.
    AsFound,
}

fn brace_place(brace: &Atom) -> Brace {
    let Some(parent) = brace.token.as_ref().and_then(|t| t.parent()) else {
        return Brace::AsFound;
    };
    let empty = !parent.text().contains_char('\n')
        && parent
            .children_with_tokens()
            .filter(|e| e.kind() != WHITESPACE)
            .count()
            == 2;
    let empty_stays = |place| if empty { Brace::AsFound } else { place };
    match brace.parent {
        CLASS_BODY => match parent.parent().map(|g| g.kind()) {
            Some(ANON_CLASS) => Brace::AsFound,
            _ => empty_stays(Brace::OwnLine),
        },
        BLOCK => match parent.parent().map(|g| g.kind()) {
            Some(FUNCTION_DECL | METHOD_DECL) => {
                let signature = parent
                    .parent()
                    .and_then(|g| g.children().find(|c| c.kind() == PARAM_LIST));
                // `) {` when the parenthesis is on a line of its own
                if signature.is_some_and(|p| closes_on_own_line(&p)) {
                    Brace::SameLine
                } else {
                    empty_stays(Brace::OwnLine)
                }
            }
            // a statement of its own, in a list of them
            Some(BLOCK | SOURCE_FILE | CLASS_BODY) | None => Brace::AsFound,
            Some(PROPERTY_HOOK) => Brace::AsFound,
            _ => Brace::SameLine,
        },
        SWITCH_STMT | MATCH_EXPR | PROPERTY_HOOKS | TRAIT_ADAPTATIONS => Brace::SameLine,
        _ => Brace::AsFound,
    }
}

/// Does the last token of `list` start a line?
fn closes_on_own_line(list: &SyntaxNode) -> bool {
    let Some(close) = list.last_token() else {
        return false;
    };
    close
        .prev_token()
        .is_some_and(|t| t.kind() == WHITESPACE && t.text().contains('\n'))
}

fn joins_closing_brace(next: &Atom) -> bool {
    matches!(next.kind, ELSE_KW | ELSEIF_KW | CATCH_KW | FINALLY_KW)
        || (next.kind == WHILE_KW && next.parent == DO_WHILE_STMT)
}

/// The width of indentation as written.
fn column(indent: &str, width: u32) -> u32 {
    let mut col = 0;
    for c in indent.chars() {
        if c == '\t' {
            col += width - col % width;
        } else {
            col += 1;
        }
    }
    col
}

fn write_indent(out: &mut String, col: u32, options: &Options) {
    match options.indent {
        crate::Indent::Spaces(_) => out.extend(std::iter::repeat_n(' ', col as usize)),
        crate::Indent::Tabs => {
            let width = options.indent.width();
            out.extend(std::iter::repeat_n('\t', (col / width) as usize));
            out.extend(std::iter::repeat_n(' ', (col % width) as usize));
        }
    }
}

fn range(start: usize, end: usize) -> TextRange {
    TextRange::new(
        TextSize::try_from(start).unwrap_or_default(),
        TextSize::try_from(end).unwrap_or_default(),
    )
}

fn push_edit(edits: &mut Vec<Edit>, source: &str, start: usize, end: usize, text: String) {
    if source[start..end] != text {
        edits.push(Edit {
            range: range(start, end),
            text,
        });
    }
}

struct Layout<'a> {
    source: &'a str,
    options: &'a Options,
    newline: &'static str,
    frames: Vec<Frame>,
    /// The line being written: its column, and where it was.
    line_new: u32,
    line_old: u32,
}

impl Layout<'_> {
    fn width(&self) -> u32 {
        self.options.indent.width()
    }

    fn top(&mut self) -> &mut Frame {
        self.frames.last_mut().expect("the file's own frame")
    }

    fn push_frame(&mut self, kind: FrameKind, col: u32, inline: bool, end: Option<usize>) {
        let width = self.width();
        self.frames.push(Frame {
            kind,
            inner: col + width,
            closer: col,
            at_start: true,
            item_new: col + width,
            item_old: self.line_old,
            inline,
            end,
            hanging: false,
            continued: false,
        });
    }

    /// The column of `next`, starting a line that was at `old` if it was a line at all.
    fn column_of(&mut self, atoms: &[Atom], i: usize, old: Option<u32>) -> u32 {
        let next = &atoms[i];
        let width = self.width();
        if matches!(next.kind, R_PAREN | R_BRACK | R_CURLY) {
            return self.top().closer;
        }
        let frame = self.frames.last().expect("the file's own frame");
        // a chain's `;` on a line of its own goes with where it started
        if next.kind == SEMI {
            return frame.item_new;
        }
        if is_comment(next.kind) && frame.at_start {
            let following = atoms[i + 1..].iter().find(|a| !is_comment(a.kind));
            // written at the closing brace's level, it's about what comes after that
            if let Some(closing) = following.filter(|a| a.kind == R_CURLY)
                && old.is_some_and(|o| o <= old_column(self.source, closing.start, width))
                && !frame.inline
            {
                return frame.closer;
            }
            return match following.map(|a| a.kind) {
                _ if frame.kind != FrameKind::Switch => frame.inner,
                // `// no break` says something about the case above it
                Some(CASE_KW | DEFAULT_KW) => {
                    let case = following.expect("just found");
                    let case_old = old_column(self.source, case.start, width);
                    if old.is_some_and(|o| o > case_old) {
                        frame.inner + width
                    } else {
                        frame.inner
                    }
                }
                _ => frame.inner + width,
            };
        }
        if frame.at_start {
            return match frame.kind {
                FrameKind::Switch if !matches!(next.kind, CASE_KW | DEFAULT_KW) => {
                    frame.inner + width
                }
                _ => frame.inner,
            };
        }
        // the `{` of a declaration whose head is broken over lines
        if next.kind == L_CURLY && is_structural_brace(next) {
            return frame.item_new;
        }
        // a statement's own level is taken only by more of a chain that's left it already
        let chain_goes_on = frame.continued && matches!(next.kind, ARROW | NULLSAFE_ARROW);
        // `new`, then a doc comment for the anonymous class
        let new_on_its_own = i > 0
            && (atoms[i - 1].kind == NEW_KW
                || (is_comment(atoms[i - 1].kind) && next.kind == CLASS_KW));
        let least = match frame.kind {
            FrameKind::List => 0,
            FrameKind::Statements | FrameKind::Switch if chain_goes_on || new_on_its_own => 0,
            FrameKind::Statements | FrameKind::Switch => i64::from(width),
        };
        let relative = old.map_or(least, |o| i64::from(o) - i64::from(frame.item_old));
        let col = i64::from(frame.item_new) + relative.max(least);
        let lowest = if frame.hanging {
            frame.item_new
        } else {
            frame.inner
        };
        u32::try_from(col.max(i64::from(lowest))).unwrap_or(frame.inner)
    }

    /// Bookkeeping once `atom` is placed.
    fn after(&mut self, atoms: &[Atom], i: usize) {
        let atom = &atoms[i];
        if is_comment(atom.kind) {
            return;
        }
        let (line_new, line_old) = (self.line_new, self.line_old);
        let is_closer = matches!(atom.kind, R_PAREN | R_BRACK | R_CURLY);
        if !is_closer {
            let frame = self.top();
            if frame.at_start {
                frame.at_start = false;
                frame.hanging = line_new < frame.inner;
                frame.continued = false;
                frame.item_new = line_new;
                frame.item_old = line_old;
            }
        }
        match atom.kind {
            L_PAREN | L_BRACK | ATTR_OPEN => {
                self.push_frame(FrameKind::List, line_new, false, None);
            }
            L_CURLY => {
                let (kind, inline) = if is_structural_brace(atom) {
                    let kind = match atom.parent {
                        SWITCH_STMT => FrameKind::Switch,
                        MATCH_EXPR => FrameKind::List,
                        _ => FrameKind::Statements,
                    };
                    (kind, may_stay_inline(atom))
                } else {
                    (FrameKind::List, false)
                };
                // a statement's body closes where the statement started, however its head is broken
                let col = if is_statement_brace(atom) {
                    self.top().item_new
                } else {
                    line_new
                };
                self.push_frame(kind, col, inline, None);
            }
            R_PAREN | R_BRACK | R_CURLY => {
                if self.frames.len() > 1 {
                    self.frames.pop();
                }
                let starts_next = (atom.kind == R_CURLY && ends_statement(atom))
                    || (atom.kind == R_BRACK && atom.parent == ATTRIBUTE_GROUP);
                if starts_next {
                    self.top().at_start = true;
                }
            }
            COLON if atom.parent == BLOCK => {
                let end = atom
                    .token
                    .as_ref()
                    .and_then(|t| t.parent())
                    .map(|n| usize::from(n.text_range().end()));
                self.push_frame(FrameKind::Statements, line_new, false, end);
            }
            COLON if atom.parent == SWITCH_STMT => {
                let end = atom.token.as_ref().and_then(|t| t.parent()).and_then(|n| {
                    n.children_with_tokens()
                        .find(|e| e.kind() == ENDSWITCH_KW)
                        .map(|e| usize::from(e.text_range().start()))
                });
                self.push_frame(FrameKind::Switch, line_new, false, end);
            }
            COLON if matches!(atom.parent, SWITCH_CASE | LABEL_STMT) => {
                self.top().at_start = true;
            }
            SEMI if self.top().kind != FrameKind::List || atom.parent == FOR_STMT => {
                self.top().at_start = true;
            }
            COMMA if self.top().kind == FrameKind::List => {
                self.top().at_start = true;
            }
            OPEN_TAG | CLOSE_TAG => {
                self.top().at_start = true;
            }
            _ => {}
        }
    }

    /// Takes off the frames of `:` blocks that have ended by `at`.
    fn close_ended(&mut self, at: usize) {
        while let Some(end) = self.frames.last().and_then(|f| f.end) {
            if end > at || self.frames.len() < 2 {
                break;
            }
            self.frames.pop();
            self.top().at_start = true;
        }
    }

    fn inline(&self) -> bool {
        self.frames.iter().any(|f| f.inline)
    }

    /// A multi-line comment moved from column `old` to `new`: its lines go along with it.
    fn comment_lines(&self, atom: &Atom, old: u32, new: u32, edits: &mut Vec<Edit>) {
        let text = &self.source[atom.start..atom.end];
        if !text.contains('\n') {
            return;
        }
        let lines: Vec<(usize, &str)> = text
            .match_indices('\n')
            .map(|(i, _)| i + 1)
            .zip(text.split('\n').skip(1))
            .collect();
        // `*`s line up one column in, whatever's been done to them
        let stars = lines.iter().all(|(_, l)| l.trim_start().starts_with('*'));
        for (at, line) in &lines {
            let ws = line.len() - line.trim_start_matches([' ', '\t']).len();
            let body = line[ws..].trim_end_matches('\r');
            let start = atom.start + at;
            let col = if stars {
                new + 1
            } else {
                let here = column(&line[..ws], self.width());
                let moved = i64::from(here) + i64::from(new) - i64::from(old);
                u32::try_from(moved.max(0)).unwrap_or(0)
            };
            if body.is_empty() {
                // nothing but whitespace on this line
                if ws > 0 {
                    push_edit(edits, self.source, start, start + ws, String::new());
                }
                continue;
            }
            let mut indent = String::new();
            write_indent(&mut indent, col, self.options);
            push_edit(edits, self.source, start, start + ws, indent);
            if stars {
                let trimmed = body.trim_end_matches([' ', '\t']);
                let end = start + ws + trimmed.len();
                let line_end = start + ws + body.len();
                if end < line_end {
                    push_edit(edits, self.source, end, line_end, String::new());
                }
            }
        }
    }
}

/// The column of the line of `at` in the source.
fn old_column(source: &str, at: usize, width: u32) -> u32 {
    let start = source[..at].rfind('\n').map_or(0, |i| i + 1);
    let indent = &source[start..at];
    let ws = indent.len() - indent.trim_start_matches([' ', '\t']).len();
    column(&indent[..ws], width)
}

fn newline_of(source: &str) -> &'static str {
    match source.find('\n') {
        Some(i) if i > 0 && source.as_bytes()[i - 1] == b'\r' => "\r\n",
        _ => "\n",
    }
}

/// Does `atom`'s own text end in a line break, as the tags' do?
fn ends_in_newline(atom: &Atom, source: &str) -> bool {
    matches!(atom.kind, OPEN_TAG | CLOSE_TAG) && source[..atom.end].ends_with('\n')
}

pub(crate) fn layout(root: &SyntaxNode, source: &str, options: &Options) -> Vec<Edit> {
    let atoms = atoms(root);
    let mut edits = Vec::new();
    if atoms.is_empty() {
        return edits;
    }
    let template = is_template(&atoms);
    let mut l = Layout {
        source,
        options,
        newline: newline_of(source),
        frames: vec![Frame {
            kind: FrameKind::Statements,
            inner: 0,
            closer: 0,
            at_start: true,
            item_new: 0,
            item_old: 0,
            inline: false,
            end: None,
            hanging: false,
            continued: false,
        }],
        line_new: 0,
        line_old: 0,
    };
    l.after(&atoms, 0);
    for i in 1..atoms.len() {
        let (prev, next) = (&atoms[i - 1], &atoms[i]);
        l.close_ended(next.start);
        let gap = &source[prev.end..next.start];
        let gap_lines = gap.matches('\n').count();
        let line_breaks = gap_lines + usize::from(ends_in_newline(prev, source));
        // what the gap can't lose
        let html = matches!(prev.kind, INLINE_HTML | HALT_DATA)
            || matches!(next.kind, INLINE_HTML | HALT_DATA)
            || (prev.kind == CLOSE_TAG)
            || (next.kind == OPEN_TAG)
            || (next.kind == OPEN_TAG_WITH_ECHO);
        if html {
            l.after(&atoms, i);
            continue;
        }
        let trailing_ws = |gap: &str| gap.rsplit('\n').next().unwrap_or("").to_owned();
        let old = (line_breaks > 0).then(|| column(&trailing_ws(gap), l.width()));
        if template {
            if line_breaks == 0 {
                let text = same_line(prev, next, source, gap);
                push_edit(&mut edits, source, prev.end, next.start, text);
            } else if let Some(o) = old {
                l.line_old = o;
                l.line_new = o;
            }
            l.after(&atoms, i);
            continue;
        }

        let inline = l.inline();
        let breaks = wants_break(prev, next, source, inline, line_breaks);
        let breaks = match breaks {
            Some(b) => b,
            None => line_breaks > 0,
        };
        if !breaks {
            let text = same_line(prev, next, source, gap);
            push_edit(&mut edits, source, prev.end, next.start, text);
            l.after(&atoms, i);
            continue;
        }

        let blank = (line_breaks >= 2 && !no_blank(prev, next)) || wants_blank(prev, next);
        let col = l.column_of(&atoms, i, old);
        let mut text = String::new();
        let own_break = ends_in_newline(prev, source);
        let mut lines = 1 + usize::from(blank);
        if own_break {
            lines -= 1;
        }
        for _ in 0..lines {
            text.push_str(l.newline);
        }
        write_indent(&mut text, col, options);
        push_edit(&mut edits, source, prev.end, next.start, text);
        l.line_new = col;
        if !is_comment(next.kind) {
            let frame = l.top();
            if !frame.at_start {
                frame.continued = true;
            }
        }
        // a line that wasn't one yet is reckoned from the line it was part of
        if let Some(o) = old {
            l.line_old = o;
        }
        if is_comment(next.kind)
            && let Some(o) = old
        {
            l.comment_lines(next, o, col, &mut edits);
        }
        l.after(&atoms, i);
    }

    // comments end where their text does, and files in a line break
    for atom in &atoms {
        if is_line_comment(atom, source) {
            let text = &source[atom.start..atom.end];
            let trimmed = text.trim_end_matches([' ', '\t']);
            if trimmed.len() < text.len() {
                edits.push(Edit {
                    range: range(atom.start + trimmed.len(), atom.end),
                    text: String::new(),
                });
            }
        }
    }
    let last = atoms.last().expect("not empty");
    if !template
        && !matches!(
            last.kind,
            INLINE_HTML | CLOSE_TAG | HALT_DATA | HALT_COMPILER_KW
        )
    {
        let own = ends_in_newline(last, source);
        let text = if own {
            String::new()
        } else {
            l.newline.to_owned()
        };
        push_edit(&mut edits, source, last.end, source.len(), text);
    }
    edits.sort_by_key(|e| e.range.start());
    edits
}

/// `true` or `false` if something says whether to break between these; `None` to keep it as found.
fn wants_break(
    prev: &Atom,
    next: &Atom,
    source: &str,
    inline: bool,
    line_breaks: usize,
) -> Option<bool> {
    if is_line_comment(prev, source) {
        return Some(next.kind != CLOSE_TAG);
    }
    if prev.kind == HEREDOC || prev.kind == OPEN_TAG {
        return (line_breaks > 0).then_some(true);
    }
    if is_comment(next.kind) {
        // a comment after code stays there; before code, as it is
        return None;
    }
    if is_comment(prev.kind) {
        return None;
    }
    if next.kind == CLOSE_TAG {
        return None;
    }
    if wants_blank(prev, next) {
        return Some(true);
    }
    if inline {
        return None;
    }
    if next.kind == SEMI {
        return None;
    }
    if next.kind == L_CURLY && is_structural_brace(next) {
        match brace_place(next) {
            Brace::OwnLine => return Some(true),
            Brace::SameLine => return Some(false),
            Brace::AsFound => return None,
        }
    }
    if prev.kind == R_CURLY && joins_closing_brace(next) {
        return Some(false);
    }
    if prev.kind == L_CURLY && is_structural_brace(prev) {
        return if next.kind == R_CURLY {
            None
        } else {
            Some(true)
        };
    }
    if next.kind == R_CURLY && is_structural_brace(next) {
        return Some(true);
    }
    if prev.kind == SEMI && !matches!(prev.parent, FOR_STMT) {
        return Some(true);
    }
    if prev.kind == R_CURLY
        && ends_statement(prev)
        && !matches!(next.kind, COMMA | R_PAREN | R_BRACK)
    {
        return Some(true);
    }
    None
}

fn no_blank(prev: &Atom, next: &Atom) -> bool {
    let opens = |a: &Atom| matches!(a.kind, L_PAREN | L_BRACK | ATTR_OPEN | L_CURLY);
    let closes = |a: &Atom| matches!(a.kind, R_PAREN | R_BRACK | R_CURLY);
    opens(prev) || (closes(next) && !is_comment(prev.kind))
}

fn same_line(prev: &Atom, next: &Atom, source: &str, gap: &str) -> String {
    match between(prev, next, source) {
        Space::None => String::new(),
        Space::One => " ".to_owned(),
        Space::Keep => {
            if gap.is_empty() {
                String::new()
            } else {
                " ".to_owned()
            }
        }
        Space::Aligned => {
            if gap.len() > 1 {
                gap.replace('\t', "    ")
            } else {
                " ".to_owned()
            }
        }
        Space::Verbatim => gap.to_owned(),
    }
}
