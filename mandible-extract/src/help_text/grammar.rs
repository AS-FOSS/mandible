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
    /// Every recognized spelling, in document order — not just the first
    /// of each shape. See docs/shapes.md S-083.
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
        // Two long-like spellings in a row need an explicit `,`/`|`
        // between them; a short may still run into a long (or vice versa)
        // on bare whitespace. See docs/shapes.md S-083.
        let mut last_was_long_like = false;
        // Whether an explicit `,`/`|` has appeared anywhere earlier in
        // *this* alias run — see the whitespace-continuation rules below.
        let mut saw_explicit_anywhere = false;
        loop {
            let before = rest;
            rest = skip_separators(rest);
            if rest.is_empty() {
                break;
            }
            // The bare word `or` joining two spellings (`-h  or  --help`)
            // is an explicit alias separator alongside `,`/`|`. Word-bounded
            // and gated on a real spelling following it (`alias_follows`),
            // so a value or a description merely spelled "or" is never
            // consumed. See docs/shapes.md S-099.
            let or_alias = strip_or_alias_separator(rest);
            let explicit = or_alias.is_some() || saw_explicit_separator(before, rest);
            if let Some(after_or) = or_alias {
                rest = after_or;
            }

            // Rule (i): a row's separator style is fixed by its first
            // separator. Once an explicit `,`/`|` has appeared in this
            // run, a later bare-whitespace spelling is not a continuation
            // — it's the next thing on the line (dpkg-split's `-a|--auto
            // -o <complete> <part>`). See docs/shapes.md S-083.
            if !explicit && saw_explicit_anywhere {
                break;
            }

            // In alias position, a multi-letter single-dash run with no
            // abbreviation bracket is its own spelling, not `try_short`'s
            // truncate-to-first-character fallback (gold's `-G, -shared`
            // would otherwise collide with `-s, --strip-all`). See
            // docs/shapes.md S-083.
            if explicit {
                if let Some((spelling, tail)) = try_alias_position_single_dash_long(rest) {
                    if already_collected(&spec, &spelling) {
                        break;
                    }
                    saw_explicit_anywhere = true;
                    last_was_long_like = true;
                    spec.spellings.push(spelling);
                    rest = tail;
                    continue;
                }
            }

            if let Some((spelling, tail)) = try_short(rest) {
                if last_was_long_like && is_long_like(&spelling) && !explicit {
                    break;
                }
                if already_collected(&spec, &spelling) {
                    break;
                }
                // Rules (ii)/(iii), scoped to "previous spelling was a
                // genuine short". See docs/shapes.md S-083.
                if !explicit && spec.spellings.last().is_some_and(is_genuine_short) {
                    // (ii) screen's `-D -RR`: continuation must also be
                    // one letter, not a longer run that truncates to one.
                    if short_run_char_count(rest) != Some(1) {
                        break;
                    }
                    // (iii) xxd's `-r -s off ...`: a trailing bare value
                    // means two flags in a worked example, not aliases.
                    if trailing_token_is_a_value(tail) {
                        break;
                    }
                }
                saw_explicit_anywhere |= explicit;
                last_was_long_like = is_long_like(&spelling);
                spec.spellings.push(spelling);
                rest = tail;
                continue;
            }
            if let Some((spelling, tail)) = try_long(rest) {
                if last_was_long_like && is_long_like(&spelling) && !explicit {
                    break;
                }
                if already_collected(&spec, &spelling) {
                    break;
                }
                saw_explicit_anywhere |= explicit;
                last_was_long_like = is_long_like(&spelling);
                spec.spellings.push(spelling);
                rest = tail;
                continue;
            }
            // Only at the very start of an alias run, or right after a
            // real separator was consumed — never glued straight onto a
            // longer token's own unconsumed tail with nothing between
            // them. Defense in depth alongside `try_bare_sigil`'s own
            // whitelist: the same class of bug (an unrelated token's
            // failed-parse leftover misread as a fresh alias) is exactly
            // what fabricated a bogus `+` alias on `as`'s real
            // `--gstabs+` in an earlier version of this function. See
            // docs/shapes.md S-096.
            let sigil_position_ok = spec.spellings.is_empty() || rest != before;
            if sigil_position_ok {
                if let Some((spelling, tail)) = try_bare_sigil(rest) {
                    if already_collected(&spec, &spelling) {
                        break;
                    }
                    saw_explicit_anywhere |= explicit;
                    last_was_long_like = false;
                    spec.spellings.push(spelling);
                    rest = tail;
                    continue;
                }
            }
            break;
        }

        rest = skip_separators(rest);
        if rest.is_empty() {
            spec.fully_consumed = true;
            return spec;
        }

        // Leftover text that itself parses as another flag spelling never
        // becomes a value (pod2html's `--quiet --noquiet --verbose
        // --noverbose`): honest incompleteness over a guess. S-083.
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
        // First value wins: a repeated placeholder names one value once.
        if spec.value_name.is_none() {
            spec.value_name = Some(value_name);
            spec.value_kind = kind;
        }

        // The alias list may continue past it via `,`/`|`, see
        // [`alias_continues`], or via the word `or` when the second
        // spelling repeats the value too (`icupkg`'s `-s path or
        // --sourcedir path`), see [`or_alias_continues`]. S-099.
        let Some(next) = alias_continues(tail).or_else(|| or_alias_continues(tail)) else {
            spec.fully_consumed = tail.trim().is_empty();
            return spec;
        };
        rest = next;
    }
}

// The alias list a value spec used to terminate: argparse and the sg_*
// family repeat the value placeholder after each spelling
// (`-p PID, --pid PID`), which used to drop the second spelling. See
// docs/shapes.md S-083; oracle at xtask/src/dropped_alias.rs shares this
// rule deliberately. A false positive here merges two distinct flags,
// worse than the dropped-alias defect it fixes.

/// True when `c` separates the spellings of one flag rather than
/// belonging to a value: `,` or `|`. Whitespace deliberately excluded —
/// see [`skip_separators`] and docs/shapes.md S-083.
fn is_alias_separator(c: char) -> bool {
    c == ',' || c == '|'
}

/// True when `after_separator` (text right after a `,`/`|`) really is the
/// next spelling: a spelling must parse there, and what follows it must
/// terminate it — a `}`/`)`/`]` directly after means the dash was inside a
/// bracketed value (`{a,-b}`), not an alias separator.
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

/// `rest` with a leading `or`-joined-alias separator removed, when `rest`
/// opens with the bare word `or` followed by whitespace and then a real
/// spelling ([`alias_follows`]) — `-h  or  --help`. Word-bounded (`or` must
/// be followed by whitespace, never glued to the next token) so a value
/// spec or a description spelled "or" is never mistaken for this. See
/// docs/shapes.md S-099.
fn strip_or_alias_separator(rest: &str) -> Option<&str> {
    let after = rest.strip_prefix("or")?;
    if !after.starts_with(|c: char| c.is_ascii_whitespace()) {
        return None;
    }
    let after = after.trim_start_matches(' ');
    if !alias_follows(after) {
        return None;
    }
    or_alias_ends_the_spec(after).then_some(after)
}

/// True when the spelling opening `after` is the last thing in the spec
/// fragment, or is followed by a real column boundary, or by another `or`
/// in a chain (`icupkg`'s `-h or -? or --help`). `pod2man`'s prose
/// sentence `--lquote or --rquote overrides --quotes.` and `java`'s
/// `-m or --module <module>/<mainclass> are passed as the arguments`
/// both continue after a single space, so neither is a row joining two
/// spellings. See docs/shapes.md S-099.
fn or_alias_ends_the_spec(after: &str) -> bool {
    let token_len = after.find([' ', '\t', ',', '|']).unwrap_or(after.len());
    let tail = &after[token_len..];
    if tail.is_empty() || tail.starts_with(['\t', ',', '|', '=', '[']) || tail.starts_with("  ") {
        return true;
    }
    let chained = tail.trim_start_matches(' ');
    chained
        .strip_prefix("or")
        .is_some_and(|t| t.starts_with([' ', '\t']))
}

