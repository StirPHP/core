//! Code completion: members after `->`/`::`, and names in scope.

use rustc_hash::FxHashSet;
use smol_str::SmolStr;
use stir_checker::model_api::{self, MemberTy};
use stir_checker::{Ctx, Def, LocalKind, typed_file};
use stir_db::{Db, SourceFile, Workspace};
use stir_hir::name::{Fqn, Scope};
use stir_hir::{ClassKind, Symbol, SymbolKind, Visibility, file_items};
use stir_syntax::{SyntaxKind::*, SyntaxNode, TextRange, TextSize};
use stir_types::{Ty, TyKind};

use crate::hover::local_label;
use crate::imports::{ImportEdit, Imports};
use crate::nav;
use crate::render;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    Local,
    Param,
    Function,
    Method,
    Property,
    Const,
    EnumCase,
    Class,
    Interface,
    Enum,
    Keyword,
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
    /// Among those of a rank: how good a match it is, lower first.
    pub order: u32,
    /// Its full name, when the label is only the end of it.
    pub qualified: Option<String>,
    /// The `use` line it takes.
    pub import: Option<ImportEdit>,
    pub deprecated: bool,
}

impl Item {
    fn new(label: impl Into<String>, kind: ItemKind, insert: impl Into<String>) -> Item {
        Item {
            label: label.into(),
            kind,
            detail: None,
            insert: insert.into(),
            rank: 0,
            order: 0,
            qualified: None,
            import: None,
            deprecated: false,
        }
    }

    fn rank(mut self, rank: u8) -> Item {
        self.rank = rank;
        self
    }

    fn detail(mut self, detail: impl Into<String>) -> Item {
        self.detail = Some(detail.into());
        self
    }
}

#[derive(Debug, Default)]
pub struct Completions {
    pub items: Vec<Item>,
    /// The range the inserted text replaces (the word being typed).
    pub replace: TextRange,
    /// More results exist for a longer prefix.
    pub incomplete: bool,
}

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

fn ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || !c.is_ascii()
}

/// How well a name matches what's been typed, lower first; `None` if it doesn't. `lower` is the
/// name in lowercase, `query` what's typed in lowercase.
fn match_score(name: &str, lower: &str, query: &str, typed: &str) -> Option<u32> {
    if query.is_empty() {
        return Some(4);
    }
    if lower == query {
        return Some(0);
    }
    if lower.starts_with(query) {
        return Some(if name.starts_with(typed) { 1 } else { 2 });
    }
    if at_word_starts(name, query) {
        return Some(3);
    }
    if lower.contains(query) {
        return Some(4);
    }
    // the letters in order: the closer together the better, and only if they're not all over
    let bytes = lower.as_bytes();
    let mut at = 0;
    let (mut start, mut end) = (None, 0);
    for q in query.bytes() {
        let found = bytes[at..].iter().position(|b| *b == q)?;
        at += found;
        start.get_or_insert(at);
        end = at;
        at += 1;
    }
    let span = end - start.unwrap_or(0) + 1;
    Some(5 + (span - query.len()).min(40) as u32)
}

/// Do the letters match the starts of `name`'s words: `FB` for `FooBar`, `f_b` for `foo_bar`?
fn at_word_starts(name: &str, query: &str) -> bool {
    let mut q = query.bytes().peekable();
    let mut previous = b'_';
    let mut within = false;
    for b in name.bytes() {
        let starts_word =
            (b.is_ascii_uppercase() && !previous.is_ascii_uppercase()) || previous == b'_';
        let lower = b.to_ascii_lowercase();
        if let Some(&want) = q.peek()
            && lower == want
            && (starts_word || within)
        {
            q.next();
            within = true;
        } else {
            within = false;
        }
        previous = b;
    }
    q.peek().is_none()
}

/// What can be written where the cursor is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    /// An expression or a statement.
    Anywhere,
    /// After `new`.
    New,
    /// After `extends`, or where a class can be named but not instantiated.
    Class,
    /// After `implements`.
    Interface,
    /// `use` in a class's body.
    Trait,
    /// A type: of a parameter, a return or a property.
    Type,
    /// `use` outside classes, and what it can import.
    Import(ImportKind),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImportKind {
    Class,
    Function,
    Const,
}

