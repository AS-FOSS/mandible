//! `comma-glued-option-value` (atlas S-116): a single-dash spelling whose
//! first character is a letter, followed directly by one or more letters
//! and then a comma, reaches the tree truncated to its first character,
//! with the rest of the run and the comma folded into the value name
//! (`-Wa,<options>` becomes `-W` valued `"a,<options>"`).
//!
//! Fixtures: `corpus/gcc/13.3.0/` (`-Wa,<options>`, `-Wp,<options>`,
//! `-Wl,<options>`).

use crate::family_row::{leading_token, opens_description_column};
use mandible_core::{CommandNode, Dashes};

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

/// `-XY...,value`: a single dash, a run of two or more ASCII letters, a
/// comma directly glued to the run's end, then a non-empty tail that is
/// neither whitespace nor another dash — the same three-way guard
/// `mandible_extract::help_text::grammar::try_short` applies, so this
/// detector and the fix it measures can never disagree about scope.
/// Returns the run (the spelling this token *should* become).
fn comma_glued_run(token: &str) -> Option<&str> {
    let rest = token.strip_prefix('-')?;
    let comma = rest.find(',')?;
    let run = &rest[..comma];
    if run.chars().count() < 2 || !run.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let after = &rest[comma + 1..];
    if after.is_empty() || after.starts_with(['-', ' ', '\t']) {
        return None;
    }
    Some(run)
}

fn tree_has_single_dash_spelling(root: &CommandNode, name: &str) -> bool {
    root.flags().any(|e| {
        e.spellings
            .iter()
            .any(|s| s.dashes == Dashes::Single && s.name == name)
    })
}

pub fn detect(raw: &str, root: &CommandNode) -> Report {
    let mut seen = std::collections::BTreeSet::new();
    let mut findings = Vec::new();
    for line in raw.lines() {
        let Some((token, rest)) = leading_token(line) else {
            continue;
        };
        if !opens_description_column(rest) {
            continue;
        }
        let Some(run) = comma_glued_run(token) else {
            continue;
        };
        if !seen.insert(run.to_string()) {
            continue;
        }
        if !tree_has_single_dash_spelling(root, run) {
            findings.push(Finding {
                name: run.to_string(),
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

/// gcc/g++'s real rows, byte-exact (`corpus/gcc/13.3.0/help.txt`,
/// `corpus/aarch64-linux-gnu-g++-13`).
pub(crate) const GCC_WA_ROW: &str =
    "  -Wa,<options>            Pass comma-separated <options> on to the assembler.\n";

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
            name: "gcc's own bytes, `-Wa,<options>` truncated to `-W`",
            why: "the defect itself: the tree carries only a single-letter `-W`, never the whole \
                  `-Wa` run",
            expect: Expect::Fires(1),
            raw: GCC_WA_ROW.to_string(),
            root: node_with_flags("gcc", vec![single_dash_flag("W")]),
        },
        SelfCheck {
            name: "`-Wa` recovered as its own single-dash spelling",
            why: "once the tree carries the whole run, the same raw row must go silent",
            expect: Expect::Silent,
            raw: GCC_WA_ROW.to_string(),
            root: node_with_flags("gcc", vec![single_dash_flag("Wa")]),
        },
        SelfCheck {
            name: "the compiler glued-value convention, no comma (`-Idirectory`)",
            why: "no comma follows the run, so this must never be claimed — the convention this \
                  rule must not regress",
            expect: Expect::Silent,
            raw: "  -Idirectory              Add directory to include path.\n".to_string(),
            root: node_with_flags("gcc", vec![single_dash_flag("I")]),
        },
        SelfCheck {
            name: "a genuine alias separated by a comma and a space (`-es, -Es`)",
            why: "a real alias always has a space before its second spelling, so this must never \
                  be claimed as a comma-glued value",
            expect: Expect::Silent,
            raw: "  -es, -Es              Silent (batch) mode\n".to_string(),
            root: node_with_flags("nvim", vec![single_dash_flag("e")]),
        },
    ]
}
