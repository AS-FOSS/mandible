//! Shared parse-status derivation (spec §13.1), used identically by the
//! coverage harness (`coverage.rs`) and the corpus regression runner
//! (`corpus.rs`).
//!
//! This used to be computed independently in `coverage::score_one` and (in
//! an earlier draft of the corpus runner) a second time here, and the work
//! order for the corpus runner called that out by name: "two independent
//! definitions of 'status' will drift, and the drift will be discovered at
//! the worst possible time." So there is exactly one function that turns an
//! [`ExtractionResult`] into a status label, [`compute`], and both callers
//! use it.

use mandible_core::{is_command_name_shaped, CommandNode};
use mandible_extract::ExtractionResult;

/// A tool/fixture's derived status plus the raw numbers that produced it,
/// so a caller (the coverage scoreboard's `Row`, or the corpus runner's
/// per-fixture report) can render its own detail without recomputing
/// anything.
#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    /// Descendant nodes failing spec §13.1's structure-sanity check (a
    /// bad-shaped name, or emptiness with no `heading_attested` evidence).
    /// Non-zero forces `label` to `"suspicious"` regardless of everything
    /// else — see [`structure_sanity`]'s doc comment on why emptiness
    /// alone isn't enough and why this is checked ahead of `%described`.
    pub suspicious_nodes: usize,
    /// True when the root degraded to spec §7 Tier B step 3's verbatim
    /// rendering (`CommandNode::unparsed` non-empty).
    pub verbatim: bool,
    /// Percentage (0-100) of *describable* flags in the merged tree with a
    /// description (spec §13's metric design rules: a usage-synopsis-only
    /// flag is excluded from both numerator and denominator, since its
    /// source could never have supplied a description — see
    /// [`mandible_extract::ExtractionResult::flag_description_ratio`]).
    /// `None` when the tree has no describable flags at all to compute a
    /// ratio over — which includes the case where it has flags, just none
    /// of them describable (a root whose only flags came from its usage
    /// synopsis, e.g. `git` before [M-16]'s `-h` fallback recovered
    /// `restore`'s described ones).
    pub pct_flags_with_text: Option<f64>,
    /// One of `"no-tier"`, `"verbatim"`, `"suspicious"`, `"low-confidence"`,
    /// `"incomplete"`, `"ok"` — see [`compute`] for the derivation order.
    pub label: &'static str,
}

/// Derive [`Status`] from a completed extraction, exactly as
/// `coverage::score_one` did before this module existed (moved here
/// unchanged, not reimplemented — see this module's doc comment).
pub fn compute(result: &ExtractionResult) -> Status {
    let suspicious_nodes = result.root.as_ref().map(structure_sanity).unwrap_or(0);
    let verbatim = result.root.as_ref().is_some_and(|r| !r.unparsed.is_empty());

    // The ratio's denominator is `describable_flag_count`, not
    // `flag_count` (spec §13's metric design rules) — a tool can have
    // flags (`flag_count() > 0`, the raw column callers like
    // `coverage::score_one` keep visible separately) that are entirely
    // usage-synopsis spellings (`describable_flag_count() == 0`), and that
    // tool has no ratio to report either. See
    // `ExtractionResult::describable_flag_count`'s doc comment.
    let describable = result.describable_flag_count();
    let pct_flags_with_text = if describable == 0 {
        None
    } else {
        Some(result.flag_description_ratio() * 100.0)
    };

    let label = if result.root.is_none() {
        "no-tier"
    } else if verbatim {
        // Spec §7 Tier B step 3: this tool produced no structure at all
        // and is showing the author's own raw text instead — distinct
        // from `suspicious` (fabricated structure) and from `no-tier`
        // (nothing at all, not even raw text). A verbatim root always has
        // zero subcommands by construction, so it can never also trip the
        // structure-sanity check below; checked first anyway so that
        // invariant is documented rather than relied on as an accident.
        "verbatim"
    } else if suspicious_nodes > 0 {
        // Checked before %described on purpose: [M-10] reported `tar` as
        // `ok` at `100% described` while 39 of its 40 nodes were
        // fabricated, because the invented nodes inflated the metric
        // instead of depressing it. A tool tripping structure-sanity is
        // suspicious regardless of how "described" its possibly-invented
        // flags look.
        "suspicious"
    } else if pct_flags_with_text.map(|p| p < 50.0).unwrap_or(false) {
        "low-confidence"
    } else if result.root.as_ref().is_some_and(has_unfollowed_confession) {
        // Spec §6 rule 2b: a confession was printed and detected but not
        // followed — an unrecognised word/shape, a failed probe, or a
        // rule 0 refusal. Only ever reached from the `ok` branch's
        // territory: `verbatim`/`suspicious`/`low-confidence` are all
        // already worse than `incomplete` on the ladder and stay exactly
        // as they were — this caps an otherwise-`ok` result, it never
        // raises a worse one. curl's real failure mode was a confident
        // `ok` on a document its own text said was truncated;
        // `incomplete` is the honest answer instead.
        "incomplete"
    } else {
        "ok"
    };

    Status {
        suspicious_nodes,
        verbatim,
        pct_flags_with_text,
        label,
    }
}

