//! Column widths for a section: where a spelling starts and how much
//! room its description gets.

use super::*;

/// The width a line is wrapped to when wrapping is off: wide enough that
/// nothing ever breaks, small enough that column arithmetic cannot overflow.
pub(super) const NO_WRAP: usize = usize::MAX / 8;

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

/// The word length a dashless spelling's own name carries once a single
/// leading sigil character (`+` in `"+render"`, `"+i"`) is stripped off —
/// zero for the bare sigil alone (`"+"`), and the name's own full length
/// for a spelling that never had one (a modifier letter, an environment
/// variable name). See [`bare_spelling_column`] and docs/shapes.md S-163.
fn dashless_word_len(name: &str) -> usize {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if !c.is_alphanumeric() => chars.count(),
        _ => name.chars().count(),
    }
}

/// [`spelling_column`] before the section's own indent is added.
pub(super) fn bare_spelling_column(entity: &Entity) -> usize {
    if entity.spellings.len() > 2 {
        return SHORT_COLUMN;
    }
    // A dashless spelling reads as a short flag's own column when its word
    // is at most one character (`+i`, the bare `+`), or when it isn't a
    // `Flag` at all — a modifier letter or an environment variable name
    // stays at the content edge whatever its length (spec §9.3, "MODIFIERS
    // and ENVIRONMENT... stay laid out like FLAGS, against the content
    // edge"). A longer dashless flag spelling (`+render`, `+extension`,
    // S-163) is a single-dash-long-style spelling instead, and belongs
    // beside `-render` in the long column. See docs/shapes.md S-163.
    if entity.spellings.iter().any(|s| {
        matches!(s.dashes, Dashes::None)
            && (entity.kind != EntityKind::Flag || dashless_word_len(&s.name) <= 1)
    }) {
        return SHORT_COLUMN;
    }
    if entity.spellings.iter().any(|s| is_drawn_short(entity, s)) {
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
    /// True when the section is laid out as a man page: the head alone on
    /// its line, the description below it (docs/design.md, "Small
    /// terminals"). Only ever true while wrapping is on.
    pub(super) man: bool,
    /// False when the user turned wrapping off: nothing in the section
    /// wraps and every row keeps its description on its own line.
    pub(super) wrap: bool,
}

impl SectionLayout {
    /// The width text in this section is wrapped to in a pane `width`
    /// wide: the pane's, or [`NO_WRAP`].
    pub(super) fn text_width(self, width: usize) -> usize {
        if self.wrap {
            width
        } else {
            NO_WRAP
        }
    }

    /// Where a man-layout description starts.
    pub(super) fn man_column(self) -> usize {
        self.indent + fit::MAN_DESC_INDENT
    }
}

/// The width that fits [`fit::SHARED_COLUMN_PERCENTILE`] of `widths` — the
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
    let rank = (n * fit::SHARED_COLUMN_PERCENTILE).div_ceil(100).max(1);
    widths[rank - 1]
}

/// The layout for one section's `entities` in a pane `width` columns wide,
/// inset by `indent`. The shared column is the lower of two bounds,
/// floored so a description never lands left of the preindented longs:
///
/// 1. The percentile (spec §9.3): fitted to the majority, widest tenth
///    excluded rather than clamped.
/// 2. The pane cap (spec §9.1a): a head past
///    [`fit::DESC_COLUMN_CAP_PERCENT`] gets no vote.
///
/// The column is never squeezed. When what is left of the pane after the
/// p90 of all heads (outliers included) is not [`fit::MIN_DESC_ROOM`] columns, the section is a
/// man layout instead (unless wrapping is off).
pub(super) fn section_layout(
    entities: &[&Entity],
    width: usize,
    indent: usize,
    wrap: bool,
) -> SectionLayout {
    let cap = width * fit::DESC_COLUMN_CAP_PERCENT / 100;
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
    let description = (percentile_width(fitting) + gap).max(floor);
    // Room is judged against every head, outliers included: a head past the
    // cap pushes its own description however the column was fitted.
    let head = percentile_width(entities.iter().map(|e| entity_head_width(e, indent)));
    SectionLayout {
        description,
        indent,
        man: wrap && !fit::has_room(fit::section_room(width, head)),
        wrap,
    }
}
