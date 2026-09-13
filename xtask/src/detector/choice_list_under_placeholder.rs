//! `choice-list-under-placeholder` (atlas S-168): a colon-terminated
//! introducer sentence directly followed by a run of bare-name list
//! items, sitting under a placeholder row (or a `+word`/`-word` pair
//! sharing one), never folded into any row's own description.
//!
//! Fixture: `corpus/Xvfb/audit-seed`. `Xvfb --help`'s own
//! `+extension`/`-extension` pair documents the placeholder `name` this
//! way: a "the following extensions can be run-time enabled/disabled:"
//! sentence, then one extension name per line.

use crate::detector::{Detector, Expect, Scope, SelfCheck, ToolEvidence};
use mandible_core::CommandNode;

/// The fewest item lines a colon introducer must be followed by before
/// this is trusted as a real list rather than an ordinary sentence that
/// happens to end in `:`. Mirrors
/// `mandible_extract`'s own `MIN_NESTED_TABLE_ROWS` floor — an
/// independent copy, since extract and xtask do not share code (S-163's
/// own precedent).
const MIN_LIST_ROWS: usize = 2;

pub struct Finding {
    pub introducer: String,
    pub items: Vec<String>,
}

pub struct Report {
    pub findings: Vec<Finding>,
}

/// True when `line`, once trimmed, is nothing but a colon-terminated
/// sentence — never itself a flag row.
fn looks_like_choice_list_introducer(line: &str) -> bool {
    let trimmed = line.trim();
    let Some(body) = trimmed.strip_suffix(':') else {
        return false;
    };
    let body = body.trim();
    !body.is_empty() && !body.starts_with('-') && !body.starts_with('+')
}

/// True when a genuine column gap (a tab, or two or more spaces past the
/// first non-blank character) exists in `line` — the same evidence
/// `choice_description_sub_row` uses to recognize a *described* row
/// (S-015), which this detector must never mistake for a bare list item.
fn has_real_column_gap(line: &str) -> bool {
    let bytes = line.as_bytes();
    let mut seen_content = false;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b' ' || b == b'\t' {
            let start = i;
            let mut had_tab = false;
            while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
                had_tab |= bytes[i] == b'\t';
                i += 1;
            }
            if seen_content && (had_tab || i - start >= 2) {
                return true;
            }
        } else {
            seen_content = true;
            i += 1;
        }
    }
    false
}

/// True when `line` is one bare-name list item: a short run of hyphenated
/// words, with no genuine column gap anywhere (that gap is what a
/// *described* row, S-015's territory, carries instead).
fn looks_like_choice_list_item(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() || has_real_column_gap(trimmed) {
        return false;
    }
    let words: Vec<&str> = trimmed.split_whitespace().collect();
    words.len() <= 6
        && words
            .iter()
            .all(|w| !w.is_empty() && w.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
}

/// True when some flag entity in `root` already carries every one of
/// `items` as its own `choices` — the fixed shape, regardless of which
/// entity (`+word` or `-word`) is checked first.
fn some_flag_carries_every_choice(root: &CommandNode, items: &[String]) -> bool {
    root.flags().any(|f| {
        items
            .iter()
            .all(|item| f.choices.iter().any(|c| &c.name == item))
    })
}

pub fn detect(raw: &str, root: &CommandNode) -> Report {
    let mut findings = Vec::new();
    let lines: Vec<&str> = raw.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        if looks_like_choice_list_introducer(lines[i]) {
            let mut j = i + 1;
            while j < lines.len() && looks_like_choice_list_item(lines[j]) {
                j += 1;
            }
            if j - (i + 1) >= MIN_LIST_ROWS {
                let items: Vec<String> = lines[i + 1..j]
                    .iter()
                    .map(|l| l.trim().to_string())
                    .collect();
                if !some_flag_carries_every_choice(root, &items) {
                    findings.push(Finding {
                        introducer: lines[i].trim().to_string(),
                        items,
                    });
                }
                i = j;
                continue;
            }
        }
        i += 1;
    }
    Report { findings }
}

