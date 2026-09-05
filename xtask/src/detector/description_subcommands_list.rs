//! `description-subcommands-list` (round 5's S-114, atlas kept the same
//! id): a `Subcommands:` sub-heading sits indented inside the
//! `Description:` prose block, followed by a bulleted `- name:
//! description` list — `pip3 config --help`'s six actions. No recognizer
//! fires on this shape today, so `pip3 config` has no children.
//!
//! Fixture: captured locally (`pip3 config --help`); not yet a corpus
//! fixture (the pip3 fixture lives on PR #130's branch, not main, per
//! round 6's brief).

use mandible_core::CommandNode;

pub struct Finding {
    pub name: String,
}

pub struct Report {
    pub findings: Vec<Finding>,
}

impl Report {
    pub fn finding_count(&self) -> usize {
        self.findings.len()
    }
}

fn leading_whitespace(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// A short, colon-terminated `Subcommands:` label (any case), same shape
/// `is_section_heading_line` reads elsewhere but checked directly here
/// rather than imported, per this crate's oracle-independence
/// convention.
fn is_subcommands_heading(line: &str) -> bool {
    let trimmed = line.trim();
    let Some(label) = trimmed.strip_suffix(':') else {
        return false;
    };
    label.eq_ignore_ascii_case("subcommands") || label.eq_ignore_ascii_case("subcommand")
}

/// One bulleted `- name: description` row: a leading `-`, then a bare
/// command name, then a colon, then prose.
fn parse_bullet_row(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim().strip_prefix('-')?.trim_start();
    let (name, desc) = trimmed.split_once(':')?;
    let name = name.trim();
    if name.is_empty()
        || !name.starts_with(|c: char| c.is_ascii_lowercase())
        || !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-'))
    {
        return None;
    }
    Some((name.to_string(), desc.trim().to_string()))
}

pub fn detect(raw: &str, root: &CommandNode) -> Report {
    let lines: Vec<&str> = raw.lines().collect();
    let mut findings = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if !is_subcommands_heading(line) {
            continue;
        }
        let heading_indent = leading_whitespace(line);
        let mut j = i + 1;
        while let Some(&row) = lines.get(j) {
            if row.trim().is_empty() {
                j += 1;
                continue;
            }
            // `pip3 config`'s own bytes indent `Subcommands:` and its
            // bulleted rows at the SAME depth (both nested one level
            // inside `Description:`), so only a strict dedent ends the
            // block, never equality.
            if leading_whitespace(row) < heading_indent {
                break;
            }
            if let Some((name, _desc)) = parse_bullet_row(row) {
                if !root.subcommands.iter().any(|c| c.name == name) {
                    findings.push(Finding { name });
                }
            }
            j += 1;
        }
    }
    Report { findings }
}

// ----------------------------------------------------------------------
// Self-checks
// ----------------------------------------------------------------------

use crate::detector::{Expect, SelfCheck};
use mandible_core::{Provenance, Source};

/// `pip3 config --help`'s own bytes, trimmed to the load-bearing block.
pub(crate) const PIP3_CONFIG_DESCRIPTION: &str = concat!(
    "Description:\n",
    "  Manage local and global configuration.\n",
    "\n",
    "  Subcommands:\n",
    "\n",
    "  - list: List the active configuration (or from the file specified)\n",
    "  - edit: Edit the configuration file in an editor\n",
    "  - get: Get the value associated with command.option\n",
);

fn node(name: &str) -> CommandNode {
    CommandNode::new(name, Provenance::single(Source::HelpText))
}

pub(crate) fn self_checks() -> Vec<SelfCheck> {
    vec![
        SelfCheck {
            name: "pip3 config's own bytes, all three actions missing",
            why: "the defect itself: a Subcommands: label nested in the Description: block, and \
                  none of its three bulleted actions reaches the tree",
            expect: Expect::Fires(3),
            raw: PIP3_CONFIG_DESCRIPTION.to_string(),
            root: node("config"),
        },
        SelfCheck {
            name: "all three actions already recovered",
            why: "once every bulleted name is a real subcommand, the same raw block must go \
                  silent",
            expect: Expect::Silent,
            raw: PIP3_CONFIG_DESCRIPTION.to_string(),
            root: {
                let mut root = node("config");
                root.subcommands.push(node("list"));
                root.subcommands.push(node("edit"));
                root.subcommands.push(node("get"));
                root
            },
        },
        SelfCheck {
            name: "an ordinary Description: block with no Subcommands: label",
            why: "with no recognized sub-heading, an indented bulleted list is ordinary prose, \
                  not a command list",
            expect: Expect::Silent,
            raw: "Description:\n  Manage things.\n\n  - a thing\n  - another thing\n".to_string(),
            root: node("prog"),
        },
    ]
}
