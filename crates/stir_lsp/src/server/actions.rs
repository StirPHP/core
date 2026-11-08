//! `textDocument/codeAction` and its `resolve`, and the commands that go with them, on the native
//! server. What's answered is in [`crate::handlers::actions`].

use tower_lsp_server::jsonrpc::Result;
use tower_lsp_server::ls_types::*;

use super::{Server, content_modified};
use crate::handlers::actions::{self, Resolved, ToResolve};

impl Server {
    pub(super) async fn actions(
        &self,
        params: CodeActionParams,
    ) -> Result<Option<CodeActionResponse>> {
        let support = *self.action_support.lock().unwrap();
        let versions = self.state.lock().unwrap().open.clone();
        let uri = params.text_document.uri.clone();
        self.read(&uri, move |s, file, map| {
            actions::code_actions(s, file, map, params, support, &versions)
        })
        .await
    }

    pub(super) async fn resolve_action(&self, mut action: CodeAction) -> Result<CodeAction> {
        let Some(what) = ToResolve::of(&action) else {
            return Ok(action);
        };
        let support = *self.action_support.lock().unwrap();
        let versions = self.state.lock().unwrap().open.clone();
        let uri = what.uri.clone();
        let result = self
            .read(&uri, move |s, file, _| {
                Some(actions::resolve(s, file, what, support, &versions))
            })
            .await?;
        match result {
            Some(Resolved::Done(edit, command)) => {
                action.edit = edit;
                action.command = command;
                Ok(action)
            }
            Some(Resolved::Stale) => Err(content_modified()),
            Some(Resolved::Refused(reason)) => {
                self.client
                    .show_message(MessageType::WARNING, format!("StirPHP: {reason}"))
                    .await;
                Ok(action)
            }
            None => Ok(action),
        }
    }

    pub(super) async fn run_command(&self, params: ExecuteCommandParams) -> Result<Option<LSPAny>> {
        let Some((uri, request)) = actions::command_request(&params) else {
            return Ok(None);
        };
        let support = *self.action_support.lock().unwrap();
        let versions = self.state.lock().unwrap().open.clone();
        let result = self
            .read(&uri, move |s, file, _| {
                actions::add_parameter(s, file, &request, support, &versions)
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
