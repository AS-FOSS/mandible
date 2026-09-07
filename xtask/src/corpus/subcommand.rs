//! Path-keyed subcommand `[contract]` fields — `must_describe_subcommand`,
//! `must_subcommand_group`, `must_display_name`, `must_accept_modifiers` —
//! split out of `contract.rs` to keep that file under the workspace's
//! line-count lint (AGENTS.md §2), the same reason `contract.rs`'s own
//! `new_field_weakened_lines` exists.

use super::*;

/// The two fields' own "no root produced" failures, called from
/// `contract.rs`'s own `check_contract_missing_root`.
pub(super) fn missing_root_failures(contract: &ContractMeta) -> Vec<ContractFailure> {
    let mut failures = Vec::new();
    if !contract.must_describe_subcommand.is_empty() {
        failures.push(ContractFailure(
            "must_describe_subcommand: no root produced".into(),
        ));
    }
    if !contract.must_subcommand_group.is_empty() {
        failures.push(ContractFailure(
            "must_subcommand_group: no root produced".into(),
        ));
    }
    failures
}

/// Both checks below, called from `contract.rs`'s own
/// `check_contract_collection_fields`.
pub(super) fn check_all(contract: &ContractMeta, root: &CommandNode) -> Vec<ContractFailure> {
    let mut failures = check_must_describe_subcommand(contract, root);
    failures.extend(check_must_subcommand_group(contract, root));
    failures
}

/// `must_describe_subcommand`: a subcommand's own rendered description
/// (`CommandNode::summary`) must contain this text, keyed by path the way
/// `must_contain_flags_by_path` is. Same substring-after-collapsing-
/// whitespace rule `must_describe`/`must_describe_positional` use.
fn check_must_describe_subcommand(
    contract: &ContractMeta,
    root: &CommandNode,
) -> Vec<ContractFailure> {
    let mut failures = Vec::new();
    for (path, expected_text) in &contract.must_describe_subcommand {
        let Some(node) = find_node_by_path(root, path) else {
            failures.push(ContractFailure(format!(
                "must_describe_subcommand: no node at path {path:?}"
            )));
            continue;
        };
        let actual = node.summary.as_ref().map_or("", |t| t.as_str());
        let actual_collapsed = collapse_whitespace(actual);
        let expected_collapsed = collapse_whitespace(expected_text);
        if !actual_collapsed.contains(&expected_collapsed) {
            failures.push(ContractFailure(format!(
                "must_describe_subcommand[{path:?}]: expected description to contain {:?}, got \
                 {:?}",
                expected_collapsed,
                truncate_for_display(&actual_collapsed, 120)
            )));
        }
    }
    failures
}

/// `must_subcommand_group`: a subcommand's own `CommandNode::group`, keyed
/// by path, matched exactly (no substring, no whitespace collapsing) —
/// `must_flag_group`'s own reasoning, one level down. The empty string
/// asserts the node carries no group at all.
fn check_must_subcommand_group(
    contract: &ContractMeta,
    root: &CommandNode,
) -> Vec<ContractFailure> {
    let mut failures = Vec::new();
    for (path, expected) in &contract.must_subcommand_group {
        let Some(node) = find_node_by_path(root, path) else {
            failures.push(ContractFailure(format!(
                "must_subcommand_group: no node at path {path:?}"
            )));
            continue;
        };
        let ok = if expected.is_empty() {
            node.group.is_none()
        } else {
            node.group.as_deref() == Some(expected.as_str())
        };
        if !ok {
            let expected_display = if expected.is_empty() {
                "(no group)".to_string()
            } else {
                format!("{expected:?}")
            };
            failures.push(ContractFailure(format!(
                "must_subcommand_group[{path:?}]: expected group {expected_display}, got {:?}",
                node.group.as_deref().unwrap_or("(none)")
            )));
        }
    }
    failures
}

/// The weakening checks for both fields above, called from
/// `contract.rs`'s own `new_field_weakened_lines`.
pub(super) fn contract_weakened_lines(
    label: &str,
    b: &ContractMeta,
    n: &ContractMeta,
) -> Vec<String> {
    let mut lines = Vec::new();
    for path in b.must_describe_subcommand.keys() {
        if !n.must_describe_subcommand.contains_key(path) {
            lines.push(format!(
                "CONTRACT WEAKENED: {label} must_describe_subcommand[{path:?}] (assertion removed)"
            ));
        }
    }
    for path in b.must_subcommand_group.keys() {
        if !n.must_subcommand_group.contains_key(path) {
            lines.push(format!(
                "CONTRACT WEAKENED: {label} must_subcommand_group[{path:?}] (assertion removed)"
            ));
        }
    }
    lines
}

/// `must_display_name`/`must_accept_modifiers`: a subcommand's own source
/// spelling and accepted-modifier letters, both keyed by path the way
/// `must_contain_flags_by_path` is keyed.
pub(super) fn check_must_display_name_and_modifiers(
    contract: &ContractMeta,
    root: &CommandNode,
) -> Vec<ContractFailure> {
    let mut failures = Vec::new();
    for (path, expected_name) in &contract.must_display_name {
        let Some(node) = find_node_by_path(root, path) else {
            failures.push(ContractFailure(format!(
                "must_display_name: no node at path {path:?}"
            )));
            continue;
        };
        let actual = node.display_name.as_deref().unwrap_or(node.name.as_str());
        if actual != expected_name {
            failures.push(ContractFailure(format!(
                "must_display_name[{path:?}]: expected {expected_name:?}, got {actual:?}"
            )));
        }
    }
    for (path, expected_letters) in &contract.must_accept_modifiers {
        let Some(node) = find_node_by_path(root, path) else {
            failures.push(ContractFailure(format!(
                "must_accept_modifiers: no node at path {path:?}"
            )));
            continue;
        };
        let missing: Vec<&str> = expected_letters
            .iter()
            .filter(|letter| {
                !node
                    .accepted_modifiers
                    .iter()
                    .any(|m| m.to_string() == **letter)
            })
            .map(|s| s.as_str())
            .collect();
        if !missing.is_empty() {
            failures.push(ContractFailure(format!(
                "must_accept_modifiers[{path:?}]: missing {}",
                missing.join(", ")
            )));
        }
    }
    failures
}
