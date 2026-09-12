//! `plus-word-option` (atlas S-163): a `+word` option row (`+bs`, `+i`,
//! `+s`) — a letter-led run after the sigil, distinct from S-095's own
//! `plus-prefixed-option` (bare `+`/`+<placeholder>` only) — reaches no
//! entity in the tree. Requires indentation, the same evidence
//! `mandible_core::family_row::leading_token` and the parser's own
//! `scan_flags_block` both require, so an unindented specimen (`Xvfb`'s
//! own column-0 rows) is out of this family's reach until S-165's
//! headingless-table defect is fixed.
//!
//! A separate detector rather than a widening of `plus-prefixed-option`:
//! that family is already `REPAIRED` and ratchet-gated at zero
//! (docs/shapes.md S-095), so folding a still-open shape into it would
//! break the gate on tools this fix has not reached. Mirrors
//! `mandible_extract::help_text::sections::flag_rows::is_claimed_plus_token`'s
//! `+word` arm, checked independently here since a detector reads only
//! `raw`+`root`.

use crate::detector::{Detector, Expect, Scope, SelfCheck, ToolEvidence};
use crate::family_row::{leading_token, opens_description_column};
use mandible_core::{CommandNode, Provenance, Source};

/// True when `token` is a `+word` spelling this family claims: `+`
/// followed by a run opening with a letter, every later character
/// alphanumeric or `-` — never a bare `+`, a bracketed placeholder
/// (S-095's own claim), or a token with no real letter run at all.
fn is_plus_word_token(token: &str) -> bool {
    let Some(rest) = token.strip_prefix('+') else {
        return false;
    };
    let mut chars = rest.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => {
            chars.all(|c| c.is_ascii_alphanumeric() || c == '-')
        }
        _ => false,
    }
}

fn tree_has_spelling(root: &CommandNode, token: &str) -> bool {
    root.flags()
        .any(|e| e.spellings.iter().any(|s| s.name == token))
}

/// True when `line`'s own leading token is flag-shaped evidence: a real
/// `-`-prefixed flag, or this same family's own `+word` claim.
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
    is_plus_word_token(token)
}

fn has_flag_shaped_neighbor(lines: &[&str], i: usize) -> bool {
    let above = lines[..i].iter().rev().find(|l| !l.trim().is_empty());
    let below = lines[i + 1..].iter().find(|l| !l.trim().is_empty());
    above.is_some_and(|l| is_flag_shaped_neighbor(l))
        || below.is_some_and(|l| is_flag_shaped_neighbor(l))
}

pub struct PlusWordOption;

impl Detector for PlusWordOption {
    fn name(&self) -> &'static str {
        "plus-word-option"
    }

    fn family(&self) -> Option<&'static str> {
        None
    }

    fn describes(&self) -> &'static str {
        "an indented `+word` option row, opening a real description column beside a flag-shaped \
         neighbor, with no entity anywhere spelled that exact token"
    }

    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        let mut findings = Vec::new();
        let lines: Vec<&str> = evidence.raw.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            let Some((token, rest)) = leading_token(line) else {
                continue;
            };
            let token = token.trim_end_matches(',');
            if !is_plus_word_token(token) || !opens_description_column(rest) {
                continue;
            }
            if !has_flag_shaped_neighbor(&lines, i) {
                continue;
            }
            if !tree_has_spelling(evidence.root, token) {
                findings.push(format!("{token:?} never became a flag, from {line:?}"));
            }
        }
        findings
    }

    fn scope(&self) -> Scope {
        Scope::full()
    }

    fn self_checks(&self) -> Vec<SelfCheck> {
        fn node_with_flags(name: &str, flags: Vec<mandible_core::Entity>) -> CommandNode {
            let mut root = CommandNode::new(name, Provenance::single(Source::HelpText));
            root.set_entities_of(mandible_core::EntityKind::Flag, flags);
            root
        }
        fn plus_word_flag(word: &str) -> mandible_core::Entity {
            let mut e = mandible_core::Entity::new(
                mandible_core::EntityKind::Flag,
                Provenance::single(Source::HelpText),
            );
            e.spellings
                .push(mandible_core::Spelling::bare(format!("+{word}")));
            e
        }

        let fzf_raw = "    -i                     Case-insensitive match (default: smart-case \
                        match)\n    +i                     Case-sensitive match\n"
            .to_string();
        // `Xvfb`'s own row shape (multi-letter, `-word` neighbor) at its
        // own indent (two spaces) — column 0, `Xvfb`'s real indentation,
        // carries no evidence at all for `leading_token` (S-165's own
        // headingless-table defect, not this family's to fix).
        let indented_multiletter_raw = "  -br                  create root window with black \
                                          background\n  +bs                  enable any backing \
                                          store support\n  -bs                  disable any \
                                          backing store support\n"
            .to_string();

        vec![
            SelfCheck {
                name: "fzf's own bytes, `+i` dropped",
                why: "the defect itself: `+i`'s leading token is no entity's spelling",
                expect: Expect::Fires(1),
                raw: fzf_raw.clone(),
                root: node_with_flags("fzf", vec![]),
            },
            SelfCheck {
                name: "fzf's own bytes, `+i` recovered as its own spelling",
                why: "once the tree carries an entity spelled `+i`, the same raw row goes silent",
                expect: Expect::Silent,
                raw: fzf_raw,
                root: node_with_flags("fzf", vec![plus_word_flag("i")]),
            },
            SelfCheck {
                name: "an indented multi-letter `+word` row, `Xvfb`'s own spelling shape",
                why: "the multi-letter shape, beside a real `-`-prefixed neighbor, at a real \
                      indent",
                expect: Expect::Fires(1),
                raw: indented_multiletter_raw.clone(),
                root: node_with_flags("Xvfb", vec![]),
            },
            SelfCheck {
                name: "the same row recovered as its own spelling",
                why: "once recovered, the same raw row goes silent",
                expect: Expect::Silent,
                raw: indented_multiletter_raw,
                root: node_with_flags("Xvfb", vec![plus_word_flag("bs")]),
            },
            SelfCheck {
                name: "a bare `+` row, S-095's own claim, not this family's",
                why: "a bare `+` has no letter run at all and must never be claimed here",
                expect: Expect::Silent,
                raw: "  +                     Start at end of file\n".to_string(),
                root: node_with_flags("vim.basic", vec![]),
            },
            SelfCheck {
                name: "a `+<placeholder>` row, S-095's own claim, not this family's",
                why: "a bracketed placeholder is S-095's own shape, distinct from a plain word",
                expect: Expect::Silent,
                raw: "  +<lnum>\t\tStart at line <lnum>\n".to_string(),
                root: node_with_flags("vim.basic", vec![]),
            },
            SelfCheck {
                name: "an unindented `+word`-led heading line, no flag-shaped neighbor",
                why: "no flag-shaped neighbor is present, so the positive evidence this family \
                      requires is absent",
                expect: Expect::Silent,
                raw: "+bs this is not a table row\n".to_string(),
                root: node_with_flags("prog", vec![]),
            },
        ]
    }
}
