//! `headingless-table-in-root-description` (atlas S-165): the root's own
//! `description` still carries several of the tree's own flag spellings
//! as substrings — the option table rendered once as prose in the
//! description and a second time as the tree's real flags (`Xvfb`, before
//! the fix). Reads only the parsed tree: a description that happens to
//! repeat three or more of the node's own recovered spellings, each at a
//! real word boundary, is treated as the table leaking through rather
//! than a coincidental mention.

use crate::detector::{Detector, Expect, Scope, SelfCheck, ToolEvidence};
use mandible_core::{CommandNode, Entity, EntityKind, Provenance, Source, Text};

/// The fewest of the tree's own flag spellings a description must repeat
/// before this reads as the table leaking through rather than an
/// ordinary sentence that happens to mention one flag by name (a real,
/// common shape: `"see --verbose for more"`).
const MIN_REPEATED_SPELLINGS: usize = 3;

/// True when `spelling` occurs in `text` at a real word boundary: not
/// preceded or followed by a letter, digit, `-` or `+` — so `-a` inside
/// `-audit` never counts as `-a` occurring.
fn occurs_at_word_boundary(text: &str, spelling: &str) -> bool {
    if spelling.is_empty() {
        return false;
    }
    let is_word_char = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '+';
    let mut start = 0;
    while let Some(idx) = text[start..].find(spelling) {
        let idx = start + idx;
        let before_ok = text[..idx]
            .chars()
            .next_back()
            .is_none_or(|c| !is_word_char(c));
        let after = idx + spelling.len();
        let after_ok = text[after..]
            .chars()
            .next()
            .is_none_or(|c| !is_word_char(c));
        if before_ok && after_ok {
            return true;
        }
        start = idx + 1;
    }
    false
}

pub struct HeadinglessTableInRootDescription;

impl Detector for HeadinglessTableInRootDescription {
    fn name(&self) -> &'static str {
        "headingless-table-in-root-description"
    }

    fn family(&self) -> Option<&'static str> {
        None
    }

    fn describes(&self) -> &'static str {
        "the root description still repeats several of the tree's own recovered flag \
         spellings, the option table rendered once as prose and again as real flags"
    }

    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        let Some(description) = evidence.root.description.as_ref().map(|t| t.as_str()) else {
            return Vec::new();
        };
        let mut repeated: Vec<String> = Vec::new();
        for flag in evidence.root.flags() {
            for spelling in &flag.spellings {
                let rendered = spelling.typed();
                if occurs_at_word_boundary(description, &rendered) {
                    repeated.push(rendered);
                    break;
                }
            }
        }
        if repeated.len() >= MIN_REPEATED_SPELLINGS {
            vec![format!(
                "root description repeats {} of the tree's own flag spellings: {:?}",
                repeated.len(),
                repeated
            )]
        } else {
            Vec::new()
        }
    }

    fn scope(&self) -> Scope {
        Scope::full()
    }

    fn self_checks(&self) -> Vec<SelfCheck> {
        fn flag(spelling: mandible_core::Spelling) -> Entity {
            let mut e = Entity::new(EntityKind::Flag, Provenance::single(Source::HelpText));
            e.spellings.push(spelling);
            e
        }
        fn node(description: Option<&str>, flags: Vec<Entity>) -> CommandNode {
            let mut root = CommandNode::new("Xvfb", Provenance::single(Source::HelpText));
            root.description = description.map(Text::sanitize);
            root.set_entities_of(EntityKind::Flag, flags);
            root
        }

        let table_prose = "use: X [:<display>] [option] -a # default pointer acceleration \
                            (factor) -ac disable access control restrictions -audit int set \
                            audit trail level";

        vec![
            SelfCheck {
                name: "Xvfb's own shape, the table duplicated into the description",
                why: "the defect itself: three of the tree's own real flags recur in the \
                      description's own prose",
                expect: Expect::Fires(1),
                raw: String::new(),
                root: node(
                    Some(table_prose),
                    vec![
                        flag(mandible_core::Spelling::single_dash("a")),
                        flag(mandible_core::Spelling::single_dash("ac")),
                        flag(mandible_core::Spelling::single_dash("audit")),
                    ],
                ),
            },
            SelfCheck {
                name: "the same tree, description trimmed to just the usage line",
                why: "once the table no longer lands in the description, the same tree goes \
                      silent",
                expect: Expect::Silent,
                raw: String::new(),
                root: node(
                    Some("use: X [:<display>] [option]"),
                    vec![
                        flag(mandible_core::Spelling::single_dash("a")),
                        flag(mandible_core::Spelling::single_dash("ac")),
                        flag(mandible_core::Spelling::single_dash("audit")),
                    ],
                ),
            },
            SelfCheck {
                name: "an ordinary description mentioning one real flag by name",
                why: "one mention is common prose (\"see --verbose\"), never the whole table \
                      leaking through, and must never fire",
                expect: Expect::Silent,
                raw: String::new(),
                root: node(
                    Some("Run the build. See --verbose for more detail."),
                    vec![flag(mandible_core::Spelling::long("verbose"))],
                ),
            },
            SelfCheck {
                name: "a node with no description at all",
                why: "nothing to repeat a spelling in",
                expect: Expect::Silent,
                raw: String::new(),
                root: node(None, vec![flag(mandible_core::Spelling::single_dash("a"))]),
            },
        ]
    }
}
