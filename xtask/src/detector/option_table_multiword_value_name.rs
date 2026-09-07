//! `option-table-multiword-value-name` (round 9, atlas S-148): the
//! option-table sibling of S-131's own usage-synopsis shape. `argparse`
//! writes an option-table row's flag and metavar on their own line when
//! the pair is too wide to share the line with the description
//! (`gdbus-codegen --help`'s `--annotate WHAT KEY VALUE WHAT KEY VALUE
//! WHAT KEY VALUE`, description wrapped below it) — a value placeholder
//! that itself holds several words, on a table row rather than inside a
//! usage-line bracket group.

// Count only, no fix shipped this round (the A5 brief): the block-
// derived option-table reader wins over the usage-derived one for a
// flag documented in both places (`help_text/sections/mod.rs`'s own
// "let the described version win"), so `--annotate` keeps only its
// first metavar word (`WHAT`) even though `usage-bracket-group-
// multiword-value` already reads the fuller value out of the usage
// line for a flag with no table row of its own. No seed-2/4/5/6
// labelled tool carries this shape, so `Detector::family` returns
// `None` (spec §13.1e rule 6).

use crate::detector::{Detector, Expect, Scope, SelfCheck, ToolEvidence};
use mandible_core::CommandNode;

pub struct Finding {
    pub flag: String,
    pub words: String,
    pub line: String,
}

pub struct Report {
    pub findings: Vec<Finding>,
}

/// A plain metavar word: letter-led, only letters/digits/`-`/`_`, and
/// (unlike `usage_bracket_group_multiword_value`'s own `plain_word`,
/// which accepts ordinary prose too) **entirely upper-case** — the
/// convention `argparse` and its siblings write a metavar in, and the
/// restriction that keeps this rule off an ordinary single-line
/// description that happens to start with capitalized prose words.
fn plain_metavar_word(w: &str) -> bool {
    !w.is_empty()
        && w.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && w.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        && w.chars().any(|c| c.is_ascii_alphabetic())
        && w.chars()
            .filter(|c| c.is_ascii_alphabetic())
            .all(|c| c.is_ascii_uppercase())
}

/// `Some((flag, "word1 word2 ..."))` when `line`, trimmed, is nothing but
/// one long-flag spelling followed by two or more all-uppercase metavar
/// words — an option-table row whose own line carries no description at
/// all (the wrapped-description shape this rule exists for). `None` for
/// a usage line (starts with a bracket, not a bare flag), a single-word
/// value, a choices brace group, or a row that also carries prose on the
/// same line.
fn multiword_table_row_shape(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();
    let mut words = trimmed.split_whitespace();
    let flag = words.next()?;
    if !flag.starts_with("--") || flag.len() < 3 || flag.contains('=') {
        return None;
    }
    let rest: Vec<&str> = words.collect();
    if rest.len() < 2 || !rest.iter().all(|w| plain_metavar_word(w)) {
        return None;
    }
    Some((flag.to_string(), rest.join(" ")))
}

fn flag_value_name<'a>(root: &'a CommandNode, flag: &str) -> Option<&'a str> {
    let trimmed = flag.trim_start_matches('-');
    root.flags()
        .find(|e| e.long() == Some(trimmed))
        .and_then(|e| e.value_name.as_deref())
}

pub fn detect(raw: &str, root: &CommandNode) -> Report {
    let mut findings = Vec::new();
    for line in raw.lines() {
        // A usage line's own continuation is indented but starts with a
        // bracket (`[--annotate ...]`), never a bare `--flag`, so it never
        // reaches `multiword_table_row_shape` at all — no explicit
        // "usage:" skip is needed the way `usage_bracket_group_multiword_value`
        // needs one to find its own line.
        let Some((flag, words)) = multiword_table_row_shape(line) else {
            continue;
        };
        if flag_value_name(root, &flag) != Some(words.as_str()) {
            findings.push(Finding {
                flag,
                words,
                line: line.to_string(),
            });
        }
    }
    Report { findings }
}

pub struct OptionTableMultiwordValueName;

