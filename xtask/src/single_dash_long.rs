//! The single-dash long option split: `-help` read as `-h` carrying a
//! required value `"elp"` (S-035). The third of the three families
//! sharing the `short && !long && value_name` fingerprint (see
//! [`crate::bundling`]'s doc comment for the table).
//!
//! `qemu-arm64-static` (S-035's fixture) documents long options and
//! genuine value-taking short flags side by side, separated only by
//! whitespace: `-help` becomes `-h` + `"elp"`; `-g port` stores
//! `value_name: "port"` correctly.
//!
//! A flag is reported when all hold: option-table-sourced
//! ([`mandible_core::Source::HelpText`], never `HelpTextSynopsis`, mirroring
//! [`crate::bundling`]'s condition 1); short spelling, no long name,
//! `Required` value; the swallowed text's name half is option-name-shaped
//! ([`is_option_name_tail`]: alphanumerics, `-`, `_`, at least one
//! letter — everything before the first `=`, [`split_glued_value`]); at
//! least [`MIN_SWALLOWED_CHARS`] characters; the reconstructed name token
//! is uniformly lowercase ([`token_is_uniformly_lowercase`]); the
//! swallowed text is not the flag's own character repeated
//! ([`crate::repeated_char`]'s family, kept disjoint); the reconstructed
//! token occurs glued and delimited in the raw text
//! ([`crate::existence::spelling_occurs`]).
//!
//! The lowercase condition is the safety argument: the GCC/Clang
//! glued-value convention (`-Zscript`, `-Dname`, `-DMACRO`, `-sDEVICE=x`)
//! satisfies every other condition but always carries an uppercase flag
//! letter, while a real long option is a lowercase word
//! (`-help`, `-cpu`, `-version`, `-pass-exit-codes`) — the same argument
//! as [`crate::bundling::swallowed_members_mix_case`], reversed. Measured
//! over the whole token, not just the tail, because of `-oOUTFILE` (its
//! flag letter is lowercase, only the argument shouts).
//!
//! Does not catch: uppercase-led single-dash long options (no signal
//! separates them from GCC-style flags); `ip`'s bracketed abbreviations
//! (`-h[uman-readable]`, `ValueKind::Optional`, declared exclusion);
//! layout-mangled rows (`sg_emc_trespass`'s `-hr:`, declared exclusion);
//! one-character tails ([`MIN_SWALLOWED_CHARS`], same reasoning as
//! `bundling::MIN_BUNDLED_MEMBERS`). Underscore was once excluded and no
//! longer is — see `help_text::sections::repair_single_dash_long_options`'s
//! "Why `_` is a name character". The count is a lower bound.

use crate::existence::spelling_occurs;
use mandible_core::{CommandNode, Entity, Provenance, Source, ValueKind};

/// The fewest characters a tail must carry before it is read as the rest of
/// a long option's name. Two, not one — deliberate lost recall, mirroring
/// `bundling::MIN_BUNDLED_MEMBERS` (S-034): at one swallowed character the
/// shape is genuinely ambiguous, and nothing about it separates real
/// collapses from correct two-character single-dash flags.
pub(crate) const MIN_SWALLOWED_CHARS: usize = 2;

