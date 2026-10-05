//! `must_keep_separate_by_path`: `must_keep_separate` for a subcommand's own
//! flags, keyed by path. Split out of `contract.rs` for the 800-code-line
//! ceiling.

use super::contract::{entity_matches_flag_spec, find_node_by_path, ContractFailure};
use super::ContractMeta;
use mandible_core::CommandNode;

/// A dropped group retires the only statement that its spellings never
/// fused, as `must_keep_separate` does.
pub(crate) fn weakened_lines(label: &str, b: &ContractMeta, n: &ContractMeta) -> Vec<String> {
    let mut lines = Vec::new();
    for (path, groups) in &b.must_keep_separate_by_path {
        let now = n.must_keep_separate_by_path.get(path);
        for group in groups {
            if !now.is_some_and(|gs| gs.contains(group)) {
                lines.push(format!(
                    "CONTRACT WEAKENED: {label} must_keep_separate_by_path[{path:?}] (dropped: {group:?})"
                ));
            }
        }
    }
    lines
}

/// One failure per group whose spellings share an entity of the node at
/// `path`. A path that resolves to no node fails by name; a spelling absent
/// from the node never collapses with anything.
pub(crate) fn failures(contract: &ContractMeta, root: &CommandNode) -> Vec<ContractFailure> {
    let mut failures = Vec::new();
    for (path, groups) in &contract.must_keep_separate_by_path {
        let Some(node) = find_node_by_path(root, path) else {
            failures.push(ContractFailure(format!(
                "must_keep_separate_by_path: no node at path {path:?}"
            )));
            continue;
        };
        for group in groups {
            let mut by_entity: std::collections::BTreeMap<usize, Vec<&str>> =
                std::collections::BTreeMap::new();
            for spelling in group {
                let found = node
                    .flags()
                    .position(|f| entity_matches_flag_spec(f, spelling));
                if let Some(idx) = found {
                    by_entity.entry(idx).or_default().push(spelling.as_str());
                }
            }
            let collapsed: Vec<String> = by_entity
                .into_values()
                .filter(|spellings| spellings.len() > 1)
                .map(|spellings| spellings.join(", "))
                .collect();
            if !collapsed.is_empty() {
                failures.push(ContractFailure(format!(
                    "must_keep_separate_by_path[{path:?}]: {group:?} collapsed onto one entity: {}",
                    collapsed.join("; ")
                )));
            }
        }
    }
    failures
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandible_core::{Entity, Provenance, Source, Spelling};

    fn prov() -> Provenance {
        Provenance::single(Source::HelpText)
    }

    fn contract() -> ContractMeta {
        let mut c = ContractMeta::default();
        c.must_keep_separate_by_path
            .insert("sub".into(), vec![vec!["-n".into(), "--table".into()]]);
        c
    }

    fn tree(fused: bool) -> CommandNode {
        let mut root = CommandNode::new("tool", prov());
        let mut sub = CommandNode::new("sub", prov());
        let mut first = Entity::flag_short('n', prov());
        if fused {
            first.spellings.push(Spelling::long("table"));
        } else {
            sub.entities.push(Entity::flag_long("table", prov()));
        }
        sub.entities.push(first);
        root.subcommands.push(sub);
        root
    }

    #[test]
    fn separate_flags_of_the_subcommand_pass() {
        assert!(failures(&contract(), &tree(false)).is_empty());
    }

    #[test]
    fn spellings_fused_on_the_subcommand_fail_naming_path_and_group() {
        let got = failures(&contract(), &tree(true));
        assert_eq!(got.len(), 1);
        assert_eq!(
            got[0].0,
            "must_keep_separate_by_path[\"sub\"]: [\"-n\", \"--table\"] collapsed onto one entity: -n, --table"
        );
    }

    #[test]
    fn an_unknown_path_fails_by_name() {
        let got = failures(&contract(), &CommandNode::new("tool", prov()));
        assert_eq!(
            got[0].0,
            "must_keep_separate_by_path: no node at path \"sub\""
        );
    }

    #[test]
    fn a_dropped_group_weakens_the_contract() {
        let lines = weakened_lines("t/1", &contract(), &ContractMeta::default());
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("must_keep_separate_by_path"));
    }
}
