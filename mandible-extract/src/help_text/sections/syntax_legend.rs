//! A bare block that is a syntax legend, not a value list: the block under
//! a `<words> syntax:` heading, or a block of `name  -- meaning` rows. The
//! text joins the node description instead of becoming some flag's
//! `choices`. `docs/shapes.md` S-210, `corpus/argdist-bpfcc/0.29.1`.

/// Fewest `name  -- meaning` rows that make a block a legend.
const MIN_LEGEND_ROWS: usize = 3;

fn is_legend_row(row: &str) -> bool {
    row.trim().split_once("  -- ").is_some()
}

/// The paragraph (heading, then the block dedented) when `block` is a legend.
pub(super) fn legend_paragraph(heading: &str, block: &[&str]) -> Option<String> {
    let rows: Vec<&str> = block
        .iter()
        .copied()
        .filter(|l| !l.trim().is_empty())
        .collect();
    if rows.is_empty() {
        return None;
    }
    let label = heading.trim().trim_end_matches(':').to_ascii_lowercase();
    let by_heading = label.ends_with(" syntax");
    let by_rows = rows.iter().filter(|r| is_legend_row(r)).count() >= MIN_LEGEND_ROWS;
    if !by_heading && !by_rows {
        return None;
    }
    let floor = rows
        .iter()
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    let body: Vec<String> = rows
        .iter()
        .map(|l| format!("  {}", l[floor..].trim_end()))
        .collect();
    Some(format!(
        "{}:\n{}",
        heading.trim().trim_end_matches(':'),
        body.join("\n")
    ))
}
