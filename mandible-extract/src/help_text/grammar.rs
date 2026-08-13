//! `winnow`-based grammar for a single flag-spec fragment, e.g.
//! `-A, --catenate, --concatenate`, `-o, --output <FILE>`,
//! `--sparse-version=MAJOR[.MINOR]`, `--occurrence[=NUMBER]`.
//!
//! Real `--help` output is wildly inconsistent, so this grammar recognizes
//! the dominant shape (an optional short flag, optional long flag(s),
//! optional value spec) and is deliberately permissive about anything
//! after that it doesn't fully understand — leftover text becomes the
//! value name verbatim rather than a parse failure. [`parse_flag_spec`]
//! reports whether it fully consumed the input, which
//! `help_text::sections` uses to compute this tier's confidence score.

use mandible_core::ValueKind;
use std::collections::HashSet;
use winnow::ascii::multispace0;
use winnow::error::ContextError;
use winnow::prelude::*;
use winnow::token::{one_of, take_while};

/// This grammar never needs winnow's richer error-context machinery — a
/// flag-spec fragment either matches the recognized shape or it doesn't,
/// and callers fall back to a best-effort split either way — so every
/// parser function here is pinned to the same concrete error type.
type Res<T> = ModalResult<T, ContextError>;

/// The result of parsing one flag-spec fragment.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FlagSpec {
    /// The short spelling, if any (only the first one found; some tools
    /// list `-A, -a` style duplicate shorts, which is rare enough to not
    /// warrant a `Vec`).
    pub short: Option<char>,
    /// The first long spelling found (tar-style multi-alias specs like
    /// `--catenate, --concatenate` only keep the first), always the *base*
    /// name — never containing `[` or `]` even when the source spelled it
    /// `--[no-]foo` (see `negatable`).
    pub long: Option<String>,
    /// True if the long spelling was written as GNU getopt_long's
    /// negatable-boolean convention, `--[no-]foo` (git's `--help`
    /// formatter renders every negatable boolean this way — a shape, not a
    /// tool name; any framework using the same convention gets the same
    /// treatment). `long` is always `"foo"`, never `"[no-]foo"`.
    pub negatable: bool,
    /// The value placeholder text, if a value spec was recognized.
    pub value_name: Option<String>,
    /// Whether the value (if any) is required or optional.
    pub value_kind: ValueKind,
    /// True if the grammar consumed the entire fragment cleanly (no
    /// leftover text it didn't understand). Used for confidence scoring.
    pub fully_consumed: bool,
}

/// Parse a flag-spec fragment (the part of a `--help` entry line before
/// the description column — already isolated by the layout parser).
pub fn parse_flag_spec(input: &str) -> FlagSpec {
    let mut rest = input.trim();
    let mut spec = FlagSpec::default();

    loop {
        rest = skip_separators(rest);
        if rest.is_empty() {
            break;
        }
        if let Some((c, tail)) = try_short(rest) {
            if spec.short.is_none() {
                spec.short = Some(c);
            }
            rest = tail;
            continue;
        }
        if let Some((name, negatable, tail)) = try_long(rest) {
            if spec.long.is_none() {
                spec.long = Some(name);
                spec.negatable = negatable;
            }
            rest = tail;
            continue;
        }
        break;
    }

    rest = skip_separators(rest);
    if rest.is_empty() {
        spec.fully_consumed = true;
        return spec;
    }

    // Whatever remains is treated as a value spec: `=VALUE`, ` VALUE`,
    // `[=VALUE]`, `[VALUE]`, or a bare `<value>`/`VALUE` token.
    if let Some((value_name, kind, tail)) = try_value(rest) {
        spec.value_name = Some(value_name);
        spec.value_kind = kind;
        rest = tail.trim();
        spec.fully_consumed = rest.is_empty();
    } else {
        spec.fully_consumed = false;
    }

    spec
}

fn skip_separators(input: &str) -> &str {
    let mut s = input;
    loop {
        let trimmed = s.trim_start_matches([' ', ',', '|']);
        if trimmed.len() == s.len() {
            return trimmed;
        }
        s = trimmed;
    }
}

fn short_dash(input: &mut &str) -> Res<char> {
    '-'.parse_next(input)
}

