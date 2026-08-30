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

use crate::entity::{Dashes, Entity, EntityKind, Spelling};
use crate::node::CommandNode;
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
        let flags = only.take_entities_of(EntityKind::Flag);
        only.set_flags(pair_aliases(flags));
        return Ok(only);
    }

    // Alias-pair each candidate's own flags before it participates in
    // cross-source merge (spec §4.4: "Pairing runs before merge").
    for c in &mut candidates {
        let flags = c.take_entities_of(EntityKind::Flag);
        c.set_flags(pair_aliases(flags));
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
    // `unparsed` (spec §7 Tier B step 3 / batch 6 part 4) is honesty
    // metadata about *how little* a source understood, not structure to
    // merge piecewise — picking the highest-structural-authority
    // non-empty contributor (like `usage`) means a candidate that actually
    // parsed real structure always wins over one that gave up, since a
    // node only ever carries `unparsed` when it has nothing else.
    let unparsed = pick_vec(
        candidates.iter().map(|c| (&c.provenance, &c.unparsed)),
        Axis::Structural,
    );
    let detected_framework = pick_option(
        candidates
            .iter()
            .map(|c| (&c.provenance, c.detected_framework.as_ref())),
        Axis::Structural,
    );
    // Same authority reasoning as `detected_framework` immediately above:
    // this is a fact about how a contributor's *own* text was obtained
    // (spec §6 rule 2b), never something to merge piecewise across
    // sources. Only `HelpTextTier` ever sets this, so in practice there is
    // rarely more than one non-`None` candidate to pick between at all.
    let confession = pick_option(
        candidates
            .iter()
            .map(|c| (&c.provenance, c.confession.as_ref())),
        Axis::Structural,
    );

    let structural_winner_idx =
        best_index(candidates.iter().map(|c| &c.provenance), Axis::Structural);
    let hidden = candidates[structural_winner_idx].hidden;
    let children_filled = candidates.iter().any(|c| c.children_filled);
    // Same "any contributor is enough" reasoning as `children_filled`:
    // this is positive evidence the node names a real command (spec
    // §13.1's structure-sanity check), and a merge can only ever add
    // evidence, never take it away — if one source recovered this node
    // from a recognized command heading, that fact doesn't stop being
    // true just because another, lower-authority source also contributed
    // a field.
    let heading_attested = candidates.iter().any(|c| c.heading_attested);
    // Same "any contributor is enough" reasoning, for the second attestation
    // bit (spec §6): a headingless-invocation-table source's existence
    // evidence doesn't stop being true because another, lower-authority
    // source also contributed a field.
    let invocation_attested = candidates.iter().any(|c| c.invocation_attested);

    let mut provenance = Provenance::default();
    for c in &candidates {
        provenance.absorb(&c.provenance);
    }

    let entities = merge_entity_lists(candidates.iter().map(|c| c.entities.clone()).collect());
    let subcommands =
        merge_subcommand_lists(candidates.iter().map(|c| c.subcommands.clone()).collect())?;
    let examples = merge_examples(candidates.iter().map(|c| c.examples.clone()).collect());

    Ok(CommandNode {
        name,
        aliases,
        summary,
        description,
        usage,
        entities,
        subcommands,
        examples,
        hidden,
        deprecated,
        children_filled,
        group,
        unparsed,
        detected_framework,
        provenance,
        heading_attested,
        invocation_attested,
        confession,
    })
}

/// Merge entities by identity across several candidate lists (already
/// alias-paired, for flags), applying the two-axis authority resolution per
/// field.
///
/// Identity is [`entity_identity`]'s: the kind, then the long name, else the
/// short letter, else the bare name a dashless kind carries, else the
/// description. Two entities of different kinds never share a bucket, so a
/// positional called `verbose` and a `--verbose` flag stay two items.
/// Relative order within each kind is the order of first appearance, which
/// is what the snapshot's per-kind sections are written in.
pub fn merge_entity_lists(lists: Vec<Vec<Entity>>) -> Vec<Entity> {
    let mut order: Vec<(EntityKind, String)> = Vec::new();
    let mut buckets: HashMap<(EntityKind, String), Vec<Entity>> = HashMap::new();
    for list in lists {
        for entity in list {
            let key = entity_identity(&entity);
            if !buckets.contains_key(&key) {
                order.push(key.clone());
            }
            buckets.entry(key).or_default().push(entity);
        }
    }
    order
        .into_iter()
        .map(|key| {
            let bucket = buckets.remove(&key).expect("key came from this map");
            merge_entity_bucket(bucket)
        })
        .collect()
}

