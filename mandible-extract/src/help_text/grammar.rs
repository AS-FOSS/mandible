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

use mandible_core::{Dashes, Spelling, ValueKind};
use std::collections::HashSet;
use winnow::ascii::multispace0;
use winnow::error::ContextError;
use winnow::prelude::*;
use winnow::token::take_while;

/// This grammar never needs winnow's richer error-context machinery — a
/// flag-spec fragment either matches the recognized shape or it doesn't,
/// and callers fall back to a best-effort split either way — so every
/// parser function here is pinned to the same concrete error type.
type Res<T> = ModalResult<T, ContextError>;

/// The result of parsing one flag-spec fragment.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FlagSpec {
    /// Every recognized spelling, in document order: `-A, --catenate,
    /// --concatenate` keeps all three, not just the first of each shape —
    /// the fix that dissolves the multi-spelling bug (#30) at its source.
    pub spellings: Vec<Spelling>,
    /// The value placeholder text, if a value spec was recognized.
    pub value_name: Option<String>,
    /// Whether the value (if any) is required or optional.
    pub value_kind: ValueKind,
    /// True if the grammar consumed the entire fragment cleanly (no
    /// leftover text it didn't understand). Used for confidence scoring.
    pub fully_consumed: bool,
}

impl FlagSpec {
    /// The short letter, if this spec has a short-flag-shaped spelling —
    /// same shape rule as [`mandible_core::Entity::short`]: one dash, one
    /// character, no abbreviation bracket.
    pub fn short(&self) -> Option<char> {
        self.spellings
            .iter()
            .find(|s| matches!(s.dashes, Dashes::Single) && s.name.chars().count() == 1)
            .and_then(|s| s.name.chars().next())
    }

    /// The long-like spelling's bare name, if any — same shape rule as
    /// [`mandible_core::Entity::long`]: two dashes always, or one dash when
    /// the name is more than a single character.
    pub fn long(&self) -> Option<&str> {
        self.spellings
            .iter()
            .find(|s| {
                matches!(s.dashes, Dashes::Double)
                    || (matches!(s.dashes, Dashes::Single) && s.name.chars().count() > 1)
            })
            .map(|s| s.name.as_str())
    }

    /// True when any spelling documents the `--[no-]name` negation
    /// convention. Test-only: production code reads `negatable` straight
    /// off each `Spelling` once `spellings` moves onto the `Entity`, with
    /// no need to ask the whole spec first.
    #[cfg(test)]
    pub fn negatable(&self) -> bool {
        self.spellings.iter().any(|s| s.negatable)
    }
}

/// Parse a flag-spec fragment (the part of a `--help` entry line before
/// the description column — already isolated by the layout parser).
pub fn parse_flag_spec(input: &str) -> FlagSpec {
    let normalized = unwrap_brace_alternation(input.trim());
    let mut rest = normalized.as_ref().trim();
    let mut spec = FlagSpec::default();

    loop {
        // One run of alias spellings: `-p`, `--pid`, `-A, --catenate`.
        //
        // Once **two** long-like spellings (`--foo`, or a single-dash long
        // like `-help`) in a row would meet on nothing but bare
        // whitespace, the second one needs an *explicit* `,`/`|` before
        // it instead. A short spelling may still run into a long one (or
        // vice versa) on bare whitespace — `iptables --help`'s real
        // `--replace -R chain rulenum` row (long-then-short, one flag,
        // documented in that order) and `jdeprscan`'s real `-? -h` table
        // cell (short-then-short) both need this — which is why the gate
        // triggers only when *both* the spelling just read and the one
        // about to be read are long-like, not merely on position. Without
        // it, a run of several genuinely distinct long options simply
        // space-separated on one line — `pod2html`'s real `--quiet
        // --noquiet --verbose --noverbose` usage-synopsis row, four
        // independently negatable flags, no comma anywhere — reads as one
        // flag's ever-growing alias list. Now that every spelling found
        // here is *kept* (not just the first of each shape), that false
        // read stops being silently dropped and starts being an actively
        // fabricated multi-spelling entity — worse than the defect this
        // loop existed to avoid.
        let mut last_was_long_like = false;
        loop {
            let before = rest;
            rest = skip_separators(rest);
            if rest.is_empty() {
                break;
            }
            let explicit = saw_explicit_separator(before, rest);
            if let Some((spelling, tail)) = try_short(rest) {
                if last_was_long_like && is_long_like(&spelling) && !explicit {
                    break;
                }
                last_was_long_like = is_long_like(&spelling);
                spec.spellings.push(spelling);
                rest = tail;
                continue;
            }
            if let Some((spelling, tail)) = try_long(rest) {
                if last_was_long_like && is_long_like(&spelling) && !explicit {
                    break;
                }
                last_was_long_like = is_long_like(&spelling);
                spec.spellings.push(spelling);
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

        // Whatever is left never becomes a value if it itself parses as
        // another flag spelling: a real value placeholder is never
        // flag-shaped, and the one case that reaches here — the alias
        // loop above refusing a further long-like spelling with no
        // explicit separator (`pod2html`'s real `--quiet --noquiet
        // --verbose --noverbose`) — must not fall back to reading the
        // next flag's own name as this flag's *value*. Honest
        // incompleteness (`fully_consumed: false`, no invented
        // `value_name`) over a guess.
        if try_short(rest).is_some() || try_long(rest).is_some() {
            spec.fully_consumed = false;
            return spec;
        }

        // Whatever remains is treated as a value spec: `=VALUE`, ` VALUE`,
        // `[=VALUE]`, `[VALUE]`, or a bare `<value>`/`VALUE` token.
        let Some((value_name, kind, tail)) = try_value(rest) else {
            spec.fully_consumed = false;
            return spec;
        };
        // First value wins: a repeated placeholder (`-p PID, --pid PID`)
        // names one value, once, however many spellings carry it.
        if spec.value_name.is_none() {
            spec.value_name = Some(value_name);
            spec.value_kind = kind;
        }

        // ...and the alias list may continue past it. See
        // [`alias_continues`] for why only an explicit `,`/`|` may resume
        // the run, and never whitespace alone.
        let Some(next) = alias_continues(tail) else {
            spec.fully_consumed = tail.trim().is_empty();
            return spec;
        };
        rest = next;
    }
}

// --- the alias list a value spec used to terminate ----------------------
//
// A flag-spec fragment is a run of spellings followed by a value spec, and
// reading it that way is correct for `-o, --output FILE`, where the value
// really is last. It is wrong for every formatter that repeats the
// placeholder after each spelling — which is what Python's `argparse` does
// by default, and what the `sg_*` family does with pipes:
//
// ```text
//   -p PID, --pid PID       trace this PID only        (argparse)
//   --count=OC|-c OC        OC is overwrite count      (sg_sanitize)
// ```
//
// `try_short` took `-p`, `try_value` took everything after it as one token
// (`PID,` — the separator rode along), and `--pid` was discarded. A fleet
// oracle for the defect lives in `xtask/src/dropped_alias.rs`; this is the
// half that closes it, and the two share their rule deliberately, exactly
// as `parse_bundled_shorts` shares its five conditions with
// `xtask/src/bundling.rs`: a detector meant to be ratcheted at zero and a
// fix meant to reach zero have to agree on what the defect *is*, or the
// zero means nothing.
//
// **The hazard runs the opposite way to the bundle's.** There, a false
// positive destroyed a correct parse. Here, a false positive *merges two
// genuinely different flags*, which is worse still: dropping a spelling
// loses information a user can recover from `--help`, while merging
// invents an alias the tool does not have, and a user who types it gets an
// error. Both predicates below are written against that, not against
// recall.

/// True when `c` separates the spellings of one flag rather than belonging
/// to a value: `,` (argparse, GNU getopt_long, tar) or `|` (the `sg_*`
/// family, and every synopsis alternation).
///
/// Whitespace is deliberately *not* a member. A bare space between two
/// spellings is already handled by [`skip_separators`], and a wide run of
/// spaces is the description column — so admitting whitespace here would
/// turn `--output FILE --other` into an alias claim about two unrelated
/// flags, which is the fabrication this whole change is written against.
fn is_alias_separator(c: char) -> bool {
    c == ',' || c == '|'
}

/// True when `after_separator` — the text immediately following a `,` or
/// `|` — really is the next spelling in an alias list.
///
/// Two conditions, and the second is the load-bearing one:
///
/// 1. A spelling parses there at all ([`try_short`]/[`try_long`]).
/// 2. **What follows that spelling terminates it.** A `}`/`)`/`]` directly
///    after means the dash was inside a bracketed *value*, not on the right
///    of an alias separator: `{a,-b}` is a choice list whose member happens
///    to start with a dash, and without this condition its `-b` would
///    become a second spelling of the flag. Real aliases are always
///    followed by whitespace, end-of-fragment, their own value spec
///    (`=`/`[`), or another separator.
fn alias_follows(after_separator: &str) -> bool {
    let after = after_separator.trim_start_matches(' ');
    let Some(tail) = try_long(after)
        .map(|(_, t)| t)
        .or_else(|| try_short(after).map(|(_, t)| t))
    else {
        return false;
    };
    tail.is_empty() || tail.starts_with([' ', '\t', '=', '[', ',', '|'])
}

/// True when `before_separator` — the value text taken so far — is
/// something an alias separator could actually be separating *from*.
///
/// A separator sits between two things. Its left side has to be a finished
/// value placeholder, which means it ends in a word character or in the
/// closer of a bracketed one (`{java,perl,tcl}`, `<file>`, `[n]`). It is
/// not enough that a separator is present.
///
/// The measured counter-example is `lsof`, whose help writes
/// `+|-e s  exempt s *RISKY*`: a "plus-or-minus" convention meaning `+e` or
/// `-e`, which is a third shape this grammar does not model at all. Its `|`
/// has `+` on the left, so nothing is being separated from a placeholder,
/// and without this condition `lsof` would gain an `-e` carrying the
/// literal value `+`. Left alone deliberately — `-e` is a real `lsof` flag
/// and recovering it is a real gain, but not one this change is entitled to
/// take as a side effect, and not with a placeholder that is wrong.
fn separator_has_a_left_operand(before_separator: &str) -> bool {
    before_separator
        .chars()
        .next_back()
        .is_some_and(|c| c.is_alphanumeric() || c == '_' || matches!(c, '}' | ')' | ']' | '>'))
}

/// The rest of an alias list continuing past a value spec, or `None`.
///
/// `after_value` is the text [`try_value`] left behind. An explicit
/// separator must be the next non-space character there — a space alone
/// never resumes the run ([`is_alias_separator`]) — and a whole spelling
/// must follow it ([`alias_follows`]).
fn alias_continues(after_value: &str) -> Option<&str> {
    let s = after_value.trim_start_matches(' ');
    let separator = s.chars().next().filter(|c| is_alias_separator(*c))?;
    let rest = &s[separator.len_utf8()..];
    alias_follows(rest).then(|| rest.trim_start_matches(' '))
}

/// Rewrite a brace-delimited alternation of flag spellings into the
/// comma-free alias list the rest of [`parse_flag_spec`] already reads:
/// `{-i|--input} <input xml file>` becomes `-i --input <input xml file>`.
/// Anything else is returned untouched, borrowed.
///
/// **Braces only.** A leading `[` is left alone here for the reason
/// [`looks_like_flag_start`] gives: in an options *table* a bracket means
/// "optional" far more often than it introduces an entry, and this function
/// runs on every table row in the fleet. The synopsis path, where a bracket
/// group is already understood as a group, handles `[` itself.
///
/// The rewrite is a normalization rather than a new parse: `skip_separators`
/// already treats `|` and whitespace as alias separators, so once the
/// delimiters are gone the existing short/long loop reads
/// `{-h|--help}` exactly the way it reads `-h, --help`.
fn unwrap_brace_alternation(input: &str) -> std::borrow::Cow<'_, str> {
    match parse_flag_alternation(input) {
        Some(alt) if alt.open == '{' => {
            std::borrow::Cow::Owned(format!("{} {}", alt.members.join(" "), alt.rest))
        }
        _ => std::borrow::Cow::Borrowed(input),
    }
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

/// True when the text consumed going from `before` to `after` contained an
/// explicit `,` or `|`, not whitespace alone. Used by [`parse_flag_spec`]'s
/// alias loop to require a real separator between the first spelling and
/// every one after it — see that loop's own doc comment for why bare
/// whitespace must never be enough on its own.
///
/// **Does not assume `after` is a suffix of `before`.** The one call site
/// happens to always pass a `before`/`after` pair where it is (`before` is
/// `rest` from the previous iteration, `after` is the same binding after
/// only `skip_separators` advanced it), but that is a caller-side
/// invariant this function's own two independent `&str` parameters do not
/// express — exactly the shape AGENTS.md's raw-byte-offset row warns
/// about (a box-drawing glyph elsewhere in a real `--help` capture landing
/// mid-character is what shipped that crash). `str::get` returns `None`
/// instead of panicking both when the byte count lands off a char
/// boundary and when `after` is longer than `before` (an out-of-order
/// call, or unrelated strings), so a future caller that gets the
/// invariant wrong degrades to "no separator" rather than aborting.
fn saw_explicit_separator(before: &str, after: &str) -> bool {
    let consumed_len = before.len().saturating_sub(after.len());
    before
        .get(..consumed_len)
        .is_some_and(|consumed| consumed.contains([',', '|']))
}

/// The same "long-like" shape rule [`mandible_core::Entity::long_spelling`]
/// uses: two dashes always, or one dash when the name is longer than a
/// single character — which an abbreviation-bracket spelling (`-r[esolve]`)
/// is, even though [`try_short`] is the function that read it.
fn is_long_like(spelling: &Spelling) -> bool {
    matches!(spelling.dashes, Dashes::Double)
        || (matches!(spelling.dashes, Dashes::Single) && spelling.name.chars().count() > 1)
}

fn short_dash(input: &mut &str) -> Res<char> {
    '-'.parse_next(input)
}

fn long_dashes<'s>(input: &mut &'s str) -> Res<&'s str> {
    "--".parse_next(input)
}

