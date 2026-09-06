//! `invocation-form-head-as-flag-group` (atlas S-137): lvm2's own
//! one-stanza-per-invocation-form `--help` layout reads a later form's own
//! invocation line as a fabricated section heading instead of the prose
//! sentence above it. Local, independent shape check (no shared code with
//! `mandible-extract`). No seed-2/4/5/6 labelled tool carries this shape,
//! so [`Detector::family`] returns `None`.
//!
//! Gated on the raw capture: a tool may correctly fall back to its own
//! head line, with no prose above it (`pydoc3`) or one missing its period
//! (`pvck`). `hits` fires only when the raw text shows a real label was
//! actually discarded.

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
/// prose label: it does not end in a full stop or colon (a real stanza
/// label always ends in a full stop, a real section heading in a colon),
/// its first word is name-shaped (alphanumeric, `-` or `_` only — never a
/// parenthesized aside like `pod2usage(-exitval`), and at least one later
/// word is [`is_invocation_flag_token`]-shaped.
pub(crate) fn looks_like_invocation_line_group(group: &str) -> bool {
    let trimmed = group.trim();
    if trimmed.is_empty() || trimmed.ends_with('.') || trimmed.ends_with(':') {
        return false;
    }
    let mut words = trimmed.split_whitespace();
    let Some(first) = words.next() else {
        return false;
    };
    if !first
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
        || !first
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
    {
        return false;
    }
    words.any(is_invocation_flag_token)
}

/// True when `raw` carries `group` as one of its own physical lines
/// (trimmed) with a genuine, full-stop-terminated prose sentence of at
/// least three words directly above it — real evidence a label existed
/// and was discarded, not merely that the group text happens to look
/// invocation-shaped.
fn has_discarded_prose_label(raw: &str, group: &str) -> bool {
    let lines: Vec<&str> = raw.lines().collect();
    let Some(idx) = lines.iter().position(|l| l.trim() == group) else {
        return false;
    };
    let Some(prev) = idx.checked_sub(1).and_then(|i| lines.get(i)) else {
        return false;
    };
    let prev = prev.trim();
    let trimmed_end = prev.trim_end();
    trimmed_end.ends_with('.')
        && !trimmed_end.ends_with("...")
        && prev.split_whitespace().count() >= 3
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
         the prose sentence a multi-form tool wrote directly above it, itself still present in \
         the raw capture"
    }

    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        evidence
            .root
            .flags()
            .filter_map(|e| {
                let group = e.group.as_deref()?;
                if !looks_like_invocation_line_group(group) {
                    return None;
                }
                if !has_discarded_prose_label(evidence.raw, group) {
                    return None;
                }
                Some(format!(
                    "{:?} grouped under invented invocation-line heading {group:?}",
                    e.spellings
                        .iter()
                        .map(|s| s.name.clone())
                        .collect::<Vec<_>>()
                ))
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
        const FORM: &str = "lvcreate -m|--mirrors Number -L|--size Size[m|UNIT] VG";
        vec![
            SelfCheck {
                name: "lvcreate's own pre-fix shape: the invocation line itself as a group, \
                       with its real label still sitting above it in the raw capture",
                why: "the defect itself: the second form's own usage line, not the prose above \
                      it, became the group, and the raw text proves the prose was there to use",
                expect: Expect::Fires(1),
                raw: format!("  Create a raid1 or mirror LV.\n  {FORM}\n"),
                root: node_with(Some(FORM)),
            },
            SelfCheck {
                name: "lvcreate's own post-fix shape: the prose sentence above the form",
                why: "once the fix lands, the group is the human-written label, a real \
                      full-stop-terminated sentence, and this must go silent",
                expect: Expect::Silent,
                raw: format!("  Create a raid1 or mirror LV.\n  {FORM}\n"),
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
            SelfCheck {
                name: "an invocation-shaped group with no raw evidence of a discarded label",
                why: "pydoc3's own stanzas fall back to their head line correctly, because no \
                      prose sentence precedes them at all (S-012's documented no-description \
                      case) — the shape alone must never be enough",
                expect: Expect::Silent,
                raw: String::new(),
                root: node_with(Some(FORM)),
            },
            SelfCheck {
                name: "a real label above the form, but missing its own terminating period",
                why: "pvck's own \"Repair LVM headers and metadata on a device\" never gained \
                      the period stanza_description_above requires, so nothing was actually \
                      discarded — a data-quality gap in the tool's own emitter, not this \
                      family's defect",
                expect: Expect::Silent,
                raw: format!("  Repair LVM headers and metadata on a device\n  {FORM}\n"),
                root: node_with(Some(FORM)),
            },
        ]
    }
}
