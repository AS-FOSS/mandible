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
    let normalized = unwrap_brace_alternation(input.trim());
    let mut rest = normalized.as_ref().trim();
    let mut spec = FlagSpec::default();

    loop {
        // One run of alias spellings: `-p`, `--pid`, `-A, --catenate`.
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
        .map(|(_, _, t)| t)
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

/// `-x` where `x` is any non-separator, non-bracket character, with an
/// optional trailing abbreviation-continuation bracket stripped and
/// discarded (see [`strip_short_abbrev_suffix`]).
fn try_short(input: &str) -> Option<(char, &str)> {
    let mut s = input;
    short_dash(&mut s).ok()?;
    let c = short_char(&mut s).ok()?;
    if let Some(rest) = strip_short_abbrev_suffix(s) {
        s = rest;
    }
    Some((c, s))
}

/// Strips a trailing abbreviation-continuation bracket glued directly onto
/// a short flag character, e.g. `ip --help`'s own `-V[ersion]`,
/// `-s[tatistics]`, `-f[amily]`, `-h[uman-readable]`: the short letter
/// already *is* the flag, and the bracket merely spells out the rest of
/// the word it abbreviates. Mirrors [`crate::help_text::sections::
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
/// glued straight onto a short letter with no `=` at all. A bracket
/// containing `=`, digits, uppercase letters, or anything else therefore
/// falls through untouched to the ordinary value-spec grammar below.
fn strip_short_abbrev_suffix(input: &str) -> Option<&str> {
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
    Some(&rest[close + 1..])
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
    trimmed.starts_with('-') || parse_flag_alternation(trimmed).is_some_and(|alt| alt.open == '{')
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
fn split_alternatives(content: &str) -> Vec<&str> {
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
fn is_bare_flag_spelling(token: &str) -> bool {
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
            assert_eq!(spec.short, Some(short), "{fragment}");
            assert_eq!(spec.long.as_deref(), Some(long), "{fragment}");
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
            assert_eq!(spec.short, Some(short), "{fragment}");
            assert_eq!(spec.long.as_deref(), Some(long), "{fragment}");
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
        assert_eq!(spec.short, Some('l'));
        assert_eq!(spec.long.as_deref(), Some("language"));
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
            assert_eq!(spec.short, None, "{fragment} must not gain a short");
        }
    }

    /// Whitespace alone never resumes an alias run — only an explicit
    /// `,`/`|` does. Without this, two unrelated flags written next to each
    /// other in a synopsis would merge into one.
    #[test]
    fn whitespace_alone_never_resumes_an_alias_run() {
        let spec = parse_flag_spec("--output FILE --other");
        assert_eq!(spec.long.as_deref(), Some("output"));
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
        assert_eq!(spec.short, None);
        assert_eq!(spec.long, None);
    }

    /// The alias run still stops where it always did when nothing follows
    /// the separator but prose or another line.
    #[test]
    fn a_separator_with_no_spelling_after_it_ends_the_run() {
        for fragment in ["--format FMT,", "--format FMT|", "-o, --output FILE"] {
            let spec = parse_flag_spec(fragment);
            assert!(
                spec.value_name.is_some() || spec.long.is_some(),
                "{fragment}"
            );
        }
        // `-o, --output FILE` is the shape that always worked; it must be
        // untouched by any of this.
        let spec = parse_flag_spec("-o, --output FILE");
        assert_eq!(spec.short, Some('o'));
        assert_eq!(spec.long.as_deref(), Some("output"));
        assert_eq!(spec.value_name.as_deref(), Some("FILE"));
        assert!(spec.fully_consumed);
    }

    /// `tar`'s real multi-alias row with a value on each: the first long
    /// name still wins and the placeholder no longer carries the separator.
    #[test]
    fn tars_repeated_long_alias_row_keeps_one_clean_value() {
        let spec = parse_flag_spec("-F, --info-script=NAME, --new-volume-script=NAME");
        assert_eq!(spec.short, Some('F'));
        assert_eq!(spec.long.as_deref(), Some("info-script"));
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
        assert_eq!(spec.short, Some('A'));
        assert_eq!(spec.long.as_deref(), Some("autobackup"));
        assert_eq!(spec.value_name.as_deref(), Some("y|n"));

        let spec =
            parse_flag_spec(bracket_flag_row_content("[ --metadatasize Size[m|UNIT] ]").unwrap());
        assert_eq!(spec.long.as_deref(), Some("metadatasize"));
        assert_eq!(spec.value_name.as_deref(), Some("Size[m|UNIT]"));

        let spec = parse_flag_spec(bracket_flag_row_content("[ -d|--debug ]").unwrap());
        assert_eq!(spec.short, Some('d'));
        assert_eq!(spec.long.as_deref(), Some("debug"));
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
        assert_eq!(spec.short, Some('2'));
        assert_eq!(spec.value_name.as_deref(), Some("CDlNuVv"));
        assert_eq!(spec.value_kind, ValueKind::Required);
    }

    /// `ip --help`'s own abbreviation convention: `-V[ersion]` names the
    /// flag `-V`, with the bracket spelling out the rest of the word it
    /// abbreviates. Before `strip_short_abbrev_suffix` existed, this parsed
    /// as `-V` taking an optional value literally named `"ersion"` — a
    /// value `ip` does not document at all, on a flag that takes none.
    #[test]
    fn short_flag_abbreviation_bracket_is_not_an_invented_value() {
        for (input, letter) in [
            ("-V[ersion]", 'V'),
            ("-s[tatistics]", 's'),
            ("-d[etails]", 'd'),
            ("-f[amily]", 'f'),
            ("-h[uman-readable]", 'h'),
            ("-l[oops]", 'l'),
            ("-a[ll]", 'a'),
            ("-c[olor]", 'c'),
        ] {
            let spec = parse_flag_spec(input);
            assert_eq!(spec.short, Some(letter), "input: {input}");
            assert_eq!(spec.value_name, None, "input: {input} must carry no value");
            assert!(spec.fully_consumed, "input: {input}");
        }
    }

    /// The discriminator is narrow on purpose: a real optional-value
    /// placeholder (upper/mixed case, or carrying its own `=`) must still
    /// parse as a value exactly as before — this change must never widen
    /// into swallowing a genuine optional argument.
    #[test]
    fn short_flag_real_optional_value_is_unaffected_by_abbrev_stripping() {
        let spec = parse_flag_spec("-o[FILE]");
        assert_eq!(spec.short, Some('o'));
        assert_eq!(spec.value_name.as_deref(), Some("FILE"));
        assert_eq!(spec.value_kind, ValueKind::Optional);

        let spec = parse_flag_spec("-x[=WHEN]");
        assert_eq!(spec.short, Some('x'));
        assert_eq!(spec.value_name.as_deref(), Some("WHEN"));
        assert_eq!(spec.value_kind, ValueKind::Optional);
    }
}
