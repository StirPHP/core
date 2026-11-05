//! `use` lines: what's unused, removing it, and putting them in order.
//!
//! An import counts as used if its name is anywhere it could mean that: as the start of any name
//! among the code's tokens, whatever that token turns out to be, or as a word in a doc comment or
//! a comment with a tag in it, which PHPDoc types are read from. That keeps some that aren't used,
//! and never removes one that is.

use std::sync::Arc;

use rustc_hash::FxHashSet;
use stir_checker::codes;
use stir_db::{Db, Diagnostic, Severity, SourceFile};
use stir_hir::ast::NodeExt;
use stir_hir::name::UseKind;
use stir_syntax::{SyntaxKind::*, SyntaxNode, TextRange, TextSize};

use super::{Change, Cx, FileEdit, Kind, Offer};
use crate::imports::{self, kind_order, sort_key, use_kind};

/// One name a `use` declaration imports.
struct Clause {
    /// The clause, without what separates it from the next.
    node: SyntaxNode,
    kind: UseKind,
    /// What it's known as here.
    alias: String,
    /// What it imports.
    full: String,
}

/// One `use` declaration.
struct UseStmt {
    node: SyntaxNode,
    /// From its keyword to its semicolon.
    core: TextRange,
    clauses: Vec<Clause>,
}

/// A part of the file that has its own imports: a namespace, or a whole file with none.
struct Region {
    range: TextRange,
    uses: Vec<UseStmt>,
}

fn alias_of(clause: &SyntaxNode, name: &str) -> String {
    match clause.child(NAME) {
        Some(alias) => alias.text().to_string().trim().to_owned(),
        None => name.rsplit('\\').next().unwrap_or(name).to_owned(),
    }
}

fn name_of(clause: &SyntaxNode) -> Option<String> {
    let name = clause.child(NAME_REF)?;
    Some(name.text().to_string().trim().to_owned())
}

fn clause_kind(clause: &SyntaxNode, outer: UseKind) -> UseKind {
    if clause.has_token(FUNCTION_KW) {
        UseKind::Function
    } else if clause.has_token(CONST_KW) {
        UseKind::Const
    } else {
        outer
    }
}

fn read_stmt(node: &SyntaxNode) -> Option<UseStmt> {
    let outer = use_kind(node);
    let mut clauses = Vec::new();
    for clause in node.children_of(USE_CLAUSE) {
        let prefix = name_of(&clause)?;
        match clause.child(USE_GROUP) {
            Some(group) => {
                for inner in group.children_of(USE_CLAUSE) {
                    let name = name_of(&inner)?;
                    clauses.push(Clause {
                        kind: clause_kind(&inner, clause_kind(&clause, outer)),
                        alias: alias_of(&inner, &name),
                        full: format!("{prefix}\\{name}"),
                        node: inner,
                    });
                }
            }
            None => clauses.push(Clause {
                kind: clause_kind(&clause, outer),
                alias: alias_of(&clause, &prefix),
                full: prefix,
                node: clause,
            }),
        }
    }
    let tokens = node.tokens();
    let start = tokens.first()?.text_range().start();
    let end = node.text_range().end();
    Some(UseStmt {
        node: node.clone(),
        core: TextRange::new(start, end),
        clauses,
    })
}

fn regions(root: &SyntaxNode) -> Vec<Region> {
    let mut out = Vec::new();
    let read = |parent: &SyntaxNode, range: TextRange, out: &mut Vec<Region>| {
        let uses: Vec<UseStmt> = parent
            .children()
            .filter(|n| n.kind() == USE_DECL && range.contains_range(n.text_range()))
            .filter_map(|n| read_stmt(&n))
            .collect();
        out.push(Region { range, uses });
    };
    let namespaces = root.children_of(NAMESPACE_DECL);
    let unbraced: Vec<&SyntaxNode> = namespaces.iter().filter(|n| n.child(BLOCK).is_none()).collect();
    for ns in &namespaces {
        if let Some(block) = ns.child(BLOCK) {
            read(&block, block.text_range(), &mut out);
        }
    }
    let end = root.text_range().end();
    if unbraced.is_empty() {
        // a file's global code, or what there is around braced namespaces, which is nothing
        read(root, root.text_range(), &mut out);
    } else {
        let start = unbraced[0].text_range().start();
        if start > TextSize::from(0) {
            read(root, TextRange::new(0.into(), start), &mut out);
        }
        for (i, ns) in unbraced.iter().enumerate() {
            let stop = unbraced.get(i + 1).map_or(end, |n| n.text_range().start());
            read(root, TextRange::new(ns.text_range().end(), stop), &mut out);
        }
    }
    out
}

