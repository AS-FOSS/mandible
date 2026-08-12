//! The existence detector: the second half of what spec.md's WS4 called a
//! single "anti-fabrication oracle" and turned out, on inspection, to be
//! **two** distinct checks with two distinct victims:
//!
//! - [`crate::misattribution`] (built first): does a description belong to
//!   the flag it is attached to? Its victim was `lsof`'s three-column
//!   options table, whose second and third columns bled into the first
//!   flag's description.
//! - This module: does everything the help-text tier emits actually
//!   *occur* in the tool's own raw output — or did the parser invent it?
//!
//! **Its victim is [M-10],** this project's worst shipped defect: `tar`
//! gained 39 phantom subcommands with names like *"treat them as errors"*
//! and *"extracting (default)"* — sentence fragments off a wrapped
//! continuation line, promoted to sibling command nodes by a layout parser
//! that mistook a re-indented line of prose for a new table entry. `dd`
//! picked up 40 of its own, `less` 65, and `apt-get` collected seven words
//! straight out of its own description paragraph. Every one of those
//! shipped at a *reported* `100%` on the old `%described` column, because
//! a fabricated node's own (fabricated) flags looked exactly as
//! "described" as a real one's — [`crate::misattribution`]'s doc comment
//! makes the identical point about `lsof`'s misattributed text; this is
//! the other way the same column lies.
//!
//! Multi-word fabrications of that exact shape can no longer reach the
//! tree today — `mandible_core::is_command_name_shaped` rejects any
//! candidate name containing a space, and every tier that proposes a bare-
//! word subcommand is gated on it. What's left is the narrower, still-real
//! failure mode this module exists to catch: a single lowercase *word*
//! (indistinguishable in shape from a real command name) lifted from
//! running prose rather than a genuine table entry, or a flag spelling
//! invented rather than read. This module doesn't assume [M-10] is
//! reachable again; it checks the literal claim regardless of mechanism —
//! see [`detect`]'s own test module for a synthetic replay against
//! `corpus/tar/1.35/help.txt`, `tar`'s own real war story.
//!
//! # The rule
//!
//! > Every subcommand name and flag spelling the help-text tier emits must
//! > occur literally in the raw input — and for a subcommand, at a
//! > line-start-ish position.
//!
//! **Flags** are checked by literal substring occurrence anywhere in the
//! raw text, at a word boundary (never embedded inside a longer, unrelated
//! spelling — see [`spelling_occurs`]). A flag's own cell in real
//! `--help` output routinely glues a value spec directly onto it with no
//! separating space (`--gpg-sign[=KID]`, `--sparse-version=MAJOR[.MINOR]`),
//! so the boundary check only requires that nothing *word-shaped*
//! (alphanumeric, `-`, `_`) immediately follows the candidate spelling —
//! `[`, `=`, `,`, `.`, whitespace, and end-of-text are all valid neighbours.
//!
//! **Subcommand names** additionally require the occurrence to be the
//! first whitespace-delimited word on some physical line of the raw text
//! (after trimming only leading whitespace) — the real, measured shape of
//! every genuine command-list entry this project's own corpus carries
//! (`corpus/git/2.43.0/help.txt`: `"   clone     Clone a repository..."`,
//! one name per line, indented, nothing before it). A bare substring check
//! alone would be too weak here in the other direction from flags:
//! ordinary English words (`"list"`, `"add"`, `"get"`) are exactly the
//! words real subcommands are named, and exactly the words that turn up
//! constantly in unrelated running prose — a substring-only check would
//! wave through a name manufactured from a random sentence as long as that
//! sentence happened to contain the same word once, anywhere. Requiring
//! line-start position doesn't fully close that gap (a word-grid layout —
//! several names on one line, no single one of them at true column zero —
//! would false-positive under a *stricter* reading of "line start"; this
//! project's own corpus has no such fixture to measure against, so the
//! honest thing is to say so rather than silently harden against a case
//! never actually observed) but it is the rule spec asked for, checked
//! against the one real layout this project has actually captured.
//!
//! # Pre-normalization spellings — the part a naive comparison gets wrong
//!
//! The IR's stored form is not the input's form; comparing a stored
//! spelling against the raw text byte-for-byte produces false positives on
//! every one of these, all real, all exercised by this module's tests:
//!
//! - **Alias pairing** (`mandible_core::merge::pair_aliases`) merges a
//!   short and long row that arrived as separate items with identical
//!   descriptions into one `Flag` carrying both `short` and `long`. Each
//!   spelling is still checked independently against the *whole* raw text
//!   (not required to sit on the same line or even the same cell as its
//!   partner) — pairing only ever unifies items that came from the same
//!   raw text in the first place, so both spellings remain literally
//!   present somewhere in it; requiring adjacency would be a stronger
//!   claim than the rule needs and would false-positive on legitimately
//!   pairable rows.
//! - **Value stripping**: `--gpg-sign[=KID]` is stored as `long: "gpg-sign"`
//!   with the value spec parsed off into `value_name`/`value_kind`
//!   (`mandible-extract/src/help_text/grammar.rs`'s `try_value`). Comparing
//!   only the stripped `"gpg-sign"` against the raw text would demand an
//!   exact match that never occurs verbatim in real output. This module
//!   checks the base spelling as a *prefix* at a word boundary instead
//!   (see [`spelling_occurs`]), so `--gpg-sign[=KID]`, `--gpg-sign=KID`,
//!   and bare `--gpg-sign` all attest the same stored flag.
//! - **Negatable booleans**: `--[no-]source` is stored as `long: "source"`,
//!   `negatable: true` — `long` never contains the brackets
//!   (`mandible_core::Flag::negatable`'s own doc comment). The raw text
//!   never contains the bare substring `--source` at all in this shape; it
//!   contains `--[no-]source` (or getopt_long's shorter `--[no]source`).
//!   [`long_candidates`] builds both bracketed forms (plus the bare form,
//!   as a harmless third candidate) for a negatable flag and accepts a
//!   match against any one of them.
//! - **GCC/Clang/binutils single-dash multi-character flags**
//!   (`-fdump-scos`, `-cl-ext=<value>`, `-Wplacement-new=1`): the short-flag
//!   grammar takes exactly one character as `short` and glues everything
//!   after it onto `value_name` verbatim, so `short` alone is not the whole
//!   spelling — measured fleet-wide as this task's own real regression, not
//!   a hypothetical: a first version of this module compared only the bare
//!   `-x` form and reported 848 fabrications for `lto-dump` alone (960
//!   combined with its two symlinks), 710 for `clang`, all of them entries
//!   like `-fdump-scos` that are entirely real, just never present as the
//!   bare, isolated `-f` this module was checking for — GCC's own text
//!   never once writes `-f` on its own. [`short_candidates`] reconstructs
//!   `-x<value_name>` (and `-x=<value_name>`, covering the other branch of
//!   `grammar::try_value`) as a fallback and checks that instead — see its
//!   own doc comment, and the general lesson below.
//!
//! **The general lesson, worth stating because the next reader will be
//! tempted the same way:** [`spelling_occurs`]'s strict-prefix boundary
//! exists to stop `-v` from matching inside the unrelated, longer
//! `--verbose` — a real, necessary guard. Applied uncritically to *every*
//! short flag, the identical guard rejected `-f` matching inside
//! `-fdump-scos`, where the extra characters aren't an unrelated word at
//! all, they're the rest of the same flag a weaker (and, for this
//! convention, wrong) upstream grammar split in two. A guard that prevents
//! one false-positive class can silently manufacture a much larger one
//! elsewhere; the fix is never to weaken the guard, it's to recognize which
//! stored field the split value actually landed in and reconstruct it —
//! exactly the same "compare against the pre-normalization spelling"
//! discipline this whole section is already built on, just for the field
//! nobody had a fleet-scale counter-example for yet.
//!
//! # No new probes, not gated
//!
//! Same two properties as [`crate::misattribution`], for the same reasons —
//! see that module's doc comment in full. In short: this reuses
//! [`crate::misattribution::RecordingProbe`], so it costs zero additional
//! subprocess spawns beyond what Tier B's own root `--help`/`-h` probe
//! already pays for; and it is a brand-new metric with no fleet-wide
//! baseline, so `xtask/src/main.rs` reports it in every footer and never
//! folds it into `--check`'s pass/fail decision (spec §13.1b's metric
//! design rules: a metric nobody has measured a baseline for must not
//! silently fail a run the first time it's computed).
//!
//! # Scope: help-text tier only
//!
//! Only nodes and flags whose [`mandible_core::Provenance::sources`]
//! includes [`mandible_core::Source::HelpText`] or
//! [`mandible_core::Source::HelpTextSynopsis`] are checked. Every other
//! source — Cobra `__complete`, a completion script, a native dynamic
//! probe, a vendored catalog — is a *structural* source: its names and
//! spellings come from the tool's own machinery, not from prose, and
//! legitimately never appear in `--help` text at all (a cobra subcommand
//! can be `Hidden: true` and never printed anywhere a human reads). Checking
//! those against captured help text would be pure noise, not signal — the
//! same reasoning [`crate::misattribution`] applies to picking its own
//! index source.
//!
//! This also explains why checking the *whole* merged tree against the
//! *root's* raw text is correct rather than merely convenient: the
//! coverage sweep (`xtask::coverage::score_one`) calls
//! [`mandible_extract::Runner::extract_full`], which requests only the
//! **root** from every detecting tier (spec §5.2 step 1) — nested
//! subcommands are never independently re-probed in this pipeline path,
//! so a help-text-sourced tree's entire structure, root down to its
//! deepest node, was built from parsing that one captured string. There is
//! no second, unrecorded raw text a deeper node's fields could have
//! legitimately come from instead.

