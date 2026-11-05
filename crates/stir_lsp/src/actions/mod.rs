//! Quick fixes and refactorings: what can be done at a place in a file, and the edits that does.
//!
//! This is plain functions over the database, with nothing of the protocol's or of a runtime's in
//! the computation, so that it also works in the browser. A request has two steps, as code actions
//! have over the protocol: [`offers`] says what can be done, which has to stay fast because an
//! editor asks each time the cursor moves, and an [`Offer`]'s `build` works out the edits when one
//! is chosen. [`resolve`] finds the offer again from its `id`, on the same text.
//!
//! Nothing is offered on a file with syntax errors: it's being typed, and what the tree says of it
//! can't be relied on. A [`Change`] that wouldn't leave every file it touches parsing is an error,
//! whatever produced it, rather than an edit.

use std::cell::OnceCell;

use stir_db::{Db, Diagnostic, FileOrigin, PhpVersions, SourceFile, Workspace};
use stir_syntax::{SyntaxKind, SyntaxNode, SyntaxToken, TextRange, TextSize};

mod classes;
mod docs;
mod extract;
mod flow;
mod fix_members;
mod foreign;
mod hierarchy;
mod fix_names;
mod fix_types;
mod members;
mod organize;
mod reindent;
pub mod protocol;
mod signature;
mod style;
mod suppress;
mod types;
mod variables;

pub use organize::unused_import_hints;
pub use signature::{NewParameter, add_parameter_change, ask_parameter_at};
pub use style::Style;

/// What kind of an action it is, as the protocol has them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    QuickFix,
    RefactorExtract,
    RefactorInline,
    RefactorRewrite,
    OrganizeImports,
    FixAll,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::QuickFix => "quickfix",
            Kind::RefactorExtract => "refactor.extract",
            Kind::RefactorInline => "refactor.inline",
            Kind::RefactorRewrite => "refactor.rewrite",
            Kind::OrganizeImports => "source.organizeImports",
            Kind::FixAll => "source.fixAll",
        }
    }
}

/// One edit of one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEdit {
    pub file: SourceFile,
    pub range: TextRange,
    pub text: String,
}

/// What choosing an action does.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Change {
    pub edits: Vec<FileEdit>,
    /// A name the person is likely to want to change, where it is once the edits are done, in
    /// the file of the first edit.
    pub rename_at: Option<TextSize>,
}

impl Change {
    pub fn new(edits: Vec<FileEdit>) -> Change {
        Change {
            edits,
            rename_at: None,
        }
    }
}

/// The reason nothing can be done, for the person who asked.
pub type Refusal = String;
/// Works out the edits of an offer.
pub type Build<'c> = Box<dyn FnOnce() -> Result<Change, Refusal> + 'c>;

/// An action that can be chosen.
pub struct Offer<'c> {
    /// What identifies it among the offers for this text and range.
    pub id: String,
    pub title: String,
    pub kind: Kind,
    /// The one obvious fix of what it fixes.
    pub preferred: bool,
    /// What it fixes: the range and code of a diagnostic.
    pub fixes: Vec<(TextRange, String)>,
    /// It can't be done here, and this is why. An editor that can shows it greyed out.
    pub disabled: Option<String>,
    /// The search takes the whole workspace: not for a client that can't ask for it later.
    pub costly: bool,
    pub build: Build<'c>,
}

impl<'c> Offer<'c> {
    pub fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        kind: Kind,
        build: impl FnOnce() -> Result<Change, Refusal> + 'c,
    ) -> Offer<'c> {
        Offer {
            id: id.into(),
            title: title.into(),
            kind,
            preferred: false,
            fixes: Vec::new(),
            disabled: None,
            costly: false,
            build: Box::new(build),
        }
    }

    /// Something that can't be done, saying why.
    pub fn refused(
        id: impl Into<String>,
        title: impl Into<String>,
        kind: Kind,
        reason: impl Into<String>,
    ) -> Offer<'c> {
        let reason: String = reason.into();
        let why = reason.clone();
        let mut offer = Offer::new(id, title, kind, move || Err(why));
        offer.disabled = Some(reason);
        offer
    }

    pub fn fixing(mut self, range: TextRange, code: &str) -> Self {
        self.fixes.push((range, code.to_owned()));
        self
    }

    pub fn preferred(mut self) -> Self {
        self.preferred = true;
        self
    }

    pub fn costly(mut self) -> Self {
        self.costly = true;
        self
    }
}

/// A diagnostic of someone else's that the editor is showing here: only PHPStan's so far, which
/// only have a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignDiagnostic {
    pub range: TextRange,
    pub code: String,
}

