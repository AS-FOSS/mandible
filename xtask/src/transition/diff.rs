//! The full computed diff between two [`super::ParsedScoreboard`]s
//! ([`Transition`], [`diff`]) and the field-level comparison
//! ([`field_diff`]) — rendered by [`super::render_text`]/
//! [`super::render_markdown`], never disagreeing about what changed.

use super::fingerprint::{ParsedFingerprint, EMPTY_FINGERPRINT};
use super::ParsedScoreboard;

/// One matched tool's flag-count comparison. Kept as a signed delta
/// alongside both raw counts — never reduced to a single "net" number, per
/// this module's doc comment on why netting hides exactly the losses that
/// caught real regressions on this branch.
pub(super) struct FlagDelta<'a> {
    pub(super) tool: &'a str,
    pub(super) before: usize,
    pub(super) after: usize,
}

impl FlagDelta<'_> {
    pub(super) fn delta(&self) -> i64 {
        self.after as i64 - self.before as i64
    }
}

/// One matched tool's status change.
pub(super) struct StatusTransition<'a> {
    pub(super) tool: &'a str,
    pub(super) before: &'a str,
    pub(super) after: &'a str,
}

/// One matched tool's field-level diff (WS2 part 2) — the granularity a
/// bare flag-count delta cannot see. Every list is a set of stable flag
/// identities or subcommand paths ([`crate::coverage::flag_identity`]),
/// never a count, per this module's requirement to report *what* changed,
/// not just *how many* — a count here would rebuild exactly the blind spot
/// this task exists to close.
pub(super) struct FieldDiff<'a> {
    pub(super) tool: &'a str,
    pub(super) flags_added: Vec<&'a str>,
    pub(super) flags_removed: Vec<&'a str>,
    /// Flags present on both sides whose description's presence or hash
    /// differs — catches both "text deleted" (`has_description` flips) and
    /// "text changed to something else" (hash differs, presence unchanged).
    pub(super) description_changed: Vec<&'a str>,
    /// Flags present on both sides whose choices-list hash differs —
    /// catches both an added/fabricated choices list and a removed one
    /// (`None` on one side, `Some` on the other hashes as unequal).
    pub(super) choices_changed: Vec<&'a str>,
    /// Flags present on both sides whose `value_name` text differs.
    pub(super) value_name_changed: Vec<&'a str>,
    pub(super) subcommands_added: Vec<&'a str>,
    pub(super) subcommands_removed: Vec<&'a str>,
    pub(super) tier_changed: Option<(&'a str, &'a str)>,
    pub(super) framework_changed: Option<(&'a str, &'a str)>,
}

impl FieldDiff<'_> {
    /// True if this tool has at least one field-level change — the
    /// predicate that decides whether it earns a row in the report at all
    /// ([`diff`] only ever constructs a `FieldDiff` when this would be
    /// true, but kept as a named method rather than inlined so the "what
    /// counts as changed" list has exactly one definition).
    fn is_empty(&self) -> bool {
        self.flags_added.is_empty()
            && self.flags_removed.is_empty()
            && self.description_changed.is_empty()
            && self.choices_changed.is_empty()
            && self.value_name_changed.is_empty()
            && self.subcommands_added.is_empty()
            && self.subcommands_removed.is_empty()
            && self.tier_changed.is_none()
            && self.framework_changed.is_none()
    }
}

/// The full computed diff between two scoreboards, ready to render in
/// either format — computed once, rendered by [`render_text`] or
/// [`render_markdown`] so the two formats can never disagree about what
/// changed, only how it's displayed.
pub struct Transition<'a> {
    pub(super) before: &'a ParsedScoreboard,
    pub(super) after: &'a ParsedScoreboard,
    pub(super) appeared: Vec<&'a str>,
    pub(super) disappeared: Vec<&'a str>,
    pub(super) near_cap: Vec<&'a str>,
    pub(super) status_transitions: Vec<StatusTransition<'a>>,
    pub(super) flag_gains: Vec<FlagDelta<'a>>,
    pub(super) flag_losses: Vec<FlagDelta<'a>>,
    /// Per-tool field-level diffs — only tools with at least one change
    /// ([`FieldDiff::is_empty`] false), sorted by tool name. Empty (not
    /// absent) when neither side's scoreboard carries a `#fp` footer at
    /// all, or when every matched tool's fingerprint is identical.
    pub(super) field_diffs: Vec<FieldDiff<'a>>,
    /// Tools present, matched, and outside the near-cap exclusion, but
    /// whose fingerprint could not be compared because at least one side's
    /// scoreboard predates the `#fp` footer (`ParsedScoreboard::fingerprints`'s
    /// doc comment) — reported so "no field-level changes" is never
    /// confused with "field-level comparison wasn't possible."
    pub(super) field_diff_unmeasured: usize,
}

