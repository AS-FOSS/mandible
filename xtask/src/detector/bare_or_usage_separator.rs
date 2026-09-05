//! `bare-or-usage-separator` (round 5): a usage line whose only content is
//! the word `or` (any case, optional trailing colon) is a pure separator
//! between two usage forms — the physical shape `sg_luns` writes
//! (`corpus/sg_luns/1.45`, `Usage: ... DEVICE\n     or\n       sg_luns
//! --test=ALUN ...`). Before this family's parser fix
//! (`mandible-extract/src/help_text/sections/mod.rs`,
//! `is_bare_or_form_separator`), that bare line read as an ordinary
//! continuation and glued the word `or` onto the end of the first form's
//! last token.
//!
//! Measured by whether the raw text carries such a bare-separator line at
//! all *and* the extracted tree still shows a usage form whose last token
//! is the bare word `or` — the after-the-fact symptom, since checking the
//! parser's own intermediate state is not available from `raw`+`root`
//! alone.
//!
//! No seed-2/4/5/6 labelled tool carries this shape under an existing
//! `mandible_core::audit::DEFECT_FAMILIES` entry, so [`Detector::family`]
//! returns `None` — spec §13.1e rule 6.

use crate::detector::{Detector, Expect, Scope, SelfCheck, ToolEvidence};
use mandible_core::{CommandNode, Provenance, Source, Text};

/// True if `line`'s only content, once trimmed, is the word `or` — any
/// case — with an optional trailing colon. Mirrors
/// `mandible_extract`'s (private) `is_bare_or_form_separator` predicate,
/// checked independently here since a detector reads only `raw`+`root`.
fn is_bare_or_line(line: &str) -> bool {
    line.trim().trim_end_matches(':').eq_ignore_ascii_case("or")
}

/// True if `entry`'s last whitespace-separated token is the bare word
/// `or` — the glued-continuation symptom this family's bug leaves behind.
fn ends_with_bare_or(entry: &str) -> bool {
    entry
        .trim_end()
        .rsplit(char::is_whitespace)
        .next()
        .map(|w| w.eq_ignore_ascii_case("or"))
        .unwrap_or(false)
}

pub struct BareOrUsageSeparator;

impl Detector for BareOrUsageSeparator {
    fn name(&self) -> &'static str {
        "bare-or-usage-separator"
    }

    fn family(&self) -> Option<&'static str> {
        None
    }

    fn describes(&self) -> &'static str {
        "the raw text carries a usage line holding only the word `or`, and a usage form in the \
         tree still ends with that bare word glued onto its last token"
    }

    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        if !evidence.raw.lines().any(is_bare_or_line) {
            return Vec::new();
        }
        evidence
            .root
            .usage
            .iter()
            .filter(|form| ends_with_bare_or(form.as_str()))
            .map(|form| format!("usage form ends with a glued bare `or`: {:?}", form.as_str()))
            .collect()
    }

    fn scope(&self) -> Scope {
        Scope::full()
    }

    fn self_checks(&self) -> Vec<SelfCheck> {
        fn node_with_usage(usage: &[&str]) -> CommandNode {
            let mut root = CommandNode::new("sg_luns", Provenance::single(Source::HelpText));
            root.usage = usage.iter().map(|u| Text::sanitize_preserving_layout(u)).collect();
            root
        }
        let raw = "Usage: sg_luns    [--decode] [--help] DEVICE\n     or\n       sg_luns    \
                   --test=ALUN [--decode]\n";
        vec![
            SelfCheck {
                name: "sg_luns's own bytes, pre-fix shape (`or` glued onto the first form)",
                why: "the defect itself: the raw text carries a bare `or` separator line and the \
                      tree's first usage form still ends with it",
                expect: Expect::Fires(1),
                raw: raw.to_string(),
                root: node_with_usage(&[
                    "Usage: sg_luns    [--decode] [--help] DEVICE or",
                    "       sg_luns    --test=ALUN [--decode]",
                ]),
            },
            SelfCheck {
                name: "sg_luns's own bytes, post-fix shape (separator dropped)",
                why: "once the parser drops the bare `or` line, the same raw bytes must go \
                      silent even though the separator line is still there",
                expect: Expect::Silent,
                raw: raw.to_string(),
                root: node_with_usage(&[
                    "Usage: sg_luns    [--decode] [--help] DEVICE",
                    "       sg_luns    --test=ALUN [--decode]",
                ]),
            },
            SelfCheck {
                name: "a usage form ending in a real word that is not `or`",
                why: "an ordinary usage form must never be claimed just because the raw text \
                      happens to mention `or` in a sentence elsewhere",
                expect: Expect::Silent,
                raw: "Usage: foo [-a] BAR\nSome sentence that says or so.\n".to_string(),
                root: node_with_usage(&["Usage: foo [-a] BAR"]),
            },
        ]
    }
}
