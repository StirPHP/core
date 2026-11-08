//! The requests of the protocol, answered from a snapshot of the database: what both the native
//! server ([`crate::server`]) and the one in the browser ([`crate::session`]) answer with, each
//! taking care of its own transport, threads and time.

pub mod actions;
pub mod diagnostics;

use std::collections::HashMap;
use std::path::Path;

use ls_types::*;
use serde_json::{Value, json};
use stir_db::{Db, RootDatabase, SourceFile, Workspace};
use stir_syntax::{TextRange, TextSize};

use crate::convert::{Encoding, LineMap};
use crate::links::Links;
use crate::{complete, hierarchy, hints, hover, nav, symbols, tokens};

/// A request that can't be answered as asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    InvalidParams(String),
    /// The document changed since what the request is about was worked out.
    ContentModified,
}

/// What a request is answered from.
#[derive(Clone, Copy)]
pub struct Snapshot<'a> {
    pub db: &'a RootDatabase,
    pub ws: Workspace,
    pub enc: Encoding,
    pub links: &'a dyn Links,
}

impl Snapshot<'_> {
    pub fn location(&self, file: SourceFile, range: TextRange) -> Option<Location> {
        let map = LineMap::new(file.text(self.db), self.enc);
        Some(Location::new(
            self.links.uri(self.db, file)?,
            map.range(range),
        ))
    }

    /// The workspace file at `path`.
    pub fn file_at(&self, path: &Path) -> Option<SourceFile> {
        self.ws
            .files(self.db)
            .iter()
            .find(|f| f.path(self.db) == path)
            .copied()
    }
}

