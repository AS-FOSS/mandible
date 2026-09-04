//! `glued-optional-group-spelling` (atlas S-111): a flag whose value spec
//! is two or more glued optional groups (`-V[N][fname]`) reaches the tree
//! with the groups folded into one space-joined name (`N fname`) instead
//! of the source spelling `[N][fname]`.
//!
//! Distinct from `crate::second_optional_value_dropped`, which claims a
//! *lost* second value; this family claims the reformatted spelling even
//! once both values survive the fold.
//!
//! Fixtures: `corpus/vim.basic/audit-seed4/`, `corpus/nvim/0.9.5/`.

use crate::family_row::{leading_token, opens_description_column};
use mandible_core::CommandNode;

pub struct Finding {
    pub flag: char,
    pub value_name: String,
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

/// `-X[A][B]...`: a short flag letter followed by two or more glued
/// bracket groups with nothing else in the token. Returns the flag
/// character and the exact bracket sequence, e.g. `('V', "[N][fname]")`.
fn glued_optional_groups(token: &str) -> Option<(char, &str)> {
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
    if !after.starts_with('[') {
        return None;
    }
    let mut depth = 0i32;
    let mut groups = 0usize;
    for c in after.chars() {
        match c {
            '[' => {
                if depth == 0 {
                    groups += 1;
                }
                depth += 1;
            }
            ']' => {
                depth -= 1;
                if depth < 0 {
                    return None;
                }
            }
            _ if depth == 0 => return None,
            _ => {}
        }
    }
    if depth != 0 || groups < 2 {
        return None;
    }
    Some((letter, after))
}

pub fn detect(raw: &str, root: &CommandNode) -> Report {
    let mut findings = Vec::new();
    for line in raw.lines() {
        let Some((token, rest)) = leading_token(line) else {
            continue;
        };
        if !opens_description_column(rest) {
            continue;
        }
        let Some((letter, spelling)) = glued_optional_groups(token) else {
            continue;
        };
        let Some(entity) = root.flags().find(|e| e.short() == Some(letter)) else {
            continue;
        };
        if let Some(v) = &entity.value_name {
            if v != spelling {
                findings.push(Finding {
                    flag: letter,
                    value_name: v.clone(),
                    source_spelling: spelling.to_string(),
                    line: line.to_string(),
                });
            }
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

/// nvim's real row, byte-exact (`corpus/nvim/0.9.5/help.txt`).
pub(crate) const NVIM_V_ROW: &str = "  -V[N][file]           Verbose [level][file]\n";

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
            name: "vim.basic's own bytes, `-V`'s spelling reformatted",
            why: "the defect itself: the source spells the groups `[N][fname]`, glued; the tree \
                  carries the space-joined `N fname`",
            expect: Expect::Fires(1),
            raw: VIM_V_ROW.to_string(),
            root: node_with_flags("vim.basic", vec![short_flag_with_value('V', "N fname")]),
        },
        SelfCheck {
            name: "nvim's own bytes, `-V`'s spelling reformatted",
            why: "the same shape on a second tool",
            expect: Expect::Fires(1),
            raw: NVIM_V_ROW.to_string(),
            root: node_with_flags("nvim", vec![short_flag_with_value('V', "N file")]),
        },
        SelfCheck {
            name: "`-V`'s value already matches the source spelling",
            why: "once the value name is kept exactly as documented, the same raw row must go \
                  silent",
            expect: Expect::Silent,
            raw: VIM_V_ROW.to_string(),
            root: node_with_flags("vim.basic", vec![short_flag_with_value('V', "[N][fname]")]),
        },
        SelfCheck {
            name: "a single bracketed value, not two glued groups",
            why: "the token scan requires two or more glued groups; one bracket alone is an \
                  ordinary optional value and not this shape",
            expect: Expect::Silent,
            raw: "   -V[N]\t\tBe verbose [level N]\n".to_string(),
            root: node_with_flags("prog", vec![short_flag_with_value('V', "N")]),
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