pub struct ChoiceListUnderPlaceholder;

impl Detector for ChoiceListUnderPlaceholder {
    fn name(&self) -> &'static str {
        "choice-list-under-placeholder"
    }

    fn family(&self) -> Option<&'static str> {
        None
    }

    fn describes(&self) -> &'static str {
        "a colon-introduced list of bare-name choices that never reached any flag's own \
         `choices` (S-168)"
    }

    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        detect(evidence.raw, evidence.root)
            .findings
            .iter()
            .map(|f| {
                format!(
                    "{:?} introduces {} items never attached as choices",
                    f.introducer,
                    f.items.len()
                )
            })
            .collect()
    }

    fn scope(&self) -> Scope {
        Scope::full()
    }

    fn self_checks(&self) -> Vec<SelfCheck> {
        self_checks()
    }
}

// ----------------------------------------------------------------------
// Self-checks
// ----------------------------------------------------------------------

use mandible_core::{Choice, Entity, Provenance, Spelling};

/// Xvfb's own shape, byte-exact (`corpus/Xvfb/audit-seed/help.stderr.txt`).
const XVFB_EXTENSION_LIST: &str = "+extension name        Enable extension\n\
     -extension name        Disable extension\n\
      Only the following extensions can be run-time enabled/disabled:\n\
     \tGeneric Event Extension\n\
     \tMIT-SHM\n\
     \tXTEST\n";

fn flag_with_choices(spelling: Spelling, choices: Vec<&str>) -> Entity {
    let mut e = Entity::new(mandible_core::EntityKind::Flag, Provenance::default());
    e.spellings = vec![spelling];
    e.choices = choices
        .into_iter()
        .map(|c| Choice {
            name: c.to_string(),
            description: None,
        })
        .collect();
    e
}

fn node_with(flags: Vec<Entity>) -> CommandNode {
    let mut root = CommandNode::new("Xvfb", Provenance::default());
    root.set_entities_of(mandible_core::EntityKind::Flag, flags);
    root
}

pub(crate) fn self_checks() -> Vec<SelfCheck> {
    vec![
        SelfCheck {
            name: "Xvfb's own extension list, never attached to either half of the pair",
            why: "the defect itself: the colon-introduced list reaches no flag's own choices",
            expect: Expect::Fires(1),
            raw: XVFB_EXTENSION_LIST.to_string(),
            root: node_with(vec![
                flag_with_choices(Spelling::bare("+extension"), vec![]),
                flag_with_choices(Spelling::single_dash("extension"), vec![]),
            ]),
        },
        SelfCheck {
            name: "Xvfb's own list, attached to the `-word` half",
            why: "once either half of the pair carries every item as its own choices, the \
                  list is accounted for",
            expect: Expect::Silent,
            raw: XVFB_EXTENSION_LIST.to_string(),
            root: node_with(vec![
                flag_with_choices(Spelling::bare("+extension"), vec![]),
                flag_with_choices(
                    Spelling::single_dash("extension"),
                    vec!["Generic Event Extension", "MIT-SHM", "XTEST"],
                ),
            ]),
        },
        SelfCheck {
            name: "an ordinary sentence ending in a colon with nothing list-shaped beneath it",
            why: "one line, or none, is cheap to produce by coincidence — never enough evidence \
                  of a real list on its own",
            expect: Expect::Silent,
            raw: "Notes:\nThis tool reads its configuration from the environment.\n".to_string(),
            root: node_with(vec![]),
        },
        SelfCheck {
            name: "as's own described sub-option rows, each with a real column gap",
            why: "a described row (S-015's own territory) is never mistaken for a bare list \
                  item: the column gap between name and description is exactly what a plain \
                  list item never carries",
            expect: Expect::Silent,
            raw: "Sub-options [default hls]:\n\
                  \tc      omit false conditionals\n\
                  \td      omit debugging directives\n\
                  \tg      include general info\n"
                .to_string(),
            root: node_with(vec![]),
        },
    ]
}
