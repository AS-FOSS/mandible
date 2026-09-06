//! `spaced-single-dash-long` (atlas S-117): a single-dash long option
//! with an uppercase flag letter and a spaced, not glued, value
//! (`-Xassembler <arg>`) truncated to its first character. Repaired by
//! `help_text::sections::repair::spaced_value_placeholder`.
//!
//! Requires exactly one space before an angle- or bracket-delimited
//! placeholder, never a wider gap: a wider gap (`ld`'s `-Bgroup`) is
//! indistinguishable from the glued-value convention's own row
//! (`-Dname`), an honest miss, not counted here.
//!
//! Fixtures: `corpus/aarch64-linux-gnu-g++-13/13.3.0/`.

use mandible_core::CommandNode;

pub struct Finding {
    pub name: String,
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

/// `-Xword <placeholder>`: a single dash, one uppercase letter, a run of
/// two or more lowercase letters, then exactly one space and an angle- or
/// bracket-delimited placeholder token. Returns the reconstructed name
/// (`"Xassembler"`). One space only, never a wider column gap — see this
/// module's doc comment for why a wider gap is out of scope.
fn spaced_single_dash_long(token: &str, rest: &str) -> Option<String> {
    let word = token.strip_prefix('-')?;
    let mut chars = word.chars();
    let first = chars.next()?;
    if !first.is_ascii_uppercase() {
        return None;
    }
    let tail: String = chars.collect();
    if tail.chars().count() < 2 || !tail.chars().all(|c| c.is_ascii_lowercase()) {
        return None;
    }
    let mut rest_chars = rest.chars();
    if rest_chars.next() != Some(' ') {
        return None;
    }
    let after_space = rest_chars.as_str();
    if after_space.starts_with([' ', '\t']) {
        return None;
    }
    let placeholder_shaped = after_space.starts_with('<') || after_space.starts_with('[');
    placeholder_shaped.then(|| word.to_string())
}

fn tree_has_single_dash_spelling(root: &CommandNode, name: &str) -> bool {
    root.flags().any(|e| {
        e.spellings
            .iter()
            .any(|s| s.dashes == mandible_core::Dashes::Single && s.name == name)
    })
}

pub fn detect(raw: &str, root: &CommandNode) -> Report {
    let mut seen = std::collections::BTreeSet::new();
    let mut findings = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim_start();
        if trimmed.len() == line.len() {
            continue; // no leading indentation: a heading, not a row
        }
        let Some(token) = trimmed.split_whitespace().next() else {
            continue;
        };
        let rest = &trimmed[token.len()..];
        let Some(name) = spaced_single_dash_long(token, rest) else {
            continue;
        };
        if !seen.insert(name.clone()) {
            continue;
        }
        if !tree_has_single_dash_spelling(root, &name) {
            findings.push(Finding {
                name,
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

/// g++'s real row, byte-exact (`corpus/aarch64-linux-gnu-g++-13/13.3.0/help.txt`).
pub(crate) const GXX_XASSEMBLER_ROW: &str =
    "  -Xassembler <arg>        Pass <arg> on to the assembler.\n";

fn single_dash_flag(name: &str) -> Entity {
    let mut e = Entity::flag_spelled(
        None,
        None,
        false,
        false,
        Provenance::single(Source::HelpText),
    );
    e.spellings = vec![mandible_core::Spelling::single_dash(name)];
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
            name: "g++'s own bytes, `-Xassembler` truncated to `-X`",
            why: "the defect itself: the tree carries only a single-letter `-X`, never the whole \
                  `Xassembler` name",
            expect: Expect::Fires(1),
            raw: GXX_XASSEMBLER_ROW.to_string(),
            root: node_with_flags("g++", vec![single_dash_flag("X")]),
        },
        SelfCheck {
            name: "`-Xassembler` recovered as its own single-dash spelling",
            why: "once the tree carries the whole name, the same raw row must go silent",
            expect: Expect::Silent,
            raw: GXX_XASSEMBLER_ROW.to_string(),
            root: node_with_flags("g++", vec![single_dash_flag("Xassembler")]),
        },
        SelfCheck {
            name: "the glued-value convention, no space before the value (`-DMACRO`)",
            why: "no space separates the flag from its value, so this must never be claimed — \
                  the convention this rule must not regress",
            expect: Expect::Silent,
            raw: "  -DMACRO                  Define MACRO.\n".to_string(),
            root: node_with_flags("gcc", vec![single_dash_flag("D")]),
        },
        SelfCheck {
            name: "ld's own row, a wide column gap before its description (`-Bgroup`)",
            why: "a whole column, not one space, separates the swallowed name from its own \
                  description — indistinguishable from the glued-value convention's row \
                  (`-Dname`), so this stays an honest miss, never claimed",
            expect: Expect::Silent,
            raw: "  -Bgroup                     Selects group name lookup rules for DSO\n"
                .to_string(),
            root: node_with_flags("ld", vec![single_dash_flag("B")]),
        },
    ]
}
