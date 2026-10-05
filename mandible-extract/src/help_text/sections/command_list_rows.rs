//! A run of `<tool> <word> ...` rows, each followed by an indented prose
//! description, is a command list, never the usage form. `docs/shapes.md`
//! S-211, `corpus/corepack/0.34.6`.

use super::*;

/// Fewest rows that make a run a command list.
const MIN_ROWS: usize = 3;

fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// True when `lines[at]` is a row that names the tool and is followed by an
/// indented description that is prose, not a usage continuation.
fn is_described_row(lines: &[&str], at: usize, name: &str) -> bool {
    let row = lines[at];
    let Some(next) = lines.get(at + 1) else {
        return false;
    };
    let text = next.trim_start();
    starts_with_tool_name(row.trim_start(), name)
        && indent(next) > indent(row)
        && text.starts_with(|c: char| c.is_alphabetic())
}

/// True when `lines[idx]` belongs to a run of at least [`MIN_ROWS`] such
/// rows, the rows separated by blank lines.
pub(super) fn is_command_list_row(lines: &[&str], idx: usize, name: &str) -> bool {
    if !is_described_row(lines, idx, name) {
        return false;
    }
    let mut first = idx;
    while first >= 3 && lines[first - 1].trim().is_empty() && is_described_row(lines, first - 3, name)
    {
        first -= 3;
    }
    let mut rows = 0;
    let mut at = first;
    while at < lines.len() && is_described_row(lines, at, name) {
        rows += 1;
        at += 2;
        if lines.get(at).is_some_and(|l| l.trim().is_empty()) {
            at += 1;
        }
    }
    rows >= MIN_ROWS
}

/// `t` without a leading shell prompt (`$ corepack <command>`).
pub(super) fn without_prompt(t: &str) -> &str {
    t.strip_prefix("$ ").map_or(t, str::trim_start)
}

/// The invocation table that a flush commands heading at `lines[at]`
/// introduces directly (`General commands` over `corepack cache clean`
/// rows): the index after the table, its nodes and the entry counts.
pub(super) fn table_under_commands_heading(
    lines: &[&str],
    at: usize,
    name: &str,
    raw: &str,
    profile: Option<&FrameworkProfile>,
) -> Option<(usize, Vec<CommandNode>, usize, usize)> {
    let heading = lines[at].trim();
    if indent(lines[at]) != 0 || !is_recognized_command_heading(heading, profile) {
        return None;
    }
    let first = (at + 1..lines.len()).find(|&j| !lines[j].trim().is_empty())?;
    if !starts_with_tool_name(lines[first].trim_start(), name) {
        return None;
    }
    let (end, mut nodes, seen, clean) = scan_headingless_invocation_table(lines, first, name, raw)?;
    if heading_can_name_a_group(heading) {
        for node in &mut nodes {
            node.group = Some(heading.to_string());
        }
    }
    Some((end, nodes, seen, clean))
}