impl Transition<'_> {
    /// **The identical/changed determination `sweep-diff` reports.** A run
    /// is only "identical" when *nothing* changed across every dimension
    /// this module measures — appearances, disappearances, status,
    /// flag-count, and now field-level content — not merely when the
    /// coarser dimensions stayed flat. This is exactly the gap PR #14 fell
    /// through: `pngfix`'s and `pod2man`'s flag *counts* were unchanged (a
    /// description going empty doesn't remove the flag, and a fabricated
    /// choices list doesn't add one), so a determination based on counts
    /// alone would still call that run identical. Non-blocking either way
    /// (maintainer decision D4, this module's own doc comment) — this
    /// governs what the report *says*, never the exit code.
    pub fn is_identical(&self) -> bool {
        self.appeared.is_empty()
            && self.disappeared.is_empty()
            && self.status_transitions.is_empty()
            && self.flag_gains.is_empty()
            && self.flag_losses.is_empty()
            && self.field_diffs.is_empty()
    }
}

/// Compute the transition between two parsed scoreboards.
///
/// Tools whose `ms` is [`near_timeout_cap`] on *either* side are excluded
/// from status transitions and flag deltas entirely (spec §13.1b rule 3;
/// maintainer decision D4) and reported only in their own section — a
/// status or count derived under timeout pressure is a statement about the
/// machine that ran it, not the parser, and mixing it into the headline
/// numbers is exactly the `waagent2.0` false regression (AGENTS.md) this
/// rule exists to stop from recurring.
pub fn diff<'a>(before: &'a ParsedScoreboard, after: &'a ParsedScoreboard) -> Transition<'a> {
    let mut appeared = Vec::new();
    let mut disappeared = Vec::new();
    let mut near_cap = Vec::new();
    let mut status_transitions = Vec::new();
    let mut flag_gains = Vec::new();
    let mut flag_losses = Vec::new();
    let mut field_diffs = Vec::new();
    let mut field_diff_unmeasured = 0usize;

    for (tool, after_row) in &after.rows {
        let Some(before_row) = before.rows.get(tool) else {
            appeared.push(tool.as_str());
            continue;
        };
        if before_row.near_cap() || after_row.near_cap() {
            near_cap.push(tool.as_str());
            continue;
        }
        if before_row.status != after_row.status {
            status_transitions.push(StatusTransition {
                tool,
                before: &before_row.status,
                after: &after_row.status,
            });
        }
        if before_row.flags != after_row.flags {
            let d = FlagDelta {
                tool,
                before: before_row.flags,
                after: after_row.flags,
            };
            if d.delta() > 0 {
                flag_gains.push(d);
            } else {
                flag_losses.push(d);
            }
        }

        let tier_changed = (before_row.tiers != after_row.tiers)
            .then_some((before_row.tiers.as_str(), after_row.tiers.as_str()));
        let framework_changed = (before_row.framework != after_row.framework)
            .then_some((before_row.framework.as_str(), after_row.framework.as_str()));

        // Three states, not two (the defect this match used to have:
        // `coverage::fingerprint_lines` used to skip a row with no flags and
        // no subcommands, so a tool that lost every flag produced a line on
        // the "before" side and none on the "after" side, and fell into the
        // catch-all below — "unmeasured" — instead of reporting the total
        // loss it actually was). Now that every row gets a `#fp` line
        // unconditionally, a line is absent on *both* sides only for a
        // genuinely legacy scoreboard pair; absent on *one* side only means
        // "no record for this side," read as empty (`EMPTY_FINGERPRINT`'s
        // own doc comment) so the diff still reports the present side's
        // flags/subcommands as added or removed rather than staying silent.
        match (before.fingerprints.get(tool), after.fingerprints.get(tool)) {
            (None, None) => {
                // Neither side has a `#fp` entry for this tool — the
                // genuine legacy case (this scoreboard pair predates the
                // footer entirely, or — vanishingly rarely — this one row's
                // line failed to parse on both sides). Field-level
                // comparison is impossible, not "nothing changed"
                // (`ParsedScoreboard::fingerprints`'s doc comment). Still
                // surface a tier/framework change if one was found from the
                // ordinary columns, which every scoreboard shape carries.
                if tier_changed.is_some() || framework_changed.is_some() {
                    field_diffs.push(FieldDiff {
                        tool,
                        flags_added: Vec::new(),
                        flags_removed: Vec::new(),
                        description_changed: Vec::new(),
                        choices_changed: Vec::new(),
                        value_name_changed: Vec::new(),
                        subcommands_added: Vec::new(),
                        subcommands_removed: Vec::new(),
                        tier_changed,
                        framework_changed,
                    });
                } else {
                    field_diff_unmeasured += 1;
                }
            }
            (bfp, afp) => {
                // At least one side has a real entry — diff it against the
                // other side's entry, or against `EMPTY_FINGERPRINT` when
                // the other side has none. Covers both the ordinary
                // both-measured case and the deletion/mixed-vintage case.
                let bfp = bfp.unwrap_or(&EMPTY_FINGERPRINT);
                let afp = afp.unwrap_or(&EMPTY_FINGERPRINT);
                let fd = field_diff(tool, bfp, afp, tier_changed, framework_changed);
                if !fd.is_empty() {
                    field_diffs.push(fd);
                }
            }
        }
    }
    for tool in before.rows.keys() {
        if !after.rows.contains_key(tool) {
            disappeared.push(tool.as_str());
        }
    }

    appeared.sort_unstable();
    disappeared.sort_unstable();
    near_cap.sort_unstable();
    // Losses first within their own list, worst (most flags lost) first —
    // "the bar is losses, not net" extends to ranking: the tool that lost
    // the most is the one worth looking at first.
    flag_losses.sort_by_key(|d| (d.delta(), d.tool.to_string()));
    flag_gains.sort_by_key(|d| (std::cmp::Reverse(d.delta()), d.tool.to_string()));
    status_transitions.sort_by_key(|t| t.tool.to_string());
    field_diffs.sort_by_key(|d| d.tool.to_string());

    Transition {
        before,
        after,
        appeared,
        disappeared,
        near_cap,
        status_transitions,
        flag_gains,
        flag_losses,
        field_diffs,
        field_diff_unmeasured,
    }
}

