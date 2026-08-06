//! Two-axis merge (spec §4.4) and alias pairing (spec §4.4, "Flags unify by
//! alias pairing").
//!
//! Merge combines several extraction results for *the same logical node*
//! (one [`CommandNode`] per contributing tier) into one. Each field is
//! resolved against whichever axis governs it — [`Axis::Structural`] for
//! names/nesting/arity/existence, [`Axis::Prose`] for descriptions/
//! summaries/examples — using the highest-authority contributing source on
//! that axis. `None`/empty never displaces a value. Ties break toward the
//! earlier contributor. This is deliberately *not* "first tier in attempt
//! order wins": attempt order is a cost ordering (spec §7); conflict
//! resolution is authority (spec §4.4).

use crate::node::{CommandNode, Flag, Positional};
use crate::provenance::{Axis, Provenance};
use crate::text::Text;
use std::collections::HashMap;
use thiserror::Error;

/// Errors merge can return.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum MergeError {
    /// `merge_nodes` was called with no candidates.
    #[error("cannot merge zero candidate nodes")]
    Empty,
}

/// Merge several same-node extraction results into one, per spec §4.4.
///
/// `candidates` should all represent the same logical command (same name /
/// same position in the tree), each typically produced by a single
/// [`ExtractionTier`](../mandible_extract/trait.ExtractionTier.html) and
/// carrying that tier's `Source` in its own `provenance`. Order matters only
/// for tie-breaking (earlier wins).
pub fn merge_nodes(mut candidates: Vec<CommandNode>) -> Result<CommandNode, MergeError> {
    if candidates.is_empty() {
        return Err(MergeError::Empty);
    }
    if candidates.len() == 1 {
        let mut only = candidates.pop().expect("len checked above");
        only.flags = pair_aliases(only.flags);
        return Ok(only);
    }

    // Alias-pair each candidate's own flags before it participates in
    // cross-source merge (spec §4.4: "Pairing runs before merge").
    for c in &mut candidates {
        let flags = std::mem::take(&mut c.flags);
        c.flags = pair_aliases(flags);
    }

    let name = pick_option(
        candidates.iter().map(|c| {
            (
                &c.provenance,
                if c.name.is_empty() {
                    None
                } else {
                    Some(&c.name)
                },
            )
        }),
        Axis::Structural,
    )
    .unwrap_or_else(|| candidates[0].name.clone());

    let mut aliases: Vec<String> = Vec::new();
    for c in &candidates {
        for a in &c.aliases {
            if !aliases.contains(a) {
                aliases.push(a.clone());
            }
        }
    }

    let summary = pick_option(
        candidates
            .iter()
            .map(|c| (&c.provenance, c.summary.as_ref())),
        Axis::Prose,
    );
    let description = pick_option(
        candidates
            .iter()
            .map(|c| (&c.provenance, c.description.as_ref())),
        Axis::Prose,
    );
    let usage = pick_vec(
        candidates.iter().map(|c| (&c.provenance, &c.usage)),
        Axis::Structural,
    );
    let deprecated = pick_option(
        candidates
            .iter()
            .map(|c| (&c.provenance, c.deprecated.as_ref())),
        Axis::Prose,
    );
    let group = pick_option(
        candidates.iter().map(|c| (&c.provenance, c.group.as_ref())),
        Axis::Structural,
    );

    let structural_winner_idx =
        best_index(candidates.iter().map(|c| &c.provenance), Axis::Structural);
    let hidden = candidates[structural_winner_idx].hidden;
    let children_filled = candidates.iter().any(|c| c.children_filled);

    let mut provenance = Provenance::default();
    for c in &candidates {
        provenance.absorb(&c.provenance);
    }

    let flags = merge_flag_lists(candidates.iter().map(|c| c.flags.clone()).collect());
    let positionals =
        merge_positional_lists(candidates.iter().map(|c| c.positionals.clone()).collect());
    let subcommands =
        merge_subcommand_lists(candidates.iter().map(|c| c.subcommands.clone()).collect())?;
    let examples = merge_examples(candidates.iter().map(|c| c.examples.clone()).collect());

    Ok(CommandNode {
        name,
        aliases,
        summary,
        description,
        usage,
        flags,
        positionals,
        subcommands,
        examples,
        hidden,
        deprecated,
        children_filled,
        group,
        provenance,
    })
}