/// The identity two entities must share to be the same item.
///
/// The kind leads, because a flag and a positional that happen to be
/// spelled alike are unrelated items, not two sources' accounts of one.
/// After that: the long name, else the short letter — the pre-0.5.0
/// `Flag`'s own preference order — else, for a kind spelled without dashes
/// (a positional's `pathspec`, a modifier letter, a variable name), that
/// bare name, which is what the pre-0.5.0 `Positional` merged on. The
/// description is the last resort for a flag with no spelling at all.
fn entity_identity(e: &Entity) -> (EntityKind, String) {
    let key = match (e.long(), e.short()) {
        (Some(l), _) => format!("L:{l}"),
        (None, Some(s)) => format!("S:{s}"),
        (None, None) if !e.spellings.is_empty() => format!("N:{}", e.primary_name()),
        (None, None) => format!(
            "D:{}",
            e.description.as_ref().map(|d| d.as_str()).unwrap_or("")
        ),
    };
    (e.kind, key)
}

fn merge_entity_bucket(mut bucket: Vec<Entity>) -> Entity {
    if bucket.len() == 1 {
        return bucket.pop().expect("len checked");
    }

    // The spelling halves are resolved **independently**, exactly as they
    // were when they were four separate `Flag` fields, and only then
    // reassembled into a `spellings` vec. Picking a whole `Spelling` by
    // authority instead would silently couple them: a high-authority
    // source that omits the `[no-]` a lower-authority one documented would
    // start erasing the negatability, which no field-level rule here has
    // ever done.
    let short = bucket.iter().find_map(|f| f.short());
    let long_name = pick_option(
        bucket
            .iter()
            .map(|f| (&f.provenance, f.long_spelling().map(|s| &s.name))),
        Axis::Structural,
    );
    // One dash or two, and negatability, are facts about how the tool
    // spells this option: a single source that saw it is enough, because
    // no other source can have seen the same flag spelled the other way.
    let negatable = bucket.iter().any(|f| f.negatable());
    let single_dash = bucket.iter().any(|f| f.single_dash());
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
    let see_also = pick_vec(
        bucket.iter().map(|f| (&f.provenance, &f.see_also)),
        Axis::Prose,
    );

    let mut provenance = Provenance::default();
    for f in &bucket {
        provenance.absorb(&f.provenance);
    }

    // Short first, then long — the order the display spelling reads in
    // (`-i, --interactive`), and the order the snapshot's `short`/`long`
    // keys are recovered from.
    let mut spellings = Vec::new();
    if let Some(c) = short {
        spellings.push(Spelling::short(c));
    }
    if let Some(name) = long_name {
        spellings.push(Spelling {
            name,
            dashes: if single_dash {
                Dashes::Single
            } else {
                Dashes::Double
            },
            negatable,
        });
    }
    // A dashless spelling — a positional's name, a modifier letter, a
    // variable name — has no short/long halves to resolve independently,
    // and it *is* the bucket's identity, so every entity here carries the
    // same one. Take it verbatim rather than reconstructing it.
    if spellings.is_empty() {
        if let Some(bare) = bucket.iter().find_map(|e| e.spellings.first()) {
            spellings.push(bare.clone());
        }
    }

    // Identity is spelling-based, so every entity in a bucket is the same
    // kind; take it from the first rather than assuming `Flag`, so the
    // later positional/modifier/env-var stages inherit this unchanged.
    let mut merged = Entity::new(bucket[0].kind, provenance);
    merged.spellings = spellings;
    merged.value_name = value_name;
    merged.value_kind = value_kind;
    merged.choices = choices;
    merged.repeatable = repeatable;
    merged.required = required;
    merged.hidden = hidden;
    merged.deprecated = deprecated;
    merged.inherited = inherited;
    merged.group = group;
    merged.description = description;
    merged.default = default;
    merged.see_also = see_also;
    merged.env_var = env_var;
    merged
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
pub fn pair_aliases(flags: Vec<Entity>) -> Vec<Entity> {
    let mut result: Vec<Entity> = Vec::with_capacity(flags.len());
    'outer: for flag in flags {
        if flag.short().is_some() && flag.long().is_some() {
            result.push(flag);
            continue;
        }
        // A flag with neither spelling can't be paired meaningfully.
        if flag.short().is_none() && flag.long().is_none() {
            result.push(flag);
            continue;
        }
        for existing in result.iter_mut() {
            if complementary(existing, &flag)
                && same_description(existing, &flag)
                && same_value_shape(existing, &flag)
            {
                absorb_pair(existing, flag);
                continue 'outer;
            }
        }
        result.push(flag);
    }
    result
}

