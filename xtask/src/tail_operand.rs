//! The `unparsed-positional` tail-operand detector (atlas S-041): a usage
//! line's own last operand token never becomes a positional entity, so the
//! root reports zero positionals while its synopsis names one.
//!
//! Fixtures: `corpus/bashbug/audit-seed4/`, `corpus/lessecho/audit-seed4/`,
//! `corpus/vim.basic/audit-seed4/`. Shape detail in `docs/shapes.md` S-041.

use mandible_core::CommandNode;

pub struct Finding {
    /// The operand name lifted from the usage line's own tail, e.g.
    /// `"file"`.
    pub operand: String,
    /// The usage line it came from, verbatim.
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

/// The first physical line that looks like a usage synopsis (`usage:` or
/// `Usage:`, anywhere at the start of the line's own trimmed text).
fn first_usage_line(raw: &str) -> Option<&str> {
    raw.lines()
        .find(|l| l.trim_start().to_ascii_lowercase().starts_with("usage:"))
}

/// `s` cut at the first run of `gap` or more consecutive spaces — the
/// description column boundary a real usage line's inline trailing prose
/// (vim.basic's `edit specified file(s)`) sits behind. Char-indexed
/// throughout, never a raw byte slice, so a non-ASCII description cannot
/// panic this on a boundary that isn't a char boundary.
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

/// Split `s` into whitespace-delimited groups, treating a `[...]` span as
/// one group even when it contains internal spaces (`"[file ..]"` stays
/// one token, matching how a synopsis's own bracket notation groups an
/// optional clause). Unmatched brackets degrade gracefully: once opened, a
/// group simply runs to the next matching close or the end of the string.
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

/// A lowercase word standing in for the tool's own flag list collectively
/// (`[arguments]`, `[options]`) rather than naming a real operand — the
/// same "stand-in, not an argument anyone passes" shape
/// `crate::existence`'s operand check already recognizes for `OPTION`-style
/// placeholders, spelled lowercase here because that's how a synopsis like
/// vim.basic's `[arguments]` writes it. Checked case-insensitively so
/// `[Options]` reads the same as `[options]`.
fn is_flag_list_placeholder(word: &str) -> bool {
    matches!(
        word.to_ascii_lowercase().as_str(),
        "options" | "option" | "opts" | "args" | "arguments" | "flags"
    )
}

/// Whether `group` (brackets stripped) is something other than a real
/// operand: a `-`-led flag/flag-cluster, or [`is_flag_list_placeholder`].
fn is_flag_or_placeholder(stripped: &str) -> bool {
    stripped.starts_with('-') || is_flag_list_placeholder(stripped)
}

/// The usage line's own trailing operand, or `None` when the last token is
/// flag-shaped, a flag-list placeholder, elliptical-only, empty, not
/// lowercase-led, or when any group *before* it is something other than a
/// flag or a flag-list placeholder.
///
/// The last condition is load-bearing: without it calibration fired on
/// `apt-extracttemplates` and `psfaddtable`, whose usage lines are several
/// bare operands rather than flags plus one trailing operand. That is a
/// different shape and this detector does not claim it.
fn tail_operand(usage_line: &str) -> Option<String> {
    let lower = usage_line.to_ascii_lowercase();
    let idx = lower.find("usage:")?;
    let after = &usage_line[idx + "usage:".len()..];
    let before_desc =
        cut_before_wide_gap(after, mandible_extract::help_text::MIN_COLUMN_GAP_SPACES);
    let mut groups = group_tokens(before_desc.trim());
    if groups.len() < 2 {
        return None;
    }
    groups.remove(0); // the program name itself
    while let Some(last) = groups.last() {
        let stripped = last.trim_matches(|c| c == '[' || c == ']');
        if !stripped.is_empty() && stripped.chars().all(|c| c == '.') {
            groups.pop();
        } else {
            break;
        }
    }
    let last = groups.last()?;
    let stripped = last.trim_matches(|c| c == '[' || c == ']');
    let word = stripped.split_whitespace().next()?;
    let word = word.trim_end_matches('.');
    if word.is_empty() || word.starts_with('-') || is_flag_list_placeholder(word) {
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
    for earlier in &groups[..groups.len() - 1] {
        let earlier_stripped = earlier.trim_matches(|c| c == '[' || c == ']');
        if !is_flag_or_placeholder(earlier_stripped) {
            return None;
        }
    }
    Some(word.to_string())
}

pub fn detect(raw: &str, root: &CommandNode) -> Report {
    if root.positionals().next().is_some() {
        // Already recovered — whatever else this tool's TUI shows, the
        // tree itself is not missing this operand.
        return Report {
            findings: Vec::new(),
        };
    }
    let Some(usage_line) = first_usage_line(raw) else {
        return Report {
            findings: Vec::new(),
        };
    };
    let Some(operand) = tail_operand(usage_line) else {
        return Report {
            findings: Vec::new(),
        };
    };
    Report {
        findings: vec![Finding {
            operand,
            usage_line: usage_line.to_string(),
        }],
    }
}

// ----------------------------------------------------------------------
// Self-checks
// ----------------------------------------------------------------------

use crate::detector::{Expect, SelfCheck};
use mandible_core::{Entity, Provenance, Source};

/// `bashbug --help`'s real first two lines, byte-exact
/// (`corpus/bashbug/audit-seed4/help.txt`).
pub(crate) const BASHBUG_USAGE: &str =
    "Usage: /usr/bin/bashbug [--help] [--version] [bug-report-email-address]\n";

/// `lessecho --help`'s real stderr usage line, byte-exact
/// (`corpus/lessecho/audit-seed4/help.stderr.txt`).
pub(crate) const LESSECHO_USAGE: &str =
    "usage: lessecho [-ox] [-cx] [-pn] [-dn] [-mx] [-nn] [-ex] [-fn] [-a] file ...\n";

/// `vim.basic --help`'s real first usage line, byte-exact
/// (`corpus/vim.basic/audit-seed4/help.txt`).
pub(crate) const VIM_USAGE: &str =
    "Usage: vim [arguments] [file ..]       edit specified file(s)\n";

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

fn node_with_positional(name: &str, positional: &str) -> CommandNode {
    let mut root = CommandNode::new(name, Provenance::single(Source::HelpText));
    let mut p = Entity::positional(positional, Provenance::single(Source::HelpText));
    p.required = true;
    root.set_entities_of(mandible_core::EntityKind::Positional, vec![p]);
    root
}

pub(crate) fn self_checks() -> Vec<SelfCheck> {
    vec![
        SelfCheck {
            name: "bashbug's own bytes, operand dropped",
            why: "the defect itself: `[bug-report-email-address]` is the usage line's own \
                  trailing bracketed operand, and the root has zero positionals",
            expect: Expect::Fires(1),
            raw: BASHBUG_USAGE.to_string(),
            root: node_with_flags(
                "bashbug",
                vec![flag(Some("help"), None), flag(Some("version"), None)],
            ),
        },
        SelfCheck {
            name: "lessecho's own bytes, bare trailing operand dropped",
            why: "the same shape without brackets: `file ...` is bare and required, not \
                  optional, and the tail-token rule must reach it exactly the same way",
            expect: Expect::Fires(1),
            raw: LESSECHO_USAGE.to_string(),
            root: node_with_flags(
                "lessecho",
                vec![
                    flag(None, Some('o')),
                    flag(None, Some('c')),
                    flag(None, Some('p')),
                    flag(None, Some('d')),
                    flag(None, Some('m')),
                    flag(None, Some('n')),
                    flag(None, Some('e')),
                    flag(None, Some('f')),
                    flag(None, Some('a')),
                ],
            ),
        },
        SelfCheck {
            name: "vim.basic's own bytes, operand dropped behind inline prose",
            why: "the hardest of the three: `[file ..]` sits before a trailing inline \
                  description on the same physical line, and the description-column cut must \
                  land in the right place for the tail group to still be `[file ..]` and not \
                  a fragment of the description",
            expect: Expect::Fires(1),
            raw: VIM_USAGE.to_string(),
            root: node_with_flags(
                "vim.basic",
                vec![flag(None, Some('v')), flag(None, Some('e'))],
            ),
        },
        SelfCheck {
            name: "lessecho's operand, already recovered",
            why: "the positional-present gate: once the tree carries `file` as a real \
                  positional, the same raw bytes must go silent — whatever the TUI shows \
                  elsewhere is not this detector's concern",
            expect: Expect::Silent,
            raw: LESSECHO_USAGE.to_string(),
            root: node_with_positional("lessecho", "file"),
        },
        SelfCheck {
            name: "a usage line ending in a flag, not an operand",
            why: "the nearest real false positive: the last bracket group is flag-shaped \
                  (`[-h]`) once its own brackets are stripped, so there is no operand to claim \
                  and the detector must find none",
            expect: Expect::Silent,
            raw: "Usage: prog [-v] [-h]\n".to_string(),
            root: node_with_flags("prog", vec![flag(None, Some('v')), flag(None, Some('h'))]),
        },
        SelfCheck {
            name: "an ALL-CAPS metavariable tail, deliberately out of scope",
            why: "the declared scope limit: an upper-case-led operand (`FILE`) is easy to \
                  confuse with a flag's own value-name convention, so this detector requires a \
                  lowercase-led name and stays silent here on purpose, not by accident",
            expect: Expect::Silent,
            raw: "Usage: prog [OPTION]... FILE\n".to_string(),
            root: node_with_flags("prog", vec![]),
        },
        SelfCheck {
            name: "multiple bare operands, not one flag-list-plus-tail",
            why: "the false alarm calibration actually found: `apt-extracttemplates`'s \
                  `Usage: apt-extracttemplates file1 [file2 ...]` ends in an operand-shaped tail \
                  too, but `file1` earlier on the line is *also* bare and non-flag — this is a \
                  multiple-bare-operand usage line, a genuinely different (and harder) shape than \
                  a flag list with one operand tacked on the end, and the earlier-groups gate \
                  must keep it out",
            expect: Expect::Silent,
            raw: "Usage: apt-extracttemplates file1 [file2 ...]\n".to_string(),
            root: node_with_flags("apt-extracttemplates", vec![]),
        },
        SelfCheck {
            name: "a flag-list placeholder as the tail, not a real operand",
            why: "the other false alarm calibration found: `rust-lldb`'s \
                  `USAGE: lldb [options]` ends in `options`, a stand-in for the tool's own flag \
                  list (the same shape `[arguments]` is in vim.basic's true positive), not a \
                  real operand — must stay silent on the word itself, not just when it is a \
                  *preceding* group",
            expect: Expect::Silent,
            raw: "USAGE: lldb [options]\n".to_string(),
            root: node_with_flags("lldb", vec![]),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_bashbugs_bracketed_tail_operand() {
        let root = node_with_flags(
            "bashbug",
            vec![flag(Some("help"), None), flag(Some("version"), None)],
        );
        let report = detect(BASHBUG_USAGE, &root);
        assert_eq!(report.finding_count(), 1);
        assert_eq!(report.findings[0].operand, "bug-report-email-address");
    }

    #[test]
    fn finds_lessechos_bare_tail_operand() {
        let root = node_with_flags("lessecho", vec![flag(None, Some('a'))]);
        let report = detect(LESSECHO_USAGE, &root);
        assert_eq!(report.finding_count(), 1);
        assert_eq!(report.findings[0].operand, "file");
    }

    #[test]
    fn finds_vims_tail_operand_behind_inline_prose() {
        let root = node_with_flags("vim.basic", vec![flag(None, Some('v'))]);
        let report = detect(VIM_USAGE, &root);
        assert_eq!(report.finding_count(), 1);
        assert_eq!(report.findings[0].operand, "file");
    }

    #[test]
    fn stays_silent_once_the_positional_is_recovered() {
        let root = node_with_positional("lessecho", "file");
        assert_eq!(detect(LESSECHO_USAGE, &root).finding_count(), 0);
    }

    #[test]
    fn stays_silent_on_a_flag_shaped_tail() {
        let root = node_with_flags("prog", vec![flag(None, Some('h'))]);
        assert_eq!(detect("Usage: prog [-v] [-h]\n", &root).finding_count(), 0);
    }

    #[test]
    fn every_self_check_holds() {
        for case in self_checks() {
            let expected = match case.expect {
                Expect::Fires(n) => n,
                Expect::Silent => 0,
            };
            let report = detect(&case.raw, &case.root);
            assert_eq!(
                report.finding_count(),
                expected,
                "{}: expected {} finding(s)",
                case.name,
                expected
            );
        }
    }
}
