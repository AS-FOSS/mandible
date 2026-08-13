//! The dropped-alias defect: a flag documented with **both** a short and a
//! long spelling reaches the tree carrying only one of them.
//!
//! The fourth fleet oracle, after [`crate::misattribution`] (is a
//! description attached to the right flag?), [`crate::existence`] (does this
//! spelling occur in the tool's own output at all?) and [`crate::bundling`]
//! (was a switch set read as one valued flag?).
//!
//! # The victim: an alias list interrupted by a value spec
//!
//! `help_text::grammar::parse_flag_spec` reads a flag-spec fragment as a run
//! of spellings followed by a value spec, and it stops reading spellings the
//! moment the value spec starts. That is correct for `-o, --output FILE`,
//! where the value really is last — and wrong for every framework that
//! repeats the placeholder after each spelling, which is what argparse does
//! by default:
//!
//! ```text
//!   -p PID, --pid PID  trace this PID only
//! ```
//!
//! `try_short` takes `-p`, and everything after it becomes one value token:
//! `PID,` — trailing comma and all — while `--pid` is discarded. The same
//! fragment written `-h, --help` (no value) parses perfectly, which is
//! exactly why the defect is invisible from a summary count: the tool's
//! *other* flags are fine.
//!
//! Five real examples from `audit/2.toml`'s seed-2 human review, all seven
//! of them labelled `dropped-alias`, five of them this module's:
//!
//! | tool | raw fragment | parsed as | alias lost |
//! |---|---|---|---|
//! | `filegone-bpfcc` | `-p PID, --pid PID` | `-p` + value `PID,` | `--pid` |
//! | `gethostlatency-bpfcc` | `-p PID, --pid PID` | `-p` + value `PID,` | `--pid` |
//! | `javaflow-bpfcc` | `-M METHOD, --method METHOD` | `-M` + value `METHOD,` | `--method` |
//! | `rubyobjnew-bpfcc` | `-C TOP_COUNT, --top-count TOP_COUNT` | `-C` + value `TOP_COUNT,` | `--top-count` |
//! | `sg_sanitize` | `--count=OC|-c OC` | `--count` + value `OC\|-c` | `-c` |
//!
//! # Two arms, because the grammar draws the value boundary in two places
//!
//! The defect has one cause and two surface shapes, and which one appears
//! depends only on whether the separator happened to be glued to the value
//! token or to sit beside it:
//!
//! * **Arm 1 — the separator follows the placeholder.** `-p PID, --pid PID`
//!   stores `PID,`: `take_rest_value_token` takes a run of non-whitespace,
//!   so the comma rode along. The alias sits in the raw text immediately
//!   after.
//! * **Arm 2 — the separator is inside the placeholder.**
//!   `--count=OC|-c OC` stores `OC|-c`: there is no whitespace anywhere in
//!   `OC|-c`, so the whole thing became the value and the alias is now a
//!   substring of a field that is supposed to be a placeholder.
//!
//! Both arms report the same finding and the fix closes both at once, but
//! they are separate code paths here because a detector that only knew one
//! of them would read zero on half the family and look healthy.
//!
//! # Why this needs its own oracle
//!
//! [`crate::existence`] is **structurally blind** to it, for the same reason
//! it is blind to [`crate::bundling`]: nothing was invented. Every spelling
//! in the tree occurs in the raw text, `-p` included. What is wrong is what
//! is *absent*, and an anti-fabrication oracle asks the opposite question.
//! [`crate::misattribution`] is blind too — the descriptions all land on the
//! right flags; it is the identity of the flag that is half-missing.
//!
//! # The hazard that governs this family, and how every condition serves it
//!
//! **Merging a short and a long spelling that are genuinely different flags
//! is a fabrication, and fabrication is strictly worse than dropping.**
//! Dropping loses information a user can still find in `--help`; merging
//! invents an alias the tool does not have, and a user who types it gets an
//! error. The fleet already has a live example of exactly this going wrong:
//! the existence oracle's standing baseline is dominated by mis-merged GCC
//! aliases.
//!
//! So every condition below is a rejection of a specific shape that *looks*
//! like an interrupted alias list and is not:
//!
//! 1. **The flag carries a value spec** ([`Flag::value_name`]). No value, no
//!    interruption — an alias list with nothing between its members already
//!    parses correctly, and a pair separated by anything *other* than a
//!    value spec is a different defect (see [`Scope`] below).
//! 2. **The kept spelling and its value occur, glued as the tree stores
//!    them, in the raw text** ([`anchor_end`]). `value_name` is stored
//!    verbatim, so a successful match means the reconstructed fragment *is*
//!    the tool's own text rather than merely something like it — the same
//!    reconstruct-and-search condition [`crate::bundling`] leans on.
//! 3. **An alias separator (`,` or `|`) sits at the placeholder's own
//!    boundary** — as its last character, as the next non-space character
//!    after it, or as the last separator inside it. Bounded there
//!    deliberately: scanning any further reaches the description column,
//!    where `--format FMT   same as --output` would read as an alias list
//!    and is prose.
//! 4. **What follows that separator is a whole flag spelling and nothing
//!    else** ([`whole_spelling`]). `{a,-b}` — a choice list whose member
//!    happens to start with a dash — leaves `-b}` after its separator, which
//!    is not a spelling, and is left alone. So is `jdeprscan`'s real
//!    `--release 7|8|9|10|11|12|13|14|15|16|17`, whose pipes are followed by
//!    digits.
//! 5. **That spelling is nowhere in the tree** ([`spellings_in_tree`]).
//!    This is what makes the finding a *drop* rather than an observation:
//!    a tool that documents `-p PID, --pid PID` and whose tree already
//!    carries `--pid` has nothing wrong with it, and this condition is the
//!    one that goes false the instant the family is repaired.
//!
//! # What it deliberately does not catch
//!
//! Two of the seven labelled tools, both named in [`Detector::scope`] with a
//! structural [`crate::detector::Ground`] rather than left to look like
//! recall:
//!
//! * **`eqn`'s `{-v | --version}`** — the alias separator is inside a brace
//!   alternation group, which the manifest labels as its own family
//!   (`brace-alternation-flag`). Nothing about it is an interrupted value
//!   spec; the synopsis tokenizer never opened the group at all.
//! * **`jdeprscan`'s `-l    --list`** — the two spellings are separated by a
//!   four-space run, which is the layout splitter's own description-column
//!   boundary. The two never arrive at the flag-spec grammar as one
//!   fragment, so there is no fragment for this rule to read.
//!
//! A third shape is out of reach of the *data model* rather than of this
//! rule: `jdeprscan`'s `-? -h --help` names two shorts and
//! [`mandible_core::Flag`] holds one `short: Option<char>`, exactly as
//! `-A, --catenate, --concatenate` names two longs and it holds one `long`.
//! Nothing can be reported there that a wider `Flag` would not have to fix
//! first.

