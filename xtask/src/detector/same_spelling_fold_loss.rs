//! `same-spelling-fold-loss` (round 5): two root entities share one
//! identity key (`mandible_core::merge::entity_identity`'s) but disagree
//! about taking a value, or each carry a different description —
//! docs/shapes.md S-102, `corpus/vim.basic/audit-seed4`'s bare `+` vs
//! valued `+<lnum>`. `icupkg`'s `-tl`/`-tb`/`-te` is the same collision
//! one layer earlier; a prototyped extraction fold moved only that one
//! tool, below the five-tool bar, so it was not shipped and still
//! surfaces here.
//!
//! No seed-2/4/5/6 labelled tool carries this shape, so
//! [`Detector::family`] returns `None` — spec §13.1e rule 6.

use crate::detector::{Detector, Expect, Scope, SelfCheck, ToolEvidence};
use mandible_core::{CommandNode, Dashes, Entity, Provenance, Source, Text, ValueKind};

/// The bucket key two entities must share to be the same item, mirroring
/// `mandible_core::merge::entity_identity` (private to that crate) closely
/// enough for detection purposes: long name (with dash count), else short
/// letter, else the bare name a dashless kind carries. `None` for an
/// entity with no spelling at all — nothing to collide on.
fn identity_key(e: &Entity) -> Option<String> {
    if let Some(l) = e.long_spelling() {
        let dashes = if matches!(l.dashes, Dashes::Double) {
            "2"
        } else {
            "1"
        };
        return Some(format!("L:{dashes}:{}", l.name));
    }
    if let Some(s) = e.short() {
        return Some(format!("S:{s}"));
    }
    if !e.spellings.is_empty() {
        return Some(format!("N:{}", e.primary_name()));
    }
    None
}

/// True when `a` and `b` are the shape this detector claims: same identity
/// key (checked by the caller), and either a difference in whether they
/// take a value at all, or two different documented descriptions.
fn disagrees(a: &Entity, b: &Entity) -> bool {
    let value_disagrees = (a.value_kind == ValueKind::None) != (b.value_kind == ValueKind::None);
    let desc_disagrees = matches!((&a.description, &b.description), (Some(x), Some(y)) if x != y);
    value_disagrees || desc_disagrees
}

pub struct SameSpellingFoldLoss;

impl Detector for SameSpellingFoldLoss {
    fn name(&self) -> &'static str {
        "same-spelling-fold-loss"
    }

    fn family(&self) -> Option<&'static str> {
        None
    }

    fn describes(&self) -> &'static str {
        "two root entities share one identity key (same long/short spelling) but disagree \
         about taking a value or about their own description — a later spelling-keyed fold \
         keeps only one row's information"
    }

    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        let mut by_key: std::collections::BTreeMap<String, Vec<&Entity>> =
            std::collections::BTreeMap::new();
        for e in &evidence.root.entities {
            if let Some(key) = identity_key(e) {
                by_key.entry(key).or_default().push(e);
            }
        }
        let mut findings = Vec::new();
        for (key, group) in by_key {
            if group.len() < 2 {
                continue;
            }
            for i in 0..group.len() {
                for j in (i + 1)..group.len() {
                    if disagrees(group[i], group[j]) {
                        findings.push(format!(
                            "{key}: {:?} vs {:?}",
                            group[i]
                                .description
                                .as_ref()
                                .map(Text::as_str)
                                .unwrap_or(""),
                            group[j]
                                .description
                                .as_ref()
                                .map(Text::as_str)
                                .unwrap_or(""),
                        ));
                    }
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
            let mut root = CommandNode::new("t", Provenance::single(Source::HelpText));
            root.entities = flags;
            root
        }
        fn bare_plus() -> Entity {
            let mut e = Entity::new(
                mandible_core::EntityKind::Flag,
                Provenance::single(Source::HelpText),
            );
            e.spellings = vec![mandible_core::Spelling::bare("+")];
            e.description = Some(Text::sanitize("Start at end of file"));
            e
        }
        fn valued_plus() -> Entity {
            let mut e = Entity::new(
                mandible_core::EntityKind::Flag,
                Provenance::single(Source::HelpText),
            );
            e.spellings = vec![mandible_core::Spelling::bare("+")];
            e.value_name = Some("lnum".to_string());
            e.value_kind = ValueKind::Required;
            e.description = Some(Text::sanitize("Start at line <lnum>"));
            e
        }
        vec![
            SelfCheck {
                name: "vim.basic's own bytes, bare `+` vs valued `+<lnum>`",
                why: "the defect itself: two entities share the bare `+` spelling and disagree \
                      about taking a value",
                expect: Expect::Fires(1),
                raw: String::new(),
                root: node_with(vec![bare_plus(), valued_plus()]),
            },
            SelfCheck {
                name: "icupkg's own bytes, already folded into one `-t`/`--type` entity",
                why: "once `fold_glued_choice_rows` has merged the three rows into one entity's \
                      `choices`, there is only one entity to collide with itself, so this must \
                      stay silent",
                expect: Expect::Silent,
                raw: String::new(),
                root: node_with(vec![{
                    let mut e = Entity::flag_spelled(
                        None,
                        Some("type".to_string()),
                        false,
                        false,
                        Provenance::single(Source::HelpText),
                    );
                    e.spellings.insert(0, mandible_core::Spelling::short('t'));
                    e.value_kind = ValueKind::Required;
                    e.choices = vec![
                        mandible_core::Choice::described(
                            "l",
                            Text::sanitize("output for little-endian/ASCII charset family"),
                        ),
                        mandible_core::Choice::described(
                            "b",
                            Text::sanitize("output for big-endian/ASCII charset family"),
                        ),
                    ];
                    e
                }]),
            },
            SelfCheck {
                name: "two rows sharing a spelling that agree on shape and description",
                why: "an ordinary cross-source duplicate (the same row re-parsed twice) must \
                      never be claimed",
                expect: Expect::Silent,
                raw: String::new(),
                root: node_with(vec![bare_plus(), bare_plus()]),
            },
        ]
    }
}