impl Place {
    fn accepts(self, kind: SymbolKind) -> bool {
        use ClassKind as C;
        matches!(
            (self, kind),
            (Place::Anywhere, _)
                | (Place::New, SymbolKind::Class(C::Class))
                | (Place::Class | Place::Type, SymbolKind::Class(_))
                | (Place::Interface, SymbolKind::Class(C::Interface))
                | (Place::Trait, SymbolKind::Class(C::Trait))
                | (Place::Import(ImportKind::Class), SymbolKind::Class(_))
                | (Place::Import(ImportKind::Function), SymbolKind::Function)
                | (Place::Import(ImportKind::Const), SymbolKind::Const)
        )
    }
}

/// The word before `at`, with what's between skipped.
fn word_before(text: &str, at: usize) -> &str {
    let trimmed = text[..at].trim_end();
    let start = trimmed
        .char_indices()
        .rev()
        .take_while(|(_, c)| ident_char(*c))
        .last()
        .map_or(trimmed.len(), |(i, _)| i);
    &trimmed[start..]
}

/// What the name starting at `start` is, from what's before it.
fn place_at(root: &SyntaxNode, text: &str, start: usize, offset: TextSize) -> Place {
    let before = word_before(text, start);
    let lower = before.to_ascii_lowercase();
    let in_class_body = || {
        root.token_at_offset(offset)
            .left_biased()
            .and_then(|t| t.parent())
            .is_some_and(|n| n.ancestors().any(|a| a.kind() == CLASS_BODY))
    };
    match lower.as_str() {
        "new" => Place::New,
        "extends" | "instanceof" => Place::Class,
        "implements" => Place::Interface,
        "use" if in_class_body() => Place::Trait,
        "use" => Place::Import(ImportKind::Class),
        "function" | "const"
            if word_before(text, text[..start].trim_end().len() - before.len())
                .eq_ignore_ascii_case("use") =>
        {
            if lower == "function" {
                Place::Import(ImportKind::Function)
            } else {
                Place::Import(ImportKind::Const)
            }
        }
        _ => {
            let previous = text[..start].trim_end();
            // `implements A, B` and `catch (A | B`
            let token = root
                .token_at_offset(offset)
                .left_biased()
                .and_then(|t| t.parent());
            let around = token.as_ref().and_then(|n| {
                n.ancestors().find(|a| {
                    matches!(
                        a.kind(),
                        NAMED_TYPE | IMPLEMENTS_CLAUSE | EXTENDS_CLAUSE | CATCH_CLAUSE
                    )
                })
            });
            match around.map(|n| n.kind()) {
                Some(IMPLEMENTS_CLAUSE) => Place::Interface,
                Some(EXTENDS_CLAUSE | CATCH_CLAUSE) => Place::Class,
                Some(NAMED_TYPE) => Place::Type,
                _ if previous.ends_with(['|', '&'])
                    && word_before(text, previous.len() - 1).eq_ignore_ascii_case("catch") =>
                {
                    Place::Class
                }
                _ => Place::Anywhere,
            }
        }
    }
}

/// PHP's own types, where a type goes.
const TYPE_KEYWORDS: &[&str] = &[
    "array", "bool", "callable", "false", "float", "int", "iterable", "mixed", "never", "null",
    "object", "parent", "self", "static", "string", "true", "void",
];

