//! A [`Change`] and an [`Offer`] in the protocol's terms, so that a client of any kind gets the
//! same from them.

use std::collections::HashMap;

use ls_types::{
    AnnotatedTextEdit, CodeAction, CodeActionDisabled, CodeActionKind, DocumentChangeOperation,
    DocumentChanges, OneOf, OptionalVersionedTextDocumentIdentifier, Position, TextDocumentEdit,
    TextEdit, Uri, WorkspaceEdit,
};
use stir_db::{Db, SourceFile};

use super::{Change, Kind, Offer};
use crate::convert::{Encoding, LineMap};

/// What the client said it can do with workspace edits and code actions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EditSupport {
    pub document_changes: bool,
    pub encoding_utf8: bool,
}

pub fn kind(kind: Kind) -> CodeActionKind {
    CodeActionKind::new(kind.as_str())
}

/// The workspace edit of `change`. `uri` gives a file's URI and `version` its version if it's
/// open in the editor; a file that isn't gets no version, as the protocol has it.
pub fn workspace_edit(
    db: &dyn Db,
    change: &Change,
    enc: Encoding,
    support: EditSupport,
    uri: &dyn Fn(SourceFile) -> Option<Uri>,
    version: &dyn Fn(SourceFile) -> Option<i32>,
) -> Option<WorkspaceEdit> {
    let mut files: Vec<SourceFile> = Vec::new();
    for e in &change.edits {
        if !files.contains(&e.file) {
            files.push(e.file);
        }
    }
    let mut documents = Vec::new();
    let mut changes: HashMap<Uri, Vec<TextEdit>> = HashMap::new();
    for file in files {
        let map = LineMap::new(file.text(db), enc);
        let mut edits: Vec<&super::FileEdit> =
            change.edits.iter().filter(|e| e.file == file).collect();
        edits.sort_by_key(|e| (e.range.start(), e.range.end()));
        let lsp: Vec<TextEdit> = edits
            .into_iter()
            .map(|e| TextEdit {
                range: map.range(e.range),
                new_text: e.text.clone(),
            })
            .collect();
        let target = uri(file)?;
        if support.document_changes {
            documents.push(TextDocumentEdit {
                text_document: OptionalVersionedTextDocumentIdentifier {
                    uri: target,
                    version: version(file),
                },
                edits: lsp
                    .into_iter()
                    .map(OneOf::Left::<TextEdit, AnnotatedTextEdit>)
                    .collect(),
            });
        } else {
            changes.insert(target, lsp);
        }
    }
    Some(if support.document_changes {
        WorkspaceEdit {
            document_changes: Some(DocumentChanges::Operations(
                documents
                    .into_iter()
                    .map(DocumentChangeOperation::Edit)
                    .collect(),
            )),
            ..Default::default()
        }
    } else {
        WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }
    })
}

/// Where the first file of `change` has the rename it wants, once it's been edited.
pub fn rename_position(
    db: &dyn Db,
    change: &Change,
    enc: Encoding,
) -> Option<(SourceFile, Position)> {
    let at = change.rename_at?;
    let file = change.edits.first()?.file;
    let text = super::apply(file.text(db), change, file).ok()?;
    Some((file, LineMap::new(&text, enc).position(at)))
}

/// The action of `offer`, without its edit.
pub fn code_action(offer: &Offer<'_>, disabled_support: bool) -> Option<CodeAction> {
    if offer.disabled.is_some() && !disabled_support {
        return None;
    }
    Some(CodeAction {
        title: offer.title.clone(),
        kind: Some(kind(offer.kind)),
        is_preferred: offer.preferred.then_some(true),
        disabled: offer.disabled.as_ref().map(|reason| CodeActionDisabled {
            reason: reason.clone(),
        }),
        ..Default::default()
    })
}
