//! The program-wide index of declarations.

use rustc_hash::FxHashMap;
use smol_str::SmolStr;
use stir_db::{FileOrigin, SourceFile};

/// Where a declaration lives: a file and its index in that file's item list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ItemLoc {
    pub file: SourceFile,
    pub index: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DefIndex {
    /// lowercase FQN → classes/interfaces/traits/enums/records
    pub classes: FxHashMap<SmolStr, Vec<ItemLoc>>,
    /// lowercase FQN → functions (extension functions are excluded)
    pub functions: FxHashMap<SmolStr, Vec<ItemLoc>>,
    /// lowercase short name → extension functions with that name
    pub extensions: FxHashMap<SmolStr, Vec<ItemLoc>>,
    /// FQN (case-sensitive) → constants
    pub consts: FxHashMap<SmolStr, Vec<ItemLoc>>,
    /// lowercase FQN → type aliases
    pub type_aliases: FxHashMap<SmolStr, Vec<ItemLoc>>,
    /// Files by origin, for diagnostics filtering.
    pub origins: FxHashMap<SourceFile, FileOrigin>,
}

impl DefIndex {
    /// Prefer project declarations over vendor ones, and those over stubs.
    pub fn sort(&mut self) {
        let origins = self.origins.clone();
        let rank = |l: &ItemLoc| match origins.get(&l.file) {
            Some(FileOrigin::Source) => 0,
            Some(FileOrigin::Vendor) => 1,
            _ => 2,
        };
        for map in [
            &mut self.classes,
            &mut self.functions,
            &mut self.extensions,
            &mut self.consts,
            &mut self.type_aliases,
        ] {
            for locs in map.values_mut() {
                locs.sort_by_key(rank);
            }
        }
    }
}
