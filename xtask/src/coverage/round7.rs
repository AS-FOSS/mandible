//! The round-7 family detectors, atlas S-130 to S-136.
//! Split into their own file for the same line-count reason `round8.rs`
//! and `round9.rs` are.

use super::score::FAMILY_DETECTOR_SAMPLES_PER_ROW;
use crate::detector::{Detector, ToolEvidence};
use mandible_core::CommandNode;

/// The round-7 family detectors, atlas S-130 to S-134: `pvdisplay`'s
/// duplicated placeholder, `icupkg`'s two unfixed `or`-joined row shapes,
/// and issue #135's two multi-word bracket-group shapes. Split out for the
/// same line-count reason [`round6_block_family_counts`] is.
pub(super) fn round7_family_counts(
    raw: &str,
    root: &CommandNode,
) -> Vec<(&'static str, usize, Vec<String>)> {
    let cap = FAMILY_DETECTOR_SAMPLES_PER_ROW;
    let evidence = ToolEvidence { raw, root };
    let vd =
        crate::detector::value_name_duplicates_choices::ValueNameDuplicatesChoices.hits(&evidence);
    let cv = crate::detector::choice_value_rows_unfolded::ChoiceValueRowsUnfolded.hits(&evidence);
    let sg = crate::detector::or_joined_alias_single_space_gap::detect(raw, root);
    let bracket: Vec<Box<dyn crate::detector::Detector>> = vec![
        Box::new(
            crate::detector::usage_bracket_group_multiword_value::UsageBracketGroupMultiwordValue,
        ),
        Box::new(
            crate::detector::trailing_bracket_group_multiword_operand::TrailingBracketGroupMultiwordOperand,
        ),
    ];
    let mut out = vec![
        (
            "value-name-duplicates-choices",
            vd.len(),
            vd.into_iter().take(cap).collect(),
        ),
        (
            "choice-value-rows-unfolded",
            cv.len(),
            cv.into_iter().take(cap).collect(),
        ),
        (
            "or-joined-alias-single-space-gap",
            sg.finding_count(),
            sg.findings
                .iter()
                .take(cap)
                .map(|f| format!("{:?}/{:?} never joined, from {:?}", f.short, f.long, f.line))
                .collect(),
        ),
    ];
    out.extend(bracket.iter().map(|d| {
        let hits = d.hits(&evidence);
        (d.name(), hits.len(), hits.into_iter().take(cap).collect())
    }));
    out
}

/// The two round-7 family detectors, atlas S-135 and S-136, split out
/// for the same line-count reason [`round4_family_counts`] is.
pub(super) fn round7_usage_family_counts(
    raw: &str,
    root: &CommandNode,
) -> Vec<(&'static str, usize, Vec<String>)> {
    let cap = FAMILY_DETECTOR_SAMPLES_PER_ROW;
    let uc = crate::detector::usage_text_continuation_fold::detect(raw, root);
    let nv = crate::detector::numbered_variadic_usage_tail::detect(raw, root);
    vec![
        (
            "usage-text-continuation-fold",
            uc.finding_count(),
            uc.findings
                .iter()
                .take(cap)
                .map(|f| format!("{:?} folded into usage {:?}", f.continuation, f.usage))
                .collect(),
        ),
        (
            "numbered-variadic-usage-tail",
            nv.finding_count(),
            nv.findings
                .iter()
                .take(cap)
                .map(|f| {
                    format!(
                        "{:?} never became a positional, from the usage line {:?}",
                        f.positional, f.usage_line
                    )
                })
                .collect(),
        ),
    ]
}
