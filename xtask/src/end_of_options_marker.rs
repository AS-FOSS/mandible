//! `end-of-options-marker` (atlas S-096): a `--` row in an options block
//! is dropped — no entity in the tree is spelled `--`.
//!
//! Same root cause as `crate::plus_prefixed_option`:
//! `help_text::sections::layout::is_flag_shaped` requires a character
//! right after the leading sigil, so the bare `--` token is never read as
//! flag-shaped and the whole row is dropped.
//!
//! Fixtures: `corpus/vim.basic/audit-seed4/`, `corpus/nvim/0.9.5/`.

use crate::family_row::{leading_token, opens_description_column};
use mandible_core::CommandNode;

pub struct Finding {
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

fn tree_has_end_of_options_marker(root: &CommandNode) -> bool {
    root.flags().any(|e| {
        e.spellings
            .iter()
            .any(|s| s.name == "--" || s.render() == "--")
    })
}

pub fn detect(raw: &str, root: &CommandNode) -> Report {
    // A tool-wide question, not a per-row one: once any entity anywhere is
    // spelled `--`, the marker was recovered and every candidate row goes
    // silent together.
    if tree_has_end_of_options_marker(root) {
        return Report {
            findings: Vec::new(),
        };
    }
    let mut findings = Vec::new();
    for line in raw.lines() {
        let Some((token, rest)) = leading_token(line) else {
            continue;
        };
        if token == "--" && opens_description_column(rest) {
            findings.push(Finding {
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
pub(crate) const VIM_DASHDASH_ROW: &str = "Arguments:\n   --\t\t\tOnly file names after this\n";

/// nvim's real row, byte-exact (`corpus/nvim/0.9.5/help.txt`).
pub(crate) const NVIM_DASHDASH_ROW: &str =
    "Options:\n  --                    Only file names after this\n";

fn flag(long: Option<&str>, short: Option<char>) -> Entity {
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

fn dashdash_flag() -> Entity {
    let mut e = Entity::new(
        mandible_core::EntityKind::Flag,
        Provenance::single(Source::HelpText),
    );
    e.spellings.push(mandible_core::Spelling::bare("--"));
    e
}

pub(crate) fn self_checks() -> Vec<SelfCheck> {
    vec![
        SelfCheck {
            name: "vim.basic's own bytes, `--` dropped",
            why: "the defect itself: `--`'s row is never any entity's spelling",
            expect: Expect::Fires(1),
            raw: VIM_DASHDASH_ROW.to_string(),
            root: node_with_flags("vim.basic", vec![flag(None, Some('v'))]),
        },
        SelfCheck {
            name: "nvim's own bytes, `--` dropped",
            why: "the same shape on a second tool",
            expect: Expect::Fires(1),
            raw: NVIM_DASHDASH_ROW.to_string(),
            root: node_with_flags("nvim", vec![flag(None, Some('c'))]),
        },
        SelfCheck {
            name: "`--` recovered as a real spelling",
            why: "once the tree carries an entity spelled `--`, the same raw row must go silent",
            expect: Expect::Silent,
            raw: NVIM_DASHDASH_ROW.to_string(),
            root: node_with_flags("nvim", vec![dashdash_flag()]),
        },
        SelfCheck {
            name: "a real long flag beginning with two dashes (`--help`)",
            why: "the token check is exact equality with `--`, never a prefix match, so an \
                  ordinary long flag must never be claimed",
            expect: Expect::Silent,
            raw: "Options:\n  --help                Print this help message\n".to_string(),
            root: node_with_flags("prog", vec![flag(Some("help"), None)]),
        },
        SelfCheck {
            name: "`--` leading a sentence, not an aligned option row",
            why: "single-spaced prose (\"-- means options end here\") never opens a real \
                  description-column gap, so this is not a candidate row at all",
            expect: Expect::Silent,
            raw: "  -- means options end here\n".to_string(),
            root: node_with_flags("prog", vec![]),
        },
    ]
}
