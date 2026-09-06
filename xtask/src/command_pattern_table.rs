//! The `command-pattern-table` detector (atlas S-141): a command table's
//! rows are command *patterns*, not bare names — a leading word plus
//! literal words, flag groups and `<PLACEHOLDER>`s (`fail2ban-client`'s
//! `restart [--unban] [--if-exists] <JAIL>`). A parser reading only the
//! leading word drops the rest of the field outright. Distinct from
//! `command_row_argument_placeholder` (S-129), whose grammar requires an
//! ALL-CAPS-only tail; this shape mixes literal lowercase words with
//! placeholders, under a block that may carry centered ALL-CAPS group
//! labels (`BASIC`, `JAIL CONTROL`) that must not read as rows.
//! Fixture: `corpus/fail2ban-client/1.0.2/`. Not a labelled seed-2
//! member — `self_checks` carries the evidentiary weight (design.md
//! §13.1e rule 6).

use mandible_core::{is_command_name_shaped, CommandNode};

/// A line whose trimmed content is nothing but spaced ASCII-uppercase
/// words is a centered group label (`BASIC`, `JAIL CONTROL`), never a row.
fn is_group_label(trimmed: &str) -> bool {
    !trimmed.is_empty()
        && trimmed.chars().count() <= 40
        && trimmed
            .split_whitespace()
            .all(|w| w.chars().all(|c| c.is_ascii_uppercase()))
}

fn leading_whitespace(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// The byte offset of the first run of 2+ spaces in `line`, after some
/// non-whitespace content — this detector's own column-gap finder,
/// independent of the parser's `find_multi_space_gap`.
fn find_gap(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut run = 0usize;
    let mut seen_content = false;
    for (i, b) in bytes.iter().enumerate() {
        if *b == b' ' {
            run += 1;
            if run >= 2 && seen_content {
                return Some(i - run + 1);
            }
        } else {
            if *b != b'\t' {
                seen_content = true;
            }
            run = 0;
        }
    }
    None
}

/// True if `s` mentions "command(s)" as a whole word — the vocabulary a
/// command-table heading is gated on (spec §7 Tier B rule 1). Kept as its
/// own copy, the same convention `command_row_argument_placeholder` uses.
fn mentions_command_word(s: &str) -> bool {
    s.split(|c: char| !c.is_alphanumeric())
        .any(|w| matches!(w.to_lowercase().as_str(), "command" | "commands"))
}

/// A short, colon-terminated, plain-word label naming a command block.
fn is_command_heading(line: &str) -> bool {
    let trimmed = line.trim();
    let Some(label) = trimmed.strip_suffix(':') else {
        return false;
    };
    if label.is_empty() || label.chars().count() > 60 {
        return false;
    }
    if !label
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == ' ' || c == '-' || c == '_')
    {
        return false;
    }
    mentions_command_word(label)
}

/// The row's own name field: `Some(field)` when `line`'s content — either
/// the text before a description-column gap, or the whole trimmed line
/// when it carries no gap of its own (the description wraps entirely onto
/// later, deeper-indented lines) — is two or more whitespace-separated
/// words led by a command-name-shaped token.
fn row_pattern_field(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() || is_group_label(trimmed) {
        return None;
    }
    let field = match find_gap(line) {
        Some(gap) => {
            let desc = line.get(gap..)?.trim();
            if desc.is_empty() || find_gap(desc).is_some() {
                return None;
            }
            line.get(..gap)?.trim()
        }
        None => trimmed,
    };
    let mut words = field.split_whitespace();
    let first = words.next()?;
    if !is_command_name_shaped(first) || words.next().is_none() {
        return None;
    }
    Some(field.to_string())
}

/// One row this detector's own grammar recognizes as a command pattern,
/// whose exact pattern is not attested anywhere in the parsed tree.
pub struct Finding {
    pub name: String,
    pub pattern: String,
    pub row: String,
}

pub struct Report {
    pub findings: Vec<Finding>,
}

impl Report {
    pub fn finding_count(&self) -> usize {
        self.findings.len()
    }
}

