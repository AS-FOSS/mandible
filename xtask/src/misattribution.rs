//! The misattribution detector: the project's first **correctness**
//! instrument, as opposed to every prior one (the corpus, the sweep, a
//! snapshot), which measures *quantity* — whether a flag has text attached,
//! how many nodes came back, whether today's parse matches yesterday's.
//! None of them compares output to truth.
//!
//! `lsof` (`corpus/lsof/4.95.0`, `[xfail]`) is the proof: it scored 79%
//! "described" while roughly a quarter of its flags were actually correct.
//! Its options table packs three flag+description pairs onto one physical
//! line:
//!
//! ```text
//!   -?|-h list help          -a AND selections (OR)     -b avoid kernel blocks
//! ```
//!
//! The generic parser reads `-?` and swallows the other two columns as its
//! description — so the tree tells a reader `-?` means "AND selections
//! (OR)". `-a` and `-b` never reach the tree at all. This module answers one
//! question with a number: is `lsof` isolated, or is misattribution
//! widespread?
//!
//! # The rule, and why the obvious one fails
//!
//! The obvious formulation is "flag a description containing the literal
//! spelling of another flag *the parser emitted*." That misses `lsof`:
//! `-?`'s description contains `-a` and `-b`, and neither `-a` nor `-b` is
//! in `lsof`'s extracted tree — under-extraction is the *other half* of the
//! same bug, so a check keyed on emitted flags is blind exactly where
//! extraction already failed.
//!
//! This keys on the **raw captured `--help` text** instead: a description is
//! suspect when it contains a flag-shaped token (`-x`, `--word`, `+x`,
//! `+|-x`) that also appears, elsewhere in the same raw text, at a
//! **column-aligned definition position** — the second or third flag-shaped
//! cell of a line that has two or more such cells, at a character offset
//! that recurs across several such lines. That is the actual mechanism of
//! column bleed: a real N-column table repeats its column boundaries row
//! after row.
//!
//! **Why the definition position excludes a token's own (first) column,**
//! and why that matters as much as the recurrence threshold: legitimate
//! prose cross-references a real flag (`du --help`: "`-H` ... equivalent to
//! `--dereference-args (-D)`", where `-D`/`--dereference-args` are `du`'s
//! own, single-column, perfectly legitimate flag definitions two rows
//! above). Keying on *any* definition position — "this token is defined
//! somewhere in the help text" — flags that as suspect too, which is a real,
//! measured false positive, not a hypothetical one (see this module's
//! tests). Restricting the index to a row's *second-or-later* flag-shaped
//! cell, and only trusting a column offset once it recurs
//! [`MIN_COLUMN_RECURRENCE`] times, removes it: `du` and `git` have zero
//! lines with two or more flag-shaped cells at all (measured against their
//! real `--help` output), so nothing is ever added to their index in the
//! first place — the restriction, not the threshold alone, is what saves
//! the `-D` case. `tar` has three lines that incidentally start their
//! second cell with `-T` (real prose: "`-T` treats file names...", "`-T`
//! reads null-terminated names..."), which land at only two distinct
//! offsets, each recurring at most twice — below the threshold, and
//! correctly excluded.
//!
//! # Cells aren't fields: the alias-column false positives
//!
//! The rule above, read literally ("the row's second-or-later flag-shaped
//! *cell*"), still over-triggers on a whole class of real tables: a row that
//! spells one option's short and long forms as two separate cells sharing
//! one description (`nano --help`: `-A  --smarthome  Enable smart home
//! key`). Read cell-by-cell, `--smarthome` looks exactly like a hidden
//! second flag — and did, before this was fixed: it flagged all 52 of
//! `nano`'s options, all false positives, on this project's own full sweep.
//! `-A` and `--smarthome` are the same option; there is nothing to
//! misattribute.
//!
//! [`fields_in_line`] groups cells into logical **fields** instead of
//! reading them one at a time, folding a run of *bare* flag-shaped cells
//! (nothing but the spelling, no trailing prose) into the field they
//! started, and only starting a new field once real trailing text appears.
//! `nano`'s row becomes one field (`-A`+`--smarthome` share the same
//! description); `unzip --help`'s genuinely different two-column table
//! (`-p  extract files to pipe...     -l  list files (short format)`,
//! measured on the same sweep, `unzip -p`'s description really does swallow
//! `-l`'s) still becomes two, because each cell there carries its own
//! trailing description before the next flag starts — see
//! [`Field::is_bare`].
//!
//! Two more real shapes needed their own fix once the alias case was
//! caught, both measured on the same full sweep:
//!
//! - **Tabs, not spaces, before the description.** `debconf --help`:
//!   `-o,  --owner=package<TAB><TAB>Set the package...` — [`cells`] only
//!   split on 2+ spaces, so the tab-glued alias-plus-description read as one
//!   cell with real trailing text, defeating the bare check. [`cells`] now
//!   treats a single tab as a boundary on its own.
//! - **A value placeholder on the short flag's own cell.** `patch --help`:
//!   `-p NUM  --strip=NUM  Strip NUM leading components...` — `-p NUM` has
//!   real-looking trailing text (`NUM`), which isn't a description, it's the
//!   same option's own value notation repeated on both spellings.
//!   [`is_value_placeholder_only`] recognizes a bracketed (`<dir>`) or all-
//!   uppercase (`NUM`, `FILE`) single-word trailing as still-bare.
//!
//! Both of those apply to *either* side of an alias pair — a placeholder can
//! land on the short cell before the long one arrives (`patch`) or the other
//! way around (`thin_metadata_size --help`: `-b  --block-size
//! BLOCKSIZE[...]  Block size...`, where the bare `-b` comes first). Folding
//! only checked a bare *new* cell against an already-open field at first,
//! which fixed `patch` but not `thin_metadata_size`; [`fields_in_line`] now
//! decides foldability from *each* cell's own weakness (empty or
//! placeholder-only trailing) before ever looking at the field it might join,
//! so either order works.
//!
//! [`is_value_placeholder_only`] also recognizes an upper-case *name* followed by
//! a bracketed decoration whose own casing is never checked
//! (`BLOCKSIZE[bskKmMgGtTpPeEzZyY]` — mixed-case unit suffixes, but
//! `BLOCKSIZE` itself is the all-uppercase shape the check already knew),
//! which is what fixed `thin_metadata_size` once the alias-order fix above
//! made it reachable at all.
//!
//! **What's left, honestly.** `arptables --help`'s `--append  -A
//! chain<TAB><TAB>Append to chain` still false-positives: `chain` is a
//! lower-case English word standing in for a value placeholder,
//! indistinguishable from a real short description without more context
//! than one cell offers. The same family (`arptables-nft`, `ebtables`/
//! `ebtables-nft`, `iptables`/`ip6tables` and their `-legacy`/`-nft`/
//! `-translate` siblings, `ntfswipe`) shares this exact shape.
//! [`is_value_placeholder_only`] deliberately doesn't chase a lower-case
//! placeholder — see its own doc comment on why under-suppressing is the
//! safer failure direction here. `objcopy --help`'s `--redefine-syms`/
//! `--strip-symbols`/`--keep-symbols` false-positive for a different reason
//! entirely: they are
//! genuine, deliberate prose cross-references to *other*, real, differently-
//! named flags (`--redefine-sym`, `-N`, `-K`) — exactly the legitimate
//! `--foo implies --bar` shape this detector is a heuristic over, not immune
//! to.
//!
//! **Confirmed true positives beyond `lsof` itself**, the same bug in tools
//! nobody had looked at: `unzip --help`'s and `zipinfo`'s two-column option
//! tables (`-p  extract files to pipe...     -l  list files (short
//! format)`) and `infocmp --help`'s (`-0  print single-row  ...  -e  format
//! output for C initializer`) all genuinely pack two *different* flags per
//! row and lose the second one's real description the same way `lsof` does.
//!
//! # No new probes
//!
//! This is a measurement over text the pipeline **already captures**, never
//! a second probe of the tool. [`RecordingProbe`] is a transparent
//! passthrough to [`LiveProbe`] — every call it forwards is a call the
//! pipeline was going to make anyway (spec §7 Tier B's own root `--help`/
//! `-h` probe) — that additionally remembers the bytes each call returned.
//! `HelpTextTier::extract_node` (`mandible-extract/src/help_text/mod.rs`)
//! parses that text into a tree and then drops it; this module is the first
//! caller that keeps a copy around afterward.
//!
//! # Not gated
//!
//! This is a brand-new metric with no baseline, over a detector whose false
//! positive rate is an open question this module's own doc comment on
//! [`detect`] reports rather than hides. Spec §13.1's own rule — a metric
//! that can be gamed by the failure mode it detects is worse than none —
//! cuts the other way here too: a metric nobody has measured a baseline for
//! must not silently fail a run the first time it's computed. See
//! `xtask/src/main.rs`: `misattribution_suspect_count` is reported in every
//! footer, compared against the previous run for visibility, and never part
//! of `--check`'s pass/fail decision.

