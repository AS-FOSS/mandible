//! The `usage-optional-word-table` detector (atlas S-167): a `Usage:`
//! block's own rows repeat the tool's name plus one command word
//! abbreviated with a bracket suffix (`lldb-server`'s `v[ersion]`,
//! `g[dbserver]`, `p[latform]`), which a parser reading the block as an
//! ordinary usage synopsis folds into `usage` instead of recovering as
//! subcommands. Independent of `mandible-extract`'s own recognizer
//! (`help_text::sections::usage_optional_word`): this module re-derives
//! the raw shape from the captured bytes alone and checks the tree for
//! absence, the same separation every other detector in this crate keeps.

use mandible_core::CommandNode;

/// `token` reads as one command word abbreviated by a single bracket
/// group opened right after one leading lowercase letter (`v[ersion]`).
/// Mirrors `mandible-extract`'s own `optional_abbrev_word` shape test,
/// kept as an independent copy since that crate cannot import this one.
fn looks_like_optional_abbrev_word(token: &str) -> bool {
    let mut chars = token.chars();
    let Some(lead) = chars.next() else {
        return false;
    };
    if !lead.is_ascii_lowercase() {
        return false;
    }
    let rest = &token[lead.len_utf8()..];
    let Some(inner) = rest.strip_prefix('[').and_then(|s| s.strip_suffix(']')) else {
        return false;
    };
    !inner.is_empty() && inner.chars().all(|c| c.is_ascii_lowercase())
}

/// The recovered word (`v[ersion]` -> `version`) for a token
/// [`looks_like_optional_abbrev_word`] admits.
fn abbrev_word_name(token: &str) -> String {
    let mut chars = token.chars();
    let lead = chars.next().expect("checked non-empty by the caller");
    let rest = &token[lead.len_utf8()..];
    let inner = rest
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .expect("checked by looks_like_optional_abbrev_word");
    format!("{lead}{inner}")
}

/// True when `first` is `name` itself or names `name` under a different
/// spelling (a full path, or `name`'s own dotted stem) — the same
/// tolerance `mandible-extract`'s `starts_with_tool_name_spelled_differently`
/// keeps, reimplemented narrowly here.
fn names_the_tool(first: &str, name: &str) -> bool {
    let basename = first.rsplit('/').next().unwrap_or(first);
    basename == name || basename == name.split('.').next().unwrap_or(name)
}

/// One raw row this detector's own grammar recognizes, whose recovered
/// name is missing from the tree.
pub struct Finding {
    pub name: String,
    pub display: String,
}

pub struct Report {
    pub findings: Vec<Finding>,
}

impl Report {
    pub fn finding_count(&self) -> usize {
        self.findings.len()
    }
}

const MIN_ROWS: usize = 2;

/// Whether `root`'s own subcommands already carry a node named `name`
/// with `display_name` (or a bare `name` when the row carried no bracket
/// suffix — never this shape, since every row here does) matching
/// `display`.
fn tree_attests(root: &CommandNode, name: &str, display: &str) -> bool {
    root.subcommands
        .iter()
        .any(|c| c.name == name && c.display_name.as_deref() == Some(display))
}

pub fn detect(raw: &str, root: &CommandNode) -> Report {
    let lines: Vec<&str> = raw.lines().collect();
    let Some(heading_idx) = lines
        .iter()
        .position(|l| l.trim().eq_ignore_ascii_case("usage:"))
    else {
        return Report {
            findings: Vec::new(),
        };
    };
    let mut rows: Vec<(String, String)> = Vec::new();
    let mut i = heading_idx + 1;
    while let Some(&line) = lines.get(i) {
        let t = line.trim();
        if t.is_empty() {
            break;
        }
        let mut words = t.split_whitespace();
        let Some(first) = words.next() else {
            break;
        };
        if !names_the_tool(first, &root.name) {
            break;
        }
        let Some(word) = words.next() else {
            break;
        };
        if !looks_like_optional_abbrev_word(word) {
            break;
        }
        if !words.all(|w| w.starts_with('[') && w.ends_with(']')) {
            break;
        }
        rows.push((abbrev_word_name(word), word.to_string()));
        i += 1;
    }
    if rows.len() < MIN_ROWS {
        return Report {
            findings: Vec::new(),
        };
    }
    let findings = rows
        .into_iter()
        .filter(|(name, display)| !tree_attests(root, name, display))
        .map(|(name, display)| Finding { name, display })
        .collect();
    Report { findings }
}