fn long_name<'s>(input: &mut &'s str) -> Res<&'s str> {
    take_while(1.., |c: char| c.is_alphanumeric() || c == '-').parse_next(input)
}

/// `-x` where `x` is any non-separator, non-bracket character, or
/// `-xy...[rest]` where an abbreviation-continuation bracket (see
/// [`parse_abbrev_bracket`]) immediately follows a run of one or more such
/// characters: `ip --help`'s own `-V[ersion]` (a one-letter prefix) and
/// `-rc[vbuf]` (a two-letter prefix, issue #49's duplicate-`-r` row) are
/// the same shape at different prefix lengths, and this reads both with
/// the same rule. With no bracket, only the run's first character is ever
/// consumed — a plain short flag, exactly the pre-abbreviation-model
/// behavior — so a genuinely glued value (`-2CDlNuVv`) is untouched.
fn try_short(input: &str) -> Option<(Spelling, &str)> {
    let mut s = input;
    short_dash(&mut s).ok()?;
    let run_end = s
        .char_indices()
        .find(|(_, c)| !is_short_char(*c))
        .map_or(s.len(), |(i, _)| i);
    if run_end == 0 {
        return None;
    }
    let run = &s[..run_end];
    let after_run = &s[run_end..];
    if let Some((content, rest)) = parse_abbrev_bracket(after_run) {
        let prefix_len = run.chars().count();
        if prefix_len <= MAX_ABBREV_PREFIX_LEN {
            return Some((
                Spelling {
                    name: format!("{run}{content}"),
                    dashes: Dashes::Single,
                    negatable: false,
                    abbrev: Some(prefix_len),
                },
                rest,
            ));
        }
    }
    // No abbreviation bracket (or its content didn't validate): an
    // ordinary short flag, one character, exactly as `short_char` always
    // read it — anything past that first character is left for the rest
    // of the grammar (a glued value, or its own token) to interpret.
    let c = run.chars().next()?;
    Some((Spelling::short(c), &s[c.len_utf8()..]))
}

fn is_short_char(c: char) -> bool {
    c != ' ' && c != ',' && c != '=' && c != '[' && c != '-'
}

/// The longest prefix [`try_short`]/[`try_long`] will read as an
/// abbreviation before a bracket, in characters.
///
/// Every measured real convention (`ip`'s `-r[esolve]`, `-rc[vbuf]`,
/// `-ts[hort]`, `-br[ief]`; `-V[ersion]`) is one or two letters, so three
/// is generous headroom, not a tight fit. It exists to keep the
/// abbreviation model from swallowing a usage-synopsis *placeholder*
/// written in the identical shape: `unzip`'s own `[-opts[modifiers]]`
/// means "any of the single-letter options below, optionally followed by
/// any of the modifier letters below" — `opts` and `modifiers` are plain
/// English words, not a real flag's name and its abbreviation, and
/// `"opts" + "modifiers"` is not one coherent word the way `"r" +
/// "esolve"` is. A per-tool exception list could never generalize (spec
/// §1); a length bound generalizes from the shape every real specimen
/// measured so far actually has.
const MAX_ABBREV_PREFIX_LEN: usize = 3;

/// Parses (and validates) an abbreviation-continuation bracket glued
/// directly onto a flag spelling, e.g. `ip --help`'s own `-V[ersion]`,
/// `-rc[vbuf]`, `--br[ief]`: the prefix before the bracket already *is*
/// the flag as far as identity goes, and the bracket merely spells out the
/// rest of the word it abbreviates. Mirrors [`crate::help_text::sections::
/// strip_optional_modifier_suffix`]'s command-name convention (`m[ab]`
/// names the command `m`) on the flag side, per spec's own note that the
/// two are the same shape in different documents.
///
/// Without this, [`try_value`]'s `[VALUE]` arm reads the bracket as a
/// fabricated optional value: `-V[ersion]` came out as `-V` taking an
/// optional value literally named `"ersion"` — a value the tool does not
/// document at all, on a flag (`ip -V`) that takes none.
///
/// Structural, not semantic, and narrow by construction so it cannot claim
/// a real optional-value placeholder for itself: content must open with an
/// ASCII lowercase letter and contain nothing but ASCII lowercase letters
/// and hyphens (`human-readable`). Every optional-value convention this
/// project's fleet has actually measured is one of upper/mixed-case
/// (`[FILE]`, `--occurrence[=NUMBER]`), angle-delimited (`<value>`), or
/// carries a leading `=` (`[=WHEN]`) — never a bare, all-lowercase word
/// glued straight onto a spelling with no `=` at all. A bracket containing
/// `=`, digits, uppercase letters, or anything else therefore falls
/// through untouched to the ordinary value-spec grammar below. Returns the
/// bracket's content and the text following it.
fn parse_abbrev_bracket(input: &str) -> Option<(&str, &str)> {
    let rest = input.strip_prefix('[')?;
    let close = rest.find(']')?;
    let content = &rest[..close];
    let mut chars = content.chars();
    let first = chars.next()?;
    if !first.is_ascii_lowercase() {
        return None;
    }
    if !content.chars().all(|c| c.is_ascii_lowercase() || c == '-') {
        return None;
    }
    let mut after = &rest[close + 1..];
    // Drop stray closing punctuation immediately following the bracket —
    // `ip --help`'s own `OPTIONS := { -V[ersion] | ... | -c[olor]}` cuts
    // its last alternative's row with the alternation group's own closing
    // `}` still glued on, with nothing between it and the bracket this
    // function just closed. Nothing about the abbreviation convention
    // produces trailing punctuation of its own, so a `}`/`)`/`]` sitting
    // right here can only be leftover from an enclosing group this
    // fragment was cut out of — never a value. Without this, `-c[olor]}`
    // moved from a merely-doubtful `value_name: "olor"` (this convention's
    // old, wrong-but-not-punctuation reading) to an outright
    // `value_name: "}"`, `Required` — a fabrication in the exact flag this
    // function exists to stop fabricating.
    while matches!(after.chars().next(), Some('}' | ')' | ']')) {
        after = &after[1..];
    }
    Some((content, after))
}