/// What a client can do beyond the protocol's basics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ClientAbilities {
    /// It can ask for what a new parameter is: see [`signature`].
    pub asks_for_parameters: bool,
}

/// A problem at a place in the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// What it's about.
    pub range: TextRange,
    /// Where the editor shows it, which for what only has a line is the whole line.
    pub shown: TextRange,
    pub code: String,
}

/// What every provider of offers looks at.
pub struct Cx<'c> {
    pub db: &'c dyn Db,
    pub ws: Workspace,
    pub file: SourceFile,
    pub text: &'c str,
    pub root: SyntaxNode,
    /// What's selected, without the whitespace around it. Empty for a cursor.
    pub range: TextRange,
    /// The selection as it came.
    pub raw_range: TextRange,
    pub style: Style,
    /// The versions of PHP this file has to run on.
    pub versions: PhpVersions,
    pub foreign: &'c [ForeignDiagnostic],
    pub abilities: ClientAbilities,
    only: Option<Vec<String>>,
    ctx: OnceCell<stir_checker::Ctx<'c>>,
}

impl<'c> Cx<'c> {
    pub fn new(
        db: &'c dyn Db,
        ws: Workspace,
        file: SourceFile,
        range: TextRange,
        foreign: &'c [ForeignDiagnostic],
        only: Option<Vec<String>>,
        abilities: ClientAbilities,
    ) -> Cx<'c> {
        let text = file.text(db).as_str();
        let range = TextRange::new(
            range.start().min(TextSize::of(text)),
            range.end().min(TextSize::of(text)),
        );
        let versions = stir_checker::php_version::file_versions(text, ws.settings(db).php_version);
        Cx {
            db,
            ws,
            file,
            text,
            root: stir_db::parse(db, file).syntax(),
            range: trim_range(text, range),
            raw_range: range,
            style: Style::of(text),
            versions,
            foreign,
            abilities,
            only,
            ctx: OnceCell::new(),
        }
    }

    pub fn ctx(&self) -> &stir_checker::Ctx<'c> {
        self.ctx
            .get_or_init(|| stir_checker::Ctx::new(self.db, self.ws))
    }

    pub fn typed(&self) -> &'c stir_checker::TypedFile {
        stir_checker::typed_file(self.db, self.ws, self.file)
    }

    /// The offsets of the file as text.
    pub fn slice(&self, range: TextRange) -> &'c str {
        &self.text[usize::from(range.start())..usize::from(range.end())]
    }

    /// Is an action of `kind` of any interest to whoever's asking?
    pub fn wants(&self, kind: Kind) -> bool {
        let Some(only) = &self.only else {
            // source actions only when they're asked for, by what runs them on a save, say
            return !matches!(kind, Kind::FixAll);
        };
        let name = kind.as_str();
        only.iter().any(|o| {
            name == o || (name.starts_with(o.as_str()) && name.as_bytes().get(o.len()) == Some(&b'.'))
        })
    }

    /// The diagnostics this applies to: those of the file touching the selection.
    pub fn diagnostics(&self) -> Vec<&'c Diagnostic> {
        let at = self.raw_range;
        stir_checker::check_file(self.db, self.ws, self.file)
            .iter()
            .filter(|d| {
                d.range.start() <= at.end() && at.start() <= d.range.end()
            })
            .collect()
    }

    /// What's wrong around the selection, ours and what can safely be placed of someone else's.
    pub fn found(&self) -> Vec<Found> {
        let mut out: Vec<Found> = self
            .diagnostics()
            .into_iter()
            .map(|d| Found {
                range: d.range,
                shown: d.range,
                code: d.code.to_string(),
            })
            .collect();
        out.extend(foreign::located(self));
        out
    }

    /// The token at `offset`: between two, the word if one is, then what follows, then what isn't
    /// whitespace.
    pub fn token_at(&self, offset: TextSize) -> Option<SyntaxToken> {
        let wordy = |t: &SyntaxToken| {
            t.kind().is_keyword()
                || matches!(
                    t.kind(),
                    SyntaxKind::IDENT
                        | SyntaxKind::VARIABLE
                        | SyntaxKind::NAME_QUALIFIED
                        | SyntaxKind::NAME_FULLY_QUALIFIED
                )
        };
        let at = self.root.token_at_offset(offset);
        let right = at.clone().right_biased();
        let left = at.left_biased();
        match (left, right) {
            (Some(l), Some(r)) if l == r => Some(r),
            (Some(l), Some(r)) => Some(if wordy(&r) || (!wordy(&l) && !r.kind().is_trivia()) {
                r
            } else if wordy(&l) || !l.kind().is_trivia() {
                l
            } else {
                r
            }),
            (l, r) => r.or(l),
        }
    }

    /// The token at the start of the selection.
    pub fn here(&self) -> Option<SyntaxToken> {
        self.token_at(self.range.start())
    }

    pub fn edit(&self, range: TextRange, text: impl Into<String>) -> FileEdit {
        FileEdit {
            file: self.file,
            range,
            text: text.into(),
        }
    }

    pub fn insert(&self, at: TextSize, text: impl Into<String>) -> FileEdit {
        self.edit(TextRange::empty(at), text)
    }
}