// ----------------------------------------------------------------------
// Self-checks
// ----------------------------------------------------------------------

use crate::detector::{Expect, SelfCheck};
use mandible_core::{Provenance, Source};

pub(crate) const LLDB_SERVER_USAGE: &str = "Usage:\n  lldb-server v[ersion]\n  lldb-server g[dbserver] [options]\n  lldb-server p[latform] [options]\nInvoke subcommand for additional help\n";

fn node(name: &str) -> CommandNode {
    CommandNode::new(name, Provenance::single(Source::HelpText))
}

pub(crate) fn self_checks() -> Vec<SelfCheck> {
    vec![
        SelfCheck {
            name: "lldb-server's own bytes, a bare tree missing all three subcommands",
            why: "the defect itself: a tool whose only structure is a bare `Usage:` block \
                  reads as ordinary usage text, not subcommands, unless the tree already \
                  carries the three abbreviated words",
            expect: Expect::Fires(3),
            raw: LLDB_SERVER_USAGE.to_string(),
            root: node("lldb-server"),
        },
        SelfCheck {
            name: "a correctly repaired tree",
            why: "once every row's own word reaches the tree with its source spelling as \
                  `display_name`, the detector has nothing left to report",
            expect: Expect::Silent,
            raw: LLDB_SERVER_USAGE.to_string(),
            root: {
                let mut root = node("lldb-server");
                let mut version = node("version");
                version.display_name = Some("v[ersion]".to_string());
                let mut gdbserver = node("gdbserver");
                gdbserver.display_name = Some("g[dbserver]".to_string());
                let mut platform = node("platform");
                platform.display_name = Some("p[latform]".to_string());
                root.subcommands = vec![version, gdbserver, platform];
                root
            },
        },
        SelfCheck {
            name: "an ordinary usage line with real flags, not this shape",
            why: "a genuine usage synopsis (`Usage: foo [OPTIONS] <file>`) carries content on \
                  the `Usage:` line itself and must never be misread as this bare-heading \
                  shape",
            expect: Expect::Silent,
            raw: "Usage: foo [OPTIONS] <file>\n".to_string(),
            root: node("foo"),
        },
        SelfCheck {
            name: "ar's modifier-table shape, never mistaken for this one",
            why: "`r[ab][f][u]` opens a SECOND bracket group right after the first closes, \
                  which this detector's own single-bracket-group grammar refuses; ar's shape \
                  reaches the tree through the unrelated `commands:`-heading path, never a \
                  bare `Usage:` block",
            expect: Expect::Silent,
            raw: "Usage:\n  ar r[ab][f][u]\n  ar m[ab]\n".to_string(),
            root: node("ar"),
        },
        SelfCheck {
            name: "a single row is too cheap a coincidence to act on",
            why: "one abbreviated word alone could be a coincidence; the floor is two, the \
                  same floor the parser itself uses",
            expect: Expect::Silent,
            raw: "Usage:\n  foo v[ersion]\n".to_string(),
            root: node("foo"),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fires_on_lldb_servers_own_bytes_against_a_bare_tree() {
        let report = detect(LLDB_SERVER_USAGE, &node("lldb-server"));
        assert_eq!(report.finding_count(), 3);
    }

    #[test]
    fn every_self_check_holds() {
        for case in self_checks() {
            let expected = case.expect.expected_hits();
            let report = detect(&case.raw, &case.root);
            assert_eq!(
                report.finding_count(),
                expected,
                "{}: expected {} finding(s), got {:?}",
                case.name,
                expected,
                report
                    .findings
                    .iter()
                    .map(|f| &f.display)
                    .collect::<Vec<_>>()
            );
        }
    }
}