/// What the server can do, for `initialize`. `utf8`: the client takes UTF-8 positions.
pub fn capabilities(utf8: bool) -> ServerCapabilities {
    let legend = SemanticTokensLegend {
        token_types: tokens::TOKEN_TYPES
            .iter()
            .map(|t| SemanticTokenType::new(t))
            .collect(),
        token_modifiers: tokens::TOKEN_MODIFIERS
            .iter()
            .map(|m| SemanticTokenModifier::new(m))
            .collect(),
    };
    ServerCapabilities {
        position_encoding: Some(if utf8 {
            PositionEncodingKind::UTF8
        } else {
            PositionEncodingKind::UTF16
        }),
        text_document_sync: Some(TextDocumentSyncCapability::Options(
            TextDocumentSyncOptions {
                open_close: Some(true),
                change: Some(TextDocumentSyncKind::INCREMENTAL),
                // PHPStan runs on a save
                save: Some(TextDocumentSyncSaveOptions::Supported(true)),
                ..Default::default()
            },
        )),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        definition_provider: Some(OneOf::Left(true)),
        declaration_provider: Some(DeclarationCapability::Simple(true)),
        implementation_provider: Some(ImplementationProviderCapability::Simple(true)),
        call_hierarchy_provider: Some(CallHierarchyServerCapability::Simple(true)),
        type_definition_provider: Some(TypeDefinitionProviderCapability::Simple(true)),
        references_provider: Some(OneOf::Left(true)),
        document_highlight_provider: Some(OneOf::Left(true)),
        rename_provider: Some(OneOf::Right(RenameOptions {
            prepare_provider: Some(true),
            work_done_progress_options: Default::default(),
        })),
        completion_provider: Some(CompletionOptions {
            trigger_characters: Some(vec![">".into(), ":".into(), "$".into(), "\\".into()]),
            ..Default::default()
        }),
        signature_help_provider: Some(SignatureHelpOptions {
            trigger_characters: Some(vec!["(".into(), ",".into()]),
            ..Default::default()
        }),
        document_symbol_provider: Some(OneOf::Left(true)),
        workspace_symbol_provider: Some(OneOf::Left(true)),
        semantic_tokens_provider: Some(SemanticTokensServerCapabilities::SemanticTokensOptions(
            SemanticTokensOptions {
                legend,
                full: Some(SemanticTokensFullOptions::Bool(true)),
                range: Some(false),
                ..Default::default()
            },
        )),
        inlay_hint_provider: Some(OneOf::Left(true)),
        folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
        selection_range_provider: Some(SelectionRangeProviderCapability::Simple(true)),
        document_formatting_provider: Some(OneOf::Left(true)),
        document_range_formatting_provider: Some(OneOf::Left(true)),
        code_action_provider: Some(CodeActionProviderCapability::Options(CodeActionOptions {
            code_action_kinds: Some(
                actions::KINDS
                    .iter()
                    .map(|k| CodeActionKind::new(k))
                    .collect(),
            ),
            resolve_provider: Some(true),
            ..Default::default()
        })),
        execute_command_provider: Some(ExecuteCommandOptions {
            commands: vec![actions::ADD_PARAMETER_COMMAND.to_owned()],
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// The answer to `initialize`.
pub fn initialize_result(utf8: bool) -> InitializeResult {
    InitializeResult {
        offset_encoding: None,
        server_info: Some(ServerInfo {
            name: "stirphp".into(),
            version: Some(env!("CARGO_PKG_VERSION").into()),
        }),
        capabilities: capabilities(utf8),
    }
}

/// Does the client take UTF-8 positions?
pub fn wants_utf8(params: &InitializeParams) -> bool {
    params
        .capabilities
        .general
        .as_ref()
        .and_then(|g| g.position_encodings.as_ref())
        .is_some_and(|e| e.contains(&PositionEncodingKind::UTF8))
}

/// Does the client want a type hierarchy registered? It's the only way to offer one: the server
/// capabilities of the protocol's types have no field for it.
pub fn type_hierarchy_dynamic(params: &InitializeParams) -> bool {
    params
        .capabilities
        .text_document
        .as_ref()
        .and_then(|t| t.type_hierarchy.as_ref())
        .and_then(|t| t.dynamic_registration)
        .unwrap_or(false)
}

/// The registration of the type hierarchy, for [`type_hierarchy_dynamic`] clients.
pub fn type_hierarchy_registration() -> Registration {
    let selector = DocumentFilter {
        language: Some("php".into()),
        scheme: None,
        pattern: None,
    };
    let options = TypeHierarchyRegistrationOptions {
        text_document_registration_options: TextDocumentRegistrationOptions {
            document_selector: Some(vec![selector]),
        },
        ..Default::default()
    };
    Registration {
        id: "stirphp-type-hierarchy".into(),
        method: "textDocument/prepareTypeHierarchy".into(),
        register_options: serde_json::to_value(options).ok(),
    }
}

/// The workspace folder the client opened.
pub fn workspace_folder(params: &InitializeParams) -> Option<Uri> {
    #[allow(deprecated)]
    params
        .workspace_folders
        .as_ref()
        .and_then(|f| f.first())
        .map(|f| f.uri.clone())
        .or(params.root_uri.clone())
}

fn symbol_kind(kind: symbols::SymbolKind) -> SymbolKind {
    use symbols::SymbolKind as K;
    match kind {
        K::Function => SymbolKind::FUNCTION,
        K::Class => SymbolKind::CLASS,
        K::Interface => SymbolKind::INTERFACE,
        K::Enum => SymbolKind::ENUM,
        K::Method => SymbolKind::METHOD,
        K::Property => SymbolKind::PROPERTY,
        K::Constant => SymbolKind::CONSTANT,
        K::EnumMember => SymbolKind::ENUM_MEMBER,
    }
}

#[allow(deprecated)]
fn document_symbol(s: symbols::Symbol, map: &LineMap) -> DocumentSymbol {
    DocumentSymbol {
        name: s.name,
        detail: s.detail,
        kind: symbol_kind(s.kind),
        tags: s.deprecated.then(|| vec![SymbolTag::DEPRECATED]),
        deprecated: s.deprecated.then_some(true),
        range: map.range(s.full_range),
        selection_range: map.range(s.focus_range),
        children: Some(
            s.children
                .into_iter()
                .map(|c| document_symbol(c, map))
                .collect(),
        ),
    }
}

fn completion_kind(kind: complete::ItemKind) -> CompletionItemKind {
    use complete::ItemKind as K;
    match kind {
        K::Local => CompletionItemKind::VARIABLE,
        K::Param => CompletionItemKind::VARIABLE,
        K::Function => CompletionItemKind::FUNCTION,
        K::Method => CompletionItemKind::METHOD,
        K::Property => CompletionItemKind::PROPERTY,
        K::Const => CompletionItemKind::CONSTANT,
        K::EnumCase => CompletionItemKind::ENUM_MEMBER,
        K::Class => CompletionItemKind::CLASS,
        K::Interface => CompletionItemKind::INTERFACE,
        K::Enum => CompletionItemKind::ENUM,
        K::Keyword => CompletionItemKind::KEYWORD,
    }
}

/// The rename edit for one occurrence: keeps the occurrence's `$` (variables and
/// static properties are written with it, others without).
fn renamed(old: &str, new: &str) -> String {
    let bare = new.trim_start_matches('$');
    if old.starts_with('$') {
        format!("${bare}")
    } else {
        bare.to_owned()
    }
}

fn valid_identifier(name: &str) -> bool {
    let bare = name.strip_prefix('$').unwrap_or(name);
    let mut chars = bare.chars();
    chars
        .next()
        .is_some_and(|c| c.is_alphabetic() || c == '_' || !c.is_ascii())
        && chars.all(|c| c.is_alphanumeric() || c == '_' || !c.is_ascii())
}

/// A place in a workspace file a client hands back to us later: its path may be `stubs://`, which
/// no URI says.
fn hierarchy_data(db: &dyn Db, file: SourceFile, offset: TextSize) -> Value {
    json!({
        "path": file.path(db).to_string_lossy(),
        "offset": u32::from(offset),
    })
}

/// The file and offset of [`hierarchy_data`].
fn from_hierarchy_data(s: &Snapshot, data: Option<&Value>) -> Option<(SourceFile, TextSize)> {
    let data = data?;
    let path = Path::new(data.get("path")?.as_str()?);
    let offset = u32::try_from(data.get("offset")?.as_u64()?).ok()?;
    Some((s.file_at(path)?, offset.into()))
}

fn call_item(s: &Snapshot, item: &hierarchy::CallItem) -> Option<CallHierarchyItem> {
    let db = s.db;
    let map = LineMap::new(item.file.text(db), s.enc);
    let kind = match item.kind {
        hierarchy::CallKind::Function => SymbolKind::FUNCTION,
        hierarchy::CallKind::Method => SymbolKind::METHOD,
        hierarchy::CallKind::Constructor => SymbolKind::CONSTRUCTOR,
        hierarchy::CallKind::File => SymbolKind::FILE,
    };
    let mut data = hierarchy_data(db, item.file, item.focus_range.start());
    data["end"] = json!(u32::from(item.full_range.end()));
    data["start"] = json!(u32::from(item.full_range.start()));
    data["top"] = json!(item.kind == hierarchy::CallKind::File);
    Some(CallHierarchyItem {
        name: item.name.clone(),
        kind,
        tags: None,
        detail: item.detail.clone(),
        uri: s.links.uri(db, item.file)?,
        range: map.range(item.full_range),
        selection_range: map.range(item.focus_range),
        data: Some(data),
    })
}

fn type_kind(kind: stir_hir::ClassKind) -> SymbolKind {
    match kind {
        stir_hir::ClassKind::Class => SymbolKind::CLASS,
        stir_hir::ClassKind::Interface => SymbolKind::INTERFACE,
        stir_hir::ClassKind::Trait => SymbolKind::CLASS,
        stir_hir::ClassKind::Enum => SymbolKind::ENUM,
    }
}

fn type_hierarchy_item(s: &Snapshot, item: &hierarchy::TypeItem) -> Option<TypeHierarchyItem> {
    let map = LineMap::new(item.file.text(s.db), s.enc);
    Some(TypeHierarchyItem {
        name: item.name.clone(),
        kind: type_kind(item.kind),
        tags: None,
        detail: item.detail.clone(),
        uri: s.links.uri(s.db, item.file)?,
        range: map.range(item.full_range),
        selection_range: map.range(item.focus_range),
        data: Some(json!({ "key": item.key.as_str() })),
    })
}

fn type_key(item: &TypeHierarchyItem) -> Option<String> {
    Some(item.data.as_ref()?.get("key")?.as_str()?.to_owned())
}

fn scalar_or_array(locations: Vec<Location>) -> Option<GotoDefinitionResponse> {
    match locations.len() {
        0 => None,
        1 => locations
            .into_iter()
            .next()
            .map(GotoDefinitionResponse::Scalar),
        _ => Some(GotoDefinitionResponse::Array(locations)),
    }
}

/// How to indent: the project's say, then the editor's.
pub fn format_options(
    project: Option<stir_neon::phpstan::FormatIndent>,
    editor: &FormattingOptions,
) -> stir_fmt::Options {
    use stir_neon::phpstan::FormatIndent;
    let indent = match project {
        Some(FormatIndent::Tab) => stir_fmt::Indent::Tabs,
        Some(FormatIndent::Spaces(n)) => stir_fmt::Indent::Spaces(u32::from(n)),
        None if editor.insert_spaces => stir_fmt::Indent::Spaces(editor.tab_size.clamp(1, 16)),
        None => stir_fmt::Indent::Tabs,
    };
    stir_fmt::Options { indent }
}

fn text_edits(map: &LineMap, edits: Vec<stir_fmt::Edit>) -> Vec<TextEdit> {
    edits
        .into_iter()
        .map(|e| TextEdit {
            range: map.range(e.range),
            new_text: e.text,
        })
        .collect()
}

// Requests about a document: `file` is the document's, `map` its lines.

pub fn hover(s: &Snapshot, file: SourceFile, map: &LineMap, params: HoverParams) -> Option<Hover> {
    let pos = params.text_document_position_params.position;
    let h = hover::hover(s.db, s.ws, file, map.offset(pos))?;
    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: h.markdown,
        }),
        range: Some(map.range(h.range)),
    })
}