use mandible_core::{CommandNode, Flag};
use mandible_extract::exec::{ExecError, ExecOutput, InertArgv, LiveProbe, Probe};
use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

// `MIN_COLUMN_RECURRENCE`, `is_flag_shaped`, `cells`, `first_word`, and
// `is_value_placeholder_only` are imported from `mandible_extract::help_text`
// rather than restated here — see that module's doc comment on the
// re-export for why: this crate's own multi-column splitter (the thing this
// detector audits) needs the exact same vocabulary, and a second copy of
// "what counts as a flag-shaped token" is precisely the class of bug this
// module's own doc comment on [`pick_stream`] names by number (200 of 656
// fabrications). `fields_in_line` below is the one piece that is NOT
// imported — see its own doc comment for why that difference is deliberate
// and load-bearing rather than an oversight.

/// A probe that behaves exactly like [`LiveProbe`] — every call is
/// forwarded to it unmodified, so this spawns nothing [`LiveProbe`] alone
/// wouldn't have — but also remembers the bytes each call returned, keyed
/// by the exact argv sent ([`InertArgv::args`], matching
/// [`mandible_extract::exec::Transcript`]'s own keying rationale: the real
/// argument vector a tier sends, never the shape or a guess at intent).
///
/// This is the whole mechanism behind "no new probes": [`HelpTextTier`]
/// (`mandible-extract/src/help_text/mod.rs`) already fetches a tool's root
/// `--help`/`-h` text to parse it, then drops the string once parsing is
/// done. Driving extraction through this probe instead of a bare
/// [`LiveProbe`] costs zero additional subprocess spawns and lets
/// [`RecordingProbe::root_help_text`] hand back exactly the bytes Tier B
/// already parsed, after the fact.
///
/// [`HelpTextTier`]: mandible_extract::help_text::HelpTextTier
pub struct RecordingProbe {
    inner: LiveProbe,
    recordings: Mutex<HashMap<Vec<String>, ExecOutput>>,
}

impl RecordingProbe {
    /// A fresh recorder with nothing captured yet — one per tool (never
    /// shared across a sweep's tools), so `root_help_text` can never
    /// answer with a different tool's bytes.
    pub fn new() -> Self {
        RecordingProbe {
            inner: LiveProbe,
            recordings: Mutex::new(HashMap::new()),
        }
    }

    /// The raw text Tier B's root probe actually built the tree from —
    /// `--help`, `-h`, or (spec §6 rule 2b) the truncation-confession
    /// follow-up when one was recorded and followed. Same "prefer
    /// `--help`'s stdout/stderr; fall back to `-h` only if `--help`
    /// produced nothing on either stream" rule
    /// `help_text::probe_help_text_reporting_flag` already applies, with
    /// the expansion checked first — see [`Self::find_root_expansion`]'s
    /// doc comment for why that ordering is load-bearing. This reads what
    /// that call already decided; it decides nothing itself and issues no
    /// probe of its own. `None` when the tool has no recording for any
    /// shape at all (nothing was ever detected, or the tier never ran for
    /// it).
    pub fn root_help_text(&self) -> Option<String> {
        let recordings = self.recordings.lock().unwrap_or_else(|e| e.into_inner());
        root_help_text_from(&recordings)
    }

    /// The full recorded `(argv, output)` pair behind [`Self::root_help_text`]
    /// — same "expansion, then `--help`, then `-h` only if the one before it
    /// produced nothing" selection, but handing back the raw [`ExecOutput`]
    /// (separate stdout/stderr, exit code) and the exact argv used, rather
    /// than one merged string. `crate::audit` uses this to write a
    /// corpus-shaped capture (`corpus/README.md`'s `[[capture]]`) without a
    /// second probe of the tool — same "no new probes" property
    /// [`Self::root_help_text`] already documents.
    pub fn root_help_capture(&self) -> Option<(Vec<String>, ExecOutput)> {
        let recordings = self.recordings.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((argv, expanded)) = Self::find_root_expansion_capture(&recordings) {
            if !expanded.stdout.is_empty() || !expanded.stderr.is_empty() {
                return Some((argv, expanded.clone()));
            }
        }
        if let Some(long) = recordings.get(&InertArgv::HelpLong.args()) {
            if !long.stdout.is_empty() || !long.stderr.is_empty() {
                return Some((InertArgv::HelpLong.args(), long.clone()));
            }
        }
        recordings
            .get(&InertArgv::HelpShort.args())
            .map(|short| (InertArgv::HelpShort.args(), short.clone()))
    }