fn short_char(input: &mut &str) -> Res<char> {
    one_of(|c: char| c != ' ' && c != ',' && c != '=' && c != '[' && c != '-').parse_next(input)
}

fn long_dashes<'s>(input: &mut &'s str) -> Res<&'s str> {
    "--".parse_next(input)
}

fn long_name<'s>(input: &mut &'s str) -> Res<&'s str> {
    take_while(1.., |c: char| c.is_alphanumeric() || c == '-').parse_next(input)
}

/// `-x` where `x` is any non-separator, non-bracket character.
fn try_short(input: &str) -> Option<(char, &str)> {
    let mut s = input;
    short_dash(&mut s).ok()?;
    let c = short_char(&mut s).ok()?;
    Some((c, s))
}

/// `--long-name` (letters, digits, `-`), optionally prefixed with GNU
/// getopt_long's negatable-boolean bracket, `--[no-]long-name` or
/// `--[no]long-name`. Returns `(base_name, negatable, rest)` — `base_name`
/// never contains `[`/`]` either way.
fn try_long(input: &str) -> Option<(String, bool, &str)> {
    let mut s = input;
    long_dashes(&mut s).ok()?;
    let negatable = strip_negatable_prefix(s).is_some_and(|rest| {
        s = rest;
        true
    });
    let name = long_name(&mut s).ok()?;
    Some((name.to_string(), negatable, s))
}

/// Strips a leading `[no-]` or `[no]` from `input`, if present — the
/// bracketed negation prefix `--[no-]foo` puts right after its dashes.
/// Recognized structurally (an optional-looking bracket whose content is
/// exactly `no`/`no-`), never by which tool happens to emit it: any
/// getopt_long-style formatter uses the identical convention.
fn strip_negatable_prefix(input: &str) -> Option<&str> {
    let rest = input.strip_prefix('[')?;
    let rest = rest.strip_prefix("no")?;
    let rest = rest.strip_prefix('-').unwrap_or(rest);
    rest.strip_prefix(']')
}

fn open_bracket(input: &mut &str) -> Res<char> {
    '['.parse_next(input)
}

fn close_bracket(input: &mut &str) -> Res<char> {
    ']'.parse_next(input)
}

fn equals_sign(input: &mut &str) -> Res<char> {
    '='.parse_next(input)
}

fn value_inside_brackets<'s>(input: &mut &'s str) -> Res<&'s str> {
    take_while(1.., |c: char| c != ']').parse_next(input)
}

/// A value spec following the flag token(s): `=VALUE`, ` VALUE`,
/// `[=VALUE]`, `[VALUE]`, `<value>`, or a bare uppercase-ish word. Returns
/// `(value_name, kind, rest)`.
fn try_value(input: &str) -> Option<(String, ValueKind, &str)> {
    let mut s = input;

    // Optional-value bracketed forms: `[=VALUE]` or `[VALUE]`.
    if open_bracket(&mut s).is_ok() {
        let _has_eq = equals_sign(&mut s).is_ok();
        let name = value_inside_brackets(&mut s).ok()?;
        close_bracket(&mut s).ok()?;
        return Some((name.to_string(), ValueKind::Optional, s));
    }

    // `=VALUE`
    if equals_sign(&mut s).is_ok() {
        let (name, tail) = take_rest_value_token(s);
        return Some((name, ValueKind::Required, tail));
    }

    // ` VALUE` / `<value>` / bare token after whitespace-only separation.
    let _: Res<&str> = multispace0.parse_next(&mut s);
    if s.is_empty() {
        return None;
    }
    let (name, tail) = take_rest_value_token(s);
    if name.is_empty() {
        return None;
    }
    Some((name, ValueKind::Required, tail))
}

/// Take one "value token": either an angle/brace-delimited placeholder
/// (`<value>`, `{a|b|c}`) or a run of non-whitespace characters.
fn take_rest_value_token(input: &str) -> (String, &str) {
    let mut s = input;
    if let Some(rest) = s.strip_prefix('<') {
        if let Some(end) = rest.find('>') {
            let name = format!("<{}>", &rest[..end]);
            return (name, &rest[end + 1..]);
        }
    }
    let end = s.find(char::is_whitespace).unwrap_or(s.len());
    let name = &s[..end];
    s = &s[end..];
    (name.to_string(), s)
}

