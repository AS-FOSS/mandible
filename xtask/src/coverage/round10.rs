//! The round-10 family detectors, atlas S-155 and S-156. S-155 is an
//! alternation value name becoming choices; fixed. S-156 is a
//! description tail enumerating choices; counted only, below the
//! five-tool bar. Split into its own file for the same line-count reason
//! `round7.rs`, `round8.rs` and `round9.rs` are.

use super::score::FAMILY_DETECTOR_SAMPLES_PER_ROW;
use mandible_core::CommandNode;

pub(super) fn round10_family_counts(
    raw: &str,
    root: &CommandNode,
) -> Vec<(&'static str, usize, Vec<String>)> {
    let cap = FAMILY_DETECTOR_SAMPLES_PER_ROW;
    let alt = crate::detector::alternation_value_is_choices::detect(raw, root);
    let tail = crate::detector::description_tail_enumerates_choices::detect(raw, root);
    vec![
        (
            "alternation-value-is-choices",
            alt.finding_count(),
            alt.findings
                .iter()
                .take(cap)
                .map(|f| {
                    format!(
                        "{:?}{} never became choices, from {:?}",
                        f.members, f.delimiter, f.line
                    )
                })
                .collect(),
        ),
        (
            "description-tail-enumerates-choices",
            tail.finding_count(),
            tail.findings
                .iter()
                .take(cap)
                .map(|f| {
                    format!(
                        "-{} {:?} tail never became choices: {:?}",
                        f.long, f.label, f.members
                    )
                })
                .collect(),
        ),
    ]
}
