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
    while first >= 3
        && lines[first - 1].trim().is_empty()
        && is_described_row(lines, first - 3, name)
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
    attach_row_invocations(&mut nodes, &lines[first..end], name);
    if heading_can_name_a_group(heading) {
        for node in &mut nodes {
            node.group = Some(heading.to_string());
        }
    }
    Some((end, nodes, seen, clean))
}

/// Give each node the whole invocation line of every row that named it as
/// its usage form, and a description a second row of the same node adds
/// (`corepack install` over "Install the package manager..." and then
/// `corepack install <-g,--global> ...` over "Install package managers on
/// the system") as the node's description after the first row's summary.
fn attach_row_invocations(nodes: &mut [CommandNode], rows: &[&str], name: &str) {
    let base = indent(rows[0]);
    let mut at = 0;
    while at < rows.len() {
        let line = rows[at];
        at += 1;
        let trimmed = line.trim_start();
        if indent(line) != base || !starts_with_tool_name(trimmed, name) {
            continue;
        }
        let Some(run) = invocation_table_row_run(trimmed, name) else {
            continue;
        };
        let described = (at..rows.len())
            .take_while(|&j| !rows[j].trim().is_empty() && indent(rows[j]) > base)
            .count();
        let desc = rows[at..at + described]
            .iter()
            .map(|l| l.trim())
            .collect::<Vec<_>>()
            .join(" ");
        at += described;
        let Some(mut node) = nodes.iter_mut().find(|n| n.name == run[0]) else {
            continue;
        };
        if let Some(child) = run.get(1) {
            let Some(inner) = node.subcommands.iter_mut().find(|n| n.name == *child) else {
                continue;
            };
            node = inner;
        }
        let text = Text::sanitize(trimmed);
        if !node.usage.contains(&text) {
            node.usage.push(text);
        }
        let same = node.summary.as_ref().is_some_and(|s| s.as_str() == desc);
        if !desc.is_empty() && node.summary.is_some() && !same {
            node.description = Some(match node.description.take() {
                Some(d) => Text::sanitize(&format!("{}\n{desc}", d.as_str())),
                None => Text::sanitize(&desc),
            });
        }
    }
}

/// The index after the region at `at` that is no heading: a hard-wrapped
/// prose sentence, a flush example command that continues onto deeper
/// lines, or the invocation table under a flush commands heading.
pub(super) fn region_end(
    inp: &BodyInput,
    tool_name: Option<&str>,
    at: usize,
    st: &mut BodyScan,
) -> Option<usize> {
    if let Some(end) = wrapped_prose_region_end(inp.lines, at) {
        return Some(end);
    }
    if let Some(end) = super::example_command_line::example_command_end(inp.lines, at) {
        return Some(end);
    }
    let name = tool_name?;
    let (end, nodes, seen, clean) =
        table_under_commands_heading(inp.lines, at, name, inp.raw, inp.profile)?;
    st.total_entries += seen;
    st.clean_entries += clean;
    for node in nodes {
        st.result.try_push_subcommand(node);
    }
    st.command_mode = false;
    Some(end)
}
