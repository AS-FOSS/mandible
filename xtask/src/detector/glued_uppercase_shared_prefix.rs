//! `glued-uppercase-shared-prefix` (atlas S-139): a single-dash token with
//! a lowercase-led name carrying an interior uppercase letter, glued
//! straight to a tab- or column-gapped description with no value at all
//! (mksquashfs's `-noI`, tab-separated from "do not compress inode
//! table").
//!
//! Not repaired: a candidate discriminator (a name sharing its
//! lowercase-led prefix with a sibling row) was measured and moved only
//! `mksquashfs` and `sqfstar`, two tools on a full-`PATH` sweep, below
//! the five-tool floor AGENTS.md §3.1 requires. This detector stays as
//! the instrument; the fixture stays `[xfail]` with the count in its
//! reason. See docs/shapes.md S-139.
//!
//! Deliberately not gated on exactly one space, unlike
//! `spaced-single-dash-long` (atlas S-117): that shape's value sits one
//! space after the name, this one has no value at all.
//!
//! Fixtures: `corpus/mksquashfs/4.6.1/`.

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

/// `-nameUpper`: a single dash, a lowercase-led run of ASCII alphanumerics
/// carrying at least one interior uppercase letter, glued straight to a
/// tab or a run of two or more spaces — never exactly one space, which is
/// `spaced-single-dash-long`'s own shape (an actual spaced value, not a
/// bare tab gap). Returns the bare name (`"noI"`).
fn glued_uppercase_row(token: &str, rest: &str) -> Option<String> {
    let name = token.strip_prefix('-')?;
    let mut chars = name.chars();
    let first = chars.next()?;
    if !first.is_ascii_lowercase() {
        return None;
    }
    if name.chars().count() < 2 || !name.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    if !name.chars().any(|c| c.is_ascii_uppercase()) {
        return None;
    }
    let mut rest_chars = rest.chars();
    let gapped = match rest_chars.next() {
        Some('\t') => true,
        Some(' ') => rest_chars.next() == Some(' '),
        _ => false,
    };
    gapped.then(|| name.to_string())
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
        let Some(token) = line.split_whitespace().next() else {
            continue;
        };
        let Some(offset) = line.find(token) else {
            continue;
        };
        let rest = &line[offset + token.len()..];
        let Some(name) = glued_uppercase_row(token, rest) else {
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

/// mksquashfs's real row, byte-exact (`corpus/mksquashfs/4.6.1/help.stderr.txt`).
pub(crate) const MKSQUASHFS_NOI_ROW: &str = "-noI\t\t\tdo not compress inode table\n";
/// mksquashfs's sibling row, sharing `-noI`'s own `\"no\"` prefix.
pub(crate) const MKSQUASHFS_NOD_ROW: &str = "-noD\t\t\tdo not compress data blocks\n";

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
            name: "mksquashfs's own bytes, `-noI` truncated to `-n`",
            why: "the defect itself: the tree carries only a single-letter `-n`, never the whole \
                  `noI` name",
            expect: Expect::Fires(1),
            raw: MKSQUASHFS_NOI_ROW.to_string(),
            root: node_with_flags("mksquashfs", vec![single_dash_flag("n")]),
        },
        SelfCheck {
            name: "`-noI` recovered as its own single-dash spelling",
            why: "once the tree carries the whole name, the same raw row must go silent",
            expect: Expect::Silent,
            raw: format!("{MKSQUASHFS_NOI_ROW}{MKSQUASHFS_NOD_ROW}"),
            root: node_with_flags("mksquashfs", vec![single_dash_flag("noI"), single_dash_flag("noD")]),
        },
        SelfCheck {
            name: "the glued-value convention, an uppercase flag letter with no interior letter \
                   (`-D`)",
            why: "a single-letter short flag never carries an interior letter at all, so this \
                  must never be claimed",
            expect: Expect::Silent,
            raw: "-D\t\t\tDefine a macro.\n".to_string(),
            root: node_with_flags("gcc", vec![single_dash_flag("D")]),
        },
        SelfCheck {
            name: "a single space, `spaced-single-dash-long`'s own shape",
            why: "a value spaced one column after the name is atlas S-117's shape, not this one",
            expect: Expect::Silent,
            raw: "-Xassembler <arg>        Pass <arg> on to the assembler.\n".to_string(),
            root: node_with_flags("gcc", vec![single_dash_flag("X")]),
        },
    ]
}
