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

// Round 9 widens the question to a value two same-identity entities both
// genuinely take, but name differently: `lvcreate`'s `--type` reaches the
// tree once per invocation form (docs/shapes.md S-137), each form naming a
// different literal value, and a fold that picks one form's name over
// another's drops every name but one — docs/shapes.md S-147.
// `mandible_core::merge::merge_entity_bucket` now unions every name
// instead; the widened `value_name_disagrees` below is what lets the
// fleet count measure that.

use crate::detector::{Detector, Expect, Scope, SelfCheck, ToolEvidence};
use mandible_core::{CommandNode, Dashes, Entity, EntityKind, Provenance, Source, Text, ValueKind};

/// The bucket key two entities must share to be the same item, mirroring
/// `mandible_core::merge::entity_identity` (private to that crate): the
/// kind leads — a flag and a positional spelled alike are unrelated items,
/// never one bucket — then long name (with dash count), else short
/// letter, else the bare name a dashless kind carries. `None` for an
/// entity with no spelling at all — nothing to collide on.
fn identity_key(e: &Entity) -> Option<(EntityKind, String)> {
    let key = if let Some(l) = e.long_spelling() {
        let dashes = if matches!(l.dashes, Dashes::Double) {
            "2"
        } else {
            "1"
        };
        format!("L:{dashes}:{}", l.name)
    } else if let Some(s) = e.short() {
        format!("S:{s}")
    } else if !e.spellings.is_empty() {
        format!("N:{}", e.primary_name())
    } else {
        return None;
    };
    Some((e.kind, key))
}

/// True when `a` and `b` are the shape this detector claims: same identity
/// key (checked by the caller), and any of — a difference in whether they
/// take a value at all, two different documented descriptions, or (both
/// genuinely taking a value) two different literal value names.
fn disagrees(a: &Entity, b: &Entity) -> bool {
    let value_disagrees = (a.value_kind == ValueKind::None) != (b.value_kind == ValueKind::None);
    let desc_disagrees = matches!((&a.description, &b.description), (Some(x), Some(y)) if x != y);
    value_disagrees || desc_disagrees || value_name_disagrees(a, b)
}

/// The `lvcreate` half of the shape (docs/shapes.md S-147): both entities
/// really do take a value — neither is `ValueKind::None` — so
/// [`disagrees`]'s own `value_disagrees` stays silent, yet the literal
/// name each names differs. Covers the asymmetric case too, where one
/// form's name was folded away entirely because it duplicated that form's
/// own `choices` (S-130's `value_name_duplicates_its_own_choices`, the
/// shape of `--type raid1|mirror`): `None` there still names no value, so
/// a single-winner pick would drop it exactly as it drops a plain
/// mismatch.
fn value_name_disagrees(a: &Entity, b: &Entity) -> bool {
    if a.value_kind == ValueKind::None || b.value_kind == ValueKind::None {
        return false;
    }
    a.value_name.as_deref() != b.value_name.as_deref()
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
        let mut by_key: std::collections::HashMap<(EntityKind, String), Vec<&Entity>> =
            std::collections::HashMap::new();
        for e in &evidence.root.entities {
            if let Some(key) = identity_key(e) {
                by_key.entry(key).or_default().push(e);
            }
        }
        let mut findings = Vec::new();
        for ((kind, key), group) in by_key {
            // A group above this size is not a real same-spelling
            // collision — it is some other shape entirely (a degenerate
            // tree with many entities sharing an empty bare name, say),
            // and no tool this detector was built for behind it looks
            // like this. The real specimens (icupkg, vim.basic) top out
            // at 3. One finding per *bucket* below, not per pair — a
            // bucket of size `n` is one loss, not `n choose 2` of them.
            const MAX_GROUP: usize = 16;
            if group.len() < 2 || group.len() > MAX_GROUP {
                continue;
            }
            let disagreeing_pair = (0..group.len())
                .flat_map(|i| ((i + 1)..group.len()).map(move |j| (i, j)))
                .find(|&(i, j)| disagrees(group[i], group[j]));
            if let Some((i, j)) = disagreeing_pair {
                findings.push(format!(
                    "{kind:?} {key} ({} rows): {:?} vs {:?}",
                    group.len(),
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
            SelfCheck {
                name: "lvcreate's own bytes: --type linear, --type striped, \
                       --type raid1|mirror (docs/shapes.md S-147)",
                why: "the new defect: three entities share the `--type` spelling, all three \
                      genuinely take a value, so the old value-kind check stays silent, but each \
                      names (or, for the choices form, fails to name) a different literal value",
                expect: Expect::Fires(1),
                raw: String::new(),
                root: node_with(vec![
                    {
                        let mut e = Entity::flag_long("type", Provenance::single(Source::HelpText));
                        e.value_kind = ValueKind::Required;
                        e.value_name = Some("linear".to_string());
                        e.group = Some("Create a linear LV.".to_string());
                        e
                    },
                    {
                        let mut e = Entity::flag_long("type", Provenance::single(Source::HelpText));
                        e.value_kind = ValueKind::Required;
                        e.value_name = Some("striped".to_string());
                        e.group = Some("Create a striped LV.".to_string());
                        e
                    },
                    {
                        let mut e = Entity::flag_long("type", Provenance::single(Source::HelpText));
                        e.value_kind = ValueKind::Required;
                        // S-130's own fix drops `value_name` here, since it is
                        // nothing but `choices` rejoined with `|`.
                        e.choices = vec![
                            mandible_core::Choice::bare("raid1"),
                            mandible_core::Choice::bare("mirror"),
                        ];
                        e.group = Some("Create a raid1 or mirror LV.".to_string());
                        e
                    },
                ]),
            },
            SelfCheck {
                name: "two required entities sharing a spelling that name the same value",
                why: "every ordinary tool re-probed twice (docs/shapes.md's own root-refill \
                      shape) reaches this bucket with duplicate, agreeing rows — the value-name \
                      question must stay silent when the name genuinely agrees",
                expect: Expect::Silent,
                raw: String::new(),
                root: node_with(vec![
                    {
                        let mut e =
                            Entity::flag_long("output", Provenance::single(Source::HelpText));
                        e.value_kind = ValueKind::Required;
                        e.value_name = Some("FILE".to_string());
                        e
                    },
                    {
                        let mut e =
                            Entity::flag_long("output", Provenance::single(Source::HelpText));
                        e.value_kind = ValueKind::Required;
                        e.value_name = Some("FILE".to_string());
                        e
                    },
                ]),
            },
        ]
    }
}
