//! `positional-description-block` (atlas S-127): the usage block is
//! followed by an indented block naming each positional's own description
//! (`invoke-rc.d`'s `basename - Initscript ID, as per update-rc.d(8)`),
//! and the description never reaches the tree because no field ever asks
//! for it: the tree's positional is recovered by name, its description is
//! `None`.
//!
//! Fixture: `corpus/invoke-rc.d/1.66`.

use mandible_core::CommandNode;

pub struct Finding {
    pub name: String,
    pub description: String,
}

pub struct Report {
    pub findings: Vec<Finding>,
}

impl Report {
    pub fn finding_count(&self) -> usize {
        self.findings.len()
    }
}

fn leading_whitespace(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// Local copy of the dash-separator finder every bare-block reader in
/// this crate and `mandible_extract` shares: a hyphen with a space on
/// each side.
fn find_dash_separator(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut i = 1;
    while i + 1 < bytes.len() {
        if bytes[i] == b'-' && bytes[i - 1] == b' ' && bytes[i + 1] == b' ' {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Find, right after the raw text's own `usage:`/`Usage:` line, a block of
/// `name - description` rows (one blank line permitted in between) whose
/// names are a subset of `root`'s own positional names — the shape the
/// parser should have consumed into each positional's description but
/// left in `root.unparsed`/nowhere at all.
pub fn detect(raw: &str, root: &CommandNode) -> Report {
    let positional_names: Vec<&str> = root.positionals().map(|p| p.primary_name()).collect();
    if positional_names.is_empty() {
        return Report {
            findings: Vec::new(),
        };
    }
    let lines: Vec<&str> = raw.lines().collect();
    let Some(usage_idx) = lines
        .iter()
        .position(|l| l.trim_start().to_ascii_lowercase().starts_with("usage:"))
    else {
        return Report {
            findings: Vec::new(),
        };
    };
    let mut i = usage_idx + 1;
    // Skip the rest of the (possibly multi-line) usage synopsis itself:
    // any run of non-blank lines right after the `usage:` label line.
    while lines.get(i).is_some_and(|l| !l.trim().is_empty()) {
        i += 1;
    }
    while lines.get(i).is_some_and(|l| l.trim().is_empty()) {
        i += 1;
    }
    let Some(&first) = lines.get(i) else {
        return Report {
            findings: Vec::new(),
        };
    };
    if first.trim().is_empty() {
        return Report {
            findings: Vec::new(),
        };
    }
    let baseline = leading_whitespace(first);
    let mut rows: Vec<(String, String)> = Vec::new();
    let mut j = i;
    while let Some(&line) = lines.get(j) {
        if line.trim().is_empty() {
            break;
        }
        let indent = leading_whitespace(line);
        if indent < baseline {
            break;
        }
        if indent == baseline {
            let Some(dash_idx) = find_dash_separator(line) else {
                break;
            };
            let name = line[..dash_idx].trim().to_string();
            let desc = line[dash_idx + 1..].trim_start().to_string();
            if !positional_names.contains(&name.as_str()) {
                return Report {
                    findings: Vec::new(),
                };
            }
            rows.push((name, desc));
        } else if let Some(last) = rows.last_mut() {
            last.1.push(' ');
            last.1.push_str(line.trim());
        } else {
            return Report {
                findings: Vec::new(),
            };
        }
        j += 1;
    }

    let findings = rows
        .into_iter()
        .filter(|(name, _)| {
            root.positionals()
                .find(|p| p.primary_name() == name)
                .is_some_and(|p| p.description.is_none())
        })
        .map(|(name, description)| Finding { name, description })
        .collect();
    Report { findings }
}

// ----------------------------------------------------------------------
// Self-checks
// ----------------------------------------------------------------------

use crate::detector::{Expect, SelfCheck};
use mandible_core::{Entity, Provenance, Source};

/// `invoke-rc.d`'s real bytes, byte-exact (`corpus/invoke-rc.d/1.66/help.txt`).
pub(crate) const INVOKE_RC_D_USAGE: &str = concat!(
    "Usage: invoke-rc.d [options] <basename> <action> [extra parameters]\n",
    "\n",
    "  basename - Initscript ID, as per update-rc.d(8)\n",
    "  action   - Initscript action. Known actions are:\n",
    "                start, [force-]stop, [try-]restart,\n",
    "                [force-]reload, status\n",
);

fn node_with_positionals(name: &str, names: &[&str]) -> CommandNode {
    let mut root = CommandNode::new(name, Provenance::single(Source::HelpText));
    let entities = names
        .iter()
        .map(|n| Entity::positional(*n, Provenance::single(Source::HelpText)))
        .collect();
    root.set_entities_of(mandible_core::EntityKind::Positional, entities);
    root
}

pub(crate) fn self_checks() -> Vec<SelfCheck> {
    vec![
        SelfCheck {
            name: "invoke-rc.d's own bytes, neither description recovered",
            why: "the defect itself: both positionals exist by name and neither carries a \
                  description",
            expect: Expect::Fires(2),
            raw: INVOKE_RC_D_USAGE.to_string(),
            root: node_with_positionals("invoke-rc.d", &["basename", "action"]),
        },
        SelfCheck {
            name: "both descriptions already attached",
            why: "once a positional carries a description, the same raw block must go silent \
                  for it",
            expect: Expect::Silent,
            raw: INVOKE_RC_D_USAGE.to_string(),
            root: {
                let mut root = node_with_positionals("invoke-rc.d", &["basename", "action"]);
                for p in root.entities.iter_mut() {
                    p.description = Some(mandible_core::Text::sanitize("described already"));
                }
                root
            },
        },
        SelfCheck {
            name: "no positionals recovered at all",
            why: "with nothing to match the block's names against, this must never guess",
            expect: Expect::Silent,
            raw: INVOKE_RC_D_USAGE.to_string(),
            root: CommandNode::new("invoke-rc.d", Provenance::single(Source::HelpText)),
        },
        SelfCheck {
            name: "a dash-separated block whose names are not positionals",
            why: "an unrelated ` - ` list right under a usage line (a real command table, say) \
                  must not be laundered into a positional description just because it has the \
                  same separator",
            expect: Expect::Silent,
            raw: "Usage: prog <thing>\n\n  clone - clone a repo\n  init  - create one\n"
                .to_string(),
            root: node_with_positionals("prog", &["thing"]),
        },
    ]
}
