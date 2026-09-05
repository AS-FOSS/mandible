//! `bare-or-usage-separator` (round 5): a usage line holding only the
//! word `or` is a pure form separator (docs/shapes.md S-112). One cause,
//! two symptoms, both modeled — an unindented bare `or` truncates the
//! rest of the block (`update-catalog` and 3 siblings); an indented one
//! glues onto the prior form (`sg_luns`, `sg_test_rwbuf`). Modeling only
//! one would undercount the family, the loosening AGENTS.md §3.1 forbids
//! the other way. Per bare-or line: the next line missing from every
//! `root.usage` form is truncation; a form still ending with it is gluing.
//! `family()` is `None` — no seed-2/4/5/6 tool carries this shape.

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
/// `or` — the gluing symptom.
fn ends_with_bare_or(entry: &str) -> bool {
    entry
        .trim_end()
        .rsplit(char::is_whitespace)
        .next()
        .map(|w| w.eq_ignore_ascii_case("or"))
        .unwrap_or(false)
}

/// The first non-blank physical line strictly after `raw`'s line `after`,
/// trimmed — what the second usage form is supposed to start with.
fn next_nonblank_line<'a>(lines: &[&'a str], after: usize) -> Option<&'a str> {
    lines[after + 1..]
        .iter()
        .find(|l| !l.trim().is_empty())
        .map(|l| l.trim())
}

/// The last non-blank physical line strictly before `raw`'s line `before`,
/// trimmed — the line the usage scan has to have reached for the block
/// to have stopped *at* the bare-or line rather than earlier, for some
/// unrelated reason this family did not cause.
fn prev_nonblank_line<'a>(lines: &[&'a str], before: usize) -> Option<&'a str> {
    lines[..before]
        .iter()
        .rev()
        .find(|l| !l.trim().is_empty())
        .map(|l| l.trim())
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
        "a raw usage block holds a line whose only content is the word `or`, and the tree \
         either lost everything after it (truncation) or still ends a form with it (gluing)"
    }

    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        let lines: Vec<&str> = evidence.raw.lines().collect();
        let mut findings = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            if !is_bare_or_line(line) {
                continue;
            }
            // The block must have actually reached this line for its
            // truncation to be this family's doing — otherwise an
            // unrelated, earlier-firing rule ended it first (a wrapped
            // continuation that itself looks like a flag row, say), and
            // claiming this bare-or line broke it would be inventing a
            // cause the raw bytes don't support.
            let reached_here = prev_nonblank_line(&lines, i).is_some_and(|prev| {
                evidence
                    .root
                    .usage
                    .iter()
                    .any(|u| u.as_str().contains(prev))
            });
            let next = next_nonblank_line(&lines, i);
            let truncated = reached_here
                && match next {
                    Some(needle) => !evidence
                        .root
                        .usage
                        .iter()
                        .any(|u| u.as_str().contains(needle)),
                    None => false,
                };
            if truncated {
                findings.push(format!(
                    "bare `or` line truncated the usage block: {:?} never reached the tree",
                    next.unwrap_or("")
                ));
                continue;
            }
            if evidence
                .root
                .usage
                .iter()
                .any(|f| ends_with_bare_or(f.as_str()))
            {
                findings
                    .push("bare `or` line glued onto the end of the prior usage form".to_string());
            }
        }
        findings
    }

    fn scope(&self) -> Scope {
        Scope::full()
    }

    fn self_checks(&self) -> Vec<SelfCheck> {
        fn node_with_usage(usage: &[&str]) -> CommandNode {
            let mut root = CommandNode::new("sg_luns", Provenance::single(Source::HelpText));
            root.usage = usage
                .iter()
                .map(|u| Text::sanitize_preserving_layout(u))
                .collect();
            root
        }
        let glued_raw = "Usage: sg_luns    [--decode] [--help] DEVICE\n     or\n       sg_luns    \
                         --test=ALUN [--decode]\n";
        // `update-catalog`'s own bytes, byte-exact in shape: a column-0
        // `or` separates two forms, each spanning two physical lines.
        let truncated_raw = "Usage:\n    update-catalog <options> --add --super \
                              <centralized_catalog>\n    update-catalog <options> --add \
                              <centralized_catalog> <ordinary_catalog>\nor\n    update-catalog \
                              <options> --remove --super <centralized_catalog>\n    \
                              update-catalog <options> --remove <centralized_catalog> \
                              <ordinary_catalog>\n";
        vec![
            SelfCheck {
                name: "sg_luns's own bytes, pre-fix shape (`or` glued onto the first form)",
                why: "the defect's gluing symptom: an indented bare `or` line falls through \
                      every break check and its text joins the end of the prior form",
                expect: Expect::Fires(1),
                raw: glued_raw.to_string(),
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
                raw: glued_raw.to_string(),
                root: node_with_usage(&[
                    "Usage: sg_luns    [--decode] [--help] DEVICE",
                    "       sg_luns    --test=ALUN [--decode]",
                ]),
            },
            SelfCheck {
                name: "update-catalog's own bytes, pre-fix shape (`or` truncates the block)",
                why: "the defect's other symptom: an unindented bare `or` ends the whole usage \
                      block there, so the second form (`--remove`) never reaches the tree at all",
                expect: Expect::Fires(1),
                raw: truncated_raw.to_string(),
                root: node_with_usage(&[
                    "Usage:",
                    "    update-catalog <options> --add --super <centralized_catalog>",
                    "    update-catalog <options> --add <centralized_catalog> <ordinary_catalog>",
                ]),
            },
            SelfCheck {
                name: "update-catalog's own bytes, post-fix shape (second form recovered)",
                why: "once the parser recovers the second form, the same raw bytes must go \
                      silent",
                expect: Expect::Silent,
                raw: truncated_raw.to_string(),
                root: node_with_usage(&[
                    "Usage:",
                    "    update-catalog <options> --add --super <centralized_catalog>",
                    "    update-catalog <options> --add <centralized_catalog> <ordinary_catalog>",
                    "    update-catalog <options> --remove --super <centralized_catalog>",
                    "    update-catalog <options> --remove <centralized_catalog> \
                     <ordinary_catalog>",
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
            SelfCheck {
                name: "sg_test_rwbuf's own bytes, block already ended before the `or` line",
                why: "an unrelated, earlier-firing rule (a wrapped continuation line that itself \
                      reads as a flag row) truncates the block before the bare `or` is ever \
                      reached, so the missing second form is not this family's doing and must \
                      not be claimed",
                expect: Expect::Silent,
                raw: "Usage: sg_test_rwbuf [--addrd=AR] [--addwr=AW] [--help] [--quick]\n         \
                      --size=SZ [--times=NUM] [--verbose] [--version]\n            DEVICE\n or\n  \
                      sg_test_rwbuf DEVICE SZ [AW] [AR]\n"
                    .to_string(),
                root: node_with_usage(&[
                    "Usage: sg_test_rwbuf [--addrd=AR] [--addwr=AW] [--help] [--quick]",
                ]),
            },
        ]
    }
}
