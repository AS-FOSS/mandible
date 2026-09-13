//! The round-10 family detectors, atlas S-150 upward. Split into its own
//! file for the same line-count reason `round7.rs`, `round8.rs` and
//! `round9.rs` are.

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
    let alt = crate::detector::alternation_value_is_choices::detect(raw, root);
    let tail = crate::detector::description_tail_enumerates_choices::detect(raw, root);
    let sbw =
        crate::detector::spaced_bare_word_table_value::SpacedBareWordTableValue.hits(&evidence);
    let gbar = crate::detector::glued_bracket_angle_run::GluedBracketAngleRun.hits(&evidence);
    let sjab = crate::detector::slash_joined_alias_outside_bullet::SlashJoinedAliasOutsideBullet
        .hits(&evidence);
    let csa = crate::detector::comma_swallowed_alias::CommaSwallowedAlias.hits(&evidence);
    let env_col =
        crate::detector::header_declared_env_column::HeaderDeclaredEnvColumn.hits(&evidence);
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
        (
            "spaced-bare-word-table-value",
            sbw.len(),
            sbw.into_iter().take(cap).collect(),
        ),
        (
            "glued-bracket-angle-run",
            gbar.len(),
            gbar.into_iter().take(cap).collect(),
        ),
        (
            "slash-joined-alias-outside-bullet",
            sjab.len(),
            sjab.into_iter().take(cap).collect(),
        ),
        (
            "comma-swallowed-alias",
            csa.len(),
            csa.into_iter().take(cap).collect(),
        ),
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
        (
            "header-declared-env-column",
            env_col.len(),
            env_col.into_iter().take(cap).collect(),
        ),
    ]
}