    /// A clone of **every** `(argv, output)` pair this probe recorded during
    /// the single extraction pass it drove — not just the root `--help`
    /// capture [`Self::root_help_capture`] picks out, but everything sent,
    /// however many probes the tool's framework needed (cobra's two-probe
    /// protocol included). Fed into [`mandible_extract::exec::Transcript`]
    /// by `crate::queue::cmd_freeze` so `crate::queue::cmd_reclassify` can
    /// replay the exact same extraction later with zero subprocess spawns —
    /// same "no new probes" property [`Self::root_help_text`] already
    /// documents, extended to cover a full re-run rather than just the
    /// display pair.
    pub fn all_recordings(&self) -> HashMap<Vec<String>, ExecOutput> {
        self.recordings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Find a recorded root [`InertArgv::HelpExpand`] entry (spec §6 rule
    /// 2b), if any, by its **rendered shape** rather than the `InertArgv`
    /// value that produced it — [`RecordingProbe`] only ever sees rendered
    /// argv (same design `mandible_extract::exec::Transcript` keys on, and
    /// for the identical reason). A root expansion always renders to
    /// exactly `["--help", word]`: two elements, `"--help"` first. No other
    /// variant renders that way — `HelpLongForPath`'s `[..words, "--help"]`
    /// always *ends* in `--help` for non-empty `words`, never starts with
    /// it, so this is unambiguous without needing to track which
    /// `InertArgv` constructed which recording.
    ///
    /// **Checked before the plain `--help` recording, not after.** Once a
    /// confession is followed, `HelpTextTier::extract_node` builds the
    /// tree from the *expanded* document — the tree's own flags simply
    /// aren't in the original summary text at all. Preferring the plain
    /// `--help` recording here would hand `existence::detect`/
    /// `misattribution::detect` the wrong ground truth for every flag the
    /// expansion recovered, and both would report them as fabricated
    /// spellings that don't occur in "the raw text" — a false-positive
    /// storm with nothing to do with either detector's own logic. Measured
    /// on a full PATH sweep: curl alone produced 293 spurious existence
    /// "fabrications" before this ordering, zero after.
    fn find_root_expansion(recordings: &HashMap<Vec<String>, ExecOutput>) -> Option<&ExecOutput> {
        recordings
            .iter()
            .find(|(argv, _)| is_root_help_expansion_argv(argv))
            .map(|(_, output)| output)
    }

    /// [`Self::find_root_expansion`], also handing back the matched argv —
    /// the `(Vec<String>, &ExecOutput)` shape [`Self::root_help_capture`]
    /// needs.
    fn find_root_expansion_capture(
        recordings: &HashMap<Vec<String>, ExecOutput>,
    ) -> Option<(Vec<String>, &ExecOutput)> {
        recordings
            .iter()
            .find(|(argv, _)| is_root_help_expansion_argv(argv))
            .map(|(argv, output)| (argv.clone(), output))
    }
}

/// The raw help text a set of recorded `(argv, output)` pairs represents:
/// prefer a root `--help <topic>` expansion (spec §6 rule 2b), then plain
/// `--help`, and fall back to `-h` only when the one before it produced
/// nothing on either stream.
///
/// **Factored out of [`RecordingProbe::root_help_text`] rather than
/// restated**, because a second copy of this selection is precisely the bug
/// spec §13.1c's K2 table records: `RecordingProbe` once carried its own
/// superseded copy of [`pick_stream`], and on every tool that banners to
/// stdout and helps to stderr the existence oracle compared a correct tree
/// against a version string — 200 spurious fabrications from one duplicated
/// decision. `crate::detector` needs the same selection over a corpus
/// fixture's frozen captures rather than a live probe's, so it calls this.
pub fn root_help_text_from(recordings: &HashMap<Vec<String>, ExecOutput>) -> Option<String> {
    if let Some(expanded) = RecordingProbe::find_root_expansion(recordings) {
        if !expanded.stdout.is_empty() || !expanded.stderr.is_empty() {
            return Some(pick_stream(&expanded.stdout, &expanded.stderr));
        }
    }
    if let Some(long) = recordings.get(&InertArgv::HelpLong.args()) {
        if !long.stdout.is_empty() || !long.stderr.is_empty() {
            return Some(pick_stream(&long.stdout, &long.stderr));
        }
    }
    recordings
        .get(&InertArgv::HelpShort.args())
        .map(|short| pick_stream(&short.stdout, &short.stderr))
}

/// True when `argv` is the rendered shape of a *root* [`InertArgv::HelpExpand`]
/// (`words` empty): exactly `["--help", word]`. See
/// [`RecordingProbe::find_root_expansion`]'s doc comment for why this is
/// unambiguous against every other `InertArgv` shape.
fn is_root_help_expansion_argv(argv: &[String]) -> bool {
    argv.len() == 2 && argv.first().map(String::as_str) == Some("--help")
}

impl Default for RecordingProbe {
    fn default() -> Self {
        Self::new()
    }
}

impl Probe for RecordingProbe {
    fn run(
        &self,
        tool_path: &Path,
        argv: &InertArgv,
        timeout: Duration,
    ) -> Result<ExecOutput, ExecError> {
        let result = self.inner.run(tool_path, argv, timeout)?;
        if let Ok(mut recordings) = self.recordings.lock() {
            recordings.insert(argv.args(), result.clone());
        }
        Ok(result)
    }
}

/// The parser's own stream choice ([`pick_stream`]) and its multi-column
/// vocabulary ([`is_flag_shaped`], [`cells`], [`first_word`],
/// [`is_value_placeholder_only`], [`MIN_COLUMN_RECURRENCE`]) — all five
/// imported rather than restated.
///
/// `pick_stream` used to be a two-line local copy here ("stdout if
/// non-empty, else stderr"), justified in a comment on the grounds that the
/// rule was small and "unlikely to drift silently". It drifted silently.
/// `help_text::pick_stream` had already been corrected to judge each stream
/// on whether it *looks like help output* — because `openssl cmp --help`
/// prints two diagnostic lines to stdout and its whole help document to
/// stderr — and the copy here was never updated. Every tool of that shape
/// therefore had its correctly-parsed tree compared against a version
/// banner, so both oracles reported all of its flags as invented: 200 of
/// 656 fleet-wide existence fabrications, every one of them false, on tools
/// like `mkfs.fat`, `tune2fs`, `btrfs-convert`, `xfs_scrub` and `encguess`.
/// An oracle that reads different bytes than the parser read is not
/// measuring the parser.
///
/// The other four names were audited for the identical hazard and found to
/// have it: this crate's own multi-column splitter
/// (`mandible_extract::help_text::sections`) is the thing this whole module
/// audits, and it needs the exact same definitions of "flag-shaped token",
/// "cell", and "bare value placeholder" that this module uses to decide
/// what counts as column-bleed evidence. A second, private copy of any one
/// of them would let this oracle silently stop agreeing with the parser it
/// audits — exactly the `pick_stream` failure mode, just in different
/// vocabulary. There is exactly one right answer to each of "which stream
/// did Tier B parse", "what does Tier B consider flag-shaped", "where does
/// Tier B split a table row", and "what does Tier B consider a bare value
/// placeholder" — all four live in `help_text`, and this instrument asks
/// rather than guessing.
use mandible_extract::help_text::{
    cells, first_word, is_flag_shaped, is_value_placeholder_only, pick_stream,
    MIN_COLUMN_RECURRENCE,
};

/// The characters a real flag token in prose is commonly wrapped in
/// (`(-D)`, `-a,`, `"--foo".`) and never itself starts or ends with.
const TOKEN_TRIM: [char; 9] = ['(', ')', '[', ']', ',', '.', ';', '\'', '"'];

/// Every flag-shaped token found in free-form `text` (a flag description,
/// or a table cell), trimmed of common surrounding punctuation.
fn flag_tokens_in(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|word| word.trim_matches(|c| TOKEN_TRIM.contains(&c)))
        .filter(|word| is_flag_shaped(word))
        .map(str::to_string)
        .collect()
}

/// The set of flag-shaped tokens attested at a column-aligned, secondary
/// definition position somewhere in `raw` — see this module's doc comment
/// for the full rule and the false positives excluding the primary column
/// avoids.
struct DefinitionIndex {
    attested: BTreeSet<String>,
    /// True once at least one column offset met [`MIN_COLUMN_RECURRENCE`]
    /// — i.e., whether the column-alignment signal fired at all for this
    /// tool's text. Reported so a sweep can say how often the strengthened
    /// rule actually had something to strengthen.
    column_aligned: bool,
}

/// One column entry in a row, built by [`fields_in_line`]: a flag-shaped
/// cell (or a *chain* of them — see below) plus whatever descriptive prose
/// followed before the next one started.
struct Field {
    /// Character offset of the field's first flag-shaped cell — the
    /// position [`build_definition_index`] buckets recurrence counts by.
    offset: usize,
    /// Every flag-shaped spelling folded into this field. Usually one; two
    /// when a row spells one option as adjacent bare cells (`-A
    /// --smarthome`, nothing between them) — see [`fields_in_line`]'s doc
    /// comment.
    tokens: Vec<String>,
    /// Accumulated non-flag-shaped text following this field's token(s),
    /// whether glued into the same cell (`-a AND selections (OR)`, single
    /// spaces) or in later cells before the next field starts (`-p
    /// <2+ spaces> extract files...`). Empty means "nothing but the
    /// spelling(s) themselves" — see [`Field::is_bare`].
    trailing: String,
}

impl Field {
    /// True when this field carries no descriptive text of its own at
    /// all — the shape of a row's own long-form alias
    /// (`nano --help`'s `-A  --smarthome  <shared description>`), never of
    /// a second, real flag (which always has *some* trailing text, even if
    /// short — spec's own example table: every hidden-column entry pairs a
    /// flag with a description).
    fn is_bare(&self) -> bool {
        let trailing = self.trailing.trim();
        trailing.is_empty() || is_value_placeholder_only(trailing)
    }
}

/// Group `line`'s cells (see [`cells`]) into [`Field`]s: one field per
/// *logical* column entry, not one per raw cell.
///
/// **Why cells alone aren't the right unit.** A row can spell one option's
/// short and long forms as two consecutive *bare* cells with nothing
/// between them (`nano --help`: `-A             --smarthome
/// Enable smart home key` — three raw cells, but two spellings of *one*
/// option sharing *one* description). Treating `--smarthome` as its own
/// secondary "flag" flagged every one of `nano`'s 52 options as a false
/// positive (measured on this project's own aarch64 dev box, full sweep) —
/// `-A` and `--smarthome` are the same flag, there is nothing left to
/// "misattribute". A run of bare flag-shaped cells with no prose between
/// them is therefore folded into a single field's `tokens`; the field
/// starts fresh only once real trailing text (or end of line) breaks the
/// run. `unzip --help`'s real two-column table (`-p  extract files to
/// pipe...     -l  list files (short format)`) is the case this must NOT
/// also fold away: its second cell has its own trailing description before
/// the next flag, so it starts a genuinely new field.
fn fields_in_line(line: &str) -> Vec<Field> {
    let mut fields: Vec<Field> = Vec::new();
    for (offset, content) in cells(line) {
        let token = first_word(&content);
        if !is_flag_shaped(token) {
            // Plain prose: belongs to whichever field is currently open.
            // A line that starts with prose before any flag-shaped cell
            // has no open field yet — that content isn't part of any
            // flag's definition, so it's simply dropped.
            if let Some(last) = fields.last_mut() {
                if !last.trailing.is_empty() {
                    last.trailing.push(' ');
                }
                last.trailing.push_str(&content);
            }
            continue;
        }
        // This cell's own trailing text, if any — glued on with a single
        // space (`-a AND selections (OR)`) or empty (a bare cell like
        // `--smarthome`, or `-p NUM` whose "trailing" is just its own value
        // placeholder). `strip_prefix`, not a byte offset (AGENTS.md's rule
        // against slicing tool output at a raw index): `token` is exactly
        // `content`'s own prefix up to its first whitespace, so this always
        // matches, but spelling it as a prefix strip rather than
        // `content[token.len()..]` costs nothing and needs no boundary
        // reasoning to trust.
        let own_trailing = content
            .strip_prefix(token)
            .unwrap_or(&content)
            .trim()
            .to_string();
        // Weak trailing — nothing, or only a value placeholder — is not
        // evidence this cell is a genuinely new field; only *this* cell's
        // own content decides that, checked before folding so a cell that
        // itself carries a real description (`unzip`'s `-l  list files
        // (short format)`) never gets swallowed into an open alias chain
        // just because the chain started bare.
        let own_trailing_weak = own_trailing.is_empty() || is_value_placeholder_only(&own_trailing);
        if own_trailing_weak {
            if let Some(last) = fields.last_mut() {
                if last.is_bare() {
                    // Still mid alias-chain (`nano`'s `-A  --smarthome`,
                    // `thin_metadata_size`'s `-b  --block-size
                    // BLOCKSIZE[...]`): another spelling of the same
                    // option, not a new field. Keep any placeholder text
                    // as the field's own trailing so a later *real*
                    // description (this row's actual prose cell, still to
                    // come) still has somewhere to attach.
                    last.tokens.push(token.to_string());
                    if last.trailing.trim().is_empty() && !own_trailing.is_empty() {
                        last.trailing = own_trailing;
                    }
                    continue;
                }
            }
        }
        fields.push(Field {
            offset,
            tokens: vec![token.to_string()],
            trailing: own_trailing,
        });
    }
    fields
}

fn build_definition_index(raw: &str) -> DefinitionIndex {
    let mut offset_counts: HashMap<usize, usize> = HashMap::new();
    let mut offset_tokens: HashMap<usize, BTreeSet<String>> = HashMap::new();

    for line in raw.lines() {
        let fields = fields_in_line(line);
        if fields.len() < 2 {
            // Not a multi-field line — nothing here is evidence of a
            // second table column, so nothing from it enters the index.
            // This is what keeps a single-column tool (`git`, `du`) at an
            // empty index regardless of how much its prose cross-
            // references its own, legitimately-defined flags, and what
            // keeps an alias-chain-plus-shared-description row (`nano`)
            // from ever producing more than one field in the first place.
            continue;
        }
        // The leftmost field is this row's own definition — never a
        // candidate for the *secondary*-column index (see the module doc
        // comment's `du -H`/`-D` case).
        for field in fields.iter().skip(1) {
            if field.is_bare() {
                continue;
            }
            *offset_counts.entry(field.offset).or_insert(0) += 1;
            offset_tokens
                .entry(field.offset)
                .or_default()
                .extend(field.tokens.iter().cloned());
        }
    }

    let mut attested = BTreeSet::new();
    let mut column_aligned = false;
    for (offset, count) in &offset_counts {
        if *count >= MIN_COLUMN_RECURRENCE {
            column_aligned = true;
            if let Some(tokens) = offset_tokens.get(offset) {
                attested.extend(tokens.iter().cloned());
            }
        }
    }
    DefinitionIndex {
        attested,
        column_aligned,
    }
}

/// One flag whose description contains a flag-shaped token attested
/// elsewhere in the tool's own raw help text at a column-aligned definition
/// position.
pub struct Suspect {
    /// Space-separated path to the node this flag belongs to, tool name
    /// included (`"lsof"`, `"git remote add"`).
    pub path: String,
    /// The flag's own canonical spelling (`"-?"`, `"--verbose"`), for
    /// display.
    pub flag: String,
    /// The flag's description exactly as extracted.
    pub description: String,
    /// Every attested token found inside `description`, deduplicated,
    /// sorted.
    pub offending_tokens: Vec<String>,
}

/// The result of analyzing one tool.
pub struct MisattributionReport {
    pub suspects: Vec<Suspect>,
    /// [`DefinitionIndex::column_aligned`] — whether this tool's raw text
    /// had any column-aligned secondary position at all. A tool can have
    /// zero suspects yet still be `column_aligned` (a real multi-column
    /// table whose descriptions just don't happen to mention a neighbour),
    /// and — the case that matters more — a tool can have *no* suspects
    /// specifically *because* nothing met the recurrence bar, which is what
    /// this field lets a caller tell apart from "nothing to find here."
    pub column_aligned: bool,
}

impl MisattributionReport {
    pub fn suspect_count(&self) -> usize {
        self.suspects.len()
    }
}

/// Canonical spellings for `flag` (`-x`/`--word`), used both for the
/// `Suspect::flag` display value and to exclude a flag's own spelling from
/// counting as an offending token in its own description.
fn own_spellings(flag: &Flag) -> Vec<String> {
    let mut spellings = Vec::new();
    if let Some(short) = flag.short {
        spellings.push(format!("-{short}"));
    }
    if let Some(long) = &flag.long {
        // One dash or two, from the flag itself — a single-dash long option
        // (`mandible_core::Flag::single_dash`) is spelled `-vv`, never
        // `--vv`, and getting this wrong would stop excluding a flag's own
        // spelling from its own description.
        let dashes = if flag.single_dash { "-" } else { "--" };
        spellings.push(format!("{dashes}{long}"));
    }
    spellings
}

fn find_suspects(node: &CommandNode, path: &str, index: &DefinitionIndex, out: &mut Vec<Suspect>) {
    for flag in &node.flags {
        let Some(description) = flag.description.as_ref() else {
            continue;
        };
        let description = description.as_str();
        let own = own_spellings(flag);
        let mut offending: Vec<String> = flag_tokens_in(description)
            .into_iter()
            .filter(|token| index.attested.contains(token) && !own.contains(token))
            .collect();
        offending.sort();
        offending.dedup();
        if !offending.is_empty() {
            out.push(Suspect {
                path: path.to_string(),
                flag: own
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "(unnamed)".to_string()),
                description: description.to_string(),
                offending_tokens: offending,
            });
        }
    }
    for child in &node.subcommands {
        let child_path = format!("{path} {}", child.name);
        find_suspects(child, &child_path, index, out);
    }
}

