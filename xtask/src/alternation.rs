//! The `brace-alternation-flag` detector: the fourth fleet oracle, after
//! [`crate::misattribution`], [`crate::existence`] and [`crate::bundling`].
//!
//! **Its victim is a delimited alternation of flag spellings.** Three tools
//! in `audit/2.toml`'s seed-2 human review write their flags as an
//! alternation group, in three different renderings, and all three lost real
//! flags to it:
//!
//! | tool | as written | what reached the tree |
//! |---|---|---|
//! | `cache_restore` | `{-i\|--input} <input xml file>` | nothing — eight rows, zero flags |
//! | `eqn` | `{-v \| --version}` | `--version` carrying the literal value `"}"`; `-v` gone |
//! | `xfs_io` | `[[-c\|-C] cmd]...` | nothing — neither `-c` nor `-C` |
//!
//! The three look unalike on the page and are one shape underneath: a
//! `{...}` or `[...]` group whose members, split on `|`, are bare flag
//! spellings and nothing else. That is why they are one detector rather than
//! three, and it is the same judgment call spec §13.1e asks of every family
//! — a name that covered several shapes would have to be split, and this one
//! does not.
//!
//! # Why this needs its own oracle
//!
//! [`crate::existence`] is structurally blind to it in both directions.
//! `eqn`'s `--version` occurs literally in the raw text, so existence
//! attests it cleanly while the parse hangs a stray `}` on it as a required
//! value; and a spelling that never reached the tree at all is not something
//! an oracle asking "was this invented?" can see, because nothing was
//! invented. [`crate::bundling`] is blind too, and says so out loud: its
//! `xfs_io` fixture comment records that the alternation is "a different
//! family from the bundle (the members are separated by `|`, not glued), so
//! the cluster grammar neither helps nor hinders it".
//!
//! # The rule, and where it lives
//!
//! **Not here.** The predicate that decides whether a group names flags is
//! [`mandible_extract::help_text::parse_flag_alternation`] — the extractor's
//! own, imported rather than restated. This project has already paid for the
//! alternative: `xtask::misattribution` once carried a hand-copied
//! `pick_stream`, it drifted silently past a real fix, and the oracle
//! produced **200 of 656 fleet-wide fabrications** measuring its own
//! different guess instead of the parser. A detector meant to be ratcheted
//! at zero and a fix meant to reach zero have to agree, character for
//! character, on what the defect is; sharing the function is the only way to
//! guarantee that they do.
//!
//! What this module adds on top of that shared rule is the three decisions
//! that are the *detector's* own:
//!
//! 1. **At least [`MIN_ALTERNATIVES`] members.** An "alternation" of one is
//!    a bracketed optional flag (`[-v]`), which the synopsis path has always
//!    read correctly; counting it here would turn this into a general
//!    unparsed-flag detector wearing the wrong family's name.
//! 2. **Verbatim trees are skipped.** A tool whose help text produced no
//!    structure at all loses every spelling in it, alternation or not. That
//!    is `verbatim-fallback`, it has its own detector, and letting this one
//!    fire there would inflate a fleet count with tools whose alternation
//!    was never the reason for anything.
//! 3. **Two witnesses, both named in the family's own definition** — the
//!    family reads "*is dropped entirely or keeps a brace as its value*", so
//!    a spelling missing from the tree and a delimiter left inside a
//!    `value_name` are both reported, each with the group it came from.
//!
//! # What it deliberately does not catch
//!
//! **A member carrying its own value.** `sg_sanitize`'s `--count=OC|-c OC`
//! is an alternation of two spellings that *each* restate the value, and it
//! is labelled `value-name-mangled`, not this family. Nothing on its shape
//! says whether one value or two are meant, and the extractor's
//! `is_bare_flag_spelling` refuses it for that reason; this detector inherits
//! the refusal by construction rather than by a second decision.
//!
//! **Undelimited alternations.** `sg_sanitize`'s ordinary rows —
//! `--ause|-A`, `--block|-B` — are `|`-separated and *already parsed
//! correctly* (`skip_separators` treats `|` as an alias separator). A
//! detector that fired on them would be reporting working tools, which is
//! this project's standing rule inverted.
//!
//! The count is therefore a lower bound on the family, which is the right
//! direction for a number that becomes a gate: a false negative leaves a bug
//! unreported, a false positive blocks the fix.
//!
//! # No new probes
//!
//! Identical to the three oracles before it: this reads the raw bytes and
//! the tree a sweep already produced, so it costs zero additional subprocess
//! spawns.

