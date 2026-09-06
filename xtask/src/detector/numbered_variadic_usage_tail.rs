//! `numbered-variadic-usage-tail` (atlas S-136): a usage line's trailing
//! tail is written `X1 [X2 ...]`, and the tree carries no positional for
//! it. `apt-sortpkgs`'s `[options] file1 [file2 ...]` names one variadic
//! positional, `file`, not two operands; round 6 declined the wider
//! bare-tail shape (S-109) as ambiguous, but the numbering here is the
//! evidence that shape lacks. Fixture: `corpus/apt-sortpkgs/2.8.3`. A
//! local, independent copy of `recover_primary_tail_operands`'s own
//! grouping and operand parsing, not an import.

use mandible_core::CommandNode;

pub struct Finding {
    pub positional: String,
    pub usage_line: String,
}

pub struct Report {
    pub findings: Vec<Finding>,
}

impl Report {
    pub fn finding_count(&self) -> usize {
        self.findings.len()
    }
}

fn first_usage_line(raw: &str) -> Option<&str> {
    raw.lines()
        .find(|l| l.trim_start().to_ascii_lowercase().starts_with("usage:"))
}

fn group_tokens(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut depth = 0i32;
    for c in s.chars() {
        match c {
            '[' => {
                depth += 1;
                cur.push(c);
            }
            ']' => {
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

/// Usage-synopsis tokens that stand in for the tool's own option list
/// rather than naming an operand (`tar [OPTION...]`) — the same closed
/// set `mandible_extract`'s own `is_option_list_placeholder` reads, a
/// local copy rather than an import.
fn is_option_list_placeholder(word: &str) -> bool {
    ["option", "options", "flag", "flags", "arguments"]
        .iter()
        .any(|p| word.eq_ignore_ascii_case(p))
}

/// One trailing group as `(name, required, repeatable)` — the same
/// reading `recover_primary_tail_operands`'s own `parse_operand_group`
/// gives, a local copy rather than an import.
fn parse_operand_group(group: &str) -> Option<(String, bool, bool)> {
    let stripped = group.trim_matches(|c| c == '[' || c == ']');
    let mut words = stripped.split_whitespace();
    let raw_word = words.next()?;
    let marker_repeat = {
        let trimmed = raw_word.trim_end_matches([']', ')']);
        trimmed.len() - trimmed.trim_end_matches('.').len() >= 2
    };
    let word = raw_word.trim_end_matches('.');
    if word.is_empty() || word.starts_with('-') || is_option_list_placeholder(word) {
        return None;
    }
    let mut chars = word.chars();
    let first = chars.next()?;
    if !first.is_ascii_lowercase() {
        return None;
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return None;
    }
    let mut inline_repeat = false;
    for w in words {
        if w.chars().all(|c| c == '.') {
            inline_repeat = true;
        } else {
            return None;
        }
    }
    let required = !group.starts_with('[');
    Some((word.to_string(), required, marker_repeat || inline_repeat))
}

/// Split `word`'s own trailing run of ASCII digits off as an integer.
fn split_trailing_integer(word: &str) -> Option<(&str, u64)> {
    let digit_count = word.chars().rev().take_while(char::is_ascii_digit).count();
    if digit_count == 0 || digit_count == word.len() {
        return None;
    }
    let (stem, digits) = word.split_at(word.len() - digit_count);
    digits.parse::<u64>().ok().map(|n| (stem, n))
}

/// `first` and `second`, the usage line's own last two groups, read as
/// `X1 [X2 ...]`: `first` required and not itself repeatable, `second`
/// optional and marked repeatable, both sharing an alphabetic stem with
/// `second`'s trailing integer exactly one more than `first`'s.
fn numbered_variadic_tail_stem(
    first: &(String, bool, bool),
    second: &(String, bool, bool),
) -> Option<String> {
    let (name1, required1, repeat1) = first;
    let (name2, required2, repeat2) = second;
    if !*required1 || *repeat1 || *required2 || !*repeat2 {
        return None;
    }
    let (stem1, n1) = split_trailing_integer(name1)?;
    let (stem2, n2) = split_trailing_integer(name2)?;
    if stem1.is_empty() || stem1 != stem2 || n2 != n1 + 1 {
        return None;
    }
    Some(stem1.to_string())
}

/// Walk backward from the tail, collecting the maximal run of
/// operand-shaped groups, in source order — the same walk
/// `recover_primary_tail_operands` does, so this detector's own
/// `collected` matches the fix's exactly for any given line.
fn collect_trailing_operands(groups: &mut Vec<String>) -> Vec<(String, bool, bool)> {
    let mut collected = Vec::new();
    let mut pending_repeat = false;
    while let Some(last) = groups.last() {
        let bare = last.trim_matches(|c| c == '[' || c == ']');
        if !bare.is_empty() && bare.chars().all(|c| c == '.') {
            pending_repeat = true;
            groups.pop();
            continue;
        }
        let Some((word, required, marker_repeat)) = parse_operand_group(last) else {
            break;
        };
        collected.push((word, required, marker_repeat || pending_repeat));
        pending_repeat = false;
        groups.pop();
    }
    collected.reverse();
    collected
}

/// The usage line's own numbered-tail stem, `None` unless its trailing
/// operand-shaped run is exactly two groups long and reads as
/// `X1 [X2 ...]`.
fn detect_stem(usage_line: &str) -> Option<String> {
    let lower = usage_line.to_ascii_lowercase();
    let idx = lower.find("usage:")?;
    let after = &usage_line[idx + "usage:".len()..];
    let mut groups = group_tokens(after.trim());
    if groups.is_empty() {
        return None;
    }
    groups.remove(0); // the program name itself
    let collected = collect_trailing_operands(&mut groups);
    let [first, second] = collected.as_slice() else {
        return None;
    };
    numbered_variadic_tail_stem(first, second)
}

pub fn detect(raw: &str, root: &CommandNode) -> Report {
    let Some(usage_line) = first_usage_line(raw) else {
        return Report {
            findings: Vec::new(),
        };
    };
    let Some(stem) = detect_stem(usage_line) else {
        return Report {
            findings: Vec::new(),
        };
    };
    let already_present = root.positionals().any(|p| p.primary_name() == stem);
    let findings = if already_present {
        Vec::new()
    } else {
        vec![Finding {
            positional: stem,
            usage_line: usage_line.to_string(),
        }]
    };
    Report { findings }
}

// ----------------------------------------------------------------------
// Self-checks
// ----------------------------------------------------------------------

use crate::detector::{Expect, SelfCheck};
use mandible_core::{Entity, Provenance, Source};

pub(crate) const APT_SORTPKGS_USAGE: &str = "Usage: apt-sortpkgs [options] file1 [file2 ...]\n";

fn node_with_positionals(name: &str, names: &[&str]) -> CommandNode {
    let mut root = CommandNode::new(name, Provenance::single(Source::HelpText));
    let entities = names
        .iter()
        .map(|n| Entity::positional(*n, Provenance::single(Source::HelpText)))
        .collect();
    root.set_entities_of(mandible_core::EntityKind::Positional, entities);
    root
}

pub(crate) fn self_checks() -> Vec<SelfCheck> {
    vec![
        SelfCheck {
            name: "apt-sortpkgs's own bytes, no positional recovered at all",
            why: "the defect itself: `file1 [file2 ...]` names one variadic positional, file, \
                  and the tree carries none",
            expect: Expect::Fires(1),
            raw: APT_SORTPKGS_USAGE.to_string(),
            root: node_with_positionals("apt-sortpkgs", &[]),
        },
        SelfCheck {
            name: "the variadic file positional already recovered",
            why: "once the fix lands, the same usage line must go silent",
            expect: Expect::Silent,
            raw: APT_SORTPKGS_USAGE.to_string(),
            root: node_with_positionals("apt-sortpkgs", &["file"]),
        },
        SelfCheck {
            name: "apt-extracttemplates's own bytes, no `[options]` group in front at all",
            why: "the numbering is its own evidence: the shape is claimed even with no earlier \
                  flag-list group, unlike the declined bare-tail ambiguity (S-109)",
            expect: Expect::Fires(1),
            raw: "Usage: apt-extracttemplates file1 [file2 ...]\n".to_string(),
            root: node_with_positionals("apt-extracttemplates", &[]),
        },
        SelfCheck {
            name: "a bare tail with no numbering at all, gcc's own shape",
            why: "S-109's own declined ambiguity: two bare names with no shared stem and no \
                  successor integer must never be claimed by this narrower rule",
            expect: Expect::Silent,
            raw: "Usage: widget [options] infile [outfile ...]\n".to_string(),
            root: node_with_positionals("widget", &[]),
        },
        SelfCheck {
            name: "a numbered pair whose integers are not consecutive",
            why: "the successor-integer test must hold exactly: file1 beside file3 names no \
                  real repetition convention this rule recognizes",
            expect: Expect::Silent,
            raw: "Usage: widget [options] file1 [file3 ...]\n".to_string(),
            root: node_with_positionals("widget", &[]),
        },
        SelfCheck {
            name: "psfaddtable's own bytes, three bare operands and no numbering",
            why: "a run of more than two groups is out of this rule's narrow scope regardless \
                  of naming",
            expect: Expect::Silent,
            raw: "Usage: psfaddtable infont intable outfont\n".to_string(),
            root: node_with_positionals("psfaddtable", &[]),
        },
    ]
}
