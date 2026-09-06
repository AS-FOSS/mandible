//! `usage-text-continuation-fold` (atlas S-135): a `usage:` line's own
//! tab-indented continuation sentence, describing what the tool does
//! rather than more invocation grammar, folds straight into the usage
//! text instead of being dropped. `makeconv`'s `usage: makeconv
//! [-options] files...` plus a tab-indented description line becomes one
//! `usage` string carrying both. Fixture: `corpus/makeconv/6.2`. A local,
//! independent copy of the parser's own shape check, not an import.

use mandible_core::CommandNode;

pub struct Finding {
    pub continuation: String,
    pub usage: String,
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

/// True when `line` (raw, untrimmed) opens with a literal tab and reads
/// as a plain-English continuation with no invocation grammar: no
/// bracket/angle/brace/pipe delimiter, no dash-led flag token, no bare
/// ALL-CAPS metavariable, no repetition ellipsis, and no terminal period
/// (a period-terminated sentence is a different, already handled shape —
/// `is_prose_sentence`'s own hanging-indent guard). Gated on the literal
/// tab, not indentation alone: `unzip`'s own genuine, two-space-indented
/// synopsis continuation must stay out of scope even though it also ends
/// with no period.
fn looks_like_unpunctuated_description_continuation(line: &str) -> bool {
    if !line.starts_with('\t') {
        return false;
    }
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.ends_with('.') || trimmed.contains("...") {
        return false;
    }
    if trimmed.contains(['[', ']', '<', '>', '{', '}', '|']) {
        return false;
    }
    let words: Vec<&str> = trimmed.split_whitespace().collect();
    if words.len() < 5 {
        return false;
    }
    if words.iter().any(|w| w.starts_with('-')) {
        return false;
    }
    !words.iter().any(|w| {
        let core = w.trim_matches(|c: char| !c.is_alphanumeric());
        core.len() > 1 && core.chars().all(|c| c.is_ascii_uppercase())
    })
}

pub fn detect(raw: &str, root: &CommandNode) -> Report {
    let lines: Vec<&str> = raw.lines().collect();
    let Some(idx) = lines
        .iter()
        .position(|l| l.trim_start().to_ascii_lowercase().starts_with("usage:"))
    else {
        return Report {
            findings: Vec::new(),
        };
    };
    let base_indent = leading_whitespace(lines[idx]);
    let Some(&next) = lines.get(idx + 1) else {
        return Report {
            findings: Vec::new(),
        };
    };
    if next.trim().is_empty() || leading_whitespace(next) <= base_indent {
        return Report {
            findings: Vec::new(),
        };
    }
    if !looks_like_unpunctuated_description_continuation(next) {
        return Report {
            findings: Vec::new(),
        };
    }
    let continuation = next.trim();
    let findings = root
        .usage
        .iter()
        .filter(|u| u.as_str().contains(continuation))
        .map(|u| Finding {
            continuation: continuation.to_string(),
            usage: u.as_str().to_string(),
        })
        .collect();
    Report { findings }
}

// ----------------------------------------------------------------------
// Self-checks
// ----------------------------------------------------------------------

use crate::detector::{Expect, SelfCheck};
use mandible_core::{Provenance, Source, Text};

pub(crate) const MAKECONV_USAGE: &str =
    "usage: makeconv [-options] files...\n\tread .ucm codepage mapping files and write .cnv files\n";

fn node_with_usage(name: &str, usage: &str) -> CommandNode {
    let mut root = CommandNode::new(name, Provenance::single(Source::HelpText));
    root.usage = vec![Text::sanitize(usage)];
    root
}

pub(crate) fn self_checks() -> Vec<SelfCheck> {
    vec![
        SelfCheck {
            name: "makeconv's own bytes, the continuation folded into usage",
            why: "the defect itself: the tab-indented description sentence has no terminating \
                  period, so it folds straight into the usage text",
            expect: Expect::Fires(1),
            raw: MAKECONV_USAGE.to_string(),
            root: node_with_usage(
                "makeconv",
                "usage: makeconv [-options] files... read .ucm codepage mapping files and write \
                 .cnv files",
            ),
        },
        SelfCheck {
            name: "the continuation correctly dropped",
            why: "once the fix lands, the usage line must carry only the invocation itself, so \
                  this must go silent",
            expect: Expect::Silent,
            raw: MAKECONV_USAGE.to_string(),
            root: node_with_usage("makeconv", "usage: makeconv [-options] files..."),
        },
        SelfCheck {
            name: "a genuine wrapped usage line with no terminal punctuation, ar's own shape",
            why: "a real continuation of the invocation itself — brackets, an angle-bracket \
                  metavar and a trailing ellipsis, no period — must never be mistaken for a \
                  dropped description",
            expect: Expect::Silent,
            raw: "Usage: /usr/bin/ar [emulation options] [-]{dmpqrstx}[abcDfilMNoOPsSTuvV]\n \
                  [--plugin <name>] [member-name] [count] archive-file file...\n"
                .to_string(),
            root: node_with_usage(
                "ar",
                "Usage: /usr/bin/ar [emulation options] [-]{dmpqrstx}[abcDfilMNoOPsSTuvV] \
                 [--plugin <name>] [member-name] [count] archive-file file...",
            ),
        },
        SelfCheck {
            name: "lsof's own same-indent bracketed continuation, no period either",
            why: "a bracket-only continuation with no period must stay out of scope even when \
                  every word check but the bracket one would otherwise pass",
            expect: Expect::Silent,
            raw: "usage: [-?abhKlnNoOPRtUvVX] [+|-c c] [+|-d s]\n [-F [f]] [-g [s]] [-i [i]]\n"
                .to_string(),
            root: node_with_usage(
                "lsof",
                "usage: [-?abhKlnNoOPRtUvVX] [+|-c c] [+|-d s] [-F [f]] [-g [s]] [-i [i]]",
            ),
        },
        SelfCheck {
            name: "unzip's own real bytes, a two-space-indented continuation with no period",
            why: "the real fleet counter-example the fix must not damage: unzip's own genuine \
                  synopsis continuation ends in a semicolon, carries no bracket or dash, and yet \
                  must stay folded into usage exactly as it is today, because it opens with \
                  spaces, never a literal tab",
            expect: Expect::Silent,
            raw: "Usage: unzip [-Z] [-opts[modifiers]] file[.zip] [list] [-x xlist] [-d exdir]\n  \
                  Default action is to extract files in list, except those in xlist, to exdir;\n"
                .to_string(),
            root: node_with_usage(
                "unzip",
                "Usage: unzip [-Z] [-opts[modifiers]] file[.zip] [list] [-x xlist] [-d exdir] \
                 Default action is to extract files in list, except those in xlist, to exdir;",
            ),
        },
    ]
}
