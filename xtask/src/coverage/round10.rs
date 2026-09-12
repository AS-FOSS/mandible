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
    let gbar = crate::detector::glued_bracket_angle_run::GluedBracketAngleRun.hits(&evidence);
    let sjab = crate::detector::slash_joined_alias_outside_bullet::SlashJoinedAliasOutsideBullet
        .hits(&evidence);
    let csa = crate::detector::comma_swallowed_alias::CommaSwallowedAlias.hits(&evidence);
    vec![
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
    ]
}
