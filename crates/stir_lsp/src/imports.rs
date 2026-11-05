//! How to refer to a name from some place in a file: as it is, through a `use` line that's there,
//! or by adding one in the right place. A name that can't be imported without changing what the
//! file means is written in full.

use smol_str::SmolStr;
use stir_db::{Db, SourceFile, Workspace};
use stir_hir::file_items;
use stir_hir::name::{Fqn, Scope, UseKind};
use stir_syntax::{SyntaxKind::*, SyntaxNode, TextRange, TextSize};

/// A `use` line to add.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportEdit {
    pub at: TextSize,
    pub text: String,
}

/// What to write for a name, and the import that takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameRef {
    pub text: String,
    pub import: Option<ImportEdit>,
}

/// One `use` declaration of the part of the file a name is wanted in.
struct UseDecl {
    kind: UseKind,
    /// What it's sorted by.
    key: String,
    range: TextRange,
}

/// What's needed to tell how to refer to names at one place in a file.
pub struct Imports<'a> {
    text: &'a str,
    /// All of the imports of the namespace the place is in, those below it too.
    scope: Scope,
    /// Lowercase short names of the classes the namespace declares in this file.
    declared_classes: Vec<SmolStr>,
    declared_functions: Vec<SmolStr>,
    declared_consts: Vec<SmolStr>,
    uses: Vec<UseDecl>,
    /// Where the first import goes when there are none: after this, on the following line.
    anchor: Option<TextSize>,
    /// Inside `namespace X { ... }`: the indentation of what's in there.
    indent: String,
    braced: bool,
    newline: &'static str,
    /// Are global classes written `\Foo` here, rather than imported?
    global_by_backslash: bool,
}

/// PER-CS's order of the kinds of imports.
fn kind_order(kind: UseKind) -> u8 {
    match kind {
        UseKind::Class => 0,
        UseKind::Function => 1,
        UseKind::Const => 2,
    }
}

fn use_kind(node: &SyntaxNode) -> UseKind {
    let has = |kind| node.children_with_tokens().any(|e| e.kind() == kind);
    if has(FUNCTION_KW) {
        UseKind::Function
    } else if has(CONST_KW) {
        UseKind::Const
    } else {
        UseKind::Class
    }
}

fn sort_key(name: &str) -> String {
    // a shorter path sorts before what's below it
    name.trim_start_matches('\\')
        .to_ascii_lowercase()
        .replace('\\', "\u{1}")
}

