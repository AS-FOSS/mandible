//! `usage-alternative-or-prefix` (atlas S-107): a usage continuation
//! line's own marker word `or` (`   or: vim [arguments] -`, `  or:
//! /usr/bin/cp [OPTION]... SOURCE... DIRECTORY`) reaches
//! `CommandNode::usage` with the marker still inside the form, so it
//! renders as if it were part of the synopsis.
//!
//! Fixtures: `corpus/vim.basic/audit-seed4/`, `corpus/cp/9.4/`,
//! `corpus/du/9.4/`.

use mandible_core::CommandNode;

pub struct Finding {
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

pub fn detect(_raw: &str, root: &CommandNode) -> Report {
    let mut findings = Vec::new();
    for form in &root.usage {
        let trimmed = form.as_str().trim_start();
        if trimmed.starts_with("or:") || trimmed.starts_with("or ") {
            findings.push(Finding {
                line: form.as_str().to_string(),
            });
        }
    }
    Report { findings }
}

// ----------------------------------------------------------------------
// Self-checks
// ----------------------------------------------------------------------

use crate::detector::{Expect, SelfCheck};
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
            name: "vim.basic's own bytes, three `or:` continuations",
            why: "the defect itself: each continuation's own `or:` marker reaches the usage \
                  form, byte-exact from `corpus/vim.basic/audit-seed4/help.txt`",
            expect: Expect::Fires(3),
            raw: String::new(),
            root: node_with_usage(
                "vim.basic",
                &[
                    "Usage: vim [arguments] [file ..]       edit specified file(s)",
                    "   or: vim [arguments] -               read text from stdin",
                    "   or: vim [arguments] -t tag          edit file where tag is defined",
                    "   or: vim [arguments] -q [errorfile]  edit file with first error",
                ],
            ),
        },
        SelfCheck {
            name: "cp's own bytes, two `or:` continuations",
            why: "the same shape on a second tool, byte-exact from `corpus/cp/9.4/help.txt`",
            expect: Expect::Fires(2),
            raw: String::new(),
            root: node_with_usage(
                "cp",
                &[
                    "Usage: /usr/bin/cp [OPTION]... [-T] SOURCE DEST",
                    "  or:  /usr/bin/cp [OPTION]... SOURCE... DIRECTORY",
                    "  or:  /usr/bin/cp [OPTION]... -t DIRECTORY SOURCE...",
                ],
            ),
        },
        SelfCheck {
            name: "the first form alone, no continuation",
            why: "a usage block with only one form carries no `or:` marker to fabricate",
            expect: Expect::Silent,
            raw: String::new(),
            root: node_with_usage("prog", &["Usage: prog [OPTION]... FILE"]),
        },
        SelfCheck {
            name: "the word \"or\" inside ordinary prose, not a form's own prefix",
            why: "the check is a prefix test after trimming, so `or` appearing mid-sentence must \
                  never be claimed",
            expect: Expect::Silent,
            raw: String::new(),
            root: node_with_usage("prog", &["Usage: prog A or B"]),
        },
    ]
}
