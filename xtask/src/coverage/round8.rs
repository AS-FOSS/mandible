//! The round-8 family detector, atlas S-140: `fail2ban-client`'s dash-led
//! description continuation, measured as two separate halves (the raw
//! structural shape and the tree artifact it produces). Split into its
//! own file for the same line-count reason `score.rs`'s own round
//! functions are.

use super::score::FAMILY_DETECTOR_SAMPLES_PER_ROW;
use mandible_core::CommandNode;

pub(super) fn round8_family_counts(
    raw: &str,
    root: &CommandNode,
) -> Vec<(&'static str, usize, Vec<String>)> {
    let cap = FAMILY_DETECTOR_SAMPLES_PER_ROW;
    let dc = crate::detector::description_continuation_dash_flag::detect(raw, root);
    vec![
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