/// Merge flags by identity (long name, else short letter, else
/// description-as-fallback) across several already alias-paired flag lists,
/// applying the same two-axis authority resolution per field.
pub fn merge_flag_lists(lists: Vec<Vec<Flag>>) -> Vec<Flag> {
    let mut order: Vec<String> = Vec::new();
    let mut buckets: HashMap<String, Vec<Flag>> = HashMap::new();
    for list in lists {
        for flag in list {
            let key = flag_identity(&flag);
            if !buckets.contains_key(&key) {
                order.push(key.clone());
            }
            buckets.entry(key).or_default().push(flag);
        }
    }
    order
        .into_iter()
        .map(|key| {
            let bucket = buckets.remove(&key).expect("key came from this map");
            merge_flag_bucket(bucket)
        })
        .collect()
}

fn flag_identity(f: &Flag) -> String {
    match (&f.long, f.short) {
        (Some(l), _) => format!("L:{l}"),
        (None, Some(s)) => format!("S:{s}"),
        (None, None) => format!(
            "D:{}",
            f.description.as_ref().map(|d| d.as_str()).unwrap_or("")
        ),
    }
}

fn merge_flag_bucket(mut bucket: Vec<Flag>) -> Flag {
    if bucket.len() == 1 {
        return bucket.pop().expect("len checked");
    }

    let short = bucket.iter().find_map(|f| f.short);
    let long = pick_option(
        bucket.iter().map(|f| (&f.provenance, f.long.as_ref())),
        Axis::Structural,
    );
    let value_name = pick_option(
        bucket
            .iter()
            .map(|f| (&f.provenance, f.value_name.as_ref())),
        Axis::Structural,
    );
    let value_kind = bucket
        .iter()
        .map(|f| f.value_kind)
        .max_by_key(|k| match k {
            crate::node::ValueKind::None => 0,
            crate::node::ValueKind::Optional => 1,
            crate::node::ValueKind::Required => 2,
        })
        .unwrap_or(crate::node::ValueKind::None);
    let choices = pick_vec(
        bucket.iter().map(|f| (&f.provenance, &f.choices)),
        Axis::Prose,
    );
    let repeatable = bucket.iter().any(|f| f.repeatable);
    let required = bucket.iter().any(|f| f.required);
    let hidden = bucket.iter().all(|f| f.hidden) && !bucket.is_empty();
    let deprecated = pick_option(
        bucket
            .iter()
            .map(|f| (&f.provenance, f.deprecated.as_ref())),
        Axis::Prose,
    );
    let inherited = bucket.iter().any(|f| f.inherited);
    let group = pick_option(
        bucket.iter().map(|f| (&f.provenance, f.group.as_ref())),
        Axis::Structural,
    );
    let description = pick_option(
        bucket
            .iter()
            .map(|f| (&f.provenance, f.description.as_ref())),
        Axis::Prose,
    );
    let default = pick_option(
        bucket.iter().map(|f| (&f.provenance, f.default.as_ref())),
        Axis::Prose,
    );
    let env_var = pick_option(
        bucket.iter().map(|f| (&f.provenance, f.env_var.as_ref())),
        Axis::Structural,
    );

    let mut provenance = Provenance::default();
    for f in &bucket {
        provenance.absorb(&f.provenance);
    }

    Flag {
        short,
        long,
        value_name,
        value_kind,
        choices,
        repeatable,
        required,
        hidden,
        deprecated,
        inherited,
        group,
        description,
        default,
        env_var,
        provenance,
    }
}

/// Merge positionals across candidate lists by name identity.
pub fn merge_positional_lists(lists: Vec<Vec<Positional>>) -> Vec<Positional> {
    let mut order: Vec<String> = Vec::new();
    let mut buckets: HashMap<String, Vec<Positional>> = HashMap::new();
    for list in lists {
        for p in list {
            if !buckets.contains_key(&p.name) {
                order.push(p.name.clone());
            }
            buckets.entry(p.name.clone()).or_default().push(p);
        }
    }
    order
        .into_iter()
        .map(|name| {
            let mut bucket = buckets.remove(&name).expect("key came from this map");
            if bucket.len() == 1 {
                return bucket.pop().expect("len checked");
            }
            let required = bucket.iter().any(|p| p.required);
            let variadic = bucket.iter().any(|p| p.variadic);
            let description = pick_option(
                bucket
                    .iter()
                    .map(|p| (&p.provenance, p.description.as_ref())),
                Axis::Prose,
            );
            let mut provenance = Provenance::default();
            for p in &bucket {
                provenance.absorb(&p.provenance);
            }
            Positional {
                name,
                required,
                variadic,
                description,
                provenance,
            }
        })
        .collect()
}