use crate::existence::is_word_char;
use mandible_core::{CommandNode, Flag, Provenance, Source, ValueKind};
use std::collections::BTreeSet;

/// The two characters a help formatter uses to separate the spellings of
/// one flag: `,` (argparse, GNU getopt_long, tar) and `|` (the `sg_*`
/// family, and every synopsis alternation).
///
/// Closed at two on purpose. Whitespace is *not* here and must not be: a
/// bare space between two spellings is already handled by the grammar's own
/// `skip_separators`, and a *wide* run of spaces is the description column
/// (see this module's doc comment on `jdeprscan`). Admitting whitespace
/// would turn every `--foo BAR --baz` in a synopsis into an alias claim.
const ALIAS_SEPARATORS: [char; 2] = [',', '|'];

/// One flag whose documented alias never reached the tree.
pub struct DroppedAlias {
    /// Space-separated path to the node owning the flag, e.g.
    /// `"filegone-bpfcc"`.
    pub path: String,
    /// The spelling that did reach the tree, e.g. `"-p"`.
    pub kept: String,
    /// The spelling documented beside it that did not, e.g. `"--pid"`.
    pub dropped: String,
    /// The tool's own text this was read from, reconstructed from the
    /// fields the tree stores — e.g. `"-p PID, --pid"`. Printed in every
    /// report so a finding is checkable against the capture by hand.
    pub witness: String,
}

/// The result of analyzing one tool.
pub struct AliasReport {
    pub drops: Vec<DroppedAlias>,
}

impl AliasReport {
    /// How many documented spellings this tool lost.
    pub fn drop_count(&self) -> usize {
        self.drops.len()
    }
}

