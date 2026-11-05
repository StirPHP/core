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
    /// lowercase FQN → functions
    pub functions: FxHashMap<SmolStr, Vec<ItemLoc>>,
    /// FQN (case-sensitive) → constants
    pub consts: FxHashMap<SmolStr, Vec<ItemLoc>>,
    /// lowercase FQN → type aliases
    pub type_aliases: FxHashMap<SmolStr, Vec<ItemLoc>>,
    /// Files by origin, for diagnostics filtering.
    pub origins: FxHashMap<SourceFile, FileOrigin>,
    /// Files by precedence when declarations collide (lower wins).
    pub ranks: FxHashMap<SourceFile, u8>,
}

impl DefIndex {
    /// The precedence of a file's declarations: project sources, then vendor
    /// files and PHP stubs.
    pub fn rank(origin: FileOrigin) -> u8 {
        match origin {
            FileOrigin::Source => 0,
            FileOrigin::Vendor => 1,
            FileOrigin::Stub => 2,
        }
    }

    /// Orders every declaration list by [`DefIndex::rank`].
    pub fn sort(&mut self) {
        let ranks = self.ranks.clone();
        let rank = |l: &ItemLoc| ranks.get(&l.file).copied().unwrap_or(u8::MAX);
        for map in [
            &mut self.classes,
            &mut self.functions,
            &mut self.consts,
            &mut self.type_aliases,
        ] {
            for locs in map.values_mut() {
                locs.sort_by_key(rank);
            }
        }
    }
}