pub fn definition(
    s: &Snapshot,
    file: SourceFile,
    map: &LineMap,
    params: GotoDefinitionParams,
) -> Option<GotoDefinitionResponse> {
    let pos = params.text_document_position_params.position;
    let (db, ws) = (s.db, s.ws);
    // a name in a string that a framework knows: a hook, a view, a route
    let named = stir_checker::script::links::string_targets(db, ws, file, map.offset(pos));
    if !named.is_empty() {
        let locs: Vec<Location> = named
            .into_iter()
            .filter_map(|(f, range)| s.location(f, range))
            .collect();
        return Some(match <[Location; 1]>::try_from(locs) {
            Ok([one]) => GotoDefinitionResponse::Scalar(one),
            Err(many) => GotoDefinitionResponse::Array(many),
        });
    }
    let (_, def) = nav::def_at(db, ws, file, map.offset(pos))?;
    let t = nav::target(db, ws, file, &def)?;
    let loc = s.location(t.file, t.focus_range)?;
    Some(GotoDefinitionResponse::Scalar(loc))
}

pub fn type_definition(
    s: &Snapshot,
    file: SourceFile,
    map: &LineMap,
    params: GotoDefinitionParams,
) -> Option<GotoDefinitionResponse> {
    let pos = params.text_document_position_params.position;
    let (db, ws) = (s.db, s.ws);
    let offset = map.offset(pos);
    let typed = stir_checker::typed_file(db, ws, file);
    let ty = match nav::def_at(db, ws, file, offset) {
        Some((range, stir_checker::Def::Local { decl })) => typed
            .types
            .get(&range)
            .cloned()
            .or_else(|| typed.locals.get(&decl).map(|l| l.0.clone()))?,
        Some((range, _)) => typed.types.get(&range).cloned()?,
        None => typed
            .types
            .iter()
            .filter(|(r, _)| r.contains(offset))
            .min_by_key(|(r, _)| r.len())?
            .1
            .clone(),
    };
    let ty = stir_types::without_null(&ty);
    let key = match ty.kind() {
        stir_types::TyKind::Class { name, .. } => name.to_ascii_lowercase(),
        _ => return None,
    };
    let def = stir_checker::Def::Class(key.trim_start_matches('\\').into());
    let t = nav::target(db, ws, file, &def)?;
    Some(GotoDefinitionResponse::Scalar(
        s.location(t.file, t.focus_range)?,
    ))
}

