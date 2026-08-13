//! The bundled-short-flag collapse detector: the third fleet oracle, after
//! [`crate::misattribution`] (is a description attached to the right flag?)
//! and [`crate::existence`] (does this spelling occur in the tool's own
//! output at all?).
//!
//! **Its victim is the synopsis flag cluster.** A usage line that opens
//! `[-AbdDefhHIJKlLnNOpqStuUvxX#]` is naming 26 bundled boolean flags in
//! the ordinary getopt convention. `help_text::grammar::parse_flag_spec`
//! reads the token as *one* flag — `try_short` takes the first character
//! and `try_value` glues every remaining character onto it as a required
//! value — so the tree gains `-A` with `value_name:
//! "bdDefhHIJKlLnNOpqStuUvxX#"` and loses the other 25 flags entirely.
//! Five real examples, all from `audit/2.toml`'s seed-2 human review, four
//! of them judged `wrong` explicitly for this:
//!
//! | tool | raw synopsis token | parsed as | real flags destroyed |
//! |---|---|---|---|
//! | `tcpdump` | `[-AbdDefhHIJKlLnNOpqStuUvxX#]` | `-A` + value `bdDefhHIJKlLnNOpqStuUvxX#` | 25 |
//! | `xfs_io` | `[-adfinrRstVx]` | `-a` + value `dfinrRstVx` | 10 |
//! | `tmux` | `[-2CDlNuVv]` | `-2` + value `CDlNuVv` | 7 |
//! | `filefrag` | `[-BeEksvxX]` | `-B` + value `eEksvxX` | 7 |
//! | `ssh-keygen` | `[-hU]` | `-h` + value `U` | 1 |
//!
//! # Why this needs its own oracle
//!
//! [`crate::existence`] is **structurally blind** to this family, and not
//! by accident: it asks whether a flag spelling occurs literally in the raw
//! text, and a collapsed `-2` *does* occur — inside `[-2CDlNuVv]`, at a
//! clean word boundary. It attests, correctly, and the parse is still badly
//! wrong. Zero fabrications is a claim about invention, never a claim about
//! a correct parse. This module measures what that one cannot see: text
//! that was read, and read as the wrong *shape*.
//!
//! # The signature, and how it was derived
//!
//! Not from the five tools above — from the fleet. `xtask audit freeze`
//! captured every one of this machine's 2,304 `PATH` tools' raw `--help`
//! bytes, and the signature was tuned against *every glued short-flag
//! token in every synopsis on the box*, looking at both what it caught and
//! what it rejected. Two rounds of that changed the rule materially, and
//! both changes are recorded on the predicates below: a first version
//! required the cluster to be alphabetized, which the fleet says misses
//! about a third of real bundles (`tree`, `e2fsck`, `tic`, `mkfs.ext4`,
//! `badblocks`, `zipinfo` and the whole `fc-*` family all list their
//! switches unsorted), and a first version measured case-mixing across the
//! whole cluster, which admits every single-dash long option there is
//! (`cargo -Zscript`, `rpcgen -Dname`, `makewhatis -Tutf8`).
//!
//! Every condition is a discriminator against a specific real
//! counter-example, because the failure that matters here is the false
//! positive: `tmux`'s own synopsis line carries `[-c shell-command]`,
//! `[-f file]`, `[-L socket-name]`, `[-S socket-path]` and `[-T features]`
//! beside its collapsed `[-2CDlNuVv]`, and every one of those is a genuine
//! value-taking short flag. A detector that fires on those is worse than no
//! detector, because this number is meant to be ratchet-gated at zero once
//! the grammar is fixed.
//!
//! A flag is reported when **all** of these hold:
//!
//! 1. **It is synopsis-sourced** ([`mandible_core::Source::HelpTextSynopsis`]).
//!    That is the only source that produces this shape: `sections::
//!    push_usage_flag` is the sole writer, and an `Options:`-block row
//!    reaching the same grammar arrives with a description and a different
//!    source. Restricting to it also keeps the whole GCC/Clang single-dash
//!    convention (`-fdump-scos`, `-Wall`, `-cl-ext=<value>` — thousands of
//!    flags fleet-wide, every one genuinely one flag with a glued value)
//!    out of the population entirely, since those come from option tables.
//! 2. **It has a short spelling, no long name, and a `Required` value.**
//!    A collapsed cluster is one bare `-xyz` token: the grammar cannot have
//!    seen a long name in it, and `Optional` means the raw text wrote
//!    brackets (`-a[bcd]`), which is a value spec a human deliberately
//!    typed, not a bundle.
//! 3. **The cluster occurs glued in the raw text** —
//!    [`crate::existence::spelling_occurs`] against `-<short><value_name>`,
//!    i.e. the reconstructed token with no separator, delimited on both
//!    sides. This is the load-bearing separator check and it alone
//!    disposes of most of the false-positive population: `-c
//!    shell-command` stores `value_name: "shell-command"` but the raw text
//!    spells it with a space, so `-cshell-command` never occurs and the
//!    flag is never a candidate. It is also what makes the claim exact —
//!    `value_name` is stored verbatim (`grammar::try_value`), so a
//!    successful match means the reconstructed string *is* the raw token,
//!    not merely something like it.
//! 4. **Every member is a plausible single-character flag name**
//!    ([`is_bundle_member_char`]). `filefrag`'s own other option,
//!    `[-b{blocksize}[KMG]]`, is glued and synopsis-sourced and would
//!    otherwise pass — its value starts with `{`, which is not a flag name.
//! 5. **The members are pairwise distinct**, case-sensitively. A bundle is
//!    a *set* of switches; `-Wall`'s doubled `l` is a word.
//! 6. **Either** the members are alphabetized ([`is_alphabetized`]) **or**
//!    the swallowed members alone span both cases
//!    ([`swallowed_members_mix_case`]). Two independent pieces of evidence
//!    for "this is a switch set, not a word", each measured against the
//!    fleet, each carrying a large family the other cannot see — the
//!    predicates' own doc comments name both families and both
//!    counter-example sets.
//! 7. **At least [`MIN_BUNDLED_MEMBERS`] members are being swallowed.**
//!
//! # Three defect families share this structural fingerprint
//!
//! `short && !long && value_name` — a bare short flag carrying a value —
//! is not one bug. The calibration work against `audit/2.toml` found
//! **three** distinct families producing it, all filed under the same
//! `k1` label, and only the first is this module's:
//!
//! | family | example | stored as | is it this module's? |
//! |---|---|---|---|
//! | bundled shorts | `tmux [-2CDlNuVv]` | `-2` + `CDlNuVv` | **yes** |
//! | single-dash long options | `gcc -pass-exit-codes` | `-p` + `ass-exit-codes` | no |
//! | repeated-character flags | `bpftrace -vv` | `-v` + `v` | no |
//!
//! Firing on the other two would corrupt this module's fleet count in the
//! most damaging possible way — plausibly, with a number that looks like
//! evidence. The discriminator is the *shape of the swallowed text*, and
//! each family is rejected by a condition that already exists for its own
//! reasons:
//!
//! - A **single-dash long option** swallows a word: hyphens and repeated
//!   letters (condition 4 and condition 5), uniformly cased and unordered
//!   (condition 6). `-pass-exit-codes`, `-Zscript`, `-Dname`, `-Tutf8`,
//!   `-Idirectory` are all rejected several times over —
//!   `real_single_dash_long_options_stay_silent` and
//!   `a_single_dash_long_option_with_hyphens_stays_silent`.
//! - A **repeated-character flag** swallows one or more copies of the
//!   flag's own letter: `-vv` is a single swallowed member (condition 7),
//!   and `-vvv`/`-DDD` repeat it (condition 5) —
//!   `repeated_character_flags_stay_silent`.
//!
//! # What it deliberately does not catch
//!
//! `MIN_BUNDLED_MEMBERS = 2` costs a real hit — `ssh-keygen`'s `[-hU]`,
//! one of the five human-labelled examples — and it is still right: at one
//! swallowed member the shape is genuinely ambiguous, and the fleet's
//! one-member population is roughly half real collapses and half completely
//! correct parses of real multi-character single-dash flags (`rpcgen -Ss`,
//! `psfxtable -it`, `sg_map -st`, `mandoc -ac`, `which -as`, `xxd -ps`,
//! `lessecho`'s seven `[-ox]`-shaped character arguments). See that
//! constant's own doc comment for the full list.
//!
//! Two further families are knowingly out of reach, both from the fleet
//! scan:
//!
//! - **Unsorted, uniformly-cased bundles** — `rpcbind`'s `[-adhilswfr]`,
//!   `umount.nfs`'s `[-fvnrlh]`, `blkmapd`'s `[-hdf]`,
//!   `debconf-set-selections`'s `[-vcu]`, `fc-validate`'s `[-Vhv]`. Neither
//!   signal fires, and nothing distinguishes them on shape from a
//!   lowercase word.
//! - **Bundles that repeat a switch to mean "more of it"** — `strace`'s
//!   `[-ACdffhiqqrtttTvVwxxyyzZ]`, `wpa_supplicant`'s `[-BddhKLqqstuvW]`.
//!   Condition 5 rejects them by construction.
//!
//! The fleet count this module reports is therefore a **lower bound** on
//! the defect, which is the correct direction for a number that will become
//! a gate: a false negative leaves a real bug unreported, a false positive
//! blocks the fix.
//!
//! # The measurement
//!
//! A full `PATH` sweep on this aarch64 box (2,302 tools) reports **58 tools
//! with a collapse, destroying 465 real flags** — an average of 8 lost
//! flags per affected tool, and 22 at the worst (`groff`'s
//! `[-abcCeEgGijklNpRsStUVXzZ]`). Every one of the 58 was checked against
//! its own captured help text by hand and every one is a real getopt
//! bundle; no false positive was found.
//!
//! **58 is well below the 91 tools whose raw synopsis contains a bundle**,
//! and the gap is not a miss — it is the difference between the text and
//! the tree. `od`'s usage line really does say `[-abcdfilosx]`, but `od`
//! also documents `-a` in an ordinary options table, so
//! `sections::flag_spelling_already_present` drops the usage-derived flag
//! and the tree ends up correct. Same for `tree`, `whereis`, `umount`,
//! `rpcbind` and about thirty others. The defect only survives into the
//! tree when the synopsis is the *only* place the flags are named — which
//! is exactly the population this module reports, and the reason it reads
//! the tree rather than grepping the text.
//!
//! # No new probes, not gated
//!
//! Identical to [`crate::existence`] on both counts, for identical reasons:
//! it reads the same [`crate::misattribution::RecordingProbe`] capture the
//! sweep already paid for, so it costs zero additional subprocess spawns,
//! and it is reported in every scoreboard footer without ever contributing
//! to `--check`'s pass/fail decision (spec §13.1b: a metric with no measured
//! baseline must not silently fail a run the first time it is computed).