use mandible_core::CommandNode;
use mandible_extract::help_text::parse_flag_alternation;
use std::collections::BTreeSet;

/// The fewest alternatives a group must offer before this detector reads it
/// as an alternation.
///
/// Two, and the reason is ownership rather than ambiguity: a one-member
/// group is `[-v]`, an ordinary bracketed optional flag that
/// `sections::usage_segments` has always handled, and every synopsis in the
/// fleet is full of them. Counting those would make this detector's number a
/// measure of unparsed flags generally — a real defect, with its own family
/// (`unparsed-flag`) and its own reasons — under a name that claims to be
/// about alternation.
pub(crate) const MIN_ALTERNATIVES: usize = 2;

/// Characters that mean a group delimiter, or the alternation bar itself,
/// survived into a stored `value_name` — the second half of the family's own
/// definition ("*or keeps a brace as its value*"). `eqn`'s `--version` came
/// out of `{-v | --version}` carrying exactly `"}"`.
const LEAKED_DELIMITERS: [char; 5] = ['{', '}', '[', ']', '|'];

/// One flag spelling an alternation group names that the tree does not have,
/// or one flag whose value spec kept a piece of the group's punctuation.
pub struct Finding {
    /// Space-separated path to the node concerned, e.g. `"xfs_io"`.
    pub path: String,
    /// The group as the tool's own text writes it, e.g. `"{-v | --version}"`.
    pub group: String,
    /// What went wrong, in the vocabulary of the family's two halves.
    pub detail: String,
}

/// The result of analyzing one tool.
pub struct AlternationReport {
    pub findings: Vec<Finding>,
}

impl AlternationReport {
    /// How many spellings-or-values this tool got wrong across every
    /// alternation group in its help text.
    pub fn finding_count(&self) -> usize {
        self.findings.len()
    }
}

/// One alternation group found in the raw text: the span as written, and the
/// bare spellings it names.
struct Group {
    text: String,
    members: Vec<String>,
}

/// Every alternation group of flag spellings in `raw`, in source order.
///
/// Scanned **per line** (an alternation group never spans one) and at every
/// delimiter position rather than only at token starts, because the anchor
/// case is nested: `[[-c|-C] cmd]` offers nothing at its outer bracket — the
/// outer group's single member is the string `[-c|-C] cmd`, which is not a
/// bare flag spelling — and everything at its inner one.
fn groups(raw: &str) -> Vec<Group> {
    let mut out = Vec::new();
    for line in raw.lines() {
        for (byte_pos, c) in line.char_indices() {
            if c != '[' && c != '{' {
                continue;
            }
            let Some(alt) = parse_flag_alternation(&line[byte_pos..]) else {
                continue;
            };
            if alt.members.len() < MIN_ALTERNATIVES {
                continue;
            }
            out.push(Group {
                text: alt.group,
                members: alt.members,
            });
        }
    }
    out
}

/// True when any flag anywhere in `node`'s tree carries `member`'s spelling.
///
/// `member` is always a bare spelling (`parse_flag_alternation` guarantees
/// it), so the question is a single field comparison: a long name for
/// `--input`, a short character for `-c`.
fn spelling_present(node: &CommandNode, member: &str) -> bool {
    let matches = |f: &mandible_core::Flag| {
        if let Some(name) = member.strip_prefix("--") {
            f.long.as_deref() == Some(name)
        } else if let Some(rest) = member.strip_prefix('-') {
            rest.chars().next().is_some_and(|c| f.short == Some(c))
        } else {
            false
        }
    };
    node.flags.iter().any(matches) || node.subcommands.iter().any(|c| spelling_present(c, member))
}

/// The flag anywhere in `node`'s tree that carries `member`'s spelling and a
/// `value_name` with a group delimiter still in it, if there is one.
fn leaked_value(node: &CommandNode, member: &str) -> Option<String> {
    for flag in &node.flags {
        let is_member = if let Some(name) = member.strip_prefix("--") {
            flag.long.as_deref() == Some(name)
        } else if let Some(rest) = member.strip_prefix('-') {
            rest.chars().next().is_some_and(|c| flag.short == Some(c))
        } else {
            false
        };
        if !is_member {
            continue;
        }
        if let Some(value) = &flag.value_name {
            if value.chars().any(|c| LEAKED_DELIMITERS.contains(&c)) {
                return Some(value.clone());
            }
        }
    }
    node.subcommands.iter().find_map(|c| leaked_value(c, member))
}

