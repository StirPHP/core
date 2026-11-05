//! Document outline, workspace symbols, folding and selection ranges.

use stir_db::{Db, FileOrigin, SourceFile, Workspace};
use stir_hir::{ClassKind, FileItems, file_items};
use stir_syntax::{SyntaxKind::*, TextRange, TextSize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Function,
    Class,
    Interface,
    Enum,
    Method,
    Property,
    Constant,
    EnumMember,
}

#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    pub detail: Option<String>,
    pub kind: SymbolKind,
    pub full_range: TextRange,
    pub focus_range: TextRange,
    pub deprecated: bool,
    pub children: Vec<Symbol>,
}

fn leaf(name: &str, kind: SymbolKind, full: TextRange, focus: TextRange) -> Symbol {
    Symbol {
        name: name.to_owned(),
        detail: None,
        kind,
        full_range: full.cover(focus),
        focus_range: focus,
        deprecated: false,
        children: Vec::new(),
    }
}

fn class_kind(kind: ClassKind) -> SymbolKind {
    match kind {
        ClassKind::Class | ClassKind::Trait => SymbolKind::Class,
        ClassKind::Interface => SymbolKind::Interface,
        ClassKind::Enum => SymbolKind::Enum,
    }
}

/// The outline of a file.
pub fn document_symbols(db: &dyn Db, file: SourceFile) -> Vec<Symbol> {
    let items: &FileItems = file_items(db, file);
    let root = stir_db::parse(db, file).syntax();
    let name_in = |range: TextRange, name: &str| {
        crate::nav::name_token(&root, range, name)
            .map(|t| t.text_range())
            .unwrap_or(range)
    };
    let mut out = Vec::new();
    for f in &items.functions {
        let mut symbol = leaf(
            f.name.short(),
            SymbolKind::Function,
            f.ptr.text_range(),
            f.name_range,
        );
        symbol.deprecated = f.deprecated.is_some();
        out.push(symbol);
    }
    for c in &items.consts {
        let mut symbol = leaf(
            c.name.short(),
            SymbolKind::Constant,
            c.range,
            name_in(c.range, c.name.short()),
        );
        symbol.deprecated = c.deprecated.is_some();
        out.push(symbol);
    }

    for c in items.classes.iter().filter(|c| !c.is_anonymous()) {
        let mut s = leaf(
            c.name.short(),
            class_kind(c.kind),
            c.ptr.text_range(),
            c.name_range,
        );
        s.deprecated = c.deprecated.is_some();
        let child = |name: &str, kind, full, focus, deprecated: &Option<smol_str::SmolStr>| {
            let mut symbol = leaf(name, kind, full, focus);
            symbol.deprecated = deprecated.is_some();
            symbol
        };

        for p in c.properties.iter().filter(|p| !p.magic) {
            s.children.push(child(
                &p.name,
                SymbolKind::Property,
                p.range,
                name_in(p.range, &p.name),
                &p.deprecated,
            ));
        }

        for k in &c.consts {
            s.children.push(child(
                &k.name,
                SymbolKind::Constant,
                k.range,
                name_in(k.range, &k.name),
                &k.deprecated,
            ));
        }
        for k in &c.cases {
            s.children.push(child(
                &k.name,
                SymbolKind::EnumMember,
                k.range,
                name_in(k.range, &k.name),
                &k.deprecated,
            ));
        }
        for m in c.methods.iter().filter(|m| !m.magic) {
            s.children.push(child(
                &m.name,
                SymbolKind::Method,
                m.ptr.text_range(),
                m.name_range,
                &m.deprecated,
            ));
        }
        s.children.sort_by_key(|c| c.full_range.start());
        out.push(s);
    }
    out.sort_by_key(|s| s.full_range.start());
    out
}