/// True when `before_separator` ends in a finished value placeholder — a
/// word character or a bracketed closer — something a separator could
/// actually separate from. lsof's `+|-e s` has `+` on the left, so this
/// refuses it rather than fabricating `-e` with a literal `+` value. See
/// docs/shapes.md S-086.
fn separator_has_a_left_operand(before_separator: &str) -> bool {
    before_separator
        .chars()
        .next_back()
        .is_some_and(|c| c.is_alphanumeric() || c == '_' || matches!(c, '}' | ')' | ']' | '>'))
}

/// The rest of an alias list continuing past a value spec, or `None`. An
/// explicit separator must be the next non-space character in
/// `after_value` — a space alone never resumes the run — and a whole
/// spelling must follow it.
fn alias_continues(after_value: &str) -> Option<&str> {
    let s = after_value.trim_start_matches(' ');
    let separator = s.chars().next().filter(|c| is_alias_separator(*c))?;
    let rest = &s[separator.len_utf8()..];
    alias_follows(rest).then(|| rest.trim_start_matches(' '))
}

/// The rest of an alias list continuing past a value spec via the bare
/// word `or`, mirroring [`alias_continues`]'s `,`/`|` handling for the
/// value-carrying form of the or-joined alias (`icupkg`'s `-s path or
/// --sourcedir path  directory for the --add items`). The second
/// spelling must itself carry a value — one space then a bare token, not
/// the two-space run that would already be the naive column gap — and
/// that value must end the spec fragment, meet a real column boundary, or
/// chain into another `or`, the same gate [`or_alias_ends_the_spec`]
/// already applies, shifted past one value token. See docs/shapes.md
/// S-099.
fn or_alias_continues(after_value: &str) -> Option<&str> {
    let s = after_value.trim_start_matches(' ');
    let after_or = s.strip_prefix("or")?;
    if !after_or.starts_with(|c: char| c.is_ascii_whitespace()) {
        return None;
    }
    let spelling_start = after_or.trim_start_matches(' ');
    if !alias_follows(spelling_start) {
        return None;
    }
    let spelling_len = spelling_start
        .find([' ', '\t', ',', '|'])
        .unwrap_or(spelling_start.len());
    let after_spelling = &spelling_start[spelling_len..];
    let value_part = after_spelling
        .strip_prefix(' ')
        .filter(|v| !v.starts_with(' '))?;
    or_alias_ends_the_spec(value_part).then_some(spelling_start)
}

/// Rewrite a brace-delimited alternation of flag spellings into the
/// comma-free alias list [`parse_flag_spec`] already reads:
/// `{-i|--input} <input xml file>` becomes `-i --input <input xml file>`.
/// Braces only — a leading `[` is left alone (see
/// [`looks_like_flag_start`]: in an options table it usually means
/// "optional", not "entry starts here"). See docs/shapes.md S-084.
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
/// explicit `,`/`|`, not whitespace alone.
///
/// Does not assume `after` is a suffix of `before`: `str::get` returns
/// `None` (never panics) when the byte count lands off a char boundary or
/// `after` is longer than `before`, degrading to "no separator" rather
/// than aborting. See docs/shapes.md S-081.
fn saw_explicit_separator(before: &str, after: &str) -> bool {
    let consumed_len = before.len().saturating_sub(after.len());
    before
        .get(..consumed_len)
        .is_some_and(|consumed| consumed.contains([',', '|']))
}

/// Same "long-like" shape rule as [`mandible_core::Entity::long_spelling`]:
/// two dashes always, or one dash with a name longer than one character —
/// which includes an abbreviation-bracket spelling like `-r[esolve]` even
/// though [`try_short`] read it.
fn is_long_like(spelling: &Spelling) -> bool {
    matches!(spelling.dashes, Dashes::Double)
        || (matches!(spelling.dashes, Dashes::Single) && spelling.name.chars().count() > 1)
}

/// True when `spec` already carries a spelling with the same rendered
/// identity (dashes + name) as `candidate`. A duplicate is never a
/// correct parse, so the alias loop refuses to record one a second time
/// (honest incompleteness, `fully_consumed: false`, rather than
/// fabricating a repeat reading). See docs/shapes.md S-085.
fn already_collected(spec: &FlagSpec, candidate: &Spelling) -> bool {
    spec.spellings
        .iter()
        .any(|s| s.dashes == candidate.dashes && s.name == candidate.name)
}

/// True when `spelling` is a genuine one-letter short — the same shape
/// [`FlagSpec::short`] and [`mandible_core::Entity::short`] use — as
/// opposed to a long-like spelling or a value. Used only to decide whether
/// the *previous* spelling in an alias run was short, for the
/// whitespace-continuation rules in [`parse_flag_spec`]'s alias loop.
fn is_genuine_short(spelling: &Spelling) -> bool {
    matches!(spelling.dashes, Dashes::Single) && spelling.name.chars().count() == 1
}

/// The character count of the short-flag run at the front of `input`
/// (after its leading `-`, before any abbreviation bracket or terminator),
/// or `None` if `input` is not short-flag-shaped at all.
///
/// Mirrors the run [`try_short`] itself computes, exposed separately so a
/// caller can tell "this really is a one-letter short" apart from "this is
/// a longer, unbracketed run that [`try_short`]'s first-character fallback
/// merely *truncates* down to one letter" — the distinction
/// [`parse_flag_spec`]'s whitespace-continuation rule (ii) needs.
fn short_run_char_count(input: &str) -> Option<usize> {
    let rest = input.strip_prefix('-')?;
    let run_end = rest
        .char_indices()
        .find(|(_, c)| !is_short_char(*c))
        .map_or(rest.len(), |(i, _)| i);
    if run_end == 0 {
        return None;
    }
    Some(rest[..run_end].chars().count())
}

/// True when the text right after a whitespace-continued spelling is a
/// bare value token, not nothing, another alias, or another flag spelling.
/// Used by rule (iii): a trailing value means a usage example naming two
/// flags (xxd's `-r -s off`), not one flag's two spellings. S-083.
fn trailing_token_is_a_value(tail: &str) -> bool {
    let t = tail.trim_start_matches(' ');
    if t.is_empty() || t.starts_with([',', '|']) {
        return false;
    }
    try_short(t).is_none() && try_long(t).is_none()
}

/// In alias position (text right after an explicit `,`/`|`), a
/// multi-letter single-dash run with no abbreviation bracket is a
/// spelling in its own right (gold's `-G, -shared`), not `try_short`'s
/// truncate-to-first-character fallback. `None` for a genuine one-letter
/// run or one a valid abbreviation bracket already claims. See
/// docs/shapes.md S-083.
fn try_alias_position_single_dash_long(input: &str) -> Option<(Spelling, &str)> {
    let mut s = input;
    short_dash(&mut s).ok()?;
    // Returns the whole run as the name, so it must also stop at `|` or a
    // pipe-alternation row (socat-mux.sh) swallows the next separator.
    let run_end = s
        .char_indices()
        .find(|(_, c)| !is_short_char(*c) || *c == '|')
        .map_or(s.len(), |(i, _)| i);
    if run_end < 2 {
        return None;
    }
    let run = &s[..run_end];
    let after_run = &s[run_end..];
    if parse_abbrev_bracket(after_run).is_some() {
        return None;
    }
    Some((
        Spelling {
            name: run.to_string(),
            dashes: Dashes::Single,
            negatable: false,
            abbrev: None,
        },
        after_run,
    ))
}

fn short_dash(input: &mut &str) -> Res<char> {
    '-'.parse_next(input)
}

fn long_dashes<'s>(input: &mut &'s str) -> Res<&'s str> {
    "--".parse_next(input)
}