use mandible_core::{CommandNode, Flag, Provenance, Source};
use std::collections::HashSet;

/// Whether `flag_char` may not immediately follow (or precede) a candidate
/// spelling for it to count as a genuine, isolated occurrence — the same
/// "not embedded in something longer" guard on both sides. Deliberately
/// narrower than `misattribution::is_flag_char`: this only needs to reject
/// the case of one spelling being a strict prefix of a different, longer
/// one (`--foo` inside `--foobar`), not to recognize every legal short-flag
/// character shape.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '-' || c == '_'
}

/// True when `candidate` occurs in `raw` as an isolated token: nothing
/// word-shaped ([`is_word_char`]) immediately precedes or follows the
/// match. Char-indexed throughout (never a byte-offset `&str` slice,
/// AGENTS.md's rule against slicing captured tool output at a raw byte
/// offset) via `Vec<char>` windows, exactly as
/// `misattribution::cells`'s own column math is char-indexed for the same
/// reason.
///
/// This is a **prefix-tolerant** boundary, not an exact-token match: the
/// character *after* the candidate is allowed to be anything that isn't
/// word-shaped, which is what lets `--gpg-sign` (the stored, value-
/// stripped spelling) match against `--gpg-sign[=KID]` (the raw text's
/// actual spelling) — see this module's doc comment on value stripping.
fn spelling_occurs(raw: &str, candidate: &str) -> bool {
    let hay: Vec<char> = raw.chars().collect();
    let needle: Vec<char> = candidate.chars().collect();
    if needle.is_empty() || hay.len() < needle.len() {
        return false;
    }
    for start in 0..=(hay.len() - needle.len()) {
        if hay[start..start + needle.len()] != needle[..] {
            continue;
        }
        let before_ok = start == 0 || !is_word_char(hay[start - 1]);
        let end = start + needle.len();
        let after_ok = end == hay.len() || !is_word_char(hay[end]);
        if before_ok && after_ok {
            return true;
        }
    }
    false
}

