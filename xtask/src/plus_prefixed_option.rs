//! `plus-prefixed-option` (atlas S-095): a `+`-led option row (`+`,
//! `+<lnum>`, `+<cmd>`) reaches no entity in the tree at all.
//!
//! Root cause: `help_text::sections::layout::is_flag_shaped` requires a
//! character right after the leading sigil, so a bare `+` is false, and
//! `<` is not in `is_flag_char`, so `+<lnum>`/`+<cmd>` are false too — the
//! row is never recognized as a flag row and is dropped whole.
//!
//! Fixtures: `corpus/vim.basic/audit-seed4/`, `corpus/nvim/0.9.5/`.

use crate::family_row::{leading_token, opens_description_column};
use mandible_core::CommandNode;

pub struct Finding {
    /// The `+`-led token, verbatim from its own row (`"+"`, `"+<lnum>"`).
    pub token: String,
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

/// True when `token` is a `+`-prefixed option spelling this family
/// claims: `+` alone, or `+` followed by a bracketed placeholder
/// (`+<lnum>`, `+<cmd>`) — never `++` or a token with a real letter
/// straight after the sigil (`+d`, which `is_flag_shaped` already reads).
fn is_claimed_plus_token(token: &str) -> bool {
    let Some(rest) = token.strip_prefix('+') else {
        return false;
    };
    rest.is_empty() || rest.starts_with('<')
}

/// Whether the tree carries any entity spelled with this exact token,
/// under either plausible representation an entity might hold a
/// `+`-led spelling in (a literal name, or a rendered form).
fn tree_has_spelling(root: &CommandNode, token: &str) -> bool {
    root.flags().any(|e| {
        e.spellings
            .iter()
            .any(|s| s.name == token || s.render() == token)
    })
}

pub fn detect(raw: &str, root: &CommandNode) -> Report {
    let mut findings = Vec::new();
    for line in raw.lines() {
        let Some((token, rest)) = leading_token(line) else {
            continue;
        };
        let token = token.trim_end_matches(',');
        if !is_claimed_plus_token(token) || !opens_description_column(rest) {
            continue;
        }
        if !tree_has_spelling(root, token) {
            findings.push(Finding {
                token: token.to_string(),
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

/// vim.basic's real two `+`-led rows, byte-exact
/// (`corpus/vim.basic/audit-seed4/help.txt`).
pub(crate) const VIM_PLUS_ROWS: &str = concat!(
    "Arguments:\n",
    "   +\t\t\tStart at end of file\n",
    "   +<lnum>\t\tStart at line <lnum>\n",
);

/// nvim's real `+` and `+<cmd>` rows, byte-exact
/// (`corpus/nvim/0.9.5/help.txt`).
pub(crate) const NVIM_PLUS_ROWS: &str = concat!(
    "Options:\n",
    "  +                     Start at end of file\n",
    "  +<cmd>, -c <cmd>      Execute <cmd> after config and first file\n",
);

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

/// An entity spelled bare `+` — the recovered form a fix might produce.
fn plus_flag() -> Entity {
    let mut e = Entity::new(
        mandible_core::EntityKind::Flag,
        Provenance::single(Source::HelpText),
    );
    e.spellings.push(mandible_core::Spelling::bare("+"));
    e
}

pub(crate) fn self_checks() -> Vec<SelfCheck> {
    vec![
        SelfCheck {
            name: "vim.basic's own bytes, `+` and `+<lnum>` dropped",
            why: "the defect itself: neither row's leading token is any entity's spelling",
            expect: Expect::Fires(2),
            raw: VIM_PLUS_ROWS.to_string(),
            root: node_with_flags("vim.basic", vec![flag(None, Some('v'))]),
        },
        SelfCheck {
            name: "nvim's own bytes, `+` and `+<cmd>` dropped",
            why: "the same shape on a second tool, the second alias `+<cmd>` on a comma-joined row",
            expect: Expect::Fires(2),
            raw: NVIM_PLUS_ROWS.to_string(),
            root: node_with_flags("nvim", vec![flag(None, Some('c'))]),
        },
        SelfCheck {
            name: "`+` recovered as a real spelling",
            why: "once the tree carries an entity spelled `+`, the same raw row must go silent",
            expect: Expect::Silent,
            raw: "Options:\n  +                     Start at end of file\n".to_string(),
            root: node_with_flags("nvim", vec![plus_flag()]),
        },
        SelfCheck {
            name: "a real `+`-cluster flag (`+d`), out of this family's claim",
            why: "lsof-style `+d` already has a real letter right after the sigil and \
                  `is_flag_shaped` reads it fine — this detector must not claim it",
            expect: Expect::Silent,
            raw: "Options:\n  +d                    directory\n".to_string(),
            root: node_with_flags("lsof", vec![]),
        },
        SelfCheck {
            name: "an unindented `+`-led heading line",
            why: "a heading has no indentation and is not an option row at all",
            expect: Expect::Silent,
            raw: "+ this is not indented\n".to_string(),
            root: node_with_flags("prog", vec![]),
        },
    ]
}
