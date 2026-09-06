//! `invocation-form-head-as-flag-group` (atlas S-137): lvm2's own
//! one-stanza-per-invocation-form `--help` layout reads each later form's
//! own invocation line (`lvcreate -m|--mirrors Number -L|--size
//! Size[m|UNIT] VG`) as a fabricated section heading, and files that
//! form's option rows under it — instead of the prose sentence directly
//! above it (`Create a raid1 or mirror LV.`), the only human-written
//! label the tool gives them. A local, independent copy of the shape
//! check, not an import: `xtask` and `mandible-extract` do not share
//! code (corpus/README.md's own rule for oracles). No seed-2/4/5/6
//! labelled tool carries this shape, so [`Detector::family`] returns
//! `None`.

use crate::detector::{Detector, Expect, SelfCheck, ToolEvidence};
use mandible_core::{CommandNode, Entity, Provenance, Source};

/// True when `token` (already known to start the group's second-or-later
/// word) reads as a flag spelling an invocation line would carry:
/// dash-led, at least one more character, and every character after an
/// optional leading `(` is alphanumeric, `-` or `|` — clean enough to
/// exclude a parenthetical aside like `"(-x)"`, which a real heading
/// sometimes carries and this detector must not claim.
fn is_invocation_flag_token(token: &str) -> bool {
    let core = token.trim_start_matches('(');
    if !core.starts_with('-') || core.len() < 2 {
        return false;
    }
    let after_dash = &core[1..];
    after_dash
        .chars()
        .next()
        .is_some_and(|c| c.is_alphanumeric())
        && core
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '|'))
}

/// True when `group`, trimmed, reads as an invocation line rather than a
/// prose label: it does not end in a full stop (a real stanza label
/// always does, `stanza_description_above`'s own terminator requirement),
/// its first word is name-shaped (no leading dash, no leading digit), and
/// at least one later word is [`is_invocation_flag_token`]-shaped.
pub(crate) fn looks_like_invocation_line_group(group: &str) -> bool {
    let trimmed = group.trim();
    if trimmed.is_empty() || trimmed.ends_with('.') {
        return false;
    }
    let mut words = trimmed.split_whitespace();
    let Some(first) = words.next() else {
        return false;
    };
    let Some(first_char) = first.chars().next() else {
        return false;
    };
    if !first_char.is_alphabetic() {
        return false;
    }
    words.any(is_invocation_flag_token)
}

pub struct InvocationFormHeadAsFlagGroup;

impl Detector for InvocationFormHeadAsFlagGroup {
    fn name(&self) -> &'static str {
        "invocation-form-head-as-flag-group"
    }

    fn family(&self) -> Option<&'static str> {
        None
    }

    fn describes(&self) -> &'static str {
        "a flag's own group is an invocation line (tool name plus flag spellings) rather than \
         the prose sentence a multi-form tool wrote directly above it"
    }

    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        let _ = evidence.raw;
        evidence
            .root
            .flags()
            .filter_map(|e| {
                let group = e.group.as_deref()?;
                looks_like_invocation_line_group(group).then(|| {
                    format!(
                        "{:?} grouped under invented invocation-line heading {group:?}",
                        e.spellings
                            .iter()
                            .map(|s| s.name.clone())
                            .collect::<Vec<_>>()
                    )
                })
            })
            .collect()
    }

    fn self_checks(&self) -> Vec<SelfCheck> {
        fn node_with(group: Option<&str>) -> CommandNode {
            let mut root = CommandNode::new("lvcreate", Provenance::single(Source::HelpText));
            let mut flag = Entity::flag_long("mirrorlog", Provenance::single(Source::HelpText));
            flag.group = group.map(str::to_string);
            root.entities = vec![flag];
            root
        }
        vec![
            SelfCheck {
                name: "lvcreate's own pre-fix shape: the invocation line itself as a group",
                why: "the defect itself: the second form's own usage line, not the prose above \
                      it, became the group",
                expect: Expect::Fires(1),
                raw: String::new(),
                root: node_with(Some(
                    "lvcreate -m|--mirrors Number -L|--size Size[m|UNIT] VG",
                )),
            },
            SelfCheck {
                name: "lvcreate's own post-fix shape: the prose sentence above the form",
                why: "once the fix lands, the group is the human-written label, a real \
                      full-stop-terminated sentence, and this must go silent",
                expect: Expect::Silent,
                raw: String::new(),
                root: node_with(Some("Create a raid1 or mirror LV.")),
            },
            SelfCheck {
                name: "a real named section, never an invocation line",
                why: "an ordinary heading with no flag-shaped word must never be claimed",
                expect: Expect::Silent,
                raw: String::new(),
                root: node_with(Some("Common options for command:")),
            },
            SelfCheck {
                name: "no group at all",
                why: "an ungrouped flag has nothing for this detector to read",
                expect: Expect::Silent,
                raw: String::new(),
                root: node_with(None),
            },
            SelfCheck {
                name: "a real heading carrying a parenthetical flag aside",
                why: "a heading like a tool's own \"Startup options (-x)\" must not be mistaken \
                      for an invocation line just because it names a flag in passing",
                expect: Expect::Silent,
                raw: String::new(),
                root: node_with(Some("Startup options (-x)")),
            },
        ]
    }
}
