//! `lowdown-bullet-option-row` (atlas S-144): lowdown's man-page-like
//! `--help` rendering (nix/Lix, issue #138) writes every flag as a
//! `·`-led bullet row too, and two of its own shapes lose information
//! the generic flag grammar never reads: a `/`-joined second spelling
//! (`"--print-build-logs / -L Print..."`) and a run of lowercase bare
//! value names before the description's sentence start (`"--option name
//! value Set..."`). Independent re-implementation (no shared code with
//! `mandible_extract`). No seed-2/4/5/6/7 labelled tool carries this
//! shape, so [`Detector::family`] returns `None`.

use crate::detector::{Detector, Expect, SelfCheck, ToolEvidence};
use mandible_core::{CommandNode, Entity};

/// The primary spelling and the rest of a `·`-marked option bullet row's
/// own text, when it opens with a `-`-led token.
fn bullet_option_row(line: &str) -> Option<(&str, &str)> {
    let trimmed = line.trim_start();
    let text = trimmed.strip_prefix('\u{b7}')?.strip_prefix(' ')?;
    let spec_end = text.find(' ')?;
    let (spec, rest) = text.split_at(spec_end);
    spec.starts_with('-').then_some((spec, rest.trim_start()))
}

/// `rest` (everything on the row after the primary spelling) names a
/// `/`-joined second spelling that the row loses: exactly `/ <spelling>`
/// with one space on each side of the slash.
fn slash_alias(rest: &str) -> Option<&str> {
    let after = rest.strip_prefix("/ ")?;
    let alias = after.split_whitespace().next()?;
    alias.starts_with('-').then_some(alias)
}

/// `rest` (everything on the row after the primary spelling, and past any
/// `/`-joined alias) opens with a run of lowercase bare words before a
/// capitalized sentence word — S-144's value-name shape.
fn lowercase_value_name(rest: &str) -> Option<&str> {
    let first = rest.split_whitespace().next()?;
    let is_lowercase_word = |w: &str| !w.is_empty() && w.chars().all(|c| c.is_ascii_lowercase());
    is_lowercase_word(first).then_some(first)
}

fn find_flag<'a>(root: &'a CommandNode, spelling: &str) -> Option<&'a Entity> {
    let bare = spelling.trim_start_matches('-');
    root.flags()
        .find(|f| f.spellings.iter().any(|s| s.name == bare))
}

pub struct LowdownBulletOptionRow;

impl Detector for LowdownBulletOptionRow {
    fn name(&self) -> &'static str {
        "lowdown-bullet-option-row"
    }

    fn family(&self) -> Option<&'static str> {
        None
    }

    fn describes(&self) -> &'static str {
        "a `·`-marked option bullet row's own `/`-joined alias or lowercase value-name run never \
         reached the matching flag"
    }

    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        let mut findings = Vec::new();
        for line in evidence.raw.lines() {
            let Some((spec, rest)) = bullet_option_row(line) else {
                continue;
            };
            let Some(flag) = find_flag(evidence.root, spec) else {
                continue;
            };
            if let Some(alias) = slash_alias(rest) {
                let bare = alias.trim_start_matches('-');
                if !flag.spellings.iter().any(|s| s.name == bare) {
                    findings.push(format!("{alias:?} never became an alias of {spec:?}"));
                }
            } else if let Some(value_name) = lowercase_value_name(rest) {
                let carries = flag
                    .value_name
                    .as_deref()
                    .is_some_and(|v| v.contains(value_name));
                if !carries {
                    findings.push(format!("{spec:?} lost its own value name {value_name:?}"));
                }
            }
        }
        findings
    }

    fn self_checks(&self) -> Vec<SelfCheck> {
        self_checks()
    }
}

// ----------------------------------------------------------------------
// Self-checks
// ----------------------------------------------------------------------

use mandible_core::{Provenance, Source, Spelling};

fn flag_node(spellings: &[&str], value_name: Option<&str>) -> CommandNode {
    let mut root = CommandNode::new("nix", Provenance::single(Source::HelpText));
    let mut entity = Entity::flag_long(
        spellings[0].trim_start_matches('-'),
        Provenance::single(Source::HelpText),
    );
    entity.spellings = spellings
        .iter()
        .map(|s| {
            let bare = s.trim_start_matches('-');
            if s.starts_with("--") {
                Spelling::long(bare)
            } else {
                Spelling::single_dash(bare)
            }
        })
        .collect();
    entity.value_name = value_name.map(str::to_string);
    root.entities = vec![entity];
    root
}

fn self_checks() -> Vec<SelfCheck> {
    vec![
        SelfCheck {
            name: "nix's own slash-aliased row, the alias never recovered",
            why: "the defect itself: `-L` never became an alias of `--print-build-logs`",
            expect: Expect::Fires(1),
            raw: "  · --print-build-logs / -L Print full build logs\n".to_string(),
            root: flag_node(&["--print-build-logs"], None),
        },
        SelfCheck {
            name: "nix's own slash-aliased row, the alias already recovered",
            why: "once `-L` is a spelling of the same flag, the same row must go silent",
            expect: Expect::Silent,
            raw: "  · --print-build-logs / -L Print full build logs\n".to_string(),
            root: flag_node(&["--print-build-logs", "-L"], None),
        },
        SelfCheck {
            name: "nix's own multi-value-name row, the value name lost",
            why: "the defect itself: `--option`'s own `name value` run never reached \
                  `value_name`",
            expect: Expect::Fires(1),
            raw: "  · --option name value Set the Lix configuration\n".to_string(),
            root: flag_node(&["--option"], None),
        },
        SelfCheck {
            name: "nix's own multi-value-name row, the value name already recovered",
            why: "once `value_name` carries the run, the same row must go silent",
            expect: Expect::Silent,
            raw: "  · --option name value Set the Lix configuration\n".to_string(),
            root: flag_node(&["--option"], Some("<name value>")),
        },
        SelfCheck {
            name: "an ordinary option row with no value name at all",
            why: "`--debug Set the logging verbosity level` starts its sentence immediately; \
                  zero value-name words means nothing for this detector to claim",
            expect: Expect::Silent,
            raw: "  · --debug Set the logging verbosity level to 'debug'.\n".to_string(),
            root: flag_node(&["--debug"], None),
        },
    ]
}
