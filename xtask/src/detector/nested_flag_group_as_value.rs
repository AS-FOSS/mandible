//! `nested-flag-group-as-value` (round 11, atlas S-170): a usage-synopsis
//! bracket group whose own value spec is itself entirely option spellings
//! (optionally an ellipsis) — `dmsetup --help`'s `[-h|--help
//! [-c|-C|--columns]]` and `[-v|--verbose [-v|--verbose ...]]` — is a
//! nested optional group of flags, never a value. Fires when a flag's own
//! `value_name` still reads that way (the defect itself); the fix
//! (`help_text::grammar::try_value`, shared by every framework) refuses to
//! fabricate that value at all, so a repaired tree carries no such
//! `value_name` and this goes silent.

use crate::detector::{Detector, Expect, Scope, SelfCheck, ToolEvidence};
use mandible_core::CommandNode;

pub struct Finding {
    pub spelling: String,
    pub value: String,
}

pub struct Report {
    pub findings: Vec<Finding>,
}

/// Same shape test `mandible_extract::help_text::grammar` uses at the fix
/// site, re-derived locally (this crate cannot depend on
/// `mandible-extract`): an option spelling, dash-led with a name-shaped
/// tail, or a bare ellipsis word.
fn is_flag_spelling_or_ellipsis(tok: &str) -> bool {
    let tok = tok.trim();
    if tok.is_empty() {
        return false;
    }
    if tok.chars().all(|c| c == '.') {
        return true;
    }
    let stripped = tok.trim_start_matches('-');
    stripped.len() != tok.len()
        && !stripped.is_empty()
        && stripped
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// True when `value`, brackets stripped, is entirely `|`-separated option
/// spellings (each optionally followed by a bare ellipsis word) — never a
/// real value a reader could supply.
fn value_is_pure_flag_alternation(value: &str) -> bool {
    let content = value.trim().trim_start_matches('[').trim_end_matches(']');
    // Requires a real `|` alternation — a lone dash-led word (nvim's own
    // `--remote`'s value `-subcommand`) is an oddly spelled but real
    // value, never this shape.
    if !content.contains('|') {
        return false;
    }
    let parts: Vec<&str> = content
        .split('|')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    parts.len() > 1
        && parts.iter().all(|part| {
            let mut words = part.split_whitespace();
            let Some(first) = words.next() else {
                return false;
            };
            is_flag_spelling_or_ellipsis(first) && words.all(is_flag_spelling_or_ellipsis)
        })
}

pub fn detect(raw: &str, root: &CommandNode) -> Report {
    let mut findings = Vec::new();
    for flag in root.flags() {
        let Some(value) = flag.value_name.as_deref() else {
            continue;
        };
        if !value_is_pure_flag_alternation(value) {
            continue;
        }
        // A light existence check: the suspicious value text should occur
        // literally in the tool's own raw output, the same discipline the
        // existence oracle applies elsewhere (spec §13.1).
        let bare = value.trim_matches(|c| c == '[' || c == ']');
        if !raw.contains(bare) {
            continue;
        }
        let spelling = flag
            .long()
            .map(str::to_string)
            .or_else(|| flag.short().map(|c| c.to_string()))
            .unwrap_or_default();
        findings.push(Finding {
            spelling,
            value: value.to_string(),
        });
    }
    Report { findings }
}

pub struct NestedFlagGroupAsValue;

impl Detector for NestedFlagGroupAsValue {
    fn name(&self) -> &'static str {
        "nested-flag-group-as-value"
    }

    fn family(&self) -> Option<&'static str> {
        None
    }

    fn describes(&self) -> &'static str {
        "a flag whose own value_name is entirely `|`-separated option spellings (optionally an \
         ellipsis) — a nested optional group of flags a usage synopsis wrote, never a real value"
    }

    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        detect(evidence.raw, evidence.root)
            .findings
            .iter()
            .map(|f| {
                format!(
                    "{} carries value_name {:?}, itself only flag spellings",
                    f.spelling, f.value
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

pub(crate) const DMSETUP_HELP_ROW: &str = "[--version] [-h|--help [-c|-C|--columns]]\n";
pub(crate) const DMSETUP_VERBOSE_ROW: &str = "[-v|--verbose [-v|--verbose ...]] [-f|--force]\n";

fn flag_with_value(long: &str, short: Option<char>, value_name: Option<&str>) -> Entity {
    let mut e = Entity::flag_long(long, Provenance::single(Source::HelpText));
    if let Some(c) = short {
        e.spellings.insert(0, mandible_core::Spelling::short(c));
    }
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
            name: "dmsetup's own bytes, --help carrying its own nested group as a value",
            why: "the defect itself: -c|-C|--columns is a nested optional group of flags, never \
                  --help's own value",
            expect: Expect::Fires(1),
            raw: DMSETUP_HELP_ROW.to_string(),
            root: node_with_flags(
                "dmsetup",
                vec![flag_with_value("help", Some('h'), Some("-c|-C|--columns"))],
            ),
        },
        SelfCheck {
            name: "dmsetup's own bytes, --help fixed to carry no value",
            why: "once the fix refuses to fabricate the value, the same raw row must go silent",
            expect: Expect::Silent,
            raw: DMSETUP_HELP_ROW.to_string(),
            root: node_with_flags("dmsetup", vec![flag_with_value("help", Some('h'), None)]),
        },
        SelfCheck {
            name: "dmsetup's own bytes, --verbose's ellipsis-carrying nested group",
            why: "the same shape with a bare ellipsis word inside the nested group",
            expect: Expect::Fires(1),
            raw: DMSETUP_VERBOSE_ROW.to_string(),
            root: node_with_flags(
                "dmsetup",
                vec![flag_with_value(
                    "verbose",
                    Some('v'),
                    Some("-v|--verbose ..."),
                )],
            ),
        },
        SelfCheck {
            name: "a real choice value, never flag-shaped",
            why: "`--manglename {none|hex|auto}` names real choice words, not option spellings — \
                  this rule must stay silent",
            expect: Expect::Silent,
            raw: "[--manglename {none|hex|auto}]\n".to_string(),
            root: node_with_flags(
                "dmsetup",
                vec![flag_with_value("manglename", None, Some("none|hex|auto"))],
            ),
        },
    ]
}
