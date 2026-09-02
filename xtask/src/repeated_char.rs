//! The repeated-character flag misread: `-vv` read as `-v` carrying a
//! required value spelled `"v"`. The second of the three families sharing
//! the `short && !long && value_name` fingerprint (S-035;
//! [`crate::bundling`]'s doc comment has the family table). `bpftrace`'s
//! option table documents `-v`/`-vv` and `-d`/`-dd` as separate real
//! flags; the tree only kept the boolean forms.
//!
//! Found by reading a flag that's present and mis-shaped, not by
//! searching for one that's absent.
//!
//! A flag is reported when all hold: short spelling, no long name,
//! `Required` value; the swallowed value is the flag's own character
//! repeated ([`value_repeats_short`] — the family's whole identity, and
//! what keeps the three families provably disjoint: a bundle is a set so
//! can't satisfy it, a long option's tail is a word so can't either); the
//! reconstructed token occurs glued and delimited in the raw text
//! ([`crate::existence::spelling_occurs`]); the tool's own document
//! declares the bare short flag a boolean ([`documents_bare_boolean`]).
//!
//! That last condition is the safety argument: `lessecho`'s real `[-nn]`
//! (a genuine "-n followed by a number" flag) satisfies every other
//! condition and is indistinguishable by token shape alone, but
//! `lessecho` never documents a bare `-n`, while `bpftrace` documents
//! both `-v` and `-vv` with different descriptions — a tool documenting
//! `-v` as taking no value has said `-vv` cannot be `-v` carrying a
//! value. The fix (`help_text::sections::repair_repeated_character_flags`)
//! is written against the identical condition.
//!
//! Does not catch a repeated-character flag whose bare form the tool
//! never documents (`strace`'s `[-DDD]`, `wpa_supplicant`'s
//! `[-BddhKLqqstuvW]`) — token shape alone can't tell those from
//! `lessecho`'s `[-nn]`. Count is a lower bound.

use crate::existence::spelling_occurs;
use mandible_core::{CommandNode, Entity, Provenance, Source, ValueKind};

/// True when `value` is one or more copies of `short` and nothing else.
///
/// `-vv` stores `value_name: "v"`, `-vvv` stores `"vv"`, `strace`'s `[-DDD]`
/// stores `"DD"` — one, two and two copies of the flag's own character. The
/// emptiness guard matters: an empty value is `ValueKind::Required` with
/// nothing in it, which `chars().all(..)` would call vacuously true.
///
/// Case-sensitive, like every other spelling comparison in this project:
/// `-v` and `-V` are different flags, so `-vV` is not this family (it is a
/// two-member bundle, and [`crate::bundling`]'s business).
fn value_repeats_short(short: char, value: &str) -> bool {
    !value.is_empty() && value.chars().all(|c| c == short)
}

/// True when some flag in `flags` is the bare boolean spelling of `short` —
/// the same short character, taking no value at all.
///
/// The tool's own statement that `-v` is a switch, which is what makes `-vv`
/// unreadable as "`-v` with a value". See this module's doc comment: this is
/// the condition that keeps `lessecho`'s genuine `[-nn]` out.
///
/// A long alias is not disqualifying — `-V, --version` is still a boolean —
/// so only the short spelling and the value kind are compared.
fn documents_bare_boolean(flags: &[Entity], short: char) -> bool {
    flags
        .iter()
        .any(|f| f.short() == Some(short) && f.value_kind == ValueKind::None)
}

/// One repeated-character flag read as its own first character plus a value.
pub struct Misread {
    /// Space-separated path to the node owning the flag, e.g. `"killsnoop.bt"`.
    pub path: String,
    /// The surviving flag's spelling, e.g. `"-v"` — which is a real flag of
    /// this tool in its own right, and is exactly why the defect is invisible
    /// to [`crate::existence`]: `-v` occurs in the raw text, attested, at a
    /// clean boundary, and the parse is still wrong.
    pub spelling: String,
    /// The real token this misread, e.g. `"-vv"` — the spelling a user would
    /// type and cannot find anywhere in the tree.
    pub token: String,
}

/// The result of analyzing one tool.
pub struct RepeatReport {
    pub misreads: Vec<Misread>,
}

