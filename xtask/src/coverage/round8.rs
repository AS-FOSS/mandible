//! The round-8 family detectors, atlas S-137, S-139 and S-140. Split
//! into their own file for the same line-count reason `score.rs`'s own
//! round functions are.

use super::score::FAMILY_DETECTOR_SAMPLES_PER_ROW;
use crate::detector::{Detector, ToolEvidence};
use mandible_core::CommandNode;

pub(super) fn round8_family_counts(
    raw: &str,
    root: &CommandNode,
) -> Vec<(&'static str, usize, Vec<String>)> {
    let cap = FAMILY_DETECTOR_SAMPLES_PER_ROW;
    let evidence = ToolEvidence { raw, root };
    let ih = crate::detector::invocation_form_head_as_flag_group::InvocationFormHeadAsFlagGroup
        .hits(&evidence);
    let gu = crate::detector::glued_uppercase_shared_prefix::detect(raw, root);
    let dc = crate::detector::description_continuation_dash_flag::detect(raw, root);
    vec![
        (
            "invocation-form-head-as-flag-group",
            ih.len(),
            ih.into_iter().take(cap).collect(),
        ),
        (
            "glued-uppercase-shared-prefix",
            gu.finding_count(),
            gu.findings
                .iter()
                .take(cap)
                .map(|f| {
                    format!(
                        "-{} never became its own spelling, from {:?}",
                        f.name, f.line
                    )
                })
                .collect(),
        ),
        (
            "description-continuation-dash-flag-shape",
            dc.shape_count(),
            dc.shape
                .iter()
                .take(cap)
                .map(|f| {
                    format!(
                        "{:?} opens at the description column of {:?}",
                        f.continuation, f.row
                    )
                })
                .collect(),
        ),
        (
            "description-continuation-dash-flag-value",
            dc.value_count(),
            dc.value
                .iter()
                .take(cap)
                .map(|f| {
                    format!(
                        "{:?} carries a bare quote character as its value",
                        f.spelling
                    )
                })
                .collect(),
        ),
    ]
}
