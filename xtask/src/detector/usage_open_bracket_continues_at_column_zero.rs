//! `usage-open-bracket-continues-at-column-zero` (atlas S-142): the
//! document's opening physical line ends with a square-bracket group
//! still open, and the very next physical line continues it at column
//! zero rather than being indented under it (`mksquashfs`'s
//! `SYNTAX:mksquashfs source1 source2 ...  FILESYSTEM [OPTIONS] [-e list
//! of` / `exclude dirs/files]`). Distinct from
//! [`crate::detector::usage_label_glued_to_program_name`]: that shape is
//! about the label, this one is about where the bracket group closes.
//! Reads the tree too: a fixed tree already carries a usage entry
//! containing the continuation's own words joined onto the first line, so
//! a repaired tool no longer counts (issue #143). Fixtures:
//! `corpus/mksquashfs/4.6.1`, `corpus/sqfstar/4.6.1`. No seed-labelled
//! tool carries this shape, so [`Detector::family`] returns `None`.

use crate::detector::{Detector, Expect, SelfCheck, ToolEvidence};
use mandible_core::CommandNode;

pub struct Finding {
    pub first_line: String,
    pub continuation: String,
}

pub struct Report {
    pub findings: Vec<Finding>,
}

impl Report {
    pub fn finding_count(&self) -> usize {
        self.findings.len()
    }
}

/// Net `[`/`]` balance: positive means more opens than closes.
fn bracket_balance(line: &str) -> i32 {
    line.chars().fold(0i32, |acc, c| match c {
        '[' => acc + 1,
        ']' => acc - 1,
        _ => acc,
    })
}

/// True when `line`, once trimmed, reads as a section heading rather than
/// a genuine continuation: ends in `:` with no bracket notation at all.
/// A generic-enough proxy for `is_section_heading_line` (kept as an
/// independent copy per this module's convention — see
/// `usage_text_continuation_fold`'s own doc comment) so this detector
/// never claims the S-075 shape (a heading following an open-looking
/// synopsis) as its own.
fn looks_like_heading_not_continuation(line: &str) -> bool {
    let t = line.trim();
    t.ends_with(':') && !t.contains(['[', ']', '<', '>', '{', '}'])
}

/// True when `line`, once trimmed, opens with a dash: a flag row can
/// never be mistaken for a bracket continuation either.
fn looks_like_flag_row(line: &str) -> bool {
    line.trim_start().starts_with('-')
}

/// True when some `root.usage` entry already contains the continuation's
/// own trimmed text: the fixed parser's own signature, the bracket group
/// joined onto the line that opened it. A raw finding backed by a tree
/// that already shows this is stale. See S-142, issue #143.
fn tree_already_joins(root: &CommandNode, continuation: &str) -> bool {
    let want = continuation.trim();
    !want.is_empty() && root.usage.iter().any(|u| u.as_str().contains(want))
}

pub fn detect(raw: &str, root: &CommandNode) -> Report {
    let lines: Vec<&str> = raw.lines().collect();
    let Some(first_idx) = lines.iter().position(|l| !l.trim().is_empty()) else {
        return Report {
            findings: Vec::new(),
        };
    };
    let first = lines[first_idx];
    if bracket_balance(first) <= 0 {
        return Report {
            findings: Vec::new(),
        };
    }
    let Some(&next) = lines.get(first_idx + 1) else {
        return Report {
            findings: Vec::new(),
        };
    };
    if next.trim().is_empty() {
        return Report {
            findings: Vec::new(),
        };
    }
    let next_indent = next.len() - next.trim_start().len();
    if next_indent != 0 {
        return Report {
            findings: Vec::new(),
        };
    }
    if looks_like_heading_not_continuation(next) || looks_like_flag_row(next) {
        return Report {
            findings: Vec::new(),
        };
    }
    if tree_already_joins(root, next) {
        return Report {
            findings: Vec::new(),
        };
    }
    Report {
        findings: vec![Finding {
            first_line: first.to_string(),
            continuation: next.to_string(),
        }],
    }
}

pub struct UsageOpenBracketContinuesAtColumnZero;

