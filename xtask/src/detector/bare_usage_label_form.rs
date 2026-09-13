//! `bare-usage-label-form` (atlas S-150): a usage label alone on its own
//! line, with nothing else, contributes no invocation text — the detail
//! pane's `usage_form` renders the bare tool name for it instead.
//! Fixtures: `corpus/fdisk/2.39.3`, `corpus/pkcheck/124`.

use crate::detector::{Detector, Expect, SelfCheck, ToolEvidence};
use mandible_core::CommandNode;

pub struct Finding {
    pub entry: String,
}

pub struct Report {
    pub findings: Vec<Finding>,
}

/// True if `t` starts with `label` (`usage:` or `or:`), case-insensitive,
/// mirroring `mandible-extract`'s own `starts_with_usage_prefix`/
/// `starts_with_or_marker` — reimplemented rather than imported, the same
/// convention `usage_program_word_mismatch` already follows, since
/// `xtask` does not depend on `mandible-extract`'s internal grammar
/// modules.
fn strip_ci_label<'a>(t: &'a str, label: &str) -> Option<&'a str> {
    t.as_bytes()
        .get(..label.len())
        .filter(|b| b.eq_ignore_ascii_case(label.as_bytes()))
        .map(|_| &t[label.len()..])
}

/// True when `entry`, once trimmed, is either outright empty or carries
/// only a `usage:`/`or:` label with nothing after it — the tree-level
/// shape a fixed parser never emits (the label contributes no entry at
/// all), and the pre-fix parser always did (the label became its own
/// entry, seeded from nothing else).
fn is_bare_usage_entry(entry: &str) -> bool {
    let t = entry.trim();
    let stripped = strip_ci_label(t, "usage:")
        .or_else(|| strip_ci_label(t, "or:"))
        .unwrap_or(t);
    stripped.trim().is_empty()
}

pub fn detect(root: &CommandNode) -> Report {
    let findings = root
        .usage
        .iter()
        .filter(|u| is_bare_usage_entry(u.as_str()))
        .map(|u| Finding {
            entry: u.as_str().to_string(),
        })
        .collect();
    Report { findings }
}

pub struct BareUsageLabelForm;

impl Detector for BareUsageLabelForm {
    fn name(&self) -> &'static str {
        "bare-usage-label-form"
    }

    fn family(&self) -> Option<&'static str> {
        // No seed-labelled tool carries this shape as its own recorded
        // family (spec §13.1e rule 6): calibration has nothing to
        // generalize against.
        None
    }

    fn describes(&self) -> &'static str {
        "a root.usage entry that is empty, or carries only a `usage:`/`or:` label with no \
         invocation text of its own"
    }

    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        detect(evidence.root)
            .findings
            .into_iter()
            .map(|f| format!("bare usage entry {:?}", f.entry))
            .collect()
    }

    fn self_checks(&self) -> Vec<SelfCheck> {
        self_checks()
    }
}

// ----------------------------------------------------------------------
// Self-checks
// ----------------------------------------------------------------------

use mandible_core::{Provenance, Source, Text};

fn node_with_usage(name: &str, usage: &[&str]) -> CommandNode {
    let mut root = CommandNode::new(name, Provenance::single(Source::HelpText));
    root.usage = usage
        .iter()
        .map(|u| Text::sanitize_preserving_layout(u))
        .collect();
    root
}

pub(crate) fn self_checks() -> Vec<SelfCheck> {
    vec![
        SelfCheck {
            name: "a bare `Usage:` entry with nothing after it",
            why: "the defect itself: the pre-fix parser's own seed entry, byte-exact shape from \
                  fdisk's raw `Usage:` line before its two real forms",
            expect: Expect::Fires(1),
            raw: String::new(),
            root: node_with_usage("fdisk", &["Usage:"]),
        },
        SelfCheck {
            name: "an outright empty entry",
            why: "the same shape with the label itself already gone, so the tree-level check \
                  must not require the literal word `usage`",
            expect: Expect::Fires(1),
            raw: String::new(),
            root: node_with_usage("fdisk", &[""]),
        },
        SelfCheck {
            name: "a real form carrying real invocation text",
            why: "must stay silent on an ordinary usage form, even one that opens with the \
                  tool's own name",
            expect: Expect::Silent,
            raw: String::new(),
            root: node_with_usage("fdisk", &["fdisk [options] <disk>"]),
        },
        SelfCheck {
            name: "a real form on the fixed tree, label already dropped",
            why: "the fixed parser's own output: no bare entry survives once a real form \
                  follows it, so a tree with only real forms must read silent",
            expect: Expect::Silent,
            raw: String::new(),
            root: node_with_usage(
                "fdisk",
                &[
                    "fdisk [options] <disk>         change partition table",
                    "fdisk [options] -l [<disk>...] list partition table(s)",
                ],
            ),
        },
    ]
}