/// Is `word` a word of `text`, as names go?
fn has_word(text: &str, word: &str) -> bool {
    let is_part = |c: char| c.is_alphanumeric() || c == '_' || !c.is_ascii();
    let mut rest = text;
    let mut offset = 0;
    while let Some(at) = rest.find(word) {
        let start = offset + at;
        let end = start + word.len();
        let before_ok = text[..start].chars().next_back().is_none_or(|c| !is_part(c));
        let after_ok = text[end..].chars().next().is_none_or(|c| !is_part(c));
        if before_ok && after_ok {
            return true;
        }
        offset = start + 1;
        match text.get(offset..) {
            Some(r) => rest = r,
            None => return false,
        }
    }
    false
}

/// What's used in `region`: the first part of each name in it, lowercase unless it's a constant,
/// and all of what comments might be saying.
struct Used {
    names: FxHashSet<String>,
    exact: FxHashSet<String>,
    comments: Vec<String>,
}

impl Used {
    fn read(root: &SyntaxNode, region: &Region) -> Used {
        let mut used = Used {
            names: FxHashSet::default(),
            exact: FxHashSet::default(),
            comments: Vec::new(),
        };
        let range = region.range;
        for element in root.descendants_with_tokens() {
            let Some(t) = element.into_token() else {
                continue;
            };
            if !range.contains_range(t.text_range()) {
                continue;
            }
            match t.kind() {
                DOC_COMMENT => used.comments.push(t.text().to_owned()),
                COMMENT if t.text().contains('@') => used.comments.push(t.text().to_owned()),
                COMMENT | WHITESPACE => {}
                _ => {
                    if t.parent_ancestors().any(|n| n.kind() == USE_DECL) {
                        continue;
                    }
                    if t.kind() == IDENT || t.kind() == NAME_QUALIFIED || t.kind().is_keyword() {
                        let first = t.text().split('\\').next().unwrap_or_default();
                        used.names.insert(first.to_ascii_lowercase());
                        used.exact.insert(first.to_owned());
                    }
                }
            }
        }
        used
    }

    fn uses(&self, clause: &Clause) -> bool {
        let in_code = match clause.kind {
            UseKind::Const => self.exact.contains(&clause.alias),
            _ => self.names.contains(&clause.alias.to_ascii_lowercase()),
        };
        // `@param Foo`, and for a class any casing PHP would take there
        in_code
            || self.comments.iter().any(|c| {
                has_word(c, &clause.alias)
                    || (clause.kind == UseKind::Class
                        && has_word(&c.to_ascii_lowercase(), &clause.alias.to_ascii_lowercase()))
            })
    }
}

/// The imports of `file` nothing uses.
#[salsa::tracked(returns(ref))]
fn unused_imports(db: &dyn Db, file: SourceFile) -> Arc<Vec<TextRange>> {
    let parse = stir_db::parse(db, file);
    if !parse.errors().is_empty() {
        return Arc::new(Vec::new());
    }
    let root = parse.syntax();
    let mut out = Vec::new();
    for region in regions(&root) {
        if region.uses.is_empty() {
            continue;
        }
        let used = Used::read(&root, &region);
        for stmt in &region.uses {
            for clause in &stmt.clauses {
                if !used.uses(clause) {
                    out.push(clause.node.text_range());
                }
            }
        }
    }
    Arc::new(out)
}

/// What an editor greys out: the imports nothing uses. These are hints, and not something
/// `stirphp check` reports, because PHP and PHPStan don't mind them.
pub fn unused_import_hints(db: &dyn Db, file: SourceFile) -> Vec<Diagnostic> {
    unused_imports(db, file)
        .iter()
        .map(|range| {
            Diagnostic::custom(
                file.path(db).clone(),
                trimmed(db, file, *range),
                Severity::Hint,
                codes::UNUSED_IMPORT,
                "This import isn't used.",
            )
        })
        .collect()
}