/// Whether `pattern` (led by `name`) is already carried by the tree, under
/// any of three shapes: a node's own `display_name` matched verbatim
/// against the row's whole field, a `usage` line matched verbatim against
/// the whole field, or a `usage` line matched against the field's own
/// tail after `name` — the shape `emit_subcommands`
/// (`command_name_with_operand_placeholders`, S-129) actually stores: the
/// operand text alone, with the leading name carried by the node's own
/// `name` rather than repeated inside `usage`.
fn tree_attests(node: &CommandNode, name: &str, pattern: &str) -> bool {
    let tail = pattern.strip_prefix(name).map(str::trim_start);
    node.subcommands.iter().any(|c| {
        (c.name == name
            && (c.display_name.as_deref() == Some(pattern)
                || c.usage
                    .iter()
                    .any(|u| u.as_str() == pattern || Some(u.as_str()) == tail)))
            || tree_attests(c, name, pattern)
    })
}

/// One command block: a run of rows under a recognized command heading,
/// possibly interrupted by centered ALL-CAPS group labels, gated at the
/// row indent the block's own gapped rows establish. Skips a block whose
/// rows never establish a row indent (every row wraps its description with
/// no same-line gap at all) — a degraded miss, not a false alarm.
fn scan_block(
    lines: &[&str],
    start: usize,
    heading_indent: usize,
) -> (usize, Vec<(usize, String)>) {
    let mut end = start;
    while end < lines.len() {
        let line = lines[end];
        if !line.trim().is_empty() && leading_whitespace(line) <= heading_indent {
            break;
        }
        end += 1;
    }
    let block = &lines[start..end];
    let row_indent = block
        .iter()
        .filter(|l| find_gap(l).is_some() && row_pattern_field(l).is_some())
        .map(|l| leading_whitespace(l))
        .min();
    let Some(row_indent) = row_indent else {
        return (end, Vec::new());
    };
    let mut rows = Vec::new();
    for (offset, line) in block.iter().enumerate() {
        if line.trim().is_empty() || leading_whitespace(line) != row_indent {
            continue;
        }
        if let Some(field) = row_pattern_field(line) {
            rows.push((start + offset, field));
        }
    }
    (end, rows)
}

/// Every command-pattern row under a recognized command heading, whose
/// exact pattern the tree does not attest.
pub fn detect(raw: &str, root: &CommandNode) -> Report {
    let lines: Vec<&str> = raw.lines().collect();
    let mut findings = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if !is_command_heading(lines[i]) {
            i += 1;
            continue;
        }
        let heading_indent = leading_whitespace(lines[i]);
        let (end, rows) = scan_block(&lines, i + 1, heading_indent);
        for (idx, field) in rows {
            let name = field
                .split_whitespace()
                .next()
                .unwrap_or(&field)
                .to_string();
            if !tree_attests(root, &name, &field) {
                findings.push(Finding {
                    name,
                    pattern: field,
                    row: lines[idx].to_string(),
                });
            }
        }
        i = end;
    }
    Report { findings }
}

// ----------------------------------------------------------------------
// Self-checks
// ----------------------------------------------------------------------

use crate::detector::{Expect, SelfCheck};
use mandible_core::{Provenance, Source, Text};

/// A trimmed slice of `fail2ban-client`'s own bytes
/// (`corpus/fail2ban-client/1.0.2/help.txt`), two group labels and their
/// rows, including one same-line row and one wraps-below row.
pub(crate) const F2B_COMMAND_PATTERNS: &str = "\
Command:
                                             BASIC
    start                                    starts the server and the jails
    restart                                  restarts the server
    restart [--unban] [--if-exists] <JAIL>   restarts the jail <JAIL> (alias
                                             for 'reload --restart ... <JAIL>')
    reload [--restart] [--unban] [--all]     reloads the configuration without
                                             restarting of the server, the
                                             option '--restart' activates
    reload [--restart] [--unban] [--if-exists] <JAIL>
                                             reloads the jail <JAIL>, or
                                             restarts it (if option '--restart'
                                             specified)

                                             LOGGING
    set loglevel <LEVEL>                     sets logging level to <LEVEL>.
                                             Levels: CRITICAL, ERROR, WARNING
    get loglevel                             gets the logging level
";

fn node(name: &str) -> CommandNode {
    CommandNode::new(name, Provenance::single(Source::HelpText))
}

