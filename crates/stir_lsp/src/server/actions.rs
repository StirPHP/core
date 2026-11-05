//! `textDocument/codeAction` and its `resolve`, and the commands that go with them. What they do
//! is in [`crate::actions`]; this is the protocol's side of it.

use std::hash::Hasher as _;

use rustc_hash::{FxHashMap, FxHasher};
use serde_json::{Value, json};
use stir_db::{RootDatabase, SourceFile};
use stir_syntax::TextRange;
use tower_lsp_server::jsonrpc::Result;
use tower_lsp_server::ls_types::*;

use super::{Server, content_modified, uri_path};
use crate::actions::protocol::{self, EditSupport};
use crate::actions::{self, ClientAbilities, Cx, ForeignDiagnostic};
use crate::convert::{Encoding, LineMap};

/// The kinds of actions this has, for the capabilities.
pub(super) const KINDS: [&str; 6] = [
    "quickfix",
    "refactor.extract",
    "refactor.inline",
    "refactor.rewrite",
    "source.organizeImports",
    "source.fixAll",
];

/// What a client can do with code actions.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct ActionSupport {
    pub literals: bool,
    pub resolve: bool,
    pub disabled: bool,
    pub edits: EditSupport,
    pub rename_command: bool,
    pub abilities: ClientAbilities,
}

impl ActionSupport {
    pub(super) fn read(params: &InitializeParams) -> ActionSupport {
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
            literals: code_action.is_some_and(|c| c.code_action_literal_support.is_some()),
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
pub(super) const ASK_PARAMETER_COMMAND: &str = "stirphp.askParameter";
/// A command of ours: add the parameter the person said.
pub(super) const ADD_PARAMETER_COMMAND: &str = "stirphp.addParameter";

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

fn edit_of(
    db: &RootDatabase,
    change: &actions::Change,
    enc: Encoding,
    support: ActionSupport,
    versions: &FxHashMap<std::path::PathBuf, i32>,
) -> Option<WorkspaceEdit> {
    protocol::workspace_edit(
        db,
        change,
        enc,
        support.edits,
        &|f: SourceFile| Uri::from_file_path(f.path(db)),
        &|f: SourceFile| versions.get(f.path(db)).copied(),
    )
}

/// The command that renames what a change wants renamed, for a client that can.
fn rename_command(
    db: &RootDatabase,
    change: &actions::Change,
    enc: Encoding,
    support: ActionSupport,
) -> Option<Command> {
    if !support.rename_command {
        return None;
    }
    let (file, at) = protocol::rename_position(db, change, enc)?;
    let uri = Uri::from_file_path(file.path(db))?;
    Some(Command {
        title: "Rename".into(),
        command: RENAME_COMMAND.into(),
        arguments: Some(vec![json!(uri.to_string()), json!(at.line), json!(at.character)]),
    })
}

impl Server {
    pub(super) async fn actions(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        let support = *self.action_support.lock().unwrap();
        let versions = self.state.lock().unwrap().open.clone();
        let enc = self.encoding();
        let uri = params.text_document.uri;
        let range = params.range;
        let context = params.context;
        let only: Option<Vec<String>> = context
            .only
            .as_ref()
            .filter(|o| !o.is_empty())
            .map(|o| o.iter().map(|k| k.as_str().to_owned()).collect());
        let shown = context.diagnostics;
        let data_uri = uri.to_string();
        let offered = self
            .read(&uri, move |db, ws, file, map| {
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
                            action.edit = edit_of(db, &change, enc, support, &versions);
                            action.command = rename_command(db, &change, enc, support);
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
                                arguments: Some(vec![
                                    json!(data_uri),
                                    json!(u32::from(at)),
                                    json!(hash),
                                ]),
                            }),
                            ..Default::default()
                        })
                    }));
                }
                Some(out)
            })
            .await?;
        Ok(offered.filter(|o| !o.is_empty()))
    }

    pub(super) async fn resolve_action(&self, mut action: CodeAction) -> Result<CodeAction> {
        let Some(data) = action.data.clone() else {
            return Ok(action);
        };
        let Some(uri) = data["uri"].as_str().and_then(|u| u.parse::<Uri>().ok()) else {
            return Ok(action);
        };
        let number = |key: &str| data[key].as_u64().and_then(|v| u32::try_from(v).ok());
        let (Some(start), Some(end)) = (number("start"), number("end")) else {
            return Ok(action);
        };
        let id = data["id"].as_str().unwrap_or_default().to_owned();
        let kind = data["kind"].as_str().unwrap_or_default().to_owned();
        let hash = data["hash"].as_str().unwrap_or_default().to_owned();
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
        let support = *self.action_support.lock().unwrap();
        let versions = self.state.lock().unwrap().open.clone();
        let enc = self.encoding();
        let result = self
            .read(&uri, move |db, ws, file, _| {
                if text_hash(file.text(db)) != hash {
                    return Some(Err(None));
                }
                let range = TextRange::new(start.into(), end.max(start).into());
                let cx = Cx::new(
                    db,
                    ws,
                    file,
                    range,
                    &foreign,
                    Some(vec![kind]),
                    support.abilities,
                );
                match actions::resolve(&cx, &id) {
                    Ok(change) => Some(Ok((
                        edit_of(db, &change, enc, support, &versions),
                        rename_command(db, &change, enc, support),
                    ))),
                    Err(reason) => Some(Err(Some(reason))),
                }
            })
            .await?;
        match result {
            Some(Ok((edit, command))) => {
                action.edit = edit;
                action.command = command;
                Ok(action)
            }
            Some(Err(None)) => Err(content_modified()),
            // nothing's been done, and the person is told why: a failed request wouldn't say
            Some(Err(Some(reason))) => {
                self.client
                    .show_message(MessageType::WARNING, format!("StirPHP: {reason}"))
                    .await;
                Ok(action)
            }
            None => Ok(action),
        }
    }

    pub(super) async fn run_command(&self, params: ExecuteCommandParams) -> Result<Option<LSPAny>> {
        if params.command != ADD_PARAMETER_COMMAND {
            return Ok(None);
        }
        let Some(request) = params.arguments.first().cloned() else {
            return Ok(None);
        };
        let Some(uri) = request["uri"].as_str().and_then(|u| u.parse::<Uri>().ok()) else {
            return Ok(None);
        };
        let _ = uri_path(&uri);
        let support = *self.action_support.lock().unwrap();
        let versions = self.state.lock().unwrap().open.clone();
        let enc = self.encoding();
        let result = self
            .read(&uri, move |db, ws, file, _| {
                let at = u32::try_from(request["offset"].as_u64()?).ok()?;
                if text_hash(file.text(db)) != request["hash"].as_str()? {
                    return Some(Err("The file changed meanwhile.".to_owned()));
                }
                let parameter = actions::NewParameter {
                    declared: request["parameter"].as_str()?.to_owned(),
                    passed: request["passed"].as_str().map(str::to_owned),
                };
                Some(
                    actions::add_parameter_change(db, ws, file, at.into(), &parameter)
                        .and_then(|c| actions::finish(db, c))
                        .map(|c| edit_of(db, &c, enc, support, &versions)),
                )
            })
            .await?;
        match result {
            Some(Ok(Some(edit))) => {
                if let Err(error) = self.client.apply_edit(edit).await {
                    tracing::warn!(%error, "the editor didn't take an edit");
                }
            }
            Some(Err(reason)) => {
                self.client
                    .show_message(MessageType::WARNING, format!("StirPHP: {reason}"))
                    .await;
            }
            _ => {}
        }
        Ok(None)
    }
}
