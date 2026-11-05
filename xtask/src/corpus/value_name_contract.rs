//! `must_value_name` and `must_not_value_name`: what a flag's value
//! placeholder must and must not carry.

use super::contract::{collapse_whitespace, entity_matches_flag_spec, ContractFailure};
use super::ContractMeta;
use mandible_core::CommandNode;

/// `must_value_name`: a flag's value placeholder must still carry the text
/// the tool's own row shows. Scans EVERY matching entity, unlike
/// `must_describe`'s first match, because one spelling can head two rows
/// (`corpus/vim.basic/audit-seed4` documents `-r` twice).
pub(super) fn check_must_value_name(
    contract: &ContractMeta,
    root: &CommandNode,
) -> Vec<ContractFailure> {
    let mut failures = Vec::new();
    for (flag_spec, expected_text) in &contract.must_value_name {
        let expected = collapse_whitespace(expected_text);
        let mut seen = Vec::new();
        let mut matched = false;
        for entity in root
            .flags()
            .filter(|f| entity_matches_flag_spec(f, flag_spec))
        {
            matched = true;
            let actual = collapse_whitespace(entity.value_name.as_deref().unwrap_or(""));
            if actual.contains(&expected) {
                seen.clear();
                break;
            }
            seen.push(actual);
        }
        if !matched {
            failures.push(ContractFailure(format!(
                "must_value_name[{flag_spec:?}]: flag not present"
            )));
        } else if !seen.is_empty() {
            failures.push(ContractFailure(format!(
                "must_value_name[{flag_spec:?}]: expected a value name containing {expected:?}, got {seen:?}"
            )));
        }
    }
    failures
}

/// `must_not_value_name`: a flag's value placeholder must NOT carry the
/// given text, the mirror of `check_must_value_name` above. Scans EVERY
/// matching entity, same reason: one spelling can head two rows. Silent
/// when the flag itself is absent, the same reasoning `must_not_describe`
/// uses. See docs/shapes.md S-130.
pub(super) fn check_must_not_value_name(
    contract: &ContractMeta,
    root: &CommandNode,
) -> Vec<ContractFailure> {
    let mut failures = Vec::new();
    for (flag_spec, forbidden_text) in &contract.must_not_value_name {
        let forbidden = collapse_whitespace(forbidden_text);
        for entity in root
            .flags()
            .filter(|f| entity_matches_flag_spec(f, flag_spec))
        {
            let actual = collapse_whitespace(entity.value_name.as_deref().unwrap_or(""));
            if actual.contains(&forbidden) {
                failures.push(ContractFailure(format!(
                    "must_not_value_name[{flag_spec:?}]: value name contains {forbidden:?}, got {actual:?}"
                )));
            }
        }
    }
    failures
}
