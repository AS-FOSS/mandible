//! Checking one fixture's `[contract]` against its extracted tree, and detecting when a contract weakened against a baseline.
use super::*;

/// Contract-weakening detection: lowering `min_subcommands`, shrinking
/// `must_contain_flags`, or marking a previously-enforced fixture
/// `[xfail]` all make a real failure disappear silently.
/// `corpus/README.md` permits weakening a contract via an explicit,
/// justified edit — this makes sure a reviewer sees it happen.
///
/// This module has no git access (`tests/no_process_outside_exec.rs`
/// forbids `std::process` in `xtask/src`), so it diffs `[contract]` fields
/// between the current corpus and a second plain directory
/// (`baseline_root`, populated by whatever invokes this binary via
/// `--baseline-dir`, e.g. `git archive <base-ref> corpus | tar -x`). With
/// no `--baseline-dir`, this function is never called.
///
/// Returns one `"CONTRACT WEAKENED: <fixture> <field>"` line per weakened
/// field. Reported, not gated — a contract may legitimately weaken.
pub(crate) fn contract_weakened_lines(current: &[Fixture], baseline: &[Fixture]) -> Vec<String> {
    let mut lines = Vec::new();
    for base in baseline {
        let Some(now) = current.iter().find(|f| f.label == base.label) else {
            lines.push(format!(
                "CONTRACT WEAKENED: {} fixture-removed (present in baseline, missing now)",
                base.label
            ));
            continue;
        };
        let (b, n) = (&base.meta.contract, &now.meta.contract);

        // A framework assertion that's simply gone is a removed check —
        // never flagged for merely *changing* to a different framework
        // name, since that has no natural "weaker/stronger" ordering and a
        // real detection improvement legitimately changes it.
        if b.expected_framework.is_some() && n.expected_framework.is_none() {
            lines.push(format!(
                "CONTRACT WEAKENED: {} expected_framework (assertion removed)",
                base.label
            ));
        }

        if let Some(base_status) = &b.min_status {
            let base_rank = crate::status::status_rank(base_status);
            let now_rank = n.min_status.as_deref().and_then(crate::status::status_rank);
            if now_rank < base_rank {
                lines.push(format!(
                    "CONTRACT WEAKENED: {} min_status ({:?} -> {:?})",
                    base.label,
                    base_status,
                    n.min_status.as_deref().unwrap_or("(removed)"),
                ));
            }
        }

        if let Some(base_min) = b.min_subcommands {
            let now_min = n.min_subcommands.unwrap_or(0);
            if now_min < base_min {
                lines.push(format!(
                    "CONTRACT WEAKENED: {} min_subcommands ({base_min} -> {now_min})",
                    base.label
                ));
            }
        }

        let missing_flags: Vec<&str> = b
            .must_contain_flags
            .iter()
            .filter(|spec| !n.must_contain_flags.iter().any(|s| s == *spec))
            .map(String::as_str)
            .collect();
        if !missing_flags.is_empty() {
            lines.push(format!(
                "CONTRACT WEAKENED: {} must_contain_flags (dropped: {})",
                base.label,
                missing_flags.join(", ")
            ));
        }

        // A negative claim weakens by *losing an entry*, exactly as a
        // positive one does — the direction of the claim flips, the
        // direction of its weakening does not. Dropping
        // `must_not_contain_flags = ["---...---"]` retires the only
        // statement that the mariadb ruler is a phantom, and would let the
        // defect return unremarked. Adding an entry tightens, and is never
        // flagged, same as `must_contain_flags`.
        let dropped_forbidden: Vec<&str> = b
            .must_not_contain_flags
            .iter()
            .filter(|spec| !n.must_not_contain_flags.iter().any(|s| s == *spec))
            .map(String::as_str)
            .collect();
        if !dropped_forbidden.is_empty() {
            lines.push(format!(
                "CONTRACT WEAKENED: {} must_not_contain_flags (dropped: {})",
                base.label,
                dropped_forbidden.join(", ")
            ));
        }

        let missing_positionals: Vec<&str> = b
            .must_contain_positionals
            .iter()
            .filter(|name| !n.must_contain_positionals.iter().any(|s| s == *name))
            .map(String::as_str)
            .collect();
        if !missing_positionals.is_empty() {
            lines.push(format!(
                "CONTRACT WEAKENED: {} must_contain_positionals (dropped: {})",
                base.label,
                missing_positionals.join(", ")
            ));
        }

        let missing_modifiers: Vec<&str> = b
            .must_contain_modifiers
            .iter()
            .filter(|name| !n.must_contain_modifiers.iter().any(|s| s == *name))
            .map(String::as_str)
            .collect();
        if !missing_modifiers.is_empty() {
            lines.push(format!(
                "CONTRACT WEAKENED: {} must_contain_modifiers (dropped: {})",
                base.label,
                missing_modifiers.join(", ")
            ));
        }

        let missing_env_vars: Vec<&str> = b
            .must_contain_env_vars
            .iter()
            .filter(|name| !n.must_contain_env_vars.iter().any(|s| s == *name))
            .map(String::as_str)
            .collect();
        if !missing_env_vars.is_empty() {
            lines.push(format!(
                "CONTRACT WEAKENED: {} must_contain_env_vars (dropped: {})",
                base.label,
                missing_env_vars.join(", ")
            ));
        }

        for (path, base_specs) in &b.must_contain_flags_by_path {
            let now_specs = n.must_contain_flags_by_path.get(path);
            let missing: Vec<&str> = base_specs
                .iter()
                .filter(|spec| !now_specs.is_some_and(|specs| specs.iter().any(|s| s == *spec)))
                .map(String::as_str)
                .collect();
            if !missing.is_empty() {
                lines.push(format!(
                    "CONTRACT WEAKENED: {} must_contain_flags_by_path[{path:?}] (dropped: {})",
                    base.label,
                    missing.join(", ")
                ));
            }
        }

        lines.extend(new_field_weakened_lines(&base.label, b, n));

        let base_xfail = base.meta.xfail.as_ref().is_some_and(|x| x.broken);
        let now_xfail = now.meta.xfail.as_ref().is_some_and(|x| x.broken);
        if !base_xfail && now_xfail {
            lines.push(format!(
                "CONTRACT WEAKENED: {} xfail (newly marked broken — contract failures no longer fail the run)",
                base.label
            ));
        }
    }
    lines
}

