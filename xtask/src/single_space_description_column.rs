//! `single-space-description-column` (atlas S-105): jinfo's `-? | -h |
//! --help | -help to print this help message` has one space between its
//! last alias and its description, so the description column boundary
//! (`help_text::sections::entry::MIN_COLUMN_GAP_SPACES` = 2, or a tab)
//! never opens and the description is not found — the leading word of
//! what should be the description is read as a value name instead.
//!
//! Fixture: `corpus/jinfo/17.0.20/`.

use mandible_core::CommandNode;

pub struct Finding {
    /// Every pipe-joined spelling in the row, in document order.
    pub spellings: Vec<String>,
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

/// A single unbroken flag spelling: starts with `-`, a real character
/// right after it, and no internal whitespace at all — excludes `ip`'s
/// packed `-f[amily] { inet | inet6 | ... }` alternation member, whose
/// bracketed sub-alternation carries spaces of its own and is not one
/// alias spelling.
fn is_flag_token(s: &str) -> bool {
    !s.is_empty()
        && s.starts_with('-')
        && s.chars().nth(1).is_some_and(|c| c != ' ')
        && !s.contains(char::is_whitespace)
}

/// `line` read as a `" | "`-joined alias row whose last alias is
/// followed by exactly one space and then real description text —
/// never two spaces or a tab, which would be the ordinary, correctly
/// read description column instead.
fn parse_single_space_row(line: &str) -> Option<(Vec<String>, String)> {
    let trimmed = line.trim();
    if !trimmed.contains(" | ") {
        return None;
    }
    let parts: Vec<&str> = trimmed.split(" | ").collect();
    if parts.len() < 2 {
        return None;
    }
    for p in &parts[..parts.len() - 1] {
        if !is_flag_token(p) {
            return None;
        }
    }
    let last = parts[parts.len() - 1];
    let last_token = last.split_whitespace().next()?;
    if !is_flag_token(last_token) {
        return None;
    }
    let after = &last[last_token.len()..];
    if !after.starts_with(' ') || after.starts_with("  ") || after.starts_with('\t') {
        return None;
    }
    let description = after.trim_start().to_string();
    // A real description opens on a word, never on bare punctuation —
    // `ip`'s packed `OPTIONS := { ... | -p[retty] |\n  -f[amily] ... }`
    // alternation wraps its trailing `|` continuation onto the next
    // physical line, and without this guard that lone `|` reads as a
    // one-word "description" following a single space. Calibration
    // against seed 2 found this exact false generalization.
    if !description.starts_with(|c: char| c.is_alphabetic()) {
        return None;
    }
    let mut spellings: Vec<String> = parts[..parts.len() - 1]
        .iter()
        .map(|s| s.to_string())
        .collect();
    spellings.push(last_token.to_string());
    Some((spellings, description))
}

/// Whether any entity spelled with one of `spellings` already carries
/// `description` as its own real description — the recovered shape.
fn any_entity_has_description(root: &CommandNode, spellings: &[String], description: &str) -> bool {
    root.flags().any(|e| {
        e.spellings
            .iter()
            .any(|s| spellings.contains(&s.render()) || spellings.contains(&s.name))
            && e.description
                .as_ref()
                .is_some_and(|d| d.as_str() == description)
    })
}

pub fn detect(raw: &str, root: &CommandNode) -> Report {
    let mut findings = Vec::new();
    for line in raw.lines() {
        let Some((spellings, description)) = parse_single_space_row(line) else {
            continue;
        };
        if !any_entity_has_description(root, &spellings, &description) {
            findings.push(Finding {
                spellings,
                description,
            });
        }
    }
    Report { findings }
}

// ----------------------------------------------------------------------
// Self-checks
// ----------------------------------------------------------------------

use crate::detector::{Expect, SelfCheck};
use mandible_core::{Entity, Provenance, Source, Text};

/// jinfo's real row, byte-exact (`corpus/jinfo/17.0.20/help.stderr.txt`).
pub(crate) const JINFO_HELP_ROW: &str = "    -? | -h | --help | -help to print this help message\n";

fn flag(long: Option<&str>, short: Option<char>) -> Entity {
    Entity::flag_spelled(
        short,
        long.map(|s| s.to_string()),
        false,
        false,
        Provenance::single(Source::HelpText),
    )
}

fn node_with_flags(name: &str, flags: Vec<Entity>) -> CommandNode {
    let mut root = CommandNode::new(name, Provenance::single(Source::HelpText));
    root.set_entities_of(mandible_core::EntityKind::Flag, flags);
    root
}

fn flag_with_description(short: Option<char>, long: Option<&str>, description: &str) -> Entity {
    let mut e = flag(long, short);
    e.description = Some(Text::sanitize(description));
    e
}

pub(crate) fn self_checks() -> Vec<SelfCheck> {
    vec![
        SelfCheck {
            name: "jinfo's own bytes, description dropped",
            why: "the defect itself: the single-space gap means no entity in the group carries \
                  the real description",
            expect: Expect::Fires(1),
            raw: JINFO_HELP_ROW.to_string(),
            root: node_with_flags(
                "jinfo",
                vec![flag(None, Some('h')), flag(Some("help"), None)],
            ),
        },
        SelfCheck {
            name: "the same aliases with the description already attached",
            why: "once any entity in the group carries the real description text, the row must \
                  go silent",
            expect: Expect::Silent,
            raw: JINFO_HELP_ROW.to_string(),
            root: node_with_flags(
                "jinfo",
                vec![flag_with_description(
                    Some('h'),
                    Some("help"),
                    "to print this help message",
                )],
            ),
        },
        SelfCheck {
            name: "the same aliases with a real two-space description column",
            why: "an ordinary aligned row is not this defect — the two-space gap is the correct \
                  shape and must never be claimed",
            expect: Expect::Silent,
            raw: "    -? | -h | --help | -help  to print this help message\n".to_string(),
            root: node_with_flags("jinfo", vec![flag(None, Some('h'))]),
        },
        SelfCheck {
            name: "a pipe-joined row with no trailing prose at all",
            why: "an alias list with nothing after it is not a description-column defect",
            expect: Expect::Silent,
            raw: "    -a | -b | -c\n".to_string(),
            root: node_with_flags("prog", vec![]),
        },
    ]
}