pub fn declaration(
    s: &Snapshot,
    file: SourceFile,
    map: &LineMap,
    params: request::GotoDeclarationParams,
) -> Option<request::GotoDeclarationResponse> {
    let pos = params.text_document_position_params.position;
    let (_, def) = nav::def_at(s.db, s.ws, file, map.offset(pos))?;
    let locations = hierarchy::declarations(s.db, s.ws, file, &def)
        .into_iter()
        .filter_map(|t| s.location(t.file, t.focus_range))
        .collect();
    scalar_or_array(locations)
}

pub fn implementation(
    s: &Snapshot,
    file: SourceFile,
    map: &LineMap,
    params: request::GotoImplementationParams,
) -> Option<request::GotoImplementationResponse> {
    let pos = params.text_document_position_params.position;
    let (_, def) = nav::def_at(s.db, s.ws, file, map.offset(pos))?;
    let locations = hierarchy::implementations(s.db, s.ws, file, &def)
        .into_iter()
        .filter_map(|t| s.location(t.file, t.focus_range))
        .collect();
    Some(GotoDefinitionResponse::Array(locations))
}

pub fn prepare_call_hierarchy(
    s: &Snapshot,
    file: SourceFile,
    map: &LineMap,
    params: CallHierarchyPrepareParams,
) -> Option<Vec<CallHierarchyItem>> {
    let pos = params.text_document_position_params.position;
    let (_, item) = hierarchy::callable_at(s.db, s.ws, file, map.offset(pos))?;
    Some(vec![call_item(s, &item)?])
}