/// Count descendant nodes (root excluded — see below) that fail spec
/// §13.1's structure-sanity check: a name that doesn't look like a real
/// command ([`is_command_name_shaped`]), or a node with no flags, no
/// children, no summary, and no [`CommandNode::heading_attested`]
/// evidence.
///
/// **Why emptiness alone isn't enough.** A node that exists but carries
/// nothing is exactly what a mis-parsed continuation line or enum-value-
/// turned-subcommand looks like — *and* exactly what a real,
/// legitimately description-less command looks like (`openssl --help`
/// lists 152 real commands as a bare word grid with no per-entry
/// description). The distinguishing signal is provenance, not emptiness:
/// `heading_attested` is set only where the parser already had positive
/// evidence of a real command list, so an empty node without it is still
/// [M-10]'s phantom-subcommand shape and stays suspicious.
///
/// The root is excluded from the name-shape half: it's the literal
/// executable name resolved from `PATH` (or, for the corpus runner, the
/// fixture's declared tool name), never something a tier guessed at, and
/// plenty of legitimate real-world binaries (`NetworkManager`,
/// `aarch64-linux-gnu-cpp-13`) fail `^[a-z][a-z0-9_.-]*$` on casing or a
/// leading digit alone.
pub fn structure_sanity(root: &CommandNode) -> usize {
    root.subcommands.iter().map(count_suspicious).sum()
}

/// True when `root` or any descendant carries a confession that was
/// detected but not followed (spec §6 rule 2b): `Confession { followed:
/// false, .. }` anywhere in the tree. Recursive because a subcommand's own
/// `--help` text can confess exactly as a root's can — the ladder cap
/// isn't a root-only special case.
fn has_unfollowed_confession(node: &CommandNode) -> bool {
    node.confession.as_ref().is_some_and(|c| !c.followed)
        || node.subcommands.iter().any(has_unfollowed_confession)
}

fn count_suspicious(node: &CommandNode) -> usize {
    let bad_name = !is_command_name_shaped(&node.name);
    let empty = node.flags.is_empty()
        && node.subcommands.is_empty()
        && node.summary.is_none()
        && !node.heading_attested;
    let this_node = usize::from(bad_name || empty);
    this_node + node.subcommands.iter().map(count_suspicious).sum::<usize>()
}

/// The `min_status` ladder (`corpus/README.md`'s `[contract]` field),
/// coarsest to finest. Defined exactly once so `xtask corpus`'s contract
/// check and any future consumer agree on what "at least ok" means.
///
/// `"incomplete"` (spec §6 rule 2b) slots in **between** `"low-confidence"`
/// and `"ok"`, not after `"ok"` — an earlier doc comment here anticipated
/// the field name but guessed the wrong end of the ladder; WS5's approved
/// amendment is explicit: `ok > incomplete > low-confidence > verbatim >
/// no-tier`. A tool this status names had its structure and prose parsed
/// exactly as well as an `ok` tool — the *only* thing wrong is that its
/// own text said the document was incomplete and following it didn't
/// happen, which is a narrower, better-understood gap than
/// `low-confidence`'s "the grammar barely understood this at all".
///
/// **`"suspicious"` is deliberately not on this ladder.** It is not a
/// coarser or finer degree of the same "how much did we recover" scale
/// the other five measure — it means the parser recovered structure that
/// isn't real. AGENTS.md's structure-sanity invariant is explicit that
/// fabricated structure is *worse* than missing structure, so no
/// `min_status` floor is satisfiable by a suspicious result:
/// [`meets_min_status`] fails it outright rather than ranking it.
const STATUS_LADDER: &[&str] = &["no-tier", "verbatim", "low-confidence", "incomplete", "ok"];

/// `label`'s position on [`STATUS_LADDER`] (coarsest = 0), for callers that
/// need to compare two `min_status` values directly rather than just ask
/// "does A meet floor B" — the corpus runner's contract-weakening check
/// (`corpus.rs`) is the first one: "was `min_status` lowered" needs an
/// ordering between two contract values, not a floor check against a
/// result. `None` for an unrecognized label (including `"suspicious"`,
/// which the ladder deliberately excludes — see its own doc comment),
/// exactly as [`meets_min_status`] already treats an unrecognized label as
/// "fails closed" rather than panicking.
pub fn status_rank(label: &str) -> Option<usize> {
    STATUS_LADDER.iter().position(|s| *s == label)
}

