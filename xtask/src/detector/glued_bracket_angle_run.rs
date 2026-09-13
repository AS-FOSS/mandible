//! `glued-bracket-angle-run` (atlas S-158): a value spec that mixes a
//! bracket-optional group and a required angle placeholder, glued
//! directly onto each other with no separator, loses the half that
//! doesn't open the run — `-L [<KIND>=]<PATH>` keeps only `[<KIND>=]`,
//! `--emit <TYPE>[=<FILE>]` keeps only `<TYPE>`. Fixture:
//! `corpus/rustc/1.82.0/`.

use mandible_core::CommandNode;

pub struct Finding {
    pub run: String,
    pub line: String,
}

pub struct Report {
    pub findings: Vec<Finding>,
}

/// A `[...]<...>` or `<...>[...]` run starting at `s`, verbatim including
/// every delimiter, or `None`. Single-level brackets only, matching
/// [`mandible_extract`]'s own `take_glued_angle_group`/
/// `take_glued_bracket_group`.
fn glued_run_at(s: &str) -> Option<&str> {
    if let Some(rest) = s.strip_prefix('[') {
        let close = rest.find(']')?;
        let after_bracket = &rest[close + 1..];
        let angle_rest = after_bracket.strip_prefix('<')?;
        let angle_close = angle_rest.find('>')?;
        return Some(&s[..close + 2 + angle_close + 2]);
    }
    if let Some(rest) = s.strip_prefix('<') {
        let close = rest.find('>')?;
        let after_angle = &rest[close + 1..];
        if !after_angle.starts_with('[') {
            return None;
        }
        let bracket_rest = &after_angle[1..];
        let bracket_close = bracket_rest.find(']')?;
        return Some(&s[..close + 2 + bracket_close + 2]);
    }
    None
}

fn tree_keeps_run(root: &CommandNode, run: &str) -> bool {
    root.flags().any(|e| e.value_name.as_deref() == Some(run))
}

pub fn detect(raw: &str, root: &CommandNode) -> Report {
    let mut findings = Vec::new();
    for line in raw.lines() {
        let mut rest = line;
        while let Some(start) = rest.find(['[', '<']) {
            let candidate = &rest[start..];
            if let Some(run) = glued_run_at(candidate) {
                if !tree_keeps_run(root, run) {
                    findings.push(Finding {
                        run: run.to_string(),
                        line: line.to_string(),
                    });
                }
                rest = &candidate[run.len()..];
            } else {
                rest = &candidate[1..];
            }
        }
    }
    Report { findings }
}

// ----------------------------------------------------------------------
// Self-checks
// ----------------------------------------------------------------------

use crate::detector::{Detector, Expect, SelfCheck, ToolEvidence};
use mandible_core::{Entity, Provenance, Source};

/// rustc's real rows, byte-exact (`rustc --help`).
pub(crate) const RUSTC_L_ROW: &str =
    "    -L [<KIND>=]<PATH>  Add a directory to the library search path.\n";
pub(crate) const RUSTC_EMIT_ROW: &str = "        --emit <TYPE>[=<FILE>]\n";

fn flag_with_value(value_name: Option<&str>) -> Entity {
    let mut e = Entity::flag_spelled(
        Some('L'),
        None,
        false,
        false,
        Provenance::single(Source::HelpText),
    );
    e.value_name = value_name.map(str::to_string);
    if value_name.is_some() {
        e.value_kind = mandible_core::ValueKind::Required;
    }
    e
}

fn node_with_flags(flags: Vec<Entity>) -> CommandNode {
    let mut root = CommandNode::new("rustc", Provenance::single(Source::HelpText));
    root.set_entities_of(mandible_core::EntityKind::Flag, flags);
    root
}

pub struct GluedBracketAngleRun;

impl Detector for GluedBracketAngleRun {
    fn name(&self) -> &'static str {
        "glued-bracket-angle-run"
    }

    fn family(&self) -> Option<&'static str> {
        None
    }

    fn describes(&self) -> &'static str {
        "a value spec mixing a glued bracket-optional group and a required angle placeholder \
         loses the half that doesn't open the run"
    }

    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        detect(evidence.raw, evidence.root)
            .findings
            .into_iter()
            .map(|f| format!("{:?} never became one value name, from {:?}", f.run, f.line))
            .collect()
    }

    fn self_checks(&self) -> Vec<SelfCheck> {
        vec![
            SelfCheck {
                name: "rustc's own bytes, `-L` losing `<PATH>`",
                why: "the defect itself: the bracket group survives but the required angle \
                      placeholder glued onto it is dropped",
                expect: Expect::Fires(1),
                raw: RUSTC_L_ROW.to_string(),
                root: node_with_flags(vec![flag_with_value(Some("[<KIND>=]"))]),
            },
            SelfCheck {
                name: "`-L` recovered as one glued value name",
                why: "once the tree keeps the whole run, the same row must go silent",
                expect: Expect::Silent,
                raw: RUSTC_L_ROW.to_string(),
                root: node_with_flags(vec![flag_with_value(Some("[<KIND>=]<PATH>"))]),
            },
            SelfCheck {
                name: "rustc's own bytes, `--emit` losing `[=<FILE>]`",
                why: "the mirror shape: an angle placeholder opens the run and the glued \
                      bracket group after it is dropped",
                expect: Expect::Fires(1),
                raw: RUSTC_EMIT_ROW.to_string(),
                root: node_with_flags(vec![flag_with_value(Some("<TYPE>"))]),
            },
            SelfCheck {
                name: "`--emit` recovered as one glued value name",
                why: "once the tree keeps the whole run, the same row must go silent",
                expect: Expect::Silent,
                raw: RUSTC_EMIT_ROW.to_string(),
                root: node_with_flags(vec![flag_with_value(Some("<TYPE>[=<FILE>]"))]),
            },
        ]
    }
}
