//! One document, one heading, one row: rows identical in every field
//! under the identical heading fold to the first occurrence's position.
//! See docs/shapes.md S-187 and corpus/ffplay/6.1.1-3ubuntu5-repeat.

use mandible_core::{Entity, EntityKind};
use std::collections::HashMap;

/// Folds exact repeats of any entity kind that carry a heading. A row with
/// no heading is never folded: nothing says two such rows are one print.
pub(super) fn fold_repeated_rows(entities: &mut Vec<Entity>) {
    let mut kept: Vec<Entity> = Vec::with_capacity(entities.len());
    let mut by_key: HashMap<(EntityKind, String, String), Vec<usize>> = HashMap::new();
    for e in std::mem::take(entities) {
        let Some(group) = e.group.clone() else {
            kept.push(e);
            continue;
        };
        let key = (e.kind, group, e.primary_name().to_string());
        let seen = by_key.entry(key).or_default();
        if seen.iter().any(|&i| same_row(&kept[i], &e)) {
            continue;
        }
        seen.push(kept.len());
        kept.push(e);
    }
    *entities = kept;
}

/// Equal in every field except the provenance's confidence.
fn same_row(a: &Entity, b: &Entity) -> bool {
    a.provenance.sources == b.provenance.sources && {
        let mut a = a.clone();
        a.provenance = b.provenance.clone();
        a == *b
    }
}
