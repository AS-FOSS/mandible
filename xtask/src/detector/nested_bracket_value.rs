//! `nested-bracket-value` (atlas S-119): a short flag's value spec is one
//! bracket group with exactly one bracket nested inside it, glued
//! directly to the flag (`-e[CHAR[WIDTH]]`) and usually followed by a
//! long alias in the same shape (`--expand-tabs[=CHAR[WIDTH]]`). Before
//! the fix, the bracket matcher stopped at the row's first `]`, so the
//! long alias and the closing bracket both fell off the row.
//!
//! Fixtures: `corpus/pr/9.4/` (`-e`, `-i`, `-n`).

use crate::family_row::leading_token;
use mandible_core::CommandNode;

pub struct Finding {
    pub flag: char,
    pub source_spelling: String,
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

/// `-X[A[B]]`: a short flag letter directly followed by one bracket group
/// with exactly one bracket nested inside it. Returns the flag character
/// and the full bracketed source spelling, e.g. `('e', "[CHAR[WIDTH]]")`.
fn nested_bracket_token(token: &str) -> Option<(char, &str)> {
    let mut chars = token.char_indices();
    let (_, dash) = chars.next()?;
    if dash != '-' {
        return None;
    }
    let (letter_idx, letter) = chars.next()?;
    if !letter.is_ascii_alphanumeric() {
        return None;
    }
    let after = &token[letter_idx + letter.len_utf8()..];
    let rest = after.strip_prefix('[')?;
    let inner_open = rest.find('[')?;
    let inner_close_rel = rest[inner_open + 1..].find(']')?;
    let content_len = inner_open + 1 + inner_close_rel + 1;
    if !rest[content_len..].starts_with(']') {
        return None;
    }
    // The outer `[` plus the content plus the outer `]`.
    let full_len = 1 + content_len + 1;
    Some((letter, &after[..full_len]))
}

pub fn detect(raw: &str, root: &CommandNode) -> Report {
    let mut findings = Vec::new();
    for line in raw.lines() {
        // No `opens_description_column` gate: unlike the tab-aligned vim
        // shape (`glued-optional-group-spelling`), this row's description
        // routinely wraps onto its own continuation line, so the token's
        // own narrow shape (dash, one letter, one bracket nested inside
        // another) is the only gate this detector needs.
        let Some((token, _rest)) = leading_token(line) else {
            continue;
        };
        let Some((letter, spelling)) = nested_bracket_token(token) else {
            continue;
        };
        let Some(entity) = root.flags().find(|e| e.short() == Some(letter)) else {
            continue;
        };
        let matches = entity.value_name.as_deref() == Some(spelling);
        if !matches {
            findings.push(Finding {
                flag: letter,
                source_spelling: spelling.to_string(),
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

/// `pr`'s real row, byte-exact (`corpus/pr/9.4/help.txt`).
pub(crate) const PR_E_ROW: &str =
    "  -e[CHAR[WIDTH]], --expand-tabs[=CHAR[WIDTH]]\n                    expand input CHARs \
     (TABs) to tab WIDTH (8)\n";

fn short_flag_with_value(short: char, value_name: &str) -> Entity {
    let mut e = Entity::flag_spelled(
        Some(short),
        None,
        false,
        false,
        Provenance::single(Source::HelpText),
    );
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
            name: "pr's own bytes, `-e`'s value truncated at the first `]`",
            why: "the defect itself: the source spells the value `[CHAR[WIDTH]]`; the tree \
                  carries only the truncated `CHAR[WIDTH`",
            expect: Expect::Fires(1),
            raw: PR_E_ROW.to_string(),
            root: node_with_flags("pr", vec![short_flag_with_value('e', "CHAR[WIDTH")]),
        },
        SelfCheck {
            name: "`-e`'s value already matches the source spelling",
            why: "once the value name is kept exactly as documented, the same raw row must go \
                  silent",
            expect: Expect::Silent,
            raw: PR_E_ROW.to_string(),
            root: node_with_flags("pr", vec![short_flag_with_value('e', "[CHAR[WIDTH]]")]),
        },
        SelfCheck {
            name: "a single bracketed value with no nesting at all",
            why: "the token scan requires a bracket nested inside the outer one; one bracket \
                  alone is an ordinary optional value and not this shape",
            expect: Expect::Silent,
            raw: "  -s[CHAR], --separator[=CHAR]\n              separate columns\n".to_string(),
            root: node_with_flags("pr", vec![short_flag_with_value('s', "CHAR")]),
        },
        SelfCheck {
            name: "an ordinary flag with no bracketed value at all",
            why: "the leading-token scan requires the bracket to sit directly against the flag \
                  letter, so a plain flag must never be claimed",
            expect: Expect::Silent,
            raw: "  -h\t\thelp\n".to_string(),
            root: node_with_flags(
                "prog",
                vec![Entity::flag_spelled(
                    Some('h'),
                    None,
                    false,
                    false,
                    Provenance::single(Source::HelpText),
                )],
            ),
        },
    ]
}
