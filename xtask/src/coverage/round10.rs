//! The round-10 family detectors, atlas S-150 and S-151. S-150 is a bare
//! usage label seeding an empty form. S-151 is a usage form's own foreign
//! leading program word, the case S-108's `usage-program-word-mismatch`
//! does not already handle. Split into their own file for the same
//! line-count reason `round7.rs`, `round8.rs` and `round9.rs` are.

use super::score::FAMILY_DETECTOR_SAMPLES_PER_ROW;
use crate::detector::{Detector, ToolEvidence};
use mandible_core::CommandNode;

pub(super) fn round10_family_counts(
    raw: &str,
    root: &CommandNode,
) -> Vec<(&'static str, usize, Vec<String>)> {
    let cap = FAMILY_DETECTOR_SAMPLES_PER_ROW;
    let evidence = ToolEvidence { raw, root };
    let bare = crate::detector::bare_usage_label_form::BareUsageLabelForm.hits(&evidence);
    let foreign = crate::detector::usage_foreign_program_word::detect(raw, root);
    vec![
        (
            "bare-usage-label-form",
            bare.len(),
            bare.into_iter().take(cap).collect(),
        ),
        (
            "usage-foreign-program-word",
            foreign.finding_count(),
            foreign
                .findings
                .iter()
                .take(cap)
                .map(|f| {
                    format!(
                        "{:?} never became the node's own name, from {:?}",
                        f.token, f.line
                    )
                })
                .collect(),
        ),
    ]
}