impl Detector for OptionTableMultiwordValueName {
    fn name(&self) -> &'static str {
        "option-table-multiword-value-name"
    }

    fn family(&self) -> Option<&'static str> {
        None
    }

    fn describes(&self) -> &'static str {
        "an option-table row holding one long-flag spelling followed by two or more all-\
         uppercase metavar words and nothing else on its own line — a value placeholder that \
         itself holds a space, on a table row rather than a usage-line bracket group"
    }

    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        detect(evidence.raw, evidence.root)
            .findings
            .iter()
            .map(|f| {
                format!(
                    "{} never carried value name {:?}, from {:?}",
                    f.flag, f.words, f.line
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

/// `gdbus-codegen`'s real option-table row, byte-exact (its own `--help`).
pub(crate) const GDBUS_CODEGEN_ANNOTATE_ROW: &str =
    "  --annotate WHAT KEY VALUE WHAT KEY VALUE WHAT KEY VALUE\n\
     \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\
     Add annotation (may be used several times)\n";

fn flag_with_value(long: &str, value_name: Option<&str>) -> Entity {
    let mut e = Entity::flag_long(long, Provenance::single(Source::HelpText));
    e.value_name = value_name.map(str::to_string);
    e
}

fn node_with_flags(name: &str, flags: Vec<Entity>) -> CommandNode {
    let mut root = CommandNode::new(name, Provenance::single(Source::HelpText));
    root.set_entities_of(mandible_core::EntityKind::Flag, flags);
    root
}

pub(crate) fn self_checks() -> Vec<SelfCheck> {
    vec![
        SelfCheck {
            name: "gdbus-codegen's own bytes, --annotate's value read as one word",
            why: "the defect itself: --annotate's real value name repeats WHAT KEY VALUE three \
                  times, and the tree keeps only the first word",
            expect: Expect::Fires(1),
            raw: GDBUS_CODEGEN_ANNOTATE_ROW.to_string(),
            root: node_with_flags(
                "gdbus-codegen",
                vec![flag_with_value("annotate", Some("WHAT"))],
            ),
        },
        SelfCheck {
            name: "gdbus-codegen's own bytes, --annotate's value already whole",
            why: "once the value name reads the full repeated run, the same table row must go \
                  silent",
            expect: Expect::Silent,
            raw: GDBUS_CODEGEN_ANNOTATE_ROW.to_string(),
            root: node_with_flags(
                "gdbus-codegen",
                vec![flag_with_value(
                    "annotate",
                    Some("WHAT KEY VALUE WHAT KEY VALUE WHAT KEY VALUE"),
                )],
            ),
        },
        SelfCheck {
            name: "a single-word value, a different shape entirely",
            why: "--interface-prefix PREFIX is one flag and one word — this rule only ever \
                  claims a row with two or more words after the flag",
            expect: Expect::Silent,
            raw: "  --interface-prefix PREFIX\n                        String to strip\n"
                .to_string(),
            root: node_with_flags(
                "gdbus-codegen",
                vec![flag_with_value("interface-prefix", Some("PREFIX"))],
            ),
        },
        SelfCheck {
            name: "a choices brace group, not a multi-word value",
            why: "`--c-generate-autocleanup {none,objects,all}` is one flag and one comma-glued \
                  token — this rule's own word-splitting sees a single word, not two or more",
            expect: Expect::Silent,
            raw: "  --c-generate-autocleanup {none,objects,all}\n                        Generate autocleanup support\n"
                .to_string(),
            root: node_with_flags(
                "gdbus-codegen",
                vec![flag_with_value("c-generate-autocleanup", Some("{none,objects,all}"))],
            ),
        },
        SelfCheck {
            name: "an ordinary description sharing the flag's own line",
            why: "a compact single-line row (`--output FILE   Write output into the specified \
                  file`) must stay silent — its description words are lower-case prose, never \
                  all upper-case metavars",
            expect: Expect::Silent,
            raw: "  --output FILE         Write output into the specified file\n".to_string(),
            root: node_with_flags("gdbus-codegen", vec![flag_with_value("output", Some("FILE"))]),
        },
    ]
}
