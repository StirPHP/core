//! placeholder
use super::{Cx, Offer, FileEdit};
pub fn offers<'c>(_cx: &'c Cx<'c>, _out: &mut Vec<Offer<'c>>) {}
pub fn refactor_offers<'c>(_cx: &'c Cx<'c>, _out: &mut Vec<Offer<'c>>) {}
pub fn nullable_everywhere(_cx: &Cx<'_>) -> Vec<FileEdit> { Vec::new() }