/// The weakening checks for the three fields added alongside
/// `must_keep_separate`, `must_attach_choices` and `must_describe` — split
/// out of [`contract_weakened_lines`] to keep that function under the
/// workspace's line-count lint.
fn new_field_weakened_lines(label: &str, b: &ContractMeta, n: &ContractMeta) -> Vec<String> {
    let mut lines = Vec::new();

    // `must_keep_separate`: a dropped group retires the only statement
    // that its spellings never fused, exactly the reasoning
    // `must_not_contain_flags` above already carries for its own negative
    // claim.
    let dropped_groups: Vec<String> = b
        .must_keep_separate
        .iter()
        .filter(|group| !n.must_keep_separate.iter().any(|g| g == *group))
        .map(|group| format!("{group:?}"))
        .collect();
    if !dropped_groups.is_empty() {
        lines.push(format!(
            "CONTRACT WEAKENED: {label} must_keep_separate (dropped: {})",
            dropped_groups.join(", ")
        ));
    }

    for (flag, base_choices) in &b.must_attach_choices {
        let now_choices = n.must_attach_choices.get(flag);
        let missing: Vec<&str> = base_choices
            .iter()
            .filter(|c| !now_choices.is_some_and(|cs| cs.iter().any(|x| x == *c)))
            .map(String::as_str)
            .collect();
        if !missing.is_empty() {
            lines.push(format!(
                "CONTRACT WEAKENED: {label} must_attach_choices[{flag:?}] (dropped: {})",
                missing.join(", ")
            ));
        }
    }

    // `must_describe`: only flagged when the assertion disappears
    // entirely, mirroring `expected_framework`'s own rule — a changed
    // expected substring has no natural stronger/weaker ordering, so only
    // its outright removal is reported.
    for flag in b.must_describe.keys() {
        if !n.must_describe.contains_key(flag) {
            lines.push(format!(
                "CONTRACT WEAKENED: {label} must_describe[{flag:?}] (assertion removed)"
            ));
        }
    }

    // `must_value_name`: same rule as `must_describe` above, plus the
    // repetition half of `must_contain_positionals` — dropping a `...`
    // suffix retires the claim that the marker survived, which no
    // string-equality drop check would see.
    for flag in b.must_value_name.keys() {
        if !n.must_value_name.contains_key(flag) {
            lines.push(format!(
                "CONTRACT WEAKENED: {label} must_value_name[{flag:?}] (assertion removed)"
            ));
        }
    }
    for spec in b.must_contain_positionals.iter() {
        let Some(base) = spec.strip_suffix("...") else {
            continue;
        };
        if n.must_contain_positionals.iter().any(|s| s == spec) {
            continue;
        }
        if n.must_contain_positionals.iter().any(|s| s == base) {
            lines.push(format!(
                "CONTRACT WEAKENED: {label} must_contain_positionals[{base:?}] (repetition marker dropped)"
            ));
        }
    }

    lines
}