/// True if `input` (an already-isolated potential flag-spec fragment)
/// starts with something recognizable as a flag at all — used by the
/// layout parser to decide whether a line begins a new flag entry.
pub fn looks_like_flag_start(input: &str) -> bool {
    let trimmed = input.trim_start();
    trimmed.starts_with('-')
}

// --- the bundled-short-flag cluster ------------------------------------
//
// A usage synopsis that opens `[-2CDlNuVv]` or `[-AbdDefhHIJKlLnNOpqStuUvxX#]`
// is naming a *set* of bundled boolean switches in the ordinary getopt
// convention, not one flag. [`parse_flag_spec`] alone cannot see that:
// `try_short` takes the first character and `try_value` glues every
// remaining character on as a required value, so the tree gains `-2` with
// `value_name: "CDlNuVv"` and loses the other seven flags entirely. A
// fleet sweep of this machine's 2,302 `PATH` tools measured the cost at
// **58 tools and 465 destroyed flags**, an average of 8 lost flags per
// affected tool and 22 at the worst (`groff`'s `[-abcCeEgGijklNpRsStUVXzZ]`).
//
// [`parse_bundled_shorts`] recognizes the cluster so the synopsis path can
// emit one boolean flag per member instead. It is *not* wired into
// [`parse_flag_spec`], and must not be: the identical shape from an
// option-*table* row is the GCC/Clang single-dash convention
// (`-fdump-scos`, `-Wall`, `-Idirectory`) where the glued text really is a
// value, thousands of correct parses fleet-wide. Only the synopsis produces
// the collapse, so only the synopsis caller (`sections::extract_usage_flags`)
// asks this question.
//
// **Three defect families share the structural fingerprint** `short &&
// !long && value_name`, and only the first is a bundle:
//
// | family | example | is it a bundle? |
// |---|---|---|
// | bundled shorts | `tmux [-2CDlNuVv]` | **yes** |
// | single-dash long options | `cargo -Zscript`, `gcc -pass-exit-codes` | no |
// | repeated-character flags | `bpftrace -vv`, `strace [-DDD]` | no |
//
// The discriminator is the *shape of the swallowed text*, never the tool.
// The predicates below are the same ones `xtask`'s `bundling` oracle uses
// to count the defect fleet-wide, deliberately so: that detector is meant
// to read zero once this is fixed, and it can only do that if the fix and
// the measurement agree character for character on what a bundle is. Each
// one carries the real counter-example that forced it — a false positive
// here destroys a *correct* parse, which is strictly worse than leaving the
// bundle collapsed.

/// The fewest members a cluster must carry to be read as a bundle: a
/// surviving flag plus at least two swallowed ones.
///
/// Three, not two, and the difference is deliberate lost recall. At one
/// swallowed member the shape is genuinely ambiguous, and the fleet scan
/// says so out loud: of the two-character clusters, roughly half are real
/// collapses (`ssh-keygen`'s `[-hU]`, `umount`'s `[-hV]`, `ssh-agent`'s
/// `[-Dd]`) and the rest are entirely correct parses of genuine
/// multi-character single-dash flags — `rpcgen`'s `[-Sc]`/`[-Ss]`/`[-Sm]`,
/// `psfxtable`'s `[-it]`/`[-ot]`, `sg_map`'s `[-st]`, `setfont`'s `[-ou]`,
/// `mandoc`'s `[-ac]`, `which`'s `[-as]`, `xxd`'s `[-ps]`, plus `lessecho`'s
/// seven character-argument flags. Nothing about their *shape* separates the
/// two halves, so the whole class is left alone.
const MIN_CLUSTER_MEMBERS: usize = 3;

/// The fewest ASCII letters a cluster must carry before [`cluster_is_ordered`]
/// is allowed to vouch for it.
///
/// A cluster with no letters at all — `-1024`, `-0777` — is *vacuously*
/// ordered, and a glued numeric default (`[-b4096]`) would ride that
/// vacuous truth into being split into four flags. Two letters is the floor
/// at which "the letters are in order" is a statement about anything.
const MIN_ORDERED_LETTERS: usize = 2;