use crate::existence::spelling_occurs;
use mandible_core::{CommandNode, Flag, Provenance, Source, ValueKind};
use std::collections::HashSet;

/// The fewest swallowed members a cluster must have before it is reported.
///
/// Two, not one, and the difference is deliberate lost recall — see this
/// module's doc comment. At one swallowed member the shape is genuinely
/// ambiguous, and the fleet scan says so out loud: of the one-member
/// clusters that satisfy every other condition, roughly half are real
/// collapses (`ssh-keygen`'s `[-hU]`, `umount`'s `[-hV]`, `ssh-agent`'s
/// `[-Dd]`) and the rest are entirely correct parses of genuine
/// multi-character single-dash flags — `rpcgen`'s `[-Sc]`/`[-Ss]`/`[-Sm]`
/// (generate sample client/server/makefile), `psfxtable`'s
/// `[-it]`/`[-ot]`/`[-nt]` (input/output/new table), `sg_map`'s `[-st]`,
/// `setfont`'s `[-ou]`, `mandoc`'s `[-ac]`, `which`'s `[-as]`, `xxd`'s
/// `[-ps]`, plus `lessecho`'s seven character-argument flags. Nothing about
/// their *shape* separates the two halves. This detector is meant to be
/// gated at zero, and a gate that fires on a correct parse cannot be fixed
/// by anyone, so the whole class is excluded.
const MIN_BUNDLED_MEMBERS: usize = 2;