fn long_name<'s>(input: &mut &'s str) -> Res<&'s str> {
    // The first character keeps the original class (alphanumeric or `-`):
    // a real long option never starts with `_`. `less --help`'s
    // `--_<name>` row is backspace-overstrike underlining (`_\b<...`), not
    // a spelling glued onto `--`; letting `_` open the name here would
    // read that artifact as flag `--_`. See `a_leading_underscore_after_
    // dashdash_is_never_a_spelling` below.
    //
    // `_` does join `-` in the *tail*: a real long option name may
    // contain it (icupkg's `--auto_toc_prefix_with_type`, sg_luns's
    // `--lu_cong`, bpfcc's `--extended_fields`); `sections/spelling.rs`
    // already accepts it in its own name grammar and this was the odd one
    // out. See docs/shapes.md S-106.
    (
        take_while(1..=1, |c: char| c.is_alphanumeric() || c == '-'),
        take_while(0.., |c: char| c.is_alphanumeric() || c == '-' || c == '_'),
    )
        .take()
        .parse_next(input)
}

/// `-x`, or `-xy...[rest]` where an abbreviation-continuation bracket
/// immediately follows a run of one or more such characters (`ip`'s
/// `-V[ersion]`/`-rc[vbuf]`). With no bracket, only the run's first
/// character is consumed, so a genuinely glued value (`-2CDlNuVv`) is
/// untouched. See docs/shapes.md S-006.
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
    // A letter run directly glued to a comma with no following space
    // (`-Wa,<options>`) is the whole spelling up to the comma; the value
    // follows the comma. Two things this must never claim: a genuine
    // alias separator, which always has a space before the next spelling
    // (`-es, -Es`), and the existing glued-short-plus-raw-argument
    // convention `help_text::sections::repair` already owns
    // (`-Wl,-rpath=/usr/lib` — see `the_glued_value_convention_is_never_
    // repaired_when_it_carries_an_equals`), which starts with another dash.
    // Both are excluded by requiring what follows the comma to be neither
    // whitespace nor a dash. See docs/shapes.md S-116.
    if run.chars().count() > 1
        && run.chars().all(|c| c.is_ascii_alphabetic())
        && after_run
            .strip_prefix(',')
            .is_some_and(|rest| !rest.is_empty() && !rest.starts_with(['-', ' ', '\t']))
    {
        return Some((
            Spelling {
                name: run.to_string(),
                dashes: Dashes::Single,
                negatable: false,
                abbrev: None,
            },
            after_run,
        ));
    }
    // No abbreviation bracket: an ordinary one-character short flag;
    // anything past it is left for the rest of the grammar.
    let c = run.chars().next()?;
    Some((Spelling::short(c), &s[c.len_utf8()..]))
}

fn is_short_char(c: char) -> bool {
    c != ' ' && c != ',' && c != '=' && c != '[' && c != '-'
}

/// The longest prefix [`try_short`]/[`try_long`] will read as an
/// abbreviation before a bracket. Real conventions are one or two letters;
/// this bound keeps the model from swallowing a synopsis placeholder like
/// unzip's `[-opts[modifiers]]`. See docs/shapes.md S-006.
const MAX_ABBREV_PREFIX_LEN: usize = 3;

/// Parses an abbreviation-continuation bracket glued onto a flag spelling
/// (`ip`'s `-V[ersion]`, `-rc[vbuf]`). Mirrors `strip_optional_modifier_
/// suffix`'s command-name convention on the flag side. Without this,
/// [`try_value`]'s `[VALUE]` arm would fabricate `-V` taking an optional
/// value literally named `"ersion"`.
///
/// Narrow by construction: content must be all ASCII lowercase letters
/// and hyphens, never upper/mixed-case, angle-delimited, or `=`-prefixed
/// (real optional-value conventions), so it can't claim those instead.
/// See docs/shapes.md S-006.
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
    // Drop stray closing punctuation right after the bracket: `ip`'s
    // `OPTIONS := { -V[ersion] | ... | -c[olor]}` leaves the group's own
    // closing `}` glued on, leftover from the enclosing group, never a
    // value. See docs/shapes.md S-006.
    while matches!(after.chars().next(), Some('}' | ')' | ']')) {
        after = &after[1..];
    }
    Some((content, after))
}

/// `--long-name`, optionally prefixed with GNU getopt_long's negatable
/// bracket `--[no-]long-name`, and optionally suffixed with an
/// abbreviation bracket `--br[ief]`. The two never coexist in the
/// measured fleet; negatable is checked first if they did. See
/// docs/shapes.md S-077.
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

/// The bare end-of-options marker `--` (atlas S-096): matches the marker
/// alone, leaving whatever follows (an alias, `cargo fmt`'s synopsis
/// `-- <rustfmt_options>...`) for the rest of this grammar. Glued onto
/// more name-shaped text (`objdump`'s `--[section-]headers`) it is left
/// alone, never fabricated into a second alias — `try_long` cannot read
/// either shape. No `+` arm: a bare `+` line has no signal telling an
/// option row apart from prose, and fabricated flags on `git-lfs`/`date`
/// in an earlier version of this function, caught in review, before it
/// was narrowed to `--` only.
fn try_bare_sigil(input: &str) -> Option<(Spelling, &str)> {
    let rest = input.strip_prefix("--")?;
    if rest.is_empty() || rest.starts_with([' ', '\t', ',', '|']) {
        return Some((Spelling::bare("--"), rest));
    }
    None
}

/// Strips a leading `[no-]`/`[no]` prefix, if present. Recognized
/// structurally (content exactly `no`/`no-`), never by tool name. See
/// docs/shapes.md S-077.
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
/// True when a bracketed group names something a reader can use: it
/// carries a letter or a digit and no unclosed `[`. `xxd`'s `-s [+][-]seek`
/// and `gold`'s `--debug [all,files,script,task][,...]` fail the first
/// test, `fzf-tmux`'s `-p [WIDTH[%][,HEIGHT[%]]]` the second, and folding
/// any of them would build a value name out of punctuation. See
/// docs/shapes.md S-097.
fn foldable_value(value: &str) -> bool {
    !value.contains('[') && value.chars().any(|c| c.is_ascii_alphanumeric())
}

