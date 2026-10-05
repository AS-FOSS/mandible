//! `must_describe_root`: text the root's own `description` must carry,
//! the positive mirror of `must_not_describe_root`.

use super::*;

pub(super) fn missing_root_failures(contract: &ContractMeta) -> Vec<ContractFailure> {
    if contract.must_describe_root.is_empty() {
        return Vec::new();
    }
    vec![ContractFailure(
        "must_describe_root: no root produced".into(),
    )]
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