/// `token` as a whole flag spelling and nothing else — `--long-name` or
/// `-x` — or `None`.
///
/// **The load-bearing rejection** (this module's condition 4). It is not
/// enough that a dash follows the separator: a choice list's member
/// (`{a,-b}` leaves `-b}`), a negative number in a range, and a stray
/// hyphen in prose all start with one. Requiring the token to *be* a
/// spelling, exactly, with nothing trailing it, is what keeps this
/// detector from claiming an alias where the raw text names a value.
fn whole_spelling(token: &str) -> Option<String> {
    if let Some(name) = token.strip_prefix("--") {
        let ok = !name.is_empty()
            && name.chars().all(|c| c.is_alphanumeric() || c == '-')
            && !name.ends_with('-');
        return ok.then(|| format!("--{name}"));
    }
    let mut chars = token.strip_prefix('-')?.chars();
    let c = chars.next()?;
    (chars.next().is_none() && c.is_alphanumeric()).then(|| format!("-{c}"))
}

/// The flag spelling documented right after an alias separator, reading
/// `after` (the raw text following the separator).
///
/// Stops at a newline: only spaces are skipped, so a spelling on the *next*
/// line can never be read as this flag's alias. A value spec glued onto the
/// alias (`--pid=PID`, `--pid[=PID]`) is trimmed off before the spelling is
/// judged, because the alias's own value shape says nothing about whether
/// the alias exists.
fn alias_after_separator(after: &str) -> Option<String> {
    let after = after.trim_start_matches(' ');
    let word: String = after.chars().take_while(|c| !c.is_whitespace()).collect();
    let spelling = word.split(['=', '[']).next()?;
    whole_spelling(spelling)
}

/// The alias `value` and the raw text after it document, when one is there.
///
/// The two arms of this module's doc comment, in order. Arm 2 is tried
/// first because a placeholder that *ends* in a separator (`PID,`) also
/// contains one, and only arm 1 can read what follows it.
fn documented_alias(value: &str, tail: &str) -> Option<String> {
    // Arm 2: the separator was swallowed into the placeholder, so the
    // alias is now a substring of it. It must run to the placeholder's very
    // end — `OC|-c` does, `{a,-b}` does not.
    if let Some(pos) = value.rfind(ALIAS_SEPARATORS) {
        let separator_len = value[pos..].chars().next()?.len_utf8();
        if let Some(spelling) = whole_spelling(&value[pos + separator_len..]) {
            return Some(spelling);
        }
    }
    // Arm 1: the placeholder ended at the separator, or the separator is
    // the very next non-space character after it. Nothing beyond that is
    // examined — see condition 3 on why the bound is where it is.
    if value.ends_with(ALIAS_SEPARATORS) {
        return alias_after_separator(tail);
    }
    let next = tail.trim_start_matches(' ');
    let separator = next
        .chars()
        .next()
        .filter(|c| ALIAS_SEPARATORS.contains(c))?;
    alias_after_separator(&next[separator.len_utf8()..])
}

/// The char index in `raw` just past the first occurrence of `needle` that
/// is not preceded by a word character.
///
/// Char-indexed throughout (AGENTS.md's rule against slicing captured tool
/// output at a raw byte offset), and it shares
/// [`crate::existence::is_word_char`] rather than carrying a second copy of
/// the boundary rule — the drift hazard `existence`'s own doc comment
/// names.
///
/// **Only the left boundary is enforced**, unlike
/// [`crate::existence::spelling_occurs`]. The needle here already ends in
/// the tool's own value placeholder (or in the separator glued to it), so
/// there is no prefix-of-a-longer-flag hazard on the right — and demanding
/// a non-word character there would reject `-p PID,--pid` written with no
/// space, which is the same defect with tighter typography.
fn anchor_end(raw: &[char], needle: &str) -> Option<usize> {
    let needle: Vec<char> = needle.chars().collect();
    if needle.is_empty() || raw.len() < needle.len() {
        return None;
    }
    (0..=(raw.len() - needle.len())).find_map(|start| {
        let matches = raw[start..start + needle.len()] == needle[..];
        let boundary_ok = start == 0 || !is_word_char(raw[start - 1]);
        (matches && boundary_ok).then_some(start + needle.len())
    })
}

