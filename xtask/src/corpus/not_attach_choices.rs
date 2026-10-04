//! `must_not_attach_choices`: values no root flag of a spelling may carry.
//! Split out of `contract.rs` for the 800-code-line ceiling.

use super::contract::{entity_matches_flag_spec, ContractFailure};
use super::ContractMeta;
use mandible_core::CommandNode;

/// A negative claim weakens by losing an entry, as `must_not_describe` does.
pub(crate) fn weakened_lines(label: &str, b: &ContractMeta, n: &ContractMeta) -> Vec<String> {
    b.must_not_attach_choices
        .keys()
        .filter(|flag| !n.must_not_attach_choices.contains_key(*flag))
        .map(|flag| {
            format!(
                "CONTRACT WEAKENED: {label} must_not_attach_choices[{flag:?}] (assertion removed)"
            )
        })
        .collect()
}

/// Every root flag matching the spelling is checked, so a block that
/// overran into another section's flag fails by name. Vacuous without a
/// matching flag.
pub(crate) fn failures(contract: &ContractMeta, root: &CommandNode) -> Vec<ContractFailure> {
    let mut failures = Vec::new();
    for (flag_spec, forbidden) in &contract.must_not_attach_choices {
        let carried: Vec<&str> = root
            .flags()
            .filter(|f| entity_matches_flag_spec(f, flag_spec))
            .flat_map(|f| f.choices.iter())
            .filter(|c| forbidden.iter().any(|x| x == &c.name))
            .map(|c| c.name.as_str())
            .collect();
        if !carried.is_empty() {
            failures.push(ContractFailure(format!(
                "must_not_attach_choices[{flag_spec:?}]: carries {}",
                carried.join(", ")
            )));
        }
    }
    failures
}
