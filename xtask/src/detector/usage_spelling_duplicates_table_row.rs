//! `usage-spelling-duplicates-table-row` (round 5): the usage-only-
//! spelling recovery pass (`mandible-extract/src/help_text/sections/
//! mod.rs`, the `extract_usage_flags` loop feeding
//! `flag_spelling_already_present`) adds a bare, undescribed entity for a
//! spelling an existing table-row entity already carries among its
//! *other* spellings.
//!
//! `icupkg` is the specimen: the table row `-h or -? or --help` parses
//! correctly into one entity carrying all three spellings, but the usage
//! line `[-h|-?|--help ]` still adds a second, bare `-?` entity with no
//! description — `flag_spelling_already_present` used to check only the
//! existing entity's *primary* `short()`/`long()` pick (the first
//! matching spelling), never every spelling it carries, so `-?` (never
//! the first short spelling in `-h, -?, --help`) read as absent. Fixed in
//! `flag_spelling_already_present` itself; this detector generalizes the
//! symptom fleet-wide.
//!
//! No seed-2/4/5/6 labelled tool carries this shape under an existing
//! `mandible_core::audit::DEFECT_FAMILIES` entry, so [`Detector::family`]
//! returns `None` — spec §13.1e rule 6.

use crate::detector::{Detector, Expect, Scope, SelfCheck, ToolEvidence};
use mandible_core::{CommandNode, Dashes, Entity, Provenance, Source, Spelling, Text};

/// True for a usage-derived duplicate: one spelling, no description, and
/// `Source::HelpTextSynopsis` is the only contributing source — exactly
/// the shape `extract_usage_flags` emits before
/// `flag_spelling_already_present` gets a chance to drop it.
fn is_usage_only_bare_spelling(e: &Entity) -> bool {
    e.spellings.len() == 1
        && e.description.is_none()
        && !e.provenance.sources.is_empty()
        && e.provenance
            .sources
            .iter()
            .all(|s| matches!(s, Source::HelpTextSynopsis))
}

fn carries_spelling(e: &Entity, dashes: Dashes, name: &str) -> bool {
    e.spellings
        .iter()
        .any(|s| s.dashes == dashes && s.name == name)
}

pub struct UsageSpellingDuplicatesTableRow;

impl Detector for UsageSpellingDuplicatesTableRow {
    fn name(&self) -> &'static str {
        "usage-spelling-duplicates-table-row"
    }

    fn family(&self) -> Option<&'static str> {
        None
    }

    fn describes(&self) -> &'static str {
        "a bare, undescribed entity sourced only from the usage synopsis carries a spelling an \
         existing table-row entity already carries among its other spellings"
    }

    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        let mut findings = Vec::new();
        for candidate in &evidence.root.entities {
            if !is_usage_only_bare_spelling(candidate) {
                continue;
            }
            let spelling = &candidate.spellings[0];
            for other in &evidence.root.entities {
                if std::ptr::eq(other, candidate) {
                    continue;
                }
                if other.spellings.len() > 1
                    && carries_spelling(other, spelling.dashes, &spelling.name)
                {
                    findings.push(format!(
                        "{:?} already carried by the row spelled {:?}",
                        spelling.name,
                        other
                            .spellings
                            .iter()
                            .map(|s| s.name.clone())
                            .collect::<Vec<_>>()
                    ));
                }
            }
        }
        findings
    }

    fn scope(&self) -> Scope {
        Scope::full()
    }

    fn self_checks(&self) -> Vec<SelfCheck> {
        fn node_with(flags: Vec<Entity>) -> CommandNode {
            let mut root = CommandNode::new("icupkg", Provenance::single(Source::HelpText));
            root.entities = flags;
            root
        }
        fn table_row_help() -> Entity {
            let mut e = Entity::new(
                mandible_core::EntityKind::Flag,
                Provenance::single(Source::HelpText),
            );
            e.spellings = vec![
                Spelling::short('h'),
                Spelling::short('?'),
                Spelling::long("help"),
            ];
            e.description = Some(Text::sanitize("print this message and exit"));
            e
        }
        fn bare_question_mark(source: Source) -> Entity {
            let mut e = Entity::new(mandible_core::EntityKind::Flag, Provenance::single(source));
            e.spellings = vec![Spelling::short('?')];
            e
        }
        vec![
            SelfCheck {
                name: "icupkg's own bytes, pre-fix shape (duplicate bare `-?`)",
                why: "the defect itself: a bare, undescribed `-?` entity duplicates a spelling \
                      the `-h, -?, --help` row already carries",
                expect: Expect::Fires(1),
                raw: String::new(),
                root: node_with(vec![
                    table_row_help(),
                    bare_question_mark(Source::HelpTextSynopsis),
                ]),
            },
            SelfCheck {
                name: "icupkg's own bytes, post-fix shape (duplicate dropped)",
                why: "once `flag_spelling_already_present` checks every spelling the table row \
                      carries, the usage-derived duplicate is never added, so there is nothing \
                      here to find",
                expect: Expect::Silent,
                raw: String::new(),
                root: node_with(vec![table_row_help()]),
            },
            SelfCheck {
                name: "a genuinely undocumented usage-only flag with no table row",
                why: "a usage-derived flag with no table-row counterpart is a real recovery, \
                      not a duplicate, and must never be claimed",
                expect: Expect::Silent,
                raw: String::new(),
                root: node_with(vec![bare_question_mark(Source::HelpTextSynopsis)]),
            },
            SelfCheck {
                name: "a table-row entity that merely repeats the usage-only entity's source",
                why: "a described entity is never itself claimed as the duplicate, however many \
                      sources contributed to it",
                expect: Expect::Silent,
                raw: String::new(),
                root: node_with(vec![table_row_help(), table_row_help()]),
            },
        ]
    }
}