pub fn prepare_type_hierarchy(
    s: &Snapshot,
    file: SourceFile,
    map: &LineMap,
    params: TypeHierarchyPrepareParams,
) -> Option<Vec<TypeHierarchyItem>> {
    let pos = params.text_document_position_params.position;
    let item = hierarchy::type_at(s.db, s.ws, file, map.offset(pos))?;
    Some(vec![type_hierarchy_item(s, &item)?])
}

pub fn references(
    s: &Snapshot,
    file: SourceFile,
    map: &LineMap,
    params: ReferenceParams,
) -> Option<Vec<Location>> {
    let pos = params.text_document_position.position;
    let include_decl = params.context.include_declaration;
    let (_, def) = nav::def_at(s.db, s.ws, file, map.offset(pos))?;
    let decl = nav::target(s.db, s.ws, file, &def);
    let refs = nav::references(s.db, s.ws, file, &def);
    Some(
        refs.into_iter()
            .filter(|(f, r)| {
                include_decl
                    || decl
                        .as_ref()
                        .is_none_or(|d| !(d.file == *f && d.focus_range == *r))
            })
            .filter_map(|(f, r)| s.location(f, r))
            .collect(),
    )
}

pub fn document_highlight(
    s: &Snapshot,
    file: SourceFile,
    map: &LineMap,
    params: DocumentHighlightParams,
) -> Option<Vec<DocumentHighlight>> {
    let pos = params.text_document_position_params.position;
    let (_, def) = nav::def_at(s.db, s.ws, file, map.offset(pos))?;
    let decl = nav::target(s.db, s.ws, file, &def);
    Some(
        nav::file_refs(s.db, s.ws, file)
            .iter()
            .filter(|(_, d)| *d == def)
            .map(|(r, _)| DocumentHighlight {
                range: map.range(*r),
                kind: Some(
                    if decl
                        .as_ref()
                        .is_some_and(|d| d.file == file && d.focus_range == *r)
                    {
                        DocumentHighlightKind::WRITE
                    } else {
                        DocumentHighlightKind::READ
                    },
                ),
            })
            .collect(),
    )
}

pub fn prepare_rename(
    s: &Snapshot,
    file: SourceFile,
    map: &LineMap,
    params: TextDocumentPositionParams,
) -> Result<Option<PrepareRenameResponse>, Failure> {
    let pos = params.position;
    let Some((range, def)) = nav::def_at(s.db, s.ws, file, map.offset(pos)) else {
        return Ok(None);
    };
    if !hover::is_project_def(s.db, s.ws, file, &def) {
        return Err(Failure::InvalidParams(
            "can't rename a declaration outside the project".into(),
        ));
    }
    // the name token itself (not `\Qualified\Name`)
    let text = &file.text(s.db)[usize::from(range.start())..usize::from(range.end())];
    let short = text.rsplit('\\').next().unwrap_or(text);
    let start = range.end() - TextSize::of(short);
    Ok(Some(PrepareRenameResponse::Range(
        map.range(TextRange::new(start, range.end())),
    )))
}

