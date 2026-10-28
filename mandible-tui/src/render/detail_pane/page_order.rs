//! Where each entity of a command stands on its page, top to bottom, for
//! ranking search hits (spec §10 rule 9).

use super::{plan_section, LIST_SECTIONS};
use crate::sort::DetailSort;
use mandible_core::{CommandNode, FlagKey};
use std::collections::HashMap;

/// Each entity's line of the page, by key: sections in page order, rows in
/// the order the page draws them in the `sort` state. A key shared by two
/// rows keeps the upper one.
pub(crate) fn page_positions(node: &CommandNode, sort: DetailSort) -> HashMap<FlagKey, usize> {
    let mut out = HashMap::new();
    let mut next = 0;
    for (kind, _, _) in LIST_SECTIONS {
        let visible: Vec<_> = node.entities_of(kind).collect();
        for group in plan_section(&visible, kind, sort) {
            for (_, entity) in group.rows {
                if let Some(key) = entity.key() {
                    out.entry(key).or_insert(next);
                }
                next += 1;
            }
        }
    }
    out
}
