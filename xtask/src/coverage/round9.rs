//! The round-9 family detectors, atlas S-143 and S-144 (issue #138).
//! Split into their own file for the same line-count reason `score.rs`'s
//! own round functions are.

use crate::detector::{Detector, ToolEvidence};
use mandible_core::CommandNode;

pub(super) fn round9_family_counts(
    raw: &str,
    root: &CommandNode,
) -> Vec<(&'static str, usize, Vec<String>)> {
    let cap = super::score::FAMILY_DETECTOR_SAMPLES_PER_ROW;
    let evidence = ToolEvidence { raw, root };
    let cmd = crate::detector::lowdown_bullet_command_row::LowdownBulletCommandRow.hits(&evidence);
    let opt = crate::detector::lowdown_bullet_option_row::LowdownBulletOptionRow.hits(&evidence);
    vec![
        (
            "lowdown-bullet-command-row",
            cmd.len(),
            cmd.into_iter().take(cap).collect(),
        ),
        (
            "lowdown-bullet-option-row",
            opt.len(),
            opt.into_iter().take(cap).collect(),
        ),
    ]
}
