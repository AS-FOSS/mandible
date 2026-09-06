//! `usage-bracket-group-multiword-value` (atlas S-131): a usage-synopsis
//! bracket group holds one flag spelling followed by two or more plain
//! words with nothing else in the group (`caffeinate`'s `[-w Process
//! ID]`) — a value placeholder that itself holds a space. The generic
//! usage-line value reader used to take only the first word, so the
//! second silently vanished from the flag's own value name (and used to
//! surface elsewhere as a fabricated positional — see
//! `crate::detector::trailing_bracket_group_multiword_operand`, S-132, a
//! different shape entirely, per spec §13.1e rule 4). No seed-2/4/5/6
//! labelled tool carries this shape, so [`Detector::family`] returns
//! `None` (spec §13.1e rule 6). Fixture: corpus/caffeinate/26.6.2, issue
//! #135.

use crate::detector::{Detector, Expect, Scope, SelfCheck, ToolEvidence};
use mandible_core::CommandNode;

pub struct Finding {
    pub flag: String,
    pub words: String,
    pub line: String,
}

pub struct Report {
    pub findings: Vec<Finding>,
}

fn first_usage_line(raw: &str) -> Option<&str> {
    raw.lines()
        .find(|l| l.trim_start().to_ascii_lowercase().starts_with("usage:"))
}

/// Whitespace-delimited groups, a `[...]` span kept as one group even with
/// internal spaces — the same convention `crate::multi_operand_usage_tail`
/// groups by.
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

/// A plain prose word: letter-led, only letters/digits/`-`/`_` — never a
/// flag, a parenthetical aside, a bracket, or an alternation member.
fn plain_word(w: &str) -> bool {
    !w.is_empty()
        && w.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && w.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// `Some((flag, "word1 word2"))` when `group` (its own brackets still
/// attached) is a single flag spelling followed by two or more plain
/// words with nothing else in it — `[-w Process ID]`. `None` for
/// anything else: an alternation, a metavar, a parenthetical aside, or a
/// single-word value, all grammar this rule declines to reason about.
fn multiword_value_shape(group: &str) -> Option<(String, String)> {
    let stripped = group.trim_matches(|c| c == '[' || c == ']');
    let mut words = stripped.split_whitespace();
    let flag = words.next()?;
    if !flag.starts_with('-') || flag.len() < 2 || flag.contains('=') {
        return None;
    }
    let rest: Vec<&str> = words.collect();
    if rest.len() < 2 || !rest.iter().all(|w| plain_word(w)) {
        return None;
    }
    Some((flag.to_string(), rest.join(" ")))
}

fn flag_value_name<'a>(root: &'a CommandNode, flag: &str) -> Option<&'a str> {
    let trimmed = flag.trim_start_matches('-');
    root.flags()
        .find(|e| {
            (trimmed.chars().count() == 1 && e.short() == trimmed.chars().next())
                || e.long() == Some(trimmed)
        })
        .and_then(|e| e.value_name.as_deref())
}

pub fn detect(raw: &str, root: &CommandNode) -> Report {
    let Some(line) = first_usage_line(raw) else {
        return Report {
            findings: Vec::new(),
        };
    };
    let mut findings = Vec::new();
    for group in group_tokens(line) {
        let Some((flag, words)) = multiword_value_shape(&group) else {
            continue;
        };
        if flag_value_name(root, &flag) != Some(words.as_str()) {
            findings.push(Finding {
                flag,
                words,
                line: line.to_string(),
            });
        }
    }
    Report { findings }
}

pub struct UsageBracketGroupMultiwordValue;

impl Detector for UsageBracketGroupMultiwordValue {
    fn name(&self) -> &'static str {
        "usage-bracket-group-multiword-value"
    }

    fn family(&self) -> Option<&'static str> {
        None
    }

    fn describes(&self) -> &'static str {
        "a usage-synopsis bracket group holding one flag spelling followed by two or more plain \
         words with nothing else in the group — a value placeholder that itself holds a space, \
         which the generic usage-line value reader used to split at the first space"
    }

    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        detect(evidence.raw, evidence.root)
            .findings
            .iter()
            .map(|f| {
                format!(
                    "{} never carried value name {:?}, from {:?}",
                    f.flag, f.words, f.line
                )
            })
            .collect()
    }

    fn scope(&self) -> Scope {
        Scope::full()
    }

    fn self_checks(&self) -> Vec<SelfCheck> {
        self_checks()
    }
}

// ----------------------------------------------------------------------
// Self-checks
// ----------------------------------------------------------------------

use mandible_core::{Entity, Provenance, Source};

/// `caffeinate`'s real usage line, byte-exact (corpus/caffeinate/26.6.2's
/// `help.stderr.txt`).
pub(crate) const CAFFEINATE_USAGE: &str =
    "usage: caffeinate [-disu] [-t timeout] [-w Process ID] [command arguments...]\n";

fn flag_with_value(short: char, value_name: Option<&str>) -> Entity {
    let mut e = Entity::flag_spelled(
        Some(short),
        None,
        false,
        false,
        Provenance::single(Source::HelpText),
    );
    e.value_name = value_name.map(str::to_string);
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
            name: "caffeinate's own bytes, `-w`'s value split at its own space",
            why: "the defect itself: `-w`'s real value name is `Process ID`, and the tree keeps \
                  only `Process`",
            expect: Expect::Fires(1),
            raw: CAFFEINATE_USAGE.to_string(),
            root: node_with_flags("caffeinate", vec![flag_with_value('w', Some("Process"))]),
        },
        SelfCheck {
            name: "caffeinate's own bytes, `-w`'s value already whole",
            why: "once the value name reads `Process ID` in full, the same usage line must go \
                  silent",
            expect: Expect::Silent,
            raw: CAFFEINATE_USAGE.to_string(),
            root: node_with_flags("caffeinate", vec![flag_with_value('w', Some("Process ID"))]),
        },
        SelfCheck {
            name: "a single-word value, a different shape entirely",
            why: "`[-t timeout]` is one flag and one word — this rule only ever claims a group \
                  with two or more words after the flag",
            expect: Expect::Silent,
            raw: "usage: caffeinate [-t timeout]\n".to_string(),
            root: node_with_flags("caffeinate", vec![flag_with_value('t', Some("timeout"))]),
        },
        SelfCheck {
            name: "iptables' own parenthetical aside, not a multi-word value",
            why: "`-h (print this help information)` reads as a flag followed by prose in \
                  parentheses, never a value placeholder holding a space — the leading `(` \
                  fails this rule's plain-word check",
            expect: Expect::Silent,
            raw: "usage: iptables [-h (print this help information)]\n".to_string(),
            root: node_with_flags("iptables", vec![flag_with_value('h', None)]),
        },
    ]
}
