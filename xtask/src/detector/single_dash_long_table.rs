//! `single-dash-long-table` (atlas S-145): a document whose option rows
//! are single-dash long spellings, tab- or column-gap separated from
//! their descriptions, carrying no `--long` row anywhere.
//! Fixtures: `corpus/mksquashfs/4.6.1/`, `corpus/sqfstar/4.6.1/`.

// A qualifying table admits every `-word` row's full token, with no
// two-character floor and no lowercase-only gate. The GCC and Clang
// glued-value convention always documents a `--long` row somewhere, so
// its own tables never qualify. That is the whole safety argument.
// This rule subsumes S-139 and widens S-117 for a qualifying document.

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

/// True when some physical line's leading token, after trimming
/// indentation, is a `--word` spelling — disqualifies the whole document
/// as a single-dash-long table.
fn document_has_double_dash_row(raw: &str) -> bool {
    raw.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed
            .strip_prefix("--")
            .is_some_and(|rest| rest.starts_with(|c: char| c.is_alphanumeric()))
    })
}

/// The bare name (dash stripped) of a single-dash long row: `trimmed`
/// opens with exactly one dash, a run of name-shaped characters, and then
/// either nothing, whitespace, or a glued `=value`. Never a `--long` row
/// (excluded by the caller's own document-level check, and defensively
/// here too) and never a bare one-character short flag, which carries no
/// table-rule information of its own.
fn single_dash_long_row(trimmed: &str) -> Option<String> {
    let rest = trimmed.strip_prefix('-')?;
    if rest.is_empty() || rest.starts_with('-') {
        return None;
    }
    let name_len = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .count();
    if name_len < 2 {
        return None;
    }
    let name: String = rest.chars().take(name_len).collect();
    let after: String = rest.chars().skip(name_len).collect();
    let row_shaped = after.is_empty() || after.starts_with(['\t', ' ', '=']);
    row_shaped.then_some(name)
}

fn tree_has_single_dash_spelling(root: &CommandNode, name: &str) -> bool {
    root.flags().any(|e| {
        e.spellings
            .iter()
            .any(|s| s.dashes == mandible_core::Dashes::Single && s.name == name)
    })
}

pub fn detect(raw: &str, root: &CommandNode) -> Report {
    if document_has_double_dash_row(raw) {
        return Report {
            findings: Vec::new(),
        };
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut findings = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim_start();
        let Some(name) = single_dash_long_row(trimmed) else {
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

/// mksquashfs's real rows, byte-exact (`corpus/mksquashfs/4.6.1/help.stderr.txt`).
pub(crate) const MKSQUASHFS_PF_ROW: &str =
    "-pf <pseudo-file>\tadd list of pseudo file definitions from <pseudo-file>\n";
pub(crate) const MKSQUASHFS_XHELP_ROW: &str =
    "-Xhelp\t\t\tprint compressor options for selected compressor\n";

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
            name: "mksquashfs's own bytes, `-pf` truncated to `-p` valued `f`",
            why: "the defect itself: a one-character swallowed tail the ordinary floor refuses, \
                  admitted here because the whole document qualifies as a table",
            expect: Expect::Fires(1),
            raw: MKSQUASHFS_PF_ROW.to_string(),
            root: node_with_flags("mksquashfs", vec![single_dash_flag("p")]),
        },
        SelfCheck {
            name: "`-pf` recovered as its own single-dash spelling",
            why: "once the tree carries the whole name, the same raw row must go silent",
            expect: Expect::Silent,
            raw: MKSQUASHFS_PF_ROW.to_string(),
            root: node_with_flags("mksquashfs", vec![single_dash_flag("pf")]),
        },
        SelfCheck {
            name: "mksquashfs's `-Xhelp`, an uppercase-led boolean row",
            why: "the table rule admits a row whatever its case, unlike the narrower rules it \
                  replaces",
            expect: Expect::Fires(1),
            raw: MKSQUASHFS_XHELP_ROW.to_string(),
            root: node_with_flags("mksquashfs", vec![single_dash_flag("X")]),
        },
        SelfCheck {
            name: "gcc's own document, `-Xassembler` beside a real `--help` row",
            why: "a document that documents even one `--long` row anywhere is never a table — \
                  the whole safety argument against the GCC/Clang glued-value convention",
            expect: Expect::Silent,
            raw: "-Xassembler <arg>        Pass <arg> on to the assembler.\n\
                   --help                   Display this information.\n"
                .to_string(),
            root: node_with_flags("gcc", vec![single_dash_flag("X")]),
        },
    ]
}
