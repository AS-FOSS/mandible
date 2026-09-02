//! Field-level fingerprint types read back from a scoreboard's `#fp`/`#fp2`
//! footer lines, and the wire-format parser for one such line.

use std::collections::BTreeMap;

/// One entity's field-level fingerprint, read back from a `#fp`/`#fp2`
/// line — [`crate::coverage`]'s `FlagFingerprint`, parsed rather than
/// shared directly: this module never depends on
/// `mandible_core`/`mandible_extract` tree types, only on the
/// already-rendered text (this module's own doc comment on why
/// `sweep-diff` reads two rendered scoreboards, never talks to the
/// extraction pipeline itself). Despite the `Flag`-era name, on a V2 line
/// this describes any `EntityKind` — a positional, a modifier, an env-var
/// item — exactly as it always described a flag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedFlagFingerprint {
    pub has_description: bool,
    pub description_hash: Option<u64>,
    pub choices_hash: Option<u64>,
    pub value_name: Option<String>,
}

/// One tool's field-level fingerprint, read back from its `#fp`/`#fp2` line
/// — on a V2 line, every entity regardless of `EntityKind` (`flags` keeps
/// its pre-generalization field name; see [`ParsedFlagFingerprint`]'s doc
/// comment on the same naming carryover).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedFingerprint {
    pub flags: BTreeMap<String, ParsedFlagFingerprint>,
    pub subcommands: std::collections::BTreeSet<String>,
}

/// The fingerprint [`diff`] substitutes for a matched tool's missing side
/// when the other side does carry a `#fp` entry for it — a `'static` empty
/// value so it can be borrowed at the same lifetime as the real,
/// scoreboard-owned fingerprints [`field_diff`] compares.
///
/// "Missing on one side only" is read as empty, not unmeasured:
/// `coverage::fingerprint_lines` emits a `#fp` line for every row
/// including an empty one, so a per-tool line is absent on only one side
/// in the mixed-vintage case (one scoreboard predates the `#fp` footer).
/// The genuinely unmeasurable case — both sides missing — is handled
/// separately in [`diff`] with the `field_diff_unmeasured` wording.
pub(super) static EMPTY_FINGERPRINT: ParsedFingerprint = ParsedFingerprint {
    flags: BTreeMap::new(),
    subcommands: std::collections::BTreeSet::new(),
};

/// Parse one `#fp <tool>\t<subs>\t<flags>` line's content (the part after
/// `"#fp "`) into `(tool, fingerprint)`, or `None` if it's malformed —
/// treated exactly like [`LineResult::Unparseable`] by the caller: skipped,
/// never panicked on, since a `#fp` line only ever exists on a scoreboard
/// this binary itself wrote.
pub(super) fn parse_fingerprint_line(rest: &str) -> Option<(String, ParsedFingerprint)> {
    let mut top = rest.splitn(3, FP_FIELD_SEP);
    let tool = fp_unescape(top.next()?);
    let subs_s = top.next().unwrap_or("");
    let flags_s = top.next().unwrap_or("");

    let mut fp = ParsedFingerprint::default();
    if !subs_s.is_empty() {
        for s in subs_s.split(',') {
            if !s.is_empty() {
                fp.subcommands.insert(fp_unescape(s));
            }
        }
    }
    if !flags_s.is_empty() {
        for entry in flags_s.split('|') {
            let (id, rest) = entry.split_once('=')?;
            let id = fp_unescape(id);
            // splitn(4, ':') so a value_name containing a colon lands whole
            // in the final piece instead of truncating at its first colon.
            let mut fields = rest.splitn(4, ':');
            let has_description = fields.next()? == "1";
            let description_hash = match fields.next()? {
                "-" => None,
                h => u64::from_str_radix(h, 16).ok(),
            };
            let choices_hash = match fields.next()? {
                "-" => None,
                h => u64::from_str_radix(h, 16).ok(),
            };
            let value_name = match fields.next()? {
                "-" => None,
                v => Some(fp_unescape(v)),
            };
            fp.flags.insert(
                id,
                ParsedFlagFingerprint {
                    has_description,
                    description_hash,
                    choices_hash,
                    value_name,
                },
            );
        }
    }
    Some((tool, fp))
}

/// Reverse [`crate::coverage::fp_escape`] on one parsed piece (tool name,
/// subcommand path, flag id, or `value_name`) of a `#fp` line.
///
/// `\\` -> `\`, `\t` -> tab, `\n` -> newline, `\p` -> `|`
/// ([`FP_FLAG_SEP`]), `\c` -> `,` ([`FP_SUBCOMMAND_SEP`]), `\e` -> `=`
/// ([`FP_ID_SEP`]), `\s` -> `:` ([`FP_ENTRY_SEP`]). An unrecognized `\X`
/// passes through verbatim, as does a trailing lone `\`.
///
/// Backward compatible with every scoreboard this binary has ever
/// written: the pre-existing `fp_escape` never emitted a backslash, so
/// with no backslash present every branch above is unreachable and the
/// string comes back unchanged. Caveat: an old scoreboard whose
/// `value_name` held a literal backslash immediately followed by one of
/// the seven escape letters would misread that pair as an escape — a
/// known, measured-zero edge case (0 backslashes found across every
/// captured `#fp` line to date).
pub(super) fn fp_unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('\\') => out.push('\\'),
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('p') => out.push(FP_FLAG_SEP),
            Some('c') => out.push(FP_SUBCOMMAND_SEP),
            Some('e') => out.push(FP_ID_SEP),
            Some('s') => out.push(FP_ENTRY_SEP),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// The literal tab [`coverage::fingerprint_lines`] separates a `#fp` line's
/// three top-level fields with — duplicated from `coverage::FP_FIELD_SEP`
/// (private to that module) for the same reason [`EXTRACT_TIMEOUT_MS`] is
/// duplicated rather than imported: a single well-known, stable character,
/// re-measured in the same commit as the other side if it ever changes.
const FP_FIELD_SEP: char = '\t';

/// Mirrors `coverage::FP_FLAG_SEP` — same duplication convention as
/// [`FP_FIELD_SEP`] above.
const FP_FLAG_SEP: char = '|';

/// Mirrors `coverage::FP_SUBCOMMAND_SEP` — same duplication convention as
/// [`FP_FIELD_SEP`] above.
const FP_SUBCOMMAND_SEP: char = ',';

/// Mirrors `coverage::FP_ID_SEP` — same duplication convention as
/// [`FP_FIELD_SEP`] above.
const FP_ID_SEP: char = '=';

/// Mirrors `coverage::FP_ENTRY_SEP` — same duplication convention as
/// [`FP_FIELD_SEP`] above.
const FP_ENTRY_SEP: char = ':';

/// The pre-generalization `#fp` line prefix — flags only, entity ids with
/// no `EntityKind` tag. Still read, never written: kept so a scoreboard
/// produced by any earlier xtask still loads, tagged
/// [`FingerprintFormat::V1`] rather than misread as V2.
pub(super) const FP_LINE_PREFIX_V1: &str = "#fp ";

/// Mirrors `coverage::FP_LINE_PREFIX_V2` — same duplication convention as
/// [`FP_FIELD_SEP`] above. Checked before [`FP_LINE_PREFIX_V1`] in
/// [`parse_scoreboard`]'s footer loop, though the two can never actually
/// collide (`"#fp2 ..."` fails `strip_prefix("#fp ")`: the character right
/// after `#fp` is `2`, not a space).
pub(super) const FP_LINE_PREFIX_V2: &str = "#fp2 ";