impl RepeatReport {
    /// How many repeated-character flags this tool misread. Unlike
    /// [`crate::bundling::BundleReport`] there is no second "destroyed"
    /// count to keep beside it: this defect loses exactly one real flag per
    /// misread — the repeated spelling itself — while the bare flag it
    /// collides with survives correctly on its own row.
    pub fn misread_count(&self) -> usize {
        self.misreads.len()
    }
}

/// Whether `flag` is a repeated-character flag misread against `siblings`
/// (the flags of the node it belongs to) and `raw`, and the real token it
/// misread — `None` when any condition in this module's doc comment fails.
fn misread_token(flag: &Entity, siblings: &[Entity], raw: &str) -> Option<String> {
    // 1. A bare short flag carrying a required value.
    let short = flag.short()?;
    if flag.long().is_some() || flag.value_kind != ValueKind::Required {
        return None;
    }
    let value = flag.value_name.as_deref()?;
    // 2. The value is this flag's own character, repeated.
    if !value_repeats_short(short, value) {
        return None;
    }
    // 4. The tool documents the bare spelling as a boolean. Before the text
    //    scan because it is the cheaper of the two remaining conditions and
    //    the more selective.
    if !documents_bare_boolean(siblings, short) {
        return None;
    }
    // 3. The token occurs, glued and delimited, in the raw text.
    let token = format!("-{short}{value}");
    if !spelling_occurs(raw, &token) {
        return None;
    }
    Some(token)
}

fn walk(node: &CommandNode, path: &str, raw: &str, out: &mut Vec<Misread>) {
    let siblings: Vec<Entity> = node.flags().cloned().collect();
    for flag in node.flags() {
        let Some(token) = misread_token(flag, &siblings, raw) else {
            continue;
        };
        let Some(short) = flag.short() else {
            continue;
        };
        out.push(Misread {
            path: path.to_string(),
            spelling: format!("-{short}"),
            token,
        });
    }
    for child in &node.subcommands {
        let child_path = format!("{path} {}", child.name);
        walk(child, &child_path, raw, out);
    }
}

/// Analyze `root`'s flags against `raw` for the repeated-character misread.
///
/// Same shape and same two arguments as [`crate::bundling::detect`] and
/// [`crate::existence::detect`], so all three are interchangeable to a caller
/// running every oracle over one capture.
pub fn detect(raw: &str, root: &CommandNode) -> RepeatReport {
    let mut misreads = Vec::new();
    walk(root, &root.name, raw, &mut misreads);
    RepeatReport { misreads }
}

// ----------------------------------------------------------------------
// The hand-built evidence, promoted out of `#[cfg(test)]`
// ----------------------------------------------------------------------
//
// Same arrangement, and for the same two runtime consumers, as
// `crate::bundling`'s promoted block: `crate::detector::calibrate` needs
// these before it will call the family REPAIRED, and
// `crate::detector::ratchet_at_zero` needs them before it will accept a
// fleet count of zero, because `count == 0` on its own is satisfied by
// deleting the detector. Neither runs under the test harness, so a
// `#[cfg(test)]` assertion cannot serve either.

/// A flag as `sections::emit_flags` builds one from an option-table row:
/// short spelling, no long name, whatever value the grammar read, and the
/// plain [`Source::HelpText`] provenance a described row carries.
fn table_flag(short: char, value: Option<&str>) -> Entity {
    let mut flag = Entity::flag_short(short, Provenance::single(Source::HelpText));
    flag.value_name = value.map(str::to_string);
    flag.value_kind = if value.is_some() {
        ValueKind::Required
    } else {
        ValueKind::None
    };
    flag
}

/// A one-node tree named `name` carrying `flags`.
fn tree(name: &str, flags: Vec<Entity>) -> CommandNode {
    let mut root = CommandNode::new(name, Provenance::single(Source::HelpText));
    root.set_flags(flags);
    root
}

/// `bpftrace`'s real `TROUBLESHOOTING OPTIONS:` block, byte-exact from
/// `corpus/killsnoop.bt/audit-seed2/help.stderr.txt`. Four rows, four real
/// flags, two of them lost.
const BPFTRACE_TROUBLESHOOTING: &str = concat!(
    "TROUBLESHOOTING OPTIONS:\n",
    "    -v                      verbose messages\n",
    "    -vv                     more verbose messages (max 2)\n",
    "    -d                      (dry run) debug info\n",
    "    -dd                     (dry run) verbose debug info\n",
);