/// The set of every physical line's first whitespace-delimited word (after
/// trimming only leading whitespace — see this module's doc comment on why
/// "line-start-ish" is checked this way), for the "is this subcommand name
/// where a real command-list entry actually sits" half of the rule.
///
/// Trailing `:`/`,`/`;` is stripped from that first token before it enters
/// the set — a tokenizer fix, not a loosening of "line-start-ish": a real
/// command-list row commonly glues punctuation straight onto the name with
/// no separating space (`gh --help`'s `  auth:        Authenticate gh and
/// git with GitHub`), so the untrimmed token was `"auth:"` while the stored
/// name is `"auth"`, and the two were never going to match byte-for-byte no
/// matter how real the entry was. Measured fleet-wide as part of this same
/// false-positive class: `gh` alone reported 27 fabrications this way. Only
/// these three characters are stripped, deliberately — they are not legal
/// command-name characters (`mandible_core::is_command_name_shaped` doesn't
/// allow them at all), so stripping them can never turn a genuinely
/// different word into a false match. This does **not** address the other,
/// larger false-positive class in the same fleet measurement — a name
/// sitting in column 2+ of a multi-column table (`busybox`, `openssl`) is
/// still missed, because only the first token of each line is considered at
/// all; broadening that would mean accepting a match anywhere on a line,
/// which is a real weakening of the "line-start-ish" guard this module's own
/// doc comment explains (it exists specifically to keep ordinary prose words
/// from false-matching), not a tokenizer bug — deferred, see `xtask audit`'s
/// K2 pre-tag instead of a detector rewrite here.
fn line_start_words(raw: &str) -> HashSet<&str> {
    raw.lines()
        .filter_map(|line| line.split_whitespace().next())
        .map(|word| word.trim_end_matches([':', ',', ';']))
        .collect()
}

