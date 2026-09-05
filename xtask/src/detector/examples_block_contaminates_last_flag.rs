//! `examples-block-contaminates-last-flag` (atlas S-126): an unheaded,
//! more-indented block of shell-invocation lines right after a flag's own
//! described row folds whole into that flag's description —
//! `nfsslower-bpfcc`'s `-p PID, --pid PID  Trace this pid only` gains all
//! five of its trailing `./nfsslower ... # ...` example lines.
//!
//! A local, independent copy of "looks like a shell invocation line",
//! mirroring `mandible_extract::help_text::sections::flag_rows`'s own
//! rule rather than importing it — an oracle built on the parser's own
//! helper would agree with the parser by construction.
//!
//! Fixture: `corpus/nfsslower-bpfcc/0.29.1`.

use mandible_core::CommandNode;

pub struct Finding {
    pub flag: String,
    pub description: String,
}

pub struct Report {
    pub findings: Vec<Finding>,
}

impl Report {
    pub fn finding_count(&self) -> usize {
        self.findings.len()
    }
}

/// A local copy of `flag_rows::looks_like_invocation_line`: the leading
/// word is `./`-prefixed or bare, and something further on the line is
/// either a `-`-led token or a `#` shell-comment marker.
fn is_flag_like_token(w: &str) -> bool {
    let mut chars = w.chars();
    chars.next() == Some('-') && chars.next().is_some_and(|c| !c.is_ascii_digit())
}

fn looks_like_invocation_line(trimmed: &str) -> bool {
    let mut words = trimmed.split_whitespace();
    let Some(first) = words.next() else {
        return false;
    };
    if !(first.starts_with("./") && first[2..].starts_with(|c: char| c.is_ascii_alphanumeric())) {
        return false;
    }
    words.any(is_flag_like_token) || trimmed.contains(" # ") || trimmed.trim_end().ends_with('#')
}

/// A description contains at least [`MIN_CONTAMINATING_LINES`]
/// example-shaped fragments glued on after its real first sentence — the
/// signature `flag_rows::scan_flags_block`'s continuation fold leaves
/// once an unheaded example block is swallowed: every trailing fragment,
/// split on the same collapsed whitespace the description was joined
/// with, reads as an invocation line.
const MIN_CONTAMINATING_LINES: usize = 2;

/// Does `description`'s tail look like two or more folded invocation
/// lines? Splits on the description's own internal structure is not
/// recoverable once joined by single spaces, so this instead asks: after
/// the first `#`-comment marker, does the remaining text contain at least
/// one more `./`-led or bare-word-plus-flag fragment? A simpler, more
/// robust signal than trying to re-segment the joined string: a
/// contaminated description carries at least two `#` comment markers
/// (one per folded example line), while an ordinary description carries
/// at most one incidental `#` if any.
fn description_carries_example_lines(description: &str) -> usize {
    description
        .split("./")
        .skip(1)
        .filter(|frag| looks_like_invocation_line(&format!("./{frag}")))
        .count()
}

pub fn detect(raw: &str, root: &CommandNode) -> Report {
    let _ = raw;
    let mut findings = Vec::new();
    for flag in root.flags() {
        let Some(desc) = flag.description.as_ref().map(|t| t.as_str()) else {
            continue;
        };
        if description_carries_example_lines(desc) >= MIN_CONTAMINATING_LINES {
            findings.push(Finding {
                flag: flag.spelling(),
                description: desc.to_string(),
            });
        }
    }
    Report { findings }
}

// ----------------------------------------------------------------------
// Self-checks
// ----------------------------------------------------------------------

use crate::detector::{Expect, SelfCheck};
use mandible_core::{Entity, Provenance, Source};

fn flag_with_description(short: char, description: &str) -> Entity {
    let mut e = Entity::flag_spelled(
        Some(short),
        None,
        false,
        false,
        Provenance::single(Source::HelpText),
    );
    e.description = Some(mandible_core::Text::sanitize(description));
    e
}

fn node_with_flags(name: &str, flags: Vec<Entity>) -> CommandNode {
    let mut root = CommandNode::new(name, Provenance::single(Source::HelpText));
    root.set_entities_of(mandible_core::EntityKind::Flag, flags);
    root
}

pub(crate) fn self_checks() -> Vec<SelfCheck> {
    vec![
        SelfCheck {
            name: "nfsslower-bpfcc's own contaminated description",
            why: "the defect itself: five example invocations folded onto the real one-line \
                  description",
            expect: Expect::Fires(1),
            raw: String::new(),
            root: node_with_flags(
                "nfsslower-bpfcc",
                vec![flag_with_description(
                    'p',
                    "Trace this pid only ./nfsslower # trace operations slower than 10ms \
                     ./nfsslower 1 # trace operations slower than 1ms ./nfsslower -j 1 # ... 1 \
                     ms, parsable output (csv)",
                )],
            ),
        },
        SelfCheck {
            name: "the same description with the example block removed",
            why: "once the description is only the real sentence, the same flag must go silent",
            expect: Expect::Silent,
            raw: String::new(),
            root: node_with_flags(
                "nfsslower-bpfcc",
                vec![flag_with_description('p', "Trace this pid only")],
            ),
        },
        SelfCheck {
            name: "an ordinary description mentioning a path once",
            why: "one incidental `./`-led word is not an example block; the floor requires two",
            expect: Expect::Silent,
            raw: String::new(),
            root: node_with_flags(
                "prog",
                vec![flag_with_description(
                    'c',
                    "run ./configure with --help for more options",
                )],
            ),
        },
    ]
}
