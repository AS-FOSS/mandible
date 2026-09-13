//! `usage-form-trailing-description` (atlas S-152, measurement only): a
//! usage form's own trailing description reaches the tree glued onto the
//! synopsis instead of staying separate — `gcc-ranlib-13`'s own form reads
//! `gcc-ranlib-13 [options] archive Generate an index to speed access to
//! archives`. Reported, not gated: this round only measures the shape and
//! proposes; no fix ships below the five-tool floor (docs/shapes.md S-152).
//!
//! Fixtures: `corpus/gcc-ranlib-13/2.42/`, `corpus/fdisk/2.39.3/`,
//! `corpus/gdk-pixbuf-thumbnailer/2.42.10/` (whichever the round committed).

use mandible_core::CommandNode;

pub struct Finding {
    pub trailing: String,
    pub line: String,
}

pub struct Report {
    pub findings: Vec<Finding>,
}

/// A plain prose word: lowercase-led, only letters/digits/`-`/`_`, at
/// least two characters — the same shape `multiword.rs`'s own
/// `plain_word` reads, a local copy rather than an import (xtask's
/// detectors keep their own independent copies of the parser's grouping
/// and word rules, by convention, so a detector never shares a bug with
/// the fix it measures).
fn plain_prose_word(w: &str) -> bool {
    w.chars().count() >= 2
        && w.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && w.chars()
            .all(|c| c.is_ascii_alphabetic() || c == '-' || c == '_')
}

/// Whitespace-delimited groups, a `[...]` or `<...>` span counted as one
/// group even with internal spaces — the same grouping
/// `group_synopsis_tokens` gives, a local copy.
fn group_tokens(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    for c in s.chars() {
        match c {
            '[' | '<' => {
                depth += 1;
                cur.push(c);
            }
            ']' | '>' => {
                depth = (depth - 1).max(0);
                cur.push(c);
            }
            c if c.is_whitespace() && depth == 0 => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn strip_label(text: &str) -> &str {
    let t = text.trim_start();
    let lower = t.to_ascii_lowercase();
    for label in ["usage:", "or:"] {
        if lower.starts_with(label) {
            return t[label.len()..].trim_start();
        }
    }
    t
}

/// The minimum trailing run of plain prose words this counts as a real
/// glued description rather than a two-word operand name (S-132/S-154
/// already read those). Three, the same floor `is_prose_sentence` uses
/// for a continuation sentence.
const MIN_TRAILING_WORDS: usize = 3;

/// `line`'s own trailing run of [`plain_prose_word`] groups, `None`
/// unless at least [`MIN_TRAILING_WORDS`] of them sit at the very end and
/// at least one non-prose synopsis group (a flag, a bracket, an ALL-CAPS
/// operand) still stands ahead of the run — a bare, all-prose line is
/// usage grammar this detector was never meant to read, not this shape.
fn trailing_description(line: &str) -> Option<String> {
    let after = strip_label(line);
    let groups = group_tokens(after);
    if groups.len() < 2 {
        return None;
    }
    let mut split = groups.len();
    while split > 0 && plain_prose_word(&groups[split - 1]) {
        split -= 1;
    }
    let trailing_count = groups.len() - split;
    if trailing_count < MIN_TRAILING_WORDS || split < 2 {
        return None;
    }
    // At least one group ahead of the program name must be real synopsis
    // grammar (a flag, a bracketed/angled group, or an ALL-CAPS operand) —
    // otherwise this "trailing run" is just an all-prose sentence with no
    // synopsis to glue onto.
    let has_real_synopsis = groups[1..split].iter().any(|g| {
        g.starts_with('-')
            || g.starts_with('[')
            || g.starts_with('<')
            || g.chars().any(|c| c.is_ascii_uppercase())
    });
    if !has_real_synopsis {
        return None;
    }
    Some(groups[split..].join(" "))
}

pub fn detect(_raw: &str, root: &CommandNode) -> Report {
    let mut findings = Vec::new();
    for form in &root.usage {
        let line = form.as_str();
        if let Some(trailing) = trailing_description(line) {
            findings.push(Finding {
                trailing,
                line: line.to_string(),
            });
        }
    }
    Report { findings }
}

pub struct UsageFormTrailingDescription;

impl crate::detector::Detector for UsageFormTrailingDescription {
    fn name(&self) -> &'static str {
        "usage-form-trailing-description"
    }

    fn family(&self) -> Option<&'static str> {
        None
    }

    fn describes(&self) -> &'static str {
        "a usage form's own trailing description (three or more plain prose words behind real \
         synopsis grammar) reaches the tree glued onto the invocation instead of staying \
         separate — measurement only this round, docs/shapes.md S-152"
    }

    fn hits(&self, evidence: &crate::detector::ToolEvidence<'_>) -> Vec<String> {
        detect(evidence.raw, evidence.root)
            .findings
            .into_iter()
            .map(|f| format!("{:?} glued onto the usage form {:?}", f.trailing, f.line))
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
            name: "gcc-ranlib-13's own bytes, the description glued onto the synopsis",
            why: "the defect itself: the tail words are the tool's own trailing description, \
                  not part of the invocation",
            expect: Expect::Fires(1),
            raw: String::new(),
            root: node_with_usage(
                "gcc-ranlib-13",
                "gcc-ranlib-13 [options] archive Generate an index to speed access to archives",
            ),
        },
        SelfCheck {
            name: "the same form with its description already split off",
            why: "once separated, the usage form must go silent",
            expect: Expect::Silent,
            raw: String::new(),
            root: node_with_usage("gcc-ranlib-13", "gcc-ranlib-13 [options] archive"),
        },
        SelfCheck {
            name: "fdisk's own bytes, a column gap in front of the description",
            why: "the description is still glued on, just spaced out — the same shape either \
                  way",
            expect: Expect::Fires(1),
            raw: String::new(),
            root: node_with_usage("fdisk", "fdisk [options] <disk> change partition table"),
        },
        SelfCheck {
            name: "a docopt bracket-alternation tail with no real prose at all",
            why: "an all-grammar tail (no plain-prose run) must never be read as a description",
            expect: Expect::Silent,
            raw: String::new(),
            root: node_with_usage("widget", "widget [OPTIONS] <disk> [list of exclude dirs]"),
        },
        SelfCheck {
            name: "a two-word bracketed operand, S-132's own shape",
            why: "two trailing prose words alone (below this rule's own three-word floor) must \
                  stay S-132/S-154's read, not this one",
            expect: Expect::Silent,
            raw: String::new(),
            root: node_with_usage("widget", "widget [OPTIONS] [INPUT FILE]"),
        },
        SelfCheck {
            name: "a bare unlabelled synopsis with no real grammar ahead of the prose",
            why: "with nothing but plain words on the whole line, there is no synopsis to glue \
                  a description onto — declined rather than guessed",
            expect: Expect::Silent,
            raw: String::new(),
            root: node_with_usage("true", "true ignored command line arguments"),
        },
    ]
}