/// True when `tail` could be the rest of a single-dash long option's name:
/// ASCII alphanumerics, `-` and `_`, with at least one ASCII letter (the
/// letter requirement stops a glued numeric argument like `-b4096`).
/// Rejects `:`/`[`/`{`/`<`/`,`/`.`/`/` — none of them appear in a long
/// option's name. `_` is admitted as a word separator, same footing as
/// `-`. `=` never reaches here: [`split_glued_value`] already consumed it.
fn is_option_name_tail(tail: &str) -> bool {
    tail.chars().any(|c| c.is_ascii_alphabetic())
        && tail
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// True when `token` carries no ASCII uppercase letter — the discriminator
/// against the GCC/Clang glued-value convention (`-Zscript`, `-DMACRO`,
/// `-oOUTFILE`). Measured over the whole token, not just the tail: an
/// uppercase flag letter with a lowercase argument (`-oOUTFILE`) must
/// still be caught, which a tail-only test would miss.
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
fn split_token(flag: &Entity, raw: &str) -> Option<String> {
    // 1. Option-table-sourced, never synopsis.
    if !is_table_sourced(&flag.provenance) {
        return None;
    }
    // 2. A bare short flag carrying a required value.
    let short = flag.short()?;
    if flag.long().is_some() || flag.value_kind != ValueKind::Required {
        return None;
    }
    let tail = flag.value_name.as_deref()?;
    // 3a. Split the swallowed text at the first `=` — see
    //     [`split_glued_value`].
    let (name_tail, _value) = split_glued_value(tail)?;
    // 4. Enough *name* to be a name rather than a character argument.
    if name_tail.chars().count() < MIN_SWALLOWED_CHARS {
        return None;
    }
    // 3. The name half is option-name-shaped.
    if !is_option_name_tail(name_tail) {
        return None;
    }
    // 6. Not the repeated-character family.
    if tail_repeats_short(short, tail) {
        return None;
    }
    let name_token = format!("-{short}{name_tail}");
    // 5. Uniformly lowercase — the discriminator against the glued-value
    //    convention. See this module's doc comment.
    if !token_is_uniformly_lowercase(&name_token) {
        return None;
    }
    // 7. The whole token — name and glued value — occurs, glued and
    //    delimited, in the raw text. Last because it is the only condition
    //    that scans the document.
    if !spelling_occurs(raw, &format!("-{short}{tail}")) {
        return None;
    }
    Some(name_token)
}

/// Split a swallowed tail into the option-name half and the glued value
/// half: `"umber=N"` → `("umber", Some("N"))`, `"elp"` → `("elp", None)`.
///
/// `None` when the tail ends at the `=` with nothing after it. The twin of
/// `mandible_extract::help_text::sections::split_glued_value`, character for
/// character, and carried here rather than imported for the reason every
/// other predicate in this module is: the oracle's rule must read completely
/// in one place, and the assertion that the two agree is what the corpus and
/// this module's tests are for.
fn split_glued_value(tail: &str) -> Option<(&str, Option<&str>)> {
    match tail.split_once('=') {
        Some((_, "")) => None,
        Some((name, value)) => Some((name, Some(value))),
        None => Some((tail, None)),
    }
}

fn walk(node: &CommandNode, path: &str, raw: &str, out: &mut Vec<Split>) {
    for flag in node.flags() {
        let Some(token) = split_token(flag, raw) else {
            continue;
        };
        let Some(short) = flag.short() else {
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

/// `dbiprof`'s real option table, byte-exact from
/// `corpus/dbiprof/1.643/help.txt` — the glued-`=value` rows and the
/// value-less rows in one table, which is the whole `=`-split problem in
/// five lines.
const DBIPROF_TABLE: &str = concat!(
    "    -number=N        show top N, defaults to 10\n",
    "    -sort=S          sort by S, defaults to total\n",
    "    -reverse         reverse the sort\n",
    "    -match=K=V       for filtering, see docs\n",
    "    -case_sensitive  for -match and -exclude\n",
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
// Ratchet: a literal table of hand-built cases, not logic. Listed in scripts/ratchet.txt.
#[allow(clippy::too_many_lines)]
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
        SelfCheck {
            name: "dbiprof's real option table, glued =value beside value-less rows",
            why: "the shape the =-split exists for: -number=N and -reverse sit in one table and \
                  only the = separates the row that used to be repaired from the one that did \
                  not. -case_sensitive rides in on the same rule now that _ is a name \
                  character, which is why this counts five and not four",
            expect: Expect::Fires(5),
            raw: DBIPROF_TABLE.to_string(),
            root: tree(
                "dbiprof",
                vec![
                    table_flag('n', Some("umber=N")),
                    table_flag('s', Some("ort=S")),
                    table_flag('r', Some("everse")),
                    table_flag('m', Some("atch=K=V")),
                    table_flag('c', Some("ase_sensitive")),
                ],
            ),
        },
        SelfCheck {
            name: "gcc's real -foffload=<targets>",
            why: "the human audit's confirmed parser bug on this family: a lowercase name half \
                  with a shouting value spec on the right of the =",
            expect: Expect::Fires(1),
            raw: "  -foffload=<targets>      Specify offloading targets.\n".to_string(),
            root: tree("gcc", vec![table_flag('f', Some("offload=<targets>"))]),
        },
        SelfCheck {
            name: "ghostscript's real -sDEVICE=png16m",
            why: "the inverse case that matters most: a genuine glued short whose argument is \
                  introduced by =, and whose name half shouts exactly where the convention puts \
                  the shout",
            expect: Expect::Silent,
            raw: "  -sDEVICE=png16m   select the output device\n".to_string(),
            root: tree("gs", vec![table_flag('s', Some("DEVICE=png16m"))]),
        },
        SelfCheck {
            name: "cpp's real -DMACRO=value",
            why: "the same inverse, in the shape the whole no-uppercase rule was written for",
            expect: Expect::Silent,
            raw: "  -DMACRO=value   define a macro\n".to_string(),
            root: tree("cpp", vec![table_flag('D', Some("MACRO=value"))]),
        },
        SelfCheck {
            name: "a spaced key=value argument",
            why: "-E var=value stores exactly what -number=N stores; only the raw text's space \
                  tells them apart, and condition 7 is what reads it",
            expect: Expect::Silent,
            raw: "  -e var=value    set an environment variable\n".to_string(),
            root: tree("t", vec![table_flag('e', Some("var=value"))]),
        },
        SelfCheck {
            name: "a tail that ends at the = with nothing after it",
            why: "split_glued_value refuses it outright rather than inventing either reading",
            expect: Expect::Silent,
            raw: "  -foo=   an empty value spec\n".to_string(),
            root: tree("t", vec![table_flag('f', Some("oo="))]),
        },
        SelfCheck {
            name: "ffplay's real AVOption row, the underscore family's bulk",
            why: "97% of the underscore population is this one table shape; the name is what the \
                  document writes at the head of the row and the value spec sits in a \
                  space-separated column the grammar never stored in value_name",
            expect: Expect::Fires(1),
            raw: "  -is_avc            <boolean>    .D.V..X.... is avc (default false)\n"
                .to_string(),
            root: tree("ffplay", vec![table_flag('i', Some("s_avc"))]),
        },
        SelfCheck {
            name: "cpp's real -DFOO_BAR",
            why: "the inverse the underscore widening had to survive: an underscore inside a \
                  glued macro name buys nothing, because condition 5 still reads the shout",
            expect: Expect::Silent,
            raw: "  -DFOO_BAR   define a macro\n".to_string(),
            root: tree("cpp", vec![table_flag('D', Some("FOO_BAR"))]),
        },
        SelfCheck {
            name: "a lowercase flag letter with a shouting underscored argument",
            why: "the -oOUTFILE shape wearing an underscore: only the whole-token case test \
                  rejects it, and it is why condition 5 is not measured on the tail alone",
            expect: Expect::Silent,
            raw: "  -oOUT_FILE   write output here\n".to_string(),
            root: tree("t", vec![table_flag('o', Some("OUT_FILE"))]),
        },
        SelfCheck {
            name: "a spaced underscored value",
            why: "-o out_file stores byte-for-byte what a glued -oout_file would; condition 7's \
                  scan of the raw text is the only thing that tells them apart",
            expect: Expect::Silent,
            raw: "  -o out_file   write output here\n".to_string(),
            root: tree("t", vec![table_flag('o', Some("out_file"))]),
        },
        SelfCheck {
            name: "gcc's real -Wl,<options>",
            why: "cross-check that the = split changed nothing for tails with no =: the comma \
                  still fails is_option_name_tail before case is consulted",
            expect: Expect::Silent,
            raw: "  -Wl,<options>            Pass comma-separated <options> on to the linker.\n"
                .to_string(),
            root: tree("gcc", vec![table_flag('W', Some("l,<options>"))]),
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

    fn report(raw: &str, name: &str, flags: Vec<Entity>) -> SplitReport {
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
        assert!(!is_option_name_tail(""));
        // `_` is a word separator inside a name, on the same footing as
        // `-` — `dbiprof`'s `-case_sensitive`, ffmpeg's `-pix_fmts`.
        assert!(is_option_name_tail("a_b"));
        assert!(is_option_name_tail("ase_sensitive"));
        assert!(is_option_name_tail("_err_detect"));
        // Still no letter, still a glued numeric argument.
        assert!(!is_option_name_tail("_42"));
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
        flag.spellings.push(mandible_core::Spelling::long("help"));
        let r = report(raw, "t", vec![flag]);
        assert_eq!(r.split_count(), 0);
    }

    #[test]
    fn a_subcommands_own_split_is_reported_at_its_own_path() {
        let raw = "usage: t sub -help\n";
        let mut root = CommandNode::new("t", Provenance::single(Source::HelpText));
        let mut sub = CommandNode::new("sub", Provenance::single(Source::HelpText));
        sub.entities.push(table_flag('h', Some("elp")));
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

    // --- the fix reaches this detector's zero ----------------------------

    /// The assertion the ratchet actually rests on, and the one thing
    /// neither a fleet count nor a self-check can state on its own: run the
    /// **real extraction pipeline** over the real captured bytes of every
    /// audited fixture and confirm this detector finds nothing left.
    ///
    /// A ratchet gated on `count == 0` is satisfied by a broken detector,
    /// which is why `detector::ratchet_at_zero` demands the self-checks too.
    /// But the self-checks run against *hand-built* trees, so they would go
    /// on holding if `sections::repair_single_dash_long_options` were
    /// deleted tomorrow — the detector would still fire on its own fixtures
    /// and the fleet count would climb back to 8,784 with nothing in the
    /// test suite noticing until someone ran a twenty-minute sweep. This
    /// test is the missing link: it replays frozen bytes through the parser
    /// the fix lives in, so deleting the fix fails here, immediately, with
    /// the tool and token named.
    ///
    /// Zero subprocesses — `corpus::replay_version` is the same frozen-bytes
    /// replay `detector::calibrate` uses.
    #[test]
    fn the_real_parser_leaves_no_split_in_any_audited_fixture() {
        let corpus_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../corpus");
        let replayed = crate::corpus::replay_version(&corpus_root, "audit-seed2")
            .expect("real corpus replays");
        assert!(
            replayed.len() > 20,
            "the audited fixture set should be substantial, got {}",
            replayed.len()
        );
        let mut offenders = Vec::new();
        for fixture in &replayed {
            let Some(root) = &fixture.root else { continue };
            for split in detect(&fixture.raw, root).splits {
                offenders.push(format!(
                    "{}: {} split into {}",
                    fixture.tool, split.token, split.spelling
                ));
            }
        }
        assert!(
            offenders.is_empty(),
            "the single-dash long-option repair no longer reaches this detector's zero — \
             the family is ratcheted at 0 in main.rs and these are live splits:\n  {}",
            offenders.join("\n  ")
        );
    }

    /// `qemu-arm64-static` from the other side: the fixture that carried
    /// this family's `[xfail]` must now produce the eleven real names, and
    /// must still produce the genuine value-taking short flags that sit on
    /// adjacent rows of the same table. A repair that reached zero by
    /// deleting flags would pass the test above and fail this one.
    #[test]
    fn qemus_long_options_and_its_valued_shorts_both_survive_the_repair() {
        let corpus_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../corpus");
        let replayed = crate::corpus::replay_version(&corpus_root, "audit-seed2")
            .expect("real corpus replays");
        let qemu = replayed
            .iter()
            .find(|f| f.tool == "qemu-arm64-static")
            .expect("the qemu fixture is in the corpus");
        let root = qemu.root.as_ref().expect("qemu extracts a tree");
        for name in [
            "help",
            "cpu",
            "dfilter",
            "one-insn-per-tb",
            "singlestep",
            "strace",
            "seed",
            "trace",
            "version",
            "perfmap",
            "jitdump",
        ] {
            assert!(
                root.flags()
                    .any(|f| f.long() == Some(name) && f.single_dash()),
                "-{name} is a real qemu option and must be in the tree under its own name"
            );
        }
        // The rows the repair must not touch, still carrying their values.
        for (short, value) in [
            ('g', "port"),
            ('L', "path"),
            ('s', "size"),
            ('D', "logfile"),
        ] {
            assert!(
                root.flags().any(|f| {
                    f.short() == Some(short)
                        && f.long().is_none()
                        && f.value_name.as_deref() == Some(value)
                }),
                "-{short} {value} is a correct parse and must survive untouched"
            );
        }
    }
}
