//! A headed block of undescribed invocation lines, each the tool's own name
//! plus a lowercase word (`brew --help`'s `Example usage:`,
//! `Troubleshooting:`, `Contributing:`, `Further help:`;
//! `docs/shapes.md` S-179). Distinct from
//! [`super::scan::scan_headingless_invocation_table`] (S-016), which needs a
//! description row under each name, and from S-071's `Examples:` fence,
//! whose worked commands are not subcommand names.
//!
//! Names only. A flag on such a row is not attached to its command: the
//! row spells the flag beside operands (`install --verbose --debug
//! FORMULA|CASK`) and the usage-flag reader would hand the operand to the
//! last flag as its value, which the help never says.

use super::*;

/// Fewest distinct command words one block must name. A single line is
/// indistinguishable from a worked example.
const MIN_BLOCK_COMMANDS: usize = 2;

/// True for a lowercase command word: `search`, `self-update`, `foo_bar`.
fn is_lowercase_word(word: &str) -> bool {
    let mut chars = word.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_'))
}

/// A line that points at documentation instead of invoking the tool: a bare
/// URL, or `man <tool>`. Allowed in the block, never a command.
fn is_reference_line(trimmed: &str, tool_name: &str) -> bool {
    let mut words = trimmed.split_whitespace();
    let first = words.next().unwrap_or("");
    if words.next().is_none() {
        return first.starts_with("http://") || first.starts_with("https://");
    }
    first == "man" && trimmed.split_whitespace().any(|w| w == tool_name)
}

/// A heading that is exactly `Example`/`Examples` stays S-071's fenced
/// region; so do usage-synopsis headings, which the usage scanner owns.
fn heading_is_excluded(heading: &str) -> bool {
    let h = heading.trim().trim_end_matches(':').trim().to_lowercase();
    matches!(h.as_str(), "example" | "examples")
        || starts_with_usage_prefix(&h)
        || h.starts_with("synopsis")
        || h.starts_with("syntax")
}

/// Push the subcommands of the block under the heading at `at` onto the
/// result and return the first line after it. S-179.
pub(super) fn recover_invocation_block(
    lines: &[&str],
    at: usize,
    tool_name: Option<&str>,
    raw: &str,
    st: &mut BodyScan,
) -> Option<usize> {
    let (end, nodes) = scan_invocation_block(lines, at, tool_name?, raw)?;
    for node in nodes {
        st.result.try_push_subcommand(node);
    }
    st.command_mode = false;
    Some(end)
}

/// Scan the block under the heading at `heading_idx`. `Some((end, nodes))`
/// only when every non-blank line of the block is an invocation row or a
/// reference line, at least [`MIN_BLOCK_COMMANDS`] distinct words are named,
/// and the block is indented under its heading. Nodes are
/// `invocation_attested`, never `heading_attested`: they are listed, never
/// probed (docs/design.md section 6).
pub(super) fn scan_invocation_block(
    lines: &[&str],
    heading_idx: usize,
    tool_name: &str,
    raw: &str,
) -> Option<(usize, Vec<CommandNode>)> {
    let heading = lines[heading_idx].trim();
    if !heading.ends_with(':') || heading_is_excluded(heading) {
        return None;
    }
    let heading_indent = leading_whitespace(lines[heading_idx]);
    let mut i = heading_idx + 1;
    let mut nodes: Vec<CommandNode> = Vec::new();
    while i < lines.len() && !lines[i].trim().is_empty() {
        let line = lines[i];
        if leading_whitespace(line) <= heading_indent {
            return None;
        }
        let trimmed = line.trim();
        if !is_reference_line(trimmed, tool_name) {
            if !starts_with_tool_name(trimmed, tool_name) {
                return None;
            }
            let word = trimmed
                .strip_prefix(tool_name)?
                .split_whitespace()
                .next()
                .filter(|w| is_lowercase_word(w) && token_occurs_literally(raw, w))?;
            if nodes.iter().all(|n| n.name != word) {
                if nodes.len() >= MAX_RECOVERED_ENTRIES {
                    return None;
                }
                let mut node = CommandNode::new(word, Provenance::single(Source::HelpText));
                node.invocation_attested = true;
                node.heading_attested = false;
                nodes.push(node);
            }
        }
        i += 1;
    }
    if nodes.len() < MIN_BLOCK_COMMANDS {
        return None;
    }
    Some((i, nodes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(raw: &str) -> Option<Vec<String>> {
        let lines: Vec<&str> = raw.lines().collect();
        scan_invocation_block(&lines, 0, "brew", raw)
            .map(|(_, nodes)| nodes.into_iter().map(|n| n.name).collect())
    }

    #[test]
    fn names_the_words_folds_repeats_and_skips_reference_lines() {
        let raw = "Further help:\n  brew commands\n  brew help [COMMAND]\n  brew help\n  man brew\n  https://docs.brew.sh\n";
        assert_eq!(scan(raw).unwrap(), ["commands", "help"]);
    }

    #[test]
    fn a_plain_examples_heading_stays_fenced() {
        assert!(scan("Examples:\n  brew install foo\n  brew list\n").is_none());
    }

    #[test]
    fn one_line_that_is_not_an_invocation_refuses_the_block() {
        assert!(scan("Troubleshooting:\n  brew config\n  see the docs\n  brew doctor\n").is_none());
        assert!(scan("Contributing:\n  brew -v\n  brew doctor\n").is_none());
    }

    #[test]
    fn a_single_word_is_not_enough() {
        assert!(scan("Contributing:\n  brew edit\n  brew edit FORMULA\n").is_none());
    }
}