/// `bpftrace`'s real `-k`/`-kk` pair from the same capture — the third
/// instance, in the ordinary `OPTIONS:` block rather than the troubleshooting
/// one, which is why it is asserted separately: the family is a property of
/// the rows, not of the section they sit in.
const BPFTRACE_KK: &str = concat!(
    "    -k             emit a warning when a bpf helper returns an error (except read \
     functions)\n",
    "    -kk            check all bpf helper functions\n",
);

/// `lessecho`'s real usage line, byte-exact from its own `--help`. Seven
/// genuine value-taking glued short flags, one of which — `[-nn]` — is
/// character-for-character this family's shape and is a correct parse. The
/// closest real thing to a false positive this detector has, and the reason
/// [`documents_bare_boolean`] exists.
pub(crate) const LESSECHO_USAGE: &str =
    "usage: lessecho [-ox] [-cx] [-pn] [-dn] [-mx] [-nn] [-ex] [-a] file ...\n";

/// `strace`'s real `[-DDD]` — a repeated-character flag whose bare `-D` is
/// never written as a standalone row, which is why it is a knowing miss
/// rather than a declared exclusion: no *labelled* tool is excluded, so the
/// gap is stated in this detector's `Scope::claim` and asserted by the
/// must-stay-silent self-check below rather than by an [`Exclusion`] entry.
///
/// [`Exclusion`]: crate::detector::Exclusion
const STRACE_USAGE: &str = "usage: strace [-ACdffhiqqrtttTvVwxxyyzZ] [-DDD] [-E var=val]\n";

