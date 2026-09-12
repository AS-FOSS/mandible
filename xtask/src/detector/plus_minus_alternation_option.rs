//! `plus-minus-alternation-option` (atlas S-163): a `+/-word`, `-/+word`,
//! `[+-]word` or `[-+]word` alternation-sigil row names two flags,
//! `+word` and `-word`, on one row. Anchored to the row's own leading
//! token, the same evidence
//! `mandible_extract::help_text::sections::flag_rows::plus_minus_alternation_word`
//! requires, so `xxd`'s `-s [+][-]seek` (whose leading token is `-s`, not
//! one of these four sigils) is never in scope — checked independently
//! here since a detector reads only `raw`+`root`.

use crate::detector::{Detector, Expect, Scope, SelfCheck, ToolEvidence};
use mandible_core::{CommandNode, Provenance, Source};

/// [`crate::family_row::leading_token`] without its `trimmed == line`
/// indentation requirement — a headingless table (`Xvfb`'s own `+/-render`
/// and `[+-]accessx` rows, S-165) carries no indentation at all. Kept
/// local rather than widening the shared helper, since other detectors
/// read it. See `plus_word_option.rs`'s own copy of this same fix.
fn leading_token(line: &str) -> Option<(&str, &str)> {
    let trimmed = line.trim_start();
    if trimmed.is_empty() {
        return None;
    }
    let token = trimmed.split_whitespace().next()?;
    let rest = &trimmed[token.len()..];
    Some((token, rest))
}

/// The base word right after a `+/-`/`-/+`/`[+-]`/`[-+]` alternation
/// sigil at the very start of `token`, when a letter-led run of
/// letters/digits/`-` follows immediately. `None` for every other token.
fn alternation_word(token: &str) -> Option<&str> {
    let rest = token
        .strip_prefix("+/-")
        .or_else(|| token.strip_prefix("-/+"))
        .or_else(|| token.strip_prefix("[+-]"))
        .or_else(|| token.strip_prefix("[-+]"))?;
    let word_end = rest
        .char_indices()
        .find(|(_, c)| !(c.is_ascii_alphanumeric() || *c == '-'))
        .map_or(rest.len(), |(i, _)| i);
    if word_end == 0 || !rest.starts_with(|c: char| c.is_ascii_alphabetic()) {
        return None;
    }
    Some(&rest[..word_end])
}

/// `spelling` is compared against [`mandible_core::Spelling::typed`], not
/// the raw `.name` field: a `-word` spelling stores its dash separately
/// (`Dashes::Single`, `.name == "word"`), while a `+word` spelling stores
/// the sigil inside `.name` itself (`Dashes::None`, S-163's own
/// convention) — `.typed()` renders both the same way a user would type
/// them, which is the only form this detector's own `plus`/`minus`
/// spellings above are built to match.
fn tree_has_spelling(root: &CommandNode, spelling: &str) -> bool {
    root.flags()
        .any(|e| e.spellings.iter().any(|s| s.typed() == spelling))
}

pub struct PlusMinusAlternationOption;

impl Detector for PlusMinusAlternationOption {
    fn name(&self) -> &'static str {
        "plus-minus-alternation-option"
    }

    fn family(&self) -> Option<&'static str> {
        None
    }

    fn describes(&self) -> &'static str {
        "a `+/-word`/`[+-]word` alternation-sigil row whose `+word` and `-word` pair does not \
         both reach the tree"
    }

    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        let mut findings = Vec::new();
        for line in evidence.raw.lines() {
            let Some((token, _)) = leading_token(line) else {
                continue;
            };
            let Some(word) = alternation_word(token) else {
                continue;
            };
            let plus = format!("+{word}");
            let minus = format!("-{word}");
            let plus_ok = tree_has_spelling(evidence.root, &plus);
            let minus_ok = tree_has_spelling(evidence.root, &minus);
            if !plus_ok || !minus_ok {
                findings.push(format!(
                    "{token:?} did not expand to both {plus:?} and {minus:?} (have {plus}={plus_ok}, {minus}={minus_ok})"
                ));
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
        fn flag(spelling: mandible_core::Spelling) -> mandible_core::Entity {
            let mut e = mandible_core::Entity::new(
                mandible_core::EntityKind::Flag,
                Provenance::single(Source::HelpText),
            );
            e.spellings.push(spelling);
            e
        }
        fn plus(word: &str) -> mandible_core::Entity {
            flag(mandible_core::Spelling::bare(format!("+{word}")))
        }
        fn minus(word: &str) -> mandible_core::Entity {
            flag(mandible_core::Spelling::single_dash(word))
        }

        let render_raw =
            "  +/-render\t\t   turn on/off RENDER extension support(default on)\n".to_string();
        let accessx_raw = "  [+-]accessx [ timeout [ ttb [ tpo [ ctrls ]]]] enable/disable \
                            accessx\n"
            .to_string();

        vec![
            SelfCheck {
                name: "Xvfb's own `+/-render` row, neither half recovered",
                why: "the defect itself: the row names two flags and the tree has neither",
                expect: Expect::Fires(1),
                raw: render_raw.clone(),
                root: node_with_flags("Xvfb", vec![]),
            },
            SelfCheck {
                name: "the same row, only `-render` recovered",
                why: "one half missing is still a loss, not a silence",
                expect: Expect::Fires(1),
                raw: render_raw.clone(),
                root: node_with_flags("Xvfb", vec![minus("render")]),
            },
            SelfCheck {
                name: "the same row, both `+render` and `-render` recovered",
                why: "once both halves reach the tree, the row goes silent",
                expect: Expect::Silent,
                raw: render_raw,
                root: node_with_flags("Xvfb", vec![plus("render"), minus("render")]),
            },
            SelfCheck {
                name: "Xvfb's own `[+-]accessx` row, neither half recovered",
                why: "the bracketed sigil order names the same pair",
                expect: Expect::Fires(1),
                raw: accessx_raw.clone(),
                root: node_with_flags("Xvfb", vec![]),
            },
            SelfCheck {
                name: "the same bracketed row, both halves recovered",
                why: "once both halves reach the tree, the row goes silent",
                expect: Expect::Silent,
                raw: accessx_raw,
                root: node_with_flags("Xvfb", vec![plus("accessx"), minus("accessx")]),
            },
            SelfCheck {
                name: "xxd's own `-s [+][-]seek` row, the named counter-case",
                why: "the leading token is `-s`, not one of the four alternation sigils, and \
                      must never fire — S-097 already refuses to fold this shape",
                expect: Expect::Silent,
                raw: "  -s [+][-]seek        seek offset (base is 16, +/- prefix optional)\n"
                    .to_string(),
                root: node_with_flags("xxd", vec![]),
            },
            SelfCheck {
                name: "an ordinary `-word` row, no sigil at all",
                why: "a plain dash-led row carries no alternation sigil and must never fire",
                expect: Expect::Silent,
                raw: "  -render               set render color alloc policy\n".to_string(),
                root: node_with_flags("Xvfb", vec![minus("render")]),
            },
        ]
    }
}