/// A single `[contract]` field that failed, human-readable and naming the
/// actual value alongside what was required — spec's own example of a
/// good failure message (`corpus/README.md`'s companion work order):
/// "git: min_subcommands 20, got 23 — OK; snapshot mismatch at
/// .positionals[1].name".
pub(crate) struct ContractFailure(pub(crate) String);

/// Check every field the `[contract]` sets against `root`, returning one
/// [`ContractFailure`] per violated field (empty = every check that was
/// actually specified passed). A field left unset in `meta.toml` asserts
/// nothing and is silently skipped.
pub(crate) fn check_contract(
    contract: &ContractMeta,
    root: Option<&CommandNode>,
) -> Vec<ContractFailure> {
    let Some(root) = root else {
        return check_contract_missing_root(contract);
    };
    let mut failures = check_contract_scalar_fields(contract, root);
    failures.extend(check_contract_collection_fields(contract, root));
    failures
}

/// No root at all trivially fails every contract field that was actually
/// specified — name them all rather than one opaque "no root" line, so the
/// report reads the same shape whether the failure is "wrong tree" or "no
/// tree".
///
/// `must_not_contain_flags` and `must_keep_separate` are deliberately
/// absent from this list. Every field above is a positive claim, which a
/// missing tree trivially breaks — "the tool has --paginate" cannot hold
/// of no tree. A negative claim is the opposite: "no root flag is spelled
/// X" is *satisfied* by a tree with no flags at all, so reporting it here
/// would announce a violation of a promise that in fact holds, which is a
/// false positive in the one place this runner's authority comes from.
/// `must_keep_separate` is negative the same way — "these spellings never
/// fused" holds vacuously when none of them exist to fuse. `must_attach_choices`
/// and `must_describe` are positive claims ("this flag exists and carries
/// this"), so a missing root fails them exactly as it fails
/// `must_contain_flags`. A fixture that produced no root still fails
/// loudly — on its snapshot, and on every positive field it set.
fn check_contract_missing_root(contract: &ContractMeta) -> Vec<ContractFailure> {
    let mut failures = Vec::new();
    if contract.expected_framework.is_some() {
        failures.push(ContractFailure(
            "expected_framework: no root produced".into(),
        ));
    }
    if contract.min_status.is_some() {
        failures.push(ContractFailure("min_status: no root produced".into()));
    }
    if contract.min_subcommands.is_some() {
        failures.push(ContractFailure("min_subcommands: no root produced".into()));
    }
    if !contract.must_contain_flags.is_empty() {
        failures.push(ContractFailure(
            "must_contain_flags: no root produced".into(),
        ));
    }
    if !contract.must_contain_flags_by_path.is_empty() {
        failures.push(ContractFailure(
            "must_contain_flags_by_path: no root produced".into(),
        ));
    }
    if !contract.must_contain_positionals.is_empty() {
        failures.push(ContractFailure(
            "must_contain_positionals: no root produced".into(),
        ));
    }
    if !contract.must_contain_modifiers.is_empty() {
        failures.push(ContractFailure(
            "must_contain_modifiers: no root produced".into(),
        ));
    }
    if !contract.must_contain_env_vars.is_empty() {
        failures.push(ContractFailure(
            "must_contain_env_vars: no root produced".into(),
        ));
    }
    if !contract.must_attach_choices.is_empty() {
        failures.push(ContractFailure(
            "must_attach_choices: no root produced".into(),
        ));
    }
    if !contract.must_describe.is_empty() {
        failures.push(ContractFailure("must_describe: no root produced".into()));
    }
    if !contract.must_value_name.is_empty() {
        failures.push(ContractFailure("must_value_name: no root produced".into()));
    }
    failures
}