/// True when `node` and everything below it carries no flag, no positional
/// and no child — the `verbatim-fallback` state, which this detector steps
/// around (see the module doc comment, decision 2).
fn tree_is_structureless(node: &CommandNode) -> bool {
    node.flags.is_empty()
        && node.positionals.is_empty()
        && node.subcommands.is_empty()
        && node.subcommands.iter().all(tree_is_structureless)
}

/// Analyze `root`'s flags against `raw` (the same raw text
/// [`crate::misattribution::RecordingProbe::root_help_text`] hands back) for
/// the brace-alternation-flag family.
///
/// Same shape and same two arguments as [`crate::existence::detect`] and
/// [`crate::bundling::detect`], so all four oracles are interchangeable to a
/// caller running every one of them over a single capture.
pub fn detect(raw: &str, root: &CommandNode) -> AlternationReport {
    let mut findings = Vec::new();
    if !root.unparsed.is_empty() && tree_is_structureless(root) {
        return AlternationReport { findings };
    }
    let mut reported: BTreeSet<(String, String)> = BTreeSet::new();
    for group in groups(raw) {
        for member in &group.members {
            if !reported.insert((group.text.clone(), member.clone())) {
                continue;
            }
            if !spelling_present(root, member) {
                findings.push(Finding {
                    path: root.name.clone(),
                    group: group.text.clone(),
                    detail: format!("{member:?} reaches no flag in the tree"),
                });
                continue;
            }
            if let Some(value) = leaked_value(root, member) {
                findings.push(Finding {
                    path: root.name.clone(),
                    group: group.text.clone(),
                    detail: format!(
                        "{member:?} kept the group's own punctuation as its value ({value:?})"
                    ),
                });
            }
        }
    }
    AlternationReport { findings }
}

// ----------------------------------------------------------------------
// The hand-built evidence, promoted out of `#[cfg(test)]`
// ----------------------------------------------------------------------
//
// Same reason as `crate::bundling`'s identical block (spec §13.1e's "a fixed
// family inverts its own calibration"): once the commit repairing a family
// lands, its labelled set has nothing left to confirm against, and
// `calibrate` and `ratchet_at_zero` both need to tell "zero because the bug
// is gone" from "zero because the detector broke" at *runtime*, where no
// test harness exists. Neither will accept a zero without these.

use mandible_core::{Flag, Provenance, Source, ValueKind};

/// `cache_restore --help`, byte-exact from
/// `corpus/cache_restore/audit-seed2/help.txt` — every row of a real
/// `Options:` block written as a brace alternation.
const CACHE_RESTORE_HELP: &str = "Usage: cache_restore [options]\nOptions:\n  {-h|--help}\n  {-i|--input} <input xml file>\n  {-o|--output} <output device or file>\n  {-q|--quiet}\n  {--metadata-version} <1 or 2>\n  {-V|--version}\n\n  {--debug-override-metadata-version} <integer>\n  {--omit-clean-shutdown}\n";

/// `eqn --help`'s real second line, byte-exact from
/// `corpus/eqn/audit-seed2/help.txt`. The spaces around the `|` are what
/// split it into three bare tokens before the fix.
const EQN_HELP: &str = "usage: /usr/bin/eqn [-CNrR] [-d xy] [-f font] [-m n] [-M dir] [-p n] [-s n] [-T name] [file ...]\nusage: /usr/bin/eqn {-v | --version}\nusage: /usr/bin/eqn --help\n";

/// `xfs_io`'s real usage line, byte-exact from
/// `corpus/xfs_io/audit-seed2/help.stderr.txt` — the nested group, and the
/// anchor case of the whole family.
const XFS_IO_USAGE: &str =
    "Usage: xfs_io [-adfinrRstVx] [-m mode] [-p prog] [[-c|-C] cmd]... file\n";

/// `git --help`'s real four-way alternation. Every one of the four is a
/// genuine flag and the parser emits all four; a detector that read
/// "alternation" as "defect" would fire here, which is the false positive
/// that matters most.
const GIT_USAGE: &str =
    "usage: git [-v | --version] [-h | --help] [-C <path>] [-p | --paginate | -P | --no-pager]\n";

/// The value alternation that must never be read as flags. Its members are
/// not flag-shaped, so `parse_flag_alternation` refuses it outright — the
/// one condition the whole family's safety rests on.
const CHOICE_USAGE: &str = "usage: t [--color={always|never|auto}] [{start|stop}]\n";

