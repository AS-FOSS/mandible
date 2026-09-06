//! `value-name-duplicates-choices` (atlas S-130): a docopt bracket row's
//! own trailing `|`-separated choice list (`flag_rows::trailing_choice_list`,
//! S-120) attaches as `choices`, and when no bracketed placeholder
//! introduced the list, the same text is *also* what the grammar read as
//! `value_name`, so the rendered screen prints the list twice
//! (`pvdisplay`'s `--configreport`, `--driverloaded`). `--units
//! [Number]`'s own `value_name` is a real, distinct placeholder and never
//! matches. Fixed in `mandible-extract/src/help_text/sections/emit.rs`'s
//! `value_name_duplicates_its_own_choices`, mirrored here so the fleet
//! count is measurable independently of the fix itself.
//!
//! No seed-2/4/5/6 labelled tool carries this shape, so
//! [`Detector::family`] returns `None`.

use crate::detector::{Detector, Expect, SelfCheck, ToolEvidence};
use mandible_core::{Choice, CommandNode, Entity, Provenance, Source};

/// True when `value_name` is nothing but `choices`' own names rejoined
/// with `|`, in the same order — the mirror of `mandible_extract`'s own
/// check, restated here rather than imported since `xtask` and
/// `mandible-extract` do not share code (`corpus/README.md`'s own rule
/// for oracles).
fn value_name_duplicates_choices(value_name: &str, choices: &[Choice]) -> bool {
    if choices.is_empty() {
        return false;
    }
    let rebuilt: Vec<&str> = value_name.split('|').collect();
    rebuilt.len() == choices.len() && rebuilt.iter().zip(choices).all(|(v, c)| *v == c.name)
}

pub struct ValueNameDuplicatesChoices;

impl Detector for ValueNameDuplicatesChoices {
    fn name(&self) -> &'static str {
        "value-name-duplicates-choices"
    }

    fn family(&self) -> Option<&'static str> {
        None
    }

    fn describes(&self) -> &'static str {
        "a flag's own value_name is nothing but its choices rejoined with `|`, so the rendered \
         screen prints the same enumerated list twice"
    }

    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        evidence
            .root
            .flags()
            .filter_map(|e| {
                let value_name = e.value_name.as_deref()?;
                value_name_duplicates_choices(value_name, &e.choices).then(|| {
                    format!(
                        "{:?} value_name {:?} duplicates its own choices",
                        e.spellings
                            .iter()
                            .map(|s| s.name.clone())
                            .collect::<Vec<_>>(),
                        value_name
                    )
                })
            })
            .collect()
    }

    fn self_checks(&self) -> Vec<SelfCheck> {
        fn node_with(flag: Entity) -> CommandNode {
            let mut root = CommandNode::new("pvdisplay", Provenance::single(Source::HelpText));
            root.entities = vec![flag];
            root
        }
        fn flag_with(value_name: &str, choice_names: &[&str]) -> Entity {
            let mut e = Entity::flag_long("configreport", Provenance::single(Source::HelpText));
            e.value_name = Some(value_name.to_string());
            e.choices = choice_names.iter().map(|n| Choice::bare(*n)).collect();
            e
        }
        vec![
            SelfCheck {
                name: "pvdisplay's own bytes, pre-fix shape (`--configreport`)",
                why: "the defect itself: value_name repeats the choices list verbatim",
                expect: Expect::Fires(1),
                raw: String::new(),
                root: node_with(flag_with(
                    "log|vg|lv|pv|pvseg|seg",
                    &["log", "vg", "lv", "pv", "pvseg", "seg"],
                )),
            },
            SelfCheck {
                name: "pvdisplay's own bytes, post-fix shape (value_name dropped)",
                why: "once the duplicate placeholder is dropped, choices alone carry the \
                      enumeration and there is nothing here to find",
                expect: Expect::Silent,
                raw: String::new(),
                root: node_with({
                    let mut e = flag_with(
                        "log|vg|lv|pv|pvseg|seg",
                        &["log", "vg", "lv", "pv", "pvseg", "seg"],
                    );
                    e.value_name = None;
                    e
                }),
            },
            SelfCheck {
                name: "pvdisplay's own `--units`, a real distinct placeholder",
                why: "a bracketed placeholder that names none of its own choices must never be \
                      claimed — `--units [Number]` keeps `Number`, not the letter list",
                expect: Expect::Silent,
                raw: String::new(),
                root: node_with(flag_with("Number", &["r", "R", "h"])),
            },
            SelfCheck {
                name: "a flag with no choices at all",
                why: "an ordinary value_name with nothing to compare against must never be \
                      claimed",
                expect: Expect::Silent,
                raw: String::new(),
                root: node_with({
                    let mut e = Entity::flag_long("format", Provenance::single(Source::HelpText));
                    e.value_name = Some("FORMAT".to_string());
                    e
                }),
            },
        ]
    }
}