fn trimmed(db: &dyn Db, file: SourceFile, range: TextRange) -> TextRange {
    let text = &file.text(db)[usize::from(range.start())..usize::from(range.end())];
    let start = usize::from(range.start()) + (text.len() - text.trim_start().len());
    TextRange::at(
        TextSize::from(start as u32),
        TextSize::from(text.trim().len() as u32),
    )
}

/// The edits that leave `stmt` without the clauses in `gone`.
fn without_clauses(cx: &Cx<'_>, stmt: &UseStmt, gone: &FxHashSet<TextRange>, out: &mut Vec<FileEdit>) {
    let all = stmt.clauses.iter().all(|c| gone.contains(&c.node.text_range()));
    if all {
        out.push(cx.edit(whole_lines(cx, stmt.core), ""));
        return;
    }
    // a list of them, or a group: what's left keeps its form
    for c in &stmt.clauses {
        let range = c.node.text_range();
        if !gone.contains(&range) {
            continue;
        }
        let parent = c.node.parent().unwrap_or_else(|| stmt.node.clone());
        out.push(cx.edit(with_a_comma(cx, &parent, &c.node, gone), ""));
    }
}

/// The range of a clause with the comma that goes with it: the next, or the one before if it's
/// the last that stays.
fn with_a_comma(
    cx: &Cx<'_>,
    parent: &SyntaxNode,
    clause: &SyntaxNode,
    gone: &FxHashSet<TextRange>,
) -> TextRange {
    let core = trimmed(cx.db, cx.file, clause.text_range());
    let siblings: Vec<_> = parent
        .children_with_tokens()
        .filter(|e| !e.kind().is_trivia())
        .collect();
    let at = siblings
        .iter()
        .position(|e| e.as_node() == Some(clause))
        .unwrap_or(0);
    let next = siblings.get(at + 1);
    let later_stays = siblings[at + 1..].iter().any(|e| {
        e.kind() == USE_CLAUSE && !gone.contains(&e.text_range())
    });
    if let Some(comma) = next.filter(|e| e.kind() == COMMA)
        && later_stays
    {
        // up to what comes after the comma
        let after = comma.text_range().end();
        let rest = &cx.text[usize::from(after)..];
        let spaces = rest.len() - rest.trim_start_matches([' ', '\t']).len();
        return TextRange::new(core.start(), after + TextSize::from(spaces as u32));
    }
    if at > 0 && siblings[at - 1].kind() == COMMA {
        return TextRange::new(siblings[at - 1].text_range().start(), core.end());
    }
    core
}

/// `range`, and its line if nothing else is on it.
fn whole_lines(cx: &Cx<'_>, range: TextRange) -> TextRange {
    let start = imports::line_start(cx.text, range.start());
    let before = &cx.text[usize::from(start)..usize::from(range.start())];
    let end_of_line = imports::line_end(cx.text, range.end());
    let after = &cx.text[usize::from(range.end())..usize::from(end_of_line)];
    if !before.trim().is_empty() || !after.trim().is_empty() {
        return range;
    }
    let rest = &cx.text[usize::from(end_of_line)..];
    let ending = if rest.starts_with("\r\n") {
        2
    } else if rest.starts_with('\n') {
        1
    } else {
        0
    };
    TextRange::new(start, end_of_line + TextSize::from(ending))
}

/// Removing the import at the cursor, and all that are unused, if there's one to remove.
pub fn remove_offers<'c>(cx: &'c Cx<'c>, out: &mut Vec<Offer<'c>>) {
    let unused = unused_imports(cx.db, cx.file);
    if unused.is_empty() {
        return;
    }
    let at = cx.raw_range;
    let here: Vec<TextRange> = unused
        .iter()
        .copied()
        .filter(|r| {
            let r = trimmed(cx.db, cx.file, *r);
            r.start() <= at.end() && at.start() <= r.end()
        })
        .collect();
    let Some(&first) = here.first() else {
        return;
    };
    let name = cx.slice(trimmed(cx.db, cx.file, first)).to_owned();
    let hint = trimmed(cx.db, cx.file, first);
    out.push(
        Offer::new(
            format!("imports.remove:{}", u32::from(first.start())),
            format!("Remove unused import `{name}`"),
            Kind::QuickFix,
            move || {
                let gone: FxHashSet<TextRange> = [first].into_iter().collect();
                Ok(Change::new(removal_edits(cx, &gone)))
            },
        )
        .fixing(hint, codes::UNUSED_IMPORT)
        .preferred(),
    );
    if unused.len() > 1 {
        out.push(Offer::new(
            "imports.remove-all",
            "Remove all unused imports",
            Kind::QuickFix,
            move || {
                let gone: FxHashSet<TextRange> = unused.iter().copied().collect();
                Ok(Change::new(removal_edits(cx, &gone)))
            },
        ));
    }
}

