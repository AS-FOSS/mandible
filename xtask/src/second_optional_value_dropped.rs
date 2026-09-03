//! `second-optional-value-dropped` (atlas S-097): a flag spec with two
//! adjacent bracketed optional values (`-V[N][fname]`) keeps only the
//! first and loses the second — the tree's entity carries a value name
//! that never mentions `fname`.
//!
//! Fixture: `corpus/vim.basic/audit-seed4/` (also present on
//! `corpus/nvim/0.9.5/`'s `-V[N][file]`, not required to be claimed
//! there).

use crate::family_row::leading_token;
use mandible_core::CommandNode;

pub struct Finding {
    pub flag: char,
    pub first_value: String,
    pub second_value: String,
}

pub struct Report {
    pub findings: Vec<Finding>,
}

impl Report {
    pub fn finding_count(&self) -> usize {
        self.findings.len()
    }
}

/// `token` read as `-<letter>[<value1>][<value2>]` with nothing after the
/// second bracket — two adjacent bracketed optional values glued to one
/// short flag.
fn parse_double_bracket(token: &str) -> Option<(char, String, String)> {
    let rest = token.strip_prefix('-')?;
    let mut chars = rest.char_indices();
    let (_, letter) = chars.next()?;
    if !letter.is_ascii_alphabetic() {
        return None;
    }
    let (next_idx, _) = chars.next()?;
    let after = &rest[next_idx..];
    let after = after.strip_prefix('[')?;
    let (value1, after) = after.split_once(']')?;
    let after = after.strip_prefix('[')?;
    let (value2, after) = after.split_once(']')?;
    if !after.is_empty() || value1.is_empty() || value2.is_empty() {
        return None;
    }
    Some((letter, value1.to_string(), value2.to_string()))
}

fn entity_value_name(root: &CommandNode, short: char) -> Option<String> {
    root.flags()
        .find(|e| e.short() == Some(short))
        .and_then(|e| e.value_name.clone())
}

pub fn detect(raw: &str, root: &CommandNode) -> Report {
    let mut findings = Vec::new();
    for line in raw.lines() {
        let Some((token, _rest)) = leading_token(line) else {
            continue;
        };
        let Some((letter, first_value, second_value)) = parse_double_bracket(token) else {
            continue;
        };
        let recovered = entity_value_name(root, letter).is_some_and(|v| v.contains(&second_value));
        if !recovered {
            findings.push(Finding {
                flag: letter,
                first_value,
                second_value,
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
pub(crate) const VIM_V_ROW: &str =
    "   -V[N][fname]\t\tBe verbose [level N] [log messages to fname]\n";

fn flag(short: char) -> Entity {
    Entity::flag_spelled(
        Some(short),
        None,
        false,
        false,
        Provenance::single(Source::HelpText),
    )
}

fn flag_with_value(short: char, value_name: &str) -> Entity {
    let mut e = flag(short);
    e.value_name = Some(value_name.to_string());
    e
}

fn node_with_flags(name: &str, flags: Vec<Entity>) -> CommandNode {
    let mut root = CommandNode::new(name, Provenance::single(Source::HelpText));
    root.set_entities_of(mandible_core::EntityKind::Flag, flags);
    root
}

pub(crate) fn self_checks() -> Vec<SelfCheck> {
    vec![
        SelfCheck {
            name: "vim.basic's own bytes, `fname` dropped",
            why: "the defect itself: only `N` survives as the value name, `fname` reaches \
                  nothing",
            expect: Expect::Fires(1),
            raw: VIM_V_ROW.to_string(),
            root: node_with_flags("vim.basic", vec![flag_with_value('V', "N")]),
        },
        SelfCheck {
            name: "both bracketed values recovered",
            why: "once the value name mentions both `N` and `fname`, the row must go silent",
            expect: Expect::Silent,
            raw: VIM_V_ROW.to_string(),
            root: node_with_flags("vim.basic", vec![flag_with_value('V', "N fname")]),
        },
        SelfCheck {
            name: "a single-bracket optional value, not this shape",
            why: "`-p[N]` has only one bracketed value — nothing is glued behind it, so this \
                  detector must not claim it",
            expect: Expect::Silent,
            raw: "   -p[N]\t\tOpen N tab pages\n".to_string(),
            root: node_with_flags("vim.basic", vec![flag('p')]),
        },
    ]
}