/// Candidate raw-text spellings for `flag`'s short spelling — the bare
/// `-x` form, plus, when `flag.value_name` is set, the *reconstructed*
/// single-dash spelling this module's doc comment describes: GCC/Clang/
/// binutils's convention of multi-character single-dash flags
/// (`-fdump-scos`, `-cl-ext=<value>`) is parsed by the (pre-existing,
/// out-of-scope-here) short-flag grammar as one character of `short` plus
/// everything after it glued onto `value_name` verbatim — so the bare `-x`
/// this module would otherwise check for never occurs standalone in real
/// output; only the *compound* `-x<value_name>` does, because that's
/// genuinely the same raw token the grammar split in two. Reconstructing
/// it and checking that instead is the identical "compare against the pre-
/// normalization spelling" principle this module already applies to a
/// long flag's stripped value spec — just applied to the other half of a
/// flag identity for once. `value_name` is stored exactly as extracted (no
/// reformatting - see `grammar::try_value`), so concatenating it directly
/// back onto `short` reconstructs the original substring byte-for-byte
/// whenever the grammar's bare-token branch produced it (`-fdump-scos`);
/// the second candidate, `-x=<value_name>`, covers the same reconstruction
/// when the grammar's `=VALUE` branch instead consumed and discarded a
/// leading `=` (a shape not yet measured in the wild for this convention,
/// but cheap to also check).
///
/// This can only *reduce* false positives, never manufacture a false
/// negative on a genuinely invented flag: it's tried only as a fallback
/// after the bare form already failed, and requires an exact, boundary-
/// respecting match of the *actual extracted value text* — a coincidental
/// collision with unrelated raw text is not a realistic risk for any
/// value_name with real content.
fn short_candidates(flag: &Flag, short: char) -> Vec<String> {
    let mut candidates = vec![format!("-{short}")];
    if let Some(value) = &flag.value_name {
        candidates.push(format!("-{short}{value}"));
        candidates.push(format!("-{short}={value}"));
    }
    candidates
}

/// Candidate raw-text spellings for `flag`'s long name, covering the
/// negatable-boolean bracket convention (this module's doc comment) — any
/// one matching is sufficient. Non-negatable flags get exactly one
/// candidate, the plain `--name` form.
fn long_candidates(flag: &Flag, long: &str) -> Vec<String> {
    if flag.negatable {
        vec![
            format!("--[no-]{long}"),
            format!("--[no]{long}"),
            format!("--{long}"),
        ]
    } else {
        vec![format!("--{long}")]
    }
}

/// The display spelling for a fabrication report — `--[no-]foo` for a
/// negatable long flag, `--foo` otherwise, matching
/// `mandible_core::Flag::spelling`'s own convention for the long half.
fn display_long(flag: &Flag, long: &str) -> String {
    if flag.negatable {
        format!("--[no-]{long}")
    } else {
        format!("--{long}")
    }
}

/// Whether `provenance` credits the help-text tier at all — see this
/// module's doc comment on why that's the right scope: `HelpText` and
/// `HelpTextSynopsis` are the only two sources whose spellings are ever
/// expected to occur in captured `--help`/`-h` prose; every other source
/// is structural and legitimately silent there.
fn is_help_text_sourced(provenance: &Provenance) -> bool {
    provenance
        .sources
        .iter()
        .any(|s| matches!(s, Source::HelpText | Source::HelpTextSynopsis))
}

