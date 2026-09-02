//! K1/K2/K3 pre-tag signature detectors: heuristics that pre-populate an
//! entry's `k1`/`k2`/`k3` suggestion (see [`super::entry_from_classified`])
//! from the extracted tree and, for K2, the [`crate::existence`] report.

use crate::existence::{self, FabricationKind};
use mandible_core::{CommandNode, Entity};

// ---------------------------------------------------------------------
// K1/K2 pre-tagging (see [`Entry::k1`]/[`Entry::k2`]'s doc comments for the
// full rationale). Each `_signature` function derives a suggested tag from
// structural properties of the extracted tree/raw text alone — no tool name
// is ever consulted, so these stay within AGENTS.md §1's "no per-tool
// logic" invariant exactly like every other detector in this workspace.
// ---------------------------------------------------------------------

/// True for a flag matching the GCC-family single-dash-long-option defect
/// signature: the short-flag grammar took one character as `short` and
/// glued the rest of a multi-character single-dash spelling onto
/// `value_name` (`-fdump-scos` -> `short=Some('f')`, `long=None`,
/// `value_name=Some("dump-scos")`). See [`Entry::k1`].
pub(super) fn is_k1_flag(flag: &Entity) -> bool {
    flag.short().is_some() && flag.long().is_none() && flag.value_name.is_some()
}

/// `(matching, total)` flag counts across `node` and every descendant, for
/// the K1 pre-tag's display line (e.g. "839/1454 flags match").
pub(super) fn k1_signature_stats(node: &CommandNode) -> (usize, usize) {
    let mut matching = node.flags().filter(|f| is_k1_flag(f)).count();
    let mut total = node.flags().count();
    for child in &node.subcommands {
        let (m, t) = k1_signature_stats(child);
        matching += m;
        total += t;
    }
    (matching, total)
}

/// The K1 pre-tag suggestion: `Some(true)` when `root`'s tree contains at
/// least one [`is_k1_flag`] match anywhere, `None` when it contains none
/// (nothing to flag — never `Some(false)`, since there is no "confirmed not
/// K1" state worth asserting for a tool that never exhibited the shape in
/// the first place).
pub(super) fn k1_signature(root: &CommandNode) -> Option<bool> {
    let (matching, _) = k1_signature_stats(root);
    if matching > 0 {
        Some(true)
    } else {
        None
    }
}

/// True when `name` occurs as *some* whitespace-delimited token anywhere in
/// `raw` — not restricted to a line's first token the way
/// `existence::line_start_words` is. Punctuation immediately touching the
/// token (as in a comma-separated list) is trimmed the same way the K2
/// false-positive class actually presents. Used only to *explain* an
/// existing existence-detector fabrication, never to suppress one directly
/// — see [`k2_signature_stats`].
pub(super) fn token_occurs_anywhere(raw: &str, name: &str) -> bool {
    raw.split_whitespace().any(|tok| {
        tok.trim_matches(|c: char| !(c.is_alphanumeric() || c == '-' || c == '_')) == name
    })
}

/// `(attributable, total)` counts of `report`'s subcommand-kind
/// fabrications plausibly explained by the existence detector's own
/// multi-column/comma-separated tokenization gap (K2) rather than genuine
/// parser fabrication. The gap itself is closed (`existence::list_row_words`
/// reads whole list rows), so this normally returns `(0, 0)`; kept as a
/// regression signal.
///
/// Attributable: name occurs as some token anywhere in the raw text
/// ([`token_occurs_anywhere`]), not at the line-start position the
/// detector requires. Flag-kind fabrications are out of scope —
/// [`existence::spelling_occurs`] already scans unconditionally.
pub(super) fn k2_signature_stats(report: &existence::ExistenceReport, raw: &str) -> (usize, usize) {
    let subcommand_names: Vec<&str> = report
        .fabrications
        .iter()
        .filter(|f| f.kind == FabricationKind::Subcommand)
        .map(|f| f.name.as_str())
        .collect();
    let total = subcommand_names.len();
    let attributable = subcommand_names
        .iter()
        .filter(|name| token_occurs_anywhere(raw, name))
        .count();
    (attributable, total)
}

