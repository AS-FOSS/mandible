//! `spaced-bare-word-table-value` (atlas S-157): inside a single-dash-long
//! table (S-145, no `--long` row anywhere), a row's value name is a bare
//! word one space after the spelling, with no bracket or angle delimiter —
//! `-Xstrategy strategy1,...,strategyN`, `-audit int`. The value reads as
//! the first word of the description instead. Fixtures: `corpus/Xvfb/`,
//! `corpus/mksquashfs/4.6.1/`, `corpus/sqfstar/4.6.1/`.

use super::single_dash_long_table::{document_has_double_dash_row, single_dash_long_row};
use mandible_core::CommandNode;

pub struct Finding {
    pub name: String,
    pub value: String,
    pub line: String,
}

pub struct Report {
    pub findings: Vec<Finding>,
}

/// The one whitespace-delimited token one space after `trimmed`'s own
/// name — never a bracket or angle placeholder (`spaced_value_placeholder`'s
/// own shape), and never anything past that first token: a ragged
/// three-column table may separate it from the next column by only one
/// more space (`qemu-arm64-static`'s `-dfilter range[,...] QEMU_DFILTER`).
fn bare_word_value(trimmed: &str, name: &str) -> Option<String> {
    let rest = &trimmed[name.len() + 1..];
    let after = rest.strip_prefix(' ')?;
    if after.starts_with([' ', '\t', '<', '[']) {
        return None;
    }
    let value = after.split_whitespace().next()?;
    value
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric())
        .then(|| value.to_string())
}

fn flag_value_name(root: &CommandNode, name: &str) -> Option<String> {
    root.flags()
        .find(|e| {
            e.spellings
                .iter()
                .any(|s| s.dashes == mandible_core::Dashes::Single && s.name == name)
        })
        .and_then(|e| e.value_name.clone())
}

pub fn detect(raw: &str, root: &CommandNode) -> Report {
    if document_has_double_dash_row(raw) {
        return Report {
            findings: Vec::new(),
        };
    }
    let mut findings = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for line in raw.lines() {
        let trimmed = line.trim_start();
        let Some(name) = single_dash_long_row(trimmed) else {
            continue;
        };
        let Some(value) = bare_word_value(trimmed, &name) else {
            continue;
        };
        if !seen.insert(name.clone()) {
            continue;
        }
        let recovered = flag_value_name(root, &name).as_deref() == Some(value.as_str());
        if !recovered {
            findings.push(Finding {
                name,
                value,
                line: line.to_string(),
            });
        }
    }
    Report { findings }
}

// ----------------------------------------------------------------------
// Self-checks
// ----------------------------------------------------------------------

use crate::detector::{Detector, Expect, SelfCheck, ToolEvidence};
use mandible_core::{Entity, Provenance, Source};

/// Xvfb's real row, byte-exact (`Xvfb --help`).
pub(crate) const XVFB_AUDIT_ROW: &str = "-audit int             set audit trail level\n";
/// mksquashfs's real row, byte-exact (`corpus/mksquashfs/4.6.1/help.stderr.txt`).
pub(crate) const MKSQUASHFS_XSTRATEGY_ROW: &str =
    "-Xstrategy strategy1,...,strategyN\tcompression strategy\n";

fn single_dash_flag(name: &str, value_name: Option<&str>) -> Entity {
    let mut e = Entity::flag_spelled(
        None,
        None,
        false,
        false,
        Provenance::single(Source::HelpText),
    );
    e.spellings = vec![mandible_core::Spelling::single_dash(name)];
    e.value_name = value_name.map(str::to_string);
    if value_name.is_some() {
        e.value_kind = mandible_core::ValueKind::Required;
    }
    e
}

fn node_with_flags(flags: Vec<Entity>) -> CommandNode {
    let mut root = CommandNode::new("Xvfb", Provenance::single(Source::HelpText));
    root.set_entities_of(mandible_core::EntityKind::Flag, flags);
    root
}

pub struct SpacedBareWordTableValue;

impl Detector for SpacedBareWordTableValue {
    fn name(&self) -> &'static str {
        "spaced-bare-word-table-value"
    }

    fn family(&self) -> Option<&'static str> {
        None
    }

    fn describes(&self) -> &'static str {
        "inside a single-dash-long table, a row's bare-word value name (no bracket, no angle \
         placeholder) is lost, its first word absorbed into the description"
    }

    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        detect(evidence.raw, evidence.root)
            .findings
            .into_iter()
            .map(|f| {
                format!(
                    "-{} never carried value {:?}, from {:?}",
                    f.name, f.value, f.line
                )
            })
            .collect()
    }

    fn self_checks(&self) -> Vec<SelfCheck> {
        vec![
            SelfCheck {
                name: "Xvfb's own bytes, `-audit` with no value name",
                why: "the defect itself: the table row's bare-word value never reached the tree",
                expect: Expect::Fires(1),
                raw: XVFB_AUDIT_ROW.to_string(),
                root: node_with_flags(vec![single_dash_flag("audit", None)]),
            },
            SelfCheck {
                name: "`-audit` recovered with value `int`",
                why: "once the tree carries the row's own bare-word value, the same row must go \
                      silent",
                expect: Expect::Silent,
                raw: XVFB_AUDIT_ROW.to_string(),
                root: node_with_flags(vec![single_dash_flag("audit", Some("int"))]),
            },
            SelfCheck {
                name: "mksquashfs's `-Xstrategy`, a comma-separated value list",
                why: "the value name is a comma run, not one bare word, and must be kept whole \
                      rather than cut at the first comma",
                expect: Expect::Fires(1),
                raw: MKSQUASHFS_XSTRATEGY_ROW.to_string(),
                root: node_with_flags(vec![single_dash_flag("Xstrategy", None)]),
            },
            SelfCheck {
                name: "gcc's own document, `-DMACRO`'s row beside a real `--help` row",
                why: "a document that documents even one `--long` row anywhere is never a \
                      table, so a bare description word is never mistaken for a value here",
                expect: Expect::Silent,
                raw: "-Doption value        Enable the option.\n\
                       --help                Display this information.\n"
                    .to_string(),
                root: node_with_flags(vec![single_dash_flag("Doption", None)]),
            },
        ]
    }
}
