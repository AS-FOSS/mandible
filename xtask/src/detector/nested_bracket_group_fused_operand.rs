//! `nested-bracket-group-fused-operand` (atlas S-154): a usage line's
//! nested bracket group of ALL-CAPS words reaches the tree as ONE
//! positional whose name is those words joined by a space (`uniq`'s
//! `[INPUT [OUTPUT]]` becoming `INPUT OUTPUT`). The sibling of
//! `trailing_bracket_group_multiword_operand` (S-132), inverted: that one
//! counts an operand the tree never got, this one counts an operand name
//! the tree invented. A FLAT group's joined name is the legitimate S-154
//! read (`mknod`'s `[MAJOR MINOR]`) and is silent here. No labelled tool
//! carries this shape, so [`Detector::family`] returns `None`. Fixtures:
//! corpus/uniq/9.4 and corpus/env/9.4.

use crate::detector::{Detector, Expect, Scope, SelfCheck, ToolEvidence};
use mandible_core::CommandNode;

pub struct Finding {
    pub name: String,
    pub group: String,
    pub line: String,
}

pub struct Report {
    pub findings: Vec<Finding>,
}

/// One ALL-CAPS operand word: uppercase letters or underscores only, and
/// more than one character, so a stray `A` or a lowercase prose word is
/// never counted. The same test the parser's own token loop applies.
fn all_caps_word(w: &str) -> bool {
    w.len() > 1 && w.chars().all(|c| c.is_uppercase() || c == '_')
}

/// Every depth-zero bracket group in `line`, each as its own source span
/// including both brackets. An unmatched `[` yields nothing, the way an
/// unterminated group means nothing to a reader either.
fn bracket_groups(line: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = None;
    for (i, c) in line.char_indices() {
        match c {
            '[' => {
                if depth == 0 {
                    start = Some(i);
                }
                depth += 1;
            }
            ']' => {
                depth -= 1;
                if depth == 0 {
                    if let Some(s) = start.take() {
                        out.push(&line[s..=i]);
                    }
                }
                depth = depth.max(0);
            }
            _ => {}
        }
    }
    out
}

/// The space-joined name a nested group's own ALL-CAPS words would fuse
/// into, or `None` when the group is not that shape. Nested means the
/// group holds a further `[` inside it, which is what makes the words
/// separate operands rather than one name: `[INPUT [OUTPUT]]` is `INPUT`
/// and an optional `OUTPUT`, never an operand called `INPUT OUTPUT`.
/// Every whitespace-delimited word in the group must read ALL-CAPS once
/// brackets and dots are trimmed, and there must be two or more, so a
/// group carrying prose or a `name=value` pair claims nothing.
fn fused_name(group: &str) -> Option<String> {
    let inner = group.get(1..group.len() - 1)?;
    if !inner.contains('[') {
        return None;
    }
    let mut words = Vec::new();
    for token in inner.split_whitespace() {
        let cleaned = token.trim_matches(|c| c == '[' || c == ']' || c == '.');
        if cleaned.is_empty() {
            continue;
        }
        if !all_caps_word(cleaned) {
            return None;
        }
        words.push(cleaned);
    }
    (words.len() >= 2).then(|| words.join(" "))
}

/// Every line of the raw capture is read, not only the primary synopsis:
/// `ptx` writes its nested group on its second invocation form (`or:
/// ptx -G [OPTION]... [INPUT [OUTPUT]]`), and the parser reads operands
/// off whichever form it treats as primary. Requiring the fused name to
/// be present in the tree keeps that breadth from costing precision —
/// nothing but the fusing rule itself produces a positional spelled with
/// a space that the group's own words spell out in order.
pub fn detect(raw: &str, root: &CommandNode) -> Report {
    let mut findings = Vec::new();
    for line in raw.lines() {
        for group in bracket_groups(line) {
            let Some(name) = fused_name(group) else {
                continue;
            };
            if root.positionals().any(|p| p.primary_name() == name) {
                findings.push(Finding {
                    name,
                    group: group.to_string(),
                    line: line.to_string(),
                });
            }
        }
    }
    Report { findings }
}

pub struct NestedBracketGroupFusedOperand;

