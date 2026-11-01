//! Code completion: members after `->`/`::`, and names in scope.

use rustc_hash::FxHashSet;
use smol_str::SmolStr;
use stir_checker::model_api::{self, MemberTy};
use stir_checker::{Ctx, Def, LocalKind, typed_file};
use stir_db::{Db, SourceFile, Workspace};
use stir_hir::{ClassKind, Visibility, file_items};
use stir_syntax::{Dialect, SyntaxKind::*, SyntaxNode, TextRange, TextSize};
use stir_types::{Ty, TyKind};

use crate::hover::local_label;
use crate::nav;
use crate::render;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    Local,
    Param,
    Function,
    Method,
    Property,
    Field,
    Const,
    EnumCase,
    Class,
    Interface,
    Enum,
    Struct,
    Keyword,
    Type,
}

#[derive(Debug, Clone)]
pub struct Item {
    pub label: String,
    pub kind: ItemKind,
    pub detail: Option<String>,
    /// Replaces [`Completions::replace`].
    pub insert: String,
    /// Lower sorts first.
    pub rank: u8,
}

#[derive(Debug, Default)]
pub struct Completions {
    pub items: Vec<Item>,
    /// The range the inserted text replaces (the word being typed).
    pub replace: TextRange,
    /// More results exist for a longer prefix.
    pub incomplete: bool,
}

const STIR_KEYWORDS: &[&str] = &[
    "val",
    "var",
    "function",
    "class",
    "data",
    "record",
    "interface",
    "sealed",
    "open",
    "abstract",
    "enum",
    "typealias",
    "when",
    "if",
    "else",
    "for",
    "foreach",
    "while",
    "do",
    "return",
    "throw",
    "try",
    "catch",
    "finally",
    "is",
    "as",
    "in",
    "out",
    "of",
    "where",
    "mut",
    "null",
    "true",
    "false",
    "this",
    "new",
    "namespace",
    "use",
    "break",
    "continue",
    "echo",
    "private",
    "protected",
    "public",
    "static",
    "override",
    "const",
];

const PHP_KEYWORDS: &[&str] = &[
    "function",
    "class",
    "interface",
    "trait",
    "enum",
    "abstract",
    "final",
    "readonly",
    "if",
    "else",
    "elseif",
    "for",
    "foreach",
    "while",
    "do",
    "switch",
    "case",
    "match",
    "return",
    "throw",
    "try",
    "catch",
    "finally",
    "instanceof",
    "new",
    "namespace",
    "use",
    "break",
    "continue",
    "echo",
    "private",
    "protected",
    "public",
    "static",
    "const",
    "null",
    "true",
    "false",
    "array",
    "fn",
    "yield",
    "isset",
    "unset",
    "empty",
];

const STIR_TYPES: &[&str] = &[
    "Int",
    "Float",
    "String",
    "Bool",
    "Unit",
    "Nothing",
    "List",
    "MutableList",
    "Map",
    "MutableMap",
];

fn ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || !c.is_ascii()
}

/// Case-insensitive subsequence match.
fn fuzzy(name: &str, prefix: &str) -> bool {
    let mut chars = name.chars().map(|c| c.to_ascii_lowercase());
    prefix
        .chars()
        .map(|c| c.to_ascii_lowercase())
        .all(|p| chars.any(|c| c == p))
}

