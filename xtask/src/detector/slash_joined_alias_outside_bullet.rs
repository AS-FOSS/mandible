//! `slash-joined-alias-outside-bullet` (atlas S-161): a `/`-joined second
//! spelling with a space on each side (`cargo-clippy`'s `-W / --warn
//! [LINT]`) reaches the tree with the first spelling valued `/` and the
//! second spelling reaching nothing, four times. Fixture:
//! `corpus/cargo-clippy/*/`.

use mandible_core::CommandNode;

pub struct Finding {
    pub short: String,
    pub long: String,
    pub line: String,
}

pub struct Report {
    pub findings: Vec<Finding>,
}

/// `-X / --long ...`: a short spelling, one space, a literal `/`, one
/// space, a `--long` spelling — the exact row shape
/// `mandible_extract::help_text::sections::bullets::join_slash_alias`
/// repairs.
fn parse_row(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim_start();
    let short_end = trimmed.find(' ')?;
    let (short, rest) = trimmed.split_at(short_end);
    if !short.starts_with('-') || short.starts_with("--") {
        return None;
    }
    let rest = rest.strip_prefix(" / ")?;
    if !rest.starts_with("--") {
        return None;
    }
    let long_end = rest.find(|c: char| c.is_whitespace()).unwrap_or(rest.len());
    Some((short.to_string(), rest[..long_end].to_string()))
}

fn tree_has_long(root: &CommandNode, name: &str) -> bool {
    root.flags().any(|e| e.long() == Some(name))
}

pub fn detect(raw: &str, root: &CommandNode) -> Report {
    let mut findings = Vec::new();
    for line in raw.lines() {
        let Some((short, long)) = parse_row(line) else {
            continue;
        };
        let long_name = long.trim_start_matches('-');
        if !tree_has_long(root, long_name) {
            findings.push(Finding {
                short,
                long,
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

/// cargo-clippy's real row, byte-exact (`cargo-clippy --help`).
pub(crate) const CLIPPY_WARN_ROW: &str = "    -W / --warn [LINT]       Set lint warnings\n";

fn node_with_flags(flags: Vec<Entity>) -> CommandNode {
    let mut root = CommandNode::new("cargo-clippy", Provenance::single(Source::HelpText));
    root.set_entities_of(mandible_core::EntityKind::Flag, flags);
    root
}

fn short_flag_valued_slash() -> Entity {
    let mut e = Entity::flag_spelled(
        Some('W'),
        None,
        false,
        false,
        Provenance::single(Source::HelpText),
    );
    e.value_name = Some("/".to_string());
    e.value_kind = mandible_core::ValueKind::Required;
    e
}

fn short_long_alias() -> Entity {
    Entity::flag_spelled(
        Some('W'),
        Some("warn".to_string()),
        false,
        false,
        Provenance::single(Source::HelpText),
    )
}

pub struct SlashJoinedAliasOutsideBullet;

impl Detector for SlashJoinedAliasOutsideBullet {
    fn name(&self) -> &'static str {
        "slash-joined-alias-outside-bullet"
    }

    fn family(&self) -> Option<&'static str> {
        None
    }

    fn describes(&self) -> &'static str {
        "a `/`-joined second spelling with a space on each side reaches nothing, the first \
         spelling keeping `/` as a fabricated value"
    }

    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        detect(evidence.raw, evidence.root)
            .findings
            .into_iter()
            .map(|f| format!("{:?} never joined {:?}, from {:?}", f.short, f.long, f.line))
            .collect()
    }

    fn self_checks(&self) -> Vec<SelfCheck> {
        vec![
            SelfCheck {
                name: "cargo-clippy's own bytes, `-W` fabricating value `/`",
                why: "the defect itself: `--warn` never reaches the tree and `-W` keeps the \
                      literal separator as its value",
                expect: Expect::Fires(1),
                raw: CLIPPY_WARN_ROW.to_string(),
                root: node_with_flags(vec![short_flag_valued_slash()]),
            },
            SelfCheck {
                name: "the same row, correctly joined",
                why: "once both spellings are one entity, the same row must go silent",
                expect: Expect::Silent,
                raw: CLIPPY_WARN_ROW.to_string(),
                root: node_with_flags(vec![short_long_alias()]),
            },
        ]
    }
}