fn flag(short: Option<char>, long: Option<&str>, value: Option<&str>, source: Source) -> Flag {
    let mut f = Flag::long("", Provenance::single(source));
    f.long = long.map(str::to_string);
    f.short = short;
    f.value_name = value.map(str::to_string);
    f.value_kind = if value.is_some() {
        ValueKind::Required
    } else {
        ValueKind::None
    };
    f
}

fn tree(name: &str, flags: Vec<Flag>) -> CommandNode {
    let mut root = CommandNode::new(name, Provenance::single(Source::HelpText));
    root.flags = flags;
    root
}

/// The hand-built cases this detector is willing to be judged on once its
/// labelled set has nothing left to say.
///
/// Both halves are present because
/// [`crate::detector::Calibration::self_checks_are_conclusive`] requires
/// both: a detector that fired on everything would satisfy every must-fire
/// case here, so the must-stay-silent cases are what make the evidence mean
/// anything.
pub(crate) fn self_checks() -> Vec<crate::detector::SelfCheck> {
    use crate::detector::{Expect, SelfCheck};

    vec![
        SelfCheck {
            name: "cache_restore's real braced options block",
            why: "five two-member rows against a tree with no flags at all, both spellings of \
                  each — the shape that reaches the grammar through an options table rather \
                  than a synopsis. Ten and not sixteen: this block's other three rows \
                  (`{--metadata-version}`, `{--omit-clean-shutdown}`, \
                  `{--debug-override-metadata-version}`) offer one alternative each and sit \
                  below MIN_ALTERNATIVES, so the declared floor is asserted here on real text \
                  as well as on the synthetic case below",
            expect: Expect::Fires(10),
            raw: CACHE_RESTORE_HELP.to_string(),
            root: tree("cache_restore", Vec::new()),
        },
        SelfCheck {
            name: "eqn's spaced brace alternation, both halves at once",
            why: "the only labelled case where the two witnesses coincide: `-v` reaches no flag \
                  and `--version` kept the group's own `}` as a required value",
            expect: Expect::Fires(2),
            raw: EQN_HELP.to_string(),
            root: tree(
                "eqn",
                vec![flag(None, Some("version"), Some("}"), Source::HelpTextSynopsis)],
            ),
        },
        SelfCheck {
            name: "xfs_io's nested alternation, the anchor case",
            why: "`[[-c|-C] cmd]` offers nothing at its outer bracket and everything at its \
                  inner one — the reason groups() scans every delimiter position rather than \
                  only token starts",
            expect: Expect::Fires(2),
            raw: XFS_IO_USAGE.to_string(),
            root: tree(
                "xfs_io",
                vec![
                    flag(Some('a'), None, None, Source::HelpTextSynopsis),
                    flag(Some('m'), None, Some("mode"), Source::HelpTextSynopsis),
                ],
            ),
        },
        SelfCheck {
            name: "git's real four-way alternation, correctly parsed",
            why: "the false-positive case that matters most: three real alternation groups on \
                  one line, every spelling in the tree, and a detector reading `alternation` as \
                  `defect` would fire on all of them",
            expect: Expect::Silent,
            raw: GIT_USAGE.to_string(),
            root: tree(
                "git",
                vec![
                    flag(Some('v'), Some("version"), None, Source::HelpTextSynopsis),
                    flag(Some('h'), Some("help"), None, Source::HelpTextSynopsis),
                    flag(Some('C'), None, Some("<path>"), Source::HelpTextSynopsis),
                    flag(Some('p'), Some("paginate"), None, Source::HelpTextSynopsis),
                    flag(Some('P'), Some("no-pager"), None, Source::HelpTextSynopsis),
                ],
            ),
        },
        SelfCheck {
            name: "a value alternation and a subcommand alternation",
            why: "`{always|never|auto}` and `{start|stop}` are the population this family would \
                  destroy if `is_bare_flag_spelling` ever loosened — neither names a flag, and \
                  `--color`'s value legitimately contains both a brace and a bar",
            expect: Expect::Silent,
            raw: CHOICE_USAGE.to_string(),
            root: tree(
                "t",
                vec![flag(
                    None,
                    Some("color"),
                    Some("{always|never|auto}"),
                    Source::HelpTextSynopsis,
                )],
            ),
        },
        SelfCheck {
            name: "a one-member group, the declared floor",
            why: "`[-v]` is an ordinary bracketed optional flag; MIN_ALTERNATIVES is what keeps \
                  this detector from becoming an unparsed-flag counter under the wrong family's \
                  name, asserted rather than merely described",
            expect: Expect::Silent,
            raw: "usage: t [-v] [-q] [-x]\n".to_string(),
            root: tree("t", Vec::new()),
        },
        SelfCheck {
            name: "a verbatim tree that happens to contain an alternation",
            why: "a tool the grammar made nothing of loses every spelling in its text; that is \
                  `verbatim-fallback`, it has its own detector, and this one must not claim it",
            expect: Expect::Silent,
            raw: "usage: t {-v | --version}\n".to_string(),
            root: {
                let mut root = CommandNode::new("t", Provenance::single(Source::HelpText));
                root.unparsed = vec![mandible_core::Text::sanitize("usage: t {-v | --version}")];
                root
            },
        },
        SelfCheck {
            name: "sg_sanitize's valued alternation, the declared out-of-family shape",
            why: "`--count=OC|-c OC` is labelled value-name-mangled, not this family: nothing on \
                  its shape says whether one value or two are meant, and the extractor's own \
                  member rule refuses it — asserted here so a loosening there cannot silently \
                  pull a second family into this count",
            expect: Expect::Silent,
            raw: "    --count=OC|-c OC     OC is overwrite count field\n".to_string(),
            root: tree(
                "sg_sanitize",
                vec![flag(None, Some("count"), Some("OC|-c"), Source::HelpText)],
            ),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every promoted case must hold, run through the same
    /// [`crate::detector::run_self_checks`] the calibration verdict and the
    /// ratchet gate use.
    #[test]
    fn every_promoted_self_check_case_holds() {
        let outcomes = crate::detector::run_self_checks(&crate::detector::BraceAlternationFlag);
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
    fn finds_the_nested_group_inside_an_outer_bracket() {
        let g = groups(XFS_IO_USAGE);
        let inner: Vec<&Group> = g.iter().filter(|g| g.text == "[-c|-C]").collect();
        assert_eq!(inner.len(), 1, "{:?}", g.iter().map(|g| &g.text).collect::<Vec<_>>());
        assert_eq!(inner[0].members, vec!["-c", "-C"]);
    }

    #[test]
    fn a_value_alternation_is_not_a_group() {
        assert!(groups("usage: t [--color={always|never|auto}]\n").is_empty());
        assert!(groups("usage: t {start|stop}\n").is_empty());
    }

    #[test]
    fn a_valued_member_is_not_a_group() {
        // `sg_sanitize`'s `--count=OC|-c OC`: the members restate the value,
        // which family is which is genuinely ambiguous, and the extractor's
        // own `is_bare_flag_spelling` refuses it. Asserted here so the two
        // families cannot merge by accident.
        assert!(groups("    [--count=OC|-c OC]\n").is_empty());
    }

    #[test]
    fn a_correctly_parsed_alternation_is_silent() {
        let r = detect(
            GIT_USAGE,
            &tree(
                "git",
                vec![
                    flag(Some('v'), Some("version"), None, Source::HelpTextSynopsis),
                    flag(Some('h'), Some("help"), None, Source::HelpTextSynopsis),
                    flag(Some('p'), Some("paginate"), None, Source::HelpTextSynopsis),
                    flag(Some('P'), Some("no-pager"), None, Source::HelpTextSynopsis),
                ],
            ),
        );
        assert_eq!(r.finding_count(), 0, "{:?}", r.findings[0].detail);
    }

    #[test]
    fn the_leaked_delimiter_witness_needs_the_flag_to_be_a_member() {
        // `--color`'s value legitimately contains `{`, `|` and `}`. It is not
        // a member of any flag alternation, so the value witness must not
        // look at it — that check alone is what separates a correct choice
        // list from `eqn`'s stray `}`.
        let r = detect(
            CHOICE_USAGE,
            &tree(
                "t",
                vec![flag(
                    None,
                    Some("color"),
                    Some("{always|never|auto}"),
                    Source::HelpTextSynopsis,
                )],
            ),
        );
        assert_eq!(r.finding_count(), 0);
    }

    #[test]
    fn each_group_member_is_reported_once_however_often_the_group_recurs() {
        let raw = "usage: t {-v | --version}\nusage: t {-v | --version}\n";
        let r = detect(raw, &tree("t", Vec::new()));
        assert_eq!(r.finding_count(), 2, "{:?}", r.findings.len());
    }
}