/// Is the cursor in a string or a comment, where names aren't code?
fn in_text(root: &SyntaxNode, offset: TextSize) -> bool {
    root.token_at_offset(offset).left_biased().is_some_and(|t| {
        matches!(
            t.kind(),
            COMMENT | DOC_COMMENT | STRING_SINGLE | STRING_DOUBLE | STRING_CONTENT | INLINE_HTML
        ) && t.text_range().end() > offset
    })
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
    let ctx = Ctx::new(db, ws);
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
            out.items
                .push(Item::new("class", ItemKind::Keyword, "class").rank(5));
        }
        return out;
    }
    let root = stir_db::parse(db, file).syntax();
    if in_text(&root, offset) {
        return out;
    }
    // what's written of a name so far, and the place it's in
    let name_start = text[..word_start]
        .char_indices()
        .rev()
        .take_while(|(_, c)| ident_char(*c) || *c == '\\')
        .last()
        .map_or(word_start, |(i, _)| i);
    let name_start = if dollar { word_start } else { name_start };
    let place = if dollar {
        Place::Anywhere
    } else {
        place_at(&root, text, name_start, offset)
    };
    if matches!(place, Place::Anywhere) {
        local_items(db, ws, file, offset, &mut out.items);
    }
    if dollar {
        return out;
    }
    let written = &text[name_start..word_start];
    if place == Place::Anywhere && written.is_empty() {
        for kw in PHP_KEYWORDS {
            out.items
                .push(Item::new(*kw, ItemKind::Keyword, *kw).rank(6));
        }
    }
    if place == Place::Type && written.is_empty() {
        for kw in TYPE_KEYWORDS {
            out.items
                .push(Item::new(*kw, ItemKind::Keyword, *kw).rank(1));
        }
    }
    if prefix.is_empty() && written.is_empty() && !matches!(place, Place::Import(_)) {
        // globals only once something is typed: the stubs are large
        out.incomplete = true;
        return out;
    }
    global_items(db, ws, file, offset, prefix, written, place, &mut out);
    if matches!(place, Place::Import(_)) {
        out.replace = TextRange::new((name_start as u32).into(), offset);
    }
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
    ty = stir_types::without_null(&ty);
    let ctx = Ctx::new(db, ws);
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
                let readonly = if *immutable { "readonly " } else { "" };
                let label = format!("{readonly}{} ${}", ty, m.name);
                let insert = if statics {
                    format!("${}", m.name)
                } else {
                    m.name.to_string()
                };
                (ItemKind::Property, label, insert)
            }
            MemberTy::Const(t) => (
                ItemKind::Const,
                format!("const {} {}", t, m.name),
                m.name.to_string(),
            ),
            MemberTy::Case => (
                ItemKind::EnumCase,
                format!("case {}", m.name),
                m.name.to_string(),
            ),
        };
        let mut item = Item::new(m.name.to_string(), kind, insert).detail(detail);
        item.deprecated = m.deprecated;
        out.push(item);
    }
}

/// Nodes that bound the visibility of the bindings declared in them.
fn scope_node(node: &SyntaxNode) -> Option<SyntaxNode> {
    node.ancestors().find(|n| {
        matches!(
            n.kind(),
            FUNCTION_DECL | METHOD_DECL | CLOSURE_EXPR | ARROW_FN_EXPR
        )
    })
}

fn local_items(
    db: &dyn Db,
    ws: Workspace,
    file: SourceFile,
    offset: TextSize,
    out: &mut Vec<Item>,
) {
    let root = stir_db::parse(db, file).syntax();
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
        if let Some(scope) = scope_node(&node)
            && !scope.text_range().contains(offset)
        {
            continue;
        }
        let name = token.text().trim_start_matches('$').to_owned();
        if !seen.insert(name.clone()) {
            continue;
        }
        let label = format!("${name}");
        let kind = if *kind == LocalKind::Param {
            ItemKind::Param
        } else {
            ItemKind::Local
        };
        out.push(Item::new(label.clone(), kind, label).detail(local_label(&name, ty)));
    }
}

/// A name that matched, for choosing the best before working out more about them.
struct Candidate<'a> {
    symbol: &'a Symbol,
    /// What the label will be: its last segment, or what's left of it after a written qualifier.
    label: &'a str,
    score: u32,
    origin: u8,
}

/// The most of these sent at once: more are found by typing more.
const LIMIT: usize = 200;