fn try_value(input: &str) -> Option<(String, ValueKind, &str)> {
    let mut s = input;

    // Optional-value bracketed forms: `[=VALUE]` or `[VALUE]`, possibly
    // followed by further bracketed optional values glued directly on
    // with no separator (`-V[N][fname]`, vim's own row; docs/shapes.md
    // S-097). `Entity::value_name` is one field, so a glued run folds
    // into one value keeping the run's own source spelling, brackets
    // included (`-V[N][fname]` -> `[N][fname]`). A single, non-glued
    // group stays bracket-free, as before. Adjacency (no whitespace
    // before the next `[`) is the only signal, structural not per-tool.
    if open_bracket(&mut s).is_ok() {
        let _has_eq = equals_sign(&mut s).is_ok();
        let name = value_inside_brackets(&mut s).ok()?;
        close_bracket(&mut s).ok()?;
        let mut combined = name.to_string();
        let mut current = name;
        let mut folded_any = false;
        while foldable_value(current) {
            let mut probe = s;
            if open_bracket(&mut probe).is_err() {
                break;
            }
            let Ok(next_name) = value_inside_brackets(&mut probe) else {
                break;
            };
            if close_bracket(&mut probe).is_err() || !foldable_value(next_name) {
                break;
            }
            if !folded_any {
                combined = format!("[{combined}]");
                folded_any = true;
            }
            combined.push('[');
            combined.push_str(next_name);
            combined.push(']');
            s = probe;
            current = next_name;
        }
        return Some((combined, ValueKind::Optional, s));
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

/// Take one "value token": an angle/brace-delimited placeholder or a run
/// of non-whitespace. Also stops at an alias separator that the next
/// spelling follows (`sg_sanitize`'s `--count=OC|-c OC` has no whitespace
/// in `OC|-c`), but keeps a separator inside a choice list
/// (`{java,perl,...}`, `7|8|9|...`). See docs/shapes.md S-083.
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

/// True if `input` starts with something recognizable as a flag — used
/// by the layout parser to decide whether a line begins a new flag entry.
/// The `-` prefix is the dominant case; the second arm is a
/// brace-delimited alternation of bare flag spellings (cache_restore).
/// Braces only, never brackets — a leading `[` usually means "optional".
/// See docs/shapes.md S-084.
pub fn looks_like_flag_start(input: &str) -> bool {
    let trimmed = input.trim_start();
    if trimmed.starts_with('-') {
        return !is_dash_underline_token(first_token(trimmed));
    }
    // Deliberately no `+` arm: a bare `+`-led line has no reliable signal
    // telling an option row (vim.basic's `+`, `+<lnum>`) apart from
    // ordinary structure that merely starts with the character
    // (git-lfs's AsciiDoc list-continuation marker, date's
    // `%`-conversion-modifier table row). See `try_bare_sigil`'s own doc
    // comment.
    parse_flag_alternation(trimmed).is_some_and(|alt| alt.open == '{')
}

/// The first whitespace-delimited token of `input`, or the whole string
/// when it carries none.
fn first_token(input: &str) -> &str {
    input.split_whitespace().next().unwrap_or(input)
}

/// True when `token` is nothing but a run of 3+ dashes — jmod's header-
/// underline row (`------  -----------`) under a two-column heading.
/// Threshold is 3, not 2, because a bare `--` end-of-options marker must
/// stay eligible to open a flag entry. See docs/shapes.md S-090.
pub fn is_dash_underline_token(token: &str) -> bool {
    token.len() >= 3 && token.bytes().all(|b| b == b'-')
}

// The docopt bracket-group flag row: LVM's emitter writes one flag per
// physical line as a whole `[...]` group, never `-`-prefixed and never a
// `{...}` alternation. A separate, row-level predicate — never widened
// into `looks_like_flag_start`, since lsof's usage-block continuation
// lines also open with `[` and that predicate doubles as the usage
// block's own terminator. See docs/shapes.md S-005.

/// The inner content of a [`looks_like_bracket_flag_row`] line, or `None`.
/// Two conditions: `input` trimmed is exactly one bracket group (nothing
/// before `[`, nothing but whitespace after `]`), and the group's content
/// starts with `-` — which turns away LVM's operand rows in the identical
/// notation (`[ COMMON_OPTIONS ]`, `[ VG|Tag ... ]`). See docs/shapes.md
/// S-005.
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
    // Refuse a row whose alias run does not finish at the first
    // whitespace gap — ethtool's `--all-groups | --groups [...]` is an
    // alternation between two different flags, not LVM's shape. Missing
    // beats invented. See docs/shapes.md S-005.
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
/// [`looks_like_paren_alternation_open`] group — leading `(`, trailing
/// `,`/`)` stripped, leaving the fragment [`parse_flag_spec`] reads.
/// `None` when the remainder doesn't itself start with `-`. See
/// docs/shapes.md S-088.
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

// The stanza head's own mode-selecting flag: LVM's emitter documents each
// mode as a prose line, then `vgchange -a|--activate y|n|ay`, then the
// mode's `[...]`/`(...)` rows. The generic heading scanner already keeps
// this line as the block's `group` text but never parses it for the flag
// it names. See docs/shapes.md S-089.

/// True if `rest` (text after a stanza head's tool-name prefix) opens
/// with a bare flag spelling and names no second flag anywhere after —
/// refused for `blkid`'s `-p [--match-tag <tag>]` and `jar`'s
/// `--update --file foo.jar ...`, both of which would otherwise fabricate
/// or drop a flag. See docs/shapes.md S-089.
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

// The flag-alternation group: a delimited alternation of flag spellings,
// with three real renderings (cache_restore, eqn, xfs_io) that each lost
// flags before this rule. Every alternative must be a bare flag spelling
// — a value alternation (`{always|never|auto}`) or a member carrying its
// own value (`[--count=OC|-c OC]`) is refused, missing over invented.
// Shared with xtask's brace-alternation-flag detector rather than
// restated. Member-count threshold belongs to each caller, not here. See
// docs/shapes.md S-084.

/// A delimited alternation of bare flag spellings, plus whatever followed
/// the closing delimiter — see [`parse_flag_alternation`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlagAlternation {
    /// The opening delimiter actually used, `'{'` or `'['`.
    pub open: char,
    /// The delimited span exactly as written, delimiters included:
    /// `"{-v | --version}"`. Carried rather than reconstructed, since
    /// subtracting trimmed `rest`'s length gets it wrong when there's
    /// surrounding whitespace.
    pub group: String,
    /// Each alternative's bare spelling in source order: `["-i", "--input"]`.
    pub members: Vec<String>,
    /// Text after the closing delimiter, trimmed — the operand the
    /// alternatives share (xfs_io's `cmd`), empty when the group stands
    /// alone. Left verbatim; interpreting it is the caller's business.
    pub rest: String,
}

/// Read `input` as a delimited alternation of bare flag spellings —
/// `{-i|--input} <input xml file>`, `[-c|-C] cmd` — anchored at `input`'s
/// first non-whitespace character. `None` unless the delimiter has a
/// matching close, splitting on `|` yields at least one alternative, and
/// every alternative is a bare flag spelling ([`is_bare_flag_spelling`]) —
/// the condition keeping a value or subcommand alternation out entirely.
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

/// Split `input` (first char `open`) into the content between `open` and
/// its matching `close`, and everything after. Depth counted over the
/// `open`/`close` pair only, so `[[-c|-C] cmd]` works. Boundaries always
/// from `char_indices`, never a raw byte offset.
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

/// Split an alternation group's content on `|` at nesting depth 0,
/// dropping empty fragments, so a nested value spec is never split
/// through. `pub(super)`: also reused by
/// `sections::split_bnf_alternation_row` for the iproute2-family shape.
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

/// True when `token` is a bare flag spelling and nothing else: `--name`
/// or `-c` (one [`is_bundle_member_char`] character). Refuses a value, a
/// bundle, a single-dash long option — narrow on purpose, the only thing
/// standing between "an alternation of flags" and "anything at all". See
/// docs/shapes.md S-084.
pub(super) fn is_bare_flag_spelling(token: &str) -> bool {
    if let Some(name) = token.strip_prefix("--") {
        let mut cs = name.chars();
        // `_` joins `-` for the same reason `long_name` does: a real
        // long option name may contain it (S-106).
        return cs.next().is_some_and(|c| c.is_ascii_alphabetic())
            && cs.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    }
    if let Some(name) = token.strip_prefix('-') {
        let mut cs = name.chars();
        return cs.next().is_some_and(is_bundle_member_char) && cs.next().is_none();
    }
    false
}

// The bundled-short-flag cluster: a usage synopsis like `[-2CDlNuVv]`
// names a set of bundled boolean switches, not one flag with a glued
// value. `parse_flag_spec` alone can't see that. Not wired into
// `parse_flag_spec` itself — the identical shape in an option-table row
// is the GCC/Clang single-dash-value convention, thousands of correct
// parses fleet-wide — only the synopsis path asks this question.
// Discriminator is the shape of the swallowed text, never the tool;
// shared character-for-character with xtask's bundling oracle. See
// docs/shapes.md S-087.

/// The fewest members a cluster must carry to be read as a bundle: a
/// surviving flag plus at least two swallowed ones. Three, not two —
/// deliberate lost recall, since two-character clusters are roughly half
/// real collapses and half genuine multi-character single-dash flags
/// (rpcgen's `[-Sc]`, etc.) with no shape distinguishing them. See
/// docs/shapes.md S-087.
const MIN_CLUSTER_MEMBERS: usize = 3;

/// The fewest ASCII letters a cluster must carry before
/// [`cluster_is_ordered`] can vouch for it. A cluster with no letters
/// (`-1024`) is vacuously ordered; two letters is the floor at which
/// "in order" is a statement about anything.
const MIN_ORDERED_LETTERS: usize = 2;

/// Whether `c` could be a single-character flag name. ASCII alphanumeric
/// plus `#` (tcpdump's real switch); rejects value-spec punctuation
/// (`{`, `<`, `=`, `-`, ...) so `filefrag`'s `[-b{blocksize}[KMG]]` and a
/// hyphenated long option both fail here. See docs/shapes.md S-087.
fn is_bundle_member_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '#'
}

/// True when `members` carries at least [`MIN_ORDERED_LETTERS`] ASCII
/// letters, all in non-decreasing case-insensitive order — the listing
/// convention a hand-written flag bundle follows. Case is folded since the
/// convention interleaves both cases of one letter (`hH`, `lL`); against
/// word-shaped values (`-Zscript`, `-Dname`) it stays quiet, unordered.
/// See docs/shapes.md S-087.
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

