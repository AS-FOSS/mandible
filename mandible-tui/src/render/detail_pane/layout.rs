//! Column widths for a section: where a spelling starts and how much
//! room its description gets.

use super::*;

/// A spelling wider than this fraction of the pane does not get to set the
/// shared column — it runs on past it instead, pushing its own first
/// description line and nothing else (see [`SectionLayout`]). Mirrors the
/// tree pane's summary-column rule (spec §9.1).
pub(super) const DESC_COLUMN_CAP_PERCENT: usize = 45;

/// The share of a section's entities the shared column is fitted to
/// (spec §9.3: "roughly the p90 spelling width — the majority, not the
/// outliers").
///
/// Not the maximum. A column fitted to the widest spelling in the section
/// is a column one entity chose for every other one, and the wider that
/// entity is the less room the rest of the section's prose gets. Fitting
/// the ninetieth percentile spends one extra line on the widest tenth and
/// gives the width back to the other nine.
pub(super) const SHARED_COLUMN_PERCENTILE: usize = 90;

/// The narrowest a description is allowed to be. A section's shared
/// column is clamped down until this much of the pane is left for prose,
/// however wide the section's heads are (spec §9.3). Measured against
/// real output: at 20 columns `docker pull`'s `--platform` description
/// breaks across six lines, one mid-word; at 28 it reads as prose.
pub(super) const MIN_DESC_WIDTH: usize = 28;

/// Where a short spelling starts: the true left edge of the content area
/// (spec §9.3). There is no uniform margin on a list section — the row's
/// own shape decides which of the two columns it starts at.
pub(super) const SHORT_COLUMN: usize = 0;

/// Where a long spelling starts, whether or not a short precedes it
/// (spec §9.3): the display width of a short prefix, `-X, `.
///
/// A row that has a short renders it at [`SHORT_COLUMN`] and its long
/// lands here by arithmetic; a row with no short is preindented to the
/// same place. That is the whole point — the eye follows the longs down
/// one column without having to know which rows happen to have a short
/// letter as well.
pub(super) const LONG_COLUMN: usize = "-X, ".len();

/// The indent POSITIONALS rows are inset by (spec §9.3): two columns, its
/// own number and deliberately not [`LONG_COLUMN`], since coupling it to
/// the flag columns would move two unrelated layouts together. MODIFIERS
/// and ENVIRONMENT are bare-name sections too but stay laid out like
/// FLAGS, against the content edge.
pub(super) const POSITIONAL_INDENT: usize = 2;

/// The column an entity's spellings start at within a section indented by
/// `indent` (spec §9.3). Shape decides it, never kind: a row whose first
/// spelling is a short (or a dashless name) starts at the content edge; a
/// long-only row is preindented to the same column a short row's long
/// lands at. A row with more than two spellings (`-h, -?, -help, --help`)
/// always flows from the short column, since there is no single "the
/// long" in it to align to.
pub(super) fn spelling_column(entity: &Entity, indent: usize) -> usize {
    indent + bare_spelling_column(entity)
}

/// [`spelling_column`] before the section's own indent is added.
pub(super) fn bare_spelling_column(entity: &Entity) -> usize {
    if entity.spellings.len() > 2 {
        return SHORT_COLUMN;
    }
    if entity
        .spellings
        .iter()
        .any(|s| matches!(s.dashes, Dashes::None))
    {
        return SHORT_COLUMN;
    }
    if entity.short_spelling().is_some() {
        return SHORT_COLUMN;
    }
    LONG_COLUMN
}

/// How a whole section is arranged. Chosen once per section, never per
/// row, and per section rather than per pane (spec §9.3): positionals,
/// flags, modifiers and environment variables have nothing to say to
/// each other's widths. A row too wide for the column pushes only its
/// own first description line right, one space past its head; every
/// other line, and every other row, stays on the column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SectionLayout {
    /// The section's shared description column: where every description
    /// line in the section begins. The value placeholder is measured as
    /// part of the spelling, not given its own aligned slot (spec §9.3).
    pub(super) description: usize,
    /// Columns every row in the section is inset by — [`POSITIONAL_INDENT`]
    /// for POSITIONALS, zero for the flag-shaped sections.
    pub(super) indent: usize,
}

/// The width that fits [`SHARED_COLUMN_PERCENTILE`] of `widths` — the
/// smallest number at least that share of the entries are within.
///
/// Zero for an empty section, and the maximum for a section small enough
/// that the percentile lands on it: a list of three flags aligns all three,
/// because "the majority, not the outliers" only has anything to exclude
/// once there is a tail to be an outlier in.
pub(super) fn percentile_width(widths: impl Iterator<Item = usize>) -> usize {
    let mut widths: Vec<usize> = widths.collect();
    if widths.is_empty() {
        return 0;
    }
    widths.sort_unstable();
    let n = widths.len();
    let rank = (n * SHARED_COLUMN_PERCENTILE).div_ceil(100).max(1);
    widths[rank - 1]
}

/// The layout for one section's `entities` in a pane `width` columns wide,
/// inset by `indent`. The shared column is the lowest of three bounds:
///
/// 1. The percentile (spec §9.3): fitted to the majority, widest tenth
///    excluded rather than clamped.
/// 2. The pane cap (spec §9.1a): a head past [`DESC_COLUMN_CAP_PERCENT`]
///    gets no vote.
/// 3. The clamp (spec §9.3): comes down until [`MIN_DESC_WIDTH`] columns
///    are left for prose.
///
/// ...floored at two past the deepest column a spelling can start at, so
/// a description never lands left of the preindented longs it belongs to.
pub(super) fn section_layout(entities: &[&Entity], width: usize, indent: usize) -> SectionLayout {
    let cap = width * DESC_COLUMN_CAP_PERCENT / 100;
    let gap = 2;

    // One measured width per row, from the pane's own left edge to the end
    // of the row's placeholder: a preindented long is measured where it
    // actually starts, and a placeholder is measured as part of the
    // spelling it belongs to rather than against a slot of its own.
    let fits = |w: usize| w + gap <= cap;
    let fitting = entities
        .iter()
        .map(|e| entity_head_width(e, indent))
        .filter(|w| fits(*w));

    let floor = indent + LONG_COLUMN + gap;
    let description = (percentile_width(fitting) + gap)
        .min(width.saturating_sub(MIN_DESC_WIDTH))
        .max(floor);
    SectionLayout {
        description,
        indent,
    }
}
