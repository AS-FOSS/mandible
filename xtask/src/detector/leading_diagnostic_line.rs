//! `leading-diagnostic-line` (atlas S-162): the chosen stream's own first
//! non-empty line is an option-rejection diagnostic (`fuser`'s `Invalid
//! option --help`, `Xvfb`'s `Unrecognized option: --help`, `nfsidmap`'s
//! `invalid option -- '-'`), and it survives into the root's own
//! `description`. Mirrors `mandible_extract::help_text::sections::preamble`'s
//! (private) `is_option_error_line`, checked independently here since a
//! detector reads only `raw`+`root` — see `bare_or_usage_separator.rs`'s own
//! doc comment for why that duplication is the accepted shape.

use crate::detector::{Detector, Expect, Scope, SelfCheck, ToolEvidence};
use mandible_core::{CommandNode, Provenance, Source, Text};

/// True if `line` (trimmed) opens with one of the four option-rejection
/// phrases, after an optional single-token `<name>: ` prefix. Deliberately
/// simpler than the parser's own `is_option_error_line`: no trailer-shape
/// bound, since a detector counts a raw shape, it does not have to decide
/// whether the line's tail is "shapely" the way the parser's containment
/// fence does.
fn looks_like_option_rejection(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    let body = match trimmed.split_once(": ") {
        Some((prefix, rest)) if !prefix.is_empty() && !prefix.contains(char::is_whitespace) => {
            rest
        }
        _ => trimmed,
    };
    let lower = body.to_ascii_lowercase();
    ["invalid option", "unrecognized option", "unknown option", "illegal option"]
        .iter()
        .any(|kw| lower.starts_with(kw))
}

/// `raw`'s own first non-empty physical line, or `None` for an empty
/// document.
fn first_nonblank_line(raw: &str) -> Option<&str> {
    raw.lines().find(|l| !l.trim().is_empty())
}

pub struct LeadingDiagnosticLine;

impl Detector for LeadingDiagnosticLine {
    fn name(&self) -> &'static str {
        "leading-diagnostic-line"
    }

    fn family(&self) -> Option<&'static str> {
        None
    }

    fn describes(&self) -> &'static str {
        "the chosen stream's own first non-empty line reads as an option-rejection diagnostic, \
         and it still occurs in the root's own description"
    }

    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        let Some(first) = first_nonblank_line(evidence.raw) else {
            return Vec::new();
        };
        if !looks_like_option_rejection(first) {
            return Vec::new();
        }
        let description = evidence
            .root
            .description
            .as_ref()
            .map(|t| t.as_str())
            .unwrap_or("");
        if description.contains(first.trim()) {
            vec![format!(
                "the leading diagnostic {first:?} still occurs in the root description"
            )]
        } else {
            Vec::new()
        }
    }

    fn scope(&self) -> Scope {
        Scope::full()
    }

    fn self_checks(&self) -> Vec<SelfCheck> {
        fn node_with_description(description: Option<&str>) -> CommandNode {
            let mut root = CommandNode::new("prog", Provenance::single(Source::HelpText));
            root.description = description.map(Text::sanitize);
            root
        }

        vec![
            SelfCheck {
                name: "fuser's own bytes, diagnostic fused into the description",
                why: "the defect itself: the diagnostic line is still contained in the root \
                      description",
                expect: Expect::Fires(1),
                raw: "/usr/bin/fuser: Invalid option --help\nUsage: fuser [-fIMuvw]\n".to_string(),
                root: node_with_description(Some(
                    "/usr/bin/fuser: Invalid option --help Show which processes use the named \
                     files",
                )),
            },
            SelfCheck {
                name: "fuser's own bytes, diagnostic dropped before the description",
                why: "once the diagnostic line is stripped before layout analysis, the same raw \
                      bytes must go silent",
                expect: Expect::Silent,
                raw: "/usr/bin/fuser: Invalid option --help\nUsage: fuser [-fIMuvw]\n".to_string(),
                root: node_with_description(Some(
                    "Show which processes use the named files, sockets, or filesystems.",
                )),
            },
            SelfCheck {
                name: "Xvfb's own bytes, no program-name prefix at all",
                why: "the bare-phrase shape, no `<name>: ` prefix, must fire the same way",
                expect: Expect::Fires(1),
                raw: "Unrecognized option: --help\nuse: X [:<display>] [option]\n".to_string(),
                root: node_with_description(Some(
                    "Unrecognized option: --help use: X [:<display>] [option]",
                )),
            },
            SelfCheck {
                name: "a real sentence merely mentioning the phrase mid-clause",
                why: "the phrase must open the line, not merely appear in it, so an ordinary \
                      sentence must never fire",
                expect: Expect::Silent,
                raw: "An invalid option combination here raises an error.\n\nUsage: mytool\n"
                    .to_string(),
                root: node_with_description(Some(
                    "An invalid option combination here raises an error.",
                )),
            },
            SelfCheck {
                name: "a document with no leading diagnostic at all",
                why: "an ordinary tool's first line is never claimed",
                expect: Expect::Silent,
                raw: "usage: mytool [OPTIONS]\n\n  -a  do a thing\n".to_string(),
                root: node_with_description(None),
            },
        ]
    }
}
