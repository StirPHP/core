//! `textDocument/codeAction` and its `resolve`, and the commands that go with them. What they do
//! is in [`crate::actions`]; this is the protocol's side of it.

use std::hash::Hasher as _;

use std::path::PathBuf;

use ls_types::*;
use rustc_hash::{FxHashMap, FxHasher};
use serde_json::{Value, json};
use stir_db::SourceFile;
use stir_syntax::TextRange;

use super::Snapshot;
use crate::actions::protocol::{self, EditSupport};
use crate::actions::{self, ClientAbilities, Cx, ForeignDiagnostic};
use crate::convert::LineMap;

/// The kinds of actions this has, for the capabilities.
pub const KINDS: [&str; 6] = [
    "quickfix",
    "refactor.extract",
    "refactor.inline",
    "refactor.rewrite",
    "source.organizeImports",
    "source.fixAll",
];

/// What a client can do with code actions.
#[derive(Debug, Clone, Copy, Default)]
pub struct ActionSupport {
    pub resolve: bool,
    pub disabled: bool,
    pub edits: EditSupport,
    pub rename_command: bool,
    pub abilities: ClientAbilities,
}

impl ActionSupport {
    pub fn read(params: &InitializeParams) -> ActionSupport {
        let caps = &params.capabilities;
        let code_action = caps
            .text_document
            .as_ref()
            .and_then(|t| t.code_action.as_ref());
        let resolve = code_action.is_some_and(|c| {
            c.data_support == Some(true)
                && c.resolve_support
                    .as_ref()
                    .is_some_and(|r| r.properties.iter().any(|p| p == "edit"))
        });
        let document_changes = caps
            .workspace
            .as_ref()
            .and_then(|w| w.workspace_edit.as_ref())
            .and_then(|e| e.document_changes)
            == Some(true);
        // what an editor's own extension adds to it, which the protocol has no word for
        let commands: Vec<&str> = params
            .initialization_options
            .as_ref()
            .and_then(|o| o.get("clientCommands"))
            .and_then(Value::as_array)
            .map(|c| c.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        ActionSupport {
            resolve,
            disabled: code_action.and_then(|c| c.disabled_support) == Some(true),
            edits: EditSupport {
                document_changes,
                encoding_utf8: false,
            },
            rename_command: commands.contains(&RENAME_COMMAND),
            abilities: ClientAbilities {
                asks_for_parameters: commands.contains(&ASK_PARAMETER_COMMAND),
            },
        }
    }
}

/// A command of the editor's: rename what's at a place, once an edit has been made.
const RENAME_COMMAND: &str = "stirphp.renameAt";
/// A command of the editor's: ask what parameter to add, then call [`ADD_PARAMETER_COMMAND`].
pub const ASK_PARAMETER_COMMAND: &str = "stirphp.askParameter";
/// A command of ours: add the parameter the person said.
pub const ADD_PARAMETER_COMMAND: &str = "stirphp.addParameter";

fn text_hash(text: &str) -> String {
    let mut hasher = FxHasher::default();
    hasher.write(text.as_bytes());
    format!("{:x}", hasher.finish())
}

fn foreign_of(diagnostics: &[Diagnostic], map: &LineMap) -> Vec<ForeignDiagnostic> {
    diagnostics
        .iter()
        .filter(|d| d.source.as_deref() == Some("phpstan"))
        .filter_map(|d| {
            let NumberOrString::String(code) = d.code.as_ref()? else {
                return None;
            };
            Some(ForeignDiagnostic {
                range: map.text_range(d.range),
                code: code.clone(),
            })
        })
        .collect()
}

/// The documents open in the editor and their versions, for the edits of a client that wants them.
pub type Versions = FxHashMap<PathBuf, i32>;

fn edit_of(
    s: &Snapshot,
    change: &actions::Change,
    support: ActionSupport,
    versions: &Versions,
) -> Option<WorkspaceEdit> {
    let db = s.db;
    protocol::workspace_edit(
        db,
        change,
        s.enc,
        support.edits,
        &|f: SourceFile| s.links.uri(db, f),
        &|f: SourceFile| versions.get(f.path(db)).copied(),
    )
}

/// The command that renames what a change wants renamed, for a client that can.
fn rename_command(
    s: &Snapshot,
    change: &actions::Change,
    support: ActionSupport,
) -> Option<Command> {
    if !support.rename_command {
        return None;
    }
    let (file, at) = protocol::rename_position(s.db, change, s.enc)?;
    let uri = s.links.uri(s.db, file)?;
    Some(Command {
        title: "Rename".into(),
        command: RENAME_COMMAND.into(),
        arguments: Some(vec![
            json!(uri.to_string()),
            json!(at.line),
            json!(at.character),
        ]),
    })
}

/// The actions offered for `params`, in `file`.
pub fn code_actions(
    s: &Snapshot,
    file: SourceFile,
    map: &LineMap,
    params: CodeActionParams,
    support: ActionSupport,
    versions: &Versions,
) -> Option<CodeActionResponse> {
    let (db, ws) = (s.db, s.ws);
    let range = params.range;
    let context = params.context;
    let only: Option<Vec<String>> = context
        .only
        .as_ref()
        .filter(|o| !o.is_empty())
        .map(|o| o.iter().map(|k| k.as_str().to_owned()).collect());
    let shown = context.diagnostics;
    let data_uri = params.text_document.uri.to_string();
    let foreign = foreign_of(&shown, map);
    let selected = map.text_range(range);
    let cx = Cx::new(db, ws, file, selected, &foreign, only, support.abilities);
    let hash = text_hash(file.text(db));
    let mut out: Vec<CodeActionOrCommand> = Vec::new();
    for offer in actions::offers(&cx) {
        let Some(mut action) = protocol::code_action(&offer, support.disabled) else {
            continue;
        };
        action.diagnostics = Some(
            shown
                .iter()
                .filter(|d| {
                    offer.fixes.iter().any(|(r, code)| {
                        map.range(*r) == d.range
                            && d.code == Some(NumberOrString::String(code.clone()))
                    })
                })
                .cloned()
                .collect(),
        )
        .filter(|d: &Vec<Diagnostic>| !d.is_empty());
        if offer.disabled.is_none() {
            if support.resolve {
                action.data = Some(json!({
                    "uri": data_uri,
                    "hash": hash,
                    "start": u32::from(selected.start()),
                    "end": u32::from(selected.end()),
                    "id": offer.id,
                    "kind": offer.kind.as_str(),
                    "foreign": foreign.iter().map(|f| json!({
                        "start": u32::from(f.range.start()),
                        "end": u32::from(f.range.end()),
                        "code": f.code,
                    })).collect::<Vec<_>>(),
                }));
            } else if offer.costly {
                // a client that can't ask later isn't made to wait for this every time
                continue;
            } else {
                let change = (offer.build)().and_then(|c| actions::finish(db, c));
                let Ok(change) = change else { continue };
                action.edit = edit_of(s, &change, support, versions);
                action.command = rename_command(s, &change, support);
            }
        }
        out.push(CodeActionOrCommand::CodeAction(action));
    }
    if support.abilities.asks_for_parameters {
        out.extend(actions::ask_parameter_at(&cx).map(|at| {
            CodeActionOrCommand::CodeAction(CodeAction {
                title: "Add parameter...".into(),
                kind: Some(CodeActionKind::REFACTOR_REWRITE),
                command: Some(Command {
                    title: "Add parameter".into(),
                    command: ASK_PARAMETER_COMMAND.into(),
                    arguments: Some(vec![json!(data_uri), json!(u32::from(at)), json!(hash)]),
                }),
                ..Default::default()
            })
        }));
    }
    (!out.is_empty()).then_some(out)
}

/// What an action to resolve says about itself (its `data`).
pub struct ToResolve {
    pub uri: Uri,
    start: u32,
    end: u32,
    id: String,
    kind: String,
    hash: String,
    foreign: Vec<ForeignDiagnostic>,
}

impl ToResolve {
    pub fn of(action: &CodeAction) -> Option<ToResolve> {
        let data = action.data.as_ref()?;
        let uri = data["uri"].as_str().and_then(|u| u.parse::<Uri>().ok())?;
        let number = |key: &str| data[key].as_u64().and_then(|v| u32::try_from(v).ok());
        let (start, end) = (number("start")?, number("end")?);
        let foreign: Vec<ForeignDiagnostic> = data["foreign"]
            .as_array()
            .map(|f| {
                f.iter()
                    .filter_map(|d| {
                        Some(ForeignDiagnostic {
                            range: TextRange::new(
                                u32::try_from(d["start"].as_u64()?).ok()?.into(),
                                u32::try_from(d["end"].as_u64()?).ok()?.into(),
                            ),
                            code: d["code"].as_str()?.to_owned(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(ToResolve {
            uri,
            start,
            end,
            id: data["id"].as_str().unwrap_or_default().to_owned(),
            kind: data["kind"].as_str().unwrap_or_default().to_owned(),
            hash: data["hash"].as_str().unwrap_or_default().to_owned(),
            foreign,
        })
    }
}

/// How resolving an action went.
pub enum Resolved {
    /// Its edit, and the command to run after it.
    Done(Option<WorkspaceEdit>, Option<Command>),
    /// The document changed since the action was offered.
    Stale,
    /// It can't be done, and the person's told why: a failed request wouldn't say.
    Refused(String),
}

pub fn resolve(
    s: &Snapshot,
    file: SourceFile,
    what: ToResolve,
    support: ActionSupport,
    versions: &Versions,
) -> Resolved {
    if text_hash(file.text(s.db)) != what.hash {
        return Resolved::Stale;
    }
    let range = TextRange::new(what.start.into(), what.end.max(what.start).into());
    let cx = Cx::new(
        s.db,
        s.ws,
        file,
        range,
        &what.foreign,
        Some(vec![what.kind]),
        support.abilities,
    );
    match actions::resolve(&cx, &what.id) {
        Ok(change) => Resolved::Done(
            edit_of(s, &change, support, versions),
            rename_command(s, &change, support),
        ),
        Err(reason) => Resolved::Refused(reason),
    }
}

/// The document an `executeCommand` of ours is about, and what it asks.
pub fn command_request(params: &ExecuteCommandParams) -> Option<(Uri, Value)> {
    if params.command != ADD_PARAMETER_COMMAND {
        return None;
    }
    let request = params.arguments.first().cloned()?;
    let uri = request["uri"]
        .as_str()
        .and_then(|u| u.parse::<Uri>().ok())?;
    Some((uri, request))
}

/// The edit that adds the parameter `request` says, for the client to apply, or why it can't.
pub fn add_parameter(
    s: &Snapshot,
    file: SourceFile,
    request: &Value,
    support: ActionSupport,
    versions: &Versions,
) -> Option<Result<Option<WorkspaceEdit>, String>> {
    let at = u32::try_from(request["offset"].as_u64()?).ok()?;
    if text_hash(file.text(s.db)) != request["hash"].as_str()? {
        return Some(Err("The file changed meanwhile.".to_owned()));
    }
    let parameter = actions::NewParameter {
        declared: request["parameter"].as_str()?.to_owned(),
        passed: request["passed"].as_str().map(str::to_owned),
    };
    Some(
        actions::add_parameter_change(s.db, s.ws, file, at.into(), &parameter)
            .and_then(|c| actions::finish(s.db, c))
            .map(|c| edit_of(s, &c, support, versions)),
    )
}