/// The scalar `[contract]` fields: `expected_framework`, `min_status`,
/// `min_subcommands`, `must_contain_flags`, `must_not_contain_flags`.
fn check_contract_scalar_fields(
    contract: &ContractMeta,
    root: &CommandNode,
) -> Vec<ContractFailure> {
    let mut failures = Vec::new();

    if let Some(expected) = &contract.expected_framework {
        let actual = root
            .detected_framework
            .clone()
            .unwrap_or_else(|| "generic".to_string());
        if &actual != expected {
            failures.push(ContractFailure(format!(
                "expected_framework: expected {expected:?}, got {actual:?}"
            )));
        }
    }

    if let Some(min_status) = &contract.min_status {
        let result_stub = extraction_result_stub(root.clone());
        let status = crate::status::compute(&result_stub);
        if !crate::status::meets_min_status(status.label, min_status) {
            failures.push(ContractFailure(format!(
                "min_status: required at least {min_status:?}, got {:?}",
                status.label
            )));
        }
    }

    if let Some(min) = contract.min_subcommands {
        let got = root.subcommands.len();
        if got < min {
            failures.push(ContractFailure(format!(
                "min_subcommands: required at least {min}, got {got}"
            )));
        }
    }

    let missing_flags: Vec<&str> = contract
        .must_contain_flags
        .iter()
        .filter(|spec| !flag_present(root, spec))
        .map(|s| s.as_str())
        .collect();
    if !missing_flags.is_empty() {
        failures.push(ContractFailure(format!(
            "must_contain_flags: missing {}",
            missing_flags.join(", ")
        )));
    }

    // The negative claim: spellings the parser must not have invented.
    // Same matcher as `must_contain_flags`, same root-only scope, negated.
    let present_forbidden: Vec<&str> = contract
        .must_not_contain_flags
        .iter()
        .filter(|spec| flag_present(root, spec))
        .map(|s| s.as_str())
        .collect();
    if !present_forbidden.is_empty() {
        failures.push(ContractFailure(format!(
            "must_not_contain_flags: present {}",
            present_forbidden.join(", ")
        )));
    }

    // The other negative-claim shape: not "this spelling was invented",
    // but "these spellings, which really exist, must not have been folded
    // onto one entity" — the alias-run fold's own failure mode. Each group
    // is checked independently; one `ContractFailure` per group that
    // actually collapsed, naming the group and which of its spellings
    // share an entity.
    for group in &contract.must_keep_separate {
        let mut by_entity: std::collections::BTreeMap<usize, Vec<&str>> =
            std::collections::BTreeMap::new();
        for spelling in group {
            if let Some(idx) = resolve_flag_entity(root, spelling) {
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
                "must_keep_separate: {group:?} collapsed onto one entity: {}",
                collapsed.join("; ")
            )));
        }
    }

    failures
}

