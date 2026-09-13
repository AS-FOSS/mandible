//! `usage-attested-single-dash-long` (atlas S-172): a single-dash-long
//! spelling the tool's own usage line spells as one stand-alone bracketed
//! token — `lshw`'s own `[-format]`, `fuser`'s own `[-SIGNAL]` — currently
//! reaches the tree split into a short flag plus a swallowed value.
//! Fixtures: `corpus/lshw/02.19/`, `corpus/fuser/23.7/`.

// Never a bundle of already-known short flags (`rpcbind`'s own
// `-adhilswfr`, deliberately left to the bundling family): a name is
// reported only when at least one of its own characters is *not* already
// a short flag elsewhere in the tree, mirroring the parser's own guard.

use mandible_core::{CommandNode, ValueKind};
use std::collections::{BTreeSet, HashSet};

pub struct Finding {
    pub name: String,
}

pub struct Report {
    pub findings: Vec<Finding>,
}

const MIN_SWALLOWED_CHARS: usize = 2;

/// Twin of `mandible_extract`'s own `is_option_name_tail`: alphanumerics,
/// `-`, `_`, at least one letter.
fn is_option_name_tail(tail: &str) -> bool {
    tail.chars().any(|c| c.is_ascii_alphabetic())
        && tail
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// A `usage:`/`Usage:`-led physical line, plus every immediately
/// following non-blank line indented under it — the common multi-line
/// synopsis continuation shape (`fuser`'s own second line).
fn usage_lines(raw: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut in_block = false;
    for line in raw.lines() {
        let trimmed = line.trim_start();
        if trimmed
            .get(..6)
            .is_some_and(|p| p.eq_ignore_ascii_case("usage:"))
        {
            in_block = true;
            out.push(line);
        } else if in_block && !line.trim().is_empty() && line.starts_with(char::is_whitespace) {
            out.push(line);
        } else {
            in_block = false;
        }
    }
    out
}

/// True when `needle` occurs in `line` bounded on both sides by a
/// bracket, whitespace, or the line's own edge.
fn line_has_standalone_token(line: &str, needle: &str) -> bool {
    let is_boundary = |c: char| c.is_whitespace() || c == '[' || c == ']';
    let mut start = 0usize;
    while let Some(rel) = line.get(start..).and_then(|s| s.find(needle)) {
        let idx = start + rel;
        let before_ok = line[..idx].chars().next_back().is_none_or(is_boundary);
        let after_idx = idx + needle.len();
        let after_ok = line[after_idx..].chars().next().is_none_or(is_boundary);
        if before_ok && after_ok {
            return true;
        }
        start = idx + 1;
    }
    false
}

pub fn detect(raw: &str, root: &CommandNode) -> Report {
    let lines = usage_lines(raw);
    if lines.is_empty() {
        return Report {
            findings: Vec::new(),
        };
    }
    let existing_shorts: HashSet<char> = root.flags().filter_map(|f| f.short()).collect();
    let mut seen = BTreeSet::new();
    let mut findings = Vec::new();
    for flag in root.flags() {
        let Some(short) = flag.short() else { continue };
        if flag.long().is_some() || flag.value_kind != ValueKind::Required {
            continue;
        }
        let Some(tail) = flag.value_name.as_deref() else {
            continue;
        };
        if !is_option_name_tail(tail) || tail.chars().count() < MIN_SWALLOWED_CHARS {
            continue;
        }
        let name = format!("{short}{tail}");
        if name.chars().all(|c| existing_shorts.contains(&c)) {
            continue;
        }
        let needle = format!("-{name}");
        if !lines.iter().any(|l| line_has_standalone_token(l, &needle)) {
            continue;
        }
        if seen.insert(name.clone()) {
            findings.push(Finding { name });
        }
    }
    Report { findings }
}

// ----------------------------------------------------------------------
// Self-checks
// ----------------------------------------------------------------------

use crate::detector::{Expect, SelfCheck};
use mandible_core::{Entity, Provenance, Source, Spelling};

pub(crate) const LSHW_USAGE_LINE: &str = "usage: /usr/bin/lshw [-format] [-options ...]\n";
pub(crate) const FUSER_USAGE_LINES: &str = "Usage: fuser [-fIMuvw] [-a|-s] [-4|-6] [-c|-m|-n SPACE]\n             [-k [-i] [-SIGNAL]] NAME...\n";

fn split_flag(short: char, tail: &str) -> Entity {
    let mut e = Entity::flag_spelled(
        None,
        None,
        false,
        false,
        Provenance::single(Source::HelpText),
    );
    e.spellings = vec![Spelling::short(short)];
    e.value_name = Some(tail.to_string());
    e.value_kind = ValueKind::Required;
    e
}

fn whole_flag(name: &str) -> Entity {
    let mut e = Entity::flag_spelled(
        None,
        None,
        false,
        false,
        Provenance::single(Source::HelpText),
    );
    e.spellings = vec![Spelling::single_dash(name)];
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
            name: "lshw's own bytes, `-format` truncated to `-f` valued `\"ormat\"`",
            why: "the defect itself: the usage line spells `-format` whole, but the tree \
                  carries only a swallowed-value split",
            expect: Expect::Fires(1),
            raw: LSHW_USAGE_LINE.to_string(),
            root: node_with_flags("lshw", vec![split_flag('f', "ormat")]),
        },
        SelfCheck {
            name: "`-format` recovered as its own single-dash spelling",
            why: "once the tree carries the whole name, the same raw line must go silent",
            expect: Expect::Silent,
            raw: LSHW_USAGE_LINE.to_string(),
            root: node_with_flags("lshw", vec![whole_flag("format")]),
        },
        SelfCheck {
            name: "fuser's own bytes, `-SIGNAL` truncated to `-S` valued `\"IGNAL\"`",
            why: "the second usage line spells `-SIGNAL` whole, inside a nested bracket",
            expect: Expect::Fires(1),
            raw: FUSER_USAGE_LINES.to_string(),
            root: node_with_flags("fuser", vec![split_flag('S', "IGNAL")]),
        },
        SelfCheck {
            name: "rpcbind's own bundle, `-adhilswfr`, never claimed as one long name",
            why: "every one of its own letters is already a real short flag elsewhere in \
                  the tree, so this is the bundling family's shape, not this one",
            expect: Expect::Silent,
            raw: "usage: rpcbind [-adhilswfr]\n".to_string(),
            root: node_with_flags(
                "rpcbind",
                vec![
                    split_flag('a', "dhilswfr"),
                    whole_flag("d"),
                    whole_flag("h"),
                    whole_flag("i"),
                    whole_flag("l"),
                    whole_flag("s"),
                    whole_flag("w"),
                    whole_flag("f"),
                    whole_flag("r"),
                ],
            ),
        },
    ]
}