/// Every flag spelling anywhere in the tree, as written (`-p`, `--pid`).
///
/// The whole tree, not just the owning node: a spelling that reached *some*
/// node is evidence the grammar can read it, and reporting it as dropped on
/// a sibling would be a finding about node placement, which is a different
/// question. Conservative in the direction this project requires — it can
/// only ever suppress a report, never manufacture one.
fn spellings_in_tree(node: &CommandNode, out: &mut BTreeSet<String>) {
    for flag in &node.flags {
        if let Some(c) = flag.short {
            out.insert(format!("-{c}"));
        }
        if let Some(long) = &flag.long {
            out.insert(format!("--{long}"));
        }
    }
    for child in &node.subcommands {
        spellings_in_tree(child, out);
    }
}

/// The kept spellings this flag could have been anchored by, longest first
/// — a long name is more specific than a single character, so matching it
/// first makes the anchor claim as exact as the tree allows.
fn kept_spellings(flag: &Flag) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(long) = &flag.long {
        out.push(format!("--{long}"));
    }
    if let Some(c) = flag.short {
        out.push(format!("-{c}"));
    }
    out
}

/// The alias `flag` documents but does not carry, with the reconstructed
/// witness — `None` when any condition in this module's doc comment fails.
///
/// Split out from [`detect`]'s walk so the five conditions read in one
/// place and can be exercised one at a time.
fn dropped_alias(flag: &Flag, raw: &[char], known: &BTreeSet<String>) -> Option<(String, String)> {
    // 1. A value spec is what interrupts an alias list; without one there
    //    is nothing for this rule to be about.
    let value = flag.value_name.as_deref()?;
    if value.is_empty() {
        return None;
    }
    for kept in kept_spellings(flag) {
        // 2. The kept spelling and its placeholder occur, glued as the tree
        //    stores them, in the tool's own text. All three joiners the
        //    grammar accepts are tried: `--count=OC`, `-p PID`, `-pPID`.
        for joiner in ["=", " ", ""] {
            let anchor = format!("{kept}{joiner}{value}");
            let Some(end) = anchor_end(raw, &anchor) else {
                continue;
            };
            let tail: String = raw[end..].iter().collect();
            // 3 and 4.
            let Some(dropped) = documented_alias(value, &tail) else {
                continue;
            };
            // 5. The alias is genuinely absent. This is the condition that
            //    goes false the moment the family is repaired, which is
            //    what makes a fleet count of zero mean anything.
            if known.contains(&dropped) {
                continue;
            }
            return Some((kept, format!("{anchor} {dropped}")));
        }
    }
    None
}

fn walk(
    node: &CommandNode,
    path: &str,
    raw: &[char],
    known: &BTreeSet<String>,
    out: &mut Vec<DroppedAlias>,
) {
    for flag in &node.flags {
        if let Some((kept, witness)) = dropped_alias(flag, raw, known) {
            let dropped = witness.rsplit(' ').next().unwrap_or_default().to_string();
            out.push(DroppedAlias {
                path: path.to_string(),
                kept,
                dropped,
                witness,
            });
        }
    }
    for child in &node.subcommands {
        let child_path = format!("{path} {}", child.name);
        walk(child, &child_path, raw, known, out);
    }
}

/// Analyze `root`'s flags against `raw` (the same raw `--help`/`-h` text
/// [`crate::misattribution::RecordingProbe::root_help_text`] hands back)
/// for the dropped-alias defect.
///
/// Same shape and same two arguments as [`crate::existence::detect`] and
/// [`crate::bundling::detect`], so all four oracles are interchangeable to
/// a caller that wants to run every one of them over a single capture.
pub fn detect(raw: &str, root: &CommandNode) -> AliasReport {
    let mut known = BTreeSet::new();
    spellings_in_tree(root, &mut known);
    let chars: Vec<char> = raw.chars().collect();
    let mut drops = Vec::new();
    walk(root, &root.name, &chars, &known, &mut drops);
    AliasReport { drops }
}

// ----------------------------------------------------------------------
// The hand-built evidence, promoted out of `#[cfg(test)]`
// ----------------------------------------------------------------------
//
// Same rationale as `crate::bundling`'s own promoted block: two consumers
// outside the test binary depend on these cases (spec §13.1e, "a fixed
// family inverts its own calibration") — `crate::detector::calibrate`,
// before it will report REPAIRED rather than broken, and
// `crate::detector::ratchet_at_zero`, before it will accept a fleet count
// of zero, since `count == 0` on its own is satisfied by deleting this
// module.