/// True when `actual` meets or exceeds the `min` floor on [`STATUS_LADDER`].
/// `"suspicious"` never meets any floor (see the ladder's doc comment).
/// An unrecognized label in either position fails closed (`false`) rather
/// than panicking, since `min` ultimately comes from a hand-edited
/// `meta.toml` a contributor could typo.
pub fn meets_min_status(actual: &str, min: &str) -> bool {
    if actual == "suspicious" {
        return false;
    }
    let actual_rank = STATUS_LADDER.iter().position(|s| *s == actual);
    let min_rank = STATUS_LADDER.iter().position(|s| *s == min);
    matches!((actual_rank, min_rank), (Some(a), Some(m)) if a >= m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ladder_orders_coarsest_to_finest() {
        assert!(meets_min_status("ok", "ok"));
        assert!(meets_min_status("ok", "low-confidence"));
        assert!(meets_min_status("ok", "verbatim"));
        assert!(meets_min_status("ok", "no-tier"));
        assert!(meets_min_status("low-confidence", "verbatim"));
        assert!(!meets_min_status("verbatim", "ok"));
        assert!(!meets_min_status("no-tier", "verbatim"));
    }

    #[test]
    fn suspicious_never_meets_any_floor_including_the_lowest() {
        assert!(!meets_min_status("suspicious", "no-tier"));
    }

    #[test]
    fn equal_ranks_meet_the_floor() {
        assert!(meets_min_status("no-tier", "no-tier"));
        assert!(meets_min_status("verbatim", "verbatim"));
    }

    /// [M-15]/spec §13's regression net, end to end: a root whose only
    /// flags are usage-synopsis-derived (undescribable by construction)
    /// must report `pct_flags_with_text: None`, not a bottomed-out ratio near
    /// 0% — exactly the shape that used to force `low-confidence` and is
    /// the defect this whole redefinition fixes.
    #[test]
    fn pct_flags_with_text_is_none_when_every_flag_is_synopsis_derived() {
        use mandible_core::{Provenance, Source};

        let mut root = CommandNode::new("git", Provenance::single(Source::HelpText));
        root.flags.push(mandible_core::Flag::long(
            "paginate",
            Provenance::single(Source::HelpTextSynopsis),
        ));
        let result = ExtractionResult {
            tool: "git".to_string(),
            root: Some(root),
            tier_statuses: Vec::new(),
            elapsed: std::time::Duration::default(),
        };
        let status = compute(&result);
        assert_eq!(status.pct_flags_with_text, None);
        assert_eq!(status.label, "ok");
    }

    /// The exact shape [M-15]/spec §13's git fixture reproduces: a root
    /// with undescribed synopsis-only flags, and a subcommand whose flags
    /// are fully described from a real `Options:`-style block. Before this
    /// redefinition, the synopsis flags counted as "undescribed" and
    /// dragged the whole tree's ratio under `status.rs`'s 50% floor
    /// (`low-confidence`); after it, they are excluded from the
    /// denominator entirely and the ratio is exactly the describable
    /// subset's own 100%.
    #[test]
    fn pct_flags_with_text_excludes_synopsis_flags_and_git_returns_to_ok() {
        use mandible_core::{Provenance, Source};

        let mut root = CommandNode::new("git", Provenance::single(Source::HelpText));
        for name in ["paginate", "git-dir", "no-pager"] {
            root.flags.push(mandible_core::Flag::long(
                name,
                Provenance::single(Source::HelpTextSynopsis),
            ));
        }
        let mut restore = CommandNode::new("restore", Provenance::single(Source::HelpText));
        for name in ["source", "staged"] {
            let mut f = mandible_core::Flag::long(name, Provenance::single(Source::HelpText));
            f.description = Some(mandible_core::Text::sanitize("described from -h"));
            restore.flags.push(f);
        }
        root.subcommands.push(restore);

        let result = ExtractionResult {
            tool: "git".to_string(),
            root: Some(root),
            tier_statuses: Vec::new(),
            elapsed: std::time::Duration::default(),
        };
        let status = compute(&result);
        assert_eq!(
            result.flag_count(),
            5,
            "raw count still includes the 3 synopsis flags"
        );
        assert_eq!(result.describable_flag_count(), 2);
        assert_eq!(status.pct_flags_with_text, Some(100.0));
        assert_eq!(status.label, "ok");
    }

    #[test]
    fn unrecognized_labels_fail_closed() {
        assert!(!meets_min_status("ok", "bogus-status"));
        assert!(!meets_min_status("typo'd-status", "ok"));
    }

    // --- spec §6 rule 2b: the `incomplete` status ---

    fn node_with_unfollowed_confession() -> CommandNode {
        use mandible_core::{Confession, Provenance, Source};
        let mut root = CommandNode::new("curl", Provenance::single(Source::HelpText));
        root.confession = Some(Confession {
            word: "all".to_string(),
            flag: "--help".to_string(),
            followed: false,
        });
        let mut f = mandible_core::Flag::long("verbose", Provenance::single(Source::HelpText));
        f.description = Some(mandible_core::Text::sanitize("be more talkative"));
        root.flags.push(f);
        root
    }

    /// The exact defect this status exists to fix: curl's real failure was
    /// a confident `ok` on a document its own text said was truncated.
    /// An otherwise-perfectly-described tree with an unfollowed confession
    /// must report `incomplete`, not `ok`.
    #[test]
    fn an_otherwise_ok_tree_with_an_unfollowed_confession_is_incomplete() {
        let result = ExtractionResult {
            tool: "curl".to_string(),
            root: Some(node_with_unfollowed_confession()),
            tier_statuses: Vec::new(),
            elapsed: std::time::Duration::default(),
        };
        let status = compute(&result);
        assert_eq!(status.label, "incomplete");
    }

    /// The cap only ever pulls an `ok` result down — it must never turn a
    /// `low-confidence` result into something that reads as *better*.
    #[test]
    fn incomplete_never_elevates_a_worse_status() {
        use mandible_core::{Confession, Provenance, Source};
        let mut root = CommandNode::new("mystery", Provenance::single(Source::HelpText));
        root.confession = Some(Confession {
            word: "all".to_string(),
            flag: "--help".to_string(),
            followed: false,
        });
        // No description at all -> pct_flags_with_text stays None-or-low;
        // add one undescribed flag to force `low-confidence` via the
        // existing 50% rule rather than `pct_flags_with_text: None`.
        root.flags.push(mandible_core::Flag::long(
            "verbose",
            Provenance::single(Source::HelpText),
        ));
        root.flags.push({
            let mut f = mandible_core::Flag::long("quiet", Provenance::single(Source::HelpText));
            f.description = None;
            f
        });
        let result = ExtractionResult {
            tool: "mystery".to_string(),
            root: Some(root),
            tier_statuses: Vec::new(),
            elapsed: std::time::Duration::default(),
        };
        let status = compute(&result);
        assert_eq!(status.label, "low-confidence");
    }

    /// A confession on a *subcommand*, not just the root, still caps the
    /// whole tree's status — the cap is not a root-only special case.
    #[test]
    fn an_unfollowed_confession_on_a_subcommand_still_caps_the_tree() {
        use mandible_core::{Confession, Provenance, Source};
        let mut root = CommandNode::new("tool", Provenance::single(Source::HelpText));
        let mut f = mandible_core::Flag::long("verbose", Provenance::single(Source::HelpText));
        f.description = Some(mandible_core::Text::sanitize("be more talkative"));
        root.flags.push(f);

        let mut child = CommandNode::new("sub", Provenance::single(Source::HelpText));
        child.confession = Some(Confession {
            word: "all".to_string(),
            flag: "--help".to_string(),
            followed: false,
        });
        let mut cf = mandible_core::Flag::long("amend", Provenance::single(Source::HelpText));
        cf.description = Some(mandible_core::Text::sanitize("amend the previous thing"));
        child.flags.push(cf);
        root.subcommands.push(child);

        let result = ExtractionResult {
            tool: "tool".to_string(),
            root: Some(root),
            tier_statuses: Vec::new(),
            elapsed: std::time::Duration::default(),
        };
        let status = compute(&result);
        assert_eq!(status.label, "incomplete");
    }

    /// A *followed* confession (the expanded document is what the tree
    /// reflects) must never cap anything — this is the success case, and
    /// it reports `ok` exactly like a tool that never confessed at all.
    #[test]
    fn a_followed_confession_does_not_cap_the_status() {
        use mandible_core::{Confession, Provenance, Source};
        let mut root = CommandNode::new("curl", Provenance::single(Source::HelpText));
        root.confession = Some(Confession {
            word: "all".to_string(),
            flag: "--help".to_string(),
            followed: true,
        });
        let mut f = mandible_core::Flag::long("verbose", Provenance::single(Source::HelpText));
        f.description = Some(mandible_core::Text::sanitize("be more talkative"));
        root.flags.push(f);

        let result = ExtractionResult {
            tool: "curl".to_string(),
            root: Some(root),
            tier_statuses: Vec::new(),
            elapsed: std::time::Duration::default(),
        };
        let status = compute(&result);
        assert_eq!(status.label, "ok");
    }

    #[test]
    fn incomplete_ranks_between_low_confidence_and_ok() {
        assert!(meets_min_status("ok", "incomplete"));
        assert!(!meets_min_status("low-confidence", "incomplete"));
        assert!(meets_min_status("incomplete", "low-confidence"));
        assert!(!meets_min_status("incomplete", "ok"));
    }
}
