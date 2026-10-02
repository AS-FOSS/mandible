//! docs/shapes.md S-167: a `Usage:` line's leading word is a subcommand
//! spelled with an optional abbreviation suffix (`lldb-server`'s
//! `g[dbserver]`). Distinct from S-020's modifier table (`ar`'s
//! `r[ab][f][u]`, several LETTERS on one command). The node's name and
//! displayed form are the whole word; the row's short prefix is an alias.

use super::heading::{starts_with_tool_name, starts_with_tool_name_spelled_differently};
use mandible_core::{CommandNode, Provenance, Source};

/// `token` read back to the emitted name (docs/design.md §7 Tier B rule 7,
/// §16), moved to `mandible-core` so `mandible-tui` can share it without a
/// real dependency on this crate; re-exported for every existing caller.
pub use mandible_core::reconstruct_abbrev_word;

/// Returns `(whole_word, short_alias)`: the full command word with the
/// brackets removed (`gdbserver`), and the bare leading letter the row
/// spelled as its abbreviation prefix (`g`) — the node's own alias, never
/// its displayed name (docs/design.md §16).
fn optional_abbrev_word(token: &str) -> Option<(String, String)> {
    let whole = reconstruct_abbrev_word(token)?;
    let lead = token.chars().next()?;
    Some((whole, lead.to_string()))
}

/// One recognized row: the tool's own name, a word matched by
/// [`optional_abbrev_word`], then zero or more `[options]`-shaped tokens —
/// anything else refuses the whole row.
fn parse_row(line: &str, tool_name: &str) -> Option<(String, String)> {
    let t = line.trim();
    let is_own_name = starts_with_tool_name(t, tool_name)
        || starts_with_tool_name_spelled_differently(t, tool_name);
    if !is_own_name {
        return None;
    }
    let mut words = t.split_whitespace();
    words.next()?; // the tool's own name, already confirmed above
    let first = words.next()?;
    let (name, alias) = optional_abbrev_word(first)?;
    for trailing in words {
        let inner = trailing
            .strip_prefix('[')
            .and_then(|s| s.strip_suffix(']'))?;
        if inner.is_empty() || !inner.chars().all(|c| c.is_ascii_lowercase()) {
            return None;
        }
    }
    Some((name, alias))
}

/// Fewest recognized rows before this shape is trusted (docs/design.md
/// §16, below AGENTS.md §3.1's five-tool bar, a recorded exception).
const MIN_ROWS: usize = 2;

/// Scan the labelled `Usage:` block at `heading_idx` for this shape.
/// `Some((end, nodes))` only when every line parses as a row up to the
/// first that doesn't, and at least [`MIN_ROWS`] did — refused whole,
/// never partially accepted. `end` is the first line NOT consumed.
pub(super) fn scan_usage_optional_word_table(
    lines: &[&str],
    heading_idx: usize,
    tool_name: &str,
) -> Option<(usize, Vec<CommandNode>)> {
    if !lines[heading_idx].trim().eq_ignore_ascii_case("usage:") {
        return None;
    }
    let mut i = heading_idx + 1;
    let mut rows: Vec<(String, String)> = Vec::new();
    while let Some(&line) = lines.get(i) {
        if line.trim().is_empty() {
            break;
        }
        let Some(row) = parse_row(line, tool_name) else {
            break;
        };
        rows.push(row);
        i += 1;
    }
    if rows.len() < MIN_ROWS {
        return None;
    }
    let nodes = rows
        .into_iter()
        .map(|(name, alias)| {
            let mut node = CommandNode::new(name.clone(), Provenance::single(Source::HelpText));
            // invocation_attested, never heading_attested (§7 rule 8);
            // abbrev_probe_attested admits a probe anyway (§6 rule 0).
            node.invocation_attested = true;
            node.heading_attested = false;
            node.children_filled = false;
            node.abbrev_probe_attested = true;
            if alias != name {
                node.aliases.push(alias);
            }
            node
        })
        .collect();
    Some((i, nodes))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LLDB_SERVER: &str = "Usage:\n  lldb-server v[ersion]\n  lldb-server g[dbserver] [options]\n  lldb-server p[latform] [options]\nInvoke subcommand for additional help\n";

    #[test]
    fn recovers_three_named_nodes_with_the_full_word_as_name_and_the_prefix_as_alias() {
        let lines: Vec<&str> = LLDB_SERVER.lines().collect();
        let (end, nodes) = scan_usage_optional_word_table(&lines, 0, "lldb-server").unwrap();
        assert_eq!(end, 4, "stops at the trailing prose sentence");
        assert_eq!(nodes.len(), 3);
        assert_eq!(nodes[0].name, "version");
        assert_eq!(nodes[0].display_name, None, "no bracketed display form");
        assert_eq!(nodes[0].aliases, vec!["v".to_string()]);
        assert!(nodes[0].invocation_attested);
        assert!(!nodes[0].heading_attested);
        assert!(nodes[0].abbrev_probe_attested);
        assert_eq!(nodes[1].name, "gdbserver");
        assert_eq!(nodes[1].display_name, None);
        assert_eq!(nodes[1].aliases, vec!["g".to_string()]);
        assert!(nodes[1].abbrev_probe_attested);
        assert_eq!(nodes[2].name, "platform");
        assert_eq!(nodes[2].display_name, None);
        assert_eq!(nodes[2].aliases, vec!["p".to_string()]);
        assert!(nodes[2].abbrev_probe_attested);
    }

    #[test]
    fn matches_the_resolved_full_path_spelling_too() {
        let raw = "Usage:\n  /usr/bin/lldb-server v[ersion]\n  /usr/bin/lldb-server g[dbserver] [options]\n  /usr/bin/lldb-server p[latform] [options]\n";
        let lines: Vec<&str> = raw.lines().collect();
        let (_, nodes) = scan_usage_optional_word_table(&lines, 0, "lldb-server").unwrap();
        assert_eq!(nodes.len(), 3);
    }

    #[test]
    fn refuses_ars_modifier_shape_a_second_bracket_group_follows() {
        assert_eq!(optional_abbrev_word("r[ab][f][u]"), None);
    }

    #[test]
    fn accepts_a_single_bracket_group_that_spells_a_whole_word() {
        assert_eq!(
            optional_abbrev_word("v[ersion]"),
            Some(("version".to_string(), "v".to_string()))
        );
    }

    #[test]
    fn refuses_a_single_row_below_the_min_rows_floor() {
        let raw = "Usage:\n  lldb-server v[ersion]\n";
        let lines: Vec<&str> = raw.lines().collect();
        assert!(scan_usage_optional_word_table(&lines, 0, "lldb-server").is_none());
    }

    #[test]
    fn refuses_a_mixed_run_whole_rather_than_partially() {
        let raw = "Usage:\n  lldb-server v[ersion]\n  lldb-server [options] <file>\n";
        let lines: Vec<&str> = raw.lines().collect();
        assert!(scan_usage_optional_word_table(&lines, 0, "lldb-server").is_none());
    }

    #[test]
    fn refuses_a_labelled_usage_line_carrying_its_own_synopsis() {
        let raw = "Usage: foo [OPTIONS]\n  foo v[ersion]\n  foo g[dbserver]\n";
        let lines: Vec<&str> = raw.lines().collect();
        assert!(scan_usage_optional_word_table(&lines, 0, "foo").is_none());
    }
}