/// Whether `c` could be a single-character flag name, i.e. a plausible
/// member of a bundle.
///
/// ASCII alphanumeric covers every letter and digit case observed (`tmux`'s
/// `-2` is a real digit flag). `#` is the one non-alphanumeric member in the
/// fleet — the last character of `tcpdump`'s `[-AbdDefhHIJKlLnNOpqStuUvxX#]`,
/// which is `tcpdump`'s real "print packet number" switch. Nothing else is
/// admitted: the point of this predicate is to reject a *value spec*, and
/// every value-spec punctuation character (`{`, `<`, `[`, `=`, `:`, `.`,
/// `-`, `_`, `/`, `|`) is exactly what it rejects — `filefrag`'s own
/// `[-b{blocksize}[KMG]]` fails here and nowhere else, and every hyphenated
/// single-dash long option (`-pass-exit-codes`) fails here too.
fn is_bundle_member_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '#'
}

/// True when `members` carries at least [`MIN_ORDERED_LETTERS`] ASCII
/// letters and all of them are in non-decreasing case-insensitive order —
/// the *listing* convention a hand-written flag bundle follows, and one half
/// of the two-signal test.
///
/// Case is folded rather than compared raw because the convention
/// interleaves the two cases of one letter (`hH`, `lL`, `uU`, `xX`, `Vv`); a
/// raw ASCII comparison would call every one of those a break in the order.
/// Non-letters are skipped rather than ordered — `tcpdump` parks its `#` at
/// the end of an otherwise perfectly alphabetical bundle and `tmux` parks
/// its `2` at the front.
///
/// It is the *only* signal that vouches for a uniformly-cased bundle, which
/// is what it is for: `od`'s `[-abcdfilosx]`, `pod2text`'s `[-aclostu]`,
/// `showmount`'s `[-adehv]`, `e2image`'s `[-cfnp]` and `whereis`'s `[-BMS]`
/// have nothing else going for them. Against word-shaped values it stays
/// quiet on its own terms: `-oOUTFILE`, `-Ipath`, `-DMACRO`, `cargo`'s real
/// `-Zscript`, `rpcgen`'s real `-Dname` and `makewhatis`'s real `-Tutf8` are
/// every one of them unordered.
fn cluster_is_ordered(members: &str) -> bool {
    let letters = || members.chars().filter(|c| c.is_ascii_alphabetic());
    if letters().count() < MIN_ORDERED_LETTERS {
        return false;
    }
    let mut previous: Option<char> = None;
    for c in letters() {
        let folded = c.to_ascii_lowercase();
        if previous.is_some_and(|prev| folded < prev) {
            return false;
        }
        previous = Some(folded);
    }
    true
}

/// True when `swallowed` — every member after the first, i.e. exactly the
/// text [`parse_flag_spec`] would have stored as a `value_name` — contains
/// both an ASCII uppercase and an ASCII lowercase letter. The other half of
/// the two-signal test.
///
/// A value placeholder is written in one case (`file`, `size`, `mode`,
/// `prog`, `OUTFILE`, `MACRO`), so a *swallowed* run spanning both cases is
/// not a placeholder at all — it is a switch set that inherited whatever
/// cases its tool's flags happen to have. This carries every real bundle
/// whose author listed the switches unsorted, roughly a third of them, which
/// [`cluster_is_ordered`] alone would miss entirely: `tree`'s
/// `[-acdfghilnpqrstuvxACDFJQNSUX]` (a sorted lowercase run then a sorted
/// uppercase one — sorted twice, so not sorted), `e2fsck`'s `[-panyrcdfktvDFV]`,
/// `tic`'s `[-1aCDcfGgIKLNrsTtUx]`, `mkfs.ext4`'s `[-jnqvDFSV]`,
/// `badblocks`'s `[-svwnfBX]`, `zipinfo`'s `[-12smlvChMtTz]`.
///
/// Measured on the *swallowed* half rather than the whole cluster
/// deliberately, and the difference is the entire single-dash-long-option
/// population: `-Zscript`, `-Dname`, `-Tutf8`, `-Idirectory` all mix case as
/// clusters (an uppercase flag letter with a lowercase word glued on) and
/// are all completely correct parses. Their swallowed halves — `script`,
/// `name`, `utf8`, `directory` — do not mix, and that is what tells them
/// apart from a bundle.
fn swallowed_members_mix_case(swallowed: &str) -> bool {
    swallowed.chars().any(|c| c.is_ascii_uppercase())
        && swallowed.chars().any(|c| c.is_ascii_lowercase())
}

