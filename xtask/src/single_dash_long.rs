//! The **single-dash long option** split: `-help` read as `-h` carrying a
//! required value `"elp"`.
//!
//! The third of the three families that share the structural fingerprint
//! `short && !long && value_name` (see [`crate::bundling`]'s doc comment for
//! the table), and the one spec §13.1's K1 pre-tag was originally named
//! after: *"a flag like `-fdump-scos` is stored as short flag `-f` with
//! `value_name` `dump-scos` instead of as the long-form spelling it actually
//! is."*
//!
//! `qemu-arm64-static` is the labelled set's clearest document — an option
//! table whose rows are single-dash long options and genuine value-taking
//! short flags, side by side:
//!
//! ```text
//! -h                                        print this help
//! -help
//! -g port              QEMU_GDB             wait gdb connection to 'port'
//! -cpu model           QEMU_CPU             select CPU (-cpu help for list)
//! -one-insn-per-tb     QEMU_ONE_INSN_PER_TB run with one guest instruction per emulated TB
//! -version             QEMU_VERSION         display version information and exit
//! ```
//!
//! Eleven of its rows are long options and the tree has none of them: `-help`
//! becomes `-h` + `"elp"`, `-cpu` becomes `-c` + `"pu"`, `-version` becomes
//! `-v` + `"ersion"`, `-one-insn-per-tb` becomes `-o` + `"ne-insn-per-tb"`.
//! Meanwhile `-g port`, `-L path`, `-B address` and `-R size` on the same
//! rows are entirely correct, and the *only* thing separating them in the
//! document is a space.
//!
//! # The rule
//!
//! A flag is reported when **all** of these hold:
//!
//! 1. **It is option-table-sourced** ([`mandible_core::Source::HelpText`],
//!    never [`mandible_core::Source::HelpTextSynopsis`]). The exact mirror of
//!    [`crate::bundling`]'s condition 1, and for the mirrored reason: a long
//!    option earns its own table row with its own description column, while a
//!    getopt *cluster* is a synopsis phenomenon. Restricting to the table
//!    keeps the entire bundle population — including the unsorted,
//!    uniformly-cased bundles the grammar's own fix knowingly cannot split
//!    (`rpcbind`'s `[-adhilswfr]`, `umount.nfs`'s `[-fvnrlh]`) — out of this
//!    detector's population by construction rather than by a threshold.
//! 2. **It has a short spelling, no long name, and a `Required` value.** The
//!    shared fingerprint. `Optional` means the raw text wrote brackets
//!    (`ip`'s `-h[uman-readable]`), which is a value spec a human typed.
//! 3. **The swallowed text is option-name-shaped**
//!    ([`is_option_name_tail`]): ASCII alphanumerics and `-`, with at least
//!    one letter. This rejects every value spec that leaks punctuation —
//!    `qemu`'s own `-E var=value` and `-d item[,...]`, `sg_emc_trespass`'s
//!    layout-mangled `-hr:` — and it is the condition that makes the claim
//!    exact rather than approximate.
//! 4. **At least [`MIN_SWALLOWED_CHARS`] characters are swallowed.** Two,
//!    not one, and the lost recall is deliberate — see that constant.
//! 5. **The whole token is uniformly lowercase**
//!    ([`token_is_uniformly_lowercase`]). The discriminator against the
//!    largest genuinely-correct population there is; see below.
//! 6. **The swallowed text is not the flag's own character repeated**
//!    ([`crate::repeated_char`]'s family). `-vvv` satisfies every condition
//!    above and belongs to the other detector; excluding it here is what
//!    makes the two provably disjoint, so neither can inflate its own count
//!    with the other's findings.
//! 7. **The reconstructed token occurs glued and delimited in the raw text**
//!    ([`crate::existence::spelling_occurs`]). The same load-bearing
//!    separator check [`crate::bundling`] uses, and the one that disposes of
//!    the whole spaced-value population: `-g port` stores `value_name:
//!    "port"` exactly as `-help` stores `"elp"`, and `-gport` never occurs.
//!
//! # Condition 5 is the safety argument
//!
//! Conditions 1–4 and 7 are satisfied, character for character, by the
//! GCC/Clang glued-value convention — thousands of **correct** parses
//! fleet-wide, every one of which this detector must stay silent on:
//! `cargo -Zscript`, `rpcgen -Dname`, `makewhatis -Tutf8`, `perl
//! -Idirectory`, `find -Olevel`, `cc -oOUTFILE`, `gcc -DMACRO`. Every single
//! one carries an **uppercase** letter, because the convention is what it is:
//! the flag is a capital and the glued text is its argument.
//!
//! The real long options are the other way round and just as consistent —
//! `-help`, `-cpu`, `-version`, `-strace`, `-seed`, `-trace`, `-perfmap`,
//! `-jitdump`, `-singlestep`, `-one-insn-per-tb`, `-dfilter`,
//! `-pass-exit-codes`, `-fdump-scos`, `-nostdlib`, `-pthread` — because a
//! long option is a *word*, and words in `--help` output are lowercase.
//!
//! This is the same species of argument as
//! [`crate::bundling::swallowed_members_mix_case`], applied in the opposite
//! direction, and it is the only signal measured against that population that
//! does not also destroy it. It is not free: it is why `-Wall`-shaped rows
//! and every uppercase-led long option are knowingly out of reach here.
//!
//! # What it deliberately does not catch
//!
//! Named, counted, and never dropped from a report:
//!
//! - **Uppercase-led single-dash long options.** Excluded by condition 5,
//!   which cannot tell them from `-Zscript`. There is no measured signal that
//!   separates the two, and buying that recall costs a false positive on a
//!   correct parse.
//! - **`ip`'s bracketed abbreviations** (`-h[uman-readable]`,
//!   `-b[atch]`, `-rc[vbuf]`). The raw text writes the optional tail in
//!   brackets, so the grammar records `ValueKind::Optional` and condition 2
//!   never admits them. A labelled member of this family, declared as an
//!   exclusion with its own witness token.
//! - **Rows whose swallowed half carries layout punctuation.**
//!   `sg_emc_trespass` writes `-hr: Set Honor Reservation bit` with no space
//!   before the colon, so the tree stores `-h` + `"r:"` and condition 3
//!   rejects it. Also a labelled member, also declared.
//! - **One-character tails** ([`MIN_SWALLOWED_CHARS`]) — `rpcgen -Ss`,
//!   `xxd -ps`, `sg_map -st`, `mandoc -ac`, `which -as`. The same
//!   two-character population `bundling::MIN_BUNDLED_MEMBERS` already
//!   excludes for the same measured reason: roughly half of it is real and
//!   nothing on the shape separates the halves.
//!
//! The count this module reports is therefore a lower bound, which is the
//! correct direction for a number that becomes a gate: a false negative
//! leaves a real bug unreported, a false positive blocks the fix.