/// Says why a rename can't be done before there's any document to look at.
pub fn check_new_name(params: &RenameParams) -> Result<(), Failure> {
    let new_name = &params.new_name;
    if valid_identifier(new_name) {
        Ok(())
    } else {
        Err(Failure::InvalidParams(format!(
            "`{new_name}` is not a valid name"
        )))
    }
}

pub fn rename(
    s: &Snapshot,
    file: SourceFile,
    map: &LineMap,
    params: RenameParams,
) -> Result<Option<WorkspaceEdit>, Failure> {
    check_new_name(&params)?;
    let pos = params.text_document_position.position;
    let new_name = params.new_name;
    let Some((_, def)) = nav::def_at(s.db, s.ws, file, map.offset(pos)) else {
        return Ok(None);
    };
    if !hover::is_project_def(s.db, s.ws, file, &def) {
        return Err(Failure::InvalidParams(
            "can't rename a declaration outside the project".into(),
        ));
    }
    let mut changes: HashMap<Uri, Vec<TextEdit>> = Default::default();
    for (f, range) in nav::references(s.db, s.ws, file, &def) {
        let text = f.text(s.db);
        let old = &text[usize::from(range.start())..usize::from(range.end())];
        // only the last segment of a qualified name changes
        let short = old.rsplit('\\').next().unwrap_or(old);
        let range = TextRange::new(range.end() - TextSize::of(short), range.end());
        let Some(uri) = s.links.uri(s.db, f) else {
            continue;
        };
        let fmap = LineMap::new(text, s.enc);
        changes.entry(uri).or_default().push(TextEdit {
            range: fmap.range(range),
            new_text: renamed(short, &new_name),
        });
    }
    Ok(Some(WorkspaceEdit {
        changes: Some(changes),
        ..Default::default()
    }))
}

pub fn completion(
    s: &Snapshot,
    file: SourceFile,
    map: &LineMap,
    params: CompletionParams,
) -> Option<CompletionResponse> {
    let pos = params.text_document_position.position;
    let c = complete::complete(s.db, s.ws, file, map.offset(pos));
    let range = map.range(c.replace);
    let items = c
        .items
        .into_iter()
        .map(|i| CompletionItem {
            deprecated: i.deprecated.then_some(true),
            filter_text: Some(i.label.clone()),
            sort_text: Some(format!("{}{:04}{}", i.rank, i.order, i.label)),
            kind: Some(completion_kind(i.kind)),
            detail: i.detail,
            label_details: i.qualified.map(|q| CompletionItemLabelDetails {
                detail: None,
                description: Some(q),
            }),
            text_edit: Some(CompletionTextEdit::Edit(TextEdit {
                range,
                new_text: i.insert,
            })),
            additional_text_edits: i.import.map(|edit| {
                let at = map.position(edit.at);
                vec![TextEdit {
                    range: Range::new(at, at),
                    new_text: edit.text,
                }]
            }),
            tags: i.deprecated.then(|| vec![CompletionItemTag::DEPRECATED]),
            label: i.label,
            ..Default::default()
        })
        .collect();
    Some(CompletionResponse::List(CompletionList {
        is_incomplete: c.incomplete,
        items,
    }))
}

/// `indent`: how the project says to indent.
pub fn formatting(
    s: &Snapshot,
    file: SourceFile,
    map: &LineMap,
    params: DocumentFormattingParams,
    indent: Option<stir_neon::phpstan::FormatIndent>,
) -> Option<Vec<TextEdit>> {
    let options = format_options(indent, &params.options);
    // a file that doesn't parse isn't touched: it's probably being typed
    let edits = stir_fmt::edits(file.text(s.db), &options).ok()?;
    Some(text_edits(map, edits))
}

pub fn range_formatting(
    s: &Snapshot,
    file: SourceFile,
    map: &LineMap,
    params: DocumentRangeFormattingParams,
    indent: Option<stir_neon::phpstan::FormatIndent>,
) -> Option<Vec<TextEdit>> {
    let options = format_options(indent, &params.options);
    let edits = stir_fmt::range_edits(file.text(s.db), map.text_range(params.range), &options);
    Some(text_edits(map, edits.ok()?))
}

