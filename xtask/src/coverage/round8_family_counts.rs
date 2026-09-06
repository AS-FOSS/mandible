//! The round-8 family detector's own coverage-footer wiring, atlas S-142's
//! two measurement halves. Split into its own file rather than added to
//! `score.rs`, which is already at `AGENTS.md`'s own 800-line ceiling
//! (`scripts/shape_guard.sh`).

use super::score::FAMILY_DETECTOR_SAMPLES_PER_ROW;
use mandible_core::CommandNode;

pub(super) fn round8_usage_family_counts(
    raw: &str,
    root: &CommandNode,
) -> Vec<(&'static str, usize, Vec<String>)> {
    let cap = FAMILY_DETECTOR_SAMPLES_PER_ROW;
    let lg = crate::detector::usage_label_glued_to_program_name::detect(raw, root);
    let ob = crate::detector::usage_open_bracket_continues_at_column_zero::detect(raw);
    vec![
        (
            "usage-label-glued-to-program-name",
            lg.finding_count(),
            lg.findings
                .iter()
                .take(cap)
                .map(|f| format!("{:?} glued to the program name, from {:?}", f.label, f.line))
                .collect(),
        ),
        (
            "usage-open-bracket-continues-at-column-zero",
            ob.finding_count(),
            ob.findings
                .iter()
                .take(cap)
                .map(|f| {
                    format!(
                        "open bracket carried from {:?} into {:?}",
                        f.first_line, f.continuation
                    )
                })
                .collect(),
        ),
    ]
}
