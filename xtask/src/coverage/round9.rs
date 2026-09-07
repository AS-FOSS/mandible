//! The round-9 family detector, atlas S-145. Split into its own file for
//! the same line-count reason `score.rs`'s own round functions are.

use mandible_core::CommandNode;

pub(super) fn round9_family_counts(
    raw: &str,
    root: &CommandNode,
) -> Vec<(&'static str, usize, Vec<String>)> {
    let cap = super::score::FAMILY_DETECTOR_SAMPLES_PER_ROW;
    let sd = crate::detector::single_dash_long_table::detect(raw, root);
    vec![(
        "single-dash-long-table",
        sd.finding_count(),
        sd.findings
            .iter()
            .take(cap)
            .map(|f| {
                format!(
                    "-{} never became its own spelling, from {:?}",
                    f.name, f.line
                )
            })
            .collect(),
    )]
}