/// A help-text-sourced flag as `sections::emit_flags` builds it: the
/// spellings the grammar recovered, the placeholder it stored verbatim, and
/// the description column beside it.
fn row_flag(short: Option<char>, long: Option<&str>, value: Option<&str>) -> Flag {
    let mut flag = Flag::long("", Provenance::single(Source::HelpText));
    flag.long = long.map(str::to_string);
    flag.short = short;
    flag.value_name = value.map(str::to_string);
    flag.value_kind = if value.is_some() {
        ValueKind::Required
    } else {
        ValueKind::None
    };
    flag
}

/// A one-node tree named `name` carrying `flags`.
fn tree(name: &str, flags: Vec<Flag>) -> CommandNode {
    let mut root = CommandNode::new(name, Provenance::single(Source::HelpText));
    root.flags = flags;
    root
}

// --- the known tools, byte-exact from their corpus captures -------------

/// `filegone-bpfcc --help`'s real options block, byte-exact from
/// `corpus/filegone-bpfcc/audit-seed2/help.txt`. The plainest instance of
/// the family there is: the `-h, --help` row directly above it parses
/// perfectly, and only the placeholder separates them.
const FILEGONE_OPTIONS: &str = "options:\n  -h, --help         show this help message and exit\n  -p PID, --pid PID  trace this PID only\n";

/// `javaflow-bpfcc`'s real choice-list row, byte-exact. The commas *inside*
/// `{java,perl,...}` are the reason condition 4 exists: a rule that stopped
/// at the first separator would read the alias as `perl`.
const JAVAFLOW_CHOICE_ROW: &str = "  -l {java,perl,php,python,ruby,tcl}, --language {java,perl,php,python,ruby,tcl}\n                        language to trace\n";

/// `sg_sanitize`'s real pipe-separated row, byte-exact from
/// `corpus/sg_sanitize/audit-seed2/help.stderr.txt` — arm 2, where the
/// separator and the whole alias were swallowed into the placeholder.
const SG_SANITIZE_COUNT_ROW: &str =
    "    --count=OC|-c OC     OC is overwrite count field (from 1 (def) to 31)\n";

/// `jdeprscan`'s real pipe-separated *value*, byte-exact — the closest real
/// thing to a false positive this detector has, and the reason condition 4
/// asks what follows a separator rather than merely that one is there.
const JDEPRSCAN_RELEASE_ROW: &str = "        --release 7|8|9|10|11|12|13|14|15|16|17\n";

/// `jdeprscan`'s real `-l    --list` row, byte-exact — the witness the
/// declared out-of-scope exclusion cites, carried here rather than retyped
/// in `crate::detector` so the row and the cases that exercise it cannot
/// drift apart.
pub(crate) const JDEPRSCAN_LIST_ROW: &str = "  -l    --list";

/// `eqn`'s real brace alternation, byte-exact from
/// `corpus/eqn/audit-seed2/help.txt` — the other declared exclusion's
/// witness, and the reviewer's own anchor case for this family.
pub(crate) const EQN_VERSION_GROUP: &str = "{-v | --version}";

