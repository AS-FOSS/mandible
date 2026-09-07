//! The round-9 family detectors, atlas S-143, S-144 and S-148.
//! S-143 and S-144 are lowdown's bullet-marked command and option rows
//! (issue #138). S-148 is the option-table sibling of S-131's own
//! usage-synopsis shape, counted only, with no parser change behind it.
//! Split into their own file for the same line-count reason `round8.rs` is.

use super::score::FAMILY_DETECTOR_SAMPLES_PER_ROW;
use crate::detector::{Detector, ToolEvidence};
use mandible_core::CommandNode;

pub(super) fn round9_family_counts(
    raw: &str,
    root: &CommandNode,
) -> Vec<(&'static str, usize, Vec<String>)> {
    let cap = FAMILY_DETECTOR_SAMPLES_PER_ROW;
    let evidence = ToolEvidence { raw, root };
    let cmd = crate::detector::lowdown_bullet_command_row::LowdownBulletCommandRow.hits(&evidence);
    let opt = crate::detector::lowdown_bullet_option_row::LowdownBulletOptionRow.hits(&evidence);
    let annotate = crate::detector::option_table_multiword_value_name::OptionTableMultiwordValueName
        .hits(&evidence);
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
        (
            "option-table-multiword-value-name",
            annotate.len(),
            annotate.into_iter().take(cap).collect(),
        ),
    ]
}