/// The hand-built cases this detector is willing to be judged on when the
/// labelled set has nothing left to say.
///
/// Both directions are present because
/// [`crate::detector::Calibration::self_checks_are_conclusive`] requires
/// both: a detector that fired on everything would satisfy every must-fire
/// case here, so the must-stay-silent cases are what make the evidence mean
/// anything.
pub(crate) fn self_checks() -> Vec<crate::detector::SelfCheck> {
    use crate::detector::{Expect, SelfCheck};

    vec![
        SelfCheck {
            name: "bpftrace's real -vv and -dd",
            why: "the two flags the audit's five .bt verdicts all name, in the block they are \
                  written in — both misread as their own letter carrying itself as a value",
            expect: Expect::Fires(2),
            raw: BPFTRACE_TROUBLESHOOTING.to_string(),
            root: tree(
                "killsnoop.bt",
                vec![
                    table_flag('v', None),
                    table_flag('v', Some("v")),
                    table_flag('d', None),
                    table_flag('d', Some("d")),
                ],
            ),
        },
        SelfCheck {
            name: "bpftrace's real -kk, outside the troubleshooting block",
            why: "the third instance, in the ordinary OPTIONS: section — the family is a \
                  property of the two rows, never of the section heading above them",
            expect: Expect::Fires(1),
            raw: BPFTRACE_KK.to_string(),
            root: tree(
                "opensnoop.bt",
                vec![table_flag('k', None), table_flag('k', Some("k"))],
            ),
        },
        SelfCheck {
            name: "a three-fold repeat",
            why: "-vvv stores two copies of its own letter rather than one; the rule is \
                  'repeated', never 'doubled', and nothing here counts to two",
            expect: Expect::Fires(1),
            raw: "usage: t [-v] [-vvv]\n".to_string(),
            root: tree(
                "t",
                vec![table_flag('v', None), table_flag('v', Some("vv"))],
            ),
        },
        SelfCheck {
            name: "lessecho's real [-nn], a correct parse of the identical token shape",
            why: "the false-positive case that matters most: -n takes a number, the token is \
                  character-for-character this family's shape, and the ONLY thing that tells \
                  them apart is that lessecho never writes a bare -n",
            expect: Expect::Silent,
            raw: LESSECHO_USAGE.to_string(),
            root: tree(
                "lessecho",
                vec![
                    table_flag('o', Some("x")),
                    table_flag('c', Some("x")),
                    table_flag('p', Some("n")),
                    table_flag('d', Some("n")),
                    table_flag('m', Some("x")),
                    table_flag('n', Some("n")),
                    table_flag('e', Some("x")),
                ],
            ),
        },
        SelfCheck {
            name: "strace's real [-DDD], the declared out-of-scope miss",
            why: "a genuine repeated-character flag whose bare -D is never written on its own \
                  row — asserted rather than described, so lowering the bar has to come with a \
                  decision about lessecho",
            expect: Expect::Silent,
            raw: STRACE_USAGE.to_string(),
            root: tree("strace", vec![table_flag('D', Some("DD"))]),
        },
        SelfCheck {
            name: "a spaced value that looks exactly like a repeat",
            why: "-v v stores value_name \"v\" beside a boolean -v just as -vv does; only the \
                  raw text's space tells them apart",
            expect: Expect::Silent,
            raw: "usage: t [-v] [-v v]\n".to_string(),
            root: tree("t", vec![table_flag('v', None), table_flag('v', Some("v"))]),
        },
        SelfCheck {
            name: "tmux's real bundled cluster",
            why: "cross-family: a bundle is a SET of switches, so it can never repeat one — the \
                  two families are disjoint by construction and this asserts it",
            expect: Expect::Silent,
            raw: "usage: tmux [-2CDlNuVv] [-c shell-command]\n".to_string(),
            root: tree(
                "tmux",
                vec![table_flag('2', Some("CDlNuVv")), table_flag('2', None)],
            ),
        },
        SelfCheck {
            name: "cargo's real -Zscript",
            why: "cross-family: a single-dash long option swallows a word, never a run of its \
                  own letter",
            expect: Expect::Silent,
            raw: "usage: t [-Z] [-Zscript]\n".to_string(),
            root: tree(
                "t",
                vec![table_flag('Z', None), table_flag('Z', Some("script"))],
            ),
        },
        SelfCheck {
            name: "-Wall, the doubled letter that is a word",
            why: "the value repeats a letter but not the FLAG's letter — the predicate compares \
                  against `short`, not against itself",
            expect: Expect::Silent,
            raw: "usage: t [-W] [-Wall]\n".to_string(),
            root: tree(
                "t",
                vec![table_flag('W', None), table_flag('W', Some("all"))],
            ),
        },
        SelfCheck {
            name: "a case-differing pair",
            why: "-vV is two different flags glued, not one repeated: spellings are compared \
                  case-sensitively everywhere in this project",
            expect: Expect::Silent,
            raw: "usage: t [-v] [-vV]\n".to_string(),
            root: tree("t", vec![table_flag('v', None), table_flag('v', Some("V"))]),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(raw: &str, name: &str, flags: Vec<Entity>) -> RepeatReport {
        detect(raw, &tree(name, flags))
    }

    /// Every promoted case must hold, run through the same
    /// [`crate::detector::run_self_checks`] the calibration verdict and the
    /// ratchet gate use — so a case that stops holding fails the test suite
    /// too, not only the two runtime consumers.
    #[test]
    fn every_promoted_self_check_case_holds() {
        let outcomes = crate::detector::run_self_checks(&crate::detector::RepeatedCharFlag);
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
        assert!(
            outcomes
                .iter()
                .any(|o| matches!(o.expect, crate::detector::Expect::Fires(_))),
            "the evidence is worthless without a case the detector must fire on"
        );
        assert!(
            outcomes
                .iter()
                .any(|o| o.expect == crate::detector::Expect::Silent),
            "the evidence is worthless without a case the detector must stay silent on"
        );
    }

    #[test]
    fn detects_bpftraces_real_vv_and_dd() {
        let r = report(
            BPFTRACE_TROUBLESHOOTING,
            "killsnoop.bt",
            vec![
                table_flag('v', None),
                table_flag('v', Some("v")),
                table_flag('d', None),
                table_flag('d', Some("d")),
            ],
        );
        assert_eq!(r.misread_count(), 2);
        let tokens: Vec<&str> = r.misreads.iter().map(|m| m.token.as_str()).collect();
        assert_eq!(tokens, vec!["-vv", "-dd"]);
        assert_eq!(r.misreads[0].spelling, "-v");
    }

    /// The false positive that decides the whole design: `lessecho`'s `-nn`
    /// is this family's token shape exactly, and it is a correct parse.
    #[test]
    fn lessechos_real_nn_stays_silent_because_it_documents_no_bare_n() {
        let r = report(
            LESSECHO_USAGE,
            "lessecho",
            vec![table_flag('n', Some("n")), table_flag('a', None)],
        );
        assert_eq!(r.misread_count(), 0);
        // ...and the identical flag *does* fire the moment the document
        // declares a bare `-n` boolean, confirming condition 4 is what was
        // doing the work rather than some other condition failing silently.
        let raw = "usage: t [-n] [-nn]\n";
        let r = report(
            raw,
            "t",
            vec![table_flag('n', None), table_flag('n', Some("n"))],
        );
        assert_eq!(r.misread_count(), 1);
    }

    #[test]
    fn a_spaced_value_never_fires_however_repeated_it_looks() {
        let raw = "usage: t [-v] [-v v]\n";
        let r = report(
            raw,
            "t",
            vec![table_flag('v', None), table_flag('v', Some("v"))],
        );
        assert_eq!(r.misread_count(), 0);
    }

    #[test]
    fn an_optional_bracketed_value_stays_silent() {
        // `-v[v]` — brackets a human typed deliberately, recorded as
        // `ValueKind::Optional`. Nothing about this family is optional.
        let raw = "usage: t [-v] [-v[v]]\n";
        let mut flag = table_flag('v', Some("v"));
        flag.value_kind = ValueKind::Optional;
        let r = report(raw, "t", vec![table_flag('v', None), flag]);
        assert_eq!(r.misread_count(), 0);
    }

    #[test]
    fn a_flag_carrying_a_long_name_stays_silent() {
        let raw = "usage: t [-v] [-vv]\n";
        let mut flag = table_flag('v', Some("v"));
        flag.spellings
            .push(mandible_core::Spelling::long("verbose"));
        let r = report(raw, "t", vec![table_flag('v', None), flag]);
        assert_eq!(r.misread_count(), 0);
    }

    #[test]
    fn the_two_other_families_sharing_the_fingerprint_stay_silent() {
        // A bundle repeats nothing (it is a set); a single-dash long option
        // swallows a word. Both are given a bare boolean sibling so that
        // condition 4 cannot be what rejects them — the value's own shape
        // has to.
        let raw = "usage: t [-2] [-2CDlNuVv] [-Z] [-Zscript]\n";
        let r = report(
            raw,
            "t",
            vec![
                table_flag('2', None),
                table_flag('2', Some("CDlNuVv")),
                table_flag('Z', None),
                table_flag('Z', Some("script")),
            ],
        );
        assert_eq!(r.misread_count(), 0);
    }

    #[test]
    fn value_repeats_short_is_case_sensitive_and_rejects_empty() {
        assert!(value_repeats_short('v', "v"));
        assert!(value_repeats_short('v', "vv"));
        assert!(value_repeats_short('D', "DD"));
        assert!(!value_repeats_short('v', "V"));
        assert!(!value_repeats_short('v', "vV"));
        assert!(!value_repeats_short('W', "all"));
        assert!(!value_repeats_short('v', ""));
    }

    #[test]
    fn documents_bare_boolean_ignores_a_long_alias_but_not_a_value() {
        let mut with_long = table_flag('V', None);
        with_long
            .spellings
            .push(mandible_core::Spelling::long("version"));
        assert!(documents_bare_boolean(&[with_long], 'V'));
        assert!(!documents_bare_boolean(&[table_flag('V', Some("x"))], 'V'));
        assert!(!documents_bare_boolean(&[table_flag('v', None)], 'V'));
    }

    #[test]
    fn a_subcommands_own_misread_is_reported_at_its_own_path() {
        let raw = "usage: t sub [-v] [-vv]\n";
        let mut root = CommandNode::new("t", Provenance::single(Source::HelpText));
        let mut sub = CommandNode::new("sub", Provenance::single(Source::HelpText));
        sub.entities.push(table_flag('v', None));
        sub.entities.push(table_flag('v', Some("v")));
        root.subcommands.push(sub);
        let r = detect(raw, &root);
        assert_eq!(r.misread_count(), 1);
        assert_eq!(r.misreads[0].path, "t sub");
    }

    #[test]
    fn empty_text_and_empty_tree_report_nothing() {
        let r = detect(
            "",
            &CommandNode::new("nothing", Provenance::single(Source::HelpText)),
        );
        assert_eq!(r.misread_count(), 0);
    }
}