impl<'a> Imports<'a> {
    /// `None` where there's no PHP to add to.
    pub fn new(db: &'a dyn Db, file: SourceFile, offset: TextSize) -> Option<Imports<'a>> {
        let text = file.text(db).as_str();
        let root = stir_db::parse(db, file).syntax();
        let open = root
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| t.kind() == OPEN_TAG)?;

        // the namespace around `offset`: a block, or the statements after `namespace X;`
        let mut parent = root.clone();
        let mut namespace_stmt = None;
        let mut end = root.text_range().end();
        let mut braced = false;
        let namespaces: Vec<SyntaxNode> = root
            .children()
            .filter(|c| c.kind() == NAMESPACE_DECL)
            .collect();
        for ns in &namespaces {
            let block = ns.children().find(|c| c.kind() == BLOCK);
            match block {
                Some(block) if ns.text_range().contains_inclusive(offset) => {
                    end = block.text_range().end() - TextSize::of('}');
                    parent = block;
                    namespace_stmt = Some(ns.clone());
                    braced = true;
                }
                None if ns.text_range().start() <= offset => {
                    namespace_stmt = Some(ns.clone());
                    end = root.text_range().end();
                }
                None => {
                    end = end.min(ns.text_range().start());
                    break;
                }
                Some(_) => {}
            }
        }
        let start = match (&namespace_stmt, braced) {
            (Some(ns), false) => ns.text_range().end(),
            _ => parent.text_range().start(),
        };

        let mut uses = Vec::new();
        for decl in parent.children().filter(|c| c.kind() == USE_DECL) {
            let range = decl.text_range();
            if range.start() < start || range.end() > end {
                continue;
            }
            let first = decl.children().find(|c| c.kind() == USE_CLAUSE);
            let name = first
                .as_ref()
                .and_then(|c| c.children().find(|n| n.kind() == NAME_REF))
                .map(|n| n.text().to_string())
                .unwrap_or_default();
            uses.push(UseDecl {
                kind: use_kind(&decl),
                key: sort_key(name.trim()),
                range,
            });
        }

        let items = file_items(db, file);
        let scope = items
            .scope_at(end.max(1.into()) - TextSize::from(1))
            .as_ref()
            .clone();
        let in_region = |r: TextRange| r.start() >= start && r.end() <= end;
        let declared_classes = items
            .classes
            .iter()
            .filter(|c| !c.is_anonymous() && in_region(c.name_range))
            .map(|c| SmolStr::new(c.name.short().to_ascii_lowercase()))
            .collect();
        let declared_functions = items
            .functions
            .iter()
            .filter(|f| in_region(f.name_range))
            .map(|f| SmolStr::new(f.name.short().to_ascii_lowercase()))
            .collect();
        let declared_consts = items
            .consts
            .iter()
            .filter(|c| in_region(c.range))
            .map(|c| SmolStr::new(c.name.short()))
            .collect();

        let anchor = match (&namespace_stmt, braced) {
            (Some(_), true) => parent
                .children_with_tokens()
                .find(|e| e.kind() == L_CURLY)
                .map(|e| e.text_range().end()),
            (Some(ns), false) => Some(ns.text_range().end()),
            (None, _) => Some(global_anchor(&root, &open, text)),
        };
        let indent = if braced {
            let first = parent.children().next();
            first
                .map(|s| line_indent(text, s.text_range().start()))
                .filter(|i| !i.is_empty())
                .unwrap_or_else(|| "    ".to_owned())
        } else {
            uses.first()
                .map(|u| line_indent(text, u.range.start()))
                .unwrap_or_default()
        };
        let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
        let global_by_backslash = prefers_backslash(&root, &scope);
        Some(Imports {
            text,
            scope,
            declared_classes,
            declared_functions,
            declared_consts,
            uses,
            anchor,
            indent,
            braced,
            newline,
            global_by_backslash,
        })
    }

    /// How to refer to the class `fqn`.
    pub fn class(&self, db: &dyn Db, ws: Workspace, fqn: &Fqn) -> NameRef {
        let lower = fqn.key();
        let short = fqn.short();
        let short_lower = short.to_ascii_lowercase();
        if let Some((alias, _)) = self
            .scope
            .classes
            .iter()
            .filter(|(_, target)| target.key() == lower)
            .min_by_key(|(alias, _)| alias.len())
        {
            return plain(self.declared_alias(alias, short));
        }
        let index = stir_hir::def_index(db, ws);
        let taken = self.scope.classes.contains_key(short_lower.as_str());
        let here = same_namespace(fqn, &self.scope.namespace);
        if here {
            return if taken {
                qualified(fqn)
            } else {
                plain(short.to_owned())
            };
        }
        if fqn.namespace().is_empty() && self.global_by_backslash {
            return qualified(fqn);
        }
        let sibling = Fqn::join(&self.scope.namespace, short).key();
        if taken
            || self.declared_classes.iter().any(|c| c == &short_lower)
            || index.classes.contains_key(&sibling)
        {
            return qualified(fqn);
        }
        self.import(UseKind::Class, fqn, short)
    }

    /// How to refer to the function `fqn`.
    pub fn function(&self, db: &dyn Db, ws: Workspace, fqn: &Fqn) -> NameRef {
        let lower = fqn.key();
        let short = fqn.short();
        let short_lower = short.to_ascii_lowercase();
        if let Some((alias, _)) = self
            .scope
            .functions
            .iter()
            .filter(|(_, target)| target.key() == lower)
            .min_by_key(|(alias, _)| alias.len())
        {
            return plain(self.declared_alias(alias, short));
        }
        let index = stir_hir::def_index(db, ws);
        let sibling = Fqn::join(&self.scope.namespace, short).key();
        let taken = self.scope.functions.contains_key(short_lower.as_str());
        let shadowed = taken || index.functions.contains_key(&sibling);
        if same_namespace(fqn, &self.scope.namespace) {
            return if taken {
                qualified(fqn)
            } else {
                plain(short.to_owned())
            };
        }
        // an unqualified call falls back to the global function
        if fqn.namespace().is_empty() {
            return if shadowed {
                qualified(fqn)
            } else {
                plain(short.to_owned())
            };
        }
        // importing over a global function would change what the file's calls of it mean
        let global = index.functions.contains_key(short_lower.as_str());
        if shadowed || global || self.declared_functions.iter().any(|f| f == &short_lower) {
            return qualified(fqn);
        }
        self.import(UseKind::Function, fqn, short)
    }