/// The fewest ASCII letters a cluster must carry before [`is_alphabetized`]
/// is allowed to vouch for it.
///
/// A cluster with no letters at all — `-1024`, `-0777` — is *vacuously*
/// alphabetized, and a glued numeric default (`[-b4096]`) would ride that
/// vacuous truth straight into the report. Two letters is the floor at
/// which "the letters are in order" is a statement about anything; every
/// real bundle in the fleet clears it by a wide margin (the narrowest,
/// `xfs_copy`'s `[-bdV]`, carries three).
const MIN_ORDERED_LETTERS: usize = 2;

/// Whether `c` could be a single-character flag name, i.e. a plausible
/// member of a bundle.
///
/// ASCII alphanumeric covers every letter and digit case observed
/// (`tmux`'s `-2` is a real digit flag). `#` is the one non-alphanumeric
/// member seen in the corpus — the last character of `tcpdump`'s
/// `[-AbdDefhHIJKlLnNOpqStuUvxX#]`, which is `tcpdump`'s real
/// "print packet number" switch. Nothing else is admitted: the point of
/// this predicate is to reject a *value spec*, and every value spec
/// punctuation character (`{`, `<`, `[`, `=`, `:`, `.`, `-`, `_`, `/`, `|`)
/// is exactly what it rejects — `filefrag`'s own `[-b{blocksize}[KMG]]`
/// fails here and nowhere else.
fn is_bundle_member_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '#'
}

/// True when `members` carries at least [`MIN_ORDERED_LETTERS`] ASCII
/// letters and all of them are in non-decreasing case-insensitive order —
/// the *listing* convention a hand-written flag bundle follows, and one
/// half of the two-signal test in this module's doc comment.
///
/// Case is folded rather than compared raw because the convention
/// interleaves the two cases of the same letter (`hH`, `lL`, `uU`, `xX`,
/// `Vv`) — a raw ASCII comparison would call every one of those a break in
/// the order.
///
/// **Non-letters are skipped rather than ordered.** `tcpdump` parks its
/// `#` at the end of an otherwise perfectly alphabetical bundle, and
/// `tmux` parks its `2` at the front; requiring those to sort with the
/// letters would reject both real cases while adding nothing.
///
/// It is the *only* signal that vouches for a uniformly-cased bundle, and
/// that is what it is for. Nine real all-one-case bundles in the fleet scan
/// have nothing else going for them: `od`'s `[-abcdfilosx]`, `pod2text`'s
/// `[-aclostu]`, `showmount`'s `[-adehv]`, `e2image`'s `[-cfnp]`,
/// `whereis`'s `[-BMS]`, `vm-support`'s `[-hux]`, `sm-notify`'s `[-dfq]`,
/// `logsave`'s `[-asv]`, `grops`'s `[-glm]`. Against word-shaped values it
/// stays quiet on its own terms: `-oOUTFILE`, `-Wall`, `-Ipath`,
/// `-DMACRO`, `cargo`'s real `-Zscript`, `rpcgen`'s real `-Dname` and
/// `makewhatis`'s real `-Tutf8` are every one of them unordered.
fn is_alphabetized(members: &str) -> bool {
    let letters = || members.chars().filter(|c| c.is_ascii_alphabetic());
    if letters().count() < MIN_ORDERED_LETTERS {
        return false;
    }
    let mut previous: Option<char> = None;
    for c in letters() {
        let folded = c.to_ascii_lowercase();
        if let Some(prev) = previous {
            if folded < prev {
                return false;
            }
        }
        previous = Some(folded);
    }
    true
}

/// True when `swallowed` — the members after the surviving first one, i.e.
/// the stored `value_name` — contains both an ASCII uppercase and an ASCII
/// lowercase letter. The other half of the two-signal test.
///
/// A value placeholder is written in one case (`file`, `size`, `mode`,
/// `prog`, `rounds`, `bits`, `OUTFILE`, `MACRO`), so a *swallowed* run that
/// spans both cases is not a placeholder at all — it is a switch set that
/// inherited whatever cases its tool's flags happen to have.
///
/// This is the signal that carries every real bundle whose author listed
/// the switches *unsorted*, which the fleet scan puts at roughly a third of
/// them and which [`is_alphabetized`] alone would have missed entirely:
/// `tree`'s `[-acdfghilnpqrstuvxACDFJQNSUX]` (a sorted lowercase run, then
/// a sorted uppercase one — sorted twice, so not sorted), `e2fsck`'s
/// `[-panyrcdfktvDFV]`, `tic`'s `[-1aCDcfGgIKLNrsTtUx]`, `mkfs.ext4`'s
/// `[-jnqvDFSV]`, `dumpe2fs`'s `[-bfghimxV]`, `badblocks`'s `[-svwnfBX]`,
/// `zipinfo`'s `[-12smlvChMtTz]`, and the whole `fc-*` family's `[-...Vh]`.
///
/// Measured on the *swallowed* half rather than on the whole cluster
/// deliberately, and the difference is the entire single-dash-long-option
/// population: `cargo -Zscript`, `rpcgen -Dname`, `makewhatis -Tutf8` and
/// `perl -Idirectory` all mix case *as clusters* (an uppercase flag letter
/// with a lowercase word glued on) and are all completely correct parses.
/// Their swallowed halves — `script`, `name`, `utf8`, `directory` — do not
/// mix, and that is what tells them apart from a bundle.
fn swallowed_members_mix_case(swallowed: &str) -> bool {
    swallowed.chars().any(|c| c.is_ascii_uppercase())
        && swallowed.chars().any(|c| c.is_ascii_lowercase())
}

/// True when every character of `cluster` is distinct, compared
/// case-sensitively.
///
/// A bundle is a *set* of switches, so it never repeats one. Case matters:
/// `-v` and `-V` are different flags and real bundles carry both (`Vv` in
/// `tmux`, `uU`/`xX`/`hH`/`lL` in `tcpdump`), so folding case here would
/// reject the very cases this module exists for. Against words it is a
/// weak filter on its own (`file`, `size` and `mode` all have distinct
/// letters) and a decisive one against the commonest doubled-letter
/// placeholders (`-Wall`, `-nn`, `-ldl`).
fn members_are_distinct(cluster: &str) -> bool {
    let mut seen = HashSet::new();
    cluster.chars().all(|c| seen.insert(c))
}

