//! `or-joined-alias-single-space-gap` (atlas S-134): `icupkg`'s own `-c or
//! --copyright include the ICU copyright notice` row joins two
//! value-free spellings with the word `or`, same as `crate::or_joined_alias`
//! (S-099) already reads, but its description starts only one space after
//! `--copyright`, not the two-space or tab gap `or_alias_ends_the_spec`
//! requires before it will treat the row as fully joined. The short
//! spelling keeps the literal word `or` as a fabricated value name and
//! `--copyright` reaches nothing. Distinct from `or-joined-alias-with-values`
//! (S-110), which covers a value on at least one spelling; here neither
//! side carries one.
//!
//! No seed-2/4/5/6 labelled tool carries this shape, so
//! [`Detector::family`] returns `None`.

use crate::detector::{Detector, Expect, SelfCheck, ToolEvidence};
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

struct Row {
    short: String,
    long: String,
}

/// `<short> or <long>` (no value on either side), an indented row, with
/// the description starting exactly one space after `<long>` — not the
/// two-space (or tab) column gap that would already let
/// `or_alias_ends_the_spec` treat the row as fully joined.
///
/// Two guards keep this from matching shapes that only superficially
/// resemble it, found by running an early, looser version fleet-wide:
/// `tokens[2]` must be a genuine double-dash long spelling, never a
/// second short flag in a three-way `or` chain (`-h or -? or --help`,
/// already S-099's own chained shape, wrongly read `-?` as "the long
/// spelling" and reported a false break every time); and the first word
/// of the description must be a bare lowercase word, never a value token
/// (`-m or --match-arch file.o  match the architecture...` carries a
/// value on the long spelling, `crate::or_joined_alias_with_values`'s own
/// territory, not this value-free shape).
fn parse_row(line: &str) -> Option<Row> {
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed == line {
        return None;
    }
    let tokens: Vec<&str> = trimmed.split_whitespace().collect();
    if tokens.len() < 4 || !tokens[0].starts_with('-') {
        return None;
    }
    if tokens[1] != "or" || !tokens[2].starts_with("--") {
        return None;
    }
    let long_tok = tokens[2];
    let idx = trimmed.find(long_tok)?;
    let after = trimmed.get(idx + long_tok.len()..)?;
    let gap_len = after.len() - after.trim_start_matches(' ').len();
    let desc = after.trim_start_matches(' ');
    if gap_len != 1 || desc.is_empty() {
        return None;
    }
    let first_word = desc.split_whitespace().next()?;
    if !first_word.chars().all(|c| c.is_ascii_lowercase()) {
        return None;
    }
    Some(Row {
        short: tokens[0].to_string(),
        long: long_tok.to_string(),
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
        let broken = !tree_has_long(root, long_name)
            || short_char.is_some_and(|c| short_value_is_literal_or(root, c));
        if broken {
            findings.push(Finding {
                short: row.short,
                long: row.long,
                line: line.to_string(),
            });
        }
    }
    Report { findings }
}

pub struct OrJoinedAliasSingleSpaceGap;

impl Detector for OrJoinedAliasSingleSpaceGap {
    fn name(&self) -> &'static str {
        "or-joined-alias-single-space-gap"
    }

    fn family(&self) -> Option<&'static str> {
        None
    }

    fn describes(&self) -> &'static str {
        "a value-free `or`-joined alias row whose description starts only one space after the \
         second spelling, so the short spelling keeps `or` itself as a fabricated value and the \
         long spelling reaches nothing"
    }

    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        detect(evidence.raw, evidence.root)
            .findings
            .into_iter()
            .map(|f| format!("{:?}/{:?} never joined, from {:?}", f.short, f.long, f.line))
            .collect()
    }

    fn self_checks(&self) -> Vec<SelfCheck> {
        use mandible_core::{Entity, Provenance, Source};

        fn node_with_flags(flags: Vec<Entity>) -> CommandNode {
            let mut root = CommandNode::new("icupkg", Provenance::single(Source::HelpText));
            root.set_entities_of(mandible_core::EntityKind::Flag, flags);
            root
        }
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
        fn short_long_flag(short: char, long: &str) -> Entity {
            Entity::flag_spelled(
                Some(short),
                Some(long.to_string()),
                false,
                false,
                Provenance::single(Source::HelpText),
            )
        }

        /// icupkg's real row, byte-exact (`icupkg --help`).
        const COPYRIGHT_ROW: &str = "\t-c or --copyright include the ICU copyright notice\n";
        /// A two-space column gap, the shape `or_alias_ends_the_spec`
        /// already handles — must never be claimed by this detector too.
        const TWO_SPACE_ROW: &str = "\t-c or --copyright  include the ICU copyright notice\n";

        vec![
            SelfCheck {
                name: "icupkg's own bytes, pre-fix shape (`-c`'s fabricated `or` value)",
                why: "the defect itself: a one-space description gap leaves `-c` with the \
                      literal value `or` and drops `--copyright` entirely",
                expect: Expect::Fires(1),
                raw: COPYRIGHT_ROW.to_string(),
                root: node_with_flags(vec![short_flag_with_value('c', "or")]),
            },
            SelfCheck {
                name: "the same row, correctly recovered",
                why: "once both spellings are one entity with no value and a real description, \
                      the same row must go silent",
                expect: Expect::Silent,
                raw: COPYRIGHT_ROW.to_string(),
                root: node_with_flags(vec![short_long_flag('c', "copyright")]),
            },
            SelfCheck {
                name: "a two-space column gap, already the working shape",
                why: "`or_alias_ends_the_spec` already treats a two-space gap as ending the \
                      spec, so this row is not this detector's own shape and must stay silent \
                      even in its broken form",
                expect: Expect::Silent,
                raw: TWO_SPACE_ROW.to_string(),
                root: node_with_flags(vec![short_flag_with_value('c', "or")]),
            },
        ]
    }
}
