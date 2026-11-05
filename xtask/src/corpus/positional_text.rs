//! Path-keyed claims about one operand's own text: `must_describe_positional_by_path`,
//! `must_not_describe_positional_by_path` and `must_attach_positional_choices_by_path`.
//! Each is `path -> operand name -> text` (or choices), the operand matched
//! on `primary_name` as `must_contain_positionals_by_path` matches it.

use super::*;

type Claims<T> = std::collections::BTreeMap<String, std::collections::BTreeMap<String, T>>;

/// A claim dropped from the baseline weakens the contract.
pub(super) fn weakened(label: &str, base: &ContractMeta, now: &ContractMeta) -> Vec<String> {
    let mut lines = Vec::new();
    let mut lost = |field: &str, was: &Claims<String>, is: &Claims<String>| {
        for (path, names) in was {
            for name in names.keys() {
                if !is.get(path).is_some_and(|m| m.contains_key(name)) {
                    lines.push(format!(
                        "CONTRACT WEAKENED: {label} {field}[{path:?}][{name:?}] (assertion removed)"
                    ));
                }
            }
        }
    };
    lost(
        "must_describe_positional_by_path",
        &base.must_describe_positional_by_path,
        &now.must_describe_positional_by_path,
    );
    lost(
        "must_not_describe_positional_by_path",
        &base.must_not_describe_positional_by_path,
        &now.must_not_describe_positional_by_path,
    );
    for (path, names) in &base.must_attach_positional_choices_by_path {
        for (name, choices) in names {
            let kept = now
                .must_attach_positional_choices_by_path
                .get(path)
                .and_then(|m| m.get(name));
            if choices.iter().any(|c| !kept.is_some_and(|k| k.contains(c))) {
                lines.push(format!(
                    "CONTRACT WEAKENED: {label} must_attach_positional_choices_by_path[{path:?}][{name:?}] (dropped choices)"
                ));
            }
        }
    }
    lines
}

/// Failures for a fixture with no root.
pub(super) fn missing_root_failures(contract: &ContractMeta) -> Vec<ContractFailure> {
    let any = !contract.must_describe_positional_by_path.is_empty()
        || !contract.must_not_describe_positional_by_path.is_empty()
        || !contract.must_attach_positional_choices_by_path.is_empty();
    if any {
        vec![ContractFailure(
            "positional text claims by path: no root produced".into(),
        )]
    } else {
        Vec::new()
    }
}

fn operand<'a>(
    root: &'a CommandNode,
    field: &str,
    path: &str,
    name: &str,
    failures: &mut Vec<ContractFailure>,
) -> Option<&'a Entity> {
    let Some(node) = contract::find_node_by_path(root, path) else {
        failures.push(ContractFailure(format!(
            "{field}: no node at path {path:?}"
        )));
        return None;
    };
    let found = node.positionals().find(|p| p.primary_name() == name);
    if found.is_none() {
        failures.push(ContractFailure(format!(
            "{field}[{path:?}][{name:?}]: positional not present"
        )));
    }
    found
}

fn text_of(entity: &Entity) -> String {
    let text = entity.description.as_ref().map_or("", |t| t.as_str());
    contract::collapse_whitespace(text)
}

pub(super) fn check(contract: &ContractMeta, root: &CommandNode) -> Vec<ContractFailure> {
    let mut failures = Vec::new();
    let field = "must_describe_positional_by_path";
    for (path, names) in &contract.must_describe_positional_by_path {
        for (name, want) in names {
            let Some(entity) = operand(root, field, path, name, &mut failures) else {
                continue;
            };
            let want = contract::collapse_whitespace(want);
            if !text_of(entity).contains(&want) {
                failures.push(ContractFailure(format!(
                    "{field}[{path:?}][{name:?}]: expected description to contain {want:?}, got {:?}",
                    text_of(entity)
                )));
            }
        }
    }
    let field = "must_not_describe_positional_by_path";
    for (path, names) in &contract.must_not_describe_positional_by_path {
        for (name, unwanted) in names {
            let Some(node) = contract::find_node_by_path(root, path) else {
                continue;
            };
            let Some(entity) = node.positionals().find(|p| p.primary_name() == name) else {
                continue;
            };
            if text_of(entity).contains(&contract::collapse_whitespace(unwanted)) {
                failures.push(ContractFailure(format!(
                    "{field}[{path:?}][{name:?}]: description contains {unwanted:?}: {:?}",
                    text_of(entity)
                )));
            }
        }
    }
    let field = "must_attach_positional_choices_by_path";
    for (path, names) in &contract.must_attach_positional_choices_by_path {
        for (name, want) in names {
            let Some(entity) = operand(root, field, path, name, &mut failures) else {
                continue;
            };
            let missing: Vec<&str> = want
                .iter()
                .filter(|w| !entity.choices.iter().any(|c| &c.name == *w))
                .map(String::as_str)
                .collect();
            if !missing.is_empty() {
                failures.push(ContractFailure(format!(
                    "{field}[{path:?}][{name:?}]: missing choices {}",
                    missing.join(", ")
                )));
            }
        }
    }
    failures
}
