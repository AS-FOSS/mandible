//! `must_one_row_in_group`: exactly one row carries a spelling under a
//! group heading. Checked on the fixture's own root and on the root
//! refilled the way the app refills it, so a fold in the parse and a fold
//! in the merge both show. See docs/shapes.md S-187.

use super::contract::{entity_matches_flag_spec, ContractFailure};
use super::ContractMeta;
use mandible_core::CommandNode;

/// `CONTRACT WEAKENED` lines: an assertion removed, or a spelling dropped
/// from a group's list.
pub(crate) fn weakened_lines(label: &str, b: &ContractMeta, n: &ContractMeta) -> Vec<String> {
    let mut lines = Vec::new();
    for (group, specs) in &b.must_one_row_in_group {
        for spec in specs {
            if !n
                .must_one_row_in_group
                .get(group)
                .is_some_and(|now| now.contains(spec))
            {
                lines.push(format!(
                    "CONTRACT WEAKENED: {label} must_one_row_in_group[{group:?}] \
                     ({spec:?} dropped)"
                ));
            }
        }
    }
    lines
}

fn count_rows(root: &CommandNode, group: &str, spec: &str) -> usize {
    root.flags()
        .filter(|f| f.group.as_ref().map_or("", |g| g.as_str()) == group)
        .filter(|f| entity_matches_flag_spec(f, spec))
        .count()
}

/// Every listed spelling has exactly one row under its group (`""` is no
/// group), before and after a refill.
pub(crate) fn check_must_one_row_in_group(
    contract: &ContractMeta,
    root: Option<&CommandNode>,
) -> Vec<ContractFailure> {
    let mut failures = Vec::new();
    if contract.must_one_row_in_group.is_empty() {
        return failures;
    }
    let Some(root) = root else {
        failures.push(ContractFailure(
            "must_one_row_in_group: no root produced".into(),
        ));
        return failures;
    };
    let refilled = mandible_core::merge_nodes(vec![root.clone(), root.clone()]).ok();
    for (group, specs) in &contract.must_one_row_in_group {
        for spec in specs {
            let mut views = vec![("parse", count_rows(root, group, spec))];
            if let Some(r) = &refilled {
                views.push(("refill", count_rows(r, group, spec)));
            }
            for (view, n) in views {
                if n != 1 {
                    failures.push(ContractFailure(format!(
                        "must_one_row_in_group[{group:?}][{spec:?}]: {n} rows after {view}, \
                         expected 1"
                    )));
                }
            }
        }
    }
    failures
}