/// Analyze `root`'s flag descriptions against `raw` (the same raw
/// `--help`/`-h` text [`RecordingProbe::root_help_text`] hands back) for
/// misattribution: a description carrying a flag-shaped token attested
/// elsewhere in the same raw text at a column-aligned definition position.
///
/// **Expect false positives.** This is a heuristic over free-form prose,
/// not a proof. `flag_tokens_in` matches a token shape, not confirmed
/// intent — a description that legitimately says "works like `-a`" where
/// `-a` *also* happens to sit in a real, recurring secondary table column
/// elsewhere in the same tool's text (rather than being the specific
/// column-bled neighbour) reads identically to true misattribution from
/// this function's point of view. The measured rate and a sample large
/// enough for a human to judge belong in the sweep's own report, not
/// asserted here — see spec §13.1b's own rule that a brand-new metric with
/// no baseline must not silently fail anything (`xtask/src/main.rs` never
/// gates on this).
pub fn detect(raw: &str, root: &CommandNode) -> MisattributionReport {
    let index = build_definition_index(raw);
    let mut suspects = Vec::new();
    find_suspects(root, &root.name, &index, &mut suspects);
    MisattributionReport {
        suspects,
        column_aligned: index.column_aligned,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandible_core::{Provenance, Source, Text};

    /// `lsof`'s real three-column options-table row, byte-exact from
    /// `corpus/lsof/4.95.0/help.stderr.txt`, is the true positive this
    /// whole module exists to catch: `-?`'s description swallows `-a` and
    /// `-b`, neither of which the (broken) parser ever extracted as its own
    /// node — this test builds the tree the way the real parser's bug
    /// actually leaves it, `-?` only, to prove the detector doesn't depend
    /// on `-a`/`-b` being present in the tree at all (spec's brief: keying
    /// on emitted flags would miss this).
    // Deliberately no `"\` line-continuation on the opening line: that
    // elides the following line's leading whitespace too, which would
    // strip the first row's real indentation and silently desync its
    // column offsets from every row after it — exactly the kind of
    // self-inflicted bug this module's own column-offset math exists to
    // guard against in *real* captures. A harmless leading blank line
    // (`raw.lines()` skips it, `cells("")` returns nothing) is the honest
    // trade.
    const LSOF_TABLE: &str = "
  -?|-h list help          -a AND selections (OR)     -b avoid kernel blocks
  -c c  cmd c ^c /c/[bix]  +c w  COMMAND width (9)    +d s  dir s files
  -d s  select by FD set   +D D  dir D tree *SLOW?*   +|-e s  exempt s *RISKY*
  -i select IPv[46] files  -K [i] list|(i)gn tasKs    -l list UID numbers
  -n no host names         -N select NFS files        -o list file offset
  -O no overhead *RISKY*   -P no port names           -Q allow failed search
  -R list paRent PID       -s list file size          -t terse listing
  -T disable TCP/TPI info  -U select Unix socket      -v list version info
  -V verbose search        +|-w  Warnings (+)         -X skip TCP&UDP* files
";

    fn node_with_flag(
        name: &str,
        short: Option<char>,
        long: Option<&str>,
        desc: &str,
    ) -> CommandNode {
        let mut node = CommandNode::new(name, Provenance::single(Source::HelpText));
        let mut flag = Flag::long(long.unwrap_or(""), Provenance::single(Source::HelpText));
        flag.short = short;
        flag.long = long.map(str::to_string);
        flag.description = Some(Text::sanitize(desc));
        node.flags.push(flag);
        node
    }

    #[test]
    fn flags_lsof_misattributed_question_mark_description() {
        let root = node_with_flag(
            "lsof",
            Some('?'),
            None,
            "-a AND selections (OR)     -b avoid kernel blocks",
        );
        let report = detect(LSOF_TABLE, &root);
        assert_eq!(report.suspect_count(), 1, "expected -? to be flagged");
        assert!(
            report.column_aligned,
            "lsof's table must trip column alignment"
        );
        let suspect = &report.suspects[0];
        assert!(suspect.offending_tokens.contains(&"-a".to_string()));
        assert!(suspect.offending_tokens.contains(&"-b".to_string()));
    }

    /// The `-a`/`-b`/`-l`/`-t`/`-v` flags `lsof`'s real parser never
    /// extracts at all must still be individually detectable *if* they
    /// ever do appear as some other flag's (mis-set) description — the
    /// detector keys on the raw text's column positions, not on what the
    /// tree happens to contain.
    #[test]
    fn detects_regardless_of_whether_offending_flags_reached_the_tree() {
        let mut root = CommandNode::new("lsof", Provenance::single(Source::HelpText));
        let mut q = Flag::long("", Provenance::single(Source::HelpText));
        q.short = Some('?');
        q.description = Some(Text::sanitize(
            "-a AND selections (OR)     -b avoid kernel blocks",
        ));
        root.flags.push(q);
        // No -a, no -b in the tree at all — matching the real bug exactly.
        let report = detect(LSOF_TABLE, &root);
        assert_eq!(report.suspect_count(), 1);
    }

    /// `du --help`'s real cross-reference: `-H`'s description legitimately
    /// says "equivalent to --dereference-args (-D)", and both
    /// `--dereference-args` and `-D` are `du`'s own real, singly-defined
    /// flag two rows above. This must NOT be flagged — the false positive
    /// this module's doc comment names by name, and the reason the
    /// secondary-column-only restriction exists.
    #[test]
    fn does_not_flag_legitimate_cross_reference_in_a_single_column_table() {
        let raw = "
  -D, --dereference-args  dereference only symlinks that are listed on the
                          command line
  -H                    equivalent to --dereference-args (-D)
  -h, --human-readable  print sizes in human readable format
";
        let root = node_with_flag(
            "du",
            Some('H'),
            None,
            "equivalent to --dereference-args (-D)",
        );
        let report = detect(raw, &root);
        assert_eq!(
            report.suspect_count(),
            0,
            "a real, single-column cross-reference must not be flagged: {:?}",
            report.suspects.first().map(|s| &s.description)
        );
        assert!(!report.column_aligned);
    }

    /// `tar --help`'s real near-miss: three different single-column rows
    /// each happen to open their description with `-T` (a real, repeated
    /// cross-reference), at only two distinct offsets, each recurring at
    /// most twice — below [`MIN_COLUMN_RECURRENCE`]. Must not be flagged.
    #[test]
    fn does_not_flag_an_accidental_two_line_coincidence_below_the_threshold() {
        let raw = "
      --no-verbatim-files-from   -T treats file names starting with dash as
                             options
      --null                 -T reads null-terminated names; implies
                             --verbatim-files-from
      --verbatim-files-from  -T reads file names verbatim (no escape or option
                             processing)
";
        let root = node_with_flag(
            "tar",
            None,
            Some("null"),
            "-T reads null-terminated names; implies --verbatim-files-from",
        );
        let report = detect(raw, &root);
        assert_eq!(report.suspect_count(), 0);
    }

    /// `nano --help`'s real table: short flag, a *bare* long-alias cell,
    /// then description — three cells, not the two-real-flags shape. This
    /// is a measured, real false positive (`aarch64` box, full sweep: all
    /// 52 of `nano`'s flags flagged) that motivated the bare-alias
    /// exclusion in `build_definition_index`. `-A`'s description doesn't
    /// even contain `--smarthome` in this test — the point is that
    /// `--smarthome` must never enter the attested set at all, from *any*
    /// row, so no later flag's real prose could accidentally collide with
    /// it either.
    #[test]
    fn does_not_flag_a_bare_long_alias_column_as_a_second_flag() {
        let raw = "
 -A             --smarthome             Enable smart home key
 -B             --backup                Save backups of existing files
 -C <dir>       --backupdir=<dir>       Directory for saving unique backup files
 -D             --boldtext              Use bold instead of reverse video text
";
        let root = node_with_flag(
            "nano",
            Some('A'),
            Some("smarthome"),
            "Enable smart home key",
        );
        let report = detect(raw, &root);
        assert_eq!(
            report.suspect_count(),
            0,
            "a bare long-alias column must never be read as a second flag: {:?}",
            report.suspects.first().map(|s| &s.description)
        );
        assert!(
            !report.column_aligned,
            "a table of [short, bare-alias, description] triples carries no column-bleed evidence"
        );
    }

    /// `patch --help`'s and `nano --help`'s real shape: the short flag's
    /// own cell carries a bare value placeholder (`-p NUM`, `-C <dir>`)
    /// before the bare long alias and shared description — a third,
    /// measured false-positive source distinct from both cases above
    /// (`Field::is_bare`/`is_value_placeholder_only`'s own doc comments).
    #[test]
    fn does_not_flag_a_short_flags_own_value_placeholder_as_evidence() {
        let raw = "  -p NUM  --strip=NUM  Strip NUM leading components from file names.\n  -o FILE  --output=FILE  Output patched files to FILE.\n  -r FILE  --reject-file=FILE  Output rejects to FILE.\n";
        let root = node_with_flag(
            "patch",
            Some('p'),
            Some("strip=NUM"),
            "Strip NUM leading components from file names.",
        );
        let report = detect(raw, &root);
        assert_eq!(
            report.suspect_count(),
            0,
            "a flag's own placeholder-bearing cell must not create false column-bleed evidence: {:?}",
            report.suspects.first().map(|s| &s.description)
        );
    }

    /// `thin_metadata_size --help`'s real table: the placeholder lands on
    /// the *long*-alias cell, not the short one (`patch`'s order, above,
    /// reversed) — `-b  --block-size BLOCKSIZE[...]  Block size...`. Proves
    /// folding doesn't depend on which side of an alias pair carries the
    /// placeholder.
    #[test]
    fn does_not_flag_a_long_aliass_own_value_placeholder_as_evidence() {
        let raw = "\t-b  --block-size BLOCKSIZE[bskKmMgGtTpPeEzZyY]  Block size of thin provisioned devices.\n\t-s  --pool-size SIZE[bskKmMgGtTpPeEzZyY]        Size of pool device.\n\t-m  --max-thins #MAXTHINS[bskKmMgGtTpPeEzZyY]   Maximum sum of all thin devices and snapshots.\n";
        let root = node_with_flag(
            "thin_metadata_size",
            Some('b'),
            Some("block-size"),
            "BLOCKSIZE[bskKmMgGtTpPeEzZyY] Block size of thin provisioned devices.",
        );
        let report = detect(raw, &root);
        assert_eq!(
            report.suspect_count(),
            0,
            "a placeholder on either side of an alias pair must not create false evidence: {:?}",
            report.suspects.first().map(|s| &s.description)
        );
    }

    /// `debconf --help`'s real table: alias pair separated by two spaces,
    /// then the shared description separated by **tabs**, not spaces
    /// (`cells`'s own doc comment). Requiring 2+ *spaces* alone read the
    /// tab-glued `--owner=package<TAB><TAB>Set the package...` as one cell
    /// with real trailing text, making the bare-alias check miss it — a
    /// measured false positive (`debconf`, full sweep) distinct from
    /// `nano`'s (space-only) case above.
    #[test]
    fn does_not_flag_a_bare_alias_separated_from_its_description_by_tabs() {
        let raw = "  -o,  --owner=package\t\tSet the package that owns the command.\n  -f,  --frontend\t\tSpecify debconf frontend to use.\n  -p,  --priority\t\tSpecify minimum priority question to show.\n";
        let root = node_with_flag(
            "debconf",
            Some('o'),
            Some("owner=package"),
            "Set the package that owns the command.",
        );
        let report = detect(raw, &root);
        assert_eq!(
            report.suspect_count(),
            0,
            "a tab-separated bare alias must not be read as a second flag: {:?}",
            report.suspects.first().map(|s| &s.description)
        );
    }

    /// `unzip --help`'s real table is the case the bare-alias exclusion
    /// must NOT also suppress: a genuine two-column table of *different*
    /// flags (`-p`/`-l`, `-f`/`-t`, ...), where the second column's cell
    /// carries a real description past the flag token — unlike a bare
    /// alias, so it must still count as evidence. Confirms the
    /// discriminator is "has trailing content", not "is a 2-column table".
    #[test]
    fn still_flags_a_genuine_two_column_table_of_different_flags() {
        let raw = "
  -p  extract files to pipe, no messages     -l  list files (short format)
  -f  freshen existing files, create none    -t  test compressed archive data
  -u  update files, create if necessary      -z  display archive comment only
  -v  list verbosely/show version info       -T  timestamp archive to latest
  -x  exclude files that follow (in xlist)   -d  extract files into exdir
";
        let root = node_with_flag(
            "unzip",
            Some('p'),
            None,
            "extract files to pipe, no messages     -l  list files (short format)",
        );
        let report = detect(raw, &root);
        assert_eq!(report.suspect_count(), 1, "expected -p to be flagged");
        assert!(report.column_aligned);
        assert!(report.suspects[0]
            .offending_tokens
            .contains(&"-l".to_string()));
    }

    /// A flag's description mentioning its own spelling (e.g. duplicated
    /// alias prose) must never flag itself.
    #[test]
    fn a_flags_own_spelling_in_its_own_description_is_never_offending() {
        let raw = "  -v, --verbose  be verbose  -q, --quiet   be quiet\n  -a, --all      show all    -n, --dry-run be a no-op\n";
        let mut node = CommandNode::new("t", Provenance::single(Source::HelpText));
        let mut v = Flag::long("verbose", Provenance::single(Source::HelpText));
        v.short = Some('v');
        v.description = Some(Text::sanitize("-v, --verbose  be verbose"));
        node.flags.push(v);
        let report = detect(raw, &node);
        assert_eq!(report.suspect_count(), 0);
    }

    #[test]
    fn empty_text_and_empty_tree_produce_no_suspects() {
        let root = CommandNode::new("nothing", Provenance::single(Source::HelpText));
        let report = detect("", &root);
        assert_eq!(report.suspect_count(), 0);
        assert!(!report.column_aligned);
    }

    #[test]
    fn is_flag_shaped_recognizes_every_documented_shape() {
        assert!(is_flag_shaped("-a"));
        assert!(is_flag_shaped("--foo"));
        assert!(is_flag_shaped("+c"));
        assert!(is_flag_shaped("+|-e"));
        assert!(is_flag_shaped("-?"));
        assert!(!is_flag_shaped("-"));
        assert!(!is_flag_shaped("--"));
        assert!(!is_flag_shaped("word"));
        assert!(!is_flag_shaped(""));
    }

    #[test]
    fn cells_splits_on_runs_of_two_or_more_spaces_by_char_offset() {
        let line = "  -a foo   -b bar";
        let cs = cells(line);
        assert_eq!(
            cs,
            vec![(2, "-a foo".to_string()), (11, "-b bar".to_string())]
        );
    }

    /// A wide (multi-byte) character earlier in the line must not desync
    /// later offsets — the exact hazard AGENTS.md's byte-slicing rule
    /// warns about, checked here for this module's own column math.
    #[test]
    fn cells_are_char_indexed_not_byte_indexed() {
        let line = "  “fancy”  -a foo";
        let cs = cells(line);
        // "“fancy”" is 7 chars (2 of them multi-byte) starting at char
        // offset 2; the second cell must start at char offset 11, not at
        // whatever a byte count would produce.
        assert_eq!(cs[1].0, 11);
    }

    /// The duplicated-logic-drift regression this module's own audit was
    /// commissioned to find: `is_flag_shaped`, `cells`, `first_word`,
    /// `is_value_placeholder_only` and `MIN_COLUMN_RECURRENCE` used to be
    /// hand-copied here from `mandible_extract::help_text::sections` (the
    /// real multi-column splitter this module audits), each carrying its
    /// own comment claiming the copy was small and safe to keep in sync by
    /// hand — the exact justification that failed for `pick_stream` (200 of
    /// 656 fleet-wide existence fabrications, this module's own doc comment
    /// on that `use`). They are now imports, not copies, so they cannot
    /// drift apart by construction; this test pins the specific edge case
    /// that made `is_value_placeholder_only` non-trivial to get right
    /// (`thin_metadata_size --help`'s `BLOCKSIZE[bskKmMgGtTpPeEzZyY]`: an
    /// all-uppercase *name* with a deliberately mixed-case bracketed
    /// decoration) directly against the imported function, so a future
    /// change to the shared definition that breaks this shape fails here
    /// too, not just in `sections.rs`'s own suite.
    #[test]
    fn is_value_placeholder_only_is_the_imported_extractor_definition() {
        assert!(is_value_placeholder_only("BLOCKSIZE[bskKmMgGtTpPeEzZyY]"));
        assert!(is_value_placeholder_only("<dir>"));
        assert!(is_value_placeholder_only("NUM"));
        assert!(is_value_placeholder_only(""));
        // `arptables --help`'s documented residual false positive: a
        // lower-case English word is deliberately NOT recognized as a
        // placeholder (see `is_value_placeholder_only`'s own doc comment on
        // why under-suppressing is the accepted failure direction here).
        // If a future change to the shared definition starts recognizing
        // lower-case words, this line is the tripwire.
        assert!(!is_value_placeholder_only("chain"));
        assert!(!is_value_placeholder_only("two words"));
    }

    /// `MIN_COLUMN_RECURRENCE` used to be two independent `const` items (one
    /// here, one in `sections.rs`) that happened to both read `3` — nothing
    /// enforced they stayed equal. Now there is exactly one, imported; this
    /// pins the value so a change to the shared threshold is visible (and
    /// reviewable) from this module's own test suite, not only from
    /// `sections.rs`'s.
    #[test]
    fn min_column_recurrence_is_the_single_shared_threshold() {
        assert_eq!(MIN_COLUMN_RECURRENCE, 3);
    }

    #[test]
    fn recording_probe_returns_none_when_nothing_was_recorded() {
        let probe = RecordingProbe::new();
        assert!(probe.root_help_text().is_none());
    }

    #[test]
    fn recording_probe_prefers_help_long_over_help_short() {
        let probe = RecordingProbe::new();
        probe.recordings.lock().unwrap().insert(
            InertArgv::HelpLong.args(),
            ExecOutput {
                stdout: b"long output".to_vec(),
                stderr: Vec::new(),
                exit_code: Some(0),
                timed_out: false,
            },
        );
        probe.recordings.lock().unwrap().insert(
            InertArgv::HelpShort.args(),
            ExecOutput {
                stdout: b"short output".to_vec(),
                stderr: Vec::new(),
                exit_code: Some(0),
                timed_out: false,
            },
        );
        assert_eq!(probe.root_help_text().as_deref(), Some("long output"));
    }

    #[test]
    fn recording_probe_falls_back_to_help_short_when_long_produced_nothing() {
        let probe = RecordingProbe::new();
        probe.recordings.lock().unwrap().insert(
            InertArgv::HelpLong.args(),
            ExecOutput {
                stdout: Vec::new(),
                stderr: Vec::new(),
                exit_code: Some(1),
                timed_out: false,
            },
        );
        probe.recordings.lock().unwrap().insert(
            InertArgv::HelpShort.args(),
            ExecOutput {
                stdout: b"short output".to_vec(),
                stderr: Vec::new(),
                exit_code: Some(0),
                timed_out: false,
            },
        );
        assert_eq!(probe.root_help_text().as_deref(), Some("short output"));
    }

    /// The drift regression: a banner on stdout must not beat the real
    /// help document on stderr.
    ///
    /// `mkfs.fat --help` prints exactly `mkfs.fat 4.2 (2021-01-31)` to
    /// stdout and its whole option table to stderr; `tune2fs`,
    /// `btrfs-convert`, `xfs_scrub`, `ntfssecaudit` and `encguess` all do
    /// the same thing. Tier B reads stderr for these (its own `pick_stream`
    /// judges each stream on whether it *looks like help output*), so an
    /// oracle that read stdout was comparing a correct tree against a
    /// version string and calling every flag in it invented — 200 of 656
    /// fleet-wide existence fabrications. This asserts the two now agree,
    /// which they do by construction: there is one `pick_stream` and this
    /// module imports it.
    #[test]
    fn recording_probe_reads_the_stream_the_parser_read_not_merely_stdout() {
        let probe = RecordingProbe::new();
        probe.recordings.lock().unwrap().insert(
            InertArgv::HelpLong.args(),
            ExecOutput {
                stdout: b"mkfs.fat 4.2 (2021-01-31)\n".to_vec(),
                stderr: b"Usage: mkfs.fat [OPTIONS] TARGET [BLOCKS]\n\
                          Options:\n  -a              Disable alignment of data structures\n\
                          \x20 -A              Toggle Atari variant\n"
                    .to_vec(),
                exit_code: Some(0),
                timed_out: false,
            },
        );
        let text = probe.root_help_text().expect("a recording exists");
        assert!(
            text.contains("-A              Toggle Atari variant"),
            "the help document on stderr must win over the stdout banner, got: {text:?}"
        );
    }

    /// The other direction of the same rule: when stdout *is* the help
    /// document, a noisy stderr must not displace it.
    #[test]
    fn recording_probe_keeps_stdout_when_stdout_is_the_help_document() {
        let probe = RecordingProbe::new();
        probe.recordings.lock().unwrap().insert(
            InertArgv::HelpLong.args(),
            ExecOutput {
                stdout: b"Usage: t [OPTIONS]\nOptions:\n  -v, --verbose  be verbose\n".to_vec(),
                stderr: b"warning: locale not set\n".to_vec(),
                exit_code: Some(0),
                timed_out: false,
            },
        );
        let text = probe.root_help_text().expect("a recording exists");
        assert!(text.contains("--verbose"), "got: {text:?}");
        assert!(!text.contains("locale not set"), "got: {text:?}");
    }

    /// Spec §6 rule 2b regression: when a root confession was followed,
    /// the recorded expansion (`["--help", "all"]`) must win over the
    /// plain `--help` recording — the tree was built from the expanded
    /// document, so that is the "raw text" `existence`/`misattribution`
    /// must compare against. Before this fix, every flag unique to the
    /// expansion read as fabricated: curl alone produced 293 spurious
    /// existence suspects on a full sweep.
    #[test]
    fn recording_probe_prefers_a_followed_root_expansion_over_plain_help() {
        let probe = RecordingProbe::new();
        probe.recordings.lock().unwrap().insert(
            InertArgv::HelpLong.args(),
            ExecOutput {
                stdout: b"summary output".to_vec(),
                stderr: Vec::new(),
                exit_code: Some(0),
                timed_out: false,
            },
        );
        probe.recordings.lock().unwrap().insert(
            InertArgv::HelpExpand {
                words: vec![],
                word: "all".to_string(),
            }
            .args(),
            ExecOutput {
                stdout: b"expanded output".to_vec(),
                stderr: Vec::new(),
                exit_code: Some(0),
                timed_out: false,
            },
        );
        assert_eq!(probe.root_help_text().as_deref(), Some("expanded output"));
        let (argv, capture) = probe
            .root_help_capture()
            .expect("an expansion was recorded");
        assert_eq!(argv, vec!["--help".to_string(), "all".to_string()]);
        assert_eq!(capture.stdout, b"expanded output");
    }

    /// The common case (no confession printed at all, or one detected but
    /// not followed) must be entirely unaffected: with no expansion
    /// recording, behavior is identical to before this fix.
    #[test]
    fn recording_probe_falls_back_to_plain_help_when_no_expansion_was_recorded() {
        let probe = RecordingProbe::new();
        probe.recordings.lock().unwrap().insert(
            InertArgv::HelpLong.args(),
            ExecOutput {
                stdout: b"summary output".to_vec(),
                stderr: Vec::new(),
                exit_code: Some(0),
                timed_out: false,
            },
        );
        assert_eq!(probe.root_help_text().as_deref(), Some("summary output"));
    }

    /// A subcommand's own `HelpLongForPath` probe (`[..words, "--help"]`)
    /// must never be mistaken for a root expansion, even though both end
    /// in `"--help"` — the discriminator is *position* (root expansion has
    /// `"--help"` first; a subcommand path has it last), not mere presence
    /// of the string.
    #[test]
    fn a_subcommand_help_probe_is_never_mistaken_for_a_root_expansion() {
        assert!(!is_root_help_expansion_argv(&[
            "sub".to_string(),
            "--help".to_string()
        ]));
        assert!(is_root_help_expansion_argv(&[
            "--help".to_string(),
            "all".to_string()
        ]));
    }
}