/// Compute one matched tool's [`FieldDiff`] from its before/after
/// fingerprints — pure set/map comparison, no I/O, no knowledge of what a
/// flag or subcommand *means*, only whether the same identity's recorded
/// fields match (this module's `no per-tool logic` invariant: nothing here
/// keys off a tool name).
pub(super) fn field_diff<'a>(
    tool: &'a str,
    before: &'a ParsedFingerprint,
    after: &'a ParsedFingerprint,
    tier_changed: Option<(&'a str, &'a str)>,
    framework_changed: Option<(&'a str, &'a str)>,
) -> FieldDiff<'a> {
    let mut flags_added = Vec::new();
    let mut flags_removed = Vec::new();
    let mut description_changed = Vec::new();
    let mut choices_changed = Vec::new();
    let mut value_name_changed = Vec::new();

    for (id, after_f) in &after.flags {
        match before.flags.get(id) {
            None => flags_added.push(id.as_str()),
            Some(before_f) => {
                if before_f.has_description != after_f.has_description
                    || before_f.description_hash != after_f.description_hash
                {
                    description_changed.push(id.as_str());
                }
                if before_f.choices_hash != after_f.choices_hash {
                    choices_changed.push(id.as_str());
                }
                if before_f.value_name != after_f.value_name {
                    value_name_changed.push(id.as_str());
                }
            }
        }
    }
    for id in before.flags.keys() {
        if !after.flags.contains_key(id) {
            flags_removed.push(id.as_str());
        }
    }

    let subcommands_added = after
        .subcommands
        .iter()
        .filter(|s| !before.subcommands.contains(*s))
        .map(String::as_str)
        .collect();
    let subcommands_removed = before
        .subcommands
        .iter()
        .filter(|s| !after.subcommands.contains(*s))
        .map(String::as_str)
        .collect();

    flags_added.sort_unstable();
    flags_removed.sort_unstable();
    description_changed.sort_unstable();
    choices_changed.sort_unstable();
    value_name_changed.sort_unstable();

    FieldDiff {
        tool,
        flags_added,
        flags_removed,
        description_changed,
        choices_changed,
        value_name_changed,
        subcommands_added,
        subcommands_removed,
        tier_changed,
        framework_changed,
    }
}
