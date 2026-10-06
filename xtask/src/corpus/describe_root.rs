//! `must_describe_root`: text the root's own `description` must carry,
//! the positive mirror of `must_not_describe_root`.

use super::*;

/// A `must_describe_root` entry dropped from the baseline weakens the claim.
pub(super) fn weakened(label: &str, base: &ContractMeta, now: &ContractMeta) -> Vec<String> {
    let dropped: Vec<&str> = base
        .must_describe_root
        .iter()
        .filter(|entry| !now.must_describe_root.contains(entry))
        .map(String::as_str)
        .collect();
    if dropped.is_empty() {
        return Vec::new();
    }
    vec![format!(
        "CONTRACT WEAKENED: {label} must_describe_root (dropped: {})",
        dropped.join(", ")
    )]
}

/// Failures for a fixture with no root: the subcommand claims and this one.
pub(super) fn missing_root_failures(contract: &ContractMeta) -> Vec<ContractFailure> {
    let mut failures = subcommand::missing_root_failures(contract);
    if !contract.must_describe_root.is_empty() {
        failures.push(ContractFailure(
            "must_describe_root: no root produced".into(),
        ));
    }
    failures
}

/// The root-description claims, negative then positive.
pub(super) fn check_describe_root(
    contract: &ContractMeta,
    root: &CommandNode,
) -> Vec<ContractFailure> {
    let mut failures = contract::check_must_not_describe_root(contract, root);
    failures.extend(check(contract, root));
    failures
}

/// Whitespace-collapsed substring match, `must_describe`'s own rule.
pub(super) fn check(contract: &ContractMeta, root: &CommandNode) -> Vec<ContractFailure> {
    if contract.must_describe_root.is_empty() {
        return Vec::new();
    }
    let description = root
        .description
        .as_ref()
        .map(|t| contract::collapse_whitespace(t.as_str()))
        .unwrap_or_default();
    let absent: Vec<&str> = contract
        .must_describe_root
        .iter()
        .filter(|text| !description.contains(&contract::collapse_whitespace(text)))
        .map(String::as_str)
        .collect();
    if absent.is_empty() {
        return Vec::new();
    }
    vec![ContractFailure(format!(
        "must_describe_root: absent {}",
        absent.join(", ")
    ))]
}