/// `--long-name` (letters, digits, `-`), optionally prefixed with GNU
/// getopt_long's negatable-boolean bracket, `--[no-]long-name` or
/// `--[no]long-name`, and optionally suffixed with an abbreviation
/// bracket, `--br[ief]` (see [`parse_abbrev_bracket`]) — the two never
/// coexist on one spelling in the measured fleet, and if they did,
/// negatable wins (checked first, so an abbreviation bracket is only ever
/// looked for once the `[no-]` prefix — if any — is already consumed).
fn try_long(input: &str) -> Option<(Spelling, &str)> {
    let mut s = input;
    long_dashes(&mut s).ok()?;
    let negatable = strip_negatable_prefix(s).is_some_and(|rest| {
        s = rest;
        true
    });
    let name = long_name(&mut s).ok()?;
    if !negatable && name.chars().count() <= MAX_ABBREV_PREFIX_LEN {
        if let Some((content, rest)) = parse_abbrev_bracket(s) {
            return Some((
                Spelling {
                    name: format!("{name}{content}"),
                    dashes: Dashes::Double,
                    negatable: false,
                    abbrev: Some(name.chars().count()),
                },
                rest,
            ));
        }
    }
    Some((
        Spelling {
            name: name.to_string(),
            dashes: Dashes::Double,
            negatable,
            abbrev: None,
        },
        s,
    ))
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
///
/// The run also stops at an alias separator that the next spelling follows
/// ([`alias_follows`]) — the boundary a placeholder shares with the rest of
/// its alias list when no space separates them. `sg_sanitize`'s real
/// `--count=OC|-c OC` has no whitespace anywhere in `OC|-c`, so without
/// this the whole thing became the value and `-c` was lost inside it.
///
/// A separator the check rejects is kept, which is the entire reason the
/// check is `alias_follows` and not merely "a separator is here":
/// `{java,perl,php,python,ruby,tcl}` carries five commas and is one
/// placeholder, and `jdeprscan`'s `7|8|9|10|11|12|13|14|15|16|17` carries
/// ten pipes and is one placeholder.
fn take_rest_value_token(input: &str) -> (String, &str) {
    let s = input;
    if let Some(rest) = s.strip_prefix('<') {
        if let Some(end) = rest.find('>') {
            let name = format!("<{}>", &rest[..end]);
            return (name, &rest[end + 1..]);
        }
    }
    let end = s
        .char_indices()
        .find(|(i, c)| {
            c.is_whitespace()
                || (is_alias_separator(*c)
                    && separator_has_a_left_operand(&s[..*i])
                    && alias_follows(&s[i + c.len_utf8()..]))
        })
        .map_or(s.len(), |(i, _)| i);
    (s[..end].to_string(), &s[end..])
}

/// True if `input` (an already-isolated potential flag-spec fragment)
/// starts with something recognizable as a flag at all — used by the
/// layout parser to decide whether a line begins a new flag entry.
///
/// The `-` prefix is the original and still the dominant answer. The second
/// arm is a **brace-delimited alternation of bare flag spellings**,
/// `{-i|--input}` / `{--omit-clean-shutdown}` — `cache_restore` writes every
/// row of its own `Options:` block that way and lost all eight flags to this
/// check returning `false`. Braces only, never brackets, and the asymmetry
/// is deliberate: a leading `[` in an options block means "optional" far
/// more often than it introduces a flag entry, while a leading `{` around
/// nothing but flag spellings has no other reading. See
/// [`parse_flag_alternation`] for what "bare flag spellings" is allowed to
/// mean — it is the same rule the synopsis path and `xtask`'s detector both
/// call, never a second copy of it.
pub fn looks_like_flag_start(input: &str) -> bool {
    let trimmed = input.trim_start();
    if trimmed.starts_with('-') {
        return !is_dash_underline_token(first_token(trimmed));
    }
    parse_flag_alternation(trimmed).is_some_and(|alt| alt.open == '{')
}

/// The first whitespace-delimited token of `input`, or the whole string
/// when it carries none.
fn first_token(input: &str) -> &str {
    input.split_whitespace().next().unwrap_or(input)
}

/// True when `token` is nothing but a run of 3 or more dashes — jmod's own
/// header-underline row, `------  -----------`, printed directly under a
/// two-column `Option`/`Description` heading to separate it from the rows
/// beneath. Every character in the token's own would-be spelling is `-`,
/// so admitting it as a flag row fabricates a flag named `----` (the two
/// leading dashes stripped as the long-flag marker, the rest read as its
/// name) — real structure invented from a table border, not a value the
/// tool documents.
///
/// The threshold is 3, not 2: `--` alone is a real, meaningful token in
/// many tools (GNU getopt's end-of-options marker, spelled as its own row
/// in some `--help` conventions) and must stay eligible to open a flag
/// entry. A dash run of 3 or more, with no other character anywhere in it,
/// has no such reading — it is always table decoration, never a spelling.
pub fn is_dash_underline_token(token: &str) -> bool {
    token.len() >= 3 && token.bytes().all(|b| b == b'-')
}

// --- the docopt bracket-group flag row ----------------------------------
//
// LVM's own help emitter (`vgck`, `vgextend`, `vgrename`, and every other
// `lv*`/`vg*`/`pv*` binary) writes one flag per physical line as a whole
// `[...]` group, never a `-`-prefixed row and never a `{...}` alternation:
//
// ```text
//   [ -d|--debug ]
//   [    --commandprofile String ]
//   [ -A|--autobackup y|n ]
//   [ --metadatasize Size[m|UNIT] ]
// ```
//
// This is a *third* row shape [`looks_like_flag_start`] cannot be widened
// to cover — see that function's own doc comment and the trap recorded in
// this fix's PR: `lsof`'s usage-block continuation lines also open with
// `[` (`[-F [f]]`), and that predicate doubles as the usage block's own
// terminator (`sections::parse_body`'s "a continuation that reads as a
// flag row ends the block" guard). Widening it to accept `[` would make
// `lsof`'s own continuation line satisfy it, ending the block one line in
// and losing the six flags documented only in later continuation lines.
//
// So this is a **separate, row-level** predicate, consulted only at the
// two places that ask "is this physical line a flag-table entry" —
// `flags_block_start`/`scan_flags_block` for the headed `Common options
// for lvm:` block, and `extract_usage_flags` for the bracket rows that
// continue a bare, unlabelled `vgck`/`vgextend VG PV ...` synopsis line —
// never the usage-block-continuation question above.

/// The inner content of a [`looks_like_bracket_flag_row`] line — the
/// cluster of `-`/`--` spellings plus, when present, the value spec that
/// follows them — or `None` if `input` is not that shape.
///
/// Two conditions, both required:
///
/// 1. `input`, once trimmed, is *exactly one* bracket group: nothing
///    before the `[`, nothing but whitespace after its matching `]`. A
///    row with a description trailing the group, or with a second group,
///    is not a shape this reads.
/// 2. The group's content, trimmed, starts with `-`. This is what turns
///    away every *operand* LVM writes in the identical notation —
///    `[ COMMON_OPTIONS ]` (a cross-reference to this very block, no
///    dash at all) and `[ VG|Tag ... ]` / `[ VG PV ... ]` (positionals) —
///    while admitting every real flag row, because every one of LVM's
///    flag rows opens with a dash and no operand row ever does.
///
/// The returned content is handed to [`parse_flag_spec`] unchanged by
/// every caller: once the outer brackets are gone, `-d|--debug`,
/// `--commandprofile String`, `-A|--autobackup y|n` and `--metadatasize
/// Size[m|UNIT]` are all shapes that grammar already reads correctly —
/// `|` is already an alias separator ([`is_alias_separator`]), and
/// [`take_rest_value_token`]'s `alias_follows` check already keeps a
/// value's own `|` (`y|n`, `Size[m|UNIT]`) from being misread as a second
/// alias, which is the same hazard `sg_sanitize`'s `--count=OC|-c OC` is
/// there for. No second value-vs-alias parser is written for this row
/// shape; the existing one already closes the ambiguity spec [7] Tier B
/// worries about here (`--color={always|never|auto}`), because
/// `is_bare_flag_spelling` is never even consulted by this path — a
/// leftover value spec becomes `value_name` exactly as it always has.
pub fn bracket_flag_row_content(input: &str) -> Option<&str> {
    let trimmed = input.trim();
    if !trimmed.starts_with('[') {
        return None;
    }
    let (content, rest) = split_at_matching_close(trimmed, '[', ']')?;
    if !rest.trim().is_empty() {
        return None;
    }
    let content = content.trim();
    if !content.starts_with('-') {
        return None;
    }
    // Refuse a row whose alias run does not actually finish at the first
    // whitespace gap — `ethtool --help`'s
    // `[ --all-groups | --groups [eth-phy] [eth-mac] [eth-ctrl] [rmon] ]`
    // is not LVM's shape at all: it is an alternation between *two
    // different* flags, only one of which carries operands, glued into
    // one bracket group. Every LVM row's aliases are unseparated by
    // whitespace (`-A|--autobackup`, `-d|--debug`) with the value (if any)
    // starting only *after* the alias run ends — so a `|` reappearing
    // right after that first whitespace gap means the alias run never
    // actually ended there, and what follows is a second alternative this
    // function's single-flag reading cannot honestly attribute. Naively
    // parsing this via `parse_flag_spec` reads `--all-groups` as carrying
    // the value `eth-phy` and drops `--groups` entirely — a real flag
    // lost to a fabricated one. Refusing the whole row is the same choice
    // [`is_bare_flag_spelling`]'s own doc comment already makes for
    // `[--count=OC|-c OC]`: missing beats invented.
    if let Some(gap) = top_level_whitespace(content) {
        if content[gap..].trim_start().starts_with('|') {
            return None;
        }
    }
    Some(content)
}

/// The byte index of the first whitespace character in `content` that
/// sits outside any `[...]`/`{...}` nesting — the boundary between a
/// bracket-group flag row's alias run and its value spec, when there is
/// one. `None` when no such whitespace exists (a boolean row like
/// `--nolocking`, or `-d|--debug`).
fn top_level_whitespace(content: &str) -> Option<usize> {
    let mut depth = 0i32;
    for (i, c) in content.char_indices() {
        match c {
            '[' | '{' => depth += 1,
            ']' | '}' => depth -= 1,
            c if c.is_whitespace() && depth == 0 => return Some(i),
            _ => {}
        }
    }
    None
}

/// True if `input` is a [`bracket_flag_row_content`] row — a whole
/// physical line consisting of exactly one `[...]` group whose content
/// opens with a dash.
pub fn looks_like_bracket_flag_row(input: &str) -> bool {
    bracket_flag_row_content(input).is_some()
}

// --- the parenthesized alternation group --------------------------------
//
// LVM's own emitter writes a *different* multi-mode-synopsis shape for a
// tool whose flags satisfy "any one of these is required, after which the
// rest are optional" (`vgchange`'s own first stanza, `lvconvert`'s stanzas
// for `--merge`/`--splitmirrors`/etc. use the same convention): one bare
// `(` opens a group, one flag entry per physical line separated by a
// trailing `,`, and the group's last entry closes with `)` on its own
// line — never `[`-brackets, and spanning many physical lines rather than
// the one-flag-per-`[...]`-line shape [`bracket_flag_row_content`] reads:
//
// ```text
//   vgchange
//   ( -l|--logicalvolume Number,
//     -p|--maxphysicalvolumes Number,
//     -u|--uuid,
//     ...
//        --setautoactivation y|n )
// ```
//
// A member row routinely opens with `-` itself (`-p|--maxphysicalvolumes
// Number,`), which [`looks_like_flag_start`] already treats as unambiguous
// evidence that a usage-block continuation line is really a flag-table row
// ending the block — correct for the shapes that predicate was built for,
// wrong here, where the row is one alternative inside a still-open group
// rather than a fresh table. Distinguishing the two is a matter of
// *running paren depth*, not per-line content, so `sections::parse_body`'s
// usage-block loop tracks it directly with [`paren_depth_delta`] rather
// than asking this module to re-decide "is this line still inside the
// group" from content alone on every line.

/// True if `t` (already left-trimmed) opens a multi-line parenthesized
/// alternation group: a bare `(` immediately followed by a flag token, with
/// the group left unclosed on this same physical line (an ordinary
/// same-line parenthetical aside, `(see below)`, always balances and is
/// refused here). A name match alone is not required — unlike
/// [`looks_like_unlabeled_synopsis_line`](super::sections::looks_like_unlabeled_synopsis_line),
/// this predicate is consulted only where the caller has already
/// established the surrounding line is a synopsis head or one of its
/// continuations, so the flag-token-right-after-`(` shape alone is the
/// evidence — prose essentially never opens a line with `(` followed
/// immediately by a bare `-`-prefixed word.
pub fn looks_like_paren_alternation_open(t: &str) -> bool {
    if paren_depth_delta(t) <= 0 {
        return false;
    }
    let Some(rest) = t.strip_prefix('(') else {
        return false;
    };
    rest.split_whitespace()
        .next()
        .is_some_and(is_bare_flag_token)
}

/// Running paren-depth contribution of one physical line: `(` count minus
/// `)` count. Shared by `sections::parse_body`'s usage-block loop (which
/// must keep treating a member row as "still inside the group" for as long
/// as the count stays above zero, regardless of what the row's own text
/// looks like) and `sections::extract_usage_flags`'s own pass over the
/// same physical lines afterward (which needs to know which lines the
/// group covers to hand them to [`paren_alternation_member_content`]
/// instead of the ordinary per-line segment walk) — one depth rule, not
/// two.
pub fn paren_depth_delta(t: &str) -> i32 {
    t.matches('(').count() as i32 - t.matches(')').count() as i32
}

/// True if `word` is a bare flag token — starts with `-`, is not just
/// `-`/`--` alone, and the dash(es) are immediately followed by an
/// alphanumeric character (so `--`, `-`, or a lone `-` used as a
/// stdin/stdout placeholder never counts).
pub(super) fn is_bare_flag_token(word: &str) -> bool {
    word.len() > 1
        && word.starts_with('-')
        && word
            .trim_start_matches('-')
            .starts_with(|c: char| c.is_alphanumeric())
}

/// The inner content of one member row inside an open
/// [`looks_like_paren_alternation_open`] group — the row with its opening
/// `(` (the group's first row only), trailing `,` (every row but the
/// last), and trailing `)` (the last row only) stripped, leaving exactly
/// the same `-`/`--`-spelling-plus-optional-value fragment a
/// [`bracket_flag_row_content`] row already hands to [`parse_flag_spec`].
/// `None` when, once stripped, the remainder does not itself start with
/// `-` — refuses a row the caller mis-tracked into the group rather than
/// fabricate a flag from content that fails this check; `parse_body`'s own
/// depth bookkeeping should never actually produce that input, but the
/// check costs nothing to keep honest.
///
/// `|` inside a member (`-l|--logicalvolume`, the alias separator; `y|n` or
/// `contiguous|cling|...`, a value's own choice list) is untouched here —
/// handed to [`parse_flag_spec`] unchanged, exactly as
/// `bracket_flag_row_content`'s own doc comment explains for the identical
/// ambiguity: [`take_rest_value_token`]'s `alias_follows` guard (built for
/// `sg_sanitize`'s `--count=OC|-c OC`) already reads a value's own `|` as
/// part of the value, never as a second alias.
pub fn paren_alternation_member_content(input: &str) -> Option<&str> {
    let mut s = input.trim();
    if let Some(rest) = s.strip_prefix('(') {
        s = rest.trim_start();
    }
    if let Some(rest) = s.strip_suffix(')') {
        s = rest.trim_end();
    }
    if let Some(rest) = s.strip_suffix(',') {
        s = rest.trim_end();
    }
    if s.starts_with('-') {
        Some(s)
    } else {
        None
    }
}

// --- the stanza head's own mode-selecting flag --------------------------
//
// LVM's emitter documents each mode as a stanza: a prose description line,
// then a synopsis head line that repeats the tool's own name followed by
// that mode's mode-selecting flag, then the mode's `[...]`/`(...)` rows:
//
// ```text
//   Activate or deactivate LVs.
//   vgchange -a|--activate y|n|ay
//     [ -K|--ignoreactivationskip ]
//     ...
// ```
//
// `sections::parse_body`'s generic heading scanner already recognizes a
// line like `vgchange -a|--activate y|n|ay` as a *heading* whenever
// more-indented content follows it (`heading_can_name_a_group`), and keeps
// its full text as the block's `group` — but the heading is only ever
// copied into `group`, never itself parsed for the flag it names, so
// `--activate` never became a flag row. This module adds only the
// predicate for recognizing the shape; `sections::recover_stanza_head_flag`
// hands the recognized remainder to [`parse_flag_spec`] unchanged, exactly
// as every other row shape in this file already does — `-a|--activate
// y|n|ay` reads as one flag (short `a`, long `activate`, value `y|n|ay`)
// for the same reason `bracket_flag_row_content`'s identical shape does:
// [`take_rest_value_token`]'s `alias_follows` guard already keeps a
// value's own `|` from being misread as a second alias.

/// True if `rest` — the text immediately following a stanza head line's own
/// tool-name prefix, already trimmed of leading whitespace — opens with a
/// bare flag spelling and names no *second* flag anywhere in what follows.
///
/// Two conditions:
///
/// 1. The first word is a bare flag spelling ([`is_bare_flag_token`]). A
///    bare invocation naming no flag at all (`vgchange` alone, `rest`
///    empty) fails it, and so does an ordinary heading whose text merely
///    happens to start with the tool's own name (`rest` starts with a
///    word, not a dash).
/// 2. **No later word in `rest` is itself a bare flag spelling**, whether
///    bracketed or bare. Every real LVM specimen this predicate was
///    measured against — `-a|--activate y|n|ay`, `--systemid String VG`,
///    `--locktype sanlock|dlm|none VG` — carries its value as a bare token
///    or a `|`-separated list and nothing else; LVM's own convention never
///    puts a second flag on the head line at all, docopt-bracketed or not.
///    Two real, otherwise indistinguishable specimens showed why this has
///    to be checked structurally rather than assumed:
///    - `blkid --help`'s labelled `Usage:` block writes `blkid -p
///      [--match-tag <tag>] [--offset <offset>] [--size <size>] [--output
///      <format>] <dev> ...` — `blkid`'s own name, a bare `-p`, then a
///      *second* flag, `--match-tag`, sitting inside a bracket group.
///      Without this clause, [`parse_flag_spec`]'s value grammar read that
///      bracket group as `-p`'s own optional value, fabricating `-p
///      [--match-tag <tag>]` while `--match-tag` itself was silently
///      dropped.
///    - `jar --help`'s own `Examples:` block (an illustrative, filled-in
///      invocation, not a synopsis at all) writes `jar --update --file
///      foo.jar --main-class com.foo.Main --module-version 1.0` — four
///      real flags chained with no brackets anywhere. Without this clause,
///      [`parse_flag_spec`]'s alias loop tried `--file` as a *second
///      spelling* of the same flag (discarding the name, since `--update`
///      already won, but still advancing past it), then read `--file`'s
///      own value `foo.jar` as `--update`'s.
///
///    Refusing the whole line in both cases is the same choice
///    [`bracket_flag_row_content`]'s own `|`-reappearance guard already
///    makes for the identical hazard shape: missing beats invented. This
///    does not cost real recall — a line shaped either way is an ordinary
///    alternate invocation form or a worked example, both already handled
///    (or correctly left alone) by the existing labelled-usage-block engine
///    (`extract_usage_flags`) and the `is_ignorable_heading`/
///    `in_ignorable_section` machinery respectively, never a stanza head
///    this reader is the only path to.
///
/// Beyond that, it does not attempt to decide where the flag's own value
/// ends and a trailing positional begins — `--systemid String VG`'s `VG`
/// and `--locktype sanlock|dlm|none VG`'s `VG` are left in `rest` for
/// [`parse_flag_spec`] to leave unconsumed on its own (its value grammar
/// already stops at the first whitespace gap that isn't a qualifying alias
/// separator), rather than trimmed here by a second, narrower
/// value-vs-positional guess.
pub fn looks_like_stanza_head_flag(rest: &str) -> bool {
    let mut words = rest.split_whitespace();
    let Some(first) = words.next() else {
        return false;
    };
    if !is_bare_flag_token(first) {
        return false;
    }
    !words.any(|w| is_bare_flag_token(w.trim_start_matches(['[', '(', '{'])))
}

// --- the flag-alternation group ----------------------------------------
//
// A *delimited alternation of flag spellings* is one notation with three
// renderings in the seed-2 audit, all three of which lost real flags:
//
// | tool | as written | was parsed as |
// |---|---|---|
// | `cache_restore` | `{-i\|--input} <input xml file>` | nothing at all — the row never started a flag entry |
// | `eqn` | `{-v \| --version}` | `--version` carrying the literal value `"}"`, `-v` gone |
// | `xfs_io` | `[[-c\|-C] cmd]...` | nothing at all — the group is not a token starting with `-` |
//
// One rule serves all three, and serves `xtask`'s `brace-alternation-flag`
// detector too (which imports this function rather than restating it —
// `help_text/mod.rs`'s re-export block records what a second copy of a
// shared predicate has already cost this project once).
//
// **The member rule is the whole safety story.** Every alternative must be
// a *bare* flag spelling — `-c` or `--input`, nothing else — so a value
// alternation (`--color={always|never|auto}`, `[{start|stop}]`) can never be
// read as flags: its members are not flag-shaped. A member carrying its own
// value (`[--count=OC|-c OC]`) is refused too, deliberately: that shape is
// the `value-name-mangled` family, it is genuinely ambiguous about which
// value belongs to which alternative, and guessing would trade a known miss
// for a possible fabrication.
//
// **The member-count threshold belongs to the caller, not here.** The three
// callers want three different floors and each one's reason is local:
// `looks_like_flag_start` accepts a single braced spelling (`cache_restore`'s
// `{--omit-clean-shutdown}` is a real row); the synopsis path requires two,
// because a one-member `[-v]` is an ordinary optional flag its existing
// bracket-group path already handles correctly; the detector requires two,
// because "an alternation of one" is not the shape it is counting.

/// A delimited alternation of bare flag spellings, plus whatever followed
/// the closing delimiter — see [`parse_flag_alternation`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlagAlternation {
    /// The opening delimiter actually used, `'{'` or `'['`. Callers that
    /// only trust one of the two filter on this rather than on a re-parse.
    pub open: char,
    /// The delimited span exactly as the tool wrote it, delimiters included
    /// and nothing after them: `"{-v | --version}"`.
    ///
    /// Carried rather than left for the caller to reconstruct from `members`
    /// and `rest`. Subtracting the *trimmed* `rest`'s length from the input's
    /// gets the span wrong whenever the text after the group has leading or
    /// trailing whitespace — a report that quotes the tool's own text has to
    /// quote it, not approximate it.
    pub group: String,
    /// Each alternative's bare spelling in source order, delimiters and
    /// surrounding whitespace stripped: `["-i", "--input"]`.
    pub members: Vec<String>,
    /// The text after the closing delimiter, trimmed — the operand the
    /// alternatives *share* when it is one (`xfs_io`'s `cmd`), empty when
    /// the group stands alone. Left verbatim rather than interpreted here;
    /// deciding whether it is a usable value spec is the caller's business.
    pub rest: String,
}

