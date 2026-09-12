//! Round-10 family detectors. Atlas S-162: a leading option-rejection
//! diagnostic line fused into the root description.

use crate::detector::{Detector, ToolEvidence};
use mandible_core::CommandNode;

pub(super) fn round10_family_counts(
    raw: &str,
    root: &CommandNode,
) -> Vec<(&'static str, usize, Vec<String>)> {
    let cap = super::score::FAMILY_DETECTOR_SAMPLES_PER_ROW;
    let evidence = ToolEvidence { raw, root };
    let leading_diagnostic =
        crate::detector::leading_diagnostic_line::LeadingDiagnosticLine.hits(&evidence);
    vec![(
        "leading-diagnostic-line",
        leading_diagnostic.len(),
        leading_diagnostic.into_iter().take(cap).collect(),
    )]
}