/// Whether `provenance` credits the usage-synopsis grammar — the only
/// source that produces the collapse (this module's doc comment, condition
/// 1). Deliberately narrower than [`crate::existence`]'s own
/// `is_help_text_sourced`, which accepts plain `HelpText` too: an option
/// *table* row that glues a value onto a short flag is the GCC convention
/// and is genuinely one flag, so admitting that source would put thousands
/// of correct parses into this population for no gain.
fn is_synopsis_sourced(provenance: &Provenance) -> bool {
    provenance
        .sources
        .iter()
        .any(|s| matches!(s, Source::HelpTextSynopsis))
}

/// One synopsis flag cluster read as a single value-taking flag.
pub struct Collapse {
    /// Space-separated path to the node owning the flag, e.g. `"tcpdump"`.
    pub path: String,
    /// The surviving flag's spelling, e.g. `"-A"` — the one member of the
    /// cluster that reached the tree at all, and it reached it with the
    /// wrong shape (a required value it does not take).
    pub spelling: String,
    /// The whole raw token this collapsed from, e.g.
    /// `"-AbdDefhHIJKlLnNOpqStuUvxX#"`.
    pub cluster: String,
    /// How many real flags this collapse destroyed: every member after the
    /// first, i.e. the character count of the swallowed value. `tcpdump`'s
    /// 26-member cluster destroys 25.
    pub destroyed: usize,
}

/// The result of analyzing one tool.
pub struct BundleReport {
    pub collapses: Vec<Collapse>,
}

impl BundleReport {
    /// How many collapsed clusters this tool has (one per surviving flag).
    pub fn collapse_count(&self) -> usize {
        self.collapses.len()
    }

    /// How many real flags those collapses destroyed in total — the number
    /// that says how much of the fleet's recall this one defect costs,
    /// which `collapse_count` alone badly understates (`tcpdump` is one
    /// collapse and 25 lost flags).
    pub fn destroyed_flag_count(&self) -> usize {
        self.collapses.iter().map(|c| c.destroyed).sum()
    }
}

/// Whether `flag` is a collapsed bundle, and the raw cluster it collapsed
/// from — `None` when any condition in this module's doc comment fails.
///
/// Split out from [`detect`]'s walk so the seven conditions are readable in
/// one place and testable one at a time.
fn collapsed_cluster(flag: &Flag, raw: &str) -> Option<String> {
    // 1. Synopsis-sourced.
    if !is_synopsis_sourced(&flag.provenance) {
        return None;
    }
    // 2. A bare short flag carrying a required value.
    let short = flag.short?;
    if flag.long.is_some() || flag.value_kind != ValueKind::Required {
        return None;
    }
    let value = flag.value_name.as_deref()?;
    // 7. Enough swallowed members to be unambiguous.
    if value.chars().count() < MIN_BUNDLED_MEMBERS {
        return None;
    }
    // 4. Every swallowed member is a plausible flag name. (Checked on the
    //    value rather than the whole cluster only because `short` is
    //    already known to be a flag name — the grammar read it as one.)
    if !value.chars().all(is_bundle_member_char) {
        return None;
    }
    let cluster = format!("-{short}{value}");
    let members: String = cluster.chars().skip(1).collect();
    // 5.
    if !members_are_distinct(&members) {
        return None;
    }
    // 6. Either signal is enough; neither on its own would see the whole
    //    family (see both predicates' doc comments for the measured halves).
    if !is_alphabetized(&members) && !swallowed_members_mix_case(value) {
        return None;
    }
    // 3. The whole cluster occurs, glued and delimited, in the raw text.
    //    Last because it is the only condition that scans the document.
    if !spelling_occurs(raw, &cluster) {
        return None;
    }
    Some(cluster)
}

fn walk(node: &CommandNode, path: &str, raw: &str, out: &mut Vec<Collapse>) {
    for flag in &node.flags {
        let Some(cluster) = collapsed_cluster(flag, raw) else {
            continue;
        };
        let Some(short) = flag.short else {
            continue;
        };
        out.push(Collapse {
            path: path.to_string(),
            spelling: format!("-{short}"),
            destroyed: cluster.chars().count() - 2,
            cluster,
        });
    }
    for child in &node.subcommands {
        let child_path = format!("{path} {}", child.name);
        walk(child, &child_path, raw, out);
    }
}