/// The hand-built cases this detector is willing to be judged on once the
/// labelled set has nothing left to say.
///
/// Both halves are present on purpose and
/// [`crate::detector::Calibration::self_checks_are_conclusive`] requires
/// both. The must-stay-silent half is where this family's real risk lives:
/// the repaired shape of every must-fire case is here too, because a
/// detector that kept firing after the fix would ratchet a repaired family
/// to failure, and one that fired on `--release 7|8|9` would be claiming an
/// alias the tool does not have — the fabrication this whole module is
/// written against.
pub(crate) fn self_checks() -> Vec<crate::detector::SelfCheck> {
    use crate::detector::{Expect, SelfCheck};

    vec![
        SelfCheck {
            name: "filegone-bpfcc's real argparse row",
            why: "arm 1, the plainest instance: `-p PID, --pid PID` stores `PID,` and drops \
                  `--pid`, while the `-h, --help` row right above it parses perfectly",
            expect: Expect::Fires(1),
            raw: FILEGONE_OPTIONS.to_string(),
            root: tree(
                "filegone-bpfcc",
                vec![
                    row_flag(Some('h'), Some("help"), None),
                    row_flag(Some('p'), None, Some("PID,")),
                ],
            ),
        },
        SelfCheck {
            name: "javaflow-bpfcc's real choice-list row",
            why: "arm 1 with six commas inside the placeholder: only condition 4 (the text after \
                  a separator must be a whole spelling) keeps this from reading `perl` as the \
                  alias",
            expect: Expect::Fires(1),
            raw: JAVAFLOW_CHOICE_ROW.to_string(),
            root: tree(
                "javaflow-bpfcc",
                vec![row_flag(
                    Some('l'),
                    None,
                    Some("{java,perl,php,python,ruby,tcl},"),
                )],
            ),
        },
        SelfCheck {
            name: "sg_sanitize's real pipe-separated row",
            why: "arm 2: the separator and the whole alias were swallowed into the placeholder, \
                  so the finding is a substring of `value_name` rather than text beside it",
            expect: Expect::Fires(1),
            raw: SG_SANITIZE_COUNT_ROW.to_string(),
            root: tree(
                "sg_sanitize",
                vec![row_flag(None, Some("count"), Some("OC|-c"))],
            ),
        },
        SelfCheck {
            name: "the same argparse row, repaired",
            why: "the case the ratchet rests on: identical bytes, and the tree now carries \
                  `--pid`. Condition 5 is the only thing that changed, and it is what tells a \
                  repaired family apart from a deleted detector",
            expect: Expect::Silent,
            raw: FILEGONE_OPTIONS.to_string(),
            root: tree(
                "filegone-bpfcc",
                vec![
                    row_flag(Some('h'), Some("help"), None),
                    row_flag(Some('p'), Some("pid"), Some("PID")),
                ],
            ),
        },
        SelfCheck {
            name: "sg_sanitize's row, repaired",
            why: "arm 2's repaired shape — the placeholder is `OC` and the tree carries `-c`, so \
                  the swallowed separator is gone from `value_name` as well as from the finding",
            expect: Expect::Silent,
            raw: SG_SANITIZE_COUNT_ROW.to_string(),
            root: tree(
                "sg_sanitize",
                vec![row_flag(Some('c'), Some("count"), Some("OC"))],
            ),
        },
        SelfCheck {
            name: "jdeprscan's real pipe-separated value",
            why: "the closest real thing to a false positive: eleven `|` separators in one \
                  placeholder, every one of them followed by digits rather than by a spelling",
            expect: Expect::Silent,
            raw: JDEPRSCAN_RELEASE_ROW.to_string(),
            root: tree(
                "jdeprscan",
                vec![row_flag(
                    None,
                    Some("release"),
                    Some("7|8|9|10|11|12|13|14|15|16|17"),
                )],
            ),
        },
        SelfCheck {
            name: "a choice list whose member starts with a dash",
            why: "`{a,-b}` puts a dash straight after a separator and is a value, not an alias — \
                  only the requirement that a spelling run to the placeholder's end rejects it",
            expect: Expect::Silent,
            raw: "  -s {a,-b}   set the sign\n".to_string(),
            root: tree("t", vec![row_flag(Some('s'), None, Some("{a,-b}"))]),
        },
        SelfCheck {
            name: "a description that mentions another flag",
            why: "the bound in condition 3, asserted: `same as --output` sits past the \
                  description column, and a rule that scanned the whole line would claim it as \
                  an alias",
            expect: Expect::Silent,
            raw: "  --format FMT   same as --output, --out\n".to_string(),
            root: tree("t", vec![row_flag(None, Some("format"), Some("FMT"))]),
        },
        SelfCheck {
            name: "an ordinary alias pair that already parses",
            why: "`-o, --output FILE` is the shape the grammar has always read correctly — the \
                  value is last, so nothing was interrupted and nothing may be reported",
            expect: Expect::Silent,
            raw: "  -o, --output FILE   write output to FILE\n".to_string(),
            root: tree("t", vec![row_flag(Some('o'), Some("output"), Some("FILE"))]),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every promoted case must hold, run through the same
    /// [`crate::detector::run_self_checks`] the calibration verdict and the
    /// ratchet gate use — so a case that stops holding fails the test suite
    /// too, not only the two runtime consumers.
    #[test]
    fn every_promoted_self_check_case_holds() {
        let outcomes = crate::detector::run_self_checks(&crate::detector::DroppedAliasDetector);
        assert!(!outcomes.is_empty());
        for outcome in &outcomes {
            assert!(
                outcome.held,
                "{}: expected {:?}, got {} hit(s): {:?}",
                outcome.name,
                outcome.expect,
                outcome.hits.len(),
                outcome.hits
            );
        }
        assert!(crate::detector::self_checks_are_conclusive(&outcomes));
    }

    #[test]
    fn reports_the_argparse_row_by_both_spellings() {
        let report = detect(
            FILEGONE_OPTIONS,
            &tree(
                "filegone-bpfcc",
                vec![
                    row_flag(Some('h'), Some("help"), None),
                    row_flag(Some('p'), None, Some("PID,")),
                ],
            ),
        );
        assert_eq!(report.drop_count(), 1);
        assert_eq!(report.drops[0].kept, "-p");
        assert_eq!(report.drops[0].dropped, "--pid");
        assert_eq!(report.drops[0].witness, "-p PID, --pid");
    }

    #[test]
    fn reports_the_swallowed_pipe_alias() {
        let report = detect(
            SG_SANITIZE_COUNT_ROW,
            &tree(
                "sg_sanitize",
                vec![row_flag(None, Some("count"), Some("OC|-c"))],
            ),
        );
        assert_eq!(report.drop_count(), 1);
        assert_eq!(report.drops[0].kept, "--count");
        assert_eq!(report.drops[0].dropped, "-c");
    }

    /// The condition that makes a fleet zero mean something: identical
    /// bytes, and the only change is that the tree now carries the alias.
    #[test]
    fn an_alias_already_in_the_tree_is_never_reported() {
        let report = detect(
            FILEGONE_OPTIONS,
            &tree(
                "filegone-bpfcc",
                vec![row_flag(Some('p'), Some("pid"), Some("PID"))],
            ),
        );
        assert_eq!(report.drop_count(), 0);
    }

    /// A separator is not evidence on its own — what follows it is.
    #[test]
    fn a_pipe_separated_value_stays_silent() {
        for value in [
            "7|8|9|10|11|12|13|14|15|16|17", // jdeprscan --release
            "json|yaml|table",
            "{a,-b}",
            "{-1,0,1}",
            "always|never|auto",
        ] {
            let raw = format!("  --opt {value}   pick one\n");
            let report = detect(
                &raw,
                &tree("t", vec![row_flag(None, Some("opt"), Some(value))]),
            );
            assert_eq!(report.drop_count(), 0, "{value} must not fire");
        }
    }

    /// The bound of condition 3: prose past the description column names
    /// other flags all the time, and none of them is an alias.
    #[test]
    fn a_flag_named_in_a_description_is_never_an_alias() {
        let raw = "  --format FMT   same as --output, --out\n";
        let report = detect(
            raw,
            &tree("t", vec![row_flag(None, Some("format"), Some("FMT"))]),
        );
        assert_eq!(report.drop_count(), 0);
    }

    /// A flag with no value spec has no interrupted alias list, whatever
    /// else the row says — the family's other shapes are declared out of
    /// scope rather than guessed at.
    #[test]
    fn a_valueless_flag_is_never_reported() {
        let raw = "  -l    --list   list deprecated APIs\n";
        let report = detect(
            raw,
            &tree("jdeprscan", vec![row_flag(Some('l'), None, None)]),
        );
        assert_eq!(report.drop_count(), 0);
    }

    /// The anchor has to be the tool's own text, not merely fields that
    /// could have come from it.
    #[test]
    fn a_flag_whose_anchor_is_absent_from_the_raw_text_stays_silent() {
        let raw = "  -q QUIET   be quiet\n";
        let report = detect(
            raw,
            &tree("t", vec![row_flag(Some('p'), None, Some("PID,"))]),
        );
        assert_eq!(report.drop_count(), 0);
    }

    /// An alias on the *next* line is not this flag's alias: only spaces
    /// are skipped after the separator.
    #[test]
    fn a_spelling_on_the_following_line_is_never_an_alias() {
        let raw = "  -p PID,\n  --pid PID   trace this PID\n";
        let report = detect(
            raw,
            &tree("t", vec![row_flag(Some('p'), None, Some("PID,"))]),
        );
        assert_eq!(report.drop_count(), 0);
    }

    #[test]
    fn whole_spelling_accepts_only_a_bare_spelling() {
        assert_eq!(whole_spelling("--pid").as_deref(), Some("--pid"));
        assert_eq!(whole_spelling("-c").as_deref(), Some("-c"));
        assert_eq!(
            whole_spelling("--top-count").as_deref(),
            Some("--top-count")
        );
        for token in ["-b}", "--", "-", "", "perl", "-cd", "--foo=BAR", "17"] {
            assert!(
                whole_spelling(token).is_none(),
                "{token:?} is not a spelling"
            );
        }
    }
}
