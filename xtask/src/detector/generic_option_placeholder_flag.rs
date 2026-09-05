//! `generic-option-placeholder-flag` (atlas S-128): a usage line's own
//! generic placeholder token, spelled WITH a leading dash glued onto the
//! word (`[-options]`), is read literally as an invented short flag —
//! `makeconv`'s `[-options] files...` becomes a fabricated `-o ptions`.
//!
//! A local, independent copy of the placeholder-word check
//! `is_option_list_placeholder` makes, not an import: an oracle built on
//! the parser's own helper would agree with it by construction. Fixture:
//! `corpus/makeconv/6.2`.

use mandible_core::CommandNode;

pub struct Finding {
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

const PLACEHOLDER_WORDS: &[&str] = &["option", "options", "opts"];

fn is_option_list_placeholder(word: &str) -> bool {
    PLACEHOLDER_WORDS
        .iter()
        .any(|p| word.eq_ignore_ascii_case(p))
}

/// True when `token` (already stripped of any wrapping `[`/`]`) is a
/// single dash directly glued onto a placeholder word: `-options`,
/// `-option`, `-opts`.
fn is_dash_prefixed_placeholder(token: &str) -> bool {
    token
        .strip_prefix('-')
        .is_some_and(is_option_list_placeholder)
}

pub fn detect(raw: &str, root: &CommandNode) -> Report {
    let mut findings = Vec::new();
    for line in raw.lines() {
        let lower = line.to_ascii_lowercase();
        if !lower.trim_start().starts_with("usage:") && !lower.contains("usage:") {
            continue;
        }
        for raw_token in line.split_whitespace() {
            let stripped = raw_token.trim_matches(|c| c == '[' || c == ']');
            if !is_dash_prefixed_placeholder(stripped) {
                continue;
            }
            // The invented flag this shape produces if read literally:
            // the second character becomes the short spelling, the rest
            // becomes a glued value name.
            let mut chars = stripped.chars();
            chars.next(); // '-'
            let Some(short) = chars.next() else {
                continue;
            };
            let value: String = chars.collect();
            let invented = root.flags().any(|f| {
                f.short() == Some(short) && f.value_name.as_deref() == Some(value.as_str())
            });
            if invented {
                findings.push(Finding {
                    token: raw_token.to_string(),
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

pub(crate) const MAKECONV_USAGE: &str = "usage: makeconv [-options] files...\n";

fn node_with_flag(name: &str, short: char, value: &str) -> CommandNode {
    let mut root = CommandNode::new(name, Provenance::single(Source::HelpText));
    let mut flag = Entity::flag_spelled(
        Some(short),
        None,
        false,
        false,
        Provenance::single(Source::HelpText),
    );
    flag.value_name = Some(value.to_string());
    root.set_entities_of(mandible_core::EntityKind::Flag, vec![flag]);
    root
}

pub(crate) fn self_checks() -> Vec<SelfCheck> {
    vec![
        SelfCheck {
            name: "makeconv's own bytes, the invented -o ptions flag",
            why: "the defect itself: `[-options]` read literally invents `-o` with glued value \
                  `ptions`",
            expect: Expect::Fires(1),
            raw: MAKECONV_USAGE.to_string(),
            root: node_with_flag("makeconv", 'o', "ptions"),
        },
        SelfCheck {
            name: "the placeholder correctly dropped, no invented flag",
            why: "once the fix lands, the same usage line must produce no `-o` at all, so this \
                  must go silent",
            expect: Expect::Silent,
            raw: MAKECONV_USAGE.to_string(),
            root: CommandNode::new("makeconv", Provenance::single(Source::HelpText)),
        },
        SelfCheck {
            name: "a tool with a real -o flag taking a value",
            why: "a genuine short flag must never be mistaken for the placeholder just because \
                  it shares a usage line with unrelated bracket groups",
            expect: Expect::Silent,
            raw: "usage: prog [-o OUTPUT] files...\n".to_string(),
            root: node_with_flag("prog", 'o', "OUTPUT"),
        },
    ]
}