    /// How to refer to the constant `fqn`.
    pub fn constant(&self, db: &dyn Db, ws: Workspace, fqn: &Fqn) -> NameRef {
        let short = fqn.short();
        if let Some((alias, _)) = self
            .scope
            .consts
            .iter()
            .filter(|(_, target)| target.as_str() == fqn.as_str())
            .min_by_key(|(alias, _)| alias.len())
        {
            return plain(alias.to_string());
        }
        let index = stir_hir::def_index(db, ws);
        let sibling = Fqn::join(&self.scope.namespace, short);
        let taken = self.scope.consts.contains_key(short);
        let shadowed = taken || index.consts.contains_key(sibling.as_str());
        if same_namespace(fqn, &self.scope.namespace) {
            return if taken {
                qualified(fqn)
            } else {
                plain(short.to_owned())
            };
        }
        if fqn.namespace().is_empty() {
            return if shadowed {
                qualified(fqn)
            } else {
                plain(short.to_owned())
            };
        }
        let global = index.consts.contains_key(short);
        if shadowed || global || self.declared_consts.iter().any(|c| c == short) {
            return qualified(fqn);
        }
        self.import(UseKind::Const, fqn, short)
    }

    /// An alias is known in lowercase: the name it's imported under if that's the class's own.
    fn declared_alias(&self, alias: &str, short: &str) -> String {
        if alias.eq_ignore_ascii_case(short) {
            short.to_owned()
        } else {
            self.spelled_alias(alias)
        }
    }

    /// `alias`, as the file spells it.
    fn spelled_alias(&self, alias: &str) -> String {
        let word = |at: usize| {
            let rest = &self.text[at..];
            let len = rest
                .find(|c: char| !(c.is_alphanumeric() || c == '_'))
                .unwrap_or(rest.len());
            &rest[..len]
        };
        for use_decl in &self.uses {
            let decl = &self.text[use_decl.range];
            let lower = decl.to_ascii_lowercase();
            let Some(at) = lower.find(&format!(" as {alias}")) else {
                continue;
            };
            let found = usize::from(use_decl.range.start()) + at + " as ".len();
            return word(found).to_owned();
        }
        alias.to_owned()
    }

    fn import(&self, kind: UseKind, fqn: &Fqn, short: &str) -> NameRef {
        let Some(edit) = self.edit(kind, fqn) else {
            return qualified(fqn);
        };
        NameRef {
            text: short.to_owned(),
            import: Some(edit),
        }
    }

    fn edit(&self, kind: UseKind, fqn: &Fqn) -> Option<ImportEdit> {
        let keyword = match kind {
            UseKind::Class => "",
            UseKind::Function => "function ",
            UseKind::Const => "const ",
        };
        let nl = self.newline;
        let indent = &self.indent;
        let line = format!("{indent}use {keyword}{};", fqn.as_str());
        let same: Vec<&UseDecl> = self.uses.iter().filter(|u| u.kind == kind).collect();
        if let (Some(first), Some(last)) = (same.first(), same.last()) {
            let key = sort_key(fqn.as_str());
            let sorted = same.windows(2).all(|w| w[0].key <= w[1].key);
            if sorted && let Some(next) = same.iter().find(|u| u.key > key) {
                return Some(ImportEdit {
                    at: line_start(self.text, next.range.start()),
                    text: format!("{line}{nl}"),
                });
            }
            let _ = first;
            return Some(ImportEdit {
                at: line_end(self.text, last.range.end()),
                text: format!("{nl}{line}"),
            });
        }
        // the kinds are kept in order, with a blank line between them
        let order = kind_order(kind);
        let before = self.uses.iter().rfind(|u| kind_order(u.kind) < order);
        if let Some(before) = before {
            return Some(ImportEdit {
                at: line_end(self.text, before.range.end()),
                text: format!("{nl}{nl}{line}"),
            });
        }
        if let Some(after) = self.uses.iter().find(|u| kind_order(u.kind) > order) {
            return Some(ImportEdit {
                at: line_start(self.text, after.range.start()),
                text: format!("{line}{nl}{nl}"),
            });
        }
        let at = line_end(self.text, self.anchor?);
        if self.braced {
            return Some(ImportEdit {
                at,
                text: format!("{nl}{line}{nl}"),
            });
        }
        let after = self.text[usize::from(at)..].trim_start_matches([' ', '\t']);
        let blank_follows =
            after.is_empty() || after.starts_with("\n\n") || after.starts_with("\r\n\r\n");
        let trailer = if blank_follows { "" } else { nl };
        // straight below `<?php`, not a line further than it needs
        let open_tag = self.text[..usize::from(at)].ends_with('\n');
        let lead = if open_tag { "" } else { nl };
        Some(ImportEdit {
            at,
            text: format!("{lead}{nl}{line}{trailer}"),
        })
    }
}

