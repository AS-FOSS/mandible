//! Rows of a page told apart by their place, not their spelling: two rows
//! may carry the same key, and the current match is one of them (spec §10).

use super::LIST_SECTIONS;
use crate::sort::RowId;
use mandible_core::{CommandNode, Entity, FlagKey};

/// The first row, in the tool's own order, that `key` addresses.
pub(crate) fn first_row_with_key(node: &CommandNode, key: &FlagKey) -> Option<RowId> {
    LIST_SECTIONS
        .iter()
        .enumerate()
        .find_map(|(at, (kind, ..))| {
            let i = node.entities_of(*kind).position(|e| e.matches_key(key))?;
            Some((at, i))
        })
}

/// The entity standing at `id` on the page of `node`.
pub(crate) fn entity_at(node: &CommandNode, id: RowId) -> Option<&Entity> {
    let (kind, ..) = LIST_SECTIONS.get(id.0)?;
    node.entities_of(*kind).nth(id.1)
}
