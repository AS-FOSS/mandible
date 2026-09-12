//! `alternation-value-is-choices` (atlas S-155): a value spec that is one
//! delimited alternation of two or more literal values —
//! `--compression=(xz|none|auto)`, `--crate-type
//! <bin|lib|rlib|dylib|cdylib|staticlib|proc-macro>`, `-l
//! {c,java,ruby,tcl}` — renders as an opaque placeholder holding the
//! whole list instead of the flag's own `choices`.
//!
//! Reimplements the shape rather than importing
//! `help_text::sections::emit`'s own private `alternation_choices` — the
//! same oracle-independence choice `choices_after_optional_placeholder`
//! already makes (corpus/README.md).
//!
//! Fixtures: `corpus/grub-mkimage/2.12/`, `corpus/rustc/1.97.1/`,
//! `corpus/tclobjnew-bpfcc/0.29.1/`.

use mandible_core::CommandNode;

pub struct Finding {
    pub delimiter: char,
    pub members: Vec<String>,
    pub line: String,
}

pub struct Report {
    pub findings: Vec<Finding>,
}

impl Report {
    pub fn finding_count(&self) -> usize {
        self.findings.len()
    }
}

/// True when `member` is a literal value rather than a metavar or a flag:
/// it opens with a lowercase letter or digit and carries nothing but
/// lowercase letters, digits, `_`, `.`, `+` or `-` after that. Rejects a
/// capitalized metavar (`Number`) and a token holding whitespace (fuser's
/// `-n SPACE`, inside a `[...]` group this detector never opens anyway).
fn is_literal_choice_member(member: &str) -> bool {
    let mut chars = member.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_lowercase() || first.is_ascii_digit())
        && chars.all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '.' | '+' | '-')
        })
}

/// The first delimited alternation candidate on `line`: an angle or paren
/// group split on `|` (at least three members — curl's own `-b, --cookie
/// <data|filename>` is the identical two-member angle shape and is an
/// either/or value-TYPE description, not a choice list; missing beats
/// invented below three), or, only when `is_argparse`, a brace group
/// split on `,` (at least two — argparse's `choices=` is a language-level
/// declaration and carries no such ambiguity). `None` when no such
/// group's content is a flat list of literal members — a real distinct
/// placeholder (`--units [Number]`, excluded since `[` is not one of the
/// three delimiters read here), a single member (`<platform>`), or a
/// metavar/flag member.
fn candidate(line: &str, is_argparse: bool) -> Option<(char, Vec<String>)> {
    let delimiters: &[(char, char, char, usize)] = if is_argparse {
        &[('<', '>', '|', 3), ('(', ')', '|', 3), ('{', '}', ',', 2)]
    } else {
        &[('<', '>', '|', 3), ('(', ')', '|', 3)]
    };
    for &(open, close, sep, min_members) in delimiters {
        let Some(start) = line.find(open) else {
            continue;
        };
        let rest = &line[start + open.len_utf8()..];
        let Some(end) = rest.find(close) else {
            continue;
        };
        let inner = &rest[..end];
        if inner.is_empty() || inner.contains(['<', '>', '(', ')', '{', '}', '[', ']']) {
            continue;
        }
        let members: Vec<&str> = inner.split(sep).collect();
        if members.len() < min_members || !members.iter().all(|m| is_literal_choice_member(m)) {
            continue;
        }
        return Some((open, members.into_iter().map(str::to_string).collect()));
    }
    None
}

pub fn detect(raw: &str, root: &CommandNode) -> Report {
    // Tier A′'s own weaker, help-text-signature step for argparse (spec
    // §7 Tier A′ rule 2.2): the fixed string argparse's `-h`/`--help`
    // always prints.
    let is_argparse = raw.contains("show this help message and exit");
    let mut findings = Vec::new();
    for line in raw.lines() {
        let Some((delimiter, members)) = candidate(line, is_argparse) else {
            continue;
        };
        let attached = root.flags().any(|e| {
            members.len() == e.choices.len()
                && members.iter().zip(&e.choices).all(|(m, c)| m == &c.name)
        });
        if !attached {
            findings.push(Finding {
                delimiter,
                members,
                line: line.to_string(),
            });
        }
    }
    Report { findings }
}

pub struct AlternationValueIsChoices;

impl crate::detector::Detector for AlternationValueIsChoices {
    fn name(&self) -> &'static str {
        "alternation-value-is-choices"
    }

    fn family(&self) -> Option<&'static str> {
        Some("alternation-value-is-choices")
    }

    fn describes(&self) -> &'static str {
        "a value spec that is one delimited alternation of two or more literal values \
         (`--compression=(xz|none|auto)`) renders as an opaque placeholder rather than the \
         flag's own choices"
    }

    fn hits(&self, evidence: &crate::detector::ToolEvidence<'_>) -> Vec<String> {
        detect(evidence.raw, evidence.root)
            .findings
            .into_iter()
            .map(|f| format!("{:?} never became choices, from {:?}", f.members, f.line))
            .collect()
    }

    fn self_checks(&self) -> Vec<SelfCheck> {
        self_checks()
    }
}

// ----------------------------------------------------------------------
// Self-checks
// ----------------------------------------------------------------------

use crate::detector::{Expect, SelfCheck};
use mandible_core::{Choice, Entity, Provenance, Source};

