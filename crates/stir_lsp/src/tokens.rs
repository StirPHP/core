//! Semantic highlighting: lexical classes from the syntax tree, refined by name
//! resolution (immutable bindings are `readonly`, mutable ones are not).

use stir_checker::{Def, LocalKind, MemberKind, typed_file};
use stir_db::{Db, FileOrigin, SourceFile, Workspace};
use stir_hir::{Binding, ClassKind, file_items};
use stir_syntax::{SyntaxKind, SyntaxKind::*, TextRange};

use crate::nav;

/// Token types, in legend order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum TokenType {
    Namespace,
    Type,
    Class,
    Enum,
    Interface,
    Struct,
    TypeParameter,
    Parameter,
    Variable,
    Property,
    EnumMember,
    Function,
    Method,
    Keyword,
    Comment,
    String,
    Number,
    Operator,
}

pub const TOKEN_TYPES: &[&str] = &[
    "namespace",
    "type",
    "class",
    "enum",
    "interface",
    "struct",
    "typeParameter",
    "parameter",
    "variable",
    "property",
    "enumMember",
    "function",
    "method",
    "keyword",
    "comment",
    "string",
    "number",
    "operator",
];

/// Token modifiers, as bits in legend order.
pub mod modifier {
    pub const DECLARATION: u32 = 1 << 0;
    pub const READONLY: u32 = 1 << 1;
    pub const STATIC: u32 = 1 << 2;
    pub const DEFAULT_LIBRARY: u32 = 1 << 3;
    pub const DOCUMENTATION: u32 = 1 << 4;
    /// A mutable binding (`var $x`, `mut x`).
    pub const MUTABLE: u32 = 1 << 5;
}

pub const TOKEN_MODIFIERS: &[&str] = &[
    "declaration",
    "readonly",
    "static",
    "defaultLibrary",
    "documentation",
    "mutable",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub range: TextRange,
    pub ty: TokenType,
    pub modifiers: u32,
}

fn lexical(kind: SyntaxKind, parent: Option<SyntaxKind>) -> Option<(TokenType, u32)> {
    if kind.is_keyword() {
        return Some((TokenType::Keyword, 0));
    }
    Some(match kind {
        COMMENT => (TokenType::Comment, 0),
        DOC_COMMENT => (TokenType::Comment, modifier::DOCUMENTATION),
        INT_NUMBER | FLOAT_NUMBER => (TokenType::Number, 0),
        STRING_SINGLE | STRING_DOUBLE | DQUOTE | BACKTICK | STRING_CONTENT | HEREDOC_START
        | HEREDOC_END => (TokenType::String, 0),
        OPEN_TAG | OPEN_TAG_WITH_ECHO | CLOSE_TAG => (TokenType::Keyword, 0),
        // contextual keywords (`val`, `record`, `is`, `when`, ...) are identifiers
        // sitting directly in their construct, not in a name
        IDENT if !matches!(parent, Some(NAME | NAME_REF)) => (TokenType::Keyword, 0),
        _ => return None,
    })
}

fn class_type(db: &dyn Db, ws: Workspace, key: &str) -> (TokenType, u32) {
    let Some(loc) = nav::class_loc(db, ws, key) else {
        return (TokenType::Class, 0);
    };
    let c = &file_items(db, loc.file).classes[loc.index as usize];
    let ty = match c.kind {
        ClassKind::Class | ClassKind::Trait => TokenType::Class,
        ClassKind::Interface => TokenType::Interface,
        ClassKind::Enum => TokenType::Enum,
        ClassKind::Record => TokenType::Struct,
    };
    let lib = if loc.file.origin(db) == FileOrigin::Source {
        0
    } else {
        modifier::DEFAULT_LIBRARY
    };
    (ty, lib)
}