fn removal_edits(cx: &Cx<'_>, gone: &FxHashSet<TextRange>) -> Vec<FileEdit> {
    let mut edits = Vec::new();
    for region in regions(&cx.root) {
        for stmt in &region.uses {
            if stmt.clauses.iter().any(|c| gone.contains(&c.node.text_range())) {
                without_clauses(cx, stmt, gone, &mut edits);
            }
        }
    }
    edits
}

/// A run of `use` declarations with nothing but comments between them, which can be reordered.
struct Run<'r> {
    stmts: Vec<&'r UseStmt>,
}

fn runs<'r>(cx: &Cx<'_>, region: &'r Region) -> Vec<Run<'r>> {
    let mut out: Vec<Run<'r>> = Vec::new();
    for stmt in &region.uses {
        let joins = out.last().and_then(|run| run.stmts.last()).is_some_and(|last| {
            let gap = TextRange::new(last.core.end(), stmt.core.start());
            only_trivia(cx, gap)
        });
        if joins {
            if let Some(run) = out.last_mut() {
                run.stmts.push(stmt);
            }
        } else {
            out.push(Run { stmts: vec![stmt] });
        }
    }
    out
}

fn only_trivia(cx: &Cx<'_>, range: TextRange) -> bool {
    if range.is_empty() {
        return true;
    }
    match cx.root.covering_element(range) {
        stir_syntax::SyntaxElement::Token(t) => t.kind().is_trivia(),
        stir_syntax::SyntaxElement::Node(n) => n
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .filter(|t| range.contains_range(t.text_range()))
            .all(|t| t.kind().is_trivia()),
    }
}

/// The line ending, at the end of `gap`'s first line, of what follows a statement, or nothing.
fn trailing_comment_end(gap: &str) -> usize {
    match gap.find('\n') {
        Some(at) => {
            if gap[..at].trim().is_empty() {
                0
            } else {
                gap[..at].trim_end().len()
            }
        }
        None => gap.trim_end().len(),
    }
}

/// `use` declarations of a run, each with what stays with it.
struct Piece {
    kind: UseKind,
    key: String,
    /// Comments above it, then it, then a comment on its line.
    text: String,
}

