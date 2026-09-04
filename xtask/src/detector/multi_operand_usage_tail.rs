//! `multi-operand-usage-tail` (atlas S-109): a usage line ends in a run of
//! two or more operands, bracketed or bare, and the tree carries fewer of
//! them than the line documents. `ar`'s trailing
//! `[member-name] [count] archive-file file...` documents four and the
//! tree carries none. Distinct from `crate::tail_operand`'s
//! `unparsed-positional`, which stops at the first non-flag group and so
//! covers a *single* trailing operand only — a run of two or more is a
//! different shape, per spec §13.1e rule 4. Fixture: `corpus/ar/audit-seed2/`.

use mandible_core::CommandNode;

pub struct Finding {
    pub operand: String,
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

/// `s` cut at the first run of `gap` or more consecutive spaces, the same
/// description-column boundary `crate::tail_operand` cuts at.
fn cut_before_wide_gap(s: &str, gap: usize) -> &str {
    let mut run = 0usize;
    let mut run_start = None;
    for (i, c) in s.char_indices() {
        if c == ' ' {
            if run == 0 {
                run_start = Some(i);
            }
            run += 1;
            if run >= gap {
                return &s[..run_start.unwrap()];
            }
        } else {
            run = 0;
        }
    }
    s
}

/// Whitespace-delimited groups, a `[...]` span kept as one group even with
/// internal spaces — the same convention `crate::tail_operand` groups by.
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

/// A real operand name: lowercase-led, trailing repetition dots trimmed
/// (atlas S-101), the rest alphanumeric/dash/underscore — never a flag
/// (`-`-led) or a flag-list placeholder.
fn operand_name(stripped: &str) -> Option<String> {
    let word = stripped.split_whitespace().next()?;
    let word = word.trim_end_matches('.');
    if word.is_empty() || word.starts_with('-') {
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
    Some(word.to_string())
}

/// A lowercase word standing in for the tool's own flag list collectively —
/// the same "stand-in, not an argument anyone passes" shape
/// `crate::tail_operand`'s own check recognizes.
fn is_flag_list_placeholder(word: &str) -> bool {
    matches!(
        word.to_ascii_lowercase().as_str(),
        "options" | "option" | "opts" | "args" | "arguments" | "flags"
    )
}

/// True when `stripped` is a flag (`-`-led once its own outer brackets are
/// trimmed) or names the tool's flag list collectively, checked word by
/// word so a two-word placeholder like `emulation options` still counts.
fn is_flag_or_placeholder_group(stripped: &str) -> bool {
    stripped.starts_with('-') || stripped.split_whitespace().any(is_flag_list_placeholder)
}

/// The usage line's own trailing contiguous run of operand-shaped groups,
/// in document order — empty unless the run holds two or more AND every
/// group before it is flag-shaped or a flag-list placeholder. The second
/// half is load-bearing: without it this fires on `apt-extracttemplates`'s
/// `file1 [file2 ...]` and `psfaddtable`'s `infont intable outfont`, both
/// judged `correct` — several *bare* operands with no flag list in front,
/// a different (and already declared out of scope by `crate::tail_operand`'s
/// own calibration) shape from an option list ending in an operand tail.
fn trailing_operand_run(usage_line: &str) -> Vec<String> {
    let lower = usage_line.to_ascii_lowercase();
    let Some(idx) = lower.find("usage:") else {
        return Vec::new();
    };
    let after = &usage_line[idx + "usage:".len()..];
    let before_desc =
        cut_before_wide_gap(after, mandible_extract::help_text::MIN_COLUMN_GAP_SPACES);
    let mut groups = group_tokens(before_desc.trim());
    if !groups.is_empty() {
        groups.remove(0); // the program name/path itself
    }
    let mut tail = Vec::new();
    while let Some(last) = groups.last() {
        let stripped = last.trim_matches(|c| c == '[' || c == ']');
        match operand_name(stripped) {
            Some(name) => {
                tail.push(name);
                groups.pop();
            }
            None => break,
        }
    }
    tail.reverse();
    if tail.len() < 2 || groups.is_empty() {
        return Vec::new();
    }
    if !groups
        .iter()
        .all(|g| is_flag_or_placeholder_group(g.trim_matches(|c| c == '[' || c == ']')))
    {
        return Vec::new();
    }
    tail
}

pub fn detect(raw: &str, root: &CommandNode) -> Report {
    let Some(usage_line) = first_usage_line(raw) else {
        return Report {
            findings: Vec::new(),
        };
    };
    let tail = trailing_operand_run(usage_line);
    let existing: Vec<&str> = root.positionals().map(|e| e.primary_name()).collect();
    let findings = tail
        .into_iter()
        .filter(|name| !existing.contains(&name.as_str()))
        .map(|operand| Finding {
            operand,
            usage_line: usage_line.to_string(),
        })
        .collect();
    Report { findings }
}

// ----------------------------------------------------------------------
// Self-checks
// ----------------------------------------------------------------------

use crate::detector::{Expect, SelfCheck};
use mandible_core::{Entity, Provenance, Source};

/// `ar`'s real usage line, byte-exact (`corpus/ar/audit-seed2/help.txt`).
pub(crate) const AR_USAGE: &str = "Usage: /usr/bin/ar [emulation options] [-]{dmpqrstx}\
[abcDfilMNoOPsSTuvV] [--plugin <name>] [member-name] [count] archive-file file...\n";

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
            name: "ar's own bytes, four trailing operands, tree carries none",
            why: "the defect itself: `[member-name] [count] archive-file file...` documents four \
                  operands and the root has zero positionals",
            expect: Expect::Fires(4),
            raw: AR_USAGE.to_string(),
            root: node_with_positionals("ar", &[]),
        },
        SelfCheck {
            name: "ar's own bytes, two of the four recovered",
            why: "fires per missing name, not per line: once two of the four are real \
                  positionals, only the other two count",
            expect: Expect::Fires(2),
            raw: AR_USAGE.to_string(),
            root: node_with_positionals("ar", &["archive-file", "file"]),
        },
        SelfCheck {
            name: "all four operands already recovered",
            why: "once the tree carries every trailing name, the same usage line must go silent",
            expect: Expect::Silent,
            raw: AR_USAGE.to_string(),
            root: node_with_positionals("ar", &["member-name", "count", "archive-file", "file"]),
        },
        SelfCheck {
            name: "a single trailing operand, the other detector's own shape",
            why: "one operand is `crate::tail_operand`'s `unparsed-positional` family, not this \
                  one — the run must be at least two long before this detector claims anything",
            expect: Expect::Silent,
            raw: "Usage: prog [OPTION]... FILE\n".to_string(),
            root: node_with_positionals("prog", &[]),
        },
        SelfCheck {
            name: "apt-extracttemplates's own bytes, bare operands with no flag list at all",
            why: "the false alarm calibration found: `file1 [file2 ...]` has nothing flag-shaped \
                  before it, a genuinely different shape from an option list ending in an \
                  operand tail, and this detector must stay out of it",
            expect: Expect::Silent,
            raw: "Usage: apt-extracttemplates file1 [file2 ...]\n".to_string(),
            root: node_with_positionals("apt-extracttemplates", &[]),
        },
        SelfCheck {
            name: "psfaddtable's own bytes, three bare operands and no flags",
            why: "the same false alarm shape with no brackets at all: every group is a bare \
                  operand, so there is no flag list in front for this family to claim",
            expect: Expect::Silent,
            raw: "Usage: psfaddtable infont intable outfont\n".to_string(),
            root: node_with_positionals("psfaddtable", &[]),
        },
    ]
}