pub fn complete(db: &dyn Db, ws: Workspace, file: SourceFile, offset: TextSize) -> Completions {
    let text = file.text(db);
    let off = usize::from(offset).min(text.len());
    let word_start = text[..off]
        .char_indices()
        .rev()
        .take_while(|(_, c)| ident_char(*c))
        .last()
        .map_or(off, |(i, _)| i);
    let dollar = text[..word_start].ends_with('$');
    let replace_start = if dollar { word_start - 1 } else { word_start };
    let prefix = &text[word_start..off];
    let mut out = Completions {
        replace: TextRange::new((replace_start as u32).into(), offset),
        ..Completions::default()
    };
    let before = &text[..replace_start];
    let ctx = Ctx::for_file(db, ws, file);
    let enclosing = enclosing_class(db, file, offset);
    if before.ends_with("->") {
        let arrow = TextSize::from(replace_start as u32 - 1);
        if let Some(recv) = receiver_type(db, ws, file, arrow) {
            member_items(&ctx, &recv, false, enclosing.as_deref(), &mut out.items);
        }
        return out;
    }
    if before.ends_with("::") {
        let colons = replace_start - 2;
        let class_name: String = {
            let start = text[..colons]
                .char_indices()
                .rev()
                .take_while(|(_, c)| ident_char(*c) || *c == '\\')
                .last()
                .map_or(colons, |(i, _)| i);
            text[start..colons].to_owned()
        };
        let at = TextSize::from(colons as u32);
        let def = match nav::def_at(db, ws, file, at) {
            Some((_, d @ Def::Class(_))) => Some(d),
            _ => nav::resolve_type_name(db, ws, file, at, &class_name, false),
        };
        if let Some(Def::Class(key)) = def
            && let Some(model) = ctx.class(&key)
        {
            member_items(
                &ctx,
                &model.self_ty(),
                true,
                enclosing.as_deref(),
                &mut out.items,
            );
            out.items.push(Item {
                label: "class".into(),
                kind: ItemKind::Keyword,
                detail: None,
                insert: "class".into(),
                rank: 5,
            });
        }
        return out;
    }
    let stir = file.dialect(db) == Dialect::Stir;
    local_items(db, ws, file, offset, dollar, &mut out.items);
    if dollar {
        return out;
    }
    // members of `this` are in scope without `$this->` in StirPHP methods
    if stir
        && let Some(key) = &enclosing
        && let Some(model) = ctx.class(key)
    {
        member_items(
            &ctx,
            &model.self_ty(),
            false,
            enclosing.as_deref(),
            &mut out.items,
        );
    }
    let keywords = if stir { STIR_KEYWORDS } else { PHP_KEYWORDS };
    for kw in keywords {
        out.items.push(Item {
            label: (*kw).into(),
            kind: ItemKind::Keyword,
            detail: None,
            insert: (*kw).into(),
            rank: 6,
        });
    }
    if stir {
        for t in STIR_TYPES {
            out.items.push(Item {
                label: (*t).into(),
                kind: ItemKind::Type,
                detail: None,
                insert: (*t).into(),
                rank: 3,
            });
        }
    }
    if prefix.is_empty() {
        // globals only once something is typed: the stubs are large
        out.incomplete = true;
        return out;
    }
    global_items(db, ws, file, offset, prefix, &mut out);
    out
}

/// The lowercase FQN of the class whose body contains `offset`.
fn enclosing_class(db: &dyn Db, file: SourceFile, offset: TextSize) -> Option<SmolStr> {
    file_items(db, file)
        .classes
        .iter()
        .filter(|c| c.ptr.text_range().contains(offset))
        .min_by_key(|c| c.ptr.text_range().len())
        .map(|c| c.name.key())
}

/// The type of the expression before the `->` at `arrow`.
fn receiver_type(db: &dyn Db, ws: Workspace, file: SourceFile, arrow: TextSize) -> Option<Ty> {
    let root = stir_db::parse(db, file).syntax();
    let token = root.token_at_offset(arrow).right_biased()?;
    let fetch = token.parent()?;
    let base = fetch.children().next()?;
    let typed = typed_file(db, ws, file);
    let mut ty = typed.types.get(&base.text_range()).cloned()?;
    ty = stir_checker::strip_platform(&stir_types::without_null(&ty));
    let ctx = Ctx::for_file(db, ws, file);
    for _ in 0..8 {
        match ty.kind() {
            TyKind::Param { id, .. } => match stir_types::TypeEnv::param_bound(&ctx, *id) {
                Some(b) => ty = b,
                None => break,
            },
            _ => break,
        }
    }
    Some(ty)
}