fn flag_with(long: &str, value_name: Option<&str>, choices: &[&str]) -> Entity {
    let mut e = Entity::flag_spelled(
        None,
        Some(long.to_string()),
        false,
        false,
        Provenance::single(Source::HelpText),
    );
    e.value_name = value_name.map(str::to_string);
    e.choices = choices.iter().map(|c| Choice::bare(*c)).collect();
    e
}

fn node_with(name: &str, flags: Vec<Entity>) -> CommandNode {
    let mut root = CommandNode::new(name, Provenance::single(Source::HelpText));
    root.set_entities_of(mandible_core::EntityKind::Flag, flags);
    root
}

pub(crate) fn self_checks() -> Vec<SelfCheck> {
    vec![
        SelfCheck {
            name: "grub-mkimage's own row, pre-fix shape (`--compression=(xz|none|auto)`)",
            why: "the defect itself: the tree still carries the whole list as an unread \
                  placeholder",
            expect: Expect::Fires(1),
            raw: "  -C, --compression=(xz|none|auto)          choose the compression\n".to_string(),
            root: node_with(
                "grub-mkimage",
                vec![flag_with("compression", Some("(xz|none|auto)"), &[])],
            ),
        },
        SelfCheck {
            name: "grub-mkimage's own row, post-fix shape (choices attached)",
            why: "once the alternation reads as choices, the identical raw row must go silent",
            expect: Expect::Silent,
            raw: "  -C, --compression=(xz|none|auto)          choose the compression\n".to_string(),
            root: node_with(
                "grub-mkimage",
                vec![flag_with("compression", None, &["xz", "none", "auto"])],
            ),
        },
        SelfCheck {
            name: "rustc's own `--crate-type` row, angle-bracket form",
            why: "the angle-bracket delimiter must generalize the same as the paren form",
            expect: Expect::Fires(1),
            raw: "    --crate-type <bin|lib|rlib|dylib|cdylib|staticlib|proc-macro>\n".to_string(),
            root: node_with(
                "rustc",
                vec![flag_with(
                    "crate-type",
                    Some("<bin|lib|rlib|dylib|cdylib|staticlib|proc-macro>"),
                    &[],
                )],
            ),
        },
        SelfCheck {
            name: "tclobjnew-bpfcc's own `--language` row, brace form under argparse",
            why: "argparse's own `{...}` metavar must generalize once the argparse marker is \
                  present in the raw text",
            expect: Expect::Fires(1),
            raw: "  -l {c,java,ruby,tcl}, --language {c,java,ruby,tcl}\n                        \
                  show this help message and exit\n"
                .to_string(),
            root: node_with(
                "tclobjnew-bpfcc",
                vec![flag_with("language", Some("{c,java,ruby,tcl}"), &[])],
            ),
        },
        SelfCheck {
            name: "a brace group with no argparse marker anywhere in the raw text",
            why: "the brace form is gated to the argparse profile; an identical brace group on \
                  a non-argparse tool must never be claimed",
            expect: Expect::Silent,
            raw: "  -l {c,java,ruby,tcl}, --language {c,java,ruby,tcl}\n".to_string(),
            root: node_with(
                "cache_restore",
                vec![flag_with("language", Some("{c,java,ruby,tcl}"), &[])],
            ),
        },
        SelfCheck {
            name: "curl's own `-b, --cookie <data|filename>`, a two-member value-TYPE description",
            why: "below three members, an angle-delimited pipe list is at least as often an \
                  either/or description of the value's own type as a real choice list — \
                  neither `data` nor `filename` is something a user picks literally, unlike \
                  `xz`/`none`/`auto` — so it must never be claimed",
            expect: Expect::Silent,
            raw: "  -b, --cookie <data|filename> Send cookies from string/file\n".to_string(),
            root: node_with("curl", vec![flag_with("cookie", Some("<data|filename>"), &[])]),
        },
        SelfCheck {
            name: "lvm2's own `--units [Number]r|R|h|...`, a real distinct placeholder",
            why: "a bracketed placeholder is not one of the three delimiters this rule reads; \
                  S-120/S-130 already own that shape and this detector must stay silent on it",
            expect: Expect::Silent,
            raw: "      --units [Number]r|R|h|H|b|B|s|S|k|K|m|M|g|G|t|T|p|P|e|E\n".to_string(),
            root: node_with(
                "pvdisplay",
                vec![flag_with(
                    "units",
                    Some("[Number]r|R|h|H|b|B|s|S|k|K|m|M|g|G|t|T|p|P|e|E"),
                    &[],
                )],
            ),
        },
        SelfCheck {
            name: "a single-member angle placeholder (`<FILE>`)",
            why: "one member is a real placeholder name, never a choice list, so it must never \
                  be claimed",
            expect: Expect::Silent,
            raw: "  -o, --output <FILE>        write output to FILE\n".to_string(),
            root: node_with("prog", vec![flag_with("output", Some("<FILE>"), &[])]),
        },
        SelfCheck {
            name: "an unbracketed alternative-type placeholder (`triplet|filename`)",
            why: "pkg-config's own `--personality=triplet|filename` has no enclosing \
                  delimiter, and nothing about the bare shape alone tells an enumerated choice \
                  list apart from an alternative-value-type placeholder — never claimed",
            expect: Expect::Silent,
            raw: "  --personality=triplet|filename   assume given personality\n".to_string(),
            root: node_with(
                "pkg-config",
                vec![flag_with("personality", Some("triplet|filename"), &[])],
            ),
        },
    ]
}
