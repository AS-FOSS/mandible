//! `double-pipe-usage-alternation` (atlas S-175, measurement only):
//! `nfsidmap`'s own usage line writes an OR alternation with `||` rather
//! than the ordinary single-`|` grammar every other recognizer here reads
//! (`parse_flag_alternation`, `parse_brace_alternation_group`), with a
//! trailing `key desc` operand pair after the alternation closes:
//! `nfsidmap [-vh] [-c || [-u|-g|-r key] || -d || -l || [-t timeout] key
//! desc]`. Reported, not gated: below the five-tool floor on a raw-shape
//! sweep of `audit/queue-captures/`, so no fix ships this round
//! (docs/shapes.md S-175).
//!
//! Fixtures: `corpus/nfsidmap/audit-seed/`, `corpus/nfsidmap/audit-seed2/`.

use mandible_core::CommandNode;

pub struct Report {
    pub findings: Vec<String>,
}

/// True when `line` carries a `||`-joined OR alternation anywhere in a
/// usage form: two or more pipe characters immediately adjacent, distinct
/// from an ordinary single-`|` alternation (`{a|b}`, `[-c|-C]`) every
/// other recognizer already understands.
fn has_double_pipe_alternation(line: &str) -> bool {
    line.as_bytes().windows(2).any(|w| w == b"||")
}

pub fn detect(_raw: &str, root: &CommandNode) -> Report {
    let mut findings = Vec::new();
    for form in &root.usage {
        let line = form.as_str();
        if has_double_pipe_alternation(line) {
            findings.push(line.to_string());
        }
    }
    Report { findings }
}

pub struct DoublePipeUsageAlternation;

impl crate::detector::Detector for DoublePipeUsageAlternation {
    fn name(&self) -> &'static str {
        "double-pipe-usage-alternation"
    }

    fn family(&self) -> Option<&'static str> {
        None
    }

    fn describes(&self) -> &'static str {
        "a usage form's OR alternation is joined with `||` rather than a single `|`, a shape no \
         alternation recognizer here reads — measurement only, docs/shapes.md S-175"
    }

    fn hits(&self, evidence: &crate::detector::ToolEvidence<'_>) -> Vec<String> {
        detect(evidence.raw, evidence.root)
            .findings
            .into_iter()
            .map(|line| format!("{line:?} carries a `||`-joined alternation"))
            .collect()
    }

    fn self_checks(&self) -> Vec<SelfCheck> {
        self_checks()
    }
}

// ----------------------------------------------------------------------
// Self-checks
// ----------------------------------------------------------------------

use crate::detector::{Expect, SelfCheck};
use mandible_core::{Provenance, Source};

fn node_with_usage(name: &str, usage: &str) -> CommandNode {
    let mut root = CommandNode::new(name, Provenance::single(Source::HelpText));
    root.usage = vec![mandible_core::Text::sanitize(usage)];
    root
}

pub(crate) fn self_checks() -> Vec<SelfCheck> {
    vec![
        SelfCheck {
            name: "nfsidmap's own bytes, a `||`-joined OR alternation",
            why: "the defect itself: `||` is not the ordinary single-`|` grammar",
            expect: Expect::Fires(1),
            raw: String::new(),
            root: node_with_usage(
                "nfsidmap",
                "Usage: nfsidmap [-vh] [-c || [-u|-g|-r key] || -d || -l || [-t timeout] key desc]",
            ),
        },
        SelfCheck {
            name: "an ordinary single-`|` alternation",
            why: "the common case every other alternation recognizer already reads must stay \
                  silent here",
            expect: Expect::Silent,
            raw: String::new(),
            root: node_with_usage("widget", "Usage: widget {device|file} [-c|-C] cmd"),
        },
    ]
}
