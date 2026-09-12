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
