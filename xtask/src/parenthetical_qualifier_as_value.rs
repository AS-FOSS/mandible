//! `parenthetical-qualifier-as-value` (atlas S-098): `-r (with file
//! name)` reads the parenthetical qualifier's leading word `(with` as the
//! flag's value name, so `file name)` reaches nothing.
//!
//! Fixture: `corpus/vim.basic/audit-seed4/`.

use crate::family_row::leading_token;
use mandible_core::CommandNode;

pub struct Finding {
    pub flag: char,
    pub value_name: String,
}

pub struct Report {
    pub findings: Vec<Finding>,
}

impl Report {
    pub fn finding_count(&self) -> usize {
        self.findings.len()
    }
}

/// True when `token` is a bare short flag: one dash, one letter.
fn is_short_flag(token: &str) -> bool {
    token.len() == 2
        && token.starts_with('-')
        && token
            .chars()
            .nth(1)
            .is_some_and(|c| c.is_ascii_alphabetic())
}

pub fn detect(raw: &str, root: &CommandNode) -> Report {
    let mut findings = Vec::new();
    for line in raw.lines() {
        let Some((token, rest)) = leading_token(line) else {
            continue;
        };
        if !is_short_flag(token) || !rest.trim_start().starts_with('(') {
            continue;
        }
        let letter = token.chars().nth(1).unwrap();
        // Every entity spelled this letter, not just the first one a
        // `.find()` would return: a short flag documented on more than one
        // row (`vim.basic`'s two `-r` rows) reaches the tree as more than
        // one entity sharing the spelling, and the defect can sit on
        // either of them.
        let hit = root
            .flags()
            .filter(|e| e.short() == Some(letter))
            .find_map(|e| e.value_name.clone().filter(|v| v.starts_with('(')));
        // The signature of the defect, read straight off the tree: a
        // value name can never legitimately begin with an open paren —
        // that character only appears here because the row's own
        // parenthetical qualifier was misread as the value.
        if let Some(value_name) = hit {
            findings.push(Finding {
                flag: letter,
                value_name,
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
pub(crate) const VIM_R_QUALIFIER_ROW: &str = "   -r (with file name)\tRecover crashed session\n";

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
            name: "vim.basic's own bytes, `(with` read as the value",
            why: "the defect itself: the tree's second `-r` carries a value name that starts \
                  with an open paren",
            expect: Expect::Fires(1),
            raw: VIM_R_QUALIFIER_ROW.to_string(),
            root: node_with_flags("vim.basic", vec![flag_with_value('r', "(with")]),
        },
        SelfCheck {
            name: "the parenthetical read as plain description text instead",
            why: "once no value name starts with `(`, the row is silent — this is what a fix \
                  looks like, whether or not the description also carries the full qualifier",
            expect: Expect::Silent,
            raw: VIM_R_QUALIFIER_ROW.to_string(),
            root: node_with_flags("vim.basic", vec![flag('r')]),
        },
        SelfCheck {
            name: "a bracketed value, not a parenthetical qualifier",
            why: "`-p[N]` is an ordinary optional value in brackets, never in parens, so this \
                  detector must not claim it",
            expect: Expect::Silent,
            raw: "   -p [N]\tOpen N tab pages\n".to_string(),
            root: node_with_flags("prog", vec![flag_with_value('p', "N")]),
        },
    ]
}
