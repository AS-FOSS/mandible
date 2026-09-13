//! `usage-foreign-program-word` (atlas S-151): a usage form's leading word
//! is a program name spelled neither as the node's own name nor its dotted
//! stem (`gcc-ranlib-13`'s form opens `/usr/bin/ranlib`). S-108 counts the
//! already-handled case; this counts the rest. Reported, not gated: the fix
//! is a render-layer substitution, so the raw text never changes.
//!
//! Fixtures: `corpus/gcc-ranlib-13/2.42/`.

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

/// Mirrors the renderer's `looks_like_option_or_placeholder`.
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

/// Mirrors the renderer's `word_names_node`.
fn word_names_node(word: &str, name: &str) -> bool {
    let b = basename(word);
    if b == name {
        return true;
    }
    match name.split_once('.') {
        Some((prefix, _)) if !prefix.is_empty() => b == prefix,
        _ => false,
    }
}

/// Mirrors the renderer's `foreign_program_word_span` gate: lowercase-led
/// past any leading path separators, no bracket, no angle, not ALL-CAPS,
/// made only of letters, digits, `.`, `-`, `_`, `+` or `/`.
fn looks_like_program_name(word: &str) -> bool {
    let Some(first_alpha) = word.chars().find(|c| c.is_alphabetic()) else {
        return false;
    };
    if !first_alpha.is_ascii_lowercase() {
        return false;
    }
    word.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+' | '/'))
}

pub fn detect(_raw: &str, root: &CommandNode) -> Report {
    let mut findings = Vec::new();
    for form in &root.usage {
        let text = strip_label(form.as_str());
        let Some(first) = text.split_whitespace().next() else {
            continue;
        };
        if looks_like_option_or_placeholder(first) {
            continue;
        }
        if word_names_node(first, &root.name) {
            // S-108's own already-handled case.
            continue;
        }
        if looks_like_program_name(first) {
            findings.push(Finding {
                token: first.to_string(),
                line: form.as_str().to_string(),
            });
        }
    }
    Report { findings }
}

pub struct UsageForeignProgramWord;

impl crate::detector::Detector for UsageForeignProgramWord {
    fn name(&self) -> &'static str {
        "usage-foreign-program-word"
    }

    fn family(&self) -> Option<&'static str> {
        None
    }

    fn describes(&self) -> &'static str {
        "a usage form's own leading word is a program name, spelled neither as the node's own \
         name nor its dotted stem — the unhandled case S-108's `usage-program-word-mismatch` \
         does not already cover"
    }

    fn hits(&self, evidence: &crate::detector::ToolEvidence<'_>) -> Vec<String> {
        detect(evidence.raw, evidence.root)
            .findings
            .into_iter()
            .map(|f| {
                format!(
                    "{:?} never became the node's own name, from {:?}",
                    f.token, f.line
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
            name: "gcc-ranlib-13's own bytes, a path naming an unrelated program",
            why: "the defect itself: `/usr/bin/ranlib`'s basename names neither the node nor its \
                  stem, byte-exact from `corpus/gcc-ranlib-13/2.42/help.txt`",
            expect: Expect::Fires(1),
            raw: String::new(),
            root: node_with_usage(
                "gcc-ranlib-13",
                &["Usage: /usr/bin/ranlib [options] archive"],
            ),
        },
        SelfCheck {
            name: "cargo-clippy's own bytes, a bare foreign program word",
            why: "the same defect with no path at all: `cargo` names neither the node \
                  `cargo-clippy` nor any stem it has",
            expect: Expect::Fires(1),
            raw: String::new(),
            root: node_with_usage("cargo-clippy", &["cargo clippy [OPTIONS] [--] [<ARGS>...]"]),
        },
        SelfCheck {
            name: "the node's own name already leads the form",
            why: "S-108's own already-handled case must never double-count here",
            expect: Expect::Silent,
            raw: String::new(),
            root: node_with_usage("cp", &["Usage: /usr/bin/cp [OPTION]... [-T] SOURCE DEST"]),
        },
        SelfCheck {
            name: "a bracketed subcommand token, never a program word",
            why: "`lldb-server`'s own form `g[dbserver] [options]` carries a subcommand in the \
                  program position; the embedded bracket must keep this silent",
            expect: Expect::Silent,
            raw: String::new(),
            root: node_with_usage("lldb-server", &["g[dbserver] [options]"]),
        },
    ]
}
