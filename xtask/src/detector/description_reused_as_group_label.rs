//! `description-reused-as-group-label` (atlas S-164): a root flag's own
//! `group` equals, verbatim (trimmed), the node's own root `description`
//! — the same sentence shown twice, once as the description and once as
//! the label over a whole flag block (`fc-scan`, `grub-macbless`). Reads
//! only the parsed tree, since both fields already live on
//! `CommandNode`/`Entity`.

use crate::detector::{Detector, Expect, Scope, SelfCheck, ToolEvidence};
use mandible_core::{CommandNode, Entity, EntityKind, Provenance, Source, Text};

pub struct DescriptionReusedAsGroupLabel;

impl Detector for DescriptionReusedAsGroupLabel {
    fn name(&self) -> &'static str {
        "description-reused-as-group-label"
    }

    fn family(&self) -> Option<&'static str> {
        None
    }

    fn describes(&self) -> &'static str {
        "a root flag's own group equals the node's root description verbatim, the same \
         sentence rendered twice"
    }

    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        let Some(description) = evidence.root.description.as_ref().map(|t| t.as_str()) else {
            return Vec::new();
        };
        let description = description.trim();
        if description.is_empty() {
            return Vec::new();
        }
        let mut groups_seen = std::collections::HashSet::new();
        let mut findings = Vec::new();
        for flag in evidence.root.flags() {
            let Some(group) = flag.group.as_deref() else {
                continue;
            };
            if group.trim() == description && groups_seen.insert(group.to_string()) {
                findings.push(format!(
                    "root flag group {group:?} repeats the root description verbatim"
                ));
            }
        }
        findings
    }

    fn scope(&self) -> Scope {
        Scope::full()
    }

    fn self_checks(&self) -> Vec<SelfCheck> {
        fn flag_with_group(spelling: &str, group: Option<&str>) -> Entity {
            let mut e = Entity::new(EntityKind::Flag, Provenance::single(Source::HelpText));
            e.spellings.push(mandible_core::Spelling::long(spelling));
            e.group = group.map(str::to_string);
            e
        }
        fn node(description: Option<&str>, flags: Vec<Entity>) -> CommandNode {
            let mut root = CommandNode::new("prog", Provenance::single(Source::HelpText));
            root.description = description.map(Text::sanitize);
            root.set_entities_of(EntityKind::Flag, flags);
            root
        }

        let sentence = "Scan font files and directories, and print resulting pattern(s)";

        vec![
            SelfCheck {
                name: "fc-scan's own shape, the sentence doubles as the group",
                why: "the defect itself: the same sentence is the description and a group",
                expect: Expect::Fires(1),
                raw: String::new(),
                root: node(
                    Some(sentence),
                    vec![flag_with_group("brief", Some(sentence))],
                ),
            },
            SelfCheck {
                name: "the same tree, the group cleared once the fix lands",
                why: "once the group is refused, the same tree goes silent",
                expect: Expect::Silent,
                raw: String::new(),
                root: node(Some(sentence), vec![flag_with_group("brief", None)]),
            },
            SelfCheck {
                name: "gcc-ranlib-13's own real label, distinct text",
                why: "a group whose text differs from the description is a real label and must \
                      never fire",
                expect: Expect::Silent,
                raw: String::new(),
                root: node(
                    Some(sentence),
                    vec![flag_with_group("brief", Some("The options are"))],
                ),
            },
            SelfCheck {
                name: "a node with no description at all",
                why: "nothing was ever spent as a description, so no group can repeat it",
                expect: Expect::Silent,
                raw: String::new(),
                root: node(None, vec![flag_with_group("brief", Some(sentence))]),
            },
        ]
    }
}
