//! A program-name line that opens the document and names a command slot,
//! ahead of a bare `Usage:` label, is the first usage form, not prose
//! (`kernel-install [OPTIONS...] COMMAND ...`). `docs/shapes.md` S-283.

use super::*;

/// The index of the opening synopsis line when `labelled_usage_start` is a
/// bare `Usage:` label and the first non-blank line above it is a
/// column-0 synopsis of the tool.
pub(super) fn line(
    lines: &[&str],
    labelled_usage_start: Option<usize>,
    tool_name: Option<&str>,
) -> Option<usize> {
    let label = labelled_usage_start?;
    if !lines[label].trim().eq_ignore_ascii_case("usage:") {
        return None;
    }
    let first = lines.iter().position(|l| !l.trim().is_empty())?;
    let t = lines[first].trim_end();
    (first < label
        && leading_whitespace(lines[first]) == 0
        && looks_like_unlabeled_synopsis_line(t, tool_name?)
        && t.split_whitespace().any(|w| w == "COMMAND"))
    .then_some(first)
}