fn trim_range(text: &str, range: TextRange) -> TextRange {
    let slice = &text[usize::from(range.start())..usize::from(range.end())];
    let start = usize::from(range.start()) + (slice.len() - slice.trim_start().len());
    let end = start + slice.trim().len();
    TextRange::new(TextSize::from(start as u32), TextSize::from(end as u32))
}

/// Everything that can be done at the selection of `cx`.
pub fn offers<'c>(cx: &'c Cx<'c>) -> Vec<Offer<'c>> {
    let mut out = Vec::new();
    if !stir_db::syntax_diagnostics(cx.db, cx.file).is_empty()
        || cx.file.origin(cx.db) != FileOrigin::Source
    {
        return out;
    }
    if cx.wants(Kind::QuickFix) {
        fix_names::offers(cx, &mut out);
        fix_types::offers(cx, &mut out);
        fix_members::offers(cx, &mut out);
        organize::remove_offers(cx, &mut out);
        suppress::offers(cx, &mut out);
    }
    if cx.wants(Kind::OrganizeImports) {
        organize::offers(cx, &mut out);
    }
    if cx.wants(Kind::FixAll) {
        organize::fix_all_offers(cx, &mut out);
    }
    if cx.wants(Kind::RefactorExtract) {
        variables::extract_offers(cx, &mut out);
        extract::offers(cx, &mut out);
    }
    if cx.wants(Kind::RefactorInline) {
        variables::inline_offers(cx, &mut out);
    }
    if cx.wants(Kind::RefactorRewrite) {
        members::offers(cx, &mut out);
        classes::offers(cx, &mut out);
        flow::offers(cx, &mut out);
        fix_types::refactor_offers(cx, &mut out);
        signature::offers(cx, &mut out);
    }
    out.retain(|o| cx.wants(o.kind));
    out.sort_by_key(|o| !o.preferred);
    out
}

/// The edits of the offer `id`, for this selection of this text, or why there aren't any.
pub fn resolve<'c>(cx: &'c Cx<'c>, id: &str) -> Result<Change, Refusal> {
    let offer = offers(cx)
        .into_iter()
        .find(|o| o.id == id)
        .ok_or_else(|| "That can't be done any more.".to_owned())?;
    finish(cx.db, (offer.build)()?)
}

/// `change`, if it's something that can be done.
pub fn finish(db: &dyn Db, change: Change) -> Result<Change, Refusal> {
    let mut files: Vec<SourceFile> = Vec::new();
    for e in &change.edits {
        if !files.contains(&e.file) {
            files.push(e.file);
        }
    }
    for file in files {
        if file.origin(db) != FileOrigin::Source {
            return Err(format!(
                "It would change `{}`, which isn't a file of this project.",
                file.path(db).display()
            ));
        }
        let text = apply(file.text(db), &change, file)?;
        if !stir_syntax::parse(&text).errors().is_empty() {
            return Err(format!(
                "It would leave `{}` with syntax errors, so it wasn't done. That's a bug in StirPHP.",
                file.path(db).display()
            ));
        }
    }
    Ok(change)
}

/// `text` with the edits of `file` in `change`. Two edits can't overlap, and what's inserted at
/// one place stays in the order of the edits.
pub fn apply(text: &str, change: &Change, file: SourceFile) -> Result<String, Refusal> {
    let mut edits: Vec<&FileEdit> = change.edits.iter().filter(|e| e.file == file).collect();
    edits.sort_by_key(|e| (e.range.start(), e.range.end()));
    let mut out = String::with_capacity(text.len());
    let mut at = 0usize;
    for e in edits {
        let (start, end) = (usize::from(e.range.start()), usize::from(e.range.end()));
        if start < at || end > text.len() {
            return Err("The edits of this overlap, so it wasn't done. That's a bug in StirPHP.".into());
        }
        out.push_str(&text[at..start]);
        out.push_str(&e.text);
        at = end;
    }
    out.push_str(&text[at..]);
    Ok(out)
}