#[allow(clippy::too_many_arguments)]
fn global_items(
    db: &dyn Db,
    ws: Workspace,
    file: SourceFile,
    offset: TextSize,
    prefix: &str,
    written: &str,
    place: Place,
    out: &mut Completions,
) {
    let table = stir_hir::symbol_table(db, ws);
    let query = prefix.to_ascii_lowercase();
    let scope = file_items(db, file).scope_at(offset).clone();
    // what a written qualifier stands for: only what's under it can be meant
    let qualifier = (!written.is_empty()).then(|| {
        let name = written.trim_end_matches('\\');
        let full = match place {
            Place::Import(_) => Fqn::new(name),
            _ => resolve_qualifier(&scope, name),
        };
        format!("{}\\", full.as_str().to_ascii_lowercase())
    });
    let qualifier = qualifier.filter(|q| q != "\\");
    // `\Fo`: named from the top, with nothing to import
    let absolute = written == "\\";
    let mut candidates: Vec<Candidate<'_>> = Vec::new();
    for symbol in table.iter() {
        if !place.accepts(symbol.kind) {
            continue;
        }
        let name = symbol.name.as_str();
        let label = match &qualifier {
            Some(q) => {
                let under = name
                    .get(..q.len())
                    .is_some_and(|h| h.eq_ignore_ascii_case(q));
                if !under {
                    continue;
                }
                &name[q.len()..]
            }
            None => symbol.name.short(),
        };
        let score = if qualifier.is_some() {
            match_score(label, &label.to_ascii_lowercase(), &query, prefix)
        } else {
            match_score(label, &symbol.short_lower, &query, prefix)
        };
        let Some(score) = score else { continue };
        candidates.push(Candidate {
            symbol,
            label,
            score,
            origin: stir_hir::DefIndex::rank(symbol.file.origin(db)),
        });
    }
    candidates.retain(|c| nav::extension_is_offered(db, ws, c.symbol.file));
    candidates.sort_by(|a, b| {
        (a.score, a.origin, a.label.len(), a.symbol.name.as_str()).cmp(&(
            b.score,
            b.origin,
            b.label.len(),
            b.symbol.name.as_str(),
        ))
    });
    if candidates.len() > LIMIT {
        candidates.truncate(LIMIT);
        out.incomplete = true;
    }
    let ctx = Ctx::new(db, ws);
    let imports = Imports::new(db, file, offset);
    for (order, c) in candidates.iter().enumerate() {
        let name = &c.symbol.name;
        let reference = if absolute || qualifier.is_some() || matches!(place, Place::Import(_)) {
            None
        } else {
            imports.as_ref().map(|i| match c.symbol.kind {
                SymbolKind::Class(_) => i.class(db, ws, name),
                SymbolKind::Function => i.function(db, ws, name),
                SymbolKind::Const => i.constant(db, ws, name),
            })
        };
        let import_place = matches!(place, Place::Import(_));
        let (label, insert, import) = match reference {
            Some(r) => (c.label.to_owned(), r.text, r.import),
            None if import_place => (name.as_str().to_owned(), name.as_str().to_owned(), None),
            None if qualifier.is_some() => (c.label.to_owned(), c.label.to_owned(), None),
            None if absolute => (c.label.to_owned(), name.as_str().to_owned(), None),
            None => (c.label.to_owned(), format!("\\{}", name.as_str()), None),
        };
        let kind = match c.symbol.kind {
            SymbolKind::Class(ClassKind::Interface) => ItemKind::Interface,
            SymbolKind::Class(ClassKind::Enum) => ItemKind::Enum,
            SymbolKind::Class(_) => ItemKind::Class,
            SymbolKind::Function => ItemKind::Function,
            SymbolKind::Const => ItemKind::Const,
        };
        let detail = match c.symbol.kind {
            SymbolKind::Function => {
                let loc = stir_hir::ItemLoc {
                    file: c.symbol.file,
                    index: c.symbol.index,
                };
                render::signature(&model_api::function_info(&ctx, loc), "function").label
            }
            _ => name.as_str().to_owned(),
        };
        let mut item = Item::new(label, kind, insert)
            .detail(detail)
            .rank(if c.origin == 0 { 2 } else { 4 });
        item.rank = if c.score <= 2 { 1 } else { item.rank };
        item.order = order as u32;
        item.qualified = Some(name.as_str().to_owned());
        item.import = import;
        let items = file_items(db, c.symbol.file);
        let index = c.symbol.index as usize;
        item.deprecated = match c.symbol.kind {
            SymbolKind::Class(_) => items.classes[index].deprecated.is_some(),
            SymbolKind::Function => items.functions[index].deprecated.is_some(),
            SymbolKind::Const => items.consts[index].deprecated.is_some(),
        };
        out.items.push(item);
    }
}

/// What a written `A\B\` stands for: an alias's target, an absolute name, or below the namespace.
fn resolve_qualifier(scope: &Scope, name: &str) -> Fqn {
    if name.is_empty() {
        return Fqn::new("");
    }
    scope.resolve_class(name)
}