/// Analyze `root`'s synopsis-sourced flags against `raw` (the same raw
/// `--help`/`-h` text [`crate::misattribution::RecordingProbe::root_help_text`]
/// hands back) for the bundled-short-flag collapse.
///
/// Same shape and same two arguments as [`crate::existence::detect`], so
/// the two are interchangeable to a caller that wants to run every oracle
/// over one capture.
pub fn detect(raw: &str, root: &CommandNode) -> BundleReport {
    let mut collapses = Vec::new();
    walk(root, &root.name, raw, &mut collapses);
    BundleReport { collapses }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandible_core::Provenance;

    /// A short flag carrying `value`, sourced from `source` — built the
    /// way `mandible_core::Flag`'s own constructors allow (there is only a
    /// `Flag::long`), then corrected, exactly as
    /// `crate::existence`'s own test helper does.
    fn short_flag(short: char, value: Option<&str>, source: Source) -> Flag {
        let mut flag = Flag::long("", Provenance::single(source));
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

    /// A synopsis-sourced flag exactly as `sections::push_usage_flag`
    /// builds it: short spelling, no long name, a required value, no
    /// description (a usage line has none to give).
    fn synopsis_flag(short: char, value: Option<&str>) -> Flag {
        short_flag(short, value, Source::HelpTextSynopsis)
    }

    fn node(name: &str) -> CommandNode {
        CommandNode::new(name, Provenance::single(Source::HelpTextSynopsis))
    }

    /// Build a one-node tree carrying `flags` and run [`detect`] over it.
    fn report(raw: &str, name: &str, flags: Vec<Flag>) -> BundleReport {
        let mut root = node(name);
        root.flags = flags;
        detect(raw, &root)
    }

    // --- the five known-bad tools, byte-exact from their corpus captures -

    /// `tcpdump --help`'s real fourth line, byte-exact from
    /// `corpus/tcpdump/audit-seed2/help.txt`. 26 members, one flag.
    const TCPDUMP_USAGE: &str =
        "Usage: tcpdump [-AbdDefhHIJKlLnNOpqStuUvxX#] [ -B size ] [ -c count ] [--count]\n";

    /// `tmux`'s real usage line, byte-exact from
    /// `corpus/tmux/audit-seed2/help.stderr.txt` — the whole reason the
    /// false-positive side of this detector matters, since five genuine
    /// value-taking short flags sit on the same line as the collapse.
    const TMUX_USAGE: &str = "usage: tmux [-2CDlNuVv] [-c shell-command] [-f file] [-L socket-name]\n            [-S socket-path] [-T features] [command [flags]]\n";

    /// `filefrag`'s real usage line, byte-exact. Carries the collapse
    /// *and* `[-b{blocksize}[KMG]]`, a glued synopsis value that must not
    /// fire.
    const FILEFRAG_USAGE: &str =
        "Usage: /usr/sbin/filefrag [-b{blocksize}[KMG]] [-BeEksvxX] file ...\n";

    /// `xfs_io`'s real usage line, byte-exact.
    const XFS_IO_USAGE: &str =
        "Usage: xfs_io [-adfinrRstVx] [-m mode] [-p prog] [[-c|-C] cmd]... file\n";

    /// `lessecho`'s real usage line, byte-exact from its own `--help`.
    /// Seven genuine value-taking glued short flags (its man page: `x` is
    /// a character, `n` a number) — the closest real thing to a false
    /// positive this detector has, and it must stay silent on all seven.
    const LESSECHO_USAGE: &str =
        "usage: lessecho [-ox] [-cx] [-pn] [-dn] [-mx] [-nn] [-ex] [-a] file ...\n";

    #[test]
    fn detects_tcpdumps_real_twenty_five_member_cluster() {
        let r = report(
            TCPDUMP_USAGE,
            "tcpdump",
            vec![synopsis_flag('A', Some("bdDefhHIJKlLnNOpqStuUvxX#"))],
        );
        assert_eq!(r.collapse_count(), 1);
        assert_eq!(r.collapses[0].spelling, "-A");
        assert_eq!(r.collapses[0].cluster, "-AbdDefhHIJKlLnNOpqStuUvxX#");
        assert_eq!(r.destroyed_flag_count(), 25);
    }

    #[test]
    fn detects_tmuxs_real_cluster_without_touching_its_five_real_valued_flags() {
        let r = report(
            TMUX_USAGE,
            "tmux",
            vec![
                synopsis_flag('2', Some("CDlNuVv")),
                synopsis_flag('c', Some("shell-command")),
                synopsis_flag('f', Some("file")),
                synopsis_flag('L', Some("socket-name")),
                synopsis_flag('S', Some("socket-path")),
                synopsis_flag('T', Some("features")),
            ],
        );
        assert_eq!(
            r.collapse_count(),
            1,
            "only the cluster may fire: {:?}",
            r.collapses.iter().map(|c| &c.cluster).collect::<Vec<_>>()
        );
        assert_eq!(r.collapses[0].cluster, "-2CDlNuVv");
        assert_eq!(r.destroyed_flag_count(), 7);
    }

    #[test]
    fn detects_filefrags_real_cluster_but_not_its_braced_block_size_value() {
        let r = report(
            FILEFRAG_USAGE,
            "filefrag",
            vec![
                synopsis_flag('b', Some("{blocksize}[KMG]")),
                synopsis_flag('B', Some("eEksvxX")),
            ],
        );
        assert_eq!(r.collapse_count(), 1);
        assert_eq!(r.collapses[0].cluster, "-BeEksvxX");
        assert_eq!(r.destroyed_flag_count(), 7);
    }

    #[test]
    fn detects_xfs_ios_real_cluster_but_not_its_spaced_values() {
        let r = report(
            XFS_IO_USAGE,
            "xfs_io",
            vec![
                synopsis_flag('a', Some("dfinrRstVx")),
                synopsis_flag('m', Some("mode")),
                synopsis_flag('p', Some("prog")),
            ],
        );
        assert_eq!(r.collapse_count(), 1);
        assert_eq!(r.collapses[0].cluster, "-adfinrRstVx");
        assert_eq!(r.destroyed_flag_count(), 10);
    }

    /// The documented, deliberate false negative: `ssh-keygen`'s `[-hU]`
    /// is a genuine collapse a human labelled `wrong`, and it is below
    /// [`MIN_BUNDLED_MEMBERS`]. Asserted, not merely described, so that
    /// lowering the threshold has to come with a decision about
    /// `lessecho` and `-jN` rather than happening by accident.
    #[test]
    fn ssh_keygens_one_member_cluster_is_deliberately_not_reported() {
        let raw =
            "       ssh-keygen -I certificate_identity -s ca_key [-hU] [-D pkcs11_provider]\n";
        let r = report(raw, "ssh-keygen", vec![synopsis_flag('h', Some("U"))]);
        assert_eq!(r.collapse_count(), 0);
    }

    /// The unsorted-bundle branch, on real fleet text: `tree`'s synopsis
    /// lists its lowercase switches in order and then its uppercase ones in
    /// (their own, also broken) order, so the cluster is not alphabetized
    /// at all. Only [`swallowed_members_mix_case`] sees it, and 27 real
    /// flags ride on that.
    #[test]
    fn detects_trees_real_unsorted_bundle() {
        let raw = "usage: tree [-acdfghilnpqrstuvxACDFJQNSUX] [--inodes] [-L level [-R]]\n";
        let value = "cdfghilnpqrstuvxACDFJQNSUX";
        let r = report(raw, "tree", vec![synopsis_flag('a', Some(value))]);
        assert!(!is_alphabetized(&format!("a{value}")));
        assert_eq!(r.collapse_count(), 1);
        assert_eq!(r.destroyed_flag_count(), 26);
    }

    /// The alphabetized branch, on real fleet text: `od`'s traditional
    /// switches are all lowercase, so nothing about their case is evidence
    /// of anything. Only [`is_alphabetized`] sees it.
    #[test]
    fn detects_ods_real_uniformly_cased_bundle() {
        let raw = "Usage: od [-abcdfilosx]... [FILE]... [[+]OFFSET[.][b]]\n";
        let value = "bcdfilosx";
        let r = report(raw, "od", vec![synopsis_flag('a', Some(value))]);
        assert!(!swallowed_members_mix_case(value));
        assert_eq!(r.collapse_count(), 1);
        assert_eq!(r.destroyed_flag_count(), 9);
    }

    // --- the false-positive side, which is the side that matters ---------

    #[test]
    fn a_spaced_value_never_fires_however_bundle_shaped_it_looks() {
        // The separator check alone: `-c shell-command` is stored with a
        // `value_name` just like a cluster is, and the *only* thing that
        // distinguishes it in the raw text is the space.
        let raw = "usage: t [-c shell-command]\n";
        let r = report(raw, "t", vec![synopsis_flag('c', Some("shell-command"))]);
        assert_eq!(r.collapse_count(), 0);
        // ...and the same flag *does* fire once the raw text glues it,
        // confirming the space is what was doing the work above rather
        // than some other condition failing silently.
        let glued = "usage: t [-cDeF]\n";
        let r = report(glued, "t", vec![synopsis_flag('c', Some("DeF"))]);
        assert_eq!(r.collapse_count(), 1);
    }

    #[test]
    fn lessechos_seven_real_glued_values_all_stay_silent() {
        let flags = vec![
            synopsis_flag('o', Some("x")),
            synopsis_flag('c', Some("x")),
            synopsis_flag('p', Some("n")),
            synopsis_flag('d', Some("n")),
            synopsis_flag('m', Some("x")),
            synopsis_flag('n', Some("n")),
            synopsis_flag('e', Some("x")),
        ];
        let r = report(LESSECHO_USAGE, "lessecho", flags);
        assert_eq!(
            r.collapse_count(),
            0,
            "lessecho's genuine character-argument flags must not fire: {:?}",
            r.collapses.iter().map(|c| &c.cluster).collect::<Vec<_>>()
        );
    }

    #[test]
    fn an_uppercase_placeholder_glued_to_a_lowercase_flag_stays_silent() {
        // `-oOUTFILE`: distinct, alphanumeric, glued, synopsis-sourced, and
        // case-mixing *as a cluster* — the shape that made the first
        // version of this rule wrong. Both surviving signals reject it: the
        // swallowed half is uniformly uppercase, and the letters are not in
        // order.
        let raw = "usage: t [-oOUTFILE]\n";
        let r = report(raw, "t", vec![synopsis_flag('o', Some("OUTFILE"))]);
        assert_eq!(r.collapse_count(), 0);
        assert!(!is_alphabetized("oOUTFILE"));
        assert!(!swallowed_members_mix_case("OUTFILE"));
    }

    /// The single-dash long option, which is the largest genuinely-correct
    /// population sharing this shape — every one of these is real, glued,
    /// synopsis-sourced, distinct and case-mixing as a whole cluster, and
    /// every one is rejected because its *swallowed* half is one word in
    /// one case.
    #[test]
    fn real_single_dash_long_options_stay_silent() {
        for (short, value) in [
            ('Z', "script"), // cargo -Zscript
            ('D', "name"),   // rpcgen -Dname
            ('T', "utf8"),   // makewhatis -Tutf8
            ('O', "level"),  // find -Olevel
            ('S', "cd"),     // sg_map -scd, minus its own repeat
        ] {
            let cluster = format!("-{short}{value}");
            let raw = format!("usage: t [{cluster}]\n");
            let r = report(&raw, "t", vec![synopsis_flag(short, Some(value))]);
            assert_eq!(r.collapse_count(), 0, "{cluster} must not fire");
        }
    }

    #[test]
    fn an_ordered_lowercase_word_value_only_survives_on_its_own_shape() {
        // `cost`, `first`, `int` are ordered, distinct, alphanumeric words.
        // Ordering is one of the two accepted signals, so these *do* fire —
        // which is the honest cost of the signal that catches `od`'s
        // `[-abcdfilosx]`. Asserted rather than hidden: a value placeholder
        // whose letters happen to be alphabetical is indistinguishable, on
        // shape, from a small alphabetized bundle.
        let raw = "usage: t [-cost]\n";
        let r = report(raw, "t", vec![synopsis_flag('c', Some("ost"))]);
        assert_eq!(r.collapse_count(), 1);
        // The commoner shapes are still rejected — an unordered word, and
        // any word that repeats a letter, which most do.
        for (short, value) in [('n', "ame"), ('f', "ile"), ('m', "ode"), ('s', "ize")] {
            let cluster = format!("-{short}{value}");
            let raw = format!("usage: t [{cluster}]\n");
            let r = report(&raw, "t", vec![synopsis_flag(short, Some(value))]);
            assert_eq!(r.collapse_count(), 0, "{cluster} must not fire");
        }
    }

    // --- the other two families with the same fingerprint ----------------

    /// Family 2 of the three that produce `short && !long && value_name`
    /// (this module's doc comment): a single-dash *long option*, whose
    /// swallowed text is a hyphenated word. Byte-exact from `gcc --help`.
    /// It is rejected on the value's own shape — hyphens are not flag
    /// names — so the rejection does not depend on the source check,
    /// which is asserted here by giving it the synopsis source it would
    /// never really have.
    #[test]
    fn a_single_dash_long_option_with_hyphens_stays_silent() {
        let raw = "  -pass-exit-codes         Exit with highest error code from a phase.\n";
        let r = report(raw, "gcc", vec![synopsis_flag('p', Some("ass-exit-codes"))]);
        assert_eq!(r.collapse_count(), 0);
        assert!(!"ass-exit-codes".chars().all(is_bundle_member_char));
    }

    /// Family 3: a flag repeated to mean "more of it". `bpftrace -vv` is
    /// stored as `-v` carrying the value `v`, which is one swallowed
    /// member ([`MIN_BUNDLED_MEMBERS`]); `-vvv` and `strace`'s real
    /// `[-DDD]` carry two, and are rejected by distinctness instead. Both
    /// halves are asserted because they are rejected by *different*
    /// conditions, and a change to either one could silently admit this
    /// whole family.
    #[test]
    fn repeated_character_flags_stay_silent() {
        let raw = "usage: t [-v] [-vv] [-vvv] [-dd] [-DDD]\n";
        for (short, value) in [('v', "v"), ('d', "d")] {
            let r = report(raw, "t", vec![synopsis_flag(short, Some(value))]);
            assert_eq!(r.collapse_count(), 0, "-{short}{value} must not fire");
        }
        for (short, value) in [('v', "vv"), ('D', "DD")] {
            let r = report(raw, "t", vec![synopsis_flag(short, Some(value))]);
            assert_eq!(r.collapse_count(), 0, "-{short}{value} must not fire");
            assert!(!members_are_distinct(&format!("{short}{value}")));
        }
    }

    /// A numeric run orders vacuously — there are no letters to be out of
    /// order — so [`MIN_ORDERED_LETTERS`] is what stops a glued numeric
    /// default from riding that vacuous truth into the report.
    #[test]
    fn a_glued_numeric_default_stays_silent() {
        let raw = "usage: t [-b1024]\n";
        let r = report(raw, "t", vec![synopsis_flag('b', Some("1024"))]);
        assert_eq!(r.collapse_count(), 0);
        assert!(!is_alphabetized("b1024"));
    }

    #[test]
    fn a_gcc_style_single_dash_flag_stays_silent_on_source_alone() {
        // `-fdump-scos` is one real flag with a glued value. It fails
        // several conditions (the `-` is not a member character, the
        // letters are unordered), but the first and most important is
        // that it is an option-table row, never a synopsis token.
        let raw = "  -fdump-scos                 [available in Ada]\n";
        let flag = short_flag('f', Some("dump-scos"), Source::HelpText);
        let r = report(raw, "gcc", vec![flag]);
        assert_eq!(r.collapse_count(), 0);
    }

    #[test]
    fn a_doubled_letter_value_stays_silent() {
        // `-Wall`: alphabetized (`a`, `l`, `l`), alphanumeric, glued.
        // Only distinctness rejects it.
        let raw = "usage: t [-Wall]\n";
        let r = report(raw, "t", vec![synopsis_flag('W', Some("all"))]);
        assert_eq!(r.collapse_count(), 0);
        assert!(!members_are_distinct("Wall"));
    }

    #[test]
    fn a_bracketed_optional_value_stays_silent() {
        // `-a[bcD]` — brackets a human typed deliberately, which the
        // grammar records as `ValueKind::Optional`. Nothing about a
        // bundle is optional.
        let raw = "usage: t [-a[bcD]]\n";
        let mut flag = synopsis_flag('a', Some("bcD"));
        flag.value_kind = ValueKind::Optional;
        let r = report(raw, "t", vec![flag]);
        assert_eq!(r.collapse_count(), 0);
    }

    #[test]
    fn a_flag_carrying_a_long_name_stays_silent() {
        // A cluster token has no room for a long name; a flag that has
        // one was read from something else entirely.
        let raw = "usage: t [-aBcD]\n";
        let mut flag = synopsis_flag('a', Some("BcD"));
        flag.long = Some("alpha".to_string());
        let r = report(raw, "t", vec![flag]);
        assert_eq!(r.collapse_count(), 0);
    }

    #[test]
    fn a_value_with_punctuation_stays_silent() {
        for value in ["a:B", "a=B", "a<B>", "a_B", "a.B", "a/B"] {
            let cluster = format!("-x{value}");
            let raw = format!("usage: t [{cluster}]\n");
            let r = report(&raw, "t", vec![synopsis_flag('x', Some(value))]);
            assert_eq!(r.collapse_count(), 0, "{cluster} must not fire");
        }
    }

    #[test]
    fn a_boolean_short_flag_with_no_value_stays_silent() {
        let raw = "usage: t [-q] [-v]\n";
        let r = report(
            raw,
            "t",
            vec![synopsis_flag('q', None), synopsis_flag('v', None)],
        );
        assert_eq!(r.collapse_count(), 0);
    }

    #[test]
    fn a_cluster_that_does_not_occur_in_the_raw_text_stays_silent() {
        // The reconstruction has to match the document, not merely the
        // stored fields — otherwise this would report on a tree whose
        // provenance says synopsis but whose text says otherwise.
        let raw = "usage: t [-q]\n";
        let r = report(raw, "t", vec![synopsis_flag('a', Some("bCd"))]);
        assert_eq!(r.collapse_count(), 0);
    }

    // --- predicates, one at a time ---------------------------------------

    #[test]
    fn is_alphabetized_folds_case_and_skips_non_letters() {
        assert!(is_alphabetized("AbdDefhHIJKlLnNOpqStuUvxX#"));
        assert!(is_alphabetized("2CDlNuVv"));
        assert!(is_alphabetized("BeEksvxX"));
        assert!(is_alphabetized("adfinrRstVx"));
        assert!(!is_alphabetized("verbose"));
        assert!(!is_alphabetized("socket-name"));
        // Fewer than `MIN_ORDERED_LETTERS` letters is not an ordering claim.
        assert!(!is_alphabetized("1024"));
        assert!(!is_alphabetized("b1024"));
    }

    #[test]
    fn swallowed_members_mix_case_rejects_a_uniformly_cased_placeholder() {
        assert!(swallowed_members_mix_case("CDlNuVv"));
        assert!(!swallowed_members_mix_case("file"));
        assert!(!swallowed_members_mix_case("OUTFILE"));
        assert!(!swallowed_members_mix_case("1234"));
        // The distinction the whole-cluster version got wrong: `script` is
        // `cargo -Zscript`'s swallowed half, and it does not mix.
        assert!(!swallowed_members_mix_case("script"));
    }

    #[test]
    fn is_bundle_member_char_admits_only_flag_names() {
        for c in ['a', 'Z', '2', '#'] {
            assert!(is_bundle_member_char(c), "{c} should be a member");
        }
        for c in ['{', '<', '[', '=', ':', '.', '-', '_', '/', '|', ' '] {
            assert!(!is_bundle_member_char(c), "{c} should not be a member");
        }
    }

    // --- tree walking ------------------------------------------------------

    #[test]
    fn a_subcommands_own_cluster_is_reported_at_its_own_path() {
        let raw = "usage: t sub [-aBcD]\n";
        let mut root = node("t");
        let mut sub = node("sub");
        sub.flags.push(synopsis_flag('a', Some("BcD")));
        root.subcommands.push(sub);
        let r = detect(raw, &root);
        assert_eq!(r.collapse_count(), 1);
        assert_eq!(r.collapses[0].path, "t sub");
    }

    // --- the real pipeline, over the committed corpus captures ------------

    /// Parse `help` through the **real** tiered pipeline — real argv
    /// construction, real grammar, real merge — by replaying it from a
    /// frozen [`Transcript`] keyed on the exact `--help` argv the help-text
    /// tier builds, exactly as `crate::corpus` replays a fixture. Spawns
    /// nothing.
    ///
    /// This exists because AGENTS.md §3.1 has a named failure for the
    /// alternative: a tier whose unit tests all passed against a mock while
    /// the tier was completely dead in the real pipeline. Every other test
    /// in this module hands [`detect`] a tree built by hand, which proves
    /// the *rule* and proves nothing about whether the parser really
    /// produces the shape the rule is written against.
    fn parse_through_the_real_pipeline(tool: &str, help: &str) -> CommandNode {
        use mandible_extract::exec::{ExecOutput, Transcript};
        use mandible_extract::{default_tiers_with_probe, ResolvedTool, Runner};
        use std::sync::Arc;

        let transcript = Transcript::new([(
            vec!["--help".to_string()],
            ExecOutput {
                stdout: help.as_bytes().to_vec(),
                stderr: Vec::new(),
                exit_code: Some(0),
                timed_out: false,
            },
        )]);
        let runner = Runner::new(default_tiers_with_probe(Arc::new(transcript)));
        let resolved = ResolvedTool {
            name: tool.to_string(),
            path: Some(std::path::PathBuf::from(format!("/corpus-replay/{tool}"))),
            version: None,
        };
        runner
            .extract_full_for(&resolved)
            .root
            .expect("the help-text tier must produce a root from a real capture")
    }

    /// `tcpdump`'s committed capture, through the real parser: the tree
    /// this asserts against is the one `mandible` would actually build, so
    /// the fields the rule reads (`HelpTextSynopsis`, no long name,
    /// `ValueKind::Required`, the verbatim `value_name`) are the parser's,
    /// not this test author's.
    #[test]
    fn the_real_parser_on_tcpdumps_real_capture_yields_exactly_one_collapse() {
        let raw = include_str!("../../corpus/tcpdump/audit-seed2/help.txt");
        let root = parse_through_the_real_pipeline("tcpdump", raw);
        let r = detect(raw, &root);
        assert_eq!(
            r.collapse_count(),
            1,
            "expected exactly tcpdump's own cluster: {:?}",
            r.collapses.iter().map(|c| &c.cluster).collect::<Vec<_>>()
        );
        assert_eq!(r.collapses[0].cluster, "-AbdDefhHIJKlLnNOpqStuUvxX#");
        assert_eq!(r.destroyed_flag_count(), 25);
    }

    /// The false-positive side, through the real parser. `tmux`'s capture
    /// is the one that matters most: five genuine value-taking short flags
    /// on the same physical line as the collapse, all five parsed into the
    /// same `short + value_name` shape the rule keys on, and only the
    /// cluster may fire.
    #[test]
    fn the_real_parser_on_tmuxs_real_capture_fires_only_on_the_cluster() {
        let raw = include_str!("../../corpus/tmux/audit-seed2/help.stderr.txt");
        let root = parse_through_the_real_pipeline("tmux", raw);
        // The five real valued flags are genuinely in the tree — otherwise
        // this test would pass for the wrong reason (nothing to fire on).
        let valued: Vec<char> = root
            .flags
            .iter()
            .filter(|f| f.short.is_some() && f.value_name.is_some())
            .filter_map(|f| f.short)
            .collect();
        for short in ['c', 'f', 'L', 'S', 'T'] {
            assert!(valued.contains(&short), "-{short} missing from {valued:?}");
        }
        let r = detect(raw, &root);
        assert_eq!(
            r.collapse_count(),
            1,
            "only the cluster may fire: {:?}",
            r.collapses.iter().map(|c| &c.cluster).collect::<Vec<_>>()
        );
        assert_eq!(r.collapses[0].cluster, "-2CDlNuVv");
    }

    /// `lessecho`'s seven genuine glued character-argument flags, through
    /// the real parser — the closest real thing to a false positive this
    /// detector has, asserted against the parser's own output rather than
    /// against a hand-built stand-in.
    #[test]
    fn the_real_parser_on_lessechos_real_synopsis_fires_on_nothing() {
        let raw = "usage: lessecho [-ox] [-cx] [-pn] [-dn] [-mx] [-nn] [-ex] [-a] file ...\n";
        let root = parse_through_the_real_pipeline("lessecho", raw);
        let r = detect(raw, &root);
        assert_eq!(
            r.collapse_count(),
            0,
            "lessecho's real flags must not fire: {:?}",
            r.collapses.iter().map(|c| &c.cluster).collect::<Vec<_>>()
        );
    }

    #[test]
    fn empty_text_and_empty_tree_report_nothing() {
        let r = detect("", &node("nothing"));
        assert_eq!(r.collapse_count(), 0);
        assert_eq!(r.destroyed_flag_count(), 0);
    }

    /// The whole of `tcpdump`'s real capture, not just its usage line:
    /// confirms nothing else in a real document fires, and that the
    /// destroyed-flag count is what the fixture's own `meta.toml` says the
    /// defect costs.
    #[test]
    fn tcpdumps_whole_real_capture_yields_exactly_one_collapse() {
        let raw = include_str!("../../corpus/tcpdump/audit-seed2/help.txt");
        let flags = vec![
            synopsis_flag('A', Some("bdDefhHIJKlLnNOpqStuUvxX#")),
            synopsis_flag('B', Some("size")),
            synopsis_flag('c', Some("count")),
            synopsis_flag('C', Some("file_size")),
            synopsis_flag('E', Some("algo:secret")),
            synopsis_flag('F', Some("file")),
            synopsis_flag('Q', Some("in|out|inout")),
            synopsis_flag('z', Some("postrotate-command")),
        ];
        let r = report(raw, "tcpdump", flags);
        assert_eq!(
            r.collapse_count(),
            1,
            "unexpected collapses: {:?}",
            r.collapses.iter().map(|c| &c.cluster).collect::<Vec<_>>()
        );
        assert_eq!(r.destroyed_flag_count(), 25);
    }
}