impl Detector for UsageOpenBracketContinuesAtColumnZero {
    fn name(&self) -> &'static str {
        "usage-open-bracket-continues-at-column-zero"
    }

    fn family(&self) -> Option<&'static str> {
        None
    }

    fn describes(&self) -> &'static str {
        "the document's opening line ends with a square-bracket group still open, and the very \
         next physical line continues it at column zero rather than being indented under it"
    }

    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        detect(evidence.raw, evidence.root)
            .findings
            .into_iter()
            .map(|f| {
                format!(
                    "open bracket carried from {:?} into {:?}",
                    f.first_line, f.continuation
                )
            })
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

pub(crate) const MKSQUASHFS_TWO_LINE_SYNTAX: &str =
    "SYNTAX:mksquashfs source1 source2 ...  FILESYSTEM [OPTIONS] [-e list of\nexclude dirs/files]\n";

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
            name: "mksquashfs's own bytes, the bracket group open across the line break",
            why: "the defect itself: `[-e list of` never closes on its own line, and the \
                  continuation `exclude dirs/files]` starts at column zero",
            expect: Expect::Fires(1),
            raw: MKSQUASHFS_TWO_LINE_SYNTAX.to_string(),
            root: node_named("mksquashfs"),
        },
        SelfCheck {
            name: "ar's own synopsis, fully balanced on one line",
            why: "S-075's own shape: the synopsis line closes every bracket it opens, so this \
                  detector must stay silent even though a heading follows at a non-zero indent",
            expect: Expect::Silent,
            raw: "Usage: ar [emulation options] [-]{dmpqrstx}[abcCDfilMNoOPsSTuvV...] archive \
                  [member-file...]\n commands:\n"
                .to_string(),
            root: node_named("ar"),
        },
        SelfCheck {
            name: "lsof's own wrapped bracket group, indented under the synopsis",
            why: "a genuine bracket continuation that is not at column zero is a different, \
                  already-handled shape (`looks_like_usage_fragment`); this detector's own scope \
                  is the column-zero case only",
            expect: Expect::Silent,
            raw: "usage: [-?abhKlnNoOPRtUvVX] [+|-c c] [+|-d s]\n [-F [f]] [-g [s]] [-i [i]]\n"
                .to_string(),
            root: node_named("lsof"),
        },
        SelfCheck {
            name: "a balanced one-line usage with an unrelated heading below it",
            why: "the ordinary case: nothing is open at line end, so a heading two lines down \
                  must never be mistaken for a continuation",
            expect: Expect::Silent,
            raw: "Usage: prog [-a] [-b]\nOptions:\n  -a  do a thing\n".to_string(),
            root: node_named("prog"),
        },
        SelfCheck {
            name: "an open bracket followed by a real section heading, not a continuation",
            why: "the caution the family brief itself raises: a column-zero line after an open \
                  bracket can be the next section's own heading rather than a continuation, and \
                  a heading-shaped line (ends in `:`, no bracket notation) must stay silent",
            expect: Expect::Silent,
            raw: "Usage: prog [-a\nOptions:\n  -a  do a thing\n".to_string(),
            root: node_named("prog"),
        },
        SelfCheck {
            name: "an open bracket followed by a flag row at column zero",
            why: "a flag row can never be a usage continuation (S-074's own rule, restated for \
                  the column-zero case): a dash-led line ends the block instead",
            expect: Expect::Silent,
            raw: "Usage: prog [-a\n-b  do a thing\n".to_string(),
            root: node_named("prog"),
        },
        SelfCheck {
            name: "the fixed tree, bracket already joined",
            why: "the repaired parser's own signature: `root.usage` already contains the \
                  continuation's own words joined onto the first line, and a raw finding \
                  against a tree that already shows this must not count",
            expect: Expect::Silent,
            raw: MKSQUASHFS_TWO_LINE_SYNTAX.to_string(),
            root: node_with_usage(
                "mksquashfs",
                "mksquashfs source1 source2 ...  FILESYSTEM [OPTIONS] [-e list of exclude dirs/files]",
            ),
        },
    ]
}