/// True when every character of `members` is distinct, compared
/// case-sensitively.
///
/// A bundle is a *set* of switches, so it never repeats one. Case matters:
/// `-v` and `-V` are different flags and real bundles carry both (`Vv` in
/// `tmux`, `uU`/`xX`/`hH`/`lL` in `tcpdump`), so folding case here would
/// reject the very cases this exists for. Against words it is a weak filter
/// on its own (`file`, `size` and `mode` all have distinct letters) and a
/// decisive one against the commonest doubled-letter shapes — `-Wall`,
/// `-ldl`, and the whole repeated-character family (`-vvv`, `strace`'s real
/// `[-DDD]`).
fn members_are_distinct(members: &str) -> bool {
    let mut seen = HashSet::new();
    members.chars().all(|c| seen.insert(c))
}

/// Read `token` — one whitespace-delimited synopsis token, brackets already
/// stripped by the caller — as a bundle of single-character boolean short
/// flags, returning its members in source order.
///
/// `None` unless **all** of these hold, each condition rejecting a specific
/// real counter-example (see the module-level notes above and each
/// predicate's own doc comment):
///
/// 1. The token is exactly one `-` followed by member characters, with no
///    whitespace, no second dash, and nothing else. This is the load-bearing
///    separator check: `tmux`'s own synopsis writes `[-c shell-command]`,
///    `[-f file]`, `[-L socket-name]`, `[-S socket-path]` and `[-T features]`
///    on the same physical line as its `[-2CDlNuVv]`, and every one of those
///    is a genuine value-taking short flag distinguished *only* by the space.
/// 2. Every member is a plausible single-character flag name
///    ([`is_bundle_member_char`]).
/// 3. There are at least [`MIN_CLUSTER_MEMBERS`] of them.
/// 4. They are pairwise distinct ([`members_are_distinct`]).
/// 5. Either the cluster is alphabetized ([`cluster_is_ordered`]) or the
///    swallowed half spans both cases ([`swallowed_members_mix_case`]) — two
///    independent pieces of evidence for "this is a switch set, not a word",
///    each carrying a large real family the other cannot see.
///
/// The knowing false negatives, both measured on the fleet and both left
/// alone under the no-false-positives rule: **unsorted, uniformly-cased
/// bundles** (`rpcbind`'s `[-adhilswfr]`, `umount.nfs`'s `[-fvnrlh]`,
/// `fc-validate`'s `[-Vhv]`) fire neither signal and are indistinguishable
/// on shape from a lowercase word; **bundles that repeat a switch to mean
/// "more of it"** (`strace`'s `[-ACdffhiqqrtttTvVwxxyyzZ]`,
/// `wpa_supplicant`'s `[-BddhKLqqstuvW]`) are rejected by condition 4.
pub fn parse_bundled_shorts(token: &str) -> Option<Vec<char>> {
    let members = token.strip_prefix('-')?;
    if members.chars().count() < MIN_CLUSTER_MEMBERS {
        return None;
    }
    if !members.chars().all(is_bundle_member_char) {
        return None;
    }
    if !members_are_distinct(members) {
        return None;
    }
    let swallowed: String = members.chars().skip(1).collect();
    if !cluster_is_ordered(members) && !swallowed_members_mix_case(&swallowed) {
        return None;
    }
    Some(members.chars().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_short_and_long() {
        let spec = parse_flag_spec("-i, --interactive");
        assert_eq!(spec.short, Some('i'));
        assert_eq!(spec.long.as_deref(), Some("interactive"));
        assert!(spec.fully_consumed);
    }

    #[test]
    fn parses_long_only() {
        let spec = parse_flag_spec("--autosquash");
        assert_eq!(spec.short, None);
        assert_eq!(spec.long.as_deref(), Some("autosquash"));
        assert!(spec.fully_consumed);
    }

    #[test]
    fn parses_required_value_with_equals() {
        let spec = parse_flag_spec("--format=FORMAT");
        assert_eq!(spec.long.as_deref(), Some("format"));
        assert_eq!(spec.value_name.as_deref(), Some("FORMAT"));
        assert_eq!(spec.value_kind, ValueKind::Required);
    }

    #[test]
    fn parses_required_value_with_space_and_angle_brackets() {
        let spec = parse_flag_spec("-o, --output <FILE>");
        assert_eq!(spec.short, Some('o'));
        assert_eq!(spec.long.as_deref(), Some("output"));
        assert_eq!(spec.value_name.as_deref(), Some("<FILE>"));
        assert_eq!(spec.value_kind, ValueKind::Required);
    }

    #[test]
    fn parses_optional_bracketed_value() {
        let spec = parse_flag_spec("--occurrence[=NUMBER]");
        assert_eq!(spec.long.as_deref(), Some("occurrence"));
        assert_eq!(spec.value_name.as_deref(), Some("NUMBER"));
        assert_eq!(spec.value_kind, ValueKind::Optional);
    }

    #[test]
    fn parses_multiple_long_aliases_keeping_first() {
        let spec = parse_flag_spec("-A, --catenate, --concatenate");
        assert_eq!(spec.short, Some('A'));
        assert_eq!(spec.long.as_deref(), Some("catenate"));
        assert!(spec.fully_consumed);
    }

    #[test]
    fn parses_gpg_sign_style_optional_short_value() {
        let spec = parse_flag_spec("-S, --gpg-sign[=<keyid>]");
        assert_eq!(spec.short, Some('S'));
        assert_eq!(spec.long.as_deref(), Some("gpg-sign"));
        assert_eq!(spec.value_kind, ValueKind::Optional);
    }

    /// GNU getopt_long's negatable-boolean convention, `--[no-]name` (git's
    /// own `--help` formatter uses it for every negatable boolean). Before
    /// this, `try_long` required an alphanumeric immediately after `--`,
    /// so `--[no-]staged` matched neither `try_short` nor `try_long` at
    /// all. The recovered `long` must be the base name, never containing
    /// `[`/`]`.
    #[test]
    fn parses_negatable_long_with_short() {
        let spec = parse_flag_spec("-S, --[no-]staged");
        assert_eq!(spec.short, Some('S'));
        assert_eq!(spec.long.as_deref(), Some("staged"));
        assert!(spec.negatable);
        assert!(spec.fully_consumed);
    }

    #[test]
    fn parses_negatable_long_only() {
        let spec = parse_flag_spec("--[no-]ignore-unmerged");
        assert_eq!(spec.short, None);
        assert_eq!(spec.long.as_deref(), Some("ignore-unmerged"));
        assert!(spec.negatable);
    }

    /// `--[no-]source <tree-ish>`: the negatable prefix and a required
    /// value spec must compose, since git uses both together.
    #[test]
    fn parses_negatable_long_with_value_spec() {
        let spec = parse_flag_spec("-s, --[no-]source <tree-ish>");
        assert_eq!(spec.short, Some('s'));
        assert_eq!(spec.long.as_deref(), Some("source"));
        assert!(spec.negatable);
        assert_eq!(spec.value_name.as_deref(), Some("<tree-ish>"));
        assert_eq!(spec.value_kind, ValueKind::Required);
    }

    /// Control case: a flag with no `[no-]` prefix must come back with
    /// `negatable: false`, unaffected.
    #[test]
    fn non_negatable_flag_is_unaffected() {
        let spec = parse_flag_spec("-2, --ours");
        assert_eq!(spec.long.as_deref(), Some("ours"));
        assert!(!spec.negatable);
    }

    #[test]
    fn messy_remainder_is_not_fully_consumed_but_still_yields_flags() {
        // A value spec winnow's simple grammar doesn't fully understand
        // (nested brackets) should still recover the flag identity.
        let spec = parse_flag_spec("--sparse-version=MAJOR[.MINOR]");
        assert_eq!(spec.long.as_deref(), Some("sparse-version"));
        assert!(spec.value_name.is_some());
    }

    #[test]
    fn looks_like_flag_start_true_for_dash() {
        assert!(looks_like_flag_start("-i, --interactive"));
        assert!(looks_like_flag_start("    --autosquash"));
    }

    #[test]
    fn looks_like_flag_start_false_for_bare_word() {
        assert!(!looks_like_flag_start("clone     Clone a repository"));
    }
}
