//! A table of invocation blocks whose first block must not be read as the
//! tool's usage: `jfr --help` prints `jfr print [--xml|--json] ...`, a wrapped
//! synopsis, then an indented description, and the next blocks repeat that
//! layout for `configure`, `metadata`, `summary` ... (`docs/shapes.md`
//! S-239). The first block is one more command block of the table
//! [`super::scan::scan_headingless_invocation_table`] reads, never the root
//! usage. Split out of `usage.rs` to keep that file under its size ceiling.

use super::*;

/// Fewest blocks (the candidate plus its siblings) that make a table.
const MIN_BLOCKS: usize = 2;

/// True for a synopsis continuation line: the wrapped rest of the invocation
/// (`[--events <filter>]`, `<file>`, `[option=value]*`), not description prose.
pub(super) fn is_synopsis_continuation(trimmed: &str) -> bool {
    trimmed.starts_with('[') || trimmed.starts_with('<')
}

/// Where the block led by `lines[idx]` ends (first blank line or first line
/// not indented deeper than the lead), and whether its last line is a
/// description: prose that is not a synopsis continuation.
fn block_extent(lines: &[&str], idx: usize) -> (usize, bool) {
    let indent = leading_whitespace(lines[idx]);
    let mut end = idx + 1;
    while end < lines.len()
        && !lines[end].trim().is_empty()
        && leading_whitespace(lines[end]) > indent
    {
        end += 1;
    }
    let described = end > idx + 1 && !is_synopsis_continuation(lines[end - 1].trim());
    (end, described)
}

/// The command word of a block lead `<tool> <word> ...`: lowercase, name
/// shaped, never a flag, bracket or placeholder.
fn lead_word<'a>(line: &'a str, tool_name: &str) -> Option<&'a str> {
    let rest = line.trim_start().strip_prefix(tool_name)?;
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let word = rest.split_whitespace().next()?;
    (word.starts_with(|c: char| c.is_ascii_lowercase()) && is_command_name_shaped(word))
        .then_some(word)
}

/// True when `lines[idx]` leads a described invocation block that belongs to
/// a run of at least [`MIN_BLOCKS`] such blocks at the same indent, each led
/// by the tool's name and a different command word, blank-line separated. Such
/// a block is a row of a command table, not a usage line.
pub(super) fn heads_command_block_table(lines: &[&str], idx: usize, tool_name: &str) -> bool {
    if lead_word(lines[idx], tool_name).is_none() {
        return false;
    }
    let indent = leading_whitespace(lines[idx]);
    // The run may start above `idx`: walk back over the blocks before it.
    let mut start = idx;
    for k in (0..idx).rev() {
        let line = lines[k];
        if line.trim().is_empty() || leading_whitespace(line) > indent {
            continue;
        }
        if leading_whitespace(line) == indent && lead_word(line, tool_name).is_some() {
            start = k;
        } else {
            break;
        }
    }
    let mut seen: Vec<&str> = Vec::new();
    let mut blocks = 0;
    let mut at = start;
    let mut holds_idx = false;
    while at < lines.len() {
        while at < lines.len() && lines[at].trim().is_empty() {
            at += 1;
        }
        if at >= lines.len() || leading_whitespace(lines[at]) != indent {
            break;
        }
        let Some(word) = lead_word(lines[at], tool_name) else {
            break;
        };
        let (next, described) = block_extent(lines, at);
        if !described || seen.contains(&word) {
            break;
        }
        holds_idx |= (at..next).contains(&idx);
        seen.push(word);
        blocks += 1;
        at = next;
    }
    holds_idx && blocks >= MIN_BLOCKS
}

/// Where the table of invocation blocks starts when the document has no usage
/// block: the first non-blank line at or after the leading prose, if it heads
/// such a table. S-239.
pub(super) fn block_table_start(
    lines: &[&str],
    usage_start: Option<usize>,
    from: usize,
    tool_name: Option<&str>,
) -> Option<usize> {
    if usage_start.is_some() {
        return None;
    }
    let name = tool_name?;
    let at = (from..lines.len()).find(|&j| !lines[j].trim().is_empty())?;
    heads_command_block_table(lines, at, name).then_some(at)
}

#[cfg(test)]
mod tests {
    use super::*;

    const JFR: &str = "Tool\n\n jfr print [--xml|--json]\n           <file>\n     Print contents.\n\n jfr summary <file>\n     Display general information.\n";

    #[test]
    fn the_first_block_of_a_described_block_table_is_not_usage() {
        let lines: Vec<&str> = JFR.lines().collect();
        assert!(heads_command_block_table(&lines, 2, "jfr"));
        assert!(heads_command_block_table(&lines, 6, "jfr"));
    }

    #[test]
    fn a_lone_synopsis_stays_usage() {
        let raw = "vgck\n      [ -d|--debug ]\n      [ -h|--help ]\n";
        let lines: Vec<&str> = raw.lines().collect();
        assert!(!heads_command_block_table(&lines, 0, "vgck"));
        let raw = " jfr print [--xml]\n     Print.\n\n jfr print <file>\n     Again.\n";
        let lines: Vec<&str> = raw.lines().collect();
        assert!(!heads_command_block_table(&lines, 0, "jfr"));
    }
}
