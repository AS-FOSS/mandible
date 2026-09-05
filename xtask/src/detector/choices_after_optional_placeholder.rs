//! `choices-after-optional-placeholder` (atlas S-120): a docopt bracket
//! row's own trailing bare `|`-separated choice list (`pvdisplay`'s own
//! `--units [Number]r|R|h|...`, `--configreport
//! log|vg|lv|pv|pvseg|seg`) never attaches as the flag's `choices`,
//! whether the trailing list follows a bracketed placeholder or stands
//! in for the whole value spec on its own.
//!
//! Reimplements the row shape independently rather than importing
//! `mandible_extract::help_text::sections`'s own private
//! `bracket_flag_row_content`/`trailing_choice_list` (crate-internal,
//! `pub(super)`) — the same "reimplement, don't import" choice
//! `usage_program_word_mismatch` already makes, so this detector can
//! never disagree with the fix only by drifting out of sync with a
//! private helper it can't see change.
//!
//! Fixtures: `corpus/pvdisplay/2.03.16/`.

use mandible_core::CommandNode;

pub struct Finding {
    pub long: String,
    pub choices: Vec<String>,
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

/// A bare identifier: letters/digits, `_`/`-`, first character
/// alphanumeric — the shape a choice value is written in.
fn is_choice_token(token: &str) -> bool {
    let mut chars = token.chars();
    chars.next().is_some_and(|c| {
        c.is_ascii_alphanumeric()
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    })
}

/// The row's own docopt bracket-group content: `trimmed` is exactly one
/// `[...]` group whose content starts with `-`. Mirrors
/// `help_text::grammar::bracket_flag_row_content`'s own two conditions.
fn bracket_row_content(trimmed: &str) -> Option<&str> {
    let inner = trimmed.strip_prefix('[')?.strip_suffix(']')?.trim();
    inner.starts_with('-').then_some(inner)
}

/// The long name a bracket row's own content documents, and the trailing
/// bare `|`-separated choice list glued or spaced onto it, if any — see
/// `help_text::sections::flag_rows::trailing_choice_list`'s own doc
/// comment for the identical backward-scan rule (stops at the first
/// space or bracket, so an alias separator that is also `|`,
/// `-A|--autobackup y|n`, is never absorbed).
fn long_name_and_choices(content: &str) -> (Option<&str>, Vec<String>) {
    // The row's leading segment (up to the first space) may itself be an
    // alias run (`-d|--debug`, `-A|--autobackup`); the long name is
    // whichever `|`-separated member of it starts with `--`.
    let first_segment = content.split_whitespace().next().unwrap_or(content);
    let long = first_segment
        .split('|')
        .find_map(|t| t.trim_end_matches(',').strip_prefix("--"));
    let trimmed_end = content.trim_end();
    let mut start = trimmed_end.len();
    for (i, c) in trimmed_end.char_indices().rev() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '|' {
            start = i;
        } else {
            break;
        }
    }
    let before = trimmed_end[..start].trim_end();
    let tail = &trimmed_end[start..];
    let choices = if !before.is_empty() && tail.contains('|') {
        let tokens: Vec<&str> = tail.split('|').collect();
        if tokens.len() >= 2 && tokens.iter().all(|t| is_choice_token(t)) {
            tokens.into_iter().map(str::to_string).collect()
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };
    (long, choices)
}

pub fn detect(raw: &str, root: &CommandNode) -> Report {
    let mut findings = Vec::new();
    for line in raw.lines() {
        let Some(content) = bracket_row_content(line.trim()) else {
            continue;
        };
        let (Some(long), choices) = long_name_and_choices(content) else {
            continue;
        };
        if choices.is_empty() {
            continue;
        }
        let attached = root.flags().any(|e| {
            e.long() == Some(long)
                && choices
                    .iter()
                    .all(|c| e.choices.iter().any(|ec| &ec.name == c))
        });
        if !attached {
            findings.push(Finding {
                long: long.to_string(),
                choices,
                line: line.to_string(),
            });
        }
    }
    Report { findings }
}

// ----------------------------------------------------------------------
// Self-checks
// ----------------------------------------------------------------------

use crate::detector::{Expect, SelfCheck};
use mandible_core::{Choice, Entity, Provenance, Source};

/// pvdisplay's real row, byte-exact (`corpus/pvdisplay/2.03.16/help.txt`).
pub(crate) const PVDISPLAY_CONFIGREPORT_ROW: &str =
    "\t[    --configreport log|vg|lv|pv|pvseg|seg ]\n";

fn long_flag(name: &str, choices: &[&str]) -> Entity {
    let mut e = Entity::flag_spelled(
        None,
        Some(name.to_string()),
        false,
        false,
        Provenance::single(Source::HelpText),
    );
    e.choices = choices.iter().map(|c| Choice::bare(*c)).collect();
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
            name: "pvdisplay's own bytes, `--configreport`'s choice list never attached",
            why: "the defect itself: the tree carries `--configreport` with no choices at all",
            expect: Expect::Fires(1),
            raw: PVDISPLAY_CONFIGREPORT_ROW.to_string(),
            root: node_with_flags("pvdisplay", vec![long_flag("configreport", &[])]),
        },
        SelfCheck {
            name: "`--configreport`'s choices already attached",
            why: "once every listed choice is on the entity, the same raw row must go silent",
            expect: Expect::Silent,
            raw: PVDISPLAY_CONFIGREPORT_ROW.to_string(),
            root: node_with_flags(
                "pvdisplay",
                vec![long_flag(
                    "configreport",
                    &["log", "vg", "lv", "pv", "pvseg", "seg"],
                )],
            ),
        },
        SelfCheck {
            name: "an ordinary bracket row with no trailing choice list",
            why: "a row with nothing after its own flag name documents no choices, so it must \
                  never be claimed",
            expect: Expect::Silent,
            raw: "\t[ -d|--debug ]\n".to_string(),
            root: node_with_flags("prog", vec![long_flag("debug", &[])]),
        },
    ]
}
