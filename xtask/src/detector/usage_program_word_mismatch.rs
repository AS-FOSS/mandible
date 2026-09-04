//! `usage-program-word-mismatch` (atlas S-108): a usage form's own leading
//! program word is the tool under a different spelling — a path
//! (`/usr/bin/cp` against node `cp`) or a stem (`vim` against node
//! `vim.basic`) — so `usage_form` in
//! `mandible-tui/src/render/detail_pane/mod.rs` never recognizes it and
//! prefixes the node name in front of the form.
//!
//! This reimplements that renderer's leading-run rule rather than
//! importing it (`xtask` does not depend on `mandible-tui`).
//!
//! Fixtures: `corpus/cp/9.4/` (path), `corpus/vim.basic/audit-seed4/`
//! (stem).

use mandible_core::CommandNode;

pub struct Finding {
    pub token: String,
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

/// Mirrors `mandible-tui`'s `looks_like_option_or_placeholder`: an option
/// (`-v`, `--verbose`), a bracketed/angled placeholder, or a bare
/// ALL-CAPS metavar ends the leading command-path run.
fn looks_like_option_or_placeholder(word: &str) -> bool {
    if word.starts_with(['-', '[', '<']) {
        return true;
    }
    let has_letter = word.chars().any(|c| c.is_alphabetic());
    has_letter && !word.chars().any(|c| c.is_lowercase())
}

/// Mirrors the renderer's `usage_form`: drop a leading `usage:`/`or:`
/// label before reading the leading run.
fn strip_label(text: &str) -> &str {
    let t = text.trim_start();
    for label in ["usage:", "or:"] {
        if t.len() >= label.len() && t[..label.len()].eq_ignore_ascii_case(label) {
            return t[label.len()..].trim_start();
        }
    }
    t
}

fn basename(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

pub fn detect(_raw: &str, root: &CommandNode) -> Report {
    let stem = root.name.split('.').next().unwrap_or(&root.name);
    let mut findings = Vec::new();
    for form in &root.usage {
        let text = strip_label(form.as_str());
        let leading: Vec<&str> = text
            .split_whitespace()
            .take_while(|w| !looks_like_option_or_placeholder(w))
            .collect();
        if leading.iter().any(|w| *w == root.name) {
            continue;
        }
        if let Some(mismatch) = leading.iter().find(|w| {
            let b = basename(w);
            b == root.name || b == stem
        }) {
            findings.push(Finding {
                token: mismatch.to_string(),
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
            name: "cp's own bytes, the path spelling",
            why: "the defect itself: `/usr/bin/cp`'s basename matches node `cp`, but the exact \
                  word `cp` never appears, byte-exact from `corpus/cp/9.4/help.txt`",
            expect: Expect::Fires(1),
            raw: String::new(),
            root: node_with_usage("cp", &["Usage: /usr/bin/cp [OPTION]... [-T] SOURCE DEST"]),
        },
        SelfCheck {
            name: "vim.basic's own bytes, the stem spelling",
            why: "the second spelling: `vim` matches node `vim.basic`'s prefix before its first \
                  `.`, byte-exact from `corpus/vim.basic/audit-seed4/help.txt`",
            expect: Expect::Fires(1),
            raw: String::new(),
            root: node_with_usage(
                "vim.basic",
                &["Usage: vim [arguments] [file ..]       edit specified file(s)"],
            ),
        },
        SelfCheck {
            name: "the node's own exact name already leads the form",
            why: "the renderer already recognizes this: the exact word `cp` in the leading run \
                  must silence the check",
            expect: Expect::Silent,
            raw: String::new(),
            root: node_with_usage("cp", &["Usage: cp [OPTION]... SOURCE DEST"]),
        },
        SelfCheck {
            name: "an unrelated leading word, no coincidental match",
            why: "the mismatch check requires the word's basename to equal the node's own name \
                  or stem — an ordinary different program word must never be claimed",
            expect: Expect::Silent,
            raw: String::new(),
            root: node_with_usage("other", &["Usage: prog [OPTION]"]),
        },
    ]
}
