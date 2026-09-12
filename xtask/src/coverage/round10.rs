//! Round-10 family detectors. Atlas S-162: a leading option-rejection
//! diagnostic line fused into the root description. S-163: a `+word`
//! option row, and a `+/-word`/`[+-]word` alternation-sigil row. S-164: a
//! root flag group that repeats the root description verbatim. S-165: a
//! headingless option table duplicated into the root description.

//! The round-10 family detectors. `header-declared-env-column` is atlas
//! S-166, W6's own header-declared three-column option table. Split into
//! its own file for the same line-count reason `round7.rs`, `round8.rs`
//! and `round9.rs` are.

use super::score::FAMILY_DETECTOR_SAMPLES_PER_ROW;
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
    let plus_word = crate::detector::plus_word_option::PlusWordOption.hits(&evidence);
    let plus_minus_alternation =
        crate::detector::plus_minus_alternation_option::PlusMinusAlternationOption.hits(&evidence);
    let description_reused_as_group =
        crate::detector::description_reused_as_group_label::DescriptionReusedAsGroupLabel
            .hits(&evidence);
    let headingless_table_in_description =
        crate::detector::headingless_table_in_root_description::HeadinglessTableInRootDescription
            .hits(&evidence);
    vec![
        (
            "leading-diagnostic-line",
            leading_diagnostic.len(),
            leading_diagnostic.into_iter().take(cap).collect(),
        ),
        (
            "plus-word-option",
            plus_word.len(),
            plus_word.into_iter().take(cap).collect(),
        ),
        (
            "plus-minus-alternation-option",
            plus_minus_alternation.len(),
            plus_minus_alternation.into_iter().take(cap).collect(),
        ),
        (
            "description-reused-as-group-label",
            description_reused_as_group.len(),
            description_reused_as_group.into_iter().take(cap).collect(),
        ),
        (
            "headingless-table-in-root-description",
            headingless_table_in_description.len(),
            headingless_table_in_description
                .into_iter()
                .take(cap)
                .collect(),
        ),
    ]
    let cap = FAMILY_DETECTOR_SAMPLES_PER_ROW;
    let evidence = ToolEvidence { raw, root };
    let env_col =
        crate::detector::header_declared_env_column::HeaderDeclaredEnvColumn.hits(&evidence);
    vec![(
        "header-declared-env-column",
        env_col.len(),
        env_col.into_iter().take(cap).collect(),
    )]
}