pub(crate) fn self_checks() -> Vec<SelfCheck> {
    vec![
        SelfCheck {
            name: "fail2ban-client's own bytes, every pattern row missing from a bare tree",
            why: "the defect itself: `restart [--unban]...`, both `reload [--restart]...` \
                  forms (same-line and wraps-below), `set loglevel <LEVEL>` and `get loglevel` \
                  never reach a tree that only has the bare single-word commands, since `set` \
                  and `get` never even exist as nodes",
            expect: Expect::Fires(5),
            raw: F2B_COMMAND_PATTERNS.to_string(),
            root: {
                let mut root = node("fail2ban-client");
                root.subcommands = ["start", "restart", "reload"]
                    .iter()
                    .map(|n| node(n))
                    .collect();
                root
            },
        },
        SelfCheck {
            name: "a correctly repaired tree",
            why: "once every row's own pattern is carried as a `usage` line on the matching \
                  node, the detector has nothing left to report",
            expect: Expect::Silent,
            raw: F2B_COMMAND_PATTERNS.to_string(),
            root: {
                let mut root = node("fail2ban-client");
                let mut restart = node("restart");
                restart.usage = vec![Text::sanitize("restart [--unban] [--if-exists] <JAIL>")];
                let mut reload = node("reload");
                reload.usage = vec![
                    Text::sanitize("reload [--restart] [--unban] [--all]"),
                    Text::sanitize("reload [--restart] [--unban] [--if-exists] <JAIL>"),
                ];
                let mut set = node("set");
                set.usage = vec![Text::sanitize("set loglevel <LEVEL>")];
                let mut get = node("get");
                get.usage = vec![Text::sanitize("get loglevel")];
                root.subcommands = vec![node("start"), restart, reload, set, get];
                root
            },
        },
        SelfCheck {
            name: "a bare single-word command row, not this shape",
            why: "an ordinary bare-name row is already `emit_subcommands`'s working case; this \
                  detector must not double-count it",
            expect: Expect::Silent,
            raw: "Command:\n    ping                                     tests if the server \
                  is alive\n"
                .to_string(),
            root: node("fail2ban-client"),
        },
        SelfCheck {
            name: "a centered ALL-CAPS group label, never mistaken for a row",
            why: "the block's own group labels (`BASIC`, `JAIL CONTROL`) are centered ALL-CAPS \
                  text with no name-shaped leading word and must not be read as a two-word row",
            expect: Expect::Silent,
            raw: "Command:\n                                             JAIL CONTROL\n"
                .to_string(),
            root: node("fail2ban-client"),
        },
        SelfCheck {
            name: "a value list under a heading naming no command",
            why: "the false alarm this shape's own heading gate exists for: the rows have this \
                  detector's exact grammar (a name-shaped word plus more), but their governing \
                  heading never mentions command(s), so no block ever opens",
            expect: Expect::Silent,
            raw: "Each CONV symbol may be:\n\n  ascii     from EBCDIC to ASCII\n  ebcdic    \
                  from ASCII to EBCDIC\n"
                .to_string(),
            root: node("dd"),
        },
        SelfCheck {
            name: "an ordinary flag row outside any command heading",
            why: "a two-word-shaped row (`-c, --conf` is not name-shaped, but a row like \
                  `set verbose` in an Options: block would still be) must not fire when the \
                  heading above it never mentions command(s)",
            expect: Expect::Silent,
            raw: "Options:\n    set verbose                              enables verbose \
                  output\n"
                .to_string(),
            root: node("prog"),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fires_on_every_missing_pattern_row_in_f2bs_own_bytes() {
        let root = {
            let mut root = node("fail2ban-client");
            root.subcommands = ["start", "restart", "reload"]
                .iter()
                .map(|n| node(n))
                .collect();
            root
        };
        let report = detect(F2B_COMMAND_PATTERNS, &root);
        assert_eq!(
            report.finding_count(),
            5,
            "{:?}",
            report
                .findings
                .iter()
                .map(|f| &f.pattern)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn every_self_check_holds() {
        for case in self_checks() {
            let expected = match case.expect {
                Expect::Fires(n) => n,
                Expect::Silent => 0,
            };
            let report = detect(&case.raw, &case.root);
            assert_eq!(
                report.finding_count(),
                expected,
                "{}: expected {} finding(s), got {:?}",
                case.name,
                expected,
                report
                    .findings
                    .iter()
                    .map(|f| &f.pattern)
                    .collect::<Vec<_>>()
            );
        }
    }
}