/// One thing the help-text tier emitted that does not occur in the tool's
/// own raw captured text.
pub struct Fabrication {
    /// Space-separated path to the node that carries this fabrication —
    /// the *parent* node for a subcommand-name fabrication (the name
    /// itself isn't part of the tree's own path since it wasn't a real
    /// node), the owning node for a flag-spelling fabrication.
    pub path: String,
    pub kind: FabricationKind,
    /// The specific spelling or name that failed to attest, for display.
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FabricationKind {
    /// A `CommandNode::name` with no line-start-ish occurrence anywhere in
    /// the raw text — [M-10]'s exact shape.
    Subcommand,
    /// A `Flag` short or long spelling with no boundary-respecting
    /// occurrence anywhere in the raw text.
    Flag,
}

/// The result of analyzing one tool.
pub struct ExistenceReport {
    pub fabrications: Vec<Fabrication>,
}

impl ExistenceReport {
    pub fn fabrication_count(&self) -> usize {
        self.fabrications.len()
    }
}

fn check_flags(node: &CommandNode, path: &str, raw: &str, out: &mut Vec<Fabrication>) {
    for flag in &node.flags {
        if !is_help_text_sourced(&flag.provenance) {
            continue;
        }
        if let Some(short) = flag.short {
            let spelling = format!("-{short}");
            let candidates = short_candidates(flag, short);
            if !candidates.iter().any(|c| spelling_occurs(raw, c)) {
                out.push(Fabrication {
                    path: path.to_string(),
                    kind: FabricationKind::Flag,
                    name: spelling,
                });
            }
        }
        if let Some(long) = &flag.long {
            let candidates = long_candidates(flag, long);
            if !candidates.iter().any(|c| spelling_occurs(raw, c)) {
                out.push(Fabrication {
                    path: path.to_string(),
                    kind: FabricationKind::Flag,
                    name: display_long(flag, long),
                });
            }
        }
    }
}

fn walk(
    node: &CommandNode,
    path: &str,
    raw: &str,
    line_starts: &HashSet<&str>,
    out: &mut Vec<Fabrication>,
) {
    check_flags(node, path, raw, out);
    for child in &node.subcommands {
        if is_help_text_sourced(&child.provenance) && !line_starts.contains(child.name.as_str()) {
            out.push(Fabrication {
                path: path.to_string(),
                kind: FabricationKind::Subcommand,
                name: child.name.clone(),
            });
        }
        let child_path = format!("{path} {}", child.name);
        walk(child, &child_path, raw, line_starts, out);
    }
}

/// Analyze `root`'s help-text-sourced subcommand names and flag spellings
/// against `raw` (the same raw `--help`/`-h` text
/// [`crate::misattribution::RecordingProbe::root_help_text`] hands back)
/// for existence: does each one occur literally in the tool's own output,
/// or was it invented?
///
/// The root node's own name is never checked — it is the literal argv0 a
/// user typed, structurally attested by construction
/// (`mandible_extract::runner::Runner::extract_full`'s own `NodeHints`),
/// never a candidate a parser could have fabricated. Its *flags* are
/// checked like any other node's.
pub fn detect(raw: &str, root: &CommandNode) -> ExistenceReport {
    let line_starts = line_start_words(raw);
    let mut fabrications = Vec::new();
    walk(root, &root.name, raw, &line_starts, &mut fabrications);
    ExistenceReport { fabrications }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::misattribution::RecordingProbe;
    use mandible_core::{Provenance, Source};

    fn help_text_flag(short: Option<char>, long: Option<&str>, negatable: bool) -> Flag {
        let mut flag = Flag::long(long.unwrap_or(""), Provenance::single(Source::HelpText));
        flag.short = short;
        flag.long = long.map(str::to_string);
        flag.negatable = negatable;
        flag
    }

    fn help_text_node(name: &str) -> CommandNode {
        CommandNode::new(name, Provenance::single(Source::HelpText))
    }

    // --- spelling_occurs -----------------------------------------------

    #[test]
    fn spelling_occurs_matches_a_bare_flag() {
        assert!(spelling_occurs(
            "  -v, --verbose  be verbose\n",
            "--verbose"
        ));
        assert!(spelling_occurs("  -v, --verbose  be verbose\n", "-v"));
    }

    #[test]
    fn spelling_occurs_matches_a_value_spec_glued_on_with_no_space() {
        // git's own real shape: `--gpg-sign[=<keyid>]`.
        assert!(spelling_occurs(
            "  -S, --gpg-sign[=<keyid>]\n              GPG-sign commits\n",
            "--gpg-sign"
        ));
    }

    #[test]
    fn spelling_occurs_rejects_a_strict_prefix_of_a_longer_flag() {
        // `--foo` must not match inside the unrelated, longer `--foobar`.
        assert!(!spelling_occurs("  --foobar   does a thing\n", "--foo"));
    }

    #[test]
    fn spelling_occurs_rejects_a_short_flag_embedded_in_a_long_ones_dashes() {
        // `-g` must not match the `-g` substring sitting inside `--gpg-sign`.
        assert!(!spelling_occurs("  --gpg-sign  GPG-sign commits\n", "-g"));
    }

    #[test]
    fn spelling_occurs_false_when_nothing_matches() {
        assert!(!spelling_occurs("  -v, --verbose  be verbose\n", "--quiet"));
    }

    // --- negatable / value-stripping candidates -------------------------

    #[test]
    fn negatable_long_matches_its_real_bracketed_raw_spelling() {
        // `--[no-]source <tree-ish>`, git's own real convention.
        let raw = "  -s, --[no-]source <tree-ish>\n         use tree-ish as source\n";
        let flag = help_text_flag(Some('s'), Some("source"), true);
        let candidates = long_candidates(&flag, "source");
        assert!(candidates.iter().any(|c| spelling_occurs(raw, c)));
    }

    #[test]
    fn non_negatable_long_does_not_get_a_bracketed_candidate_falsely_matching() {
        // A flag stored as non-negatable must not be satisfied merely
        // because *some* unrelated negatable flag's brackets happen to
        // appear elsewhere in the same raw text.
        let raw = "  --[no-]other   toggles other\n";
        let flag = help_text_flag(None, Some("source"), false);
        let candidates = long_candidates(&flag, "source");
        assert!(!candidates.iter().any(|c| spelling_occurs(raw, c)));
    }

    // --- short-flag reconstruction (GCC/Clang single-dash flags) ---------

    /// `gcc`'s (and `lto-dump`'s, a GCC LTO plugin sharing the same
    /// front-end option grammar) real, byte-exact line for its `-fdump-scos`
    /// flag (`corpus`'s own real-tool capture policy: exact strings, not
    /// paraphrased ones). Before [`short_candidates`] existed, this exact
    /// shape drove this task's own real regression: comparing only the bare
    /// `-f` (which never occurs standalone anywhere in `lto-dump --help`'s
    /// real output — every one of its hundreds of `-f...` options glues
    /// more identifier characters directly on) reported `lto-dump` at 848
    /// fabrications and `clang` at 710, both entirely false — see this
    /// module's doc comment.
    const GCC_SINGLE_DASH_LINE: &str = "  -fdump-scos                 \t\t[available in Ada]\n";

    #[test]
    fn short_candidates_reconstructs_a_glued_single_dash_multi_char_flag() {
        let flag = {
            let mut f = help_text_flag(Some('f'), None, false);
            f.value_name = Some("dump-scos".to_string());
            f
        };
        let candidates = short_candidates(&flag, 'f');
        assert!(candidates.contains(&"-fdump-scos".to_string()));
        assert!(candidates
            .iter()
            .any(|c| spelling_occurs(GCC_SINGLE_DASH_LINE, c)));
    }

    #[test]
    fn bare_short_alone_does_not_occur_in_gccs_real_single_dash_line() {
        // The other half of the regression: confirms *why* the bare form
        // alone was failing, not just that the reconstructed form passes.
        assert!(!spelling_occurs(GCC_SINGLE_DASH_LINE, "-f"));
    }

    #[test]
    fn detect_does_not_flag_gccs_real_single_dash_multi_char_flag() {
        let mut root = help_text_node("lto-dump");
        let mut flag = help_text_flag(Some('f'), None, false);
        flag.value_name = Some("dump-scos".to_string());
        root.flags.push(flag);
        let report = detect(GCC_SINGLE_DASH_LINE, &root);
        assert_eq!(
            report.fabrication_count(),
            0,
            "gcc's own real -fdump-scos must not be flagged: {:?}",
            report
                .fabrications
                .iter()
                .map(|f| &f.name)
                .collect::<Vec<_>>()
        );
    }

    /// `lto-dump --help`'s real `--param=` table, byte-exact
    /// (`  --param=lazy-modules=       \t\t[available in C++]`): the long-
    /// flag half of the same real capture, confirming value stripping
    /// (already covered generically above) holds for this exact shape too
    /// — this is the line the maintainer's own first hypothesis named
    /// specifically.
    #[test]
    fn detect_does_not_flag_lto_dumps_real_param_shape() {
        let raw = "  --param=lazy-modules=       \t\t[available in C++]\n";
        let mut root = help_text_node("lto-dump");
        let mut flag = help_text_flag(None, Some("param"), false);
        flag.value_name = Some("lazy-modules=".to_string());
        root.flags.push(flag);
        let report = detect(raw, &root);
        assert_eq!(report.fabrication_count(), 0);
    }

    #[test]
    fn short_candidates_also_covers_the_equals_separated_reconstruction() {
        // A synthetic shape (not yet measured against a real tool, unlike
        // the two above) covering `try_value`'s other branch: if the
        // grammar's `=VALUE` arm ever fires for a single-dash multi-char
        // flag (consuming and discarding a leading `=` before storing the
        // rest as `value_name`), the reconstructed spelling needs the `=`
        // put back — `short_candidates`'s second fallback candidate.
        let raw = "  -c=foo   does a thing\n";
        let flag = {
            let mut f = help_text_flag(Some('c'), None, false);
            f.value_name = Some("foo".to_string());
            f
        };
        let candidates = short_candidates(&flag, 'c');
        assert!(candidates.contains(&"-c=foo".to_string()));
        assert!(candidates.iter().any(|c| spelling_occurs(raw, c)));
    }

    #[test]
    fn detect_still_flags_a_genuinely_fabricated_short_flag_with_a_value_name() {
        // The reconstruction fallback must not blanket-suppress every
        // short flag that happens to carry a `value_name` — only one whose
        // *reconstructed* spelling genuinely occurs. `-z` with a value_name
        // that appears nowhere in the raw text must still be caught.
        let raw = "  -fdump-scos                 \t\t[available in Ada]\n";
        let mut root = help_text_node("t");
        let mut flag = help_text_flag(Some('z'), None, false);
        flag.value_name = Some("totally-invented".to_string());
        root.flags.push(flag);
        let report = detect(raw, &root);
        assert_eq!(report.fabrication_count(), 1);
        assert_eq!(report.fabrications[0].name, "-z");
    }

    // --- line-start-ish subcommand rule ----------------------------------

    #[test]
    fn line_start_words_finds_gits_real_indented_command_list() {
        let raw = "start a working area (see also: git help tutorial)\n   clone     Clone a repository into a new directory\n   init      Create an empty Git repository or reinitialize an existing one\n";
        let words = line_start_words(raw);
        assert!(words.contains("clone"));
        assert!(words.contains("init"));
        assert!(!words.contains("area"));
    }

    #[test]
    fn line_start_words_excludes_a_mid_line_word() {
        let raw = "  -k, --keep-old-files       don't replace existing files when extracting,\n                             treat them as errors\n";
        let words = line_start_words(raw);
        // "errors" is the *last* word of a wrapped continuation line, not
        // its first — must not register as a line-start word.
        assert!(!words.contains("errors"));
        // "treat" is that continuation line's own first word, and *does*
        // register — this module's rule is honestly "first word of a
        // line," not "belongs to a real command-list section"; see the
        // module doc comment on what's left unverified.
        assert!(words.contains("treat"));
    }

    // --- detect: flags ----------------------------------------------------

    #[test]
    fn detect_flags_a_fabricated_flag_spelling() {
        let raw = "  -v, --verbose  be verbose\n";
        let mut root = help_text_node("t");
        root.flags.push(help_text_flag(None, Some("quiet"), false));
        let report = detect(raw, &root);
        assert_eq!(report.fabrication_count(), 1);
        assert_eq!(report.fabrications[0].kind, FabricationKind::Flag);
        assert_eq!(report.fabrications[0].name, "--quiet");
    }

    #[test]
    fn detect_does_not_flag_a_real_flag_with_a_stripped_value_spec() {
        let raw = "  -S, --gpg-sign[=<keyid>]  GPG-sign commits\n";
        let mut root = help_text_node("git");
        let mut flag = help_text_flag(Some('S'), Some("gpg-sign"), false);
        flag.value_name = Some("<keyid>".to_string());
        root.flags.push(flag);
        let report = detect(raw, &root);
        assert_eq!(report.fabrication_count(), 0);
    }

    #[test]
    fn detect_does_not_flag_a_negatable_flag_against_its_bracketed_raw_form() {
        let raw = "  -s, --[no-]source <tree-ish>\n         use tree-ish as source\n";
        let mut root = help_text_node("git");
        root.flags
            .push(help_text_flag(Some('s'), Some("source"), true));
        let report = detect(raw, &root);
        assert_eq!(report.fabrication_count(), 0);
    }

    #[test]
    fn detect_does_not_flag_a_short_and_long_pair_from_separate_alias_rows() {
        // `mandible_core::merge::pair_aliases`'s own real shape: `-R` and
        // `--repo` arrive as two rows with an identical description and
        // get unified into one `Flag` carrying both spellings. Neither
        // needs to sit next to the other in the raw text.
        let raw = "  -R  Select another repository\n  --repo  Select another repository (long form documented on its own line)\n";
        let mut root = help_text_node("gh");
        root.flags
            .push(help_text_flag(Some('R'), Some("repo"), false));
        let report = detect(raw, &root);
        assert_eq!(report.fabrication_count(), 0);
    }

    #[test]
    fn detect_ignores_flags_not_sourced_from_help_text() {
        let raw = "  -v, --verbose  be verbose\n";
        let mut root = help_text_node("t");
        let mut invented = Flag::long(
            "totally-invented",
            Provenance::single(Source::KnownSpec {
                provider: "carapace".to_string(),
            }),
        );
        invented.short = None;
        root.flags.push(invented);
        let report = detect(raw, &root);
        assert_eq!(
            report.fabrication_count(),
            0,
            "a structurally-sourced flag must never be checked against help text"
        );
    }

    // --- detect: subcommands ----------------------------------------------

    #[test]
    fn detect_does_not_flag_gits_real_subcommands() {
        let raw = "start a working area (see also: git help tutorial)\n   clone     Clone a repository into a new directory\n   init      Create an empty Git repository or reinitialize an existing one\n";
        let mut root = help_text_node("git");
        root.subcommands.push(help_text_node("clone"));
        root.subcommands.push(help_text_node("init"));
        let report = detect(raw, &root);
        assert_eq!(report.fabrication_count(), 0);
    }

    #[test]
    fn detect_flags_a_subcommand_name_that_never_occurs_at_all() {
        let raw = "start a working area (see also: git help tutorial)\n   clone     Clone a repository into a new directory\n";
        let mut root = help_text_node("git");
        root.subcommands.push(help_text_node("clone"));
        root.subcommands.push(help_text_node("teleport"));
        let report = detect(raw, &root);
        assert_eq!(report.fabrication_count(), 1);
        assert_eq!(report.fabrications[0].kind, FabricationKind::Subcommand);
        assert_eq!(report.fabrications[0].name, "teleport");
    }

    #[test]
    fn detect_flags_a_subcommand_name_present_only_mid_line() {
        // "errors" occurs literally in the raw text (see
        // `line_start_words_excludes_a_mid_line_word` above) but never as
        // a line's first word — the shape a real command-list entry never
        // takes.
        let raw = "  -k, --keep-old-files       don't replace existing files when extracting,\n                             treat them as errors\n";
        let mut root = help_text_node("tar");
        root.subcommands.push(help_text_node("errors"));
        let report = detect(raw, &root);
        assert_eq!(report.fabrication_count(), 1);
        assert_eq!(report.fabrications[0].name, "errors");
    }

    #[test]
    fn detect_ignores_subcommands_not_sourced_from_help_text() {
        let raw = "start a working area\n   clone     Clone a repository\n";
        let mut root = help_text_node("git");
        let structural = CommandNode::new(
            "hidden-native-only",
            Provenance::single(Source::NativeDynamic {
                protocol: "cobra-dunder-complete".to_string(),
            }),
        );
        root.subcommands.push(structural);
        let report = detect(raw, &root);
        assert_eq!(
            report.fabrication_count(),
            0,
            "a structurally-sourced subcommand must never be checked against help text"
        );
    }

    #[test]
    fn detect_never_checks_the_root_nodes_own_name() {
        // The root's name is the literal argv0 the user typed — never a
        // candidate this module should second-guess, regardless of
        // whether it happens to appear in its own `--help` text.
        let raw = "Usage: definitely-not-in-this-text [OPTION...]\n";
        let root = help_text_node("some-other-name-entirely");
        let report = detect(raw, &root);
        assert_eq!(report.fabrication_count(), 0);
    }

    #[test]
    fn detect_recurses_into_real_subcommands_own_flags() {
        let raw = "  clone     Clone a repository\n  -v, --verbose  be verbose\n";
        let mut root = help_text_node("git");
        let mut clone = help_text_node("clone");
        clone
            .flags
            .push(help_text_flag(None, Some("invented"), false));
        root.subcommands.push(clone);
        let report = detect(raw, &root);
        assert_eq!(report.fabrication_count(), 1);
        assert_eq!(report.fabrications[0].path, "git clone");
        assert_eq!(report.fabrications[0].name, "--invented");
    }

    // --- the M-10 replay: tar's own real corpus text ----------------------

    /// [M-10]'s real war story, replayed against `tar`'s own committed
    /// corpus capture: a hand-built synthetic tree carrying a subcommand
    /// this module's author invented (never edited into
    /// `mandible-extract`, per this task's own constraint — no tier
    /// change could reproduce the historical bug directly, since
    /// `is_command_name_shaped` already rejects a multi-word candidate
    /// like the real *"treat them as errors"* today) proves the detector
    /// would have caught the *shape* of [M-10] against real, byte-exact
    /// tool output: a name with no line-start occurrence anywhere in it.
    #[test]
    fn detects_an_invented_subcommand_against_tars_real_corpus_text() {
        let raw = include_str!("../../corpus/tar/1.35/help.txt");
        let mut root = help_text_node("tar");
        // tar has no real subcommands at all — every one of its "commands"
        // is actually a flag (`-c`, `-x`, `-t`, ...). A phantom node here
        // is exactly [M-10]'s shape: a plausible-looking lowercase word
        // that is not a line-start entry anywhere in tar's own text.
        root.subcommands.push(help_text_node("phantomize"));
        let report = detect(raw, &root);
        assert_eq!(
            report.fabrication_count(),
            1,
            "expected the invented subcommand to be caught: {:?}",
            report
                .fabrications
                .iter()
                .map(|f| &f.name)
                .collect::<Vec<_>>()
        );
        assert_eq!(report.fabrications[0].kind, FabricationKind::Subcommand);
        assert_eq!(report.fabrications[0].name, "phantomize");
    }

    /// Confirms zero fabrications on `tar`'s real, well-formed flags —
    /// every one of tar's genuine flag spellings really does occur in its
    /// own `--help` text, negatable/value-spec forms included (`-H,
    /// --format=FORMAT`, `--sparse-version=MAJOR[.MINOR]`, ...).
    #[test]
    fn no_fabrications_on_tars_own_real_flags() {
        let raw = include_str!("../../corpus/tar/1.35/help.txt");
        let mut root = help_text_node("tar");
        for (short, long) in [
            (Some('c'), Some("create")),
            (Some('x'), Some("extract")),
            (Some('H'), Some("format")),
            (None, Some("sparse-version")),
            (None, Some("occurrence")),
        ] {
            root.flags.push(help_text_flag(short, long, false));
        }
        let report = detect(raw, &root);
        assert_eq!(
            report.fabrication_count(),
            0,
            "unexpected fabrications on tar's own real flags: {:?}",
            report
                .fabrications
                .iter()
                .map(|f| &f.name)
                .collect::<Vec<_>>()
        );
    }

    // --- RecordingProbe wiring sanity (mirrors misattribution's own) -----

    #[test]
    fn empty_text_and_empty_tree_produce_no_fabrications() {
        let root = help_text_node("nothing");
        let report = detect("", &root);
        assert_eq!(report.fabrication_count(), 0);
    }

    /// A trivial confirmation this module really does read
    /// [`RecordingProbe`] the same way `misattribution::detect` does —
    /// "no new probes" (this module's doc comment) means reusing the exact
    /// same capture, not a parallel one.
    #[test]
    fn recording_probe_text_feeds_detect_directly() {
        let probe = RecordingProbe::new();
        assert!(probe.root_help_text().is_none());
        let root = help_text_node("nothing");
        let report = detect(probe.root_help_text().unwrap_or_default().as_str(), &root);
        assert_eq!(report.fabrication_count(), 0);
    }
}
