//! The round-9 family detector, atlas S-148: the option-table sibling of
//! S-131's own usage-synopsis shape (`gdbus-codegen --annotate`'s value
//! name split at its own first word). Split into its own file for the
//! same line-count reason `round8.rs` is. Count only this round — no
//! parser change ships for it.

use super::score::FAMILY_DETECTOR_SAMPLES_PER_ROW;
use crate::detector::{Detector, ToolEvidence};
use mandible_core::CommandNode;

pub(super) fn round9_family_counts(
    raw: &str,
    root: &CommandNode,
) -> Vec<(&'static str, usize, Vec<String>)> {
    let cap = FAMILY_DETECTOR_SAMPLES_PER_ROW;
    let evidence = ToolEvidence { raw, root };
    let hits = crate::detector::option_table_multiword_value_name::OptionTableMultiwordValueName
        .hits(&evidence);
    vec![(
        "option-table-multiword-value-name",
        hits.len(),
        hits.into_iter().take(cap).collect(),
    )]
}