/// The collection-shaped `[contract]` fields: `must_contain_positionals`,
/// `must_contain_modifiers`, `must_contain_env_vars`,
/// `must_contain_flags_by_path`.
fn check_contract_collection_fields(
    contract: &ContractMeta,
    root: &CommandNode,
) -> Vec<ContractFailure> {
    let mut failures = Vec::new();

    let missing_positionals: Vec<&str> = contract
        .must_contain_positionals
        .iter()
        .filter(|spec| !positional_present(root, spec))
        .map(|s| s.as_str())
        .collect();
    if !missing_positionals.is_empty() {
        failures.push(ContractFailure(format!(
            "must_contain_positionals: missing {}",
            missing_positionals.join(", ")
        )));
    }

    let missing_modifiers: Vec<&str> = contract
        .must_contain_modifiers
        .iter()
        .filter(|name| !root.modifiers().any(|m| m.primary_name() == name.as_str()))
        .map(|s| s.as_str())
        .collect();
    if !missing_modifiers.is_empty() {
        failures.push(ContractFailure(format!(
            "must_contain_modifiers: missing {}",
            missing_modifiers.join(", ")
        )));
    }

    let missing_env_vars: Vec<&str> = contract
        .must_contain_env_vars
        .iter()
        .filter(|name| !root.env_vars().any(|v| v.primary_name() == name.as_str()))
        .map(|s| s.as_str())
        .collect();
    if !missing_env_vars.is_empty() {
        failures.push(ContractFailure(format!(
            "must_contain_env_vars: missing {}",
            missing_env_vars.join(", ")
        )));
    }

    for (path, specs) in &contract.must_contain_flags_by_path {
        let Some(node) = find_node_by_path(root, path) else {
            failures.push(ContractFailure(format!(
                "must_contain_flags_by_path: no node at path {path:?}"
            )));
            continue;
        };
        let missing: Vec<&str> = specs
            .iter()
            .filter(|spec| !flag_present(node, spec))
            .map(|s| s.as_str())
            .collect();
        if !missing.is_empty() {
            failures.push(ContractFailure(format!(
                "must_contain_flags_by_path[{path:?}]: missing {}",
                missing.join(", ")
            )));
        }
    }

    for (flag_spec, wanted_choices) in &contract.must_attach_choices {
        match root
            .flags()
            .find(|f| entity_matches_flag_spec(f, flag_spec))
        {
            None => failures.push(ContractFailure(format!(
                "must_attach_choices[{flag_spec:?}]: flag not present"
            ))),
            Some(entity) => {
                let missing: Vec<&str> = wanted_choices
                    .iter()
                    .filter(|c| !entity.choices.iter().any(|ch| &ch.name == *c))
                    .map(|s| s.as_str())
                    .collect();
                if !missing.is_empty() {
                    failures.push(ContractFailure(format!(
                        "must_attach_choices[{flag_spec:?}]: missing {}",
                        missing.join(", ")
                    )));
                }
            }
        }
    }

    for (flag_spec, expected_text) in &contract.must_describe {
        match root
            .flags()
            .find(|f| entity_matches_flag_spec(f, flag_spec))
        {
            None => failures.push(ContractFailure(format!(
                "must_describe[{flag_spec:?}]: flag not present"
            ))),
            Some(entity) => {
                let actual = entity.description.as_ref().map_or("", |t| t.as_str());
                let actual_collapsed = collapse_whitespace(actual);
                let expected_collapsed = collapse_whitespace(expected_text);
                if !actual_collapsed.contains(&expected_collapsed) {
                    failures.push(ContractFailure(format!(
                        "must_describe[{flag_spec:?}]: expected description to contain {:?}, got {:?}",
                        expected_collapsed,
                        truncate_for_display(&actual_collapsed, 120)
                    )));
                }
            }
        }
    }

    failures.extend(check_must_value_name(contract, root));

    failures
}

/// `must_value_name`: a flag's value placeholder must still carry the text
/// the tool's own row shows. Scans EVERY matching entity, unlike
/// `must_describe`'s first match, because one spelling can head two rows
/// (`corpus/vim.basic/audit-seed4` documents `-r` twice).
fn check_must_value_name(contract: &ContractMeta, root: &CommandNode) -> Vec<ContractFailure> {
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

/// Whether `root` carries the positional `spec` names. A trailing `...`
/// additionally requires the operand to be repeatable, which is the only
/// way a contract can state that a repetition marker survived
/// (`corpus/aarch64-linux-gnu-dwp/2.42`, atlas S-042).
fn positional_present(root: &CommandNode, spec: &str) -> bool {
    let (name, must_repeat) = match spec.strip_suffix("...") {
        Some(base) => (base, true),
        None => (spec, false),
    };
    root.positionals()
        .any(|p| p.primary_name() == name && (!must_repeat || p.repeatable))
}

/// Resolve a space-separated subcommand path (`"restore"`, `"remote add"`)
/// against `root`'s own `subcommands`, one path segment per level — the
/// same walk [`mandible_core::noderef::resolve`] does for the TUI's own
/// addressing, reimplemented narrowly here rather than pulled in because
/// this only ever needs a name match, never alias resolution.
fn find_node_by_path<'a>(root: &'a CommandNode, path: &str) -> Option<&'a CommandNode> {
    let mut node = root;
    for segment in path.split_whitespace() {
        node = node.subcommands.iter().find(|c| c.name == segment)?;
    }
    Some(node)
}