fn resolved(
    db: &dyn Db,
    ws: Workspace,
    file: SourceFile,
    def: &Def,
) -> Option<(TokenType, u32)> {
    let lib = |f: SourceFile| {
        if f.origin(db) == FileOrigin::Source {
            0
        } else {
            modifier::DEFAULT_LIBRARY
        }
    };
    Some(match def {
        Def::Local { decl } => {
            let (_, kind) = typed_file(db, ws, file).locals.get(decl)?.clone();
            match kind {
                LocalKind::Val => (TokenType::Variable, modifier::READONLY),
                LocalKind::Param(Binding::Val) => (TokenType::Parameter, modifier::READONLY),
                LocalKind::Param(_) => (TokenType::Parameter, modifier::MUTABLE),
                LocalKind::This => (TokenType::Variable, modifier::READONLY),
                LocalKind::Var | LocalKind::Php | LocalKind::Loop => {
                    (TokenType::Variable, modifier::MUTABLE)
                }
            }
        }
        Def::Function(loc) => (TokenType::Function, lib(loc.file)),
        Def::Const(loc) => (TokenType::Variable, modifier::READONLY | lib(loc.file)),
        Def::TypeAlias(loc) => (TokenType::Type, lib(loc.file)),
        Def::Class(key) => class_type(db, ws, key),
        Def::Member { class, kind, name } => {
            let lib = nav::class_loc(db, ws, class).map_or(0, |l| lib(l.file));
            match kind {
                MemberKind::Method => (TokenType::Method, lib),
                MemberKind::Property => {
                    let immutable = nav::class_loc(db, ws, class).is_some_and(|loc| {
                        let c = &file_items(db, loc.file).classes[loc.index as usize];
                        c.mods.is_data
                            || c.mods.is_readonly
                            || c.properties
                                .iter()
                                .find(|p| &p.name == name)
                                .map(|p| p.immutable || p.mods.is_readonly)
                                .or_else(|| {
                                    c.primary_ctor
                                        .iter()
                                        .flatten()
                                        .chain(c.methods.iter().flat_map(|m| m.sig.params.iter()))
                                        .find(|p| &p.name == name)
                                        .and_then(|p| p.promoted.as_ref())
                                        .map(|p| p.immutable || p.mods.is_readonly)
                                })
                                .unwrap_or(false)
                    });
                    let m = if immutable {
                        modifier::READONLY
                    } else {
                        modifier::MUTABLE
                    };
                    (TokenType::Property, m | lib)
                }
                MemberKind::Field => (TokenType::Property, modifier::MUTABLE),
                MemberKind::Const => (TokenType::Property, modifier::READONLY | modifier::STATIC | lib),
                MemberKind::Case => (TokenType::EnumMember, lib),
            }
        }
    })
}

/// Highlighting for the whole file, ordered by position, non-overlapping.
pub fn semantic_tokens(db: &dyn Db, ws: Workspace, file: SourceFile) -> Vec<Token> {
    let root = stir_db::parse(db, file).syntax();
    let refs = nav::file_refs(db, ws, file);
    let decls: rustc_hash::FxHashSet<TextRange> = {
        let items = file_items(db, file);
        let mut s: rustc_hash::FxHashSet<TextRange> = typed_file(db, ws, file).locals.keys().copied().collect();
        s.extend(items.functions.iter().map(|f| f.name_range));
        s.extend(items.classes.iter().map(|c| c.name_range));
        for c in &items.classes {
            s.extend(c.methods.iter().map(|m| m.name_range));
        }
        s
    };
    let mut names: rustc_hash::FxHashMap<TextRange, (TokenType, u32)> = Default::default();
    for (range, def) in refs.iter() {
        if let Some(mut t) = resolved(db, ws, file, def) {
            if decls.contains(range) {
                t.1 |= modifier::DECLARATION;
            }
            names.insert(*range, t);
        }
    }
    let mut out = Vec::new();
    for token in root.descendants_with_tokens().filter_map(|e| e.into_token()) {
        let range = token.text_range();
        if range.is_empty() {
            continue;
        }
        let parent = token.parent();
        // a resolved name: the token itself, or the name node holding it
        let named = names.get(&range).copied().or_else(|| {
            parent
                .iter()
                .flat_map(|p| p.ancestors().take(2))
                .find_map(|n| names.get(&n.text_range()).copied())
        });
        let class = match (token.kind(), named) {
            (IDENT | VARIABLE | NAME_QUALIFIED | NAME_FULLY_QUALIFIED | NAME_RELATIVE, Some(t)) => {
                Some(t)
            }
            (IDENT, None) if parent.as_ref().is_some_and(|p| p.kind() == NAME_REF)
                && parent
                    .as_ref()
                    .and_then(|p| p.parent())
                    .is_some_and(|g| g.kind() == NAMED_TYPE) =>
            {
                Some((TokenType::Type, modifier::DEFAULT_LIBRARY))
            }
            (VARIABLE, None) => Some((TokenType::Variable, modifier::MUTABLE)),
            (k, _) => lexical(k, parent.as_ref().map(|p| p.kind())),
        };
        if let Some((ty, modifiers)) = class {
            out.push(Token {
                range,
                ty,
                modifiers,
            });
        }
    }
    out
}
