//! `must_draw`: text the detail pane draws on one line of the root page.
//! Holds what the tree cannot say, such as how a value joins its flag.

use super::contract::ContractFailure;
use super::ContractMeta;
use mandible_core::CommandNode;

/// `CONTRACT WEAKENED` lines: an entry dropped from the list.
pub(crate) fn weakened_lines(label: &str, b: &ContractMeta, n: &ContractMeta) -> Vec<String> {
    b.must_draw
        .iter()
        .filter(|text| !n.must_draw.contains(text))
        .map(|text| format!("CONTRACT WEAKENED: {label} must_draw ({text:?} dropped)"))
        .collect()
}

/// Every entry must appear inside one line of the pages the pane draws for
/// the tree, whitespace collapsed on both sides.
pub(crate) fn failures(
    contract: &ContractMeta,
    root: Option<&CommandNode>,
) -> Vec<ContractFailure> {
    if contract.must_draw.is_empty() {
        return Vec::new();
    }
    let Some(root) = root else {
        return vec![ContractFailure("must_draw: no root produced".into())];
    };
    let lines: Vec<String> = super::screen::render_screens(root)
        .lines()
        .map(super::contract::collapse_whitespace)
        .collect();
    contract
        .must_draw
        .iter()
        .filter(|text| {
            let want = super::contract::collapse_whitespace(text);
            !lines.iter().any(|l| l.contains(&want))
        })
        .map(|text| ContractFailure(format!("must_draw: {text:?} is not drawn on any line")))
        .collect()
}
