//! Name resolution for the editor: what a name refers to, where that is
//! declared, and every place that refers to it.

use std::sync::Arc;

use smol_str::SmolStr;
use stir_checker::{Def, MemberKind, typed_file};
use stir_db::{Db, FileOrigin, SourceFile, Workspace};
use stir_hir::{ClassDef, ItemLoc, NodePtr, file_items};
use stir_syntax::{SyntaxKind::*, SyntaxNode, SyntaxToken, TextRange, TextSize};

/// Every resolved name of a file: the checker's resolutions, the names of the
/// file's own declarations, and class names in type positions.
#[salsa::tracked(returns(ref))]
pub fn file_refs(db: &dyn Db, ws: Workspace, file: SourceFile) -> Arc<Vec<(TextRange, Def)>> {
    let root = stir_db::parse(db, file).syntax();
    let mut map: rustc_hash::FxHashMap<TextRange, Def> = typed_file(db, ws, file).defs.clone();
    // declarations win over the checker's view of the same name (a promoted
    // constructor parameter is a property)
    for (range, def) in decl_defs(db, file, &root) {
        map.insert(range, def);
    }
    for (range, def) in type_refs(db, ws, file, &root) {
        map.entry(range).or_insert(def);
    }
    let mut out: Vec<_> = map.into_iter().collect();
    out.sort_by_key(|(r, _)| (r.start(), r.end()));
    Arc::new(out)
}

/// The token spelling `name` (or `$name`) inside `range`.
pub fn name_token(root: &SyntaxNode, range: TextRange, name: &str) -> Option<SyntaxToken> {
    if !root.text_range().contains_range(range) {
        return None;
    }
    let elem = root.covering_element(range);
    let tokens: Vec<SyntaxToken> = match elem {
        stir_syntax::SyntaxElement::Token(t) => vec![t],
        stir_syntax::SyntaxElement::Node(n) => n
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .collect(),
    };
    tokens
        .into_iter()
        .filter(|t| range.contains_range(t.text_range()) && !t.kind().is_trivia())
        .find(|t| t.text() == name || t.text().strip_prefix('$') == Some(name))
}

fn name_range(root: &SyntaxNode, range: TextRange, name: &str) -> TextRange {
    name_token(root, range, name)
        .map(|t| t.text_range())
        .unwrap_or(range)
}

fn member(class: &ClassDef, kind: MemberKind, name: &SmolStr) -> Def {
    Def::Member {
        class: class.name.key(),
        kind,
        name: name.clone(),
    }
}

/// Names of the declarations of `file`.
fn decl_defs(db: &dyn Db, file: SourceFile, root: &SyntaxNode) -> Vec<(TextRange, Def)> {
    let items = file_items(db, file);
    let mut out = Vec::new();
    let loc = |i: usize| ItemLoc {
        file,
        index: i as u32,
    };
    for (i, f) in items.functions.iter().enumerate() {
        out.push((f.name_range, Def::Function(loc(i))));
    }
    for (i, c) in items.consts.iter().enumerate() {
        out.push((
            name_range(root, c.range, c.name.short()),
            Def::Const(loc(i)),
        ));
    }

    for c in &items.classes {
        out.push((c.name_range, Def::Class(c.name.key())));
        for m in c.methods.iter().filter(|m| !m.magic) {
            out.push((m.name_range, member(c, MemberKind::Method, &m.name)));
            for p in m.sig.params.iter().filter(|p| p.promoted.is_some()) {
                out.push((p.name_range, member(c, MemberKind::Property, &p.name)));
            }
        }

        for p in c.properties.iter().filter(|p| !p.magic) {
            out.push((
                name_range(root, p.range, &p.name),
                member(c, MemberKind::Property, &p.name),
            ));
        }
        for k in &c.consts {
            out.push((
                name_range(root, k.range, &k.name),
                member(c, MemberKind::Const, &k.name),
            ));
        }
        for k in &c.cases {
            out.push((
                name_range(root, k.range, &k.name),
                member(c, MemberKind::Case, &k.name),
            ));
        }
    }
    out
}

/// Resolves a class-like name written at `offset` of `file`.
pub fn resolve_type_name(
    db: &dyn Db,
    ws: Workspace,
    file: SourceFile,
    offset: TextSize,
    text: &str,
    imported: bool,
) -> Option<Def> {
    let index = stir_hir::def_index(db, ws);
    let key = if imported {
        SmolStr::new(text.trim_start_matches('\\').to_ascii_lowercase())
    } else {
        let scope = file_items(db, file).scope_at(offset).clone();
        scope.resolve_class(text).key()
    };
    let key = SmolStr::new(key.trim_start_matches('\\'));
    if index.classes.contains_key(&key) {
        return Some(Def::Class(key));
    }

    // a name resolves in `Stir\` as a last resort
    let stir = SmolStr::new(format!("stir\\{key}"));
    if index.classes.contains_key(&stir) {
        return Some(Def::Class(stir));
    }
    None
}

