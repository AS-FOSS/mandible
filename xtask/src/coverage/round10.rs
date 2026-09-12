//! The round-10 family detectors, atlas S-157 upward.

use super::score::FAMILY_DETECTOR_SAMPLES_PER_ROW;
use crate::detector::{Detector, ToolEvidence};
use mandible_core::CommandNode;

pub(super) fn round10_family_counts(
    raw: &str,
    root: &CommandNode,
) -> Vec<(&'static str, usize, Vec<String>)> {
    let cap = FAMILY_DETECTOR_SAMPLES_PER_ROW;
    let evidence = ToolEvidence { raw, root };
    let sbw =
        crate::detector::spaced_bare_word_table_value::SpacedBareWordTableValue.hits(&evidence);
    vec![(
        "spaced-bare-word-table-value",
        sbw.len(),
        sbw.into_iter().take(cap).collect(),
    )]
}
