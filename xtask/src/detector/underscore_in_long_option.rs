//! `underscore-in-long-option` (atlas S-106): a long option whose name
//! contains `_` (`--auto_toc_prefix`, `--lu_cong`, `--extended_fields`) is
//! read as the prefix before the underscore plus the rest as a value name,
//! so the full name documented in the raw text never becomes a real long
//! spelling.
//!
//! Fixtures: `corpus/compactsnoop-bpfcc/audit-seed2/` (`--extended_fields`).
//! `icupkg` (`--auto_toc_prefix`) and `sg_luns` (`--lu_cong`) were never
//! drawn by the frozen queue and have no fixture.

use mandible_core::CommandNode;

pub struct Finding {
    pub token: String,
    pub line: String,
}

pub struct Report {
    pub findings: Vec<Finding>,
}

impl Report {
    pub fn finding_count(&self) -> usize {
        self.findings.len()
    }
}

/// Every `--name_with_underscore` token in `line`: a `--` sigil, then a
/// run of ASCII letters/digits/underscores starting with a letter and
/// containing at least one underscore, cut at the first character outside
/// that set (`]`, `=`, a space, a comma).
fn long_underscore_tokens(line: &str) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < chars.len() {
        if chars[i] == '-' && chars[i + 1] == '-' {
            let mut j = i + 2;
            let mut name = String::new();
            while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '_') {
                name.push(chars[j]);
                j += 1;
            }
            if name.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) && name.contains('_') {
                out.push(name);
            }
            i = j.max(i + 2);
        } else {
            i += 1;
        }
    }
    out
}

fn tree_has_long(root: &CommandNode, name: &str) -> bool {
    root.flags().any(|e| e.long() == Some(name))
}

pub fn detect(raw: &str, root: &CommandNode) -> Report {
    let mut seen = std::collections::BTreeSet::new();
    let mut findings = Vec::new();
    for line in raw.lines() {
        for name in long_underscore_tokens(line) {
            if !seen.insert(name.clone()) {
                continue;
            }
            if !tree_has_long(root, &name) {
                findings.push(Finding {
                    token: name,
                    line: line.to_string(),
                });
            }
        }
    }
    Report { findings }
}

// ----------------------------------------------------------------------
// Self-checks
// ----------------------------------------------------------------------

use crate::detector::{Expect, SelfCheck};
use mandible_core::{Entity, Provenance, Source};

/// icupkg's real row, byte-exact (`icupkg --help`, options block).
pub(crate) const ICUPKG_AUTO_ROW: &str =
    "\t--auto_toc_prefix            automatic ToC entries prefix\n";

/// sg_luns's real row, byte-exact (`sg_luns --help`, `where:` block).
pub(crate) const SG_LUNS_LU_CONG_ROW: &str =
    "    --lu_cong|-L         decode as if LU_CONG is set; used twice:\n";

/// compactsnoop-bpfcc's real row, byte-exact
/// (`corpus/compactsnoop-bpfcc/audit-seed2/help.txt`).
pub(crate) const COMPACTSNOOP_EXTENDED_ROW: &str =
    "  -e, --extended_fields\n                        show system memory state\n";

fn long_flag(name: &str) -> Entity {
    Entity::flag_spelled(
        None,
        Some(name.to_string()),
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

pub(crate) fn self_checks() -> Vec<SelfCheck> {
    vec![
        SelfCheck {
            name: "icupkg's own bytes, `--auto_toc_prefix` split at the underscore",
            why: "the defect itself: the tree carries only `--auto`, never the full name",
            expect: Expect::Fires(1),
            raw: ICUPKG_AUTO_ROW.to_string(),
            root: node_with_flags("icupkg", vec![long_flag("auto")]),
        },
        SelfCheck {
            name: "sg_luns's own bytes, `--lu_cong` split at the underscore",
            why: "the same shape on a second tool",
            expect: Expect::Fires(1),
            raw: SG_LUNS_LU_CONG_ROW.to_string(),
            root: node_with_flags("sg_luns", vec![long_flag("lu")]),
        },
        SelfCheck {
            name: "compactsnoop-bpfcc's own bytes, `--extended_fields` split",
            why: "the labelled seed-2 member: the tree carries only `--extended`",
            expect: Expect::Fires(1),
            raw: COMPACTSNOOP_EXTENDED_ROW.to_string(),
            root: node_with_flags("compactsnoop-bpfcc", vec![long_flag("extended")]),
        },
        SelfCheck {
            name: "`--extended_fields` recovered as a real long spelling",
            why: "once the tree carries the full underscored name, the same raw row must go \
                  silent",
            expect: Expect::Silent,
            raw: COMPACTSNOOP_EXTENDED_ROW.to_string(),
            root: node_with_flags("compactsnoop-bpfcc", vec![long_flag("extended_fields")]),
        },
        SelfCheck {
            name: "an ordinary long option with no underscore (`--help`)",
            why: "the token scan requires a literal `_` in the name, so a plain long option must \
                  never be claimed",
            expect: Expect::Silent,
            raw: "  -h, --help            show this help message and exit\n".to_string(),
            root: node_with_flags("prog", vec![long_flag("help")]),
        },
    ]
}