/// The edit of one run that puts it in order, without what `gone` says is unused.
fn organize_run(cx: &Cx<'_>, run: &Run<'_>, gone: &FxHashSet<TextRange>) -> Option<FileEdit> {
    let first = run.stmts.first()?;
    let last = run.stmts.last()?;
    let indent = super::style::indent_at(cx.text, first.core.start());
    if !super::style::starts_line(cx.text, first.core.start()) {
        return None;
    }
    let mut pieces = Vec::new();
    let mut lead = String::new();
    let mut end_of_run = last.core.end();
    for (i, stmt) in run.stmts.iter().enumerate() {
        let next_start = run.stmts.get(i + 1).map(|n| n.core.start());
        let after = stmt.core.end();
        let trailing_end = match next_start {
            Some(next) => {
                let gap = cx.slice(TextRange::new(after, next));
                trailing_comment_end(gap)
            }
            None => {
                let line_end = imports::line_end(cx.text, after);
                let rest = cx.slice(TextRange::new(after, line_end));
                let comment = rest.trim_start();
                if comment.starts_with("//") || comment.starts_with('#') || comment.starts_with("/*") {
                    end_of_run = line_end;
                    rest.trim_end().len()
                } else {
                    0
                }
            }
        };
        let tail_end = after + TextSize::from(trailing_end as u32);
        let live: Vec<&Clause> = stmt
            .clauses
            .iter()
            .filter(|c| !gone.contains(&c.node.text_range()))
            .collect();
        let body = if live.len() == stmt.clauses.len() {
            cx.slice(stmt.core).to_owned()
        } else if live.is_empty() {
            String::new()
        } else {
            rewritten(cx, stmt, gone)
        };
        let trail = cx.slice(TextRange::new(after, tail_end)).to_owned();
        if body.is_empty() {
            // a comment that was only about what's going stays with what's next
            lead = std::mem::take(&mut lead);
        } else {
            let first_live = live.first()?;
            let outer = use_kind(&stmt.node);
            pieces.push(Piece {
                kind: outer,
                key: sort_key(&first_live.full),
                text: format!("{lead}{body}{trail}"),
            });
            lead = String::new();
        }
        if let Some(next) = next_start {
            let gap_rest = cx.slice(TextRange::new(tail_end, next));
            for line in gap_rest.split_inclusive('\n') {
                let t = line.trim();
                if !t.is_empty() {
                    lead.push_str(t);
                    lead.push_str(cx.style.nl);
                    lead.push_str(&indent);
                }
            }
        }
    }
    pieces.sort_by(|a, b| {
        kind_order(a.kind)
            .cmp(&kind_order(b.kind))
            .then_with(|| a.key.cmp(&b.key))
    });
    let nl = cx.style.nl;
    let mut text = String::new();
    let mut previous: Option<UseKind> = None;
    for piece in &pieces {
        if let Some(p) = previous {
            text.push_str(nl);
            if p != piece.kind {
                text.push_str(nl);
            }
            text.push_str(&indent);
        }
        text.push_str(&piece.text);
        previous = Some(piece.kind);
    }
    if !lead.is_empty() {
        // what was above something that's gone, at the end of them all
        if !text.is_empty() {
            text.push_str(nl);
            text.push_str(&indent);
        }
        text.push_str(lead.trim_end());
    }
    let range = TextRange::new(first.core.start(), end_of_run);
    if text == cx.slice(range) {
        return None;
    }
    if text.is_empty() {
        // nothing left: the lines go, and one of the blank lines they leave
        let whole = whole_lines(cx, range);
        let rest = &cx.text[usize::from(whole.end())..];
        let blank = if rest.starts_with("\r\n") {
            2
        } else if rest.starts_with('\n') {
            1
        } else {
            0
        };
        return Some(cx.edit(
            TextRange::new(whole.start(), whole.end() + TextSize::from(blank)),
            "",
        ));
    }
    Some(cx.edit(range, text))
}

/// The text of `stmt` without clauses that are gone.
fn rewritten(cx: &Cx<'_>, stmt: &UseStmt, gone: &FxHashSet<TextRange>) -> String {
    let mut cuts: Vec<TextRange> = stmt
        .clauses
        .iter()
        .filter(|c| gone.contains(&c.node.text_range()))
        .map(|c| {
            let parent = c.node.parent().unwrap_or_else(|| stmt.node.clone());
            with_a_comma(cx, &parent, &c.node, gone)
        })
        .collect();
    cuts.sort_by_key(|r| r.start());
    let mut out = String::new();
    let mut at = stmt.core.start();
    for cut in cuts {
        if cut.start() < at {
            continue;
        }
        out.push_str(cx.slice(TextRange::new(at, cut.start())));
        at = cut.end();
    }
    out.push_str(cx.slice(TextRange::new(at, stmt.core.end())));
    out
}

/// `source.organizeImports`.
pub fn offers<'c>(cx: &'c Cx<'c>, out: &mut Vec<Offer<'c>>) {
    let regions_of = regions(&cx.root);
    let unused = unused_imports(cx.db, cx.file);
    let gone: FxHashSet<TextRange> = unused.iter().copied().collect();
    let mut edits = Vec::new();
    for region in &regions_of {
        for run in runs(cx, region) {
            edits.extend(organize_run(cx, &run, &gone));
        }
    }
    if edits.is_empty() {
        return;
    }
    out.push(Offer::new(
        "imports.organize",
        "Organize imports",
        Kind::OrganizeImports,
        move || Ok(Change::new(edits)),
    ));
}

/// `source.fixAll`: only what can't change what the code does.
pub fn fix_all_offers<'c>(cx: &'c Cx<'c>, out: &mut Vec<Offer<'c>>) {
    let unused = unused_imports(cx.db, cx.file);
    let gone: FxHashSet<TextRange> = unused.iter().copied().collect();
    let mut edits = removal_edits(cx, &gone);
    edits.extend(super::fix_types::nullable_everywhere(cx));
    if edits.is_empty() {
        return;
    }
    out.push(Offer::new(
        "fix-all",
        "Fix what's safe: unused imports and implicit nullable parameters",
        Kind::FixAll,
        move || Ok(Change::new(edits)),
    ));
}