/// Wrap a root already produced by [`extract_tree`] back into an
/// [`mandible_extract::ExtractionResult`] shape so [`crate::status::compute`]
/// (which the coverage harness also drives, spec's "one status
/// definition" requirement) can be reused here without a second
/// implementation. `tier_statuses`/`tool`/`elapsed` are irrelevant to
/// `status::compute`, which only ever looks at `root`.
fn extraction_result_stub(root: CommandNode) -> mandible_extract::ExtractionResult {
    mandible_extract::ExtractionResult {
        tool: root.name.clone(),
        root: Some(root),
        tier_statuses: Vec::new(),
        elapsed: Duration::ZERO,
    }
}

/// Whether `node`'s own flags satisfy a `must_contain_flags`/
/// `must_contain_flags_by_path`/`must_not_contain_flags` spec (the last
/// negated by its caller): `--long-name` matches any
/// [`mandible_core::Spelling`] with two dashes and that name, `-x` matches
/// any single-dash single-character spelling, anything else is matched
/// against every spelling's bare name verbatim. Checks only the one node
/// given, never recursing.
///
/// Checks every spelling, not just `Entity::short`/`Entity::long` (which
/// return one canonical spelling each): `fold_adjacent_alias_rows` can put
/// more than one short spelling on an entity (`ffplay`'s `-h, -?, -help,
/// --help`), so a contract asserting `-?` must still find it once `-h`
/// claims the canonical slot.
fn flag_present(node: &CommandNode, spec: &str) -> bool {
    node.flags().any(|f| entity_matches_flag_spec(f, spec))
}

/// The single-entity half of [`flag_present`]'s matching rule, split out
/// so [`resolve_flag_entity`] can find *which* entity a spelling resolves
/// to (for `must_keep_separate`) rather than only whether any entity
/// matches.
fn entity_matches_flag_spec(entity: &Entity, spec: &str) -> bool {
    // The bare end-of-options marker: `spec.strip_prefix("--")` below
    // would read `"--"` as a long spelling with an empty name, which no
    // real entity can ever have — checked first so a contract can state
    // `"--"` and mean the literal marker, matching how `"+"` already
    // falls through to the literal-name branch a few lines down.
    if spec == "--" {
        return entity
            .spellings
            .iter()
            .any(|s| matches!(s.dashes, Dashes::None) && s.name == "--");
    }
    if let Some(long) = spec.strip_prefix("--") {
        // Long-*like*, matching `Entity::long`'s own shape rule exactly
        // (never narrowed to `Dashes::Double` alone): two dashes, or one
        // dash with more than one character — a single-dash long option
        // (`ptargrep`'s own `-message`, `Dashes::Single`, four letters)
        // must still satisfy a `--message` contract entry the way
        // `Entity::long()` always considered it to.
        entity.spellings.iter().any(|s| {
            (matches!(s.dashes, Dashes::Double)
                || (matches!(s.dashes, Dashes::Single) && s.name.chars().count() > 1))
                && s.name == long
        })
    } else if let Some(short) = spec.strip_prefix('-') {
        short.chars().next().is_some_and(|c| {
            entity.spellings.iter().any(|s| {
                matches!(s.dashes, Dashes::Single)
                    && s.name.chars().count() == 1
                    && s.name.starts_with(c)
            })
        })
    } else {
        entity.spellings.iter().any(|s| s.name == spec)
    }
}

/// Which root flag entity (by index into `node.flags()`'s own iteration
/// order) a spelling resolves to, or `None` if nothing matches — the
/// building block `must_keep_separate` needs to tell "two spellings match
/// two different entities" apart from "two spellings match the same
/// entity", which mere presence (`flag_present`) cannot distinguish.
fn resolve_flag_entity(node: &CommandNode, spec: &str) -> Option<usize> {
    node.flags()
        .enumerate()
        .find(|(_, f)| entity_matches_flag_spec(f, spec))
        .map(|(i, _)| i)
}

/// Truncate `s` to at most `max` chars for a readable failure message,
/// appending `"..."` when truncated.
fn truncate_for_display(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max).collect();
        format!("{truncated}...")
    }
}

/// Collapse runs of whitespace to a single space and trim the ends —
/// `must_describe`'s comparison rule, applied to both sides, since a
/// description wraps and a fixture author's TOML value may too.
fn collapse_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}