/// True when `swallowed` (every member after the first — what
/// `parse_flag_spec` would store as `value_name`) contains both an ASCII
/// uppercase and lowercase letter. A value placeholder is written in one
/// case; a mixed-case swallowed run is a switch set instead. Measured on
/// the swallowed half, not the whole cluster, so `-Zscript`/`-Dname`
/// (mixed cluster, unmixed swallowed half) stay correct parses. The other
/// half of the two-signal test — see docs/shapes.md S-087.
fn swallowed_members_mix_case(swallowed: &str) -> bool {
    swallowed.chars().any(|c| c.is_ascii_uppercase())
        && swallowed.chars().any(|c| c.is_ascii_lowercase())
}

/// True when every character of `members` is distinct, case-sensitively.
/// A bundle never repeats a switch; case matters since `-v`/`-V` are
/// different flags and real bundles carry both. Decisive against the
/// repeated-character family (`-vvv`, `strace`'s `[-DDD]`). S-087.
fn members_are_distinct(members: &str) -> bool {
    let mut seen = HashSet::new();
    members.chars().all(|c| seen.insert(c))
}

/// Read `token` (one whitespace-delimited synopsis token, brackets
/// already stripped) as a bundle of single-character boolean short flags,
/// returning members in source order. `None` unless: exactly one `-` then
/// member characters with no whitespace/second dash (tmux's `[-c
/// shell-command]` stays a value-taking flag by this separator check);
/// every member is bundle-shaped; at least [`MIN_CLUSTER_MEMBERS`]; all
/// distinct; and either ordered or the swallowed half mixes case. Known
/// false negatives: unsorted uniformly-cased bundles, and repeated-switch
/// bundles (`strace`'s `[-ACdffhiqqrtttTvVwxxyyzZ]`). See docs/shapes.md
/// S-087.
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

    /// vim's `-V[N][fname]` (docs/shapes.md S-097, corpus/vim.basic's
    /// `audit-seed4` fixture): two bracketed optional values glued
    /// directly together. The value name keeps the source spelling of the
    /// glued run, brackets included, since each group is independently
    /// optional.
    #[test]
    fn parses_two_glued_optional_bracketed_values() {
        let spec = parse_flag_spec("-V[N][fname]");
        assert_eq!(spec.short(), Some('V'));
        assert_eq!(spec.value_name.as_deref(), Some("[N][fname]"));
        assert_eq!(spec.value_kind, ValueKind::Optional);
        assert!(spec.fully_consumed);
    }

    /// `g++ --help`'s own row, byte-exact (`corpus/aarch64-linux-gnu-g++-13`).
    /// A letter run glued directly to a comma is the whole spelling up to
    /// the comma, and the value follows it. See docs/shapes.md S-116.
    #[test]
    fn a_letter_run_glued_to_a_comma_is_the_whole_spelling() {
        for (row, name, value) in [
            ("-Wa,<options>", "Wa", "<options>"),
            ("-Wp,<options>", "Wp", "<options>"),
            ("-Wl,<options>", "Wl", "<options>"),
        ] {
            let spec = parse_flag_spec(row);
            assert_eq!(spec.long(), Some(name), "{row}");
            assert_eq!(spec.value_name.as_deref(), Some(value), "{row}");
            assert_eq!(spec.value_kind, ValueKind::Required, "{row}");
            assert!(spec.fully_consumed, "{row}");
        }
    }

    /// The compiler glued-value convention this rule must never touch: no
    /// comma, so the leading letter alone is the spelling and the rest is
    /// a glued value, exactly as before. See docs/shapes.md S-116.
    #[test]
    fn a_letter_run_with_no_comma_keeps_the_single_letter_glued_reading() {
        for (row, letter, value) in [
            ("-Idirectory", 'I', "directory"),
            ("-Lpath", 'L', "path"),
            ("-lname", 'l', "name"),
        ] {
            let spec = parse_flag_spec(row);
            assert_eq!(spec.short(), Some(letter), "{row}");
            assert_eq!(spec.value_name.as_deref(), Some(value), "{row}");
        }
    }

    /// A comma directly followed by a real second alias is still an
    /// ordinary alias separator, not this rule — `alias_follows` gates it.
    #[test]
    fn a_comma_that_really_introduces_an_alias_is_not_swallowed_as_a_name() {
        let spec = parse_flag_spec("-x,--extra");
        assert_eq!(spec.short(), Some('x'));
        assert_eq!(spec.long(), Some("extra"));
    }

    /// `xxd`'s own `-s [+][-]seek` row, byte-exact. Neither bracket names
    /// a value, so nothing is folded and the flag keeps what it had. See
    /// docs/shapes.md S-097.
    #[test]
    fn does_not_fold_bracket_groups_that_name_no_value() {
        let spec = parse_flag_spec("-s [+][-]seek");
        assert_eq!(spec.value_name.as_deref(), Some("+"));
    }

    /// `fzf-tmux`'s own `-p [WIDTH[%][,HEIGHT[%]]]` row, byte-exact. The
    /// first group is already an unclosed bracket, so folding a second one
    /// onto it would compound a misread. See docs/shapes.md S-097.
    #[test]
    fn does_not_fold_onto_an_unclosed_bracket() {
        let spec = parse_flag_spec("-p [WIDTH[%][,HEIGHT[%]]]");
        assert_eq!(spec.value_name.as_deref(), Some("WIDTH[%"));
    }

    /// A single bracketed optional value followed, after whitespace, by
    /// text that is not glued on must not be folded in — the shape S-097
    /// recovers is adjacency with no separator, never "starts with `[`
    /// somewhere later in the fragment".
    #[test]
    fn does_not_glue_a_bracket_separated_by_whitespace() {
        let spec = parse_flag_spec("-p[N] [not glued]");
        assert_eq!(spec.short(), Some('p'));
        assert_eq!(spec.value_name.as_deref(), Some("N"));
        assert_eq!(spec.value_kind, ValueKind::Optional);
        assert!(!spec.fully_consumed);
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

    /// Four real argparse rows. See docs/shapes.md S-083.
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

    /// sg_sanitize's real pipe-separated rows. See docs/shapes.md S-083.
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

    /// javaflow-bpfcc's real six-comma choice list. See docs/shapes.md
    /// S-083.
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

    /// A separator inside a value must never become an alias — merging
    /// two flags is worse than dropping one. See docs/shapes.md S-083.
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

    /// Whitespace alone never resumes an alias run, only an explicit
    /// `,`/`|`. S-083.
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

    // --- the `or`-joined alias, S-099 ------------------------------------

    /// vim.basic's real spec text, byte-exact, once the layout parser has
    /// admitted both spellings into it (see `spelling.rs`'s
    /// `extend_gap_past_or_joined_alias`).
    #[test]
    fn an_or_separator_joins_two_spellings_like_a_comma() {
        let spec = parse_flag_spec("-h  or  --help");
        assert_eq!(spec.short(), Some('h'));
        assert_eq!(spec.long(), Some("help"));
        assert!(spec.fully_consumed);
    }

    /// `icupkg`'s own chained row, byte-exact: three spellings joined by
    /// two `or` separators, description behind a real column gap.
    #[test]
    fn an_or_chain_joins_three_spellings() {
        let spec = parse_flag_spec("-h or -? or --help");
        assert_eq!(spec.short(), Some('h'));
        assert_eq!(spec.long(), Some("help"));
    }

    /// `pod2man`'s prose sentence, byte-exact. The word after `or` is a
    /// real spelling, and the sentence continues after one space, so this
    /// is prose about two options and not a row joining them.
    #[test]
    fn a_sentence_continuing_after_the_second_spelling_is_not_an_alias_join() {
        assert_eq!(
            strip_or_alias_separator("or --rquote overrides --quotes."),
            None
        );
        let spec = parse_flag_spec("--lquote or --rquote overrides --quotes.");
        assert_eq!(spec.long(), Some("lquote"));
        assert_ne!(spec.spellings.len(), 2, "--rquote must not become an alias");
    }

    /// A value or description that merely spells the word "or" is never
    /// mistaken for the separator — `alias_follows` demands a real
    /// spelling immediately after it.
    #[test]
    fn a_bare_or_with_no_spelling_after_it_is_not_a_separator() {
        assert_eq!(strip_or_alias_separator("or html"), None);
        assert_eq!(strip_or_alias_separator("original"), None);
        let spec = parse_flag_spec("--format or html");
        assert_eq!(spec.long(), Some("format"));
        assert_eq!(spec.value_name.as_deref(), Some("or"));
    }

    /// `icupkg`'s real value-carrying or-joined row, byte-exact (see
    /// corpus/icupkg/74.2/help.txt): both spellings repeat the value
    /// `path`. First value wins (S-083), so the joined entity's value
    /// name is the first spelling's own word.
    #[test]
    fn or_join_carries_the_value_on_both_spellings() {
        let spec = parse_flag_spec("-s path or --sourcedir path");
        assert_eq!(spec.short(), Some('s'));
        assert_eq!(spec.long(), Some("sourcedir"));
        assert_eq!(spec.value_name.as_deref(), Some("path"));
        assert!(spec.fully_consumed);
    }

    /// `icupkg`'s `-C comment or --comment comment`, byte-exact: a
    /// different word for the value than the `-s`/`--sourcedir` row above,
    /// still one value, still first-wins.
    #[test]
    fn or_join_with_value_uses_a_different_value_word() {
        let spec = parse_flag_spec("-C comment or --comment comment");
        assert_eq!(spec.short(), Some('C'));
        assert_eq!(spec.long(), Some("comment"));
        assert_eq!(spec.value_name.as_deref(), Some("comment"));
        assert!(spec.fully_consumed);
    }

    /// Negative: a value-carrying `or` join still only fires when the
    /// second spelling's own value ends the fragment or meets a real
    /// boundary. Here the second spelling's "value" runs straight into
    /// more prose on a single space, so this must not join — the same
    /// shape `a_sentence_continuing_after_the_second_spelling_is_not_an_alias_join`
    /// protects for the value-free form.
    #[test]
    fn or_join_with_value_refuses_when_the_second_value_runs_into_prose() {
        let spec = parse_flag_spec("-s path or --sourcedir word continues here");
        assert_ne!(
            spec.spellings.len(),
            2,
            "--sourcedir must not become an alias when its value runs into prose"
        );
    }

    /// lsof's real plus-or-minus convention, byte-exact. See
    /// docs/shapes.md S-086.
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
        // The shape that always worked must stay untouched.
        let spec = parse_flag_spec("-o, --output FILE");
        assert_eq!(spec.short(), Some('o'));
        assert_eq!(spec.long(), Some("output"));
        assert_eq!(spec.value_name.as_deref(), Some("FILE"));
        assert!(spec.fully_consumed);
    }

    /// tar's real multi-alias row with a value on each. S-083.
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

    /// Confirms the bracket content feeds `parse_flag_spec` correctly,
    /// not just that the brackets are stripped.
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
        // Must stay blind to `[`, or lsof's usage-block continuation
        // would end that block one line in.
        assert!(!looks_like_flag_start("[ -d|--debug ]"));
        assert!(!looks_like_flag_start("[-F [f]]"));
    }

    // --- the bundled-short-flag cluster ---------------------------------

    /// The members `parse_bundled_shorts` recovers, or `"-"` when declined.
    fn bundle(token: &str) -> String {
        match parse_bundled_shorts(token) {
            Some(members) => members.into_iter().collect(),
            None => "-".to_string(),
        }
    }

    /// Five real clusters, byte-exact. See docs/shapes.md S-087.
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

    /// The two signals each carry a family the other cannot see.
    #[test]
    fn either_ordering_or_a_case_mixing_swallowed_half_is_enough() {
        // Ordered only: od's switches are all lowercase.
        assert!(!swallowed_members_mix_case("bcdfilosx"));
        assert_eq!(bundle("-abcdfilosx"), "abcdfilosx");
        // Case-mixing only: tree sorts lowercase then uppercase separately.
        assert!(!cluster_is_ordered("acdfghilnpqrstuvxACDFJQNSUX"));
        assert_eq!(
            bundle("-acdfghilnpqrstuvxACDFJQNSUX"),
            "acdfghilnpqrstuvxACDFJQNSUX"
        );
    }

    /// Single-dash long options: correct parses that must never split.
    /// See docs/shapes.md S-087.
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

    /// Repeated-character flags: `-vv` fails the member floor, `-vvv`
    /// fails distinctness. See docs/shapes.md S-087.
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

    /// Deliberate lost recall at [`MIN_CLUSTER_MEMBERS`]: ssh-keygen's
    /// `[-hU]` is a real collapse, left alone since nothing about its
    /// shape separates it from a real two-letter flag. S-087.
    #[test]
    fn a_two_character_cluster_is_deliberately_left_alone() {
        assert_eq!(bundle("-hU"), "-"); // ssh-keygen, a real collapse
        for token in [
            "-Ss", "-it", "-st", "-ou", "-ac", "-as", "-ps", "-ox", "-pn",
        ] {
            assert_eq!(bundle(token), "-", "{token} is a real flag with a value");
        }
    }

    /// The separator is the whole difference between tmux's collapsed
    /// `[-2CDlNuVv]` and its five genuine valued flags. S-087.
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
        // A glued, ordered token does split, confirming the space did the
        // work above.
        assert_eq!(bundle("-cDeF"), "cDeF");
    }

    /// A numeric run orders vacuously (no letters); [`MIN_ORDERED_LETTERS`]
    /// keeps a glued numeric default from splitting into digits.
    #[test]
    fn a_glued_numeric_default_is_never_split() {
        for token in ["-b1024", "-j4", "-n0777"] {
            assert_eq!(bundle(token), "-", "{token} must not be split");
        }
        assert!(!cluster_is_ordered("b1024"));
    }

    /// Long options and the bare option terminator are not clusters.
    #[test]
    fn a_long_option_is_never_a_cluster() {
        for token in ["--verbose", "--no-pager", "--", "-", "abc", ""] {
            assert_eq!(bundle(token), "-", "{token:?} must not be split");
        }
    }

    /// `parse_flag_spec` itself stays unchanged: an option-table row with
    /// the identical shape is GCC's single-dash convention, genuinely one
    /// flag. The split lives at the synopsis call site.
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

    /// ip's real `OPTIONS := { ... | -c[olor]}` stray closing brace. S-006.
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

    /// ip's real `-rc[vbuf] [size]`, a two-letter abbreviation prefix.
    /// See docs/shapes.md S-006.
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

    /// jdeprscan's real `-?, -h, --help` row, all three comma-separated.
    /// See docs/shapes.md S-083.
    #[test]
    fn the_alias_loop_keeps_every_spelling_jdeprscan_style() {
        let spec = parse_flag_spec("-?, -h, --help");
        assert_eq!(spec.spellings.len(), 3, "{:?}", spec.spellings);
        assert_eq!(spec.spellings[0].render(), "-?");
        assert_eq!(spec.spellings[1].render(), "-h");
        assert_eq!(spec.spellings[2].render(), "--help");
        assert!(spec.fully_consumed);
    }

    /// jdeprscan's real `-? -h` two-column cell, bare space, no comma.
    /// See docs/shapes.md S-083 and corpus/jdeprscan/audit-seed2.
    #[test]
    fn bare_whitespace_still_continues_a_run_of_two_shorts() {
        let spec = parse_flag_spec("-? -h");
        assert_eq!(spec.spellings.len(), 2, "{:?}", spec.spellings);
        assert_eq!(spec.spellings[0].render(), "-?");
        assert_eq!(spec.spellings[1].render(), "-h");
    }

    /// GNU sort's real `-c, --check, --check=diagnose-first` row, which
    /// restates `--check`. See docs/shapes.md S-085.
    #[test]
    fn a_spelling_repeated_in_the_same_row_is_never_recorded_twice() {
        let spec = parse_flag_spec("-c, --check, --check=diagnose-first");
        assert_eq!(spec.spellings.len(), 2, "{:?}", spec.spellings);
        assert_eq!(spec.spellings[0].render(), "-c");
        assert_eq!(spec.spellings[1].render(), "--check");
    }

    /// jdb's real `-? -h --help -help` row: `-help` truncates to `-h` and
    /// collides with the already-read short. See docs/shapes.md S-085.
    #[test]
    fn jdbs_truncated_help_never_duplicates_the_short_h_already_read() {
        let spec = parse_flag_spec("-? -h --help -help");
        let names: Vec<&str> = spec.spellings.iter().map(|s| s.name.as_str()).collect();
        let h_count = names.iter().filter(|n| **n == "h").count();
        assert_eq!(h_count, 1, "no spelling may be recorded twice: {names:?}");
    }

    /// pod2html's real `--quiet --noquiet --verbose --noverbose` row:
    /// four independent negatable options, no comma. See docs/shapes.md
    /// S-083.
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

    /// iptables' real `--replace -R chain rulenum` row: long-then-short
    /// must not trigger the long-then-long gate. See docs/shapes.md S-083.
    #[test]
    fn a_long_then_short_pair_still_runs_together_on_bare_whitespace() {
        let spec = parse_flag_spec("--replace -R chain rulenum");
        assert_eq!(spec.spellings.len(), 2, "{:?}", spec.spellings);
        assert_eq!(spec.spellings[0].render(), "--replace");
        assert_eq!(spec.spellings[1].render(), "-R");
    }

    /// iptables' other real long-then-short row, `--append  -A chain`
    /// (two spaces). Rule (i) must stay silent: no explicit separator
    /// appeared. S-083.
    #[test]
    fn a_long_then_short_pair_with_no_earlier_explicit_separator_still_merges() {
        let spec = parse_flag_spec("--append  -A chain");
        assert_eq!(spec.spellings.len(), 2, "{:?}", spec.spellings);
        assert_eq!(spec.spellings[0].render(), "--append");
        assert_eq!(spec.spellings[1].render(), "-A");
    }

    /// jdeprscan's real `-? -h --help`: short-short-long, all bare
    /// whitespace, all three continuation rules stay silent. S-083.
    #[test]
    fn short_short_long_with_no_values_still_merges_on_bare_whitespace() {
        let spec = parse_flag_spec("-? -h --help");
        assert_eq!(spec.spellings.len(), 3, "{:?}", spec.spellings);
        assert_eq!(spec.spellings[0].render(), "-?");
        assert_eq!(spec.spellings[1].render(), "-h");
        assert_eq!(spec.spellings[2].render(), "--help");
        assert!(spec.fully_consumed);
    }

    /// dpkg-split's real `-a|--auto -o <complete> <part>`: rule (i)
    /// refuses to absorb `-o` after the explicit `|`. S-083.
    #[test]
    fn a_usage_example_naming_a_second_flag_after_an_explicit_run_is_not_absorbed() {
        let spec = parse_flag_spec("-a|--auto -o <complete> <part>");
        assert_eq!(spec.spellings.len(), 2, "{:?}", spec.spellings);
        assert_eq!(spec.spellings[0].render(), "-a");
        assert_eq!(spec.spellings[1].render(), "--auto");
        assert!(
            !spec.fully_consumed,
            "the unabsorbed `-o ...` must be left honestly unconsumed"
        );
    }

    /// screen's real `-D -RR`: rule (ii) refuses `-RR` as a second
    /// spelling since it's not itself a one-letter short. S-083.
    #[test]
    fn a_doubled_short_flag_usage_note_is_not_read_as_a_second_spelling() {
        let spec = parse_flag_spec("-D -RR");
        assert_eq!(spec.spellings.len(), 1, "{:?}", spec.spellings);
        assert_eq!(spec.spellings[0].render(), "-D");
        assert!(
            !spec.fully_consumed,
            "the unabsorbed `-RR` must be left honestly unconsumed"
        );
    }

    /// Rules (ii) and (iii) are each independently necessary — this
    /// constructed input is the case where (iii) alone would miss it,
    /// since a leading `|` reads as "alias continuing" not a value; only
    /// (ii)'s run-length check refuses it. S-083.
    #[test]
    fn rule_ii_independently_refuses_what_rule_iii_alone_would_miss() {
        let spec = parse_flag_spec("-D -R|--foo");
        assert_eq!(spec.spellings.len(), 1, "{:?}", spec.spellings);
        assert_eq!(spec.spellings[0].render(), "-D");
        assert!(
            !spec.fully_consumed,
            "the unabsorbed `-R|--foo` must be left honestly unconsumed"
        );
    }

    /// xxd's real `-r -s off`: rule (iii) refuses the merge since a bare
    /// value trails the second short. S-083.
    #[test]
    fn a_short_pair_usage_example_with_a_trailing_value_is_not_merged() {
        let spec = parse_flag_spec("-r -s off");
        assert_eq!(spec.spellings.len(), 1, "{:?}", spec.spellings);
        assert_eq!(spec.spellings[0].render(), "-r");
        assert!(
            !spec.fully_consumed,
            "the unabsorbed `-s off` must be left honestly unconsumed"
        );
    }

    /// gold's real `-G, -shared`: not `try_short`'s truncate-to-first-
    /// character fallback (would collide with `-s, --strip-all`). S-083.
    #[test]
    fn an_explicit_alias_separator_reads_an_unbracketed_single_dash_run_as_a_long_spelling() {
        let spec = parse_flag_spec("-G, -shared");
        assert_eq!(spec.spellings.len(), 2, "{:?}", spec.spellings);
        assert_eq!(spec.spellings[0].render(), "-G");
        assert_eq!(spec.spellings[1].render(), "-shared");
        assert!(spec.fully_consumed);
    }

    /// socat-mux.sh's real `-b|-S|-t|-T|-l <arg>`: five pipe-separated
    /// one-letter shorts. See docs/shapes.md S-083.
    #[test]
    fn a_pipe_separated_row_of_one_letter_shorts_is_unaffected_by_the_alias_position_reading() {
        let spec = parse_flag_spec("-b|-S|-t|-T|-l <arg>");
        assert_eq!(spec.spellings.len(), 5, "{:?}", spec.spellings);
        for (i, expected) in ["-b", "-S", "-t", "-T", "-l"].iter().enumerate() {
            assert_eq!(
                spec.spellings[i].render(),
                *expected,
                "{:?}",
                spec.spellings
            );
        }
        assert_eq!(spec.value_name.as_deref(), Some("<arg>"));
    }

    /// jdb's real `-? -h --help -help`: the trailing `-help` is reached
    /// only by bare whitespace, so alias-position reading must not apply.
    /// Out of scope: widening to bare whitespace. S-083.
    #[test]
    fn bare_whitespace_position_still_truncates_rather_than_reading_a_long_spelling() {
        let spec = parse_flag_spec("-? -h --help -help");
        assert!(
            !spec
                .spellings
                .iter()
                .any(|s| matches!(s.dashes, Dashes::Single) && s.name == "help"),
            "no bare-whitespace-reached spelling reads `-help` as a whole single-dash word: {:?}",
            spec.spellings
        );
    }

    /// unzip's real `[-opts[modifiers]]` placeholder: "opts" is a
    /// four-letter prefix, past [`MAX_ABBREV_PREFIX_LEN`], so this reads
    /// as ordinary `-o`. See docs/shapes.md S-006.
    #[test]
    fn an_over_long_bracket_prefix_is_not_read_as_an_abbreviation() {
        let spec = parse_flag_spec("-opts[modifiers]");
        assert_eq!(spec.spellings.len(), 1, "{:?}", spec.spellings);
        assert_eq!(spec.spellings[0].render(), "-o");
        assert_eq!(spec.spellings[0].abbrev, None);
    }

    /// The discriminator stays narrow: a real optional-value placeholder
    /// must still parse as a value exactly as before.
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

    /// vgchange's real unclosed leading paren flag group. S-088.
    #[test]
    fn paren_alternation_open_fires_on_an_unclosed_leading_paren_flag_group() {
        assert!(looks_like_paren_alternation_open(
            "( -l|--logicalvolume Number,"
        ));
        assert!(looks_like_paren_alternation_open("(    --addtag Tag,"));
    }

    /// A plain `|` alias-separator row with no leading `(` is not this
    /// shape.
    #[test]
    fn paren_alternation_open_is_false_with_no_leading_paren() {
        assert!(!looks_like_paren_alternation_open(
            "-l|--logicalvolume Number,"
        ));
        assert!(!looks_like_paren_alternation_open(
            "[ -A|--autobackup y|n ]"
        ));
    }

    /// A same-line, already-balanced parenthetical is not a group
    /// opening — the evidence is being left unclosed, not the dash.
    #[test]
    fn paren_alternation_open_refuses_a_balanced_same_line_parenthetical() {
        assert!(!looks_like_paren_alternation_open("(-x see docs)"));
        assert!(!looks_like_paren_alternation_open(
            "(-h) print this help information"
        ));
    }

    /// The three physical-line member shapes: opening, middle, closing.
    /// See docs/shapes.md S-088.
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

    /// `|` inside a member is untouched by the stripping — still an alias
    /// separator or a value's own choice list.
    #[test]
    fn paren_alternation_member_content_feeds_parse_flag_spec_correctly() {
        let content = paren_alternation_member_content("-x|--resizeable y|n,").unwrap();
        let spec = parse_flag_spec(content);
        assert_eq!(spec.short(), Some('x'));
        assert_eq!(spec.long(), Some("resizeable"));
        assert_eq!(spec.value_name.as_deref(), Some("y|n"));
    }

    /// A row that, once stripped, does not start with `-` is refused
    /// rather than fabricated.
    #[test]
    fn paren_alternation_member_content_refuses_a_non_flag_row() {
        assert_eq!(paren_alternation_member_content("( COMMON_OPTIONS,"), None);
        assert_eq!(paren_alternation_member_content("VG|Tag )"), None);
    }

    /// A bare flag token as the first word, refused for a bare invocation
    /// with nothing after the name.
    #[test]
    fn looks_like_stanza_head_flag_requires_a_leading_flag_token() {
        assert!(looks_like_stanza_head_flag("-a|--activate y|n|ay"));
        assert!(looks_like_stanza_head_flag("--refresh"));
        assert!(looks_like_stanza_head_flag("--systemid String VG"));
        assert!(!looks_like_stanza_head_flag(""));
        assert!(!looks_like_stanza_head_flag("VG"));
        assert!(!looks_like_stanza_head_flag("is a general-purpose tool"));
    }

    /// blkid and jar's real second-flag rows must both refuse. S-089.
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

    /// jmod's real header-underline row. See docs/shapes.md S-090 and
    /// corpus/jmod/17.0.20/help.txt.
    #[test]
    fn a_dash_underline_row_never_looks_like_a_flag_start() {
        assert!(!looks_like_flag_start(
            "------                              -----------"
        ));
        assert!(!looks_like_flag_start("---"));
        assert!(!looks_like_flag_start("----------"));
    }

    /// A bare `--` must stay eligible; the threshold is 3, not 2. S-090.
    #[test]
    fn a_bare_double_dash_still_looks_like_a_flag_start() {
        assert!(looks_like_flag_start("--"));
        assert!(looks_like_flag_start("-- end of options"));
    }

    // --- S-096: the bare `--` sigil. No `+` arm: see `try_bare_sigil`'s
    // own doc comment for the two fabrications (git-lfs, date) that
    // stopped a `+` arm from landing. ------------------------------------

    #[test]
    fn a_plus_led_row_never_looks_like_a_flag_start() {
        // Deliberately not recognized at all: git-lfs's bare `+` is an
        // AsciiDoc list-continuation marker in prose, and date's is a row
        // of a `%`-conversion-modifier table, not an option.
        assert!(!looks_like_flag_start("+"));
        assert!(!looks_like_flag_start("+<lnum>\t\tStart at line <lnum>"));
        assert!(!looks_like_flag_start("+  pad with zeros"));
    }

    #[test]
    fn parse_flag_spec_never_reads_a_bare_plus_as_a_spelling() {
        let spec = parse_flag_spec("+");
        assert!(spec.spellings.is_empty());
        let spec = parse_flag_spec("+<lnum>");
        assert!(spec.spellings.is_empty());
    }

    #[test]
    fn a_bare_double_dash_row_parses_as_a_flag_spelled_double_dash() {
        let spec = parse_flag_spec("--");
        assert_eq!(spec.spellings.len(), 1);
        assert_eq!(spec.spellings[0].name, "--");
        assert!(matches!(spec.spellings[0].dashes, Dashes::None));
        assert!(spec.fully_consumed);
    }

    #[test]
    fn a_long_name_that_legitimately_ends_in_plus_is_not_split() {
        // binutils `as`'s real `--gstabs+`: the trailing `+` is part of
        // the option's own name, not a second alias glued onto it.
        let spec = parse_flag_spec("--gstabs+");
        assert_eq!(spec.spellings.len(), 1);
        assert_eq!(spec.spellings[0].name, "gstabs");
        assert!(matches!(spec.spellings[0].dashes, Dashes::Double));
    }

    #[test]
    fn objdumps_bracketed_prefix_long_name_does_not_fabricate_a_dashdash_alias() {
        // `-h, --[section-]headers`: `try_long` cannot read the
        // `[section-]` optional-prefix convention, but the orphaned `--`
        // left over from that failed attempt must never become a second,
        // spurious alias of `-h`.
        let spec = parse_flag_spec("-h, --[section-]headers");
        assert_eq!(spec.spellings.len(), 1);
        assert_eq!(spec.spellings[0].name, "h");
    }

    #[test]
    fn a_glued_underscore_suffix_joins_the_long_name_not_the_value() {
        // corpus/compactsnoop-bpfcc/audit-seed2/help.txt: "  -e,
        // --extended_fields\n                        show system memory
        // state". `--extended` used to end at `_`, leaving a fabricated
        // value named `_fields` on a flag the tool never gives one. See
        // docs/shapes.md S-106.
        let spec = parse_flag_spec("-e, --extended_fields");
        assert_eq!(spec.spellings.len(), 2);
        assert_eq!(spec.spellings[1].name, "extended_fields");
        assert!(spec.value_name.is_none());
    }

    #[test]
    fn an_explicit_equals_value_that_begins_with_underscore_stays_a_value() {
        // The shape the underscore widening must not eat: `--foo=_bar`
        // separates name from value with a real `=`, so `_bar` is a
        // value spec, never folded into the flag's own name.
        let spec = parse_flag_spec("--foo=_bar");
        assert_eq!(spec.spellings.len(), 1);
        assert_eq!(spec.spellings[0].name, "foo");
        assert_eq!(spec.value_name.as_deref(), Some("_bar"));
    }

    #[test]
    fn a_leading_underscore_after_dashdash_is_never_a_spelling() {
        // `less --help`'s real `--_<name>` row, byte-exact with the
        // backspace-overstrike underlining it renders with: raw bytes are
        // `--_\x08<_\x08n_\x08a_\x08m_\x08e_\x08>` (an underscore then a
        // backspace ahead of each underlined character). The leading `_`
        // is overstrike, not a spelling: `less` has no option named `--_`.
        // A real long option name never starts with `_`, only its tail
        // may carry one, so `try_long` must refuse this row outright
        // rather than reading a fabricated `--_` flag with a garbled
        // value. See docs/shapes.md S-106.
        let spec = parse_flag_spec("--_\u{8}<_\u{8}n_\u{8}a_\u{8}m_\u{8}e_\u{8}>");
        assert!(spec.spellings.is_empty());
    }

    #[test]
    fn a_dashdash_synopsis_fragment_with_a_trailing_value_still_parses() {
        // cargo fmt's usage line: `[-- <rustfmt_options>...]`.
        let spec = parse_flag_spec("-- <rustfmt_options>...");
        assert_eq!(spec.spellings.len(), 1);
        assert_eq!(spec.spellings[0].name, "--");
        assert_eq!(spec.value_name.as_deref(), Some("<rustfmt_options>"));
    }
}