use crate::existence::spelling_occurs;
use mandible_core::{CommandNode, Flag, Provenance, Source, ValueKind};

/// The fewest characters a tail must carry before it is read as the rest of
/// a long option's name.
///
/// Two, not one, and the difference is deliberate lost recall — the same
/// measured population `bundling::MIN_BUNDLED_MEMBERS` excludes, seen from
/// the other side. At one swallowed character the shape is genuinely
/// ambiguous: `rpcgen`'s `[-Sc]`/`[-Ss]`/`[-Sm]` (generate sample
/// client/server/makefile), `psfxtable`'s `[-it]`/`[-ot]`, `sg_map`'s
/// `[-st]`, `setfont`'s `[-ou]`, `mandoc`'s `[-ac]`, `which`'s `[-as]` and
/// `xxd`'s `[-ps]` are all two-character single-dash tokens, and the fleet
/// scan behind `5f1abec` found nothing about their shape that separates the
/// real collapses from the correct parses. A detector gated at zero must
/// never fire on a correct parse, so the whole class is excluded.
pub(crate) const MIN_SWALLOWED_CHARS: usize = 2;

/// True when `tail` could be the rest of a single-dash long option's name:
/// ASCII alphanumerics and `-`, with at least one ASCII letter in it.
///
/// The letter requirement is what stops a glued numeric argument (`-b4096`,
/// `-j8`) from riding in on a run that is technically alphanumeric, exactly
/// as `bundling::MIN_ORDERED_LETTERS` stops the vacuous-ordering version of
/// the same hazard. Everything else is rejected because a long option's name
/// does not contain it: `=` (`-mtune=native`, `-E var=value`), `:`
/// (`sg_emc_trespass`'s layout-mangled `-hr:`), `[`/`{`/`<`/`,`
/// (`-d item[,...]`, `-b{blocksize}`), `.` and `/` (paths), `_` (which no
/// single-dash long option in the fleet uses, and which appears in glued
/// value placeholders that do).
fn is_option_name_tail(tail: &str) -> bool {
    tail.chars().any(|c| c.is_ascii_alphabetic())
        && tail.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// True when `token` carries no ASCII uppercase letter at all — the
/// discriminator against the GCC/Clang glued-value convention, whose whole
/// population is an uppercase flag letter with its argument glued on
/// (`-Zscript`, `-Dname`, `-Tutf8`, `-Idirectory`, `-Olevel`, `-DMACRO`,
/// `-oOUTFILE`, `-Wall`).
///
/// Measured over the *whole* token rather than only the tail, deliberately,
/// and the difference is `-oOUTFILE`: its flag letter is lowercase and only
/// the argument shouts, so a tail-only test would admit it. See this
/// module's doc comment for the full argument and for what it costs.
fn token_is_uniformly_lowercase(token: &str) -> bool {
    !token.chars().any(|c| c.is_ascii_uppercase())
}

/// True when `tail` is one or more copies of `short` — the
/// [`crate::repeated_char`] family, handed off rather than double-counted.
///
/// Duplicated in spirit rather than imported so the two detectors' rules
/// each read completely in one place; the assertion that the two agree lives
/// in this module's own tests.
fn tail_repeats_short(short: char, tail: &str) -> bool {
    !tail.is_empty() && tail.chars().all(|c| c == short)
}

/// Whether `provenance` credits the option-table grammar rather than the
/// usage synopsis — this module's condition 1, and the exact mirror of
/// [`crate::bundling`]'s.
fn is_table_sourced(provenance: &Provenance) -> bool {
    provenance.sources.contains(&Source::HelpText)
        && !provenance.sources.contains(&Source::HelpTextSynopsis)
}

/// One single-dash long option split into a short flag plus a value.
pub struct Split {
    /// Space-separated path to the node owning the flag, e.g. `"qemu-arm64-static"`.
    pub path: String,
    /// The surviving flag's spelling, e.g. `"-h"` — frequently a real flag of
    /// the tool in its own right (`qemu` documents both `-h` and `-help`),
    /// which is exactly why [`crate::existence`] is blind to this family: the
    /// spelling is attested and the parse is still wrong.
    pub spelling: String,
    /// The real token this split, e.g. `"-help"` — the spelling a user would
    /// type and cannot find anywhere in the tree.
    pub token: String,
}

/// The result of analyzing one tool.
pub struct SplitReport {
    pub splits: Vec<Split>,
}

impl SplitReport {
    /// How many single-dash long options this tool split. One lost flag
    /// each: the long spelling itself.
    pub fn split_count(&self) -> usize {
        self.splits.len()
    }
}

/// Whether `flag` is a split single-dash long option, and the real token it
/// split — `None` when any condition in this module's doc comment fails.
fn split_token(flag: &Flag, raw: &str) -> Option<String> {
    // 1. Option-table-sourced, never synopsis.
    if !is_table_sourced(&flag.provenance) {
        return None;
    }
    // 2. A bare short flag carrying a required value.
    let short = flag.short?;
    if flag.long.is_some() || flag.value_kind != ValueKind::Required {
        return None;
    }
    let tail = flag.value_name.as_deref()?;
    // 4. Enough tail to be a name rather than a character argument.
    if tail.chars().count() < MIN_SWALLOWED_CHARS {
        return None;
    }
    // 3. The tail is option-name-shaped.
    if !is_option_name_tail(tail) {
        return None;
    }
    // 6. Not the repeated-character family.
    if tail_repeats_short(short, tail) {
        return None;
    }
    let token = format!("-{short}{tail}");
    // 5. Uniformly lowercase — the discriminator against the glued-value
    //    convention. See this module's doc comment.
    if !token_is_uniformly_lowercase(&token) {
        return None;
    }
    // 7. The token occurs, glued and delimited, in the raw text. Last
    //    because it is the only condition that scans the document.
    if !spelling_occurs(raw, &token) {
        return None;
    }
    Some(token)
}

fn walk(node: &CommandNode, path: &str, raw: &str, out: &mut Vec<Split>) {
    for flag in &node.flags {
        let Some(token) = split_token(flag, raw) else {
            continue;
        };
        let Some(short) = flag.short else {
            continue;
        };
        out.push(Split {
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

/// Analyze `root`'s option-table flags against `raw` for the single-dash
/// long-option split.
pub fn detect(raw: &str, root: &CommandNode) -> SplitReport {
    let mut splits = Vec::new();
    walk(root, &root.name, raw, &mut splits);
    SplitReport { splits }
}

// ----------------------------------------------------------------------
// The hand-built evidence, promoted out of `#[cfg(test)]`
// ----------------------------------------------------------------------
//
// Same arrangement, and for the same two runtime consumers, as
// `crate::bundling`'s promoted block — `crate::detector::calibrate` and
// `crate::detector::ratchet_at_zero`, neither of which runs under the test
// harness.

/// A flag as `sections::emit_flags` builds one from an option-table row.
fn table_flag(short: char, value: Option<&str>) -> Flag {
    let mut flag = Flag::long("", Provenance::single(Source::HelpText));
    flag.long = None;
    flag.short = Some(short);
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

/// `qemu-arm64-static`'s real option table, byte-exact from
/// `corpus/qemu-arm64-static/audit-seed2/help.txt` — the long options and the
/// genuine value-taking short flags on adjacent rows, which is the whole
/// false-positive problem in six lines.
const QEMU_TABLE: &str = concat!(
    "-h                                        print this help\n",
    "-help                                     \n",
    "-g port              QEMU_GDB             wait gdb connection to 'port'\n",
    "-cpu model           QEMU_CPU             select CPU (-cpu help for list)\n",
    "-one-insn-per-tb     QEMU_ONE_INSN_PER_TB run with one guest instruction per emulated TB\n",
    "-version             QEMU_VERSION         display version information and exit\n",
);

/// `ip`'s real bracketed-abbreviation token, carried here as the witness its
/// [`crate::detector::Ground`] cites.
pub(crate) const IP_BRACKETED_TOKEN: &str = "-h[uman-readable]";

/// `sg_emc_trespass`'s real row token, carried here as the witness its
/// [`crate::detector::Ground`] cites: the help text glues a colon straight
/// onto the flag, so the tree stores `-h` + `"r:"`.
pub(crate) const SG_EMC_TRESPASS_TOKEN: &str = "-hr:";

/// The hand-built cases this detector is willing to be judged on when the
/// labelled set has nothing left to say. Both directions, for the reason
/// [`crate::detector::Calibration::self_checks_are_conclusive`] states.
pub(crate) fn self_checks() -> Vec<crate::detector::SelfCheck> {
    use crate::detector::{Expect, SelfCheck};

    let mut cases = vec![
        SelfCheck {
            name: "qemu's real table, long options beside genuine valued shorts",
            why: "the false-positive case that matters most: -g port, -cpu model and -help sit \
                  on adjacent rows of one table and only the space separates the first from the \
                  rest",
            expect: Expect::Fires(4),
            raw: QEMU_TABLE.to_string(),
            root: tree(
                "qemu-arm64-static",
                vec![
                    table_flag('h', None),
                    table_flag('h', Some("elp")),
                    table_flag('g', Some("port")),
                    table_flag('c', Some("pu")),
                    table_flag('o', Some("ne-insn-per-tb")),
                    table_flag('v', Some("ersion")),
                ],
            ),
        },
        SelfCheck {
            name: "gcc's real -pass-exit-codes",
            why: "spec §13.1's own K1 example: a hyphenated long option, which is the shape the \
                  pre-tag was named after",
            expect: Expect::Fires(1),
            raw: "  -pass-exit-codes         Exit with highest error code from a phase.\n"
                .to_string(),
            root: tree("gcc", vec![table_flag('p', Some("ass-exit-codes"))]),
        },
        SelfCheck {
            name: "gcc's real -fdump-scos",
            why: "the other K1 example spec §13.1 names by hand",
            expect: Expect::Fires(1),
            raw: "  -fdump-scos                 [available in Ada]\n".to_string(),
            root: tree("gcc", vec![table_flag('f', Some("dump-scos"))]),
        },
        SelfCheck {
            name: "a spaced value that looks exactly like a long option",
            why: "-g port stores a value_name just as -help does; only the raw text's space \
                  tells them apart",
            expect: Expect::Silent,
            raw: "  -g port    wait gdb connection to 'port'\n".to_string(),
            root: tree("t", vec![table_flag('g', Some("port"))]),
        },
        SelfCheck {
            name: "ip's real -h[uman-readable], the declared out-of-scope miss",
            why: "a labelled member of this family whose raw text writes the tail in brackets, \
                  so the grammar records ValueKind::Optional — asserted rather than described",
            expect: Expect::Silent,
            raw: "OPTIONS := { -V[ersion] | -h[uman-readable] | -j[son] }\n".to_string(),
            root: tree("ip", {
                let mut f = table_flag('h', Some("uman-readable"));
                f.value_kind = ValueKind::Optional;
                vec![f]
            }),
        },
        SelfCheck {
            name: "sg_emc_trespass's real -hr:, the other declared miss",
            why: "a labelled member whose swallowed half carries the layout's own colon, which \
                  is not an option-name character",
            expect: Expect::Silent,
            raw: "    -hr: Set Honor Reservation bit\n".to_string(),
            root: tree("sg_emc_trespass", vec![table_flag('h', Some("r:"))]),
        },
        SelfCheck {
            name: "tmux's real bundled cluster, given the table source it never has",
            why: "cross-family: the cluster is rejected on its own uppercase letters, so the \
                  rejection does not rest on the source check alone",
            expect: Expect::Silent,
            raw: "usage: tmux [-2CDlNuVv]\n".to_string(),
            root: tree("tmux", vec![table_flag('2', Some("CDlNuVv"))]),
        },
        SelfCheck {
            name: "a synopsis-sourced cluster",
            why: "condition 1 alone: an all-lowercase unsorted bundle is indistinguishable from \
                  a long option on every other condition, and only the source keeps the whole \
                  population out",
            expect: Expect::Silent,
            raw: "usage: rpcbind [-adhilswfr]\n".to_string(),
            root: {
                let mut f = table_flag('a', Some("dhilswfr"));
                f.provenance = Provenance::single(Source::HelpTextSynopsis);
                tree("rpcbind", vec![f])
            },
        },
        SelfCheck {
            name: "bpftrace's real -vv",
            why: "cross-family: a repeated-character flag satisfies every other condition and is \
                  handed off, so neither detector counts the other's findings",
            expect: Expect::Silent,
            raw: "    -vvv    even more verbose\n".to_string(),
            root: tree("t", vec![table_flag('v', Some("vv"))]),
        },
        SelfCheck {
            name: "rpcgen's real -Ss, the one-character tail",
            why: "the declared MIN_SWALLOWED_CHARS exclusion — the same ambiguous two-character \
                  population bundling::MIN_BUNDLED_MEMBERS already leaves alone",
            expect: Expect::Silent,
            raw: "usage: rpcgen [-Ss] [-Sc] [-Sm]\n".to_string(),
            root: tree("rpcgen", vec![table_flag('S', Some("s"))]),
        },
        SelfCheck {
            name: "a glued numeric default",
            why: "-b4096 is alphanumeric and lowercase and glued; only the letter requirement in \
                  is_option_name_tail rejects it",
            expect: Expect::Silent,
            raw: "  -b4096   block size\n".to_string(),
            root: tree("t", vec![table_flag('b', Some("4096"))]),
        },
    ];

    // The GCC/Clang glued-value convention: the largest genuinely-correct
    // population sharing this shape, every member of it rejected by
    // condition 5 and by nothing else.
    for (name, short, tail) in [
        ("cargo's real -Zscript", 'Z', "script"),
        ("rpcgen's real -Dname", 'D', "name"),
        ("makewhatis's real -Tutf8", 'T', "utf8"),
        ("perl's real -Idirectory", 'I', "directory"),
        ("cc's real -oOUTFILE", 'o', "OUTFILE"),
    ] {
        cases.push(SelfCheck {
            name,
            why: "a correct parse of a real glued value: the convention is an uppercase flag \
                  letter with its argument attached, and condition 5 is the only thing that \
                  tells it from a long option",
            expect: Expect::Silent,
            raw: format!("  -{short}{tail}   a real glued value\n"),
            root: tree("t", vec![table_flag(short, Some(tail))]),
        });
    }

    cases
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(raw: &str, name: &str, flags: Vec<Flag>) -> SplitReport {
        detect(raw, &tree(name, flags))
    }

    #[test]
    fn every_promoted_self_check_case_holds() {
        let outcomes = crate::detector::run_self_checks(&crate::detector::SingleDashLong);
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
        assert!(outcomes
            .iter()
            .any(|o| matches!(o.expect, crate::detector::Expect::Fires(_))));
        assert!(outcomes
            .iter()
            .any(|o| o.expect == crate::detector::Expect::Silent));
    }

    #[test]
    fn detects_qemus_real_long_options_but_not_its_spaced_values() {
        let r = report(
            QEMU_TABLE,
            "qemu-arm64-static",
            vec![
                table_flag('h', None),
                table_flag('h', Some("elp")),
                table_flag('g', Some("port")),
                table_flag('c', Some("pu")),
                table_flag('o', Some("ne-insn-per-tb")),
                table_flag('v', Some("ersion")),
            ],
        );
        let tokens: Vec<&str> = r.splits.iter().map(|s| s.token.as_str()).collect();
        assert_eq!(
            tokens,
            vec!["-help", "-cpu", "-one-insn-per-tb", "-version"]
        );
        assert_eq!(r.split_count(), 4);
    }

    #[test]
    fn a_spaced_value_never_fires_however_long_option_shaped_it_looks() {
        let raw = "  -g port    wait gdb connection to 'port'\n";
        let r = report(raw, "t", vec![table_flag('g', Some("port"))]);
        assert_eq!(r.split_count(), 0);
        // ...and the same flag does fire once the raw text glues it,
        // confirming the space was doing the work.
        let glued = "  -gport    wait gdb connection\n";
        let r = report(glued, "t", vec![table_flag('g', Some("port"))]);
        assert_eq!(r.split_count(), 1);
    }

    #[test]
    fn the_gcc_glued_value_convention_stays_silent() {
        for (short, tail) in [
            ('Z', "script"),
            ('D', "name"),
            ('T', "utf8"),
            ('I', "directory"),
            ('O', "level"),
            ('o', "OUTFILE"),
            ('W', "all"),
        ] {
            let token = format!("-{short}{tail}");
            let raw = format!("  {token}   a real glued value\n");
            let r = report(&raw, "t", vec![table_flag(short, Some(tail))]);
            assert_eq!(r.split_count(), 0, "{token} must not fire");
            assert!(!token_is_uniformly_lowercase(&token));
        }
    }

    #[test]
    fn a_synopsis_sourced_flag_stays_silent_on_source_alone() {
        // An all-lowercase unsorted bundle passes every other condition;
        // only condition 1 keeps the whole population out.
        let raw = "usage: rpcbind [-adhilswfr]\n";
        let mut flag = table_flag('a', Some("dhilswfr"));
        flag.provenance = Provenance::single(Source::HelpTextSynopsis);
        let r = report(raw, "rpcbind", vec![flag]);
        assert_eq!(r.split_count(), 0);
        assert!(is_option_name_tail("dhilswfr"));
        assert!(token_is_uniformly_lowercase("-adhilswfr"));
    }

    /// The two families that share this fingerprint are disjoint from this
    /// one *by construction*, and the disjointness is asserted in both
    /// directions here and in `crate::repeated_char`'s own tests.
    #[test]
    fn the_repeated_character_family_is_handed_off_not_double_counted() {
        let raw = "    -vvv    even more verbose\n";
        let r = report(raw, "t", vec![table_flag('v', Some("vv"))]);
        assert_eq!(r.split_count(), 0);
        assert!(tail_repeats_short('v', "vv"));
        // A tail that merely starts with the flag's letter is still a word.
        assert!(!tail_repeats_short('v', "version"));
    }

    #[test]
    fn a_one_character_tail_is_deliberately_not_reported() {
        for (short, tail) in [('s', "t"), ('p', "s"), ('a', "c"), ('a', "s")] {
            let token = format!("-{short}{tail}");
            let raw = format!("usage: t [{token}]\n");
            let r = report(&raw, "t", vec![table_flag(short, Some(tail))]);
            assert_eq!(r.split_count(), 0, "{token} is below MIN_SWALLOWED_CHARS");
        }
        assert_eq!(MIN_SWALLOWED_CHARS, 2);
    }

    #[test]
    fn is_option_name_tail_needs_a_letter_and_rejects_value_punctuation() {
        assert!(is_option_name_tail("elp"));
        assert!(is_option_name_tail("ne-insn-per-tb"));
        assert!(is_option_name_tail("print0"));
        assert!(!is_option_name_tail("4096"));
        assert!(!is_option_name_tail("r:"));
        assert!(!is_option_name_tail("var=value"));
        assert!(!is_option_name_tail("item[,...]"));
        assert!(!is_option_name_tail("c[vbuf]"));
        assert!(!is_option_name_tail("a_b"));
        assert!(!is_option_name_tail(""));
    }

    #[test]
    fn an_optional_bracketed_value_stays_silent() {
        let raw = "OPTIONS := { -h[uman-readable] }\n";
        let mut flag = table_flag('h', Some("uman-readable"));
        flag.value_kind = ValueKind::Optional;
        let r = report(raw, "ip", vec![flag]);
        assert_eq!(r.split_count(), 0);
    }

    #[test]
    fn a_flag_carrying_a_long_name_stays_silent() {
        let raw = "  -help   print this help\n";
        let mut flag = table_flag('h', Some("elp"));
        flag.long = Some("help".to_string());
        let r = report(raw, "t", vec![flag]);
        assert_eq!(r.split_count(), 0);
    }

    #[test]
    fn a_subcommands_own_split_is_reported_at_its_own_path() {
        let raw = "usage: t sub -help\n";
        let mut root = CommandNode::new("t", Provenance::single(Source::HelpText));
        let mut sub = CommandNode::new("sub", Provenance::single(Source::HelpText));
        sub.flags.push(table_flag('h', Some("elp")));
        root.subcommands.push(sub);
        let r = detect(raw, &root);
        assert_eq!(r.split_count(), 1);
        assert_eq!(r.splits[0].path, "t sub");
    }

    #[test]
    fn empty_text_and_empty_tree_report_nothing() {
        let r = detect(
            "",
            &CommandNode::new("nothing", Provenance::single(Source::HelpText)),
        );
        assert_eq!(r.split_count(), 0);
    }
}