/// The K2 pre-tag suggestion: `Some(true)` when *every* subcommand-kind
/// existence fabrication for this tool is attributable to the detector's
/// own tokenizer gap (near-certainly detector noise, not a parser defect),
/// `Some(false)` when at least one is not (worth a real look — could be a
/// genuine [M-10]-shaped fabrication), `None` when the tool has no
/// subcommand-kind fabrications to judge at all.
pub(super) fn k2_signature(report: &existence::ExistenceReport, raw: &str) -> Option<bool> {
    let (attributable, total) = k2_signature_stats(report, raw);
    if total == 0 {
        None
    } else {
        Some(attributable == total)
    }
}

/// True for a node carrying nothing at all: no flags, no subcommands, and
/// no summary — the same `empty` predicate `status::structure_sanity`'s own
/// `count_suspicious` uses, reused here rather than redefined so the two
/// "is this node genuinely empty" checks in this codebase can never drift
/// apart.
pub(super) fn is_bare_stub(node: &CommandNode) -> bool {
    node.flags().next().is_none() && node.subcommands.is_empty() && node.summary.is_none()
}

/// True for a bare stub ([`is_bare_stub`]) that is also not
/// [`CommandNode::heading_attested`] — its name came from a native/cobra
/// artifact rather than a recognized `--help` heading or headingless
/// invocation table (spec §7 Tier B). Provable from the single extraction
/// pass: `help_text::raw_help` refuses to probe any node whose
/// `heading_attested` bit is false, so unlike an ordinary un-recursed
/// subcommand, this one structurally cannot ever be probed.
///
/// A headingless-table node still counts here even though it's
/// existence-attested (`invocation_attested`) — it exempts a node from
/// being counted as *fabricated* ([`crate::status::structure_sanity`]) but
/// does not make it any less permanently un-probed.
///
/// Fixture: `corpus/git-lfs/*/help.txt`.
pub(super) fn is_attestation_gated_stub(node: &CommandNode) -> bool {
    is_bare_stub(node) && !node.heading_attested
}

/// Count of [`is_attestation_gated_stub`] matches across `node` and every
/// descendant — called only on `root`'s subcommands, never on `root`
/// itself, matching `status::structure_sanity`'s own root-exclusion
/// (`root` is the literal executable name resolved from `PATH`, never
/// something a tier guessed at, so it needs no heading to attest to).
pub(super) fn count_attestation_gated_stubs(node: &CommandNode) -> usize {
    let this = usize::from(is_attestation_gated_stub(node));
    this + node
        .subcommands
        .iter()
        .map(count_attestation_gated_stubs)
        .sum::<usize>()
}

/// Total flag count across `node` and every descendant.
pub(super) fn total_flags(node: &CommandNode) -> usize {
    node.flags().count() + node.subcommands.iter().map(total_flags).sum::<usize>()
}

/// True when `root` has at least one subcommand yet the whole tree carries
/// zero flags anywhere. `openssl`'s shape: root `--help` is a bare command
/// grid with no options section, so the single root-only extraction pass
/// ([`Runner::extract_full`], never recurses) surfaces no flag at all.
/// Most tools document at least `-h`/`--version`, which is what keeps this
/// from over-tagging every multi-level tool's ordinary lazy-fill state.
///
/// Fixture: `corpus/openssl/*/help.txt`.
pub(super) fn has_unfetched_subcommand_help(root: &CommandNode) -> bool {
    !root.subcommands.is_empty() && total_flags(root) == 0
}

/// The K3 pre-tag suggestion (see [`mandible_core::audit::Entry::k3`]):
/// `Some(true)` when `root`'s single-pass snapshot shows either known
/// cause — an attestation-gated stub anywhere in the tree, or the
/// whole-tree-zero-flags shape — `None` otherwise. Same "no `Some(false)`"
/// convention as [`k1_signature`]: there is nothing to assert-not for a
/// tool that shows neither shape.
pub(super) fn k3_signature(root: &CommandNode) -> Option<bool> {
    let gated_stubs: usize = root
        .subcommands
        .iter()
        .map(count_attestation_gated_stubs)
        .sum();
    if gated_stubs > 0 || has_unfetched_subcommand_help(root) {
        Some(true)
    } else {
        None
    }
}