fn plain(text: String) -> NameRef {
    NameRef { text, import: None }
}

fn qualified(fqn: &Fqn) -> NameRef {
    plain(format!("\\{}", fqn.as_str()))
}

fn same_namespace(fqn: &Fqn, namespace: &str) -> bool {
    fqn.namespace().eq_ignore_ascii_case(namespace)
}

fn line_start(text: &str, at: TextSize) -> TextSize {
    let at = usize::from(at);
    let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    TextSize::from(start as u32)
}

/// The end of the line of `at`, before its line break.
fn line_end(text: &str, at: TextSize) -> TextSize {
    let at = usize::from(at);
    // the opening tag has the line break in it
    if at > 0 && text.as_bytes()[at - 1] == b'\n' {
        return TextSize::from(at as u32);
    }
    let rest = &text[at..];
    let end = rest.find('\n').map_or(rest.len(), |i| i);
    let end = if rest[..end].ends_with('\r') {
        end - 1
    } else {
        end
    };
    TextSize::from((at + end) as u32)
}

fn line_indent(text: &str, at: TextSize) -> String {
    let start = usize::from(line_start(text, at));
    text[start..usize::from(at)]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect()
}

/// Where imports go in a file without a namespace: below the opening tag, a `declare`, and any
/// comment that's set off from the code by a blank line, which is a file's header.
fn global_anchor(root: &SyntaxNode, open: &stir_syntax::SyntaxToken, text: &str) -> TextSize {
    let mut anchor = open.text_range().end();
    for element in root.children_with_tokens() {
        let range = element.text_range();
        if range.end() <= anchor {
            continue;
        }
        match element.kind() {
            WHITESPACE => {}
            DECLARE_STMT => anchor = range.end(),
            COMMENT | DOC_COMMENT => {
                let after = &text[usize::from(range.end())..];
                let spaced = after.trim_start_matches([' ', '\t']);
                if spaced.starts_with("\n\n") || spaced.starts_with("\r\n\r\n") {
                    anchor = range.end();
                } else {
                    break;
                }
            }
            _ => break,
        }
    }
    anchor
}

/// Does the file write global classes as `\Foo` more than it imports them?
fn prefers_backslash(root: &SyntaxNode, scope: &Scope) -> bool {
    if scope.namespace.is_empty() {
        return false;
    }
    let imported = scope
        .classes
        .values()
        .filter(|t| t.namespace().is_empty())
        .count();
    let written = root
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| {
            t.kind() == NAME_FULLY_QUALIFIED
                && t.text().matches('\\').count() == 1
                && t.parent().is_some_and(|n| match n.kind() {
                    NAME_REF => true,
                    NAME_EXPR => n.parent().is_some_and(|p| {
                        matches!(
                            p.kind(),
                            CLASS_CONST_FETCH_EXPR
                                | STATIC_PROPERTY_FETCH_EXPR
                                | NEW_EXPR
                                | INSTANCEOF_EXPR
                        )
                    }),
                    _ => false,
                })
        })
        .count();
    written > imported
}
