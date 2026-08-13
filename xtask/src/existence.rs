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
//! > position where a real command-list entry actually sits.
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
//! **Subcommand names** additionally require the occurrence to sit at a
//! position a genuine command-list entry actually occupies — either the
//! first whitespace-delimited word of some physical line
//! ([`line_start_words`]: `corpus/git/2.43.0/help.txt`'s `"   clone
//! Clone a repository..."`, one name per line, indented, nothing before
//! it), or an item of a **list row** ([`list_row_words`]: `openssl`'s
//! column-aligned command grid, `busybox`'s comma-joined applet index).
//! A bare substring check alone would be too weak here in the other
//! direction from flags: ordinary English words (`"list"`, `"add"`,
//! `"get"`) are exactly the words real subcommands are named, and exactly
//! the words that turn up constantly in unrelated running prose — a
//! substring-only check would wave through a name manufactured from a
//! random sentence as long as that sentence happened to contain the same
//! word once, anywhere.
//!
//! The first-token half alone did not close the *other* direction, and the
//! cost was measured rather than guessed: the word-grid layout this
//! module's own doc comment once called "a case never actually observed"
//! is 359 of 656 fleet-wide fabrications, all false, all from two tools.
//! [`list_row_words`] carries the rule that admits them without admitting
//! prose, and its doc comment carries the evidence.
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
//! - **A long flag's value spec glued on with a word-shaped first
//!   character** (`--perf-no_read_workqueue` stored as `long: "perf-no"`,
//!   `value_name: "_read_workqueue"`): the same split, on the other half
//!   of the flag identity. `_` is word-shaped for [`spelling_occurs`]'s
//!   boundary — deliberately, so `--foo` cannot attest inside an unrelated
//!   `--foo_bar` — which meant the bare stored spelling was rejected
//!   against its own raw token. 54 of 656 fleet-wide fabrications, all
//!   false; [`long_candidates`] reconstructs `--<long><value>` the way
//!   [`short_candidates`] already reconstructed `-x<value>`, and carries
//!   the measured table.
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
pub(crate) fn is_word_char(c: char) -> bool {
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
///
/// `pub(crate)` for [`crate::bundling`], which asks the identical question
/// about a reconstructed spelling (does this exact token occur, delimited,
/// in the tool's own text?) and would otherwise carry a second, drifting
/// copy of this boundary rule — the duplication hazard `status.rs`'s own
/// doc comment names.
pub(crate) fn spelling_occurs(raw: &str, candidate: &str) -> bool {
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
/// different word into a false match. This does **not** by itself address a
/// name sitting in column 2+ of a multi-column table (`busybox`,
/// `openssl`); [`list_row_words`] does, and [`attested_name_positions`]
/// unions the two.
fn line_start_words(raw: &str) -> HashSet<&str> {
    let mut out = HashSet::new();
    for line in raw.lines() {
        let Some(word) = line.split_whitespace().next() else {
            continue;
        };
        let word = word.trim_end_matches([':', ',', ';']);
        out.insert(word);
        // Same class of tokenizer fix as the `:` strip above, and it must
        // use the *parser's* rule rather than a second copy of it: binutils
        // `ar` writes each command with its optional modifier groups glued
        // on (`m[ab]`, `r[ab][f][u]`), so the row's first token is not the
        // name the tree stores, and the two were never going to match
        // byte-for-byte however real the command is. Without this the five
        // bracketed `ar` operations are reported as invented — five real
        // commands, in the tool's own help text, called fabrications.
        //
        // Additive and prefix-only: `strip_optional_modifier_suffix`
        // returns its input untouched unless the whole suffix is
        // well-formed `[...]` groups, so this can only ever attest a
        // *prefix of a token that is already on the line*.
        let bare = mandible_extract::help_text::strip_optional_modifier_suffix(word);
        if bare.len() < word.len() && !bare.is_empty() {
            out.insert(bare);
        }
    }
    out
}

/// Minimum number of items a line must break into before it can be read as
/// a **list row** at all. A single item is just a word on a line; it takes
/// several side by side, separated by a real item separator rather than by
/// the single space that separates ordinary prose words, before the line is
/// evidence of a *list* instead of evidence of a sentence.
///
/// Three, not two, and the difference is a false-negative this oracle would
/// otherwise carry silently. At two, *any* two-column table whose right-hand
/// cell happens to be a single word reads as a list row and attests that
/// word — an `ENVIRONMENT` section pairing `TMPDIR` with `directory`, or an
/// index pairing `add` with the description `adds`, would attest
/// `directory`, `editor`, `adds`. A fabricated subcommand that collided with
/// one of those description words would then go unreported, which is the one
/// failure this module must not have: a permissive oracle hides the defects
/// it exists to find, and is worse than one that over-reports.
///
/// Three costs nothing against the real layouts the list-row rule exists to
/// read, which was measured rather than assumed: every qualifying line in
/// `openssl`'s command grid carries 4 items, and in `busybox`'s applet list
/// 9 to 11. Genuine indexes are wide; description tables are two columns.
const MIN_LIST_ROW_ITEMS: usize = 3;

/// Split one physical line into the items a list row would carry: first at
/// tabs and at column gaps (runs of two or more spaces), then at commas,
/// trimming each result and dropping the empties a trailing separator
/// leaves behind.
///
/// Both separators are needed because the two real layouts in the fleet
/// measurement use one each — `openssl`'s command index is space-aligned
/// into columns, `busybox`'s applet index is comma-joined and wrapped —
/// and a tool is free to combine them.
///
/// Every piece is a `&str` borrowed from `line` via `str::split` and
/// `str::trim` only. There is no offset arithmetic anywhere in here, which
/// is what keeps AGENTS.md's rule against slicing captured tool output at a
/// raw byte offset satisfied by construction rather than by care.
fn list_row_items(line: &str) -> Vec<&str> {
    line.split(['\t'])
        .flat_map(|cell| cell.split("  "))
        .flat_map(|cell| cell.split(','))
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .collect()
}

/// The set of every word that sits at an item position of a **list row** —
/// the second half of "is this subcommand name where a real command-list
/// entry actually sits", covering the entries [`line_start_words`]
/// structurally cannot see.
///
/// # Why the first-token rule alone was not enough
///
/// It is the single largest false-positive source this detector had: **359
/// of 656 fleet-wide fabrications, every one of them false**, and both
/// offenders publish their command list as a grid rather than one entry per
/// line. `openssl --help` (112 of them) prints
///
/// ```text
/// asn1parse         ca                ciphers           cmp
/// cms               crl               crl2pkcs7         dgst
/// ```
///
/// and `busybox --help` (247) prints
///
/// ```text
///     [, [[, acpid, adjtimex, ar, arch, arp, arping, ascii, ash, awk,
/// ```
///
/// Only `asn1parse` and `[` are a line's first token, so every other
/// genuine command on both lists was reported as invented.
///
/// # The rule, and why it is not just "anywhere on the line"
///
/// Accepting a match anywhere on a line would be a real weakening: this
/// detector's whole reason for a position rule is that ordinary English
/// words (`list`, `add`, `get`) are exactly what real subcommands are
/// named *and* exactly what turns up in running prose, so a name
/// manufactured from a sentence would sail through. [M-10] is that failure,
/// and it is what this module exists to catch.
///
/// So the line has to earn the reading first. A line is a list row when all
/// three hold:
///
/// 1. It breaks into at least [`MIN_LIST_ROW_ITEMS`] items at a *list*
///    separator — a tab, a column gap of two or more spaces, or a comma.
///    Single spaces are not separators here, which is the load-bearing
///    part: prose is words joined by single spaces, so a sentence stays one
///    item no matter how many words it has.
/// 2. **Every** item is a single whitespace-delimited word. One multi-word
///    item is enough to disqualify the line, because that item is prose and
///    a row that carries prose is a description row, not an index row.
/// 3. **No** item is flag-shaped (leading `-` or `+`). A row that names a
///    flag is an option table, and an option table's second column is its
///    *description* — admitting it would let a one-word description like
///    the `verbose` in `-v, --verbose      verbose` attest a fabricated
///    subcommand named "verbose".
///
/// Against [M-10]'s own text every rule fires, and rule 2 fires hardest:
/// `tar`'s continuation line `treat them as errors` has no separator on it
/// at all, so it is one four-word item and cannot be a list row — which is
/// why `errors` is still reported, and why the [M-10] replay test against
/// `tar`'s real corpus capture still passes.
///
/// # What this still cannot see
///
/// A two-item row whose items are both bare single words and neither is a
/// flag is indistinguishable, on shape alone, from a genuine two-column
/// index — `  fast    quick` reads as a list row. A fabricated subcommand
/// whose name happened to be the one-word right-hand column of some
/// non-option table would be missed. No such case occurs in the fleet
/// measurement this rule was derived from, and the honest thing is to say
/// so rather than pretend the rule is exact.
fn list_row_words(raw: &str) -> HashSet<&str> {
    let mut out = HashSet::new();
    for line in raw.lines() {
        let items = list_row_items(line);
        if items.len() < MIN_LIST_ROW_ITEMS {
            continue;
        }
        let is_list_row = items
            .iter()
            .all(|item| item.split_whitespace().count() == 1 && !item.starts_with(['-', '+']));
        if is_list_row {
            out.extend(items);
        }
    }
    out
}

/// Every position in `raw` at which a genuine command-list entry is
/// attested: a line's first token ([`line_start_words`]) or an item of a
/// list row ([`list_row_words`]). A subcommand name occurring at neither is
/// what this module calls fabricated.
fn attested_name_positions(raw: &str) -> HashSet<&str> {
    let mut set = line_start_words(raw);
    set.extend(list_row_words(raw));
    set
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

/// Whether `raw` spells `short` as a member of a **bundled short-flag
/// cluster** — `-C` inside `tmux`'s `[-2CDlNuVv]`, `-#` inside `tcpdump`'s
/// `[-AbdDefhHIJKlLnNOpqStuUvxX#]`.
///
/// # Why this exists, and why it is not a loosening
///
/// This is the same "compare against the pre-normalization spelling"
/// principle [`short_candidates`] already applies to `-fdump-scos`, and it
/// arrived for the same reason: the grammar started splitting one raw token
/// into several flags, and a per-flag literal check cannot see a spelling
/// that only exists as one character of a token.
///
/// `help_text::grammar::parse_bundled_shorts` now reads a synopsis cluster
/// as the set of switches it is, so `tmux`'s tree carries eight bare
/// booleans where it used to carry one collapsed `-2`. Seven of those eight
/// spellings occur **nowhere** in `tmux --help` on their own — `-C` appears
/// only inside the cluster token — so without this, fixing the collapse
/// would have converted a real 465-flag recall defect into a fleet-wide
/// false alarm on the one oracle built for [M-10]. That is the *K2* shape
/// exactly: 613 of the first sweep's 656 reported fabrications were
/// detector artifacts of three kinds, every one a case of this module
/// checking a spelling the parser had legitimately derived from text it
/// really read.
///
/// **It cannot manufacture a false negative.** It is reached only as a
/// fallback, after the literal forms already failed, and it attests `short`
/// only when some raw token *is* a cluster by the identical rule the
/// grammar splits on — the same function, not a restatement of it. So the
/// oracle attests exactly the spellings the parser derived and nothing
/// else: a genuinely invented `-Q` is still reported unless the tool's own
/// text writes a cluster containing `Q`, in which case `-Q` was read, not
/// invented, and reporting it would be the false alarm.
///
/// The raw text is split on brackets, braces, parens, pipes and commas as
/// well as whitespace, because a synopsis writes its clusters bracketed and
/// does not reliably put a space between the groups: `rpcgen`'s real usage
/// line is `rpcgen [-abkCLNTM][-Dname[=value]] [-i size] ...`, where the
/// cluster and the next option group are **one** whitespace token. Trimming
/// the outer brackets off that token leaves `-abkCLNTM][-Dname[=value`,
/// which is not a cluster, and `-k` — a spelling that occurs nowhere else
/// in `rpcgen --help` — would be reported as invented. Splitting on the
/// delimiters instead of trimming them costs nothing, since none of them
/// can be a cluster member in the first place.
fn occurs_as_a_bundle_member(raw: &str, short: char) -> bool {
    raw.split(|c: char| {
        c.is_whitespace() || matches!(c, '[' | ']' | '{' | '}' | '(' | ')' | '|' | ',')
    })
    .any(|token| {
        mandible_extract::help_text::parse_bundled_shorts(token)
            .is_some_and(|members| members.contains(&short))
    })
}

/// Candidate raw-text spellings for `flag`'s long name: the negatable-
/// boolean bracket convention (this module's doc comment), each also in the
/// *value-reconstructed* form [`short_candidates`] already builds for the
/// short half. Any one matching is sufficient.
///
/// # Why the long half needs the same reconstruction the short half got
///
/// [`spelling_occurs`]'s boundary treats `_` as word-shaped, deliberately —
/// it is a legal identifier character, so `--foo` matching inside an
/// unrelated `--foo_bar` would be the same false attestation `--foo` inside
/// `--foobar` is. But a flag whose *own* value spec is glued straight onto
/// it with no separator hits that guard for the opposite reason: nothing
/// unrelated follows, it is the rest of the same token, split off into
/// `value_name` by `grammar::try_value` exactly as `-fdump-scos`'s tail is.
///
/// Measured, not hypothesized — 54 of 656 fleet-wide fabrications, all
/// false, every one of this shape:
///
/// | raw text | stored `long` | stored `value_name` |
/// |---|---|---|
/// | `--perf-no_read_workqueue` (`cryptsetup`) | `perf-no` | `_read_workqueue` |
/// | `--is-x86_64-xen-domu` (`grub-file`) | `is-x86` | `_64-xen-domu` |
/// | `--fwparam_connect` (`iscsistart`) | `fwparam` | `_connect` |
/// | `--auto_toc_prefix` (`icupkg`) | `auto` | `_toc_prefix` |
/// | `--extended_fields` (`compactsnoop-bpfcc`) | `extended` | `_fields` |
/// | `--load_hidden=<string>` (`llvm-jitlink-18`) | `load` | `_hidden=<string>` |
///
/// The same reasoning `short_candidates` closes with applies unchanged
/// here: these are fallbacks, reached only after the plain form already
/// failed, and each demands an exact boundary-respecting match of the
/// *actual extracted value text*, so a genuinely invented long flag with a
/// value spec is still reported (`detect_still_flags_a_genuinely_
/// fabricated_long_flag_with_a_value_name`).
fn long_candidates(flag: &Flag, long: &str) -> Vec<String> {
    // One dash or two, from the flag itself. A single-dash long option
    // (`mandible_core::Flag::single_dash` — `qemu -help`, `bpftrace -vv`,
    // `lto-dump -CC`) holds its bare name in `long` exactly as a `--`
    // option does, so searching the raw text for `--vv` would report a
    // perfectly real, correctly-parsed flag as an invention. Measured: the
    // repeated-character repair moved `lto-dump` from 10 fabrications to 12
    // until this was fixed, and both were `-CC`/`-MM`.
    let dashes = if flag.single_dash { "-" } else { "--" };
    let bases = if flag.negatable {
        vec![
            format!("{dashes}[no-]{long}"),
            format!("{dashes}[no]{long}"),
            format!("{dashes}{long}"),
        ]
    } else {
        vec![format!("{dashes}{long}")]
    };
    let Some(value) = &flag.value_name else {
        return bases;
    };
    let mut candidates = Vec::with_capacity(bases.len() * 3);
    for base in bases {
        candidates.push(format!("{base}{value}"));
        candidates.push(format!("{base}={value}"));
        candidates.push(base);
    }
    candidates
}

/// The display spelling for a fabrication report — `--[no-]foo` for a
/// negatable long flag, `--foo` otherwise, matching
/// `mandible_core::Flag::spelling`'s own convention for the long half.
fn display_long(flag: &Flag, long: &str) -> String {
    let dashes = if flag.single_dash { "-" } else { "--" };
    if flag.negatable {
        format!("{dashes}[no-]{long}")
    } else {
        format!("{dashes}{long}")
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
            let attested = candidates.iter().any(|c| spelling_occurs(raw, c))
                || occurs_as_a_bundle_member(raw, short);
            if !attested {
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
    attested: &HashSet<&str>,
    out: &mut Vec<Fabrication>,
) {
    check_flags(node, path, raw, out);
    for child in &node.subcommands {
        if is_help_text_sourced(&child.provenance) && !attested.contains(child.name.as_str()) {
            out.push(Fabrication {
                path: path.to_string(),
                kind: FabricationKind::Subcommand,
                name: child.name.clone(),
            });
        }
        let child_path = format!("{path} {}", child.name);
        walk(child, &child_path, raw, attested, out);
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
    let attested = attested_name_positions(raw);
    let mut fabrications = Vec::new();
    walk(root, &root.name, raw, &attested, &mut fabrications);
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

    // --- bundled short-flag cluster members ------------------------------

    /// `tmux`'s real usage line, byte-exact from
    /// `corpus/tmux/audit-seed2/help.stderr.txt`.
    const TMUX_USAGE: &str = "usage: tmux [-2CDlNuVv] [-c shell-command] [-f file] [-L socket-name]\n            [-S socket-path] [-T features] [command [flags]]\n";

    /// The half that motivates [`occurs_as_a_bundle_member`]: seven of
    /// `tmux`'s eight cluster members occur *nowhere* on their own, so the
    /// literal check alone would report all seven as invented the moment
    /// the grammar started emitting them as flags.
    #[test]
    fn a_cluster_members_bare_spelling_does_not_occur_on_its_own() {
        for member in ['C', 'D', 'l', 'N', 'u', 'V'] {
            assert!(
                !spelling_occurs(TMUX_USAGE, &format!("-{member}")),
                "-{member} must not occur standalone in tmux's real usage line"
            );
        }
    }

    /// ...and the cluster check attests every one of them, because the raw
    /// text really does write them — as one token.
    #[test]
    fn every_cluster_member_is_attested_by_its_own_cluster() {
        for member in "2CDlNuVv".chars() {
            assert!(
                occurs_as_a_bundle_member(TMUX_USAGE, member),
                "-{member} must be attested by tmux's own [-2CDlNuVv]"
            );
        }
        // tcpdump's real cluster, including its non-alphanumeric member.
        let raw = "Usage: tcpdump [-AbdDefhHIJKlLnNOpqStuUvxX#] [ -B size ]\n";
        for member in "AbdDefhHIJKlLnNOpqStuUvxX#".chars() {
            assert!(occurs_as_a_bundle_member(raw, member), "-{member}");
        }
    }

    /// End to end: the whole split tree over the real capture reports zero
    /// fabrications. Without the cluster check this is seven.
    #[test]
    fn detect_does_not_flag_the_members_of_a_real_cluster() {
        let mut root = help_text_node("tmux");
        for member in "2CDlNuVv".chars() {
            root.flags.push(help_text_flag(Some(member), None, false));
        }
        for (short, value) in [('c', "shell-command"), ('f', "file"), ('T', "features")] {
            let mut flag = help_text_flag(Some(short), None, false);
            flag.value_name = Some(value.to_string());
            root.flags.push(flag);
        }
        let report = detect(TMUX_USAGE, &root);
        assert_eq!(
            report.fabrication_count(),
            0,
            "tmux's real flags must not be flagged: {:?}",
            report
                .fabrications
                .iter()
                .map(|f| &f.name)
                .collect::<Vec<_>>()
        );
    }

    /// `rpcgen`'s real usage line, byte-exact: the cluster and the next
    /// option group are one whitespace token, which is why the tokenizer
    /// splits on brackets. Found on the fleet sweep, not by reasoning —
    /// this was the one tool the first version of this check still reported,
    /// for `-k`, a spelling that appears nowhere else in `rpcgen --help`.
    const RPCGEN_USAGE: &str =
        "\trpcgen [-abkCLNTM][-Dname[=value]] [-i size] [-I [-K seconds]] [-Y path] infile\n";

    #[test]
    fn a_cluster_glued_to_the_next_bracket_group_still_attests_its_members() {
        for member in "abkCLNTM".chars() {
            assert!(
                occurs_as_a_bundle_member(RPCGEN_USAGE, member),
                "-{member} must be attested by rpcgen's own [-abkCLNTM]"
            );
        }
        // The half that made this a real finding: `-k` occurs nowhere in
        // rpcgen's output except inside that cluster.
        assert!(!spelling_occurs(RPCGEN_USAGE, "-k"));
        // ...and the neighbouring group is still not a cluster, so it
        // vouches for nothing.
        for member in ['D', 'n', 'a', 'm', 'e', 'v'] {
            assert!(!mandible_extract::help_text::parse_bundled_shorts("-Dname")
                .is_some_and(|m| m.contains(&member)));
        }
    }

    /// The check must not become a blanket amnesty for short flags. A
    /// genuinely invented spelling is still reported when the tool's text
    /// carries a cluster that does not contain it, and a token that is not
    /// a cluster by the grammar's own rule attests nothing at all.
    #[test]
    fn a_genuinely_invented_short_flag_is_still_reported_beside_a_cluster() {
        let mut root = help_text_node("tmux");
        root.flags.push(help_text_flag(Some('Q'), None, false));
        let report = detect(TMUX_USAGE, &root);
        assert_eq!(report.fabrication_count(), 1);
        assert_eq!(report.fabrications[0].name, "-Q");
        // A word-shaped token is not a cluster, so it vouches for nothing:
        // `-pass-exit-codes` must not attest `-a`, `-s`, `-e`...
        for member in ['a', 's', 'e', 'x', 'i', 't'] {
            assert!(!occurs_as_a_bundle_member(
                "  -pass-exit-codes   Exit with highest error code\n",
                member
            ));
        }
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

    // --- long-flag value reconstruction ----------------------------------

    /// `cryptsetup --help`'s real line, byte-exact. The parser stores this
    /// as `long: "perf-no"`, `value_name: "_read_workqueue"`, so the bare
    /// `--perf-no` is followed in the raw text by `_` — word-shaped, and
    /// therefore rejected by [`spelling_occurs`]'s boundary until
    /// [`long_candidates`] learned to put the value back.
    const CRYPTSETUP_UNDERSCORE_LINE: &str =
        "      --perf-no_read_workqueue          Bypass dm-crypt workqueue and process\n";

    #[test]
    fn long_candidates_reconstructs_a_glued_underscore_value() {
        let mut flag = help_text_flag(None, Some("perf-no"), false);
        flag.value_name = Some("_read_workqueue".to_string());
        let candidates = long_candidates(&flag, "perf-no");
        assert!(candidates.contains(&"--perf-no_read_workqueue".to_string()));
        assert!(candidates
            .iter()
            .any(|c| spelling_occurs(CRYPTSETUP_UNDERSCORE_LINE, c)));
    }

    #[test]
    fn bare_long_alone_does_not_occur_in_cryptsetups_real_line() {
        // The other half of the regression, stated the same way the GCC
        // short-flag pair is: confirms *why* the bare form was failing.
        assert!(!spelling_occurs(CRYPTSETUP_UNDERSCORE_LINE, "--perf-no"));
    }

    #[test]
    fn detect_does_not_flag_cryptsetups_real_underscore_flag() {
        let mut root = help_text_node("cryptsetup");
        let mut flag = help_text_flag(None, Some("perf-no"), false);
        flag.value_name = Some("_read_workqueue".to_string());
        root.flags.push(flag);
        let report = detect(CRYPTSETUP_UNDERSCORE_LINE, &root);
        assert_eq!(report.fabrication_count(), 0);
    }

    #[test]
    fn detect_still_flags_a_genuinely_fabricated_long_flag_with_a_value_name() {
        // The mirror of `detect_still_flags_a_genuinely_fabricated_short_
        // flag_with_a_value_name`: the reconstruction fallback must not
        // blanket-suppress every long flag that carries a `value_name`,
        // only one whose reconstructed spelling genuinely occurs.
        let mut root = help_text_node("t");
        let mut flag = help_text_flag(None, Some("perf-no"), false);
        flag.value_name = Some("_totally_invented".to_string());
        root.flags.push(flag);
        let report = detect(CRYPTSETUP_UNDERSCORE_LINE, &root);
        assert_eq!(report.fabrication_count(), 1);
        assert_eq!(report.fabrications[0].name, "--perf-no");
    }

    /// A single-dash long option is searched for with one dash, because
    /// that is how the tool spells it. Left at two, the repeated-character
    /// repair (`help_text::sections::repair_repeated_character_flags`)
    /// would have this oracle report `lto-dump`'s perfectly real `-CC` and
    /// `-MM` as inventions — a correctly-parsed flag counted as a
    /// fabrication, which is the exact false positive this oracle's zero is
    /// meant to be trustworthy about. Measured before the fix: `lto-dump`
    /// went from 10 fabrications to 12, and both were this.
    #[test]
    fn a_single_dash_long_option_is_searched_for_with_one_dash() {
        let raw = "    -v      verbose messages\n    -vv     more verbose messages\n";
        let mut root = help_text_node("t");
        let mut flag = help_text_flag(None, Some("vv"), false);
        flag.single_dash = true;
        root.flags.push(flag);
        assert_eq!(detect(raw, &root).fabrication_count(), 0);
        // ...and a genuinely invented one is still reported, with the
        // single-dash spelling it claims to have.
        let mut root = help_text_node("t");
        let mut flag = help_text_flag(None, Some("qq"), false);
        flag.single_dash = true;
        root.flags.push(flag);
        let report = detect(raw, &root);
        assert_eq!(report.fabrication_count(), 1);
        assert_eq!(report.fabrications[0].name, "-qq");
    }

    #[test]
    fn a_negatable_long_still_gets_its_bracketed_forms_when_it_has_a_value() {
        let mut flag = help_text_flag(None, Some("source"), true);
        flag.value_name = Some("<tree-ish>".to_string());
        let candidates = long_candidates(&flag, "source");
        assert!(candidates.contains(&"--[no-]source".to_string()));
        assert!(candidates.contains(&"--[no-]source<tree-ish>".to_string()));
        assert!(candidates.contains(&"--source".to_string()));
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

    /// binutils `ar`'s real command table, byte-exact. Each row glues the
    /// command's optional modifier groups onto its name, so the row's first
    /// token is `m[ab]` while the tree stores `m`. Both must be attested,
    /// or five real `ar` commands get reported as invented — measured: the
    /// fleet sweep went from 0 to 5 fabrications on exactly this.
    #[test]
    fn line_start_words_attests_a_command_carrying_its_modifier_groups() {
        let raw = " commands:\n  d            - delete file(s) from the archive\n  m[ab]        - move file(s) in the archive\n  r[ab][f][u]  - replace existing or insert new file(s) into the archive\n";
        let words = line_start_words(raw);
        assert!(words.contains("d"));
        assert!(words.contains("m"), "m[ab] must attest the command m");
        assert!(words.contains("r"), "r[ab][f][u] must attest the command r");
        // The whole token stays attested too — this is additive.
        assert!(words.contains("m[ab]"));
        // A bracket-led token names no command and must attest nothing new.
        let modifiers = line_start_words("  [a]          - put file(s) after [member-name]\n");
        assert!(!modifiers.contains(""));
        assert!(modifiers.contains("[a]"));
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

    // --- list rows (the multi-column / comma-joined index) ---------------

    /// `openssl --help`'s real command index, byte-exact from the fleet
    /// capture: four commands per line, column-aligned, only the first of
    /// them at a line start. 112 of the 656 fleet-wide fabrications were
    /// this one layout.
    const OPENSSL_GRID: &str = "Standard commands\nasn1parse         ca                ciphers           cmp\ncms               crl               crl2pkcs7         dgst\n";

    /// `busybox --help`'s real applet index, byte-exact: comma-joined and
    /// wrapped, trailing comma included. 247 of the 656.
    const BUSYBOX_LIST: &str = "Currently defined functions:\n    [, [[, acpid, adjtimex, ar, arch, arp, arping, ascii, ash, awk,\n    base64, basename, bc, bunzip2, busybox, bzcat, bzip2, cal, cat,\n";

    #[test]
    fn list_row_words_finds_a_column_aligned_command_grid() {
        let words = list_row_words(OPENSSL_GRID);
        for name in ["asn1parse", "ca", "ciphers", "cmp", "crl2pkcs7", "dgst"] {
            assert!(words.contains(name), "missing {name}: {words:?}");
        }
        // The section heading is prose on its own line — two words joined
        // by a single space, which is not an item separator.
        assert!(!words.contains("Standard"));
        assert!(!words.contains("commands"));
    }

    #[test]
    fn list_row_words_finds_a_comma_joined_applet_list() {
        let words = list_row_words(BUSYBOX_LIST);
        for name in ["acpid", "adjtimex", "arping", "base64", "bunzip2", "cat"] {
            assert!(words.contains(name), "missing {name}: {words:?}");
        }
        assert!(!words.contains("Currently"));
        assert!(!words.contains("defined"));
    }

    #[test]
    fn detect_does_not_flag_opensslfs_real_grid_entries() {
        let mut root = help_text_node("openssl");
        for name in ["ca", "ciphers", "cmp", "crl2pkcs7", "dgst"] {
            root.subcommands.push(help_text_node(name));
        }
        let report = detect(OPENSSL_GRID, &root);
        assert_eq!(
            report.fabrication_count(),
            0,
            "openssl's own real command grid must attest: {:?}",
            report
                .fabrications
                .iter()
                .map(|f| &f.name)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn detect_does_not_flag_busyboxs_real_applet_list() {
        let mut root = help_text_node("busybox");
        for name in ["acpid", "adjtimex", "arping", "bunzip2", "cat"] {
            root.subcommands.push(help_text_node(name));
        }
        let report = detect(BUSYBOX_LIST, &root);
        assert_eq!(report.fabrication_count(), 0);
    }

    // --- list rows: the true positives that must survive the loosening ---

    #[test]
    fn a_prose_line_is_never_a_list_row_however_many_words_it_has() {
        // [M-10]'s own shape. Single spaces are not item separators, so
        // this whole continuation line is one item and the line breaks
        // into fewer than `MIN_LIST_ROW_ITEMS`.
        let raw = "                             treat them as errors\n";
        let words = list_row_words(raw);
        assert!(words.is_empty(), "{words:?}");
    }

    #[test]
    fn detect_still_flags_prose_words_from_a_wrapped_continuation_line() {
        let raw = "  -k, --keep-old-files       don't replace existing files when extracting,\n                             treat them as errors\n";
        let mut root = help_text_node("tar");
        for name in ["them", "as", "errors"] {
            root.subcommands.push(help_text_node(name));
        }
        let report = detect(raw, &root);
        assert_eq!(
            report.fabrication_count(),
            3,
            "every mid-prose word must still be caught: {:?}",
            report
                .fabrications
                .iter()
                .map(|f| &f.name)
                .collect::<Vec<_>>()
        );
    }

    /// The description column of an *option* table must not attest a
    /// subcommand, even when that description happens to be one word.
    /// Rule 3 (no flag-shaped item) is the one doing the work here: the
    /// row names `-v`, so it is an option row, not an index row.
    #[test]
    fn an_option_row_never_attests_its_own_one_word_description() {
        let raw = "  -v, --verbose      verbose\n  -q, --quiet        silent\n";
        let words = list_row_words(raw);
        assert!(!words.contains("verbose"), "{words:?}");
        assert!(!words.contains("silent"), "{words:?}");

        let mut root = help_text_node("t");
        root.subcommands.push(help_text_node("silent"));
        let report = detect(raw, &root);
        assert_eq!(report.fabrication_count(), 1);
        assert_eq!(report.fabrications[0].name, "silent");
    }

    #[test]
    fn a_row_carrying_any_multi_word_item_is_not_a_list_row() {
        // A genuine two-column *description* table. `clone` is still
        // attested — by the first-token rule, which has not changed — but
        // nothing from the right-hand column is.
        let raw = "   clone     Clone a repository into a new directory\n";
        let words = list_row_words(raw);
        assert!(words.is_empty(), "{words:?}");
        assert!(line_start_words(raw).contains("clone"));
    }

    /// A two-column table whose right-hand cell is a single word must not
    /// attest that word. This is the shape `MIN_LIST_ROW_ITEMS = 2` let
    /// through: an `ENVIRONMENT` section and a one-word-description index
    /// are both indistinguishable from a real command grid by every other
    /// rule here, and only their *width* separates them.
    #[test]
    fn a_two_column_table_never_attests_its_right_hand_column() {
        let env = "  TMPDIR    directory\n  EDITOR    editor\n";
        let words = list_row_words(env);
        assert!(!words.contains("directory"), "{words:?}");
        assert!(!words.contains("editor"), "{words:?}");

        let index = "  add       adds\n  remove    removes\n";
        let words = list_row_words(index);
        assert!(!words.contains("adds"), "{words:?}");
        assert!(!words.contains("removes"), "{words:?}");

        // ...and the fabrication that hid behind it is reported again.
        let mut root = help_text_node("t");
        root.subcommands.push(help_text_node("editor"));
        let report = detect(env, &root);
        assert_eq!(report.fabrication_count(), 1);
        assert_eq!(report.fabrications[0].name, "editor");
    }

    /// The left-hand column of such a table is still attested, by the
    /// unchanged first-token rule — tightening the width threshold must
    /// not cost a real index entry that happens to sit in a narrow table.
    #[test]
    fn a_two_column_tables_left_column_is_still_attested() {
        let index = "  add       adds\n  remove    removes\n";
        let starts = line_start_words(index);
        assert!(starts.contains("add"));
        assert!(starts.contains("remove"));
    }

    #[test]
    fn a_single_item_line_is_not_a_list_row() {
        let raw = "        solo\n";
        assert!(list_row_words(raw).is_empty());
    }

    /// The [M-10] replay, re-run against `tar`'s real corpus capture with
    /// the loosened rule in place: the whole point of the loosening is that
    /// it must not reach real prose, and `tar --help` is nothing but
    /// option rows and wrapped prose.
    #[test]
    fn list_rows_admit_nothing_from_tars_real_corpus_text() {
        let raw = include_str!("../../corpus/tar/1.35/help.txt");
        let admitted = list_row_words(raw);
        let already = line_start_words(raw);
        let newly: Vec<&&str> = admitted.iter().filter(|w| !already.contains(*w)).collect();
        assert!(
            newly.is_empty(),
            "the list-row rule must admit no new name position in tar's own text: {newly:?}"
        );
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
