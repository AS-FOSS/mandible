//! `or-joined-alias-with-values` (atlas S-110): an alias pair joined by
//! the word `or` where **both** spellings carry a value (`-s path or
//! --sourcedir path`, `-C comment or --comment comment`). `crate::or_joined_alias`
//! (the round-3 fix) handles the value-free form only — its row parser
//! requires `or` at the second token, which a value word between the
//! short flag and `or` shifts past. `icupkg` shows two failure signatures:
//! `-s` keeps its own value `path` while `--sourcedir` is dropped
//! entirely, and `-c` (bare, no value either side) keeps the literal word
//! `or` as a fabricated value name. `icupkg` was never drawn by the
//! frozen queue and has no fixture.

use mandible_core::CommandNode;

pub struct Finding {
    pub short: String,
    pub long: String,
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

fn is_value_like(word: &str) -> bool {
    let mut chars = word.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {
            chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        }
        _ => false,
    }
}

struct Row {
    short: String,
    val1: Option<String>,
    long: String,
    val2: Option<String>,
}

/// `<short>[ <value>] or <long>[ <value>]`, an indented row.
fn parse_row(line: &str) -> Option<Row> {
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed == line {
        return None;
    }
    let tokens: Vec<&str> = trimmed.split_whitespace().collect();
    if tokens.len() < 3 || !tokens[0].starts_with('-') {
        return None;
    }
    let (or_idx, val1) = if tokens[1] == "or" {
        (1, None)
    } else if tokens.len() > 2 && tokens[2] == "or" && is_value_like(tokens[1]) {
        (2, Some(tokens[1].to_string()))
    } else {
        return None;
    };
    let long_tok = tokens.get(or_idx + 1)?;
    if !long_tok.starts_with('-') {
        return None;
    }
    let val2 = tokens
        .get(or_idx + 2)
        .filter(|w| is_value_like(w))
        .map(|w| w.to_string());
    Some(Row {
        short: tokens[0].to_string(),
        val1,
        long: (*long_tok).to_string(),
        val2,
    })
}

fn tree_has_long(root: &CommandNode, name: &str) -> bool {
    root.flags().any(|e| e.long() == Some(name))
}

fn short_value_is_literal_or(root: &CommandNode, short: char) -> bool {
    root.flags()
        .any(|e| e.short() == Some(short) && e.value_name.as_deref() == Some("or"))
}

pub fn detect(raw: &str, root: &CommandNode) -> Report {
    let mut findings = Vec::new();
    for line in raw.lines() {
        let Some(row) = parse_row(line) else {
            continue;
        };
        let long_name = row.long.trim_start_matches('-');
        let short_char = row.short.trim_start_matches('-').chars().next();
        let long_missing =
            row.val1.is_some() && row.val2.is_some() && !tree_has_long(root, long_name);
        let short_bogus = short_char.is_some_and(|c| short_value_is_literal_or(root, c));
        if long_missing || short_bogus {
            findings.push(Finding {
                short: row.short,
                long: row.long,
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
use mandible_core::{Entity, Provenance, Source};

/// icupkg's real row, byte-exact (`icupkg --help`).
pub(crate) const ICUPKG_SOURCEDIR_ROW: &str =
    "\t-s path or --sourcedir path  directory for the --add items\n";

/// icupkg's real row, byte-exact (`icupkg --help`).
pub(crate) const ICUPKG_COPYRIGHT_ROW: &str =
    "\t-c or --copyright include the ICU copyright notice\n";

fn short_flag_with_value(short: char, value_name: &str) -> Entity {
    let mut e = Entity::flag_spelled(
        Some(short),
        None,
        false,
        false,
        Provenance::single(Source::HelpText),
    );
    e.value_name = Some(value_name.to_string());
    e
}

fn short_long_flag_with_value(short: char, long: &str, value_name: &str) -> Entity {
    let mut e = Entity::flag_spelled(
        Some(short),
        Some(long.to_string()),
        false,
        false,
        Provenance::single(Source::HelpText),
    );
    e.value_name = Some(value_name.to_string());
    e
}

fn flag(short: char, long: Option<&str>) -> Entity {
    Entity::flag_spelled(
        Some(short),
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

pub(crate) fn self_checks() -> Vec<SelfCheck> {
    vec![
        SelfCheck {
            name: "icupkg's own bytes, `--sourcedir` dropped",
            why: "the defect itself: both spellings document the value `path`, but the tree \
                  keeps only `-s` and drops `--sourcedir` entirely",
            expect: Expect::Fires(1),
            raw: ICUPKG_SOURCEDIR_ROW.to_string(),
            root: node_with_flags("icupkg", vec![short_flag_with_value('s', "path")]),
        },
        SelfCheck {
            name: "icupkg's own bytes, `-c`'s fabricated `or` value",
            why: "the second failure signature: a value-free `or`-joined row where the short \
                  spelling's value name becomes the literal word `or`",
            expect: Expect::Fires(1),
            raw: ICUPKG_COPYRIGHT_ROW.to_string(),
            root: node_with_flags("icupkg", vec![short_flag_with_value('c', "or")]),
        },
        SelfCheck {
            name: "`--sourcedir` recovered, `-s`'s value clean",
            why: "once both spellings are one entity with the real value, the same row must go \
                  silent",
            expect: Expect::Silent,
            raw: ICUPKG_SOURCEDIR_ROW.to_string(),
            root: node_with_flags(
                "icupkg",
                vec![short_long_flag_with_value('s', "sourcedir", "path")],
            ),
        },
        SelfCheck {
            name: "the round-3 value-free shape, correctly recovered",
            why: "`crate::or_joined_alias`'s own repaired shape: no value word on either side, \
                  and no bogus value on the short spelling, must never be claimed by this \
                  detector too",
            expect: Expect::Silent,
            raw: "-h or --help\tPrint Help\n".to_string(),
            root: node_with_flags("prog", vec![flag('h', Some("help"))]),
        },
        SelfCheck {
            name: "an ordinary comma-joined alias, not `or`-joined at all",
            why: "the row parser requires the literal word `or` at the expected token; a comma \
                  alias never reaches it",
            expect: Expect::Silent,
            raw: "-h, --help\tPrint Help\n".to_string(),
            root: node_with_flags("prog", vec![flag('h', Some("help"))]),
        },
    ]
}
