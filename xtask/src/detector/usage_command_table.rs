//! `usage-command-table` (round 11, atlas S-169): a command table sitting
//! directly under the tool's own root `Usage:` synopsis — a bare `Usage:`
//! label, a blank line, the tool's own name alone on its line, its own
//! bracketed global flags on continuation lines, a blank line, then one
//! row per command, rows never repeating the tool's own name
//! (`dmsetup --help`'s second block; `docs/shapes.md` S-169). Local,
//! independent shape check (no shared code with `mandible-extract`), the
//! same reasoning `invocation_form_head_as_flag_group` (S-137) uses for
//! its own local re-derivation.

use crate::detector::{Detector, Expect, Scope, SelfCheck, ToolEvidence};
use mandible_core::CommandNode;

pub struct Finding {
    pub name: String,
}

pub struct Report {
    pub findings: Vec<Finding>,
}

/// A command-word-shaped token: `^[a-z][a-z0-9_.-]*$`, spec §7 Tier B
/// rule 7's own candidate test, re-derived locally rather than imported —
/// this crate cannot depend on `mandible-extract`.
fn is_command_word_shaped(tok: &str) -> bool {
    let mut chars = tok.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '.' | '-'))
}

fn leading_whitespace(line: &str) -> usize {
    let mut col = 0usize;
    for c in line.chars() {
        if c == '\t' {
            col = (col / 8 + 1) * 8;
        } else if c.is_whitespace() {
            col += 1;
        } else {
            break;
        }
    }
    col
}

/// The distinct command words the raw text's own layout names, under
/// `docs/shapes.md` S-169's exact shape — or `None` when the document
/// does not open with it at all.
fn raw_command_table_rows(raw: &str, tool_name: &str) -> Option<Vec<String>> {
    let lines: Vec<&str> = raw.lines().collect();
    let mut i = lines
        .iter()
        .position(|l| l.trim().trim_end_matches(':').eq_ignore_ascii_case("usage"))?;
    i += 1;
    if !lines.get(i).is_some_and(|l| l.trim().is_empty()) {
        return None;
    }
    i += 1;
    if lines.get(i).map(|l| l.trim()) != Some(tool_name) {
        return None;
    }
    i += 1;
    // The root's own bracketed continuation, until the blank line that
    // separates it from the command table.
    while lines.get(i).is_some_and(|l| !l.trim().is_empty()) {
        i += 1;
    }
    if !lines.get(i).is_some_and(|l| l.trim().is_empty()) {
        return None;
    }
    i += 1;
    let first = lines.get(i)?;
    if first.trim().is_empty() {
        return None;
    }
    let table_indent = leading_whitespace(first);
    if table_indent == 0 {
        return None;
    }
    let mut names = Vec::new();
    while let Some(line) = lines.get(i) {
        if line.trim().is_empty() {
            break;
        }
        let indent = leading_whitespace(line);
        if indent < table_indent {
            break;
        }
        if indent == table_indent {
            let trimmed = line.trim();
            let word_end = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
            let word = &trimmed[..word_end];
            if !is_command_word_shaped(word) {
                break;
            }
            if word == tool_name {
                // S-016's own shape, not this one.
                return None;
            }
            if !names.contains(&word.to_string()) {
                names.push(word.to_string());
            }
        }
        i += 1;
    }
    (names.len() >= 2).then_some(names)
}

pub fn detect(raw: &str, root: &CommandNode) -> Report {
    let Some(names) = raw_command_table_rows(raw, &root.name) else {
        return Report {
            findings: Vec::new(),
        };
    };
    let findings = names
        .into_iter()
        .filter(|name| !root.subcommands.iter().any(|c| &c.name == name))
        .map(|name| Finding { name })
        .collect();
    Report { findings }
}

pub struct UsageCommandTable;

impl Detector for UsageCommandTable {
    fn name(&self) -> &'static str {
        "usage-command-table"
    }

    fn family(&self) -> Option<&'static str> {
        None
    }

    fn describes(&self) -> &'static str {
        "a command table under the tool's own root `Usage:` synopsis (bare label, blank, own \
         name, bracketed continuation, blank, one row per command) whose rows never repeat the \
         tool's own name, and are missing from the parsed tree's own subcommands"
    }

    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        detect(evidence.raw, evidence.root)
            .findings
            .iter()
            .map(|f| {
                format!(
                    "{} named by the usage command table but not a subcommand",
                    f.name
                )
            })
            .collect()
    }

    fn scope(&self) -> Scope {
        Scope::full()
    }

    fn self_checks(&self) -> Vec<SelfCheck> {
        self_checks()
    }
}

// ----------------------------------------------------------------------
// Self-checks
// ----------------------------------------------------------------------

use mandible_core::{Provenance, Source};

/// dmsetup's own shape, byte-exact enough to exercise the recognizer.
pub(crate) const DMSETUP_USAGE_SHAPE: &str = "Usage:\n\
     \n\
     dmsetup\n        \
     [--version] [-h|--help [-c|-C|--columns]]\n\
     \n\
     \thelp [-c|-C|--columns]\n\
     \tcreate <dev_name>\n\
     \tremove [--deferred] [-f|--force] [--retry] <device>...\n";

fn empty_node(name: &str) -> CommandNode {
    CommandNode::new(name, Provenance::single(Source::HelpText))
}

fn node_with_subcommands(name: &str, subs: &[&str]) -> CommandNode {
    let mut root = empty_node(name);
    root.subcommands = subs.iter().map(|s| empty_node(s)).collect();
    root
}

pub(crate) fn self_checks() -> Vec<SelfCheck> {
    vec![
        SelfCheck {
            name: "dmsetup's own bytes, no commands recovered at all",
            why: "the defect itself: the whole table is missing from the tree",
            expect: Expect::Fires(3),
            raw: DMSETUP_USAGE_SHAPE.to_string(),
            root: empty_node("dmsetup"),
        },
        SelfCheck {
            name: "dmsetup's own bytes, every row recovered as its own subcommand",
            why: "once every row is a real subcommand, the same raw text must go silent",
            expect: Expect::Silent,
            raw: DMSETUP_USAGE_SHAPE.to_string(),
            root: node_with_subcommands("dmsetup", &["help", "create", "remove"]),
        },
        SelfCheck {
            name: "an ordinary `Usage: prog [opts]` tool, not this shape at all",
            why: "a one-line labelled usage synopsis must never be mistaken for the bare-label \
                  idiom this rule requires",
            expect: Expect::Silent,
            raw: "Usage: prog [opts]\n\n  frob   do the frobbing\n  twist  do the twisting\n"
                .to_string(),
            root: empty_node("prog"),
        },
        SelfCheck {
            name: "btrfs's own shape, rows repeating the tool's own name",
            why: "S-016's own shape, a table whose rows repeat the tool's name, is a different \
                  rule entirely and must never double-count here",
            expect: Expect::Silent,
            raw: "Usage:\n\
                  \n\
                  btrfs\n        \
                  [--version]\n\
                  \n\
                  \tbtrfs balance start <path>\n\
                  \tbtrfs balance pause <path>\n"
                .to_string(),
            root: empty_node("btrfs"),
        },
    ]
}
