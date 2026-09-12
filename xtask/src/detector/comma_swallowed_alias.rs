//! `comma-swallowed-alias` (atlas S-160): a third single-dash-long
//! spelling after a valued long form reaches nothing, because the value
//! spec swallowed the alias run's own trailing comma
//! (`pod2text`'s `-w, --width=width, -width`, value read as `width,`).
//! Fixture: `corpus/pod2text/*/`.

use mandible_core::CommandNode;

pub struct Finding {
    pub value: String,
    pub missing: String,
    pub line: String,
}

pub struct Report {
    pub findings: Vec<Finding>,
}

/// A row whose leading alias run repeats a `-<letter>[, --long=value, ]`
/// shape ending in a value that itself carries a trailing comma, with one
/// more single-dash spelling right after it — the exact shape
/// `recover_comma_swallowed_alias` (`mandible-extract`) repairs.
fn parse_row(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim_start();
    let eq = trimmed.find('=')?;
    let after_eq = &trimmed[eq + 1..];
    let comma = after_eq.find(", -")?;
    let value = &after_eq[..comma];
    if value.is_empty() || value.contains(' ') {
        return None;
    }
    let rest = &after_eq[comma + 2..];
    let missing_len = rest[1..]
        .find([',', ' ', '\t'])
        .map_or(rest.len() - 1, |i| i + 1);
    let missing = &rest[..missing_len];
    Some((value.to_string(), missing.to_string()))
}

fn tree_has_single_dash(root: &CommandNode, name: &str) -> bool {
    root.flags().any(|e| {
        e.spellings
            .iter()
            .any(|s| s.dashes == mandible_core::Dashes::Single && s.name == name)
    })
}

pub fn detect(raw: &str, root: &CommandNode) -> Report {
    let mut findings = Vec::new();
    for line in raw.lines() {
        let Some((value, missing)) = parse_row(line) else {
            continue;
        };
        let name = missing.trim_start_matches('-');
        if !tree_has_single_dash(root, name) {
            findings.push(Finding {
                value,
                missing,
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

/// pod2text's real row, byte-exact (`pod2text --help`).
pub(crate) const POD2TEXT_WIDTH_ROW: &str = "    -w, --width=width, -width\n";

fn node_with_flags(flags: Vec<Entity>) -> CommandNode {
    let mut root = CommandNode::new("pod2text", Provenance::single(Source::HelpText));
    root.set_entities_of(mandible_core::EntityKind::Flag, flags);
    root
}

fn w_width_flag(value_name: &str) -> Entity {
    let mut e = Entity::flag_spelled(
        Some('w'),
        Some("width".to_string()),
        false,
        false,
        Provenance::single(Source::HelpText),
    );
    e.value_name = Some(value_name.to_string());
    e.value_kind = mandible_core::ValueKind::Required;
    e
}

fn dash_width_flag() -> Entity {
    let mut e = Entity::flag_spelled(
        None,
        None,
        false,
        false,
        Provenance::single(Source::HelpText),
    );
    e.spellings = vec![mandible_core::Spelling::single_dash("width")];
    e
}

pub struct CommaSwallowedAlias;

impl Detector for CommaSwallowedAlias {
    fn name(&self) -> &'static str {
        "comma-swallowed-alias"
    }

    fn family(&self) -> Option<&'static str> {
        None
    }

    fn describes(&self) -> &'static str {
        "a value spec swallowed the alias run's own trailing comma, so a third single-dash-long \
         spelling after a valued long form reaches nothing"
    }

    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        detect(evidence.raw, evidence.root)
            .findings
            .into_iter()
            .map(|f| {
                format!(
                    "{:?} never reached the tree, value read {:?}, from {:?}",
                    f.missing, f.value, f.line
                )
            })
            .collect()
    }

    fn self_checks(&self) -> Vec<SelfCheck> {
        vec![
            SelfCheck {
                name: "pod2text's own bytes, `-width` unreached",
                why: "the defect itself: the value read `width,` with the comma glued in, and \
                      `-width` never became its own spelling",
                expect: Expect::Fires(1),
                raw: POD2TEXT_WIDTH_ROW.to_string(),
                root: node_with_flags(vec![w_width_flag("width,")]),
            },
            SelfCheck {
                name: "the same row, correctly recovered",
                why: "once `-width` reaches the tree and the comma is stripped from the value, \
                      the same row must go silent",
                expect: Expect::Silent,
                raw: POD2TEXT_WIDTH_ROW.to_string(),
                root: node_with_flags(vec![w_width_flag("width"), dash_width_flag()]),
            },
        ]
    }
}