fn member_items(
    ctx: &Ctx<'_>,
    ty: &Ty,
    statics: bool,
    enclosing: Option<&str>,
    out: &mut Vec<Item>,
) {
    for m in model_api::members(ctx, ty) {
        let visible = match m.visibility {
            Visibility::Public => true,
            Visibility::Protected => enclosing.is_some(),
            Visibility::Private => enclosing == Some(m.class.as_str()),
        };
        let wants_static = matches!(m.ty, MemberTy::Const(_) | MemberTy::Case) || m.is_static;
        if !visible || wants_static != statics {
            continue;
        }
        let (kind, detail, insert) = match &m.ty {
            MemberTy::Method(sig) => (
                ItemKind::Method,
                render::signature(sig, "function").label,
                m.name.to_string(),
            ),
            MemberTy::Property { ty, immutable } => {
                let label = if *immutable {
                    format!("val {}: {ty}", m.name)
                } else {
                    format!("var ${}: {ty}", m.name)
                };
                let insert = if statics {
                    format!("${}", m.name)
                } else {
                    m.name.to_string()
                };
                (ItemKind::Property, label, insert)
            }
            MemberTy::Field(t) => (
                ItemKind::Field,
                format!("{}: {t}", m.name),
                m.name.to_string(),
            ),
            MemberTy::Const(t) => (
                ItemKind::Const,
                format!("const {}: {t}", m.name),
                m.name.to_string(),
            ),
            MemberTy::Case => (
                ItemKind::EnumCase,
                format!("case {}", m.name),
                m.name.to_string(),
            ),
        };
        out.push(Item {
            label: m.name.to_string(),
            kind,
            detail: Some(detail),
            insert,
            rank: 0,
        });
    }
    if statics {
        return;
    }
    let mut seen = FxHashSet::default();
    for loc in model_api::applicable_extensions(ctx, ty) {
        let sig = model_api::function_info(ctx, loc);
        let name = sig.name.rsplit('\\').next().unwrap_or(&sig.name).to_owned();
        if !seen.insert(name.clone()) {
            continue;
        }
        out.push(Item {
            label: name.clone(),
            kind: ItemKind::Method,
            detail: Some(render::signature(&sig, "function").label),
            insert: name,
            rank: 1,
        });
    }
}

/// Nodes that bound the visibility of the bindings declared in them.
fn scope_node(node: &SyntaxNode, php: bool) -> Option<SyntaxNode> {
    node.ancestors().find(|n| {
        matches!(
            n.kind(),
            FUNCTION_DECL | METHOD_DECL | CLOSURE_EXPR | ARROW_FN_EXPR | LAMBDA_EXPR
        ) || (!php
            && matches!(
                n.kind(),
                BLOCK | FOREACH_STMT | FOR_STMT | CATCH_CLAUSE | WHEN_EXPR
            ))
    })
}

fn local_items(
    db: &dyn Db,
    ws: Workspace,
    file: SourceFile,
    offset: TextSize,
    dollar: bool,
    out: &mut Vec<Item>,
) {
    let root = stir_db::parse(db, file).syntax();
    let php = file.dialect(db) == Dialect::Php;
    let Some(here) = root.token_at_offset(offset).left_biased() else {
        return;
    };
    let here_fn = here.parent().and_then(|n| nav::enclosing_function(&n));
    let typed = typed_file(db, ws, file);
    let mut locals: Vec<_> = typed
        .locals
        .iter()
        .filter(|(r, (_, k))| r.end() <= offset && *k != LocalKind::This)
        .collect();
    locals.sort_by_key(|(r, _)| std::cmp::Reverse(r.start()));
    let mut seen = FxHashSet::default();
    for (range, (ty, kind)) in locals {
        let Some(token) = root.covering_element(*range).into_token() else {
            continue;
        };
        let Some(node) = token.parent() else { continue };
        let decl_fn = nav::enclosing_function(&node);
        if decl_fn.as_ref().map(|n| n.text_range()) != here_fn.as_ref().map(|n| n.text_range())
            && !decl_fn
                .as_ref()
                .is_some_and(|f| f.kind() == ARROW_FN_EXPR && f.text_range().contains(offset))
        {
            continue;
        }
        if let Some(scope) = scope_node(&node, php)
            && !scope.text_range().contains(offset)
        {
            continue;
        }
        let name = token.text().trim_start_matches('$').to_owned();
        if !seen.insert(name.clone()) {
            continue;
        }
        let has_dollar = matches!(
            kind,
            LocalKind::Var
                | LocalKind::Php
                | LocalKind::Loop
                | LocalKind::Param(stir_hir::Binding::Php)
        );
        if dollar && !has_dollar {
            continue;
        }
        let label = if has_dollar {
            format!("${name}")
        } else {
            name.clone()
        };
        out.push(Item {
            insert: label.clone(),
            label,
            kind: if matches!(kind, LocalKind::Param(_)) {
                ItemKind::Param
            } else {
                ItemKind::Local
            },
            detail: Some(local_label(&name, *kind, ty)),
            rank: 0,
        });
    }
}

