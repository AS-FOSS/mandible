//! The `wrapped-command-continuation-as-subcommand` detector (atlas
//! S-103): a command's description wraps onto a physical line with no
//! column of its own, and the grammar reads that continuation's own
//! leading word as a fresh subcommand.
//!
//! A sibling of `xtask::wrapped_prose` (`wrapped-prose-row-boundary`,
//! S-027), not the same shape: that one requires the continuation's own
//! leading spelling to start with `-` and reads it as a flag; this one
//! requires no dash at all — subcommand names never carry one — and reads
//! it as a subcommand instead. See the family's own doc comment in
//! `mandible-core/src/audit.rs`.
//!
//! Fixture: `corpus/pnpm/11.22.0/`.

use mandible_core::CommandNode;

/// One fabricated subcommand: its name, and the continuation line it was
/// read from.
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

fn leading_whitespace(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// True when the trimmed content of `line` carries an internal run of 2+
/// spaces — a real description-column gap. Scanned only after the first
/// non-space character, so a plain indented line (all its leading
/// whitespace is not content) never counts as its own gap.
fn has_wide_gap(line: &str) -> bool {
    let bytes = line.trim_start().as_bytes();
    let mut run = 0usize;
    for b in bytes {
        if *b == b' ' {
            run += 1;
            if run >= 2 {
                return true;
            }
        } else {
            run = 0;
        }
    }
    false
}

fn tree_names<'a>(node: &'a CommandNode, out: &mut Vec<&'a str>) {
    for c in &node.subcommands {
        out.push(c.name.as_str());
        tree_names(c, out);
    }
}

/// True when `lines[idx]` is itself a continuation of an unfinished row's
/// description: bare (no gap, so it cannot be a fresh row's own name
/// field) and, walking back through zero or more further gap-less,
/// non-blank, non-sentence-final lines, eventually reaching a line that
/// does carry its own description-column gap (a real row) at a shallower
/// indent than `lines[idx]` itself. That anchor row is pnpm's own `why`/
/// `ls, list`; the walk-back is what lets this reach a continuation's own
/// continuation (`tree-structure`, one line further than `package`).
///
/// A generalization of `mandible-extract`'s
/// `is_wrapped_prose_continuation`, not the same rule: that function
/// requires the immediately preceding line to itself lack a description
/// column (it never anchors on the real row directly, only on one of its
/// own later continuations), because a flag table's row and its
/// continuation share one indent. A ragged command table's continuation
/// sits one indent level *deeper* than the row that owns it, so the
/// anchor here is found by indent comparison instead. See this module's
/// own doc comment for why the two rules differ.
fn is_bare_continuation(lines: &[&str], idx: usize) -> bool {
    let cur = lines[idx];
    if cur.split_whitespace().count() != 1 || has_wide_gap(cur) {
        return false;
    }
    let cur_indent = leading_whitespace(cur);
    let mut j = idx;
    loop {
        if j == 0 {
            return false;
        }
        j -= 1;
        let line = lines[j];
        if line.trim().is_empty() {
            return false;
        }
        if has_wide_gap(line) {
            return leading_whitespace(line) < cur_indent
                && !line.trim_end().ends_with(['.', '!', '?', ':']);
        }
        if line.trim_end().ends_with(['.', '!', '?', ':']) {
            return false;
        }
    }
}

/// Every subcommand in `root` whose name equals the sole word on a raw
/// continuation line [`is_bare_continuation`] admits.
pub fn detect(raw: &str, root: &CommandNode) -> Report {
    let lines: Vec<&str> = raw.lines().collect();
    let mut names = Vec::new();
    tree_names(root, &mut names);
    let mut findings = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        let word = line.trim();
        if word.is_empty() || !names.contains(&word) {
            continue;
        }
        if is_bare_continuation(&lines, idx) {
            findings.push(Finding {
                name: word.to_string(),
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
use mandible_core::{Provenance, Source};

/// pnpm's own bytes (`corpus/pnpm/11.22.0/help.txt`), the two rows this
/// shape invents from.
pub(crate) const PNPM_WRAPPED_HELP: &str = "\
  ls, list                 Print all the versions of packages that are
                           installed, as well as their dependencies, in a
                           tree-structure
      why                  Shows all packages that depend on the specified
                           package
";

fn node(name: &str) -> CommandNode {
    CommandNode::new(name, Provenance::single(Source::HelpText))
}

fn tree_with(names: &[&str]) -> CommandNode {
    let mut root = node("pnpm");
    root.subcommands = names.iter().map(|n| node(n)).collect();
    root
}

pub(crate) fn self_checks() -> Vec<SelfCheck> {
    vec![
        SelfCheck {
            name: "pnpm's own bytes, the pre-fix tree carrying both inventions",
            why: "the defect itself: `tree-structure` is the last word of `ls, list`'s wrapped \
                  description, alone on its own continuation line; `package` opens a \
                  continuation line under `why`. Neither is a command pnpm documents",
            expect: Expect::Fires(2),
            raw: PNPM_WRAPPED_HELP.to_string(),
            root: tree_with(&["tree-structure", "package"]),
        },
        SelfCheck {
            name: "pnpm's own bytes, the repaired tree",
            why: "once the parser folds both continuations into their real rows' descriptions \
                  instead of inventing nodes from them, the tree carries neither fabricated \
                  name and the detector has nothing to report — the repaired-family case",
            expect: Expect::Silent,
            raw: PNPM_WRAPPED_HELP.to_string(),
            root: tree_with(&["list", "why"]),
        },
        SelfCheck {
            name: "a real subcommand named on its own continuation line, coincidentally",
            why: "the detector's own reach is a coincidence hazard: `run` genuinely is a pnpm \
                  command, so a tree that already has it must not be flagged just because a \
                  continuation elsewhere happens to spell the same word — the tree here has no \
                  `run` node under a continuation of the sample text, so nothing fires",
            expect: Expect::Silent,
            raw: "      exec                 Executes a shell command in scope of a project\n"
                .to_string(),
            root: tree_with(&["exec"]),
        },
        SelfCheck {
            name: "a genuine two-word continuation is not a bare word",
            why: "gate: a continuation carrying more than one word is either more prose or a \
                  real table row this rule does not claim, never a single fabricated name",
            expect: Expect::Silent,
            raw: "\
      why                  Shows all packages that depend on the
                           specified package
"
            .to_string(),
            root: tree_with(&["package"]),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fires_on_both_of_pnpms_own_inventions() {
        let root = tree_with(&["tree-structure", "package"]);
        let report = detect(PNPM_WRAPPED_HELP, &root);
        assert_eq!(report.finding_count(), 2);
    }

    #[test]
    fn stays_silent_on_the_repaired_tree() {
        let root = tree_with(&["list", "why"]);
        assert_eq!(detect(PNPM_WRAPPED_HELP, &root).finding_count(), 0);
    }

    #[test]
    fn every_self_check_holds() {
        for case in self_checks() {
            let expected = match case.expect {
                Expect::Fires(n) => n,
                Expect::Silent => 0,
            };
            let report = detect(&case.raw, &case.root);
            assert_eq!(
                report.finding_count(),
                expected,
                "{}: expected {} finding(s)",
                case.name,
                expected
            );
        }
    }
}
