//! `hash-in-spelling` (atlas S-118): gcc's `-###` reaches the tree as `-#`
//! with a fabricated value `"##"`, since the spelling grammar used to stop
//! at the first `#` and take only one character.
//!
//! Fixtures: `corpus/gcc/13.3.0/`, `corpus/aarch64-linux-gnu-g++-13/`
//! (`-###`).

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

/// `-##...`: a single dash, then a run of two or more `#` and nothing
/// else in the token. Returns the run (the spelling this token *should*
/// become).
fn hash_run(token: &str) -> Option<&str> {
    let rest = token.strip_prefix('-')?;
    if rest.chars().count() < 2 || !rest.chars().all(|c| c == '#') {
        return None;
    }
    Some(rest)
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
        let Some(run) = hash_run(token) else {
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

/// gcc/g++'s real row, byte-exact (`corpus/gcc/13.3.0/help.txt`).
pub(crate) const GCC_HASH_ROW: &str =
    "  -###                     Like -v but options quoted and commands not executed.\n";

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
            name: "gcc's own bytes, `-###` truncated to `-#`",
            why: "the defect itself: the tree carries only a single-letter `-#`, never the whole \
                  `###` run",
            expect: Expect::Fires(1),
            raw: GCC_HASH_ROW.to_string(),
            root: node_with_flags("gcc", vec![single_dash_flag("#")]),
        },
        SelfCheck {
            name: "`-###` recovered as its own single-dash spelling",
            why: "once the tree carries the whole run, the same raw row must go silent",
            expect: Expect::Silent,
            raw: GCC_HASH_ROW.to_string(),
            root: node_with_flags("gcc", vec![single_dash_flag("###")]),
        },
        SelfCheck {
            name: "an ordinary short flag with no `#` at all",
            why: "the token scan requires the run to be nothing but `#`, so a plain flag must \
                  never be claimed",
            expect: Expect::Silent,
            raw: "  -v                        Verbose.\n".to_string(),
            root: node_with_flags("gcc", vec![single_dash_flag("v")]),
        },
    ]
}
