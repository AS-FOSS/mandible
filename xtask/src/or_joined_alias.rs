//! `or-joined-alias` (atlas S-099): `-h  or  --help` is read as one flag
//! `-h` whose description begins `or --help`, so `--help` never becomes
//! an alias of its own.
//!
//! Fixture: `corpus/vim.basic/audit-seed4/`.

use mandible_core::CommandNode;

pub struct Finding {
    pub first: String,
    pub second: String,
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

/// `line` read as `<flag>  or  <flag>...` — two flag-shaped tokens joined
/// by the bare word `or`, indented like an ordinary option row.
fn parse_or_joined_row(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed == line {
        return None;
    }
    let tokens: Vec<&str> = trimmed.split_whitespace().collect();
    if tokens.len() < 3 || tokens[1] != "or" {
        return None;
    }
    if !tokens[0].starts_with('-') || !tokens[2].starts_with('-') {
        return None;
    }
    Some((tokens[0].to_string(), tokens[2].to_string()))
}

fn tree_has_spelling(root: &CommandNode, token: &str) -> bool {
    root.flags().any(|e| {
        e.spellings
            .iter()
            .any(|s| s.render() == token || s.typed() == token)
    })
}

pub fn detect(raw: &str, root: &CommandNode) -> Report {
    let mut findings = Vec::new();
    for line in raw.lines() {
        let Some((first, second)) = parse_or_joined_row(line) else {
            continue;
        };
        if !tree_has_spelling(root, &second) {
            findings.push(Finding {
                first,
                second,
                line: line.to_string(),
            });
        }
    }
    Report { findings }
}

// ----------------------------------------------------------------------
// Self-checks
// ----------------------------------------------------------------------

use crate::detector::{Expect, SelfCheck};
use mandible_core::{Entity, Provenance, Source};

/// vim.basic's real row, byte-exact (`corpus/vim.basic/audit-seed4/help.txt`).
pub(crate) const VIM_H_OR_HELP_ROW: &str =
    "   -h  or  --help\tPrint Help (this message) and exit\n";

fn flag(short: Option<char>, long: Option<&str>) -> Entity {
    Entity::flag_spelled(
        short,
        long.map(|s| s.to_string()),
        false,
        false,
        Provenance::single(Source::HelpText),
    )
}

fn node_with_flags(name: &str, flags: Vec<Entity>) -> CommandNode {
    let mut root = CommandNode::new(name, Provenance::single(Source::HelpText));
    root.set_entities_of(mandible_core::EntityKind::Flag, flags);
    root
}

pub(crate) fn self_checks() -> Vec<SelfCheck> {
    vec![
        SelfCheck {
            name: "vim.basic's own bytes, `--help` never becomes an alias",
            why: "the defect itself: no entity anywhere is spelled `--help`",
            expect: Expect::Fires(1),
            raw: VIM_H_OR_HELP_ROW.to_string(),
            root: node_with_flags("vim.basic", vec![flag(Some('h'), None)]),
        },
        SelfCheck {
            name: "both spellings recovered as one entity's aliases",
            why: "once an entity carries the `--help` spelling too, the row must go silent",
            expect: Expect::Silent,
            raw: VIM_H_OR_HELP_ROW.to_string(),
            root: node_with_flags("vim.basic", vec![flag(Some('h'), Some("help"))]),
        },
        SelfCheck {
            name: "a comma-joined alias pair, not `or`-joined",
            why: "the ordinary `-h, --help` shape already parses as one entity with two \
                  spellings and is not this row's defect — no `or` token, so it is never a \
                  candidate at all",
            expect: Expect::Silent,
            raw: "   -h, --help\tPrint Help\n".to_string(),
            root: node_with_flags("prog", vec![flag(Some('h'), Some("help"))]),
        },
        SelfCheck {
            name: "the word 'or' appearing inside an ordinary description",
            why: "an unindented sentence using the word \"or\" is not an option row at all",
            expect: Expect::Silent,
            raw: "-h or --help, pick one\n".to_string(),
            root: node_with_flags("prog", vec![]),
        },
    ]
}
