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
