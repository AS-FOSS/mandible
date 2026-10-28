//! The groups and the order of the rows a list section draws, from the
//! tool's own order or sorted A to Z (spec §9.3). The page and the search
//! ranking both read this, so a row's place in search is its drawn place.

use super::{group_key, group_label, INHERITED_GROUP};
use crate::sort::{sort_name, DetailSort};
use mandible_core::{Entity, EntityKind};
use std::collections::HashMap;

/// One run of rows under one divider, or under none.
pub(super) struct PlanGroup<'a> {
    /// The divider's label; `None` for rows that sit under no divider.
    pub(super) label: Option<String>,
    /// Whether the rows are the ones a parent command passes down.
    pub(super) inherited: bool,
    /// Each row with its place in the tool's own order of the section.
    pub(super) rows: Vec<(usize, &'a Entity)>,
}

fn sorted(rows: &mut [(usize, &Entity)]) {
    rows.sort_by_cached_key(|(i, e)| (sort_name(e), *i));
}

/// Ungrouped rows first, then each group in the order the tool introduced
/// it, then inherited rows last as their own group. Sorting reorders rows
/// only: inside each group, or over the whole section with the tool's own
/// group dividers dropped. Positionals are never sorted.
pub(super) fn plan_section<'a>(
    entities: &[&'a Entity],
    kind: EntityKind,
    sort: DetailSort,
) -> Vec<PlanGroup<'a>> {
    let mut order: Vec<Option<String>> = Vec::new();
    let mut labels: HashMap<String, String> = HashMap::new();
    let mut own: HashMap<Option<String>, Vec<(usize, &Entity)>> = HashMap::new();
    let mut inherited: Vec<(usize, &Entity)> = Vec::new();
    for (i, &e) in entities.iter().enumerate() {
        if e.inherited {
            inherited.push((i, e));
            continue;
        }
        let raw = e.group.as_deref().unwrap_or_default();
        let key = Some(group_key(raw)).filter(|k| !k.is_empty());
        if !own.contains_key(&key) {
            order.push(key.clone());
            if let Some(k) = &key {
                labels.insert(k.clone(), group_label(raw));
            }
        }
        own.entry(key).or_default().push((i, e));
    }
    let mut plan: Vec<PlanGroup<'a>> = Vec::new();
    if let Some(rows) = own.remove(&None) {
        plan.push(PlanGroup {
            label: None,
            inherited: false,
            rows,
        });
    }
    for key in order {
        let Some(key) = key else { continue };
        let Some(rows) = own.remove(&Some(key.clone())) else {
            continue;
        };
        plan.push(PlanGroup {
            label: Some(labels.remove(&key).unwrap_or(key)),
            inherited: false,
            rows,
        });
    }
    if !inherited.is_empty() {
        plan.push(PlanGroup {
            label: Some(INHERITED_GROUP.to_string()),
            inherited: true,
            rows: inherited,
        });
    }
    if kind == EntityKind::Positional {
        return plan;
    }
    match sort {
        DetailSort::Printed => {}
        DetailSort::Sections => plan.iter_mut().for_each(|g| sorted(&mut g.rows)),
        DetailSort::Overall => {
            let (mut own, rest): (Vec<_>, Vec<_>) = plan.into_iter().partition(|g| !g.inherited);
            let mut rows: Vec<(usize, &Entity)> = own.drain(..).flat_map(|g| g.rows).collect();
            sorted(&mut rows);
            plan = Vec::new();
            if !rows.is_empty() {
                plan.push(PlanGroup {
                    label: None,
                    inherited: false,
                    rows,
                });
            }
            plan.extend(rest.into_iter().map(|mut g| {
                sorted(&mut g.rows);
                g
            }));
        }
    }
    plan
}
