//! A command table sitting directly under the tool's own root `Usage:`
//! synopsis, one line per command, each carrying its own flags/operands —
//! `dmsetup --help`'s second block (`docs/shapes.md` S-169). Distinct from
//! [`super::scan::scan_headingless_invocation_table`] (S-016): that shape's
//! rows repeat the tool's own name (`btrfs balance start ...`); this one's
//! rows are the bare command word alone (`create <dev_name>`), so the
//! evidence that they name real commands is their position — directly
//! after the root's own usage block, never introduced by any heading —
//! rather than a repeated name. Split out of `usage.rs` to keep that file
//! under its own size ceiling (AGENTS.md §2).

use super::*;

/// Fewest command rows required before a run of lines is read as a real
/// per-command table rather than one stray line. Mirrors
/// [`super::scan::MIN_INVOCATION_TABLE_ROWS`]'s own reasoning. See
/// docs/shapes.md S-169.
const MIN_USAGE_COMMAND_TABLE_ROWS: usize = 2;

/// Recognize the **headingless usage command table**: a run of rows
/// directly beneath the tool's own root `Usage:` block (already scanned
/// into `usage_lines`, `lines[start]` is the first line after it), each
/// opening with a bare, `is_command_name_shaped` word that is never the
/// tool's own name, and carrying its own flags/operands on that line plus
/// any deeper-indented continuation. A repeated command word
/// (`dmsetup`'s `create <dev_name>` / `create --concise ...`) is a second
/// invocation form of the same command, folded into that command's own
/// entities rather than starting a new node. Every emitted node is
/// `invocation_attested: true`, `heading_attested: false` — a usage block
/// is not a heading, and spec §6 rule 0's second gate must keep declining
/// to probe these words. See docs/shapes.md S-169.
pub(super) fn scan_headingless_usage_command_table(
    lines: &[&str],
    start: usize,
    tool_name: Option<&str>,
    raw: &str,
) -> Option<(usize, Vec<CommandNode>)> {
    let mut i = start;
    while i < lines.len() && lines[i].trim().is_empty() {
        i += 1;
    }
    if i >= lines.len() {
        return None;
    }
    let table_indent = leading_whitespace(lines[i]);
    // A flush-left line here is prose or a heading, never this shape —
    // the table always sits under the root's own indented flags block.
    if table_indent == 0 {
        return None;
    }

    let mut nodes: Vec<CommandNode> = Vec::new();
    let mut index_by_name: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    let mut node_lines: Vec<Vec<String>> = Vec::new();
    let mut current: Option<usize> = None;
    let mut rows_seen = 0usize;

    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() {
            break;
        }
        let indent = leading_whitespace(line);
        if indent < table_indent {
            break;
        }
        let trimmed = line.trim();
        if indent == table_indent {
            let word_end = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
            let word = &trimmed[..word_end];
            if !is_command_name_shaped(word) || !token_occurs_literally(raw, word) {
                break;
            }
            if tool_name.is_some_and(|name| word == name) {
                // This is S-016's own shape instead; defer to it entirely.
                return None;
            }
            if nodes.len() >= MAX_RECOVERED_ENTRIES {
                break;
            }
            rows_seen += 1;
            let idx = *index_by_name.entry(word.to_string()).or_insert_with(|| {
                let mut node = CommandNode::new(word, Provenance::single(Source::HelpText));
                node.invocation_attested = true;
                node.heading_attested = false;
                node.children_filled = true;
                nodes.push(node);
                node_lines.push(Vec::new());
                nodes.len() - 1
            });
            current = Some(idx);
            node_lines[idx].push(trimmed.to_string());
        } else {
            // A continuation line, deeper than the table's own row indent:
            // belongs to whichever row is currently open.
            let Some(idx) = current else { break };
            node_lines[idx].push(trimmed.to_string());
        }
        i += 1;
    }

    if rows_seen < MIN_USAGE_COMMAND_TABLE_ROWS || nodes.is_empty() {
        return None;
    }

    for (idx, node) in nodes.iter_mut().enumerate() {
        let own_lines = &node_lines[idx];
        let mut flags: Vec<Entity> = Vec::new();
        for flag in extract_usage_flags(own_lines) {
            if !flag_spelling_already_present(&flag, &flags) {
                flags.push(flag);
            }
        }
        let positionals = extract_positionals(own_lines, std::collections::HashSet::new());
        node.entities.extend(flags);
        node.entities.extend(positionals);
    }

    Some((i, nodes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find_subcommand<'a>(nodes: &'a [CommandNode], name: &str) -> &'a CommandNode {
        nodes
            .iter()
            .find(|n| n.name == name)
            .unwrap_or_else(|| panic!("{name} missing from {nodes:?}"))
    }

    /// The dmsetup shape itself: a root usage block, a blank line, then a
    /// tab-indented command table whose rows never repeat `dmsetup`'s own
    /// name. See docs/shapes.md S-169.
    #[test]
    fn recovers_commands_never_naming_the_tool_with_their_own_flags() {
        let raw = "Usage:\n\
                    \n\
                    dmsetup\n        \
                    [--version] [-h|--help [-c|-C|--columns]]\n        \
                    [-v|--verbose [-v|--verbose ...]] [-f|--force]\n\
                    \n\
                    \thelp [-c|-C|--columns]\n\
                    \tcreate <dev_name>\n\
                    \t    [-j|--major <major> -m|--minor <minor>]\n\
                    \tremove [--deferred] [-f|--force] [--retry] <device>...\n\
                    \tremove_all [-f|--force]\n";
        let parsed = parse_named(raw, "dmsetup");

        assert!(
            parsed.flags.iter().all(|f| f.long() != Some("major")),
            "create's own -j/--major must not leak onto the root: {:?}",
            parsed.flags
        );

        let create = find_subcommand(&parsed.subcommands, "create");
        assert!(create.invocation_attested);
        assert!(!create.heading_attested, "a usage block is not a heading");
        let major = create
            .flags()
            .find(|f| f.long() == Some("major"))
            .expect("create's own -j/--major recovered");
        assert_eq!(major.short(), Some('j'));

        let remove = find_subcommand(&parsed.subcommands, "remove");
        assert!(remove.flags().any(|f| f.long() == Some("force")));
        assert!(remove.invocation_attested && !remove.heading_attested);

        assert!(find_subcommand(&parsed.subcommands, "remove_all").invocation_attested);
    }

    /// `create <dev_name>` / `create --concise ...` are two invocation
    /// forms of the one command `create`, never two nodes.
    #[test]
    fn a_repeated_command_word_folds_into_one_node() {
        let raw = "Usage:\n\
                    \n\
                    dmsetup\n        \
                    [--version]\n\
                    \n\
                    \tcreate <dev_name>\n\
                    \t    [-j|--major <major> -m|--minor <minor>]\n\
                    \tcreate --concise [<concise_device_spec_list>]\n\
                    \tremove [--deferred] [-f|--force] <device>...\n";
        let parsed = parse_named(raw, "dmsetup");
        let creates: Vec<_> = parsed
            .subcommands
            .iter()
            .filter(|n| n.name == "create")
            .collect();
        assert_eq!(creates.len(), 1, "must fold to one node: {:?}", creates);
        assert!(creates[0].flags().any(|f| f.long() == Some("concise")));
        assert!(creates[0].flags().any(|f| f.long() == Some("major")));
    }

    /// A table whose rows repeat the tool's own name is S-016's shape, not
    /// this one — refuse outright so the caller can try that recognizer.
    #[test]
    fn refuses_when_rows_repeat_the_tools_own_name() {
        let raw = "Usage:\n\
                    \n\
                    btrfs\n        \
                    [--version]\n\
                    \n\
                    \tbtrfs balance start <path>\n\
                    \tbtrfs balance pause <path>\n";
        let lines: Vec<&str> = raw.lines().collect();
        // Lines 0..=3 are the usage block; the table starts at the blank
        // line, index 4.
        assert!(scan_headingless_usage_command_table(&lines, 4, Some("btrfs"), raw).is_none());
    }

    /// One row alone, below the floor, must not be promoted.
    #[test]
    fn refuses_a_single_row() {
        let raw = "Usage:\n\
                    \n\
                    dmsetup\n        \
                    [--version]\n\
                    \n\
                    \thelp [-c|-C|--columns]\n";
        let lines: Vec<&str> = raw.lines().collect();
        assert!(scan_headingless_usage_command_table(&lines, 4, Some("dmsetup"), raw).is_none());
    }
}
