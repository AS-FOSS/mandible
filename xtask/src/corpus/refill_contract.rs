//! `must_value_names_after_root_refill`: split out of `contract.rs` for
//! the same line-count reason the round-7/8 detector modules split out of
//! `score.rs` (`scripts/shape_guard.sh`'s 800-code-line ceiling).
//!
//! Simulates the real app's own root-refill
//! (`mandible::background::Warmer::submit_root_fill`, which hands
//! `fill_node` the already-extracted root as `existing` alongside a fresh
//! reprobe) by running `mandible_core::merge_nodes` over two clones of a
//! fixture's own root — the same "existing plus a fresh reprobe" shape
//! `Runner::fill_node`'s own contract always produces.

// Every listed substring must then appear in the named flag's merged
// `value_name` (matched the way `must_value_name` matches: substring,
// whitespace-collapsed on both sides). See docs/shapes.md S-147:
// `merge_entity_bucket` unions every distinct value name across the
// bucket instead of picking one, and this is the only field that can
// see the union actually happened.

use super::contract::{collapse_whitespace, entity_matches_flag_spec, ContractFailure};
use super::ContractMeta;
use mandible_core::CommandNode;

/// `must_value_names_after_root_refill`'s own `CONTRACT WEAKENED` lines:
/// same rule `must_value_name` gets in `contract.rs`, plus a shrunk list
/// of expected substrings for a flag the assertion still names — that
/// retires the claim that one of several unioned names survived.
pub(crate) fn weakened_lines(label: &str, b: &ContractMeta, n: &ContractMeta) -> Vec<String> {
    let mut lines = Vec::new();
    for (flag, base_names) in &b.must_value_names_after_root_refill {
        match n.must_value_names_after_root_refill.get(flag) {
            None => lines.push(format!(
                "CONTRACT WEAKENED: {label} must_value_names_after_root_refill[{flag:?}] \
                 (assertion removed)"
            )),
            Some(now_names) => {
                for name in base_names {
                    if !now_names.contains(name) {
                        lines.push(format!(
                            "CONTRACT WEAKENED: {label} \
                             must_value_names_after_root_refill[{flag:?}] ({name:?} dropped)"
                        ));
                    }
                }
            }
        }
    }
    lines
}

/// Called from `runner.rs`'s own fixture loop, not from `contract.rs`'s
/// `check_contract` — that file sits at `scripts/shape_guard.sh`'s own
/// line-count ceiling, so this takes `root` as the `Option` the loop
/// already has and folds in its own "no root" case rather than asking
/// `check_contract_missing_root` to grow another arm.
pub(crate) fn check_must_value_names_after_root_refill(
    contract: &ContractMeta,
    root: Option<&CommandNode>,
) -> Vec<ContractFailure> {
    let mut failures = Vec::new();
    if contract.must_value_names_after_root_refill.is_empty() {
        return failures;
    }
    let Some(refilled) = refill(root, &mut failures, "must_value_names_after_root_refill") else {
        return failures;
    };
    for (flag_spec, expected_substrings) in &contract.must_value_names_after_root_refill {
        match refilled
            .flags()
            .find(|f| entity_matches_flag_spec(f, flag_spec))
        {
            None => failures.push(ContractFailure(format!(
                "must_value_names_after_root_refill[{flag_spec:?}]: flag not present after refill"
            ))),
            Some(entity) => {
                let actual = collapse_whitespace(entity.value_name.as_deref().unwrap_or(""));
                let missing: Vec<String> = expected_substrings
                    .iter()
                    .map(|s| collapse_whitespace(s))
                    .filter(|expected| !actual.contains(expected.as_str()))
                    .collect();
                if !missing.is_empty() {
                    failures.push(ContractFailure(format!(
                        "must_value_names_after_root_refill[{flag_spec:?}]: expected the merged \
                         value name to contain {missing:?}, got {actual:?}"
                    )));
                }
            }
        }
    }
    failures
}

/// `must_choices_after_root_refill`'s own `CONTRACT WEAKENED` lines, the
/// twin of [`weakened_lines`] above for the `choices` half of S-147's
/// follow-up ruling.
pub(crate) fn choices_weakened_lines(
    label: &str,
    b: &ContractMeta,
    n: &ContractMeta,
) -> Vec<String> {
    let mut lines = Vec::new();
    for (flag, base_names) in &b.must_choices_after_root_refill {
        match n.must_choices_after_root_refill.get(flag) {
            None => lines.push(format!(
                "CONTRACT WEAKENED: {label} must_choices_after_root_refill[{flag:?}] \
                 (assertion removed)"
            )),
            Some(now_names) => {
                for name in base_names {
                    if !now_names.contains(name) {
                        lines.push(format!(
                            "CONTRACT WEAKENED: {label} \
                             must_choices_after_root_refill[{flag:?}] ({name:?} dropped)"
                        ));
                    }
                }
            }
        }
    }
    lines
}

/// The `choices` twin of [`check_must_value_names_after_root_refill`]:
/// asserts every named literal survives in the refilled flag's `choices`
/// list, not its `value_name`. S-147's own follow-up ruling
/// (docs/design.md §16) moves a same-spelling bucket's disagreeing literal values
/// out of `value_name` and into one unioned `choices` list, and
/// `must_attach_choices` alone cannot see that union: it walks the raw,
/// unrefilled tree, where `.find()` sees only the first invocation
/// form's own, usually-empty `choices`.
pub(crate) fn check_must_choices_after_root_refill(
    contract: &ContractMeta,
    root: Option<&CommandNode>,
) -> Vec<ContractFailure> {
    let mut failures = Vec::new();
    if contract.must_choices_after_root_refill.is_empty() {
        return failures;
    }
    let Some(refilled) = refill(root, &mut failures, "must_choices_after_root_refill") else {
        return failures;
    };
    for (flag_spec, expected_choices) in &contract.must_choices_after_root_refill {
        match refilled
            .flags()
            .find(|f| entity_matches_flag_spec(f, flag_spec))
        {
            None => failures.push(ContractFailure(format!(
                "must_choices_after_root_refill[{flag_spec:?}]: flag not present after refill"
            ))),
            Some(entity) => {
                let missing: Vec<&str> = expected_choices
                    .iter()
                    .filter(|c| !entity.choices.iter().any(|ch| &ch.name == *c))
                    .map(|s| s.as_str())
                    .collect();
                if !missing.is_empty() {
                    failures.push(ContractFailure(format!(
                        "must_choices_after_root_refill[{flag_spec:?}]: expected the merged \
                         choices to include {missing:?}, got {:?}",
                        entity
                            .choices
                            .iter()
                            .map(|c| c.name.as_str())
                            .collect::<Vec<_>>()
                    )));
                }
            }
        }
    }
    failures
}

/// Shared "no root"/"simulated refill failed" handling for both refill
/// checks above, keyed by the caller's own contract-field name so each
/// failure still names the field it belongs to.
fn refill(
    root: Option<&CommandNode>,
    failures: &mut Vec<ContractFailure>,
    field: &str,
) -> Option<CommandNode> {
    let Some(root) = root else {
        failures.push(ContractFailure(format!("{field}: no root produced")));
        return None;
    };
    match mandible_core::merge_nodes(vec![root.clone(), root.clone()]) {
        Ok(node) => Some(node),
        Err(e) => {
            failures.push(ContractFailure(format!(
                "{field}: simulated refill failed: {e}"
            )));
            None
        }
    }
}