/// Read `input` as a delimited alternation of bare flag spellings —
/// `{-i|--input} <input xml file>`, `{-v | --version}`, `[-c|-C] cmd` —
/// anchored at `input`'s first non-whitespace character.
///
/// `None` unless **all** of these hold:
///
/// 1. The first non-whitespace character is `{` or `[`, and that delimiter
///    has a matching close (depth-counted over its own pair, so
///    `[[-c|-C] cmd]`'s outer bracket is not closed by the inner one).
/// 2. Splitting the content on `|` at the content's own nesting depth
///    yields at least one non-empty alternative.
/// 3. **Every** alternative is a bare flag spelling
///    ([`is_bare_flag_spelling`]) — this is the condition that keeps a value
///    alternation (`{always|never|auto}`) and a subcommand alternation
///    (`{start|stop}`) out entirely.
pub fn parse_flag_alternation(input: &str) -> Option<FlagAlternation> {
    let trimmed = input.trim_start();
    let mut chars = trimmed.chars();
    let open = chars.next()?;
    let close = match open {
        '{' => '}',
        '[' => ']',
        _ => return None,
    };
    let (content, rest) = split_at_matching_close(trimmed, open, close)?;
    let members: Vec<&str> = split_alternatives(content);
    if members.is_empty() || !members.iter().all(|m| is_bare_flag_spelling(m)) {
        return None;
    }
    let span = trimmed.len() - rest.len();
    Some(FlagAlternation {
        open,
        group: trimmed[..span].to_string(),
        members: members.into_iter().map(str::to_string).collect(),
        rest: rest.trim().to_string(),
    })
}