/// Whether `a` and `b` fill each other's empty spelling slot — one
/// short-only, one long-only.
///
/// **A single-dash long option is never the long half of such a pair.** The
/// alias convention this function exists for is one dash and two
/// (`-R, --repo`); a flag whose own spelling is already a single dash
/// (`-help`, `-CC`, `-vv`; see [`Entity::single_dash`]) has no short
/// alias to be paired with, and offering it one is how `gcc`-family tools
/// lose flags: `lto-dump` gives hundreds of its options the description
/// `[disabled]`, so [`same_description`] matches almost anything and the
/// real `-CC` was absorbed into an unrelated `-Wspeculative`, claiming a
/// spelling that tool does not have. Measured on a full `PATH` sweep: two
/// tools, four flags, and the only losses in the sweep-diff that found it.
fn complementary(a: &Entity, b: &Entity) -> bool {
    if a.single_dash() || b.single_dash() {
        return false;
    }
    (a.short().is_some() && a.long().is_none() && b.short().is_none() && b.long().is_some())
        || (a.long().is_some() && a.short().is_none() && b.long().is_none() && b.short().is_some())
}

/// Whether `a` and `b` agree about *taking a value at all* — the second
/// half of "these two rows are one flag", alongside [`same_description`].
///
/// A source that emits one flag as two rows emits the same value spec on
/// both; two rows that disagree about it are two different options that
/// happen to share a summary line. `ld` and `gold` are the specimen, and
/// they are the same failure shape as the `lto-dump` incident
/// [`complementary`] documents — a description common enough to collide:
///
/// ```text
///   --allow-multiple-definition Allow multiple definitions of symbols
///   -z muldefs                  Allow multiple definitions of symbols
/// ```
///
/// Two real, separately spelled options with one shared sentence. Pairing
/// them destroys one of the two and gives the survivor a value
/// (`--allow-multiple-definition muldefs`) that neither row documents.
/// Measured on a full `PATH` sweep: 7 tools, 1 flag each, and the only
/// losses in the sweep that found it.
///
/// Deliberately coarse — [`ValueKind`] only, never the placeholder's
/// spelling. A source may legitimately name the metavar on one row and not
/// the other (`-R` beside `--repo <string>`), and requiring those to match
/// would un-pair the aliases this function exists to let through.
fn same_value_shape(a: &Entity, b: &Entity) -> bool {
    a.value_kind == b.value_kind
}

