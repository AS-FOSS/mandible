//! `command-row-argument-placeholder` (atlas S-129): under a recognized
//! command heading, a row whose name column is a command name followed by
//! an argument-placeholder run (`systemctl`'s `list-units [PATTERN...]`)
//! is a command with operands, but the whole unbroken field fails the
//! bare-name shape test and the row is dropped.
//!
//! A local copy of "recognized heading"/"is a placeholder", independent
//! of `mandible_extract::help_text` (`crate::commandtable`'s own
//! convention): an oracle built on the parser's own helper would agree
//! with it by construction. Fixture: `corpus/systemctl/255`.

use mandible_core::{is_command_name_shaped, CommandNode};

/// One row this detector believes documents a command with operands, but
/// whose name is missing from the tree.
pub struct Finding {
    pub name: String,
    pub line: String,
}

pub struct Report {
    pub findings: Vec<Finding>,
}

impl Report {
    pub fn finding_count(&self) -> usize {
        self.findings.len()
    }
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// True if `s` mentions "command(s)"/"subcommand(s)"/"operation(s)" as a
/// whole word — the same vocabulary spec §7 Tier B rule 1 reads, kept as
/// its own copy rather than imported.
fn mentions_command_word(s: &str) -> bool {
    s.split(|c: char| !c.is_alphanumeric()).any(|w| {
        matches!(
            w.to_lowercase().as_str(),
            "command" | "commands" | "subcommand" | "subcommands" | "operation" | "operations"
        )
    })
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

/// `rest` is nothing but argument placeholders — every whitespace- or
/// `=`-delimited word (brackets, dots and `|` stripped first) is
/// uppercase-led. A real dropped description carries at least one
/// lowercase word and fails this immediately.
fn looks_like_operand_placeholder_run(rest: &str) -> bool {
    let cleaned: String = rest
        .chars()
        .map(|c| {
            if matches!(c, '[' | ']' | '.' | '|') {
                ' '
            } else {
                c
            }
        })
        .collect();
    let mut any = false;
    for word in cleaned.split_whitespace().flat_map(|w| w.split('=')) {
        if word.is_empty() {
            continue;
        }
        any = true;
        let mut chars = word.chars();
        match chars.next() {
            Some(c) if c.is_ascii_uppercase() => {}
            _ => return false,
        }
        if !chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_') {
            return false;
        }
    }
    any
}

/// Split a row into `(name, operand_rest)` when it is a command name
/// followed by nothing but placeholders, before any description column.
fn name_and_operand_rest(row: &str) -> Option<(&str, &str)> {
    // Cut at the first 2+-space gap (or tab) — the description column —
    // same boundary the real layout parser uses, so this never mistakes a
    // real description for operand text. The row's own leading indent is
    // trimmed first, so a gap search never mistakes that indent for the
    // description column.
    let content = row.trim_start();
    let mut cut = content.len();
    let mut run = 0usize;
    let mut seen_content = false;
    for (i, c) in content.char_indices() {
        if c == ' ' {
            run += 1;
            if seen_content && run >= 2 {
                cut = i + 1 - run;
                break;
            }
        } else if c == '\t' {
            if seen_content {
                cut = i;
                break;
            }
        } else {
            run = 0;
            seen_content = true;
        }
    }
    let field = content[..cut].trim_end();
    let (first, rest) = field.split_once(char::is_whitespace)?;
    if !is_command_name_shaped(first) {
        return None;
    }
    let rest = rest.trim();
    looks_like_operand_placeholder_run(rest).then_some((first, rest))
}

pub fn detect(raw: &str, root: &CommandNode) -> Report {
    let lines: Vec<&str> = raw.lines().collect();
    let mut findings = Vec::new();
    let mut i = 0usize;
    while i < lines.len() {
        if !is_command_heading(lines[i]) {
            i += 1;
            continue;
        }
        let heading_indent = indent_of(lines[i]);
        let mut j = i + 1;
        while j < lines.len() {
            let line = lines[j];
            if line.trim().is_empty() {
                j += 1;
                continue;
            }
            if indent_of(line) <= heading_indent {
                break;
            }
            if let Some((name, _rest)) = name_and_operand_rest(line) {
                let already_present = root.subcommands.iter().any(|c| c.name == name);
                if !already_present {
                    findings.push(Finding {
                        name: name.to_string(),
                        line: line.to_string(),
                    });
                }
            }
            j += 1;
        }
        i = j;
    }
    Report { findings }
}

// ----------------------------------------------------------------------
// Self-checks
// ----------------------------------------------------------------------

use crate::detector::{Expect, SelfCheck};
use mandible_core::{Provenance, Source};

/// A trimmed slice of `systemctl`'s own bytes (`corpus/systemctl/255/help.txt`).
pub(crate) const SYSTEMCTL_UNIT_COMMANDS: &str = concat!(
    "Unit Commands:\n",
    "  list-units [PATTERN...]             List units currently in memory\n",
    "  start UNIT...                       Start (activate) one or more units\n",
);

fn node(name: &str) -> CommandNode {
    CommandNode::new(name, Provenance::single(Source::HelpText))
}

pub(crate) fn self_checks() -> Vec<SelfCheck> {
    vec![
        SelfCheck {
            name: "systemctl's own bytes, both rows missing",
            why: "the defect itself: two command rows carry an argument placeholder in their \
                  name column and neither name reaches the tree",
            expect: Expect::Fires(2),
            raw: SYSTEMCTL_UNIT_COMMANDS.to_string(),
            root: node("systemctl"),
        },
        SelfCheck {
            name: "both names already recovered",
            why: "once a row's name is a real subcommand, the same raw row must go silent",
            expect: Expect::Silent,
            raw: SYSTEMCTL_UNIT_COMMANDS.to_string(),
            root: {
                let mut root = node("systemctl");
                root.subcommands.push(node("list-units"));
                root.subcommands.push(node("start"));
                root
            },
        },
        SelfCheck {
            name: "a single-token command row, not this shape",
            why: "an ordinary bare-name row is `emit_subcommands`'s existing, working case; this \
                  detector must not double-count it",
            expect: Expect::Silent,
            raw: "Commands:\n  preset-all       Enable/disable all unit files\n".to_string(),
            root: node("systemctl"),
        },
        SelfCheck {
            name: "a dropped description with no heading at all",
            why: "with no recognized command heading above it, a bare name-plus-word row is not \
                  this family, whatever the row itself looks like",
            expect: Expect::Silent,
            raw: "  list-units [PATTERN...]             List units currently in memory\n"
                .to_string(),
            root: node("systemctl"),
        },
        SelfCheck {
            name: "a genuine dropped description under a command heading",
            why: "an ordinary lowercase continuation word is a description gap in a different \
                  family (single-space-description-column), never an operand — this detector \
                  must stay silent rather than launder it",
            expect: Expect::Silent,
            raw: "Commands:\n  frobnicate WIDGET frobnicate the given widget\n".to_string(),
            root: node("prog"),
        },
    ]
}
