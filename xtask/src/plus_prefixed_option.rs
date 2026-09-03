//! `plus-prefixed-option` (atlas S-095): a `+`-led option row (`+`,
//! `+<lnum>`, `+<cmd>`) reaches no entity in the tree at all.
//!
//! Root cause: `is_flag_shaped` requires a character right after the
//! leading sigil, so a bare `+` is false and `+<lnum>`/`+<cmd>` are false
//! too (no `<` in `is_flag_char`) — the row is never read as a flag row.
//!
//! A bare `+` line alone is not enough evidence (`git-lfs`'s AsciiDoc
//! list-continuation marker, `date`'s `%`-modifier row both match the
//! shape without being an option): also requires a flag-shaped
//! neighboring row, see `has_flag_shaped_neighbor`. Fixtures:
//! `corpus/vim.basic/audit-seed4/`, `corpus/nvim/0.9.5/`.

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

/// Whether the tree carries this `+`-led token under any of the shapes a
/// correct parse may hold it in: a literal spelling equal to the whole
/// token (`"+<lnum>"`), or — the shape the generic parser actually
/// produces, matching `Entity::argfile_sigil`'s own convention for a bare
/// sigil plus a value — an entity spelled bare `+` whose `value_name`
/// carries the bracketed placeholder. A bare `+` token itself only needs
/// the first half: any entity spelled `+` at all, value or not.
fn tree_has_spelling(root: &CommandNode, token: &str) -> bool {
    let placeholder = token.strip_prefix('+').filter(|p| !p.is_empty());
    root.flags().any(|e| {
        let spelled_this = e
            .spellings
            .iter()
            .any(|s| s.name == token || s.render() == token);
        if spelled_this {
            return true;
        }
        let spelled_plus = e.spellings.iter().any(|s| s.name == "+");
        match placeholder {
            None => spelled_plus,
            Some(p) => spelled_plus && e.value_name.as_deref().is_some_and(|v| v.contains(p)),
        }
    })
}

/// True when `line`'s own leading token is flag-shaped evidence: a real
/// `-`-prefixed flag (`-x`, `--name`, not a bare `-`), a `+`-claimed
/// token this same family recognizes, or the bare `--` marker.
fn is_flag_shaped_neighbor(line: &str) -> bool {
    let Some((token, _)) = leading_token(line) else {
        return false;
    };
    let token = token.trim_end_matches(',');
    if let Some(rest) = token.strip_prefix("--") {
        return rest.is_empty() || rest.chars().next().is_some_and(|c| c.is_ascii_alphabetic());
    }
    if let Some(rest) = token.strip_prefix('-') {
        return rest
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric());
    }
    is_claimed_plus_token(token)
}

/// Whether some nearby row (the nearest non-blank line above or below
/// `lines[i]`, skipping blanks) is itself flag-shaped — the positive
/// option-table evidence this detector requires before claiming a `+`
/// row. See the module doc comment.
fn has_flag_shaped_neighbor(lines: &[&str], i: usize) -> bool {
    let above = lines[..i].iter().rev().find(|l| !l.trim().is_empty());
    let below = lines[i + 1..].iter().find(|l| !l.trim().is_empty());
    above.is_some_and(|l| is_flag_shaped_neighbor(l))
        || below.is_some_and(|l| is_flag_shaped_neighbor(l))
}

pub fn detect(raw: &str, root: &CommandNode) -> Report {
    let mut findings = Vec::new();
    let lines: Vec<&str> = raw.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let Some((token, rest)) = leading_token(line) else {
            continue;
        };
        let token = token.trim_end_matches(',');
        if !is_claimed_plus_token(token) || !opens_description_column(rest) {
            continue;
        }
        if !has_flag_shaped_neighbor(&lines, i) {
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

/// git-lfs's real AsciiDoc list-continuation marker, byte-exact
/// (`git-lfs --help`, the "Getting Started" numbered list).
pub(crate) const GIT_LFS_LIST_CONTINUATION: &str = concat!(
    ". Setup Git LFS on your system. You only have to do this once per user\n",
    "account:\n",
    "+\n",
    "\n",
    "git lfs install\n",
);

/// date's real `%`-conversion-modifier table row, byte-exact
/// (`date --help`, "The following optional flags may follow '%':").
pub(crate) const DATE_PERCENT_MODIFIER_ROWS: &str = concat!(
    "  0  (zero) pad with zeros\n",
    "  +  pad with zeros, and put '+' before future years with >4 digits\n",
    "  ^  use upper case if possible\n",
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
        SelfCheck {
            name: "git-lfs's own bytes, an AsciiDoc list-continuation marker",
            why: "a real false positive this detector once had: a bare `+` line with prose \
                  neighbors on both sides (a numbered step above, a shell command below), \
                  neither flag-shaped, must never fire",
            expect: Expect::Silent,
            raw: GIT_LFS_LIST_CONTINUATION.to_string(),
            root: node_with_flags("git-lfs", vec![]),
        },
        SelfCheck {
            name: "date's own bytes, a `%`-conversion-modifier table row",
            why: "the other real false positive: `+` sits among `-`, `_`, `0`, `^`, `#` \
                  modifier-character rows, none of them a real `-`-prefixed flag, so no \
                  neighbor is flag-shaped and this detector must stay silent",
            expect: Expect::Silent,
            raw: DATE_PERCENT_MODIFIER_ROWS.to_string(),
            root: node_with_flags("date", vec![]),
        },
    ]
}