impl Detector for NestedBracketGroupFusedOperand {
    fn name(&self) -> &'static str {
        "nested-bracket-group-fused-operand"
    }

    fn family(&self) -> Option<&'static str> {
        None
    }

    fn describes(&self) -> &'static str {
        "a usage line's nested bracket group of ALL-CAPS words reaches the tree as one positional \
         whose name is those words joined by a space"
    }

    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        detect(evidence.raw, evidence.root)
            .findings
            .iter()
            .map(|f| {
                format!(
                    "positional {:?} fused the nested group {:?}, from {:?}",
                    f.name, f.group, f.line
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

use mandible_core::{Entity, Provenance, Source};

/// `uniq`'s real usage line, byte-exact (corpus/uniq/9.4's `help.txt`).
pub(crate) const UNIQ_USAGE: &str = "Usage: uniq [OPTION]... [INPUT [OUTPUT]]\n";

/// `env`'s real usage line, byte-exact (corpus/env/9.4's `help.txt`).
pub(crate) const ENV_USAGE: &str =
    "Usage: env [OPTION]... [-] [NAME=VALUE]... [COMMAND [ARG]...]\n";

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
            name: "uniq's own bytes, the two words fused into one operand",
            why: "the defect itself: `[INPUT [OUTPUT]]` names two optional operands and the \
                  tree carries one called `INPUT OUTPUT`, a name uniq never documents",
            expect: Expect::Fires(1),
            raw: UNIQ_USAGE.to_string(),
            root: node_with_positionals("uniq", &["INPUT OUTPUT"]),
        },
        SelfCheck {
            name: "uniq's own bytes, the two operands read apart",
            why: "once the tree carries `INPUT` and `OUTPUT` separately, the same usage line \
                  must go silent",
            expect: Expect::Silent,
            raw: UNIQ_USAGE.to_string(),
            root: node_with_positionals("uniq", &["INPUT", "OUTPUT"]),
        },
        SelfCheck {
            name: "env's own bytes, the inner repetition marker fused too",
            why: "`[COMMAND [ARG]...]` marks the inner operand repeatable, not a joined name; \
                  the `[NAME=VALUE]...` group ahead of it must claim nothing here, since its \
                  own word is not ALL-CAPS once the `=` pair is read as one word",
            expect: Expect::Fires(1),
            raw: ENV_USAGE.to_string(),
            root: node_with_positionals("env", &["COMMAND ARG"]),
        },
        SelfCheck {
            name: "mknod's own bytes, a flat group's joined name is correct",
            why: "the legitimate S-154 read: `[MAJOR MINOR]` has no inner bracket, so its two \
                  words are one operand name and this detector must not call it a defect",
            expect: Expect::Silent,
            raw: "Usage: mknod [OPTION]... NAME TYPE [MAJOR MINOR]\n".to_string(),
            root: node_with_positionals("mknod", &["NAME", "TYPE", "MAJOR MINOR"]),
        },
        SelfCheck {
            name: "gdk-pixbuf-thumbnailer's own bytes, two flat groups side by side",
            why: "two flat groups on one line are two operand names (issue #135's own fixture), \
                  not a nesting — neither may be reported",
            expect: Expect::Silent,
            raw: "  gdk-pixbuf-thumbnailer [OPTION\u{2026}] [INPUT FILE] [OUTPUT FILE] Thumbnail \
                  images\n"
                .to_string(),
            root: node_with_positionals("gdk-pixbuf-thumbnailer", &["INPUT FILE", "OUTPUT FILE"]),
        },
        SelfCheck {
            name: "systemd-sysusers's own bytes, a flat group ending in its own dots",
            why: "`[CONFIGURATION FILE...]` is one repeatable operand: the dots mark the \
                  group's own repetition and add no nesting, so the joined name stands",
            expect: Expect::Silent,
            raw: "systemd-sysusers [OPTIONS...] [CONFIGURATION FILE...]\n".to_string(),
            root: node_with_positionals("systemd-sysusers", &["CONFIGURATION FILE"]),
        },
        SelfCheck {
            name: "parted's own bytes, three words behind two nestings",
            why: "the deepest fleet specimen: `[DEVICE [COMMAND [PARAMETERS]...]...]` fused all \
                  three words into one name, so a detector that only looked one level deep \
                  would miss it",
            expect: Expect::Fires(1),
            raw: "Usage: parted [OPTION]... [DEVICE [COMMAND [PARAMETERS]...]...]\n".to_string(),
            root: node_with_positionals("parted", &["DEVICE COMMAND PARAMETERS"]),
        },
        SelfCheck {
            name: "ptx's own bytes, the nested group on a later invocation form",
            why: "ptx writes `[INPUT [OUTPUT]]` on its second form, not its first, and the \
                  parser still read operands off it — reading only the primary synopsis line \
                  would report zero while the tree carries the fused name",
            expect: Expect::Fires(1),
            raw: "Usage: ptx [OPTION]... [INPUT]...   (without -G)\n  or:  ptx -G [OPTION]... \
                  [INPUT [OUTPUT]]\n"
                .to_string(),
            root: node_with_positionals("ptx", &["INPUT", "INPUT OUTPUT"]),
        },
    ]
}