fn same_description(a: &Entity, b: &Entity) -> bool {
    match (&a.description, &b.description) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

fn absorb_pair(existing: &mut Entity, other: Entity) {
    // Splice `other`'s spelling into the position it belongs in — a short
    // at the front, a long at the back — rather than appending it or
    // rebuilding the list from two slots.
    //
    // Appending is wrong because [`complementary`] admits the pair in
    // either arrival order: `--repo` meeting `-R` must still come out
    // spelled `-R, --repo`, never `--repo, -R`, so the rendered order
    // cannot depend on which row the source printed first.
    //
    // Rebuilding from a short/long pair is wrong for a subtler reason: it
    // silently drops any *further* spelling `existing` carries. Nothing
    // emits a multi-spelling entity yet, but the whole point of the entity
    // schema is that something soon will (ffplay's `-h, -?, -help,
    // --help`), and a lossy merge that only misbehaves once that lands is
    // exactly the kind of defect that gets blamed on the later change.
    if existing.short_spelling().is_none() {
        if let Some(s) = other.short_spelling() {
            existing.spellings.insert(0, s.clone());
        }
    }
    if existing.long_spelling().is_none() {
        if let Some(l) = other.long_spelling() {
            existing.spellings.push(l.clone());
        }
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
    use crate::node::ValueKind;
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

    /// A flag entity spelled short, long, or both — the short spelling
    /// always first, which is the order `pair_aliases` must also produce.
    fn paired_flag(short: Option<char>, long: Option<&str>, desc: &str) -> Entity {
        let mut f = Entity::new(
            crate::entity::EntityKind::Flag,
            Provenance::single(Source::NativeDynamic {
                protocol: "cobra-dunder-complete".to_string(),
            }),
        );
        if let Some(c) = short {
            f.spellings.push(Spelling::short(c));
        }
        if let Some(l) = long {
            f.spellings.push(Spelling::long(l));
        }
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
        assert_eq!(paired[0].short(), Some('R'));
        assert_eq!(paired[0].long(), Some("repo"));
    }

    /// Two rows that disagree about taking a value are two different
    /// options that happen to share a summary line, not one flag emitted
    /// twice. `ld`/`gold` write exactly that:
    ///
    /// ```text
    ///   --allow-multiple-definition Allow multiple definitions of symbols
    ///   -z muldefs                  Allow multiple definitions of symbols
    /// ```
    ///
    /// Paired, one of the two is destroyed and the survivor claims
    /// `--allow-multiple-definition muldefs`, a value neither row
    /// documents. Found by `sweep-diff` on a full `PATH` sweep — 7 tools,
    /// 1 flag each, and the only losses in it.
    #[test]
    fn does_not_pair_two_rows_that_disagree_about_taking_a_value() {
        let long = paired_flag(
            None,
            Some("allow-multiple-definition"),
            "Allow multiple definitions of symbols",
        );
        let mut short = paired_flag(Some('z'), None, "Allow multiple definitions of symbols");
        short.value_name = Some("muldefs".to_string());
        short.value_kind = ValueKind::Required;
        let paired = pair_aliases(vec![long, short]);
        assert_eq!(
            paired.len(),
            2,
            "both options must survive: {:?}",
            paired.iter().map(|f| f.spelling()).collect::<Vec<_>>()
        );
        assert!(
            paired
                .iter()
                .any(|f| f.long() == Some("allow-multiple-definition")
                    && f.short().is_none()
                    && f.value_kind == ValueKind::None),
            "the long form takes no value: {paired:?}"
        );
    }

    /// A single-dash long option has no short alias to be paired with —
    /// its own spelling is already the single-dash one. Left pairable, the
    /// `gcc` family loses flags outright: `lto-dump` gives hundreds of its
    /// options the description `[disabled]`, so `same_description` matches
    /// almost anything and the real `-CC` was absorbed into an unrelated
    /// `-Wspeculative`, which then claimed a spelling that tool does not
    /// have. Found by `sweep-diff` on a full `PATH` sweep — two tools, four
    /// flags, and the only losses in it.
    #[test]
    fn does_not_pair_a_single_dash_long_option_with_any_short_flag() {
        let mut cc = paired_flag(None, Some("CC"), "[disabled]");
        cc.spellings = vec![Spelling::single_dash("CC")];
        let flags = vec![paired_flag(Some('W'), None, "[disabled]"), cc];
        let paired = pair_aliases(flags);
        assert_eq!(
            paired.len(),
            2,
            "-CC must stay its own flag: {:?}",
            paired.iter().map(|f| f.spelling()).collect::<Vec<_>>()
        );
        // ...and the identical pair *does* unify when the long half is a
        // real `--` option, confirming `single_dash` is what rejected it.
        let flags = vec![
            paired_flag(Some('W'), None, "[disabled]"),
            paired_flag(None, Some("CC"), "[disabled]"),
        ];
        assert_eq!(pair_aliases(flags).len(), 1);
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

    /// Pairing splices the incoming spelling into place and keeps every
    /// spelling the surviving entity already had.
    ///
    /// The `-h, -?` half is the case a two-slot rebuild loses: `short()`
    /// reports only the first short, so reconstructing the entity from
    /// `short()` + `long()` would silently drop `-?`. Nothing emits a
    /// multi-spelling entity yet, which is exactly why this is pinned now
    /// — the loss would otherwise first appear as a bug in whichever later
    /// change starts emitting them.
    #[test]
    fn pairing_keeps_every_spelling_the_survivor_already_had() {
        let mut multi = paired_flag(Some('h'), None, "show help");
        multi.spellings.push(Spelling::short('?'));
        let long = paired_flag(None, Some("help"), "show help");

        let paired = pair_aliases(vec![multi, long]);
        assert_eq!(paired.len(), 1, "the pair must unify");
        assert_eq!(paired[0].spelling(), "-h, -?, --help");
    }

    /// The long half arriving *first* must still render short-first: which
    /// row the source printed first is not a fact about the flag.
    #[test]
    fn pairing_renders_short_first_whichever_row_arrived_first() {
        let long_first = vec![
            paired_flag(None, Some("repo"), "select a repository"),
            paired_flag(Some('R'), None, "select a repository"),
        ];
        let short_first = vec![
            paired_flag(Some('R'), None, "select a repository"),
            paired_flag(None, Some("repo"), "select a repository"),
        ];
        assert_eq!(pair_aliases(long_first)[0].spelling(), "-R, --repo");
        assert_eq!(pair_aliases(short_first)[0].spelling(), "-R, --repo");
    }

    #[test]
    fn merge_unifies_flags_by_identity_across_sources() {
        let mut a = node_from(Source::HelpText, "git");
        let mut fa = Entity::flag_long("interactive", Provenance::single(Source::HelpText));
        fa.spellings.insert(0, Spelling::short('i'));
        fa.description = Some(Text::sanitize("terse"));
        a.entities.push(fa);

        let mut b = node_from(
            Source::KnownSpec {
                provider: "carapace".to_string(),
            },
            "git",
        );
        let mut fb = Entity::flag_long(
            "interactive",
            Provenance::single(Source::KnownSpec {
                provider: "carapace".to_string(),
            }),
        );
        fb.spellings.insert(0, Spelling::short('i'));
        fb.description = Some(Text::sanitize("rich"));
        b.entities.push(fb);

        let merged = merge_nodes(vec![a, b]).unwrap();
        let flags: Vec<&Entity> = merged.flags().collect();
        assert_eq!(flags.len(), 1);
        assert_eq!(flags[0].description.as_ref().unwrap().as_str(), "rich");
        assert_eq!(flags[0].short(), Some('i'));
    }

    /// A positional merges on its name across sources, taking `required`
    /// and `repeatable` from any source that saw them and its description
    /// from the highest prose authority — the rule the pre-0.5.0
    /// `merge_positional_lists` applied, now one arm of the entity merge.
    #[test]
    fn merge_unifies_positionals_by_name_across_sources() {
        let mut a = node_from(Source::HelpText, "git");
        let mut pa = Entity::positional("pathspec", Provenance::single(Source::HelpText));
        pa.repeatable = true;
        pa.description = Some(Text::sanitize("terse"));
        a.entities.push(pa);

        let spec = Source::KnownSpec {
            provider: "carapace".to_string(),
        };
        let mut b = node_from(spec.clone(), "git");
        let mut pb = Entity::positional("pathspec", Provenance::single(spec));
        pb.required = true;
        pb.description = Some(Text::sanitize("rich"));
        b.entities.push(pb);

        let merged = merge_nodes(vec![a, b]).unwrap();
        let positionals: Vec<&Entity> = merged.positionals().collect();
        assert_eq!(positionals.len(), 1);
        assert_eq!(positionals[0].primary_name(), "pathspec");
        assert!(positionals[0].required, "required survives from one source");
        assert!(
            positionals[0].repeatable,
            "variadic survives from the other source"
        );
        assert_eq!(
            positionals[0].description.as_ref().unwrap().as_str(),
            "rich"
        );
    }

    /// Kind leads the merge identity, and the case that needs it is the
    /// description fallback: an entity with no spelling at all is
    /// identified by its description text, which two *different kinds* can
    /// share. Without the kind in the key these two collide, and the
    /// survivor takes its kind from whichever arrived first — a positional
    /// silently rendered as a flag, or the reverse.
    #[test]
    fn entities_of_different_kinds_never_share_a_merge_bucket() {
        let mut a = node_from(Source::HelpText, "tool");
        let shared = Text::sanitize("the thing to operate on");

        let mut spelling_less_flag =
            Entity::new(EntityKind::Flag, Provenance::single(Source::HelpText));
        spelling_less_flag.description = Some(shared.clone());
        a.entities.push(spelling_less_flag);

        let mut spelling_less_positional =
            Entity::new(EntityKind::Positional, Provenance::single(Source::HelpText));
        spelling_less_positional.description = Some(shared);
        a.entities.push(spelling_less_positional);

        let merged = merge_nodes(vec![a.clone(), a]).unwrap();
        assert_eq!(merged.flags().count(), 1, "the flag survives as a flag");
        assert_eq!(
            merged.positionals().count(),
            1,
            "the positional is not absorbed into the flag's bucket"
        );
    }
}