fn global_items(
    db: &dyn Db,
    ws: Workspace,
    file: SourceFile,
    offset: TextSize,
    prefix: &str,
    out: &mut Completions,
) {
    let index = stir_hir::def_index(db, ws);
    let scope = file_items(db, file).scope_at(offset).clone();
    let stir = file.dialect(db) == Dialect::Stir;
    let ns = scope.namespace.to_ascii_lowercase();
    let reachable = |fqn: &str| {
        let lower = fqn.to_ascii_lowercase();
        let parent = lower.rsplit_once('\\').map_or("", |(p, _)| p);
        parent.is_empty() || parent == ns || (stir && parent == "stir")
    };
    let ctx = Ctx::for_file(db, ws, file);
    let mut seen = FxHashSet::default();
    for locs in index.functions.values() {
        let Some(&loc) = locs.first() else { continue };
        let f = &file_items(db, loc.file).functions[loc.index as usize];
        let short = f.name.short();
        if !fuzzy(short, prefix) || !seen.insert((0, short.to_ascii_lowercase())) {
            continue;
        }
        let insert = if reachable(f.name.as_str()) {
            short.to_owned()
        } else {
            format!("\\{}", f.name.as_str())
        };
        let sig = model_api::function_info(&ctx, loc);
        out.items.push(Item {
            label: short.to_owned(),
            kind: ItemKind::Function,
            detail: Some(render::signature(&sig, "function").label),
            insert,
            rank: if loc.file.origin(db) == stir_db::FileOrigin::Source {
                2
            } else {
                4
            },
        });
    }
    for locs in index.classes.values() {
        let Some(&loc) = locs.first() else { continue };
        let c = &file_items(db, loc.file).classes[loc.index as usize];
        let short = c.name.short();
        if !fuzzy(short, prefix) {
            continue;
        }
        let resolved = scope.resolve_class(short).key();
        let insert = if resolved == c.name.key()
            || (stir && c.name.namespace().eq_ignore_ascii_case("stir"))
        {
            short.to_owned()
        } else {
            format!("\\{}", c.name.as_str())
        };
        out.items.push(Item {
            label: short.to_owned(),
            kind: match c.kind {
                ClassKind::Interface => ItemKind::Interface,
                ClassKind::Enum => ItemKind::Enum,
                ClassKind::Record => ItemKind::Struct,
                ClassKind::Class | ClassKind::Trait => ItemKind::Class,
            },
            detail: Some(c.name.as_str().to_owned()),
            insert,
            rank: if loc.file.origin(db) == stir_db::FileOrigin::Source {
                2
            } else {
                4
            },
        });
    }
    for locs in index.consts.values() {
        let Some(&loc) = locs.first() else { continue };
        let c = &file_items(db, loc.file).consts[loc.index as usize];
        let short = c.name.short();
        if !fuzzy(short, prefix) {
            continue;
        }
        out.items.push(Item {
            label: short.to_owned(),
            kind: ItemKind::Const,
            detail: Some(c.name.as_str().to_owned()),
            insert: if reachable(c.name.as_str()) {
                short.to_owned()
            } else {
                format!("\\{}", c.name.as_str())
            },
            rank: 4,
        });
    }
    const LIMIT: usize = 400;
    if out.items.len() > LIMIT {
        out.items.sort_by(|a, b| {
            (a.rank, !a.label.starts_with(prefix), a.label.len()).cmp(&(
                b.rank,
                !b.label.starts_with(prefix),
                b.label.len(),
            ))
        });
        out.items.truncate(LIMIT);
        out.incomplete = true;
    }
}
