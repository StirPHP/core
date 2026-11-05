//! placeholder
use stir_db::{Db, SourceFile, Workspace};
use stir_syntax::TextSize;
use super::{Change, Cx, Offer, Refusal};
pub fn offers<'c>(_cx: &'c Cx<'c>, _out: &mut Vec<Offer<'c>>) {}
pub struct NewParameter { pub declared: String, pub passed: Option<String> }
pub fn add_parameter_change(_db: &dyn Db, _ws: Workspace, _file: SourceFile, _at: TextSize, _p: &NewParameter) -> Result<Change, Refusal> { Err(String::new()) }
pub fn ask_parameter_at(_cx: &Cx<'_>) -> Option<TextSize> { None }