/// Merge subcommand lists recursively by name (spec §4.4: "Subcommands
/// merge recursively by name").
pub fn merge_subcommand_lists(
    lists: Vec<Vec<CommandNode>>,
) -> Result<Vec<CommandNode>, MergeError> {
    let mut order: Vec<String> = Vec::new();
    let mut buckets: HashMap<String, Vec<CommandNode>> = HashMap::new();
    for list in lists {
        for c in list {
            if !buckets.contains_key(&c.name) {
                order.push(c.name.clone());
            }
            buckets.entry(c.name.clone()).or_default().push(c);
        }
    }
    order
        .into_iter()
        .map(|name| {
            let bucket = buckets.remove(&name).expect("key came from this map");
            merge_nodes(bucket)
        })
        .collect()
}

fn merge_examples(lists: Vec<Vec<crate::node::Example>>) -> Vec<crate::node::Example> {
    let mut seen: Vec<Text> = Vec::new();
    let mut out = Vec::new();
    for list in lists {
        for ex in list {
            if !seen.contains(&ex.command) {
                seen.push(ex.command.clone());
                out.push(ex);
            }
        }
    }
    out
}

/// Pick the value from whichever candidate has the highest `axis` authority
/// among candidates that have `Some`. `None` never displaces `Some`. Ties
/// (equal authority) keep the earliest contributor.
fn pick_option<'a, T, I>(candidates: I, axis: Axis) -> Option<T>
where
    T: Clone + 'a,
    I: IntoIterator<Item = (&'a Provenance, Option<&'a T>)>,
{
    let mut best: Option<(u8, &'a T)> = None;
    for (prov, val) in candidates {
        if let Some(v) = val {
            let auth = prov.effective_authority(axis);
            let replace = match &best {
                None => true,
                Some((best_auth, _)) => auth > *best_auth,
            };
            if replace {
                best = Some((auth, v));
            }
        }
    }
    best.map(|(_, v)| v.clone())
}

/// Same as [`pick_option`] but for `Vec<T>`, treating an empty vec like
/// `None`.
fn pick_vec<'a, T, I>(candidates: I, axis: Axis) -> Vec<T>
where
    T: Clone + 'a,
    I: IntoIterator<Item = (&'a Provenance, &'a Vec<T>)>,
{
    let mut best: Option<(u8, &'a Vec<T>)> = None;
    for (prov, val) in candidates {
        if val.is_empty() {
            continue;
        }
        let auth = prov.effective_authority(axis);
        let replace = match &best {
            None => true,
            Some((best_auth, _)) => auth > *best_auth,
        };
        if replace {
            best = Some((auth, val));
        }
    }
    best.map(|(_, v)| v.clone()).unwrap_or_default()
}

fn best_index<'a, I>(provenances: I, axis: Axis) -> usize
where
    I: IntoIterator<Item = &'a Provenance>,
{
    let mut best_i = 0usize;
    let mut best_auth: Option<u8> = None;
    for (i, prov) in provenances.into_iter().enumerate() {
        let auth = prov.effective_authority(axis);
        let replace = match best_auth {
            None => true,
            Some(b) => auth > b,
        };
        if replace {
            best_auth = Some(auth);
            best_i = i;
        }
    }
    best_i
}

/// Unify flags that arrived as separate short/long rows from the same
/// source, per spec §4.4: sources legitimately emit a flag's short and long
/// forms as distinct items (e.g. `gh __complete pr -` returns `--repo` and
/// `-R` as separate rows with identical descriptions). Within one node's
/// flag list, items whose descriptions match exactly and whose short/long
/// slots are complementary unify into one `Flag`. Must run before merge.
pub fn pair_aliases(flags: Vec<Flag>) -> Vec<Flag> {
    let mut result: Vec<Flag> = Vec::with_capacity(flags.len());
    'outer: for flag in flags {
        if flag.short.is_some() && flag.long.is_some() {
            result.push(flag);
            continue;
        }
        // A flag with neither spelling can't be paired meaningfully.
        if flag.short.is_none() && flag.long.is_none() {
            result.push(flag);
            continue;
        }
        for existing in result.iter_mut() {
            if complementary(existing, &flag) && same_description(existing, &flag) {
                absorb_pair(existing, flag);
                continue 'outer;
            }
        }
        result.push(flag);
    }
    result
}

fn complementary(a: &Flag, b: &Flag) -> bool {
    (a.short.is_some() && a.long.is_none() && b.short.is_none() && b.long.is_some())
        || (a.long.is_some() && a.short.is_none() && b.long.is_none() && b.short.is_some())
}

fn same_description(a: &Flag, b: &Flag) -> bool {
    match (&a.description, &b.description) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

fn absorb_pair(existing: &mut Flag, other: Flag) {
    if existing.short.is_none() {
        existing.short = other.short;
    }
    if existing.long.is_none() {
        existing.long = other.long;
    }
    existing.value_name = existing.value_name.clone().or(other.value_name);
    if matches!(existing.value_kind, crate::node::ValueKind::None) {
        existing.value_kind = other.value_kind;
    }
    existing.repeatable |= other.repeatable;
    existing.required |= other.required;
    existing.hidden &= other.hidden;
    existing.inherited |= other.inherited;
    existing.default = existing.default.clone().or(other.default);
    existing.env_var = existing.env_var.clone().or(other.env_var);
    existing.provenance.absorb(&other.provenance);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provenance::Source;

    fn node_from(source: Source, name: &str) -> CommandNode {
        CommandNode::new(name, Provenance::single(source))
    }

    #[test]
    fn merge_single_candidate_is_identity() {
        let mut n = node_from(Source::HelpText, "git");
        n.summary = Some(Text::sanitize("a vcs"));
        let merged = merge_nodes(vec![n.clone()]).unwrap();
        assert_eq!(merged.name, "git");
        assert_eq!(merged.summary, n.summary);
    }

    #[test]
    fn merge_empty_is_error() {
        assert_eq!(merge_nodes(vec![]), Err(MergeError::Empty));
    }

    #[test]
    fn prose_prefers_known_spec_over_help_text() {
        let mut from_carapace = node_from(
            Source::KnownSpec {
                provider: "carapace".to_string(),
            },
            "git",
        );
        from_carapace.description = Some(Text::sanitize("rich carapace prose"));

        let mut from_help = node_from(Source::HelpText, "git");
        from_help.description = Some(Text::sanitize("terse help text"));

        let merged = merge_nodes(vec![from_help, from_carapace]).unwrap();
        assert_eq!(merged.description.unwrap().as_str(), "rich carapace prose");
    }

    #[test]
    fn structure_prefers_native_dynamic_over_known_spec() {
        let mut from_native = node_from(
            Source::NativeDynamic {
                protocol: "cobra-dunder-complete".to_string(),
            },
            "git",
        );
        from_native.usage = vec![Text::sanitize("git [--version] [--help] <command>")];

        let mut from_carapace = node_from(
            Source::KnownSpec {
                provider: "carapace".to_string(),
            },
            "git",
        );
        from_carapace.usage = vec![Text::sanitize("git [OPTIONS]")];

        let merged = merge_nodes(vec![from_carapace, from_native]).unwrap();
        assert_eq!(
            merged.usage[0].as_str(),
            "git [--version] [--help] <command>"
        );
    }

    #[test]
    fn none_never_displaces_some() {
        let from_native = node_from(
            Source::NativeDynamic {
                protocol: "cobra-dunder-complete".to_string(),
            },
            "git",
        );
        // from_native has no description (native tiers are prose-poor).
        let mut from_help = node_from(Source::HelpText, "git");
        from_help.description = Some(Text::sanitize("some description"));

        let merged = merge_nodes(vec![from_native, from_help]).unwrap();
        assert_eq!(merged.description.unwrap().as_str(), "some description");
    }

    #[test]
    fn ties_break_toward_earlier_contributor() {
        let mut a = node_from(Source::HelpText, "git");
        a.summary = Some(Text::sanitize("from a"));
        let mut b = node_from(Source::HelpText, "git");
        b.summary = Some(Text::sanitize("from b"));
        let merged = merge_nodes(vec![a, b]).unwrap();
        assert_eq!(merged.summary.unwrap().as_str(), "from a");
    }

    #[test]
    fn children_filled_is_logical_or() {
        let mut a = node_from(Source::HelpText, "git");
        a.children_filled = false;
        let mut b = node_from(
            Source::KnownSpec {
                provider: "carapace".to_string(),
            },
            "git",
        );
        b.children_filled = true;
        let merged = merge_nodes(vec![a, b]).unwrap();
        assert!(merged.children_filled);
    }

    #[test]
    fn subcommands_merge_recursively_by_name() {
        let mut a = node_from(Source::HelpText, "git");
        let mut a_rebase = node_from(Source::HelpText, "rebase");
        a_rebase.summary = Some(Text::sanitize("terse"));
        a.subcommands.push(a_rebase);

        let mut b = node_from(
            Source::KnownSpec {
                provider: "carapace".to_string(),
            },
            "git",
        );
        let mut b_rebase = node_from(
            Source::KnownSpec {
                provider: "carapace".to_string(),
            },
            "rebase",
        );
        b_rebase.description = Some(Text::sanitize("rich"));
        b.subcommands.push(b_rebase);

        let merged = merge_nodes(vec![a, b]).unwrap();
        assert_eq!(merged.subcommands.len(), 1);
        let rebase = &merged.subcommands[0];
        assert_eq!(rebase.summary.as_ref().unwrap().as_str(), "terse");
        assert_eq!(rebase.description.as_ref().unwrap().as_str(), "rich");
    }

    #[test]
    fn provenance_aggregates_all_contributors() {
        let a = node_from(Source::HelpText, "git");
        let b = node_from(
            Source::KnownSpec {
                provider: "carapace".to_string(),
            },
            "git",
        );
        let merged = merge_nodes(vec![a, b]).unwrap();
        assert_eq!(merged.provenance.sources.len(), 2);
    }

    // --- alias pairing ---

    fn paired_flag(short: Option<char>, long: Option<&str>, desc: &str) -> Flag {
        let mut f = Flag::long(
            long.unwrap_or_default(),
            Provenance::single(Source::NativeDynamic {
                protocol: "cobra-dunder-complete".to_string(),
            }),
        );
        f.long = long.map(|s| s.to_string());
        f.short = short;
        f.description = Some(Text::sanitize(desc));
        f
    }

    #[test]
    fn pairs_short_and_long_with_identical_description() {
        let flags = vec![
            paired_flag(None, Some("repo"), "Select another repository"),
            paired_flag(Some('R'), None, "Select another repository"),
        ];
        let paired = pair_aliases(flags);
        assert_eq!(paired.len(), 1);
        assert_eq!(paired[0].short, Some('R'));
        assert_eq!(paired[0].long.as_deref(), Some("repo"));
    }

    #[test]
    fn does_not_pair_different_descriptions() {
        let flags = vec![
            paired_flag(None, Some("repo"), "Select another repository"),
            paired_flag(Some('R'), None, "Something totally different"),
        ];
        let paired = pair_aliases(flags);
        assert_eq!(paired.len(), 2);
    }

    #[test]
    fn does_not_pair_two_long_only_flags() {
        let flags = vec![
            paired_flag(None, Some("repo"), "same"),
            paired_flag(None, Some("remote"), "same"),
        ];
        let paired = pair_aliases(flags);
        assert_eq!(paired.len(), 2);
    }

    #[test]
    fn pairing_is_idempotent() {
        let flags = vec![
            paired_flag(None, Some("repo"), "Select another repository"),
            paired_flag(Some('R'), None, "Select another repository"),
        ];
        let once = pair_aliases(flags);
        let twice = pair_aliases(once.clone());
        assert_eq!(once, twice);
    }

    #[test]
    fn merge_unifies_flags_by_identity_across_sources() {
        let mut a = node_from(Source::HelpText, "git");
        let mut fa = Flag::long("interactive", Provenance::single(Source::HelpText));
        fa.short = Some('i');
        fa.description = Some(Text::sanitize("terse"));
        a.flags.push(fa);

        let mut b = node_from(
            Source::KnownSpec {
                provider: "carapace".to_string(),
            },
            "git",
        );
        let mut fb = Flag::long(
            "interactive",
            Provenance::single(Source::KnownSpec {
                provider: "carapace".to_string(),
            }),
        );
        fb.short = Some('i');
        fb.description = Some(Text::sanitize("rich"));
        b.flags.push(fb);

        let merged = merge_nodes(vec![a, b]).unwrap();
        assert_eq!(merged.flags.len(), 1);
        assert_eq!(
            merged.flags[0].description.as_ref().unwrap().as_str(),
            "rich"
        );
        assert_eq!(merged.flags[0].short, Some('i'));
    }
}
