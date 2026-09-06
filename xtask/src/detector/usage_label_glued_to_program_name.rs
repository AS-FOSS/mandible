//! `usage-label-glued-to-program-name` (atlas S-142, measurement half):
//! a usage line's own label sits directly against the program name with
//! no space (`mksquashfs`'s `SYNTAX:mksquashfs source1 source2 ...`),
//! any label spelling other than the two already recognized
//! (`starts_with_usage_prefix`'s `usage:`, `starts_with_or_marker`'s
//! `or:`). Fixture: `corpus/mksquashfs/4.6.1` (xfail). No seed-labelled
//! tool carries this shape, so [`Detector::family`] returns `None`.

use crate::detector::{Detector, Expect, SelfCheck, ToolEvidence};
use mandible_core::CommandNode;

pub struct Finding {
    pub label: String,
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

/// True when `label` (already confirmed alphabetic, colon-terminated) is
/// one of the two spellings the parser already recognizes as a usage
/// marker regardless of what follows the colon — counting either here
/// would double the count against a shape that already works.
fn already_recognized_label(label: &str) -> bool {
    label.eq_ignore_ascii_case("usage") || label.eq_ignore_ascii_case("or")
}

/// True when `after` (the text right after the label's colon) opens with
/// `name` at a word boundary: the next char, if any, is neither
/// alphanumeric nor `_`, so `mksquashfs` matches `mksquashfsutils` at no
/// point but does match `mksquashfs source1 ...`. Also matches `name`
/// spelled as a full path glued the same way (`SYNTAX:/usr/bin/mksquashfs
/// ...`), the same twin shape `starts_with_tool_name_spelled_differently`
/// carries in `mandible_extract` for a *spaced* name — a probe resolves a
/// bare `PATH` lookup to the binary's full path, and the tool then echoes
/// that resolved `argv[0]` right back at its own reader.
fn opens_with_name(after: &str, name: &str) -> bool {
    let first_token = after.split_whitespace().next().unwrap_or(after);
    let basename = first_token.rsplit('/').next().unwrap_or(first_token);
    let Some(rest) = basename.strip_prefix(name) else {
        return false;
    };
    rest.is_empty()
        || !(rest
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_'))
}

pub fn detect(raw: &str, root: &CommandNode) -> Report {
    let name = root.name.as_str();
    let mut findings = Vec::new();
    if name.is_empty() {
        return Report { findings };
    }
    for line in raw.lines() {
        let t = line.trim_start();
        let Some(colon_idx) = t.find(':') else {
            continue;
        };
        let label = &t[..colon_idx];
        if label.len() < 2 || label.len() > 20 || !label.chars().all(|c| c.is_ascii_alphabetic()) {
            continue;
        }
        if already_recognized_label(label) {
            continue;
        }
        let Some(after) = t.get(colon_idx + 1..) else {
            continue;
        };
        if after.is_empty() || after.starts_with(char::is_whitespace) {
            continue;
        }
        if opens_with_name(after, name) {
            findings.push(Finding {
                label: label.to_string(),
                line: line.to_string(),
            });
        }
    }
    Report { findings }
}

pub struct UsageLabelGluedToProgramName;

impl Detector for UsageLabelGluedToProgramName {
    fn name(&self) -> &'static str {
        "usage-label-glued-to-program-name"
    }

    fn family(&self) -> Option<&'static str> {
        None
    }

    fn describes(&self) -> &'static str {
        "a usage line's own label sits directly against the tool's own name with no space, any \
         label spelling other than the two already-recognized `usage:`/`or:` markers"
    }

    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        detect(evidence.raw, evidence.root)
            .findings
            .into_iter()
            .map(|f| format!("{:?} glued to the program name, from {:?}", f.label, f.line))
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

pub(crate) const MKSQUASHFS_SYNTAX_LINE: &str =
    "SYNTAX:mksquashfs source1 source2 ...  FILESYSTEM [OPTIONS] [-e list of\n";

fn node_named(name: &str) -> CommandNode {
    CommandNode::new(name, Provenance::single(Source::HelpText))
}

fn node_with_usage(name: &str, usage: &str) -> CommandNode {
    let mut root = node_named(name);
    root.usage = vec![Text::sanitize(usage)];
    root
}

pub(crate) fn self_checks() -> Vec<SelfCheck> {
    vec![
        SelfCheck {
            name: "mksquashfs's own bytes, `SYNTAX:` glued to the program name",
            why: "the defect itself: no space between the label and the tool's own name, and \
                  the label is not one of the two already-recognized markers",
            expect: Expect::Fires(1),
            raw: MKSQUASHFS_SYNTAX_LINE.to_string(),
            root: node_named("mksquashfs"),
        },
        SelfCheck {
            name: "a real `Usage:` line, the already-recognized marker",
            why: "`usage:` is already matched by `starts_with_usage_prefix` regardless of a \
                  following space, so this detector must never also count it",
            expect: Expect::Silent,
            raw: "Usage:mksquashfs source1 source2\n".to_string(),
            root: node_with_usage("mksquashfs", "Usage:mksquashfs source1 source2"),
        },
        SelfCheck {
            name: "a real `Usage: ` line with a space, no glue at all",
            why: "a label followed by a space is not this shape; nothing here should fire",
            expect: Expect::Silent,
            raw: "Usage: mksquashfs source1 source2\n".to_string(),
            root: node_with_usage("mksquashfs", "Usage: mksquashfs source1 source2"),
        },
        SelfCheck {
            name: "a label glued to a different word entirely",
            why: "the text after the colon must open with the tool's own name, not just any \
                  word, so a label glued to unrelated text must stay silent",
            expect: Expect::Silent,
            raw: "NOTE:something unrelated\n".to_string(),
            root: node_named("mksquashfs"),
        },
        SelfCheck {
            name: "a clock-shaped colon, never a label at all",
            why: "`12:34` has a colon preceded by digits, never alphabetic label characters, so \
                  this must never be read as a usage label",
            expect: Expect::Silent,
            raw: "started at 12:34mksquashfs\n".to_string(),
            root: node_named("mksquashfs"),
        },
    ]
}
