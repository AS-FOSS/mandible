//! A `Usage: <tool> ...` line after the options have started, over a table
//! of flag rows: one more invocation form, not a heading. S-264.

use super::*;

/// The form text when `lines[i]` is a `Usage:` line naming the tool, the
/// usage block above it has closed, and flag rows follow.
pub(super) fn later_usage_form(
    lines: &[&str],
    i: usize,
    tool: &str,
    usage: &[String],
) -> Option<String> {
    let t = lines.get(i)?.trim();
    if usage.is_empty() || !starts_with_usage_prefix(t) {
        return None;
    }
    let after = t.get("usage:".len()..)?.trim_start();
    if !starts_with_tool_name(after, tool) {
        return None;
    }
    let next = lines.get(i + 1)?.trim_start();
    next.starts_with('-').then(|| t.to_string())
}

/// Records the form and returns the index past it.
pub(super) fn take(
    lines: &[&str],
    i: usize,
    tool: Option<&str>,
    result: &mut ParsedHelp,
) -> Option<usize> {
    let form = later_usage_form(lines, i, tool?, &result.usage)?;
    result.usage.push(form);
    Some(i + 1)
}
