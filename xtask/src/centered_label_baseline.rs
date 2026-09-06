//! The `centered-label-baseline` detector (atlas S-149): a bare-word
//! block opens on a centered ALL-CAPS group label indented past every row
//! beneath it (`fail2ban-client`'s `Command:` section prints `BASIC`
//! centered at column 45 with every row beneath it at column 4). The
//! parser's own `bare_block_end`
//! (`mandible-extract/src/help_text/sections/scan.rs`) used to take a
//! bare-word block's baseline indent from its own first content line, so a
//! block opening on such a label dedented on the very first real row and
//! ended before reading one. Two halves, kept separate like
//! `description_continuation_dash_flag`'s: [`LabelPrecedesShallowerLine`]
//! reads only `raw` (the shape exists in the document forever, fix or no
//! fix); [`MissingRowAfterLabel`] reads `root` too (the tree artifact the
//! unfixed parser produces, which clears once the baseline is repaired).
//! `family()` is `None` for both: no seed-2/4/5/6/7 audit tool carries
//! this shape labelled. Fixture: `corpus/fail2ban-client/1.0.2/`.

use crate::detector::{Detector, Expect, Scope, SelfCheck, ToolEvidence};
use mandible_core::{is_command_name_shaped, CommandNode, Provenance, Source};

fn leading_whitespace(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// A line whose only content is spaced ASCII-uppercase words — mirrors
/// `crate::command_pattern_table::is_group_label` and
/// `mandible_extract`'s own `is_centered_group_label`, kept as an
/// independent copy since a detector must not import the parser it
/// measures.
fn is_group_label(trimmed: &str) -> bool {
    !trimmed.is_empty()
        && trimmed.chars().count() <= 40
        && trimmed
            .split_whitespace()
            .all(|w| w.chars().all(|c| c.is_ascii_uppercase()))
}

/// One occurrence of the shape: a centered label immediately followed (past
/// any blank lines) by a shallower, non-label line — the precondition that
/// defeats a first-line baseline.
pub struct Finding {
    pub label: String,
    pub label_indent: usize,
    pub next_row: String,
    pub next_row_indent: usize,
}

pub struct Report {
    pub findings: Vec<Finding>,
}

/// Every centered label in `raw` immediately followed (past blank lines) by
/// a shallower, non-label line.
pub fn detect_shape(raw: &str) -> Report {
    let lines: Vec<&str> = raw.lines().collect();
    let mut findings = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || !is_group_label(trimmed) {
            continue;
        }
        let label_indent = leading_whitespace(line);
        let mut j = i + 1;
        while j < lines.len() && lines[j].trim().is_empty() {
            j += 1;
        }
        let Some(next) = lines.get(j) else { continue };
        let next_trimmed = next.trim();
        if next_trimmed.is_empty() || is_group_label(next_trimmed) {
            continue;
        }
        let next_indent = leading_whitespace(next);
        if next_indent < label_indent {
            findings.push(Finding {
                label: trimmed.to_string(),
                label_indent,
                next_row: next_trimmed.to_string(),
                next_row_indent: next_indent,
            });
        }
    }
    Report { findings }
}

/// Whether `root` (searched at every depth, the same walk
/// `crate::command_pattern_table::tree_attests` uses) carries a subcommand
/// named `name` anywhere.
fn tree_has_subcommand(node: &CommandNode, name: &str) -> bool {
    node.subcommands
        .iter()
        .any(|c| c.name == name || tree_has_subcommand(c, name))
}

/// One shape occurrence whose own shallower row's leading word never
/// reached the tree as a subcommand — the artifact a baseline that dedents
/// before reading any real row leaves behind.
pub struct MissingFinding {
    pub name: String,
    pub row: String,
}

pub struct TreeReport {
    pub findings: Vec<MissingFinding>,
}

impl TreeReport {
    pub fn finding_count(&self) -> usize {
        self.findings.len()
    }
}

pub fn detect_tree(raw: &str, root: &CommandNode) -> TreeReport {
    let mut findings = Vec::new();
    for finding in detect_shape(raw).findings {
        let Some(name) = finding.next_row.split_whitespace().next() else {
            continue;
        };
        if !is_command_name_shaped(name) {
            continue;
        }
        if !tree_has_subcommand(root, name) {
            findings.push(MissingFinding {
                name: name.to_string(),
                row: finding.next_row,
            });
        }
    }
    TreeReport { findings }
}

pub struct LabelPrecedesShallowerLine;

impl Detector for LabelPrecedesShallowerLine {
    fn name(&self) -> &'static str {
        "centered-label-baseline-shape"
    }
    fn family(&self) -> Option<&'static str> {
        None
    }
    fn describes(&self) -> &'static str {
        "a centered ALL-CAPS group label immediately followed by a shallower, non-label line — \
         the raw structural precondition that defeats a first-line block baseline"
    }
    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        detect_shape(evidence.raw)
            .findings
            .iter()
            .map(|f| {
                format!(
                    "{:?} (indent {}) precedes {:?} (indent {})",
                    f.label, f.label_indent, f.next_row, f.next_row_indent
                )
            })
            .collect()
    }
    fn scope(&self) -> Scope {
        Scope::full()
    }
    fn self_checks(&self) -> Vec<SelfCheck> {
        shape_self_checks()
    }
}

pub struct MissingRowAfterLabel;