pub fn signature_help(
    s: &Snapshot,
    file: SourceFile,
    map: &LineMap,
    params: SignatureHelpParams,
) -> Option<SignatureHelp> {
    let pos = params.text_document_position_params.position;
    let h = hints::signature_help(s.db, s.ws, file, map.offset(pos))?;
    let parameters = h
        .params
        .iter()
        .map(|&(s, e)| {
            let (s, e) = (s as usize, e as usize);
            ParameterInformation {
                label: ParameterLabel::LabelOffsets([
                    map.measure(&h.label[..s]),
                    map.measure(&h.label[..e]),
                ]),
                documentation: None,
            }
        })
        .collect();
    Some(SignatureHelp {
        signatures: vec![SignatureInformation {
            label: h.label,
            documentation: h.doc.map(|d| {
                Documentation::MarkupContent(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: d,
                })
            }),
            parameters: Some(parameters),
            active_parameter: Some(h.active_param),
        }],
        active_signature: Some(0),
        active_parameter: Some(h.active_param),
    })
}

pub fn document_symbol_response(
    s: &Snapshot,
    file: SourceFile,
    map: &LineMap,
    _: DocumentSymbolParams,
) -> Option<DocumentSymbolResponse> {
    let syms = symbols::document_symbols(s.db, file);
    Some(DocumentSymbolResponse::Nested(
        syms.into_iter().map(|s| document_symbol(s, map)).collect(),
    ))
}

pub fn semantic_tokens(
    s: &Snapshot,
    file: SourceFile,
    map: &LineMap,
    _: SemanticTokensParams,
) -> Option<SemanticTokensResult> {
    let text = file.text(s.db);
    let mut data = Vec::new();
    let (mut prev_line, mut prev_col) = (0u32, 0u32);
    for t in tokens::semantic_tokens(s.db, s.ws, file) {
        // tokens can't span lines
        for line in map.lines(t.range) {
            let piece = line.intersect(t.range).unwrap_or(line);
            let s = &text[usize::from(piece.start())..usize::from(piece.end())];
            let s = s.trim_end_matches(['\n', '\r']);
            if s.is_empty() {
                continue;
            }
            let pos = map.position(piece.start());
            let delta_line = pos.line - prev_line;
            let delta_start = if delta_line == 0 {
                pos.character - prev_col
            } else {
                pos.character
            };
            data.push(SemanticToken {
                delta_line,
                delta_start,
                length: map.measure(s),
                token_type: t.ty as u32,
                token_modifiers_bitset: t.modifiers,
            });
            prev_line = pos.line;
            prev_col = pos.character;
        }
    }
    Some(SemanticTokensResult::Tokens(SemanticTokens {
        result_id: None,
        data,
    }))
}

pub fn inlay_hints(
    s: &Snapshot,
    file: SourceFile,
    map: &LineMap,
    params: InlayHintParams,
) -> Option<Vec<InlayHint>> {
    let r = map.text_range(params.range);
    Some(
        hints::inlay_hints(s.db, s.ws, file, r)
            .into_iter()
            .map(|h| InlayHint {
                position: map.position(h.offset),
                label: InlayHintLabel::String(h.label),
                kind: Some(InlayHintKind::TYPE),
                text_edits: None,
                tooltip: None,
                padding_left: None,
                padding_right: None,
                data: None,
            })
            .collect(),
    )
}

pub fn folding_ranges(
    s: &Snapshot,
    file: SourceFile,
    map: &LineMap,
    _: FoldingRangeParams,
) -> Option<Vec<FoldingRange>> {
    Some(
        symbols::folding_ranges(s.db, file)
            .into_iter()
            .filter_map(|(r, kind)| {
                let start = map.position(r.start());
                let end = map.position(r.end());
                (end.line > start.line).then(|| FoldingRange {
                    start_line: start.line,
                    end_line: end.line,
                    kind: Some(match kind {
                        symbols::FoldKind::Region => FoldingRangeKind::Region,
                        symbols::FoldKind::Comment => FoldingRangeKind::Comment,
                        symbols::FoldKind::Imports => FoldingRangeKind::Imports,
                    }),
                    ..Default::default()
                })
            })
            .collect(),
    )
}