/// Split `input` (whose first character is `open`) into the content between
/// `open` and its matching `close`, and everything after that close.
///
/// Depth is counted over the `open`/`close` pair only, which is what makes
/// `[[-c|-C] cmd]` work: the inner `]` decrements to depth 1, not 0. Every
/// boundary comes from `char_indices`, never a raw byte offset
/// (`AGENTS.md`'s slicing rule).
fn split_at_matching_close(input: &str, open: char, close: char) -> Option<(&str, &str)> {
    let mut depth = 0i32;
    let mut content_start = None;
    for (byte_pos, c) in input.char_indices() {
        if c == open {
            depth += 1;
            if content_start.is_none() {
                content_start = Some(byte_pos + c.len_utf8());
            }
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                let start = content_start?;
                return Some((&input[start..byte_pos], &input[byte_pos + c.len_utf8()..]));
            }
        }
    }
    None
}

/// Split an alternation group's content on `|` at its own nesting depth 0,
/// dropping empty fragments. Depth is counted over both bracket pairs, so a
/// nested value spec on one alternative is never split through.
///
/// `pub(super)`: also reused by `sections::split_bnf_alternation_row` for
/// the iproute2-family flag-row shape, which needs the same depth-aware
/// split but over a row that is never itself wrapped in a `{`/`[` pair (the
/// wrapping delimiter is consumed earlier, by `split_shared_heading_row`,
/// on the line the row's own heading shared).
pub(super) fn split_alternatives(content: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (i, c) in content.char_indices() {
        match c {
            '[' | '{' => depth += 1,
            ']' | '}' => depth -= 1,
            '|' if depth == 0 => {
                out.push(content[start..i].trim());
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    out.push(content[start..].trim());
    out.retain(|s| !s.is_empty());
    out
}

/// True when `token` is a *bare* flag spelling and nothing else: `--name`
/// (ASCII letter first, then alphanumerics and `-`) or `-c` (exactly one
/// [`is_bundle_member_char`] character).
///
/// Everything a flag entry can additionally carry is refused here — a value
/// (`-c OC`, `--count=OC`), a bundle (`-abc`), a single-dash long option
/// (`-pass-exit-codes`), punctuation, whitespace. Narrow on purpose: this
/// predicate is the only thing standing between "an alternation of flags"
/// and "an alternation of anything at all", and the shapes it turns away
/// are every one of them a *different* defect family with its own ambiguity.
pub(super) fn is_bare_flag_spelling(token: &str) -> bool {
    if let Some(name) = token.strip_prefix("--") {
        let mut cs = name.chars();
        return cs.next().is_some_and(|c| c.is_ascii_alphabetic())
            && cs.all(|c| c.is_ascii_alphanumeric() || c == '-');
    }
    if let Some(name) = token.strip_prefix('-') {
        let mut cs = name.chars();
        return cs.next().is_some_and(is_bundle_member_char) && cs.next().is_none();
    }
    false
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
        assert_eq!(spec.short(), Some('i'));
        assert_eq!(spec.long(), Some("interactive"));
        assert!(spec.fully_consumed);
    }

    #[test]
    fn parses_long_only() {
        let spec = parse_flag_spec("--autosquash");
        assert_eq!(spec.short(), None);
        assert_eq!(spec.long(), Some("autosquash"));
        assert!(spec.fully_consumed);
    }

    #[test]
    fn parses_required_value_with_equals() {
        let spec = parse_flag_spec("--format=FORMAT");
        assert_eq!(spec.long(), Some("format"));
        assert_eq!(spec.value_name.as_deref(), Some("FORMAT"));
        assert_eq!(spec.value_kind, ValueKind::Required);
    }

    #[test]
    fn parses_required_value_with_space_and_angle_brackets() {
        let spec = parse_flag_spec("-o, --output <FILE>");
        assert_eq!(spec.short(), Some('o'));
        assert_eq!(spec.long(), Some("output"));
        assert_eq!(spec.value_name.as_deref(), Some("<FILE>"));
        assert_eq!(spec.value_kind, ValueKind::Required);
    }

    #[test]
    fn parses_optional_bracketed_value() {
        let spec = parse_flag_spec("--occurrence[=NUMBER]");
        assert_eq!(spec.long(), Some("occurrence"));
        assert_eq!(spec.value_name.as_deref(), Some("NUMBER"));
        assert_eq!(spec.value_kind, ValueKind::Optional);
    }

    #[test]
    fn parses_multiple_long_aliases_keeping_first() {
        let spec = parse_flag_spec("-A, --catenate, --concatenate");
        assert_eq!(spec.short(), Some('A'));
        assert_eq!(spec.long(), Some("catenate"));
        assert!(spec.fully_consumed);
    }

    #[test]
    fn parses_gpg_sign_style_optional_short_value() {
        let spec = parse_flag_spec("-S, --gpg-sign[=<keyid>]");
        assert_eq!(spec.short(), Some('S'));
        assert_eq!(spec.long(), Some("gpg-sign"));
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
        assert_eq!(spec.short(), Some('S'));
        assert_eq!(spec.long(), Some("staged"));
        assert!(spec.negatable());
        assert!(spec.fully_consumed);
    }

    #[test]
    fn parses_negatable_long_only() {
        let spec = parse_flag_spec("--[no-]ignore-unmerged");
        assert_eq!(spec.short(), None);
        assert_eq!(spec.long(), Some("ignore-unmerged"));
        assert!(spec.negatable());
    }

    /// `--[no-]source <tree-ish>`: the negatable prefix and a required
    /// value spec must compose, since git uses both together.
    #[test]
    fn parses_negatable_long_with_value_spec() {
        let spec = parse_flag_spec("-s, --[no-]source <tree-ish>");
        assert_eq!(spec.short(), Some('s'));
        assert_eq!(spec.long(), Some("source"));
        assert!(spec.negatable());
        assert_eq!(spec.value_name.as_deref(), Some("<tree-ish>"));
        assert_eq!(spec.value_kind, ValueKind::Required);
    }

    /// Control case: a flag with no `[no-]` prefix must come back with
    /// `negatable: false`, unaffected.
    #[test]
    fn non_negatable_flag_is_unaffected() {
        let spec = parse_flag_spec("-2, --ours");
        assert_eq!(spec.long(), Some("ours"));
        assert!(!spec.negatable());
    }

    #[test]
    fn messy_remainder_is_not_fully_consumed_but_still_yields_flags() {
        // A value spec winnow's simple grammar doesn't fully understand
        // (nested brackets) should still recover the flag identity.
        let spec = parse_flag_spec("--sparse-version=MAJOR[.MINOR]");
        assert_eq!(spec.long(), Some("sparse-version"));
        assert!(spec.value_name.is_some());
    }

    // --- the alias list a value spec used to terminate -------------------

    /// The family's four `argparse` members, byte-exact from their own
    /// captures. The placeholder is repeated after the long form, and
    /// before the fix everything after the short flag became one value
    /// token (`PID,` — separator and all) while the long form was lost.
    #[test]
    fn an_argparse_row_keeps_both_spellings_and_one_clean_value() {
        for (fragment, short, long, value) in [
            ("-p PID, --pid PID", 'p', "pid", "PID"),
            (
                "-d DURATION, --duration DURATION",
                'd',
                "duration",
                "DURATION",
            ),
            ("-M METHOD, --method METHOD", 'M', "method", "METHOD"),
            (
                "-C TOP_COUNT, --top-count TOP_COUNT",
                'C',
                "top-count",
                "TOP_COUNT",
            ),
            // hand-written shell, lowercase placeholders — no framework
            ("-t seconds, --timeout seconds", 't', "timeout", "seconds"),
        ] {
            let spec = parse_flag_spec(fragment);
            assert_eq!(spec.short(), Some(short), "{fragment}");
            assert_eq!(spec.long(), Some(long), "{fragment}");
            assert_eq!(spec.value_name.as_deref(), Some(value), "{fragment}");
            assert_eq!(spec.value_kind, ValueKind::Required, "{fragment}");
            assert!(spec.fully_consumed, "{fragment}");
        }
    }

    /// `sg_sanitize`'s real rows: the *long* form first, `|` as the
    /// separator, `=` on one side and a space on the other. There is no
    /// whitespace anywhere in `OC|-c`, so before the fix the entire thing
    /// became the placeholder and `-c` was lost inside it.
    #[test]
    fn a_pipe_separated_row_recovers_the_short_form_from_inside_the_value() {
        for (fragment, short, long, value) in [
            ("--count=OC|-c OC", 'c', "count", "OC"),
            ("--ipl=LEN|-i LEN", 'i', "ipl", "LEN"),
            ("--pattern=PF|-p PF", 'p', "pattern", "PF"),
            ("--timeout=SECS|-t SECS", 't', "timeout", "SECS"),
        ] {
            let spec = parse_flag_spec(fragment);
            assert_eq!(spec.short(), Some(short), "{fragment}");
            assert_eq!(spec.long(), Some(long), "{fragment}");
            assert_eq!(spec.value_name.as_deref(), Some(value), "{fragment}");
        }
    }

    /// `javaflow-bpfcc`'s real choice list: six commas inside one
    /// placeholder. Only [`alias_follows`] keeps the value from being cut
    /// at the first of them.
    #[test]
    fn commas_inside_a_choice_list_never_end_the_value() {
        let spec = parse_flag_spec(
            "-l {java,perl,php,python,ruby,tcl}, --language {java,perl,php,python,ruby,tcl}",
        );
        assert_eq!(spec.short(), Some('l'));
        assert_eq!(spec.long(), Some("language"));
        assert_eq!(
            spec.value_name.as_deref(),
            Some("{java,perl,php,python,ruby,tcl}")
        );
    }

    /// The false-positive side, which is the side that matters: merging two
    /// genuinely different flags invents an alias the tool does not have.
    /// Every one of these carries a separator and must still yield one
    /// placeholder and no second spelling.
    #[test]
    fn a_separator_inside_a_value_never_becomes_an_alias() {
        for (fragment, value) in [
            // jdeprscan's real --release: ten pipes, all followed by digits
            (
                "--release 7|8|9|10|11|12|13|14|15|16|17",
                "7|8|9|10|11|12|13|14|15|16|17",
            ),
            ("--format json|yaml|table", "json|yaml|table"),
            ("--color always|never|auto", "always|never|auto"),
            // a choice list whose member starts with a dash
            ("--sign {a,-b}", "{a,-b}"),
            ("--sign {-1,0,1}", "{-1,0,1}"),
        ] {
            let spec = parse_flag_spec(fragment);
            assert_eq!(spec.value_name.as_deref(), Some(value), "{fragment}");
            assert_eq!(spec.short(), None, "{fragment} must not gain a short");
        }
    }

    /// Whitespace alone never resumes an alias run — only an explicit
    /// `,`/`|` does. Without this, two unrelated flags written next to each
    /// other in a synopsis would merge into one.
    #[test]
    fn whitespace_alone_never_resumes_an_alias_run() {
        let spec = parse_flag_spec("--output FILE --other");
        assert_eq!(spec.long(), Some("output"));
        assert_eq!(spec.value_name.as_deref(), Some("FILE"));
        assert!(
            !spec.fully_consumed,
            "the trailing --other is unconsumed, not an alias"
        );
    }

    /// `lsof`'s real plus-or-minus convention, byte-exact: `+|-e s` means
    /// `+e` or `-e`, a third shape this grammar does not model. Its `|` has
    /// `+` on its left, which is not a finished placeholder, so
    /// [`separator_has_a_left_operand`] leaves the token alone rather than
    /// recovering `-e` with a literal `+` as its value.
    #[test]
    fn lsofs_plus_or_minus_token_is_left_alone() {
        assert!(!separator_has_a_left_operand("+"));
        let spec = parse_flag_spec("+|-e s");
        assert_eq!(spec.short(), None);
        assert_eq!(spec.long(), None);
    }

    /// The alias run still stops where it always did when nothing follows
    /// the separator but prose or another line.
    #[test]
    fn a_separator_with_no_spelling_after_it_ends_the_run() {
        for fragment in ["--format FMT,", "--format FMT|", "-o, --output FILE"] {
            let spec = parse_flag_spec(fragment);
            assert!(
                spec.value_name.is_some() || spec.long().is_some(),
                "{fragment}"
            );
        }
        // `-o, --output FILE` is the shape that always worked; it must be
        // untouched by any of this.
        let spec = parse_flag_spec("-o, --output FILE");
        assert_eq!(spec.short(), Some('o'));
        assert_eq!(spec.long(), Some("output"));
        assert_eq!(spec.value_name.as_deref(), Some("FILE"));
        assert!(spec.fully_consumed);
    }

    /// `tar`'s real multi-alias row with a value on each: the first long
    /// name still wins and the placeholder no longer carries the separator.
    #[test]
    fn tars_repeated_long_alias_row_keeps_one_clean_value() {
        let spec = parse_flag_spec("-F, --info-script=NAME, --new-volume-script=NAME");
        assert_eq!(spec.short(), Some('F'));
        assert_eq!(spec.long(), Some("info-script"));
        assert_eq!(spec.value_name.as_deref(), Some("NAME"));
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

    // --- the docopt bracket-group flag row ------------------------------

    #[test]
    fn bracket_flag_row_reads_lvms_common_options() {
        assert_eq!(
            bracket_flag_row_content("[ -d|--debug ]"),
            Some("-d|--debug")
        );
        assert_eq!(
            bracket_flag_row_content("[    --commandprofile String ]"),
            Some("--commandprofile String")
        );
        assert_eq!(
            bracket_flag_row_content("[ -A|--autobackup y|n ]"),
            Some("-A|--autobackup y|n")
        );
        assert_eq!(
            bracket_flag_row_content("[ --metadatasize Size[m|UNIT] ]"),
            Some("--metadatasize Size[m|UNIT]")
        );
        assert_eq!(
            bracket_flag_row_content("[ -f|--force ]"),
            Some("-f|--force")
        );
        assert_eq!(
            bracket_flag_row_content("[ --nolocking ]"),
            Some("--nolocking")
        );
    }

    #[test]
    fn bracket_flag_row_refuses_operand_rows() {
        // No dash: a cross-reference to the common-options block, never a
        // flag.
        assert_eq!(bracket_flag_row_content("[ COMMON_OPTIONS ]"), None);
        // Positionals, in the identical bracket notation.
        assert_eq!(bracket_flag_row_content("[ VG|Tag ... ]"), None);
        assert_eq!(bracket_flag_row_content("[ VG PV ... ]"), None);
    }

    #[test]
    fn bracket_flag_row_refuses_trailing_text() {
        // Not this row's shape: a description trails the group.
        assert_eq!(
            bracket_flag_row_content("[ -d|--debug ]  enable debugging"),
            None
        );
    }

    /// The content this predicate returns is handed straight to
    /// `parse_flag_spec` by every caller — confirm that pipeline actually
    /// reads the alias-vs-value ambiguity correctly, not just that the
    /// brackets are stripped.
    #[test]
    fn bracket_flag_row_content_feeds_parse_flag_spec_correctly() {
        let spec = parse_flag_spec(bracket_flag_row_content("[ -A|--autobackup y|n ]").unwrap());
        assert_eq!(spec.short(), Some('A'));
        assert_eq!(spec.long(), Some("autobackup"));
        assert_eq!(spec.value_name.as_deref(), Some("y|n"));

        let spec =
            parse_flag_spec(bracket_flag_row_content("[ --metadatasize Size[m|UNIT] ]").unwrap());
        assert_eq!(spec.long(), Some("metadatasize"));
        assert_eq!(spec.value_name.as_deref(), Some("Size[m|UNIT]"));

        let spec = parse_flag_spec(bracket_flag_row_content("[ -d|--debug ]").unwrap());
        assert_eq!(spec.short(), Some('d'));
        assert_eq!(spec.long(), Some("debug"));
        assert_eq!(spec.value_name, None);
    }

    #[test]
    fn looks_like_flag_start_still_refuses_brackets() {
        // The trap this whole row-level predicate exists to avoid:
        // `looks_like_flag_start` must stay blind to `[`, or `lsof`'s
        // usage-block continuation (`[-F [f]]`) would end that block one
        // line in.
        assert!(!looks_like_flag_start("[ -d|--debug ]"));
        assert!(!looks_like_flag_start("[-F [f]]"));
    }

    // --- the bundled-short-flag cluster ---------------------------------

    /// Helper: the members `parse_bundled_shorts` recovers, as a `String`,
    /// or `"-"` when it declined — so a test can assert both answers with
    /// one comparison and a failure message shows what was recovered.
    fn bundle(token: &str) -> String {
        match parse_bundled_shorts(token) {
            Some(members) => members.into_iter().collect(),
            None => "-".to_string(),
        }
    }

    /// The five real clusters from the seed-2 human audit, byte-exact from
    /// their own captures. Each is a *whole* set of switches, so the
    /// recovered members must be the token minus its dash, exactly.
    #[test]
    fn the_five_audited_clusters_split_into_their_members() {
        assert_eq!(
            bundle("-AbdDefhHIJKlLnNOpqStuUvxX#"),
            "AbdDefhHIJKlLnNOpqStuUvxX#" // tcpdump, 26 members
        );
        assert_eq!(bundle("-2CDlNuVv"), "2CDlNuVv"); // tmux
        assert_eq!(bundle("-adfinrRstVx"), "adfinrRstVx"); // xfs_io
        assert_eq!(bundle("-BeEksvxX"), "BeEksvxX"); // filefrag
        assert_eq!(
            bundle("-abcCeEgGijklNpRsStUVXzZ"),
            "abcCeEgGijklNpRsStUVXzZ"
        ); // groff
    }

    /// The two signals in condition 5 each carry a large real family the
    /// other cannot see, so both are exercised on fleet text.
    #[test]
    fn either_ordering_or_a_case_mixing_swallowed_half_is_enough() {
        // Ordered only: `od`'s traditional switches are all lowercase, so
        // nothing about their case is evidence of anything.
        assert!(!swallowed_members_mix_case("bcdfilosx"));
        assert_eq!(bundle("-abcdfilosx"), "abcdfilosx");
        // Case-mixing only: `tree` sorts its lowercase run and then its
        // uppercase one — sorted twice, so not sorted.
        assert!(!cluster_is_ordered("acdfghilnpqrstuvxACDFJQNSUX"));
        assert_eq!(
            bundle("-acdfghilnpqrstuvxACDFJQNSUX"),
            "acdfghilnpqrstuvxACDFJQNSUX"
        );
    }

    /// Family 2 of the three sharing the `short && !long && value_name`
    /// fingerprint: a **single-dash long option**. Every one of these is a
    /// completely correct parse today and splitting one would destroy a
    /// working tool, which is strictly worse than leaving a bundle
    /// collapsed. They mix case *as clusters* (an uppercase flag letter
    /// with a lowercase word glued on) and are rejected because their
    /// swallowed halves — `script`, `name`, `utf8`, `directory` — do not.
    #[test]
    fn single_dash_long_options_are_never_split() {
        for token in [
            "-Zscript",           // cargo
            "-Dname",             // rpcgen
            "-Tutf8",             // makewhatis
            "-Olevel",            // find
            "-Idirectory",        // perl
            "-oOUTFILE",          // the uppercase-placeholder shape
            "-pass-exit-codes",   // gcc, hyphenated
            "-fdump-scos",        // gcc, hyphenated
            "-b{blocksize}[KMG]", // filefrag's own braced value
        ] {
            assert_eq!(bundle(token), "-", "{token} must not be split");
        }
    }

    /// Family 3: a flag **repeated** to mean "more of it". `-vv` is below
    /// the member floor; `-vvv` and `strace`'s real `[-DDD]` are rejected
    /// by distinctness instead. Both halves are asserted because they fail
    /// *different* conditions, so a change to either one could silently
    /// admit the whole family.
    #[test]
    fn repeated_character_flags_are_never_split() {
        for token in ["-vv", "-dd", "-qq"] {
            assert_eq!(bundle(token), "-", "{token} must not be split");
            assert!(token.strip_prefix('-').unwrap().chars().count() < MIN_CLUSTER_MEMBERS);
        }
        for token in ["-vvv", "-DDD", "-ffff"] {
            assert_eq!(bundle(token), "-", "{token} must not be split");
            assert!(!members_are_distinct(token.strip_prefix('-').unwrap()));
        }
    }

    /// The deliberate lost recall at [`MIN_CLUSTER_MEMBERS`]: `ssh-keygen`'s
    /// `[-hU]` is a genuine collapse a human labelled `wrong`, and it is
    /// left alone because nothing about its *shape* separates it from
    /// `rpcgen`'s real `-Ss` or `xxd`'s real `-ps`. Asserted, not merely
    /// described, so lowering the floor has to come with a decision about
    /// `lessecho` rather than happening by accident.
    #[test]
    fn a_two_character_cluster_is_deliberately_left_alone() {
        assert_eq!(bundle("-hU"), "-"); // ssh-keygen, a real collapse
        for token in [
            "-Ss", "-it", "-st", "-ou", "-ac", "-as", "-ps", "-ox", "-pn",
        ] {
            assert_eq!(bundle(token), "-", "{token} is a real flag with a value");
        }
    }

    /// The separator is the whole difference between `tmux`'s collapsed
    /// `[-2CDlNuVv]` and the five genuine valued flags on its own synopsis
    /// line, so it is asserted directly rather than left implicit.
    #[test]
    fn a_spaced_value_is_never_a_cluster_however_bundle_shaped_it_looks() {
        for token in [
            "-c shell-command",
            "-f file",
            "-L socket-name",
            "-T features",
        ] {
            assert_eq!(bundle(token), "-", "{token} must not be split");
        }
        // ...and a token that is glued and ordered *does* split, confirming
        // the space was doing the work above rather than some other
        // condition failing silently.
        assert_eq!(bundle("-cDeF"), "cDeF");
    }

    /// A numeric run orders vacuously — no letters to be out of order — so
    /// [`MIN_ORDERED_LETTERS`] is what keeps a glued numeric default from
    /// riding that vacuous truth into being split into digits.
    #[test]
    fn a_glued_numeric_default_is_never_split() {
        for token in ["-b1024", "-j4", "-n0777"] {
            assert_eq!(bundle(token), "-", "{token} must not be split");
        }
        assert!(!cluster_is_ordered("b1024"));
    }

    /// Long options and the bare option terminator are not clusters, and
    /// the `--` case matters: `-` -> `-name` would otherwise look like a
    /// perfectly ordinary member run.
    #[test]
    fn a_long_option_is_never_a_cluster() {
        for token in ["--verbose", "--no-pager", "--", "-", "abc", ""] {
            assert_eq!(bundle(token), "-", "{token:?} must not be split");
        }
    }

    /// `parse_flag_spec` itself is deliberately *unchanged* — it still
    /// reads a cluster as one valued flag, because the identical shape from
    /// an option-table row is the GCC single-dash convention and is
    /// genuinely one flag. The split lives at the synopsis call site.
    #[test]
    fn parse_flag_spec_still_reads_a_cluster_as_one_valued_flag() {
        let spec = parse_flag_spec("-2CDlNuVv");
        assert_eq!(spec.short(), Some('2'));
        assert_eq!(spec.value_name.as_deref(), Some("CDlNuVv"));
        assert_eq!(spec.value_kind, ValueKind::Required);
    }

    /// `ip --help`'s own abbreviation convention: `-V[ersion]` names the
    /// flag with the full word the bracket spells out (`"Version"`,
    /// `abbrev: Some(1)`), not a short flag `-V` with an invented value —
    /// the shape is single-dash-long, exactly the rule that also makes
    /// `-rc[vbuf]` (a two-letter prefix) long-like and dissolves issue
    /// #49's duplicate `-r` row. Before this abbreviation model existed,
    /// `-V[ersion]` parsed as `-V` taking an optional value literally
    /// named `"ersion"` — a value `ip` does not document at all, on a flag
    /// that takes none.
    #[test]
    fn short_flag_abbreviation_bracket_is_not_an_invented_value() {
        for (input, prefix_len, full_name) in [
            ("-V[ersion]", 1, "Version"),
            ("-s[tatistics]", 1, "statistics"),
            ("-d[etails]", 1, "details"),
            ("-f[amily]", 1, "family"),
            ("-h[uman-readable]", 1, "human-readable"),
            ("-l[oops]", 1, "loops"),
            ("-a[ll]", 1, "all"),
            ("-c[olor]", 1, "color"),
        ] {
            let spec = parse_flag_spec(input);
            assert_eq!(spec.short(), None, "input: {input} is long-like, not short");
            assert_eq!(spec.long(), Some(full_name), "input: {input}");
            assert_eq!(spec.spellings.len(), 1, "input: {input}");
            assert_eq!(spec.spellings[0].abbrev, Some(prefix_len), "input: {input}");
            assert_eq!(spec.spellings[0].render(), input, "input: {input}");
            assert_eq!(spec.value_name, None, "input: {input} must carry no value");
            assert!(spec.fully_consumed, "input: {input}");
        }
    }

    /// `ip --help`'s own `OPTIONS := { -V[ersion] | ... | -c[olor]}` cuts
    /// its last row with the alternation group's own closing `}` still
    /// glued on. The stray brace is leftover punctuation from the
    /// enclosing group, never a value — `-c[olor]` must come out exactly
    /// as clean as `-a[ll]` does one line earlier in the same document.
    #[test]
    fn a_stray_closing_brace_after_an_abbreviation_bracket_is_not_a_value() {
        let spec = parse_flag_spec("-c[olor]}");
        assert_eq!(spec.long(), Some("color"));
        assert_eq!(spec.spellings[0].abbrev, Some(1));
        assert_eq!(
            spec.value_name, None,
            "the stray `}}` must not become a value"
        );
        assert!(spec.fully_consumed);
    }

    /// `ip --help`'s real issue #49 defect: `-rc[vbuf] [size]`, a
    /// **two**-letter abbreviation prefix. The one-letter-only model
    /// (`try_short` reading exactly one character before ever looking for
    /// a bracket) could not recognize this shape at all — `-rc[vbuf]`
    /// failed to fully consume, which refused the whole nine-alternative
    /// BNF row it sat in and fell back to the single-column read that
    /// produced ip's mangled second `-r` (a `short: 'r'` carrying
    /// `value_name: "c[vbuf]"`). With a multi-letter prefix recognized,
    /// `-rc[vbuf]` reads as its own clean flag, `Long("rcvbuf")` — a
    /// different key from `-r[esolve]`'s `Long("resolve")`, which is what
    /// dissolves the duplicate without any dedup rule.
    #[test]
    fn a_two_letter_abbreviation_prefix_is_recognized() {
        let spec = parse_flag_spec("-rc[vbuf] [size]");
        assert_eq!(spec.short(), None, "long-like, not short");
        assert_eq!(spec.long(), Some("rcvbuf"));
        assert_eq!(spec.spellings.len(), 1);
        assert_eq!(spec.spellings[0].abbrev, Some(2));
        assert_eq!(spec.spellings[0].render(), "-rc[vbuf]");
        assert_eq!(spec.value_name.as_deref(), Some("size"));
        assert_eq!(spec.value_kind, ValueKind::Optional);
        assert!(spec.fully_consumed);
    }

    /// The alias loop keeps every recognized spelling, not just the first
    /// short and first long — the fix that dissolves issue #30's
    /// multi-spelling bug at its source. `jdeprscan --help` writes exactly
    /// this row (`-?, -h, --help`, one physical line, all three aliases
    /// comma-separated) — before this fix the loop kept only the first
    /// short (`-?`) and the long (`--help`), silently dropping `-h`.
    #[test]
    fn the_alias_loop_keeps_every_spelling_jdeprscan_style() {
        let spec = parse_flag_spec("-?, -h, --help");
        assert_eq!(spec.spellings.len(), 3, "{:?}", spec.spellings);
        assert_eq!(spec.spellings[0].render(), "-?");
        assert_eq!(spec.spellings[1].render(), "-h");
        assert_eq!(spec.spellings[2].render(), "--help");
        assert!(spec.fully_consumed);
    }

    /// `jdeprscan`'s real two-column table cell, `-? -h` — two short
    /// spellings of one flag, separated by nothing but a bare space, no
    /// comma. Bare whitespace must still be allowed to continue an alias
    /// run when nothing long-like has been read yet — this is the
    /// counter-example that keeps the pod2html-shaped gate below from
    /// over-tightening (`corpus/jdeprscan/audit-seed2`).
    #[test]
    fn bare_whitespace_still_continues_a_run_of_two_shorts() {
        let spec = parse_flag_spec("-? -h");
        assert_eq!(spec.spellings.len(), 2, "{:?}", spec.spellings);
        assert_eq!(spec.spellings[0].render(), "-?");
        assert_eq!(spec.spellings[1].render(), "-h");
    }

    /// `pod2html --help`'s real usage-synopsis row: four independently
    /// negatable long options, space-separated, no comma anywhere —
    /// `--quiet --noquiet --verbose --noverbose`. Once a long-like
    /// spelling has been read, a further spelling needs an *explicit*
    /// `,`/`|` before it; bare whitespace no longer resumes the run, and
    /// the leftover text is honestly unconsumed rather than glued on as a
    /// fabricated value. Before this gate, every alias found here was
    /// *kept* (the alias-loop fix this test module also pins), so the
    /// four distinct flags read as one entity's ever-growing alias list —
    /// worse than the pre-existing defect of silently dropping the extra
    /// three.
    #[test]
    fn a_run_of_distinct_long_options_never_merges_on_bare_whitespace() {
        let spec = parse_flag_spec("--quiet --noquiet --verbose --noverbose");
        assert_eq!(spec.spellings.len(), 1, "{:?}", spec.spellings);
        assert_eq!(spec.spellings[0].render(), "--quiet");
        assert_eq!(
            spec.value_name, None,
            "the next flag's own name must never become this flag's value"
        );
        assert!(!spec.fully_consumed);
    }

    /// `iptables --help`'s real `--replace -R chain rulenum` row: one flag,
    /// long spelling first, short spelling second, separated by nothing
    /// but a bare space. The long-then-long gate above must not also
    /// catch long-then-*short* — it is keyed on both neighbors being
    /// long-like, not merely on "a long-like spelling was read", or this
    /// legitimate pair (and its five siblings in the same real document:
    /// `--list-rules -S`, `--set-counters -c`, `--ipv4 -4`, `--ipv6 -6`)
    /// would wrongly split in two.
    #[test]
    fn a_long_then_short_pair_still_runs_together_on_bare_whitespace() {
        let spec = parse_flag_spec("--replace -R chain rulenum");
        assert_eq!(spec.spellings.len(), 2, "{:?}", spec.spellings);
        assert_eq!(spec.spellings[0].render(), "--replace");
        assert_eq!(spec.spellings[1].render(), "-R");
    }

    /// `unzip --help`'s real usage-synopsis placeholder,
    /// `[-opts[modifiers]]` — "any of the single-letter options below,
    /// optionally followed by any of the modifier letters below", not a
    /// real flag named `-opts`. `"opts"` is a four-letter prefix, past
    /// [`MAX_ABBREV_PREFIX_LEN`], so this reads as the ordinary one-letter
    /// short flag `-o` (with the rest left for the grammar to interpret as
    /// a value, exactly as it did before the abbreviation model existed)
    /// rather than fabricating a `-opts[modifiers]` entity that answers to
    /// no real flag `unzip` documents.
    #[test]
    fn an_over_long_bracket_prefix_is_not_read_as_an_abbreviation() {
        let spec = parse_flag_spec("-opts[modifiers]");
        assert_eq!(spec.spellings.len(), 1, "{:?}", spec.spellings);
        assert_eq!(spec.spellings[0].render(), "-o");
        assert_eq!(spec.spellings[0].abbrev, None);
    }

    /// The discriminator is narrow on purpose: a real optional-value
    /// placeholder (upper/mixed case, or carrying its own `=`) must still
    /// parse as a value exactly as before — this change must never widen
    /// into swallowing a genuine optional argument.
    #[test]
    fn short_flag_real_optional_value_is_unaffected_by_abbrev_stripping() {
        let spec = parse_flag_spec("-o[FILE]");
        assert_eq!(spec.short(), Some('o'));
        assert_eq!(spec.value_name.as_deref(), Some("FILE"));
        assert_eq!(spec.value_kind, ValueKind::Optional);

        let spec = parse_flag_spec("-x[=WHEN]");
        assert_eq!(spec.short(), Some('x'));
        assert_eq!(spec.value_name.as_deref(), Some("WHEN"));
        assert_eq!(spec.value_kind, ValueKind::Optional);
    }

    // --- the parenthesized alternation group ----------------------------

    /// The positive case: a bare `(` immediately followed by a flag token,
    /// left unclosed on the line — `vgchange`'s own
    /// `( -l|--logicalvolume Number,`.
    #[test]
    fn paren_alternation_open_fires_on_an_unclosed_leading_paren_flag_group() {
        assert!(looks_like_paren_alternation_open(
            "( -l|--logicalvolume Number,"
        ));
        assert!(looks_like_paren_alternation_open("(    --addtag Tag,"));
    }

    /// A row using `|` as a plain alias separator, with no paren group at
    /// all, must never be claimed by this predicate — it has no leading
    /// `(`, so it is not this shape regardless of how many aliases or
    /// values it carries.
    #[test]
    fn paren_alternation_open_is_false_with_no_leading_paren() {
        assert!(!looks_like_paren_alternation_open(
            "-l|--logicalvolume Number,"
        ));
        assert!(!looks_like_paren_alternation_open(
            "[ -A|--autobackup y|n ]"
        ));
    }

    /// A same-line, already-balanced parenthetical is not a multi-line
    /// group opening, even when the first word after `(` happens to look
    /// flag-shaped — the defining evidence is that the group is left
    /// *unclosed*, not merely that `(` is followed by a dash.
    #[test]
    fn paren_alternation_open_refuses_a_balanced_same_line_parenthetical() {
        assert!(!looks_like_paren_alternation_open("(-x see docs)"));
        assert!(!looks_like_paren_alternation_open(
            "(-h) print this help information"
        ));
    }

    /// The three physical-line shapes a member row takes, stripped down to
    /// exactly what `bracket_flag_row_content` already hands
    /// `parse_flag_spec` for the bracket-row shape: opening (leading `(`,
    /// trailing `,`), middle (trailing `,` only), and closing (trailing
    /// `)` only, no comma).
    #[test]
    fn paren_alternation_member_content_strips_open_close_and_comma() {
        assert_eq!(
            paren_alternation_member_content("( -l|--logicalvolume Number,"),
            Some("-l|--logicalvolume Number")
        );
        assert_eq!(
            paren_alternation_member_content("-u|--uuid,"),
            Some("-u|--uuid")
        );
        assert_eq!(
            paren_alternation_member_content("--setautoactivation y|n )"),
            Some("--setautoactivation y|n")
        );
    }

    /// `|` inside a member is untouched by the stripping — it is still an
    /// alias separator (`-l|--logicalvolume`) or a value's own choice list
    /// (`y|n`), exactly as `bracket_flag_row_content` leaves it for
    /// `parse_flag_spec` to resolve via `take_rest_value_token`'s
    /// `alias_follows` guard.
    #[test]
    fn paren_alternation_member_content_feeds_parse_flag_spec_correctly() {
        let content = paren_alternation_member_content("-x|--resizeable y|n,").unwrap();
        let spec = parse_flag_spec(content);
        assert_eq!(spec.short(), Some('x'));
        assert_eq!(spec.long(), Some("resizeable"));
        assert_eq!(spec.value_name.as_deref(), Some("y|n"));
    }

    /// A row that, once stripped, does not start with `-` is refused
    /// outright rather than fabricated into a flag — the defensive check
    /// `parse_body`'s own depth bookkeeping should never actually need.
    #[test]
    fn paren_alternation_member_content_refuses_a_non_flag_row() {
        assert_eq!(paren_alternation_member_content("( COMMON_OPTIONS,"), None);
        assert_eq!(paren_alternation_member_content("VG|Tag )"), None);
    }

    /// `looks_like_stanza_head_flag`'s whole test: a bare flag token as the
    /// remainder's first word, real for every LVM shape it exists for, and
    /// refused for a bare invocation with nothing after the name.
    #[test]
    fn looks_like_stanza_head_flag_requires_a_leading_flag_token() {
        assert!(looks_like_stanza_head_flag("-a|--activate y|n|ay"));
        assert!(looks_like_stanza_head_flag("--refresh"));
        assert!(looks_like_stanza_head_flag("--systemid String VG"));
        assert!(!looks_like_stanza_head_flag(""));
        assert!(!looks_like_stanza_head_flag("VG"));
        assert!(!looks_like_stanza_head_flag("is a general-purpose tool"));
    }

    /// A *second* flag anywhere in the remainder refuses the whole line,
    /// bracketed or bare — `blkid`'s `-p [--match-tag <tag>] ...` and
    /// `jar`'s `--update --file foo.jar --main-class ...` are both real
    /// tools this predicate must not fire on (see its own doc comment for
    /// why each one fabricated a wrong flag before this guard existed).
    #[test]
    fn looks_like_stanza_head_flag_refuses_a_second_flag_anywhere_in_rest() {
        assert!(!looks_like_stanza_head_flag(
            "-p [--match-tag <tag>] [--offset <offset>] <dev> ..."
        ));
        assert!(!looks_like_stanza_head_flag(
            "--update --file foo.jar --main-class com.foo.Main --module-version 1.0"
        ));
        // The bare, no-second-flag shape is unaffected.
        assert!(looks_like_stanza_head_flag(
            "--locktype sanlock|dlm|none VG"
        ));
    }

    /// jmod's own header-underline row (`corpus/jmod/17.0.20/help.txt`,
    /// under its two-column `Option`/`Description` heading): a run of
    /// dashes with no name character in it must never look like a flag
    /// row, or it fabricates a flag named `----` (the leading `--` read as
    /// the long-flag marker, the rest as its name).
    #[test]
    fn a_dash_underline_row_never_looks_like_a_flag_start() {
        assert!(!looks_like_flag_start(
            "------                              -----------"
        ));
        assert!(!looks_like_flag_start("---"));
        assert!(!looks_like_flag_start("----------"));
    }

    /// `--` alone is a real, meaningful token in many tools (GNU getopt's
    /// end-of-options marker) and must stay eligible to open a flag entry
    /// — the dash-underline guard's threshold is 3, not 2, specifically so
    /// this stays true.
    #[test]
    fn a_bare_double_dash_still_looks_like_a_flag_start() {
        assert!(looks_like_flag_start("--"));
        assert!(looks_like_flag_start("-- end of options"));
    }
}