impl Detector for MissingRowAfterLabel {
    fn name(&self) -> &'static str {
        "centered-label-baseline-tree"
    }
    fn family(&self) -> Option<&'static str> {
        None
    }
    fn describes(&self) -> &'static str {
        "the shallower row right after a centered ALL-CAPS group label, whose leading word never \
         reached the tree as a subcommand — the tree artifact a baseline dedenting before any \
         real row is read leaves behind"
    }
    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        detect_tree(evidence.raw, evidence.root)
            .findings
            .iter()
            .map(|f| format!("{:?} missing, from {:?}", f.name, f.row))
            .collect()
    }
    fn scope(&self) -> Scope {
        Scope::full()
    }
    fn self_checks(&self) -> Vec<SelfCheck> {
        tree_self_checks()
    }
}

// ----------------------------------------------------------------------
// Self-checks
// ----------------------------------------------------------------------

fn node(name: &str) -> CommandNode {
    CommandNode::new(name, Provenance::single(Source::HelpText))
}

/// A trimmed slice of `fail2ban-client`'s own bytes: the `BASIC` label at
/// column 45 immediately above `start` at column 4.
const F2B_LABEL_EXCERPT: &str = "\
Command:
                                             BASIC
    start                                    starts the server and the jails
    restart                                  restarts the server
";

/// A section whose ALL-CAPS line is genuinely deeper prose wrapping, never
/// followed by a shallower row — the false alarm this shape's own
/// dedent-direction test exists for.
const NOT_A_LABEL_BEFORE_A_ROW: &str = "\
Options:
    --level <LEVEL>    one of CRITICAL, ERROR, WARNING or
                       INFO
";

/// A centered label followed by another centered label (a block with two
/// adjacent group headings and no rows at all between them) must not fire:
/// the second line is itself a label, not a row.
const LABEL_FOLLOWED_BY_LABEL: &str = "\
Command:
                                             BASIC
                                             LOGGING
    start                                    starts the server and the jails
";

fn shape_self_checks() -> Vec<SelfCheck> {
    vec![
        SelfCheck {
            name: "fail2ban-client's own bytes, BASIC preceding the shallower start row",
            why: "the defect's raw precondition: a centered label sits deeper than the very next \
                  real row",
            expect: Expect::Fires(1),
            raw: F2B_LABEL_EXCERPT.to_string(),
            root: node("fail2ban-client"),
        },
        SelfCheck {
            name: "an ALL-CAPS word wrapped inside an ordinary flag description",
            why: "an ALL-CAPS token that is part of a longer wrapped sentence, with nothing \
                  shallower following it, must never be mistaken for a group label opening a \
                  block",
            expect: Expect::Silent,
            raw: NOT_A_LABEL_BEFORE_A_ROW.to_string(),
            root: node("fail2ban-client"),
        },
        SelfCheck {
            name: "two adjacent centered labels with no row between them",
            why: "the line right after a label must itself be a real row, not another label — \
                  two group headings in a row with nothing between them is not this shape",
            expect: Expect::Silent,
            raw: LABEL_FOLLOWED_BY_LABEL.to_string(),
            root: node("fail2ban-client"),
        },
    ]
}

fn tree_self_checks() -> Vec<SelfCheck> {
    vec![
        SelfCheck {
            name: "fail2ban-client's own bytes, against a tree missing `start`",
            why: "the defect's tree artifact: the baseline dedents before `start` is ever read, \
                  so no tree carries it",
            expect: Expect::Fires(1),
            raw: F2B_LABEL_EXCERPT.to_string(),
            root: node("fail2ban-client"),
        },
        SelfCheck {
            name: "a correctly repaired tree",
            why: "once the baseline no longer dedents early, `start` reaches the tree as a real \
                  subcommand and this detector has nothing left to report",
            expect: Expect::Silent,
            raw: F2B_LABEL_EXCERPT.to_string(),
            root: {
                let mut root = node("fail2ban-client");
                root.subcommands = vec![node("start"), node("restart")];
                root
            },
        },
        SelfCheck {
            name: "no shape at all",
            why: "with no centered label preceding a shallower row, this detector must stay \
                  silent regardless of what the tree carries",
            expect: Expect::Silent,
            raw: NOT_A_LABEL_BEFORE_A_ROW.to_string(),
            root: node("fail2ban-client"),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fires_on_fail2ban_clients_own_bytes() {
        let report = detect_shape(F2B_LABEL_EXCERPT);
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].label, "BASIC");
        assert_eq!(report.findings[0].next_row_indent, 4);
    }

    #[test]
    fn tree_artifact_clears_once_repaired() {
        let mut repaired = node("fail2ban-client");
        repaired.subcommands = vec![node("start")];
        let report = detect_tree(F2B_LABEL_EXCERPT, &repaired);
        assert_eq!(report.finding_count(), 0);
    }

    #[test]
    fn every_self_check_holds_shape() {
        for case in shape_self_checks() {
            let expected = match case.expect {
                Expect::Fires(n) => n,
                Expect::Silent => 0,
            };
            let report = detect_shape(&case.raw);
            assert_eq!(report.findings.len(), expected, "{}", case.name);
        }
    }

    #[test]
    fn every_self_check_holds_tree() {
        for case in tree_self_checks() {
            let expected = match case.expect {
                Expect::Fires(n) => n,
                Expect::Silent => 0,
            };
            let report = detect_tree(&case.raw, &case.root);
            assert_eq!(report.finding_count(), expected, "{}", case.name);
        }
    }
}