/// A workspace symbol: a top-level declaration matching a query.
#[derive(Debug, Clone)]
pub struct WorkspaceSymbol {
    pub name: String,
    pub container: String,
    pub kind: SymbolKind,
    pub file: SourceFile,
    pub range: TextRange,
    pub deprecated: bool,
}

/// Declarations of the project and its dependencies whose
/// name fuzzily matches `query`.
pub fn workspace_symbols(db: &dyn Db, ws: Workspace, query: &str) -> Vec<WorkspaceSymbol> {
    let q = query.to_ascii_lowercase();
    let matches = |name: &str| {
        let mut chars = name.chars().map(|c| c.to_ascii_lowercase());
        q.chars().all(|p| chars.any(|c| c == p))
    };
    let mut out = Vec::new();
    for &file in ws.files(db) {
        if file.origin(db) == FileOrigin::Stub {
            continue;
        }
        let items = file_items(db, file);
        let mut push = |name: &stir_hir::name::Fqn, kind, range, deprecated: bool| {
            if matches(name.short()) {
                out.push(WorkspaceSymbol {
                    name: name.short().to_owned(),
                    container: name.namespace().to_owned(),
                    kind,
                    file,
                    range,
                    deprecated,
                });
            }
        };
        for f in &items.functions {
            push(
                &f.name,
                SymbolKind::Function,
                f.name_range,
                f.deprecated.is_some(),
            );
        }
        for c in items.classes.iter().filter(|c| !c.is_anonymous()) {
            push(
                &c.name,
                class_kind(c.kind),
                c.name_range,
                c.deprecated.is_some(),
            );
        }

        for c in &items.consts {
            push(
                &c.name,
                SymbolKind::Constant,
                c.range,
                c.deprecated.is_some(),
            );
        }
    }
    // project symbols first, then shorter names
    out.sort_by_key(|s| (s.file.origin(db) != FileOrigin::Source, s.name.len()));
    out.truncate(256);
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoldKind {
    Region,
    Comment,
    Imports,
}

/// Foldable regions: bodies, multi-line literals and comments, `use` blocks.
pub fn folding_ranges(db: &dyn Db, file: SourceFile) -> Vec<(TextRange, FoldKind)> {
    let root = stir_db::parse(db, file).syntax();
    let text = file.text(db);
    let multiline =
        |r: TextRange| text[usize::from(r.start())..usize::from(r.end())].contains('\n');
    let mut out = Vec::new();
    for node in root.descendants() {
        let foldable = matches!(
            node.kind(),
            BLOCK | CLASS_BODY | ARRAY_EXPR | ARG_LIST | PARAM_LIST | MATCH_EXPR | HEREDOC
        );
        if foldable && multiline(node.text_range()) {
            out.push((node.text_range(), FoldKind::Region));
        }
    }
    let mut uses: Option<TextRange> = None;
    for child in root.children() {
        if child.kind() == USE_DECL {
            uses = Some(uses.map_or(child.text_range(), |u| u.cover(child.text_range())));
        } else if let Some(u) = uses.take()
            && multiline(u)
        {
            out.push((u, FoldKind::Imports));
        }
    }
    if let Some(u) = uses
        && multiline(u)
    {
        out.push((u, FoldKind::Imports));
    }
    for token in root
        .descendants_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| matches!(t.kind(), COMMENT | DOC_COMMENT))
    {
        if multiline(token.text_range()) {
            out.push((token.text_range(), FoldKind::Comment));
        }
    }
    out
}

/// Ranges around `offset`, innermost first (for expand-selection).
pub fn selection_ranges(db: &dyn Db, file: SourceFile, offset: TextSize) -> Vec<TextRange> {
    let root = stir_db::parse(db, file).syntax();
    let Some(token) = root.token_at_offset(offset).right_biased() else {
        return Vec::new();
    };
    let mut out = vec![token.text_range()];
    for node in token.parent_ancestors() {
        let r = node.text_range();
        if out.last() != Some(&r) {
            out.push(r);
        }
    }
    out
}