pub fn selection_ranges(
    s: &Snapshot,
    file: SourceFile,
    map: &LineMap,
    params: SelectionRangeParams,
) -> Option<Vec<SelectionRange>> {
    Some(
        params
            .positions
            .iter()
            .map(|p| {
                let ranges = symbols::selection_ranges(s.db, file, map.offset(*p));
                let mut sel: Option<SelectionRange> = None;
                for r in ranges.into_iter().rev() {
                    sel = Some(SelectionRange {
                        range: map.range(r),
                        parent: sel.map(Box::new),
                    });
                }
                sel.unwrap_or(SelectionRange {
                    range: Range::new(*p, *p),
                    parent: None,
                })
            })
            .collect(),
    )
}

// Requests about the whole workspace.

pub fn incoming_calls(
    s: &Snapshot,
    params: CallHierarchyIncomingCallsParams,
) -> Option<Vec<CallHierarchyIncomingCall>> {
    let (file, offset) = from_hierarchy_data(s, params.item.data.as_ref())?;
    let (def, _) = hierarchy::callable_at(s.db, s.ws, file, offset)?;
    let calls = hierarchy::incoming_calls(s.db, s.ws, &def)
        .into_iter()
        .filter_map(|(caller, ranges)| {
            let map = LineMap::new(caller.file.text(s.db), s.enc);
            Some(CallHierarchyIncomingCall {
                from: call_item(s, &caller)?,
                from_ranges: ranges.into_iter().map(|r| map.range(r)).collect(),
            })
        })
        .collect();
    Some(calls)
}

pub fn outgoing_calls(
    s: &Snapshot,
    params: CallHierarchyOutgoingCallsParams,
) -> Option<Vec<CallHierarchyOutgoingCall>> {
    let data = params.item.data;
    let (file, _) = from_hierarchy_data(s, data.as_ref())?;
    let at = |key: &str| {
        data.as_ref()?
            .get(key)?
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
    };
    let range = TextRange::new(at("start")?.into(), at("end")?.into());
    let map = LineMap::new(file.text(s.db), s.enc);
    let calls = hierarchy::outgoing_calls(s.db, s.ws, file, range)
        .into_iter()
        .filter_map(|(callee, ranges)| {
            Some(CallHierarchyOutgoingCall {
                to: call_item(s, &callee)?,
                from_ranges: ranges.into_iter().map(|r| map.range(r)).collect(),
            })
        })
        .collect();
    Some(calls)
}

pub fn supertypes(
    s: &Snapshot,
    params: TypeHierarchySupertypesParams,
) -> Option<Vec<TypeHierarchyItem>> {
    let key = type_key(&params.item)?;
    let items = hierarchy::supertypes(s.db, s.ws, &key);
    Some(
        items
            .iter()
            .filter_map(|i| type_hierarchy_item(s, i))
            .collect(),
    )
}

pub fn subtypes(
    s: &Snapshot,
    params: TypeHierarchySubtypesParams,
) -> Option<Vec<TypeHierarchyItem>> {
    let key = type_key(&params.item)?;
    let items = hierarchy::subtypes(s.db, s.ws, &key);
    Some(
        items
            .iter()
            .filter_map(|i| type_hierarchy_item(s, i))
            .collect(),
    )
}

pub fn workspace_symbols(
    s: &Snapshot,
    params: WorkspaceSymbolParams,
) -> Option<WorkspaceSymbolResponse> {
    #[allow(deprecated)]
    let out = symbols::workspace_symbols(s.db, s.ws, &params.query)
        .into_iter()
        .filter_map(|sym| {
            Some(SymbolInformation {
                name: sym.name,
                kind: symbol_kind(sym.kind),
                tags: sym.deprecated.then(|| vec![SymbolTag::DEPRECATED]),
                deprecated: sym.deprecated.then_some(true),
                location: s.location(sym.file, sym.range)?,
                container_name: (!sym.container.is_empty()).then_some(sym.container),
            })
        })
        .collect();
    Some(WorkspaceSymbolResponse::Flat(out))
}
