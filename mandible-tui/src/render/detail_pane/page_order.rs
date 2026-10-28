//! Where each entity of a command stands on its page, top to bottom, for
//! ranking search hits (spec §10 rule 9).

use super::{group_key, LIST_SECTIONS};
use mandible_core::{CommandNode, Entity, FlagKey};
use std::collections::HashMap;

/// The entities of one section in the order the page draws them: the
/// ungrouped ones, then each group in the order the tool introduced it,
/// then the inherited ones.
fn section_order<'a>(entities: &[&'a Entity]) -> Vec<&'a Entity> {
    let mut groups: Vec<(String, Vec<&Entity>)> = Vec::new();
    let mut ungrouped = Vec::new();
    let mut inherited = Vec::new();
    for &e in entities {
        if e.inherited {
            inherited.push(e);
            continue;
        }
        let key = group_key(e.group.as_deref().unwrap_or_default());
        if key.is_empty() {
            ungrouped.push(e);
        } else if let Some(group) = groups.iter_mut().find(|(k, _)| *k == key) {
            group.1.push(e);
        } else {
            groups.push((key, vec![e]));
        }
    }
    let grouped = groups.into_iter().flat_map(|(_, g)| g);
    ungrouped
        .into_iter()
        .chain(grouped)
        .chain(inherited)
        .collect()
}

/// Each entity's line of the page, by key: sections in page order, rows in
/// the order [`section_order`] gives. A key shared by two rows keeps the
/// upper one.
pub(crate) fn page_positions(node: &CommandNode) -> HashMap<FlagKey, usize> {
    let mut out = HashMap::new();
    let mut next = 0;
    for (kind, _, _) in LIST_SECTIONS {
        let visible: Vec<&Entity> = node.entities_of(kind).collect();
        for entity in section_order(&visible) {
            if let Some(key) = entity.key() {
                out.entry(key).or_insert(next);
            }
            next += 1;
        }
    }
    out
}
