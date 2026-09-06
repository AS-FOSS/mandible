//! `choice-value-rows-unfolded` (atlas S-131): `icupkg`'s own `-tl or
//! --type l`, `-tb or --type b`, `-te or --type e` rows are one option
//! with three literal choice values, each carrying its own description —
//! but the tree carries them as three separate entities, all spelled
//! `-t`/`--type`, differing only in `value_name` and description, never
//! folded into one flag's `choices`. `docs/shapes.md` S-102 already
//! records a prototype fold that moved only `icupkg` on a full-`PATH`
//! sweep, below the five-tool bar; this detector re-measures the same
//! shape independently of that prototype.
//!
//! No seed-2/4/5/6 labelled tool carries this shape, so
//! [`Detector::family`] returns `None`.

use crate::detector::{Detector, Expect, SelfCheck, ToolEvidence};
use mandible_core::{CommandNode, Entity, Provenance, Source, Spelling};

/// A bare identifier: letters/digits, `_`/`-`, first character
/// alphanumeric — the same choice-token shape
/// `flag_rows::is_choice_token` reads (docs/shapes.md S-120), restated
/// here since `xtask` and `mandible-extract` do not share code.
fn is_choice_token(token: &str) -> bool {
    let mut chars = token.chars();
    chars.next().is_some_and(|c| {
        c.is_ascii_alphanumeric()
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    })
}

/// True when `e` is one row of the unfolded shape: a real spelling, a
/// bare choice-shaped `value_name`, its own description, and no `choices`
/// of its own yet (the fold this detector measures has not happened).
///
/// Requires *both* a short and a long spelling, found by running an
/// early, looser version fleet-wide: dropping this requirement also
/// caught GCC's `-Xassembler`/`-Xpreprocessor`/`-Xlinker` (S-117, three
/// distinct flags collapsed to the single short spelling `-X` by an
/// unrelated, already-tracked defect) and similar single-spelling
/// collisions, none of which is icupkg's shape — a real option
/// documented once per literal value, both spellings intact on every
/// row.
fn is_unfolded_choice_row(e: &Entity) -> bool {
    e.short().is_some()
        && e.long().is_some()
        && e.choices.is_empty()
        && e.description.is_some()
        && e.value_name.as_deref().is_some_and(is_choice_token)
}

pub struct ChoiceValueRowsUnfolded;

impl Detector for ChoiceValueRowsUnfolded {
    fn name(&self) -> &'static str {
        "choice-value-rows-unfolded"
    }

    fn family(&self) -> Option<&'static str> {
        None
    }

    fn describes(&self) -> &'static str {
        "two or more rows share the exact same spelling and each carry one bare choice-shaped \
         value with its own description, never folded into one flag's `choices`"
    }

    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        let entities: Vec<&Entity> = evidence.root.flags().collect();
        let mut findings = Vec::new();
        let mut i = 0;
        while i < entities.len() {
            if !is_unfolded_choice_row(entities[i]) {
                i += 1;
                continue;
            }
            let mut j = i + 1;
            let mut values = vec![entities[i].value_name.clone().unwrap()];
            while j < entities.len()
                && is_unfolded_choice_row(entities[j])
                && entities[j].spellings == entities[i].spellings
            {
                values.push(entities[j].value_name.clone().unwrap());
                j += 1;
            }
            if values.len() >= 2 {
                findings.push(format!(
                    "{:?} rows for {:?} never folded into one flag's choices",
                    values,
                    entities[i]
                        .spellings
                        .iter()
                        .map(|s| s.name.clone())
                        .collect::<Vec<_>>()
                ));
            }
            i = j.max(i + 1);
        }
        findings
    }

    fn self_checks(&self) -> Vec<SelfCheck> {
        fn node_with(flags: Vec<Entity>) -> CommandNode {
            let mut root = CommandNode::new("icupkg", Provenance::single(Source::HelpText));
            root.entities = flags;
            root
        }
        fn type_row(value: &str, desc: &str) -> Entity {
            let mut e = Entity::new(
                mandible_core::EntityKind::Flag,
                Provenance::single(Source::HelpText),
            );
            e.spellings = vec![Spelling::short('t'), Spelling::long("type")];
            e.value_name = Some(value.to_string());
            e.description = Some(mandible_core::Text::sanitize(desc));
            e
        }
        vec![
            SelfCheck {
                name: "icupkg's own bytes, pre-fix shape (three unfolded `-t`/`--type` rows)",
                why: "the defect itself: three rows share one spelling, each with its own \
                      literal value and description, none folded into `choices`",
                expect: Expect::Fires(1),
                raw: String::new(),
                root: node_with(vec![
                    type_row("l", "output for little-endian/ASCII charset family"),
                    type_row("b", "output for big-endian/ASCII charset family"),
                    type_row("e", "output for big-endian/EBCDIC charset family"),
                ]),
            },
            SelfCheck {
                name: "icupkg's own bytes, folded (one flag, three choices)",
                why: "once folded into one flag's `choices`, there is only one entity left and \
                      nothing here to find",
                expect: Expect::Silent,
                raw: String::new(),
                root: node_with(vec![{
                    let mut e = type_row("l", "");
                    e.value_name = None;
                    e.description = None;
                    e.choices = vec![
                        mandible_core::Choice::bare("l"),
                        mandible_core::Choice::bare("b"),
                        mandible_core::Choice::bare("e"),
                    ];
                    e
                }]),
            },
            SelfCheck {
                name: "a single row alone, nothing to fold",
                why: "one row can never be a run of two or more, so it must never be claimed",
                expect: Expect::Silent,
                raw: String::new(),
                root: node_with(vec![type_row("l", "output for little-endian")]),
            },
            SelfCheck {
                name: "two rows sharing a spelling but a real bracketed placeholder, not a choice",
                why: "`vim.basic`'s own `-V[N][fname]` shape repeats a spelling with a real, \
                      non-choice-shaped value on each row and must never be claimed as this \
                      family",
                expect: Expect::Silent,
                raw: String::new(),
                root: node_with(vec![
                    type_row("[N][fname]", "Be verbose [level N]"),
                    type_row("[N][file]", "Verbose [level][file]"),
                ]),
            },
            SelfCheck {
                name: "gcc's own `-Xassembler`/`-Xpreprocessor`/`-Xlinker`, a single-spelling collision",
                why: "S-117's own defect collapses three distinct flags onto the single short \
                      spelling `-X` with no long form; this is that already-tracked family, not \
                      an icupkg-shaped choice row, and must never be claimed here",
                expect: Expect::Silent,
                raw: String::new(),
                root: node_with(vec![
                    {
                        let mut e = Entity::new(
                            mandible_core::EntityKind::Flag,
                            Provenance::single(Source::HelpText),
                        );
                        e.spellings = vec![Spelling::short('X')];
                        e.value_name = Some("assembler".to_string());
                        e.description = Some(mandible_core::Text::sanitize("Pass <arg> on to the assembler."));
                        e
                    },
                    {
                        let mut e = Entity::new(
                            mandible_core::EntityKind::Flag,
                            Provenance::single(Source::HelpText),
                        );
                        e.spellings = vec![Spelling::short('X')];
                        e.value_name = Some("preprocessor".to_string());
                        e.description = Some(mandible_core::Text::sanitize("Pass <arg> on to the preprocessor."));
                        e
                    },
                ]),
            },
        ]
    }
}