/// Class names outside expressions: types, supertypes, `use` imports, `catch`.
fn type_refs(
    db: &dyn Db,
    ws: Workspace,
    file: SourceFile,
    root: &SyntaxNode,
) -> Vec<(TextRange, Def)> {
    let mut out = Vec::new();
    for node in root.descendants().filter(|n| n.kind() == NAME_REF) {
        let Some(parent) = node.parent() else {
            continue;
        };
        let imported = match parent.kind() {
            NAMED_TYPE | EXTENDS_CLAUSE | IMPLEMENTS_CLAUSE | CATCH_CLAUSE | TRAIT_USE
            | ATTRIBUTE => false,
            USE_CLAUSE => true,
            _ => continue,
        };
        // `use App\{A, B}` groups and function/const imports aren't classes
        if imported && parent.parent().is_some_and(|g| g.kind() != USE_DECL) {
            continue;
        }
        let text = node.text().to_string();
        let range = node.text_range();
        if let Some(def) = resolve_type_name(db, ws, file, range.start(), &text, imported) {
            out.push((range, def));
        }
    }
    out
}

/// The resolved name under `offset`, with its range.
pub fn def_at(
    db: &dyn Db,
    ws: Workspace,
    file: SourceFile,
    offset: TextSize,
) -> Option<(TextRange, Def)> {
    file_refs(db, ws, file)
        .iter()
        .filter(|(r, _)| r.contains_inclusive(offset))
        .min_by_key(|(r, _)| r.len())
        .cloned()
}

/// Where something is declared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavTarget {
    pub file: SourceFile,
    /// The whole declaration.
    pub full_range: TextRange,
    /// Its name.
    pub focus_range: TextRange,
}

fn ptr_range(ptr: &NodePtr) -> TextRange {
    ptr.text_range()
}

/// The class declaration with lowercase FQN `key`.
pub fn class_loc(db: &dyn Db, ws: Workspace, key: &str) -> Option<ItemLoc> {
    stir_hir::def_index(db, ws)
        .classes
        .get(key)
        .and_then(|v| v.first())
        .copied()
}

/// The declaration `def` refers to; `file` is the file of a [`Def::Local`].
pub fn target(db: &dyn Db, ws: Workspace, file: SourceFile, def: &Def) -> Option<NavTarget> {
    let at = |file, full: TextRange, focus: TextRange| {
        Some(NavTarget {
            file,
            full_range: full.cover(focus),
            focus_range: focus,
        })
    };
    match def {
        Def::Local { decl } => at(file, *decl, *decl),
        Def::Function(loc) => {
            let f = &file_items(db, loc.file).functions[loc.index as usize];
            at(loc.file, ptr_range(&f.ptr), f.name_range)
        }
        Def::Const(loc) => {
            let c = &file_items(db, loc.file).consts[loc.index as usize];
            let root = stir_db::parse(db, loc.file).syntax();
            at(
                loc.file,
                c.range,
                name_range(&root, c.range, c.name.short()),
            )
        }

        Def::Class(key) => {
            let loc = class_loc(db, ws, key)?;
            let c = &file_items(db, loc.file).classes[loc.index as usize];
            at(loc.file, ptr_range(&c.ptr), c.name_range)
        }
        Def::Member { class, kind, name } => {
            let loc = class_loc(db, ws, class)?;
            let c = &file_items(db, loc.file).classes[loc.index as usize];
            let root = stir_db::parse(db, loc.file).syntax();
            let named = |range: TextRange| at(loc.file, range, name_range(&root, range, name));
            match kind {
                MemberKind::Method => {
                    let m = c
                        .methods
                        .iter()
                        .find(|m| m.name.eq_ignore_ascii_case(name))?;
                    if m.magic {
                        at(loc.file, c.name_range, c.name_range)
                    } else {
                        at(loc.file, ptr_range(&m.ptr), m.name_range)
                    }
                }
                MemberKind::Property => {
                    if let Some(p) = c.properties.iter().find(|p| &p.name == name) {
                        return if p.magic {
                            at(loc.file, c.name_range, c.name_range)
                        } else {
                            named(p.range)
                        };
                    }
                    let p = c
                        .methods
                        .iter()
                        .flat_map(|m| m.sig.params.iter())
                        .find(|p| p.promoted.is_some() && &p.name == name)?;
                    at(loc.file, p.range, p.name_range)
                }
                MemberKind::Const => named(c.consts.iter().find(|k| &k.name == name)?.range),
                MemberKind::Case => named(c.cases.iter().find(|k| &k.name == name)?.range),
            }
        }
    }
}

/// Every reference to `def` (declarations included), by file.
pub fn references(
    db: &dyn Db,
    ws: Workspace,
    file: SourceFile,
    def: &Def,
) -> Vec<(SourceFile, TextRange)> {
    let files: Vec<SourceFile> = match def {
        Def::Local { .. } => vec![file],
        _ => ws
            .files(db)
            .iter()
            .copied()
            .filter(|f| f.origin(db) == FileOrigin::Source)
            .collect(),
    };
    let mut out = Vec::new();
    for f in files {
        for (range, d) in file_refs(db, ws, f).iter() {
            if d == def {
                out.push((f, *range));
            }
        }
    }
    out
}

/// The innermost function-like node (not a lambda) around `node`.
pub fn enclosing_function(node: &SyntaxNode) -> Option<SyntaxNode> {
    node.ancestors().find(|n| {
        matches!(
            n.kind(),
            FUNCTION_DECL | METHOD_DECL | CLOSURE_EXPR | ARROW_FN_EXPR
        )
    })
}
