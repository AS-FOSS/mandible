//! Splitting one entry line into its name and its description: the gap
//! finders (column, sentence, `=`, `:`, dash, placeholder boundary) and the
//! separator strippers that run after them.

use super::*;

pub(super) fn non_empty_text(s: &str) -> Option<Text> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(Text::sanitize(t))
    }
}

/// Split a bare-word block's raw lines into `(name_fragment,
/// description_fragment)` pairs, one per entry, folding continuation
/// lines into the preceding entry's description.
///
/// Entries are distinguished from continuation lines by indentation: the
/// block's baseline indent is the minimum indentation among its non-blank
/// lines, and a line at or near that baseline starts a new entry, while a
/// line indented well past it continues the previous entry's description.
///
/// `allow_dash_separator` (spec issue #3): when true, a new-entry line
/// with no 2+-space column gap falls back to splitting on the first
/// ` - ` (space-dash-space) run instead — apt-get's own `"update -
/// Retrieve new lists of packages"` style. The column gap is always tried
/// first and wins when present, so this never changes how a tool that
/// already uses column alignment is read.
pub(super) fn split_entries<'a>(
    block_lines: &[&'a str],
    allow_dash_separator: bool,
) -> Vec<(&'a str, String)> {
    let non_blank: Vec<&&str> = block_lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .collect();
    if non_blank.is_empty() {
        return Vec::new();
    }
    let baseline = non_blank
        .iter()
        .map(|l| leading_whitespace(l))
        .min()
        .unwrap_or(0);

    let mut entries: Vec<(&str, String)> = Vec::new();
    for line in block_lines {
        if line.trim().is_empty() {
            continue;
        }
        let indent = leading_whitespace(line);
        let is_new_entry = indent <= baseline + 1;
        if is_new_entry {
            let (spec, desc) = split_entry_line(line, allow_dash_separator);
            entries.push((spec, desc));
        } else if let Some(last) = entries.last_mut() {
            last.1.push(' ');
            last.1.push_str(line.trim());
        } else {
            // Malformed: a continuation with nothing to continue. Treat
            // as its own (spec-only) entry rather than dropping it.
            entries.push((line.trim(), String::new()));
        }
    }
    entries
}

/// Split one bare-block entry line into `(name, description)`: the usual
/// 2+-space column gap first, falling back to a ` - ` separator (spec
/// issue #3) only when `allow_dash_separator` is set and no column gap
/// was found.
pub(super) fn split_entry_line(line: &str, allow_dash_separator: bool) -> (&str, String) {
    let (spec, description) = split_entry_line_raw(line, allow_dash_separator);
    if is_synonym_not_description(&description) {
        return (spec, String::new());
    }
    (spec, description)
}

/// True if `description` is a bare option spelling rather than prose — a
/// single token beginning with `-`.
///
/// Some tools lay out two *columns of flags* rather than flag-and-prose.
/// `awk --help` is the case that forced this: it prints POSIX short options
/// beside their GNU long equivalents, tab-separated, so reading the second
/// column as a description gives `-f progfile` the "description"
/// `--file=progfile`. That is not a description, it is the same option
/// spelled differently, and asserting it would be the fabrication §1
/// forbids — worse than the honest "no description" the tool actually
/// offers, and it would have been reported as **28 flags, 100% described**.
///
/// Deliberately narrow: only a lone token counts. Real descriptions that
/// merely *start* with a dash (`-1 means unlimited`) have more than one
/// word and are untouched.
pub(super) fn is_synonym_not_description(description: &str) -> bool {
    let trimmed = description.trim();
    trimmed.starts_with('-') && !trimmed.contains(char::is_whitespace)
}

pub(super) fn split_entry_line_raw(line: &str, allow_dash_separator: bool) -> (&str, String) {
    if let Some(col) = find_description_gap(line) {
        return split_at_column(line, Some(col));
    }
    if allow_dash_separator {
        if let Some(idx) = find_dash_separator(line) {
            return split_at_dash(line, idx);
        }
    }
    split_at_column(line, None)
}

/// Find the byte offset of a ` - ` (space-dash-space) entry separator in
/// `line`, if any — the alternative to [`find_description_gap`]'s column-
/// gap separator that apt-get-style `name - description` listings use
/// (spec issue #3). Returns the offset of the dash itself. A name's own
/// internal hyphens (`dist-upgrade`, `apt-get`) never match: they have no
/// space on at least one side, so only a genuine surrounding-space
/// separator is found.
pub(super) fn find_dash_separator(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut i = 1;
    while i + 1 < bytes.len() {
        if bytes[i] == b'-' && bytes[i - 1] == b' ' && bytes[i + 1] == b' ' {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Split `line` at a ` - ` separator found by [`find_dash_separator`]:
/// `dash_idx` is the dash's own byte offset, so the name is everything
/// before the space preceding it and the description is everything after
/// the space following it — the dash and its surrounding spaces are
/// punctuation, never part of either side.
pub(super) fn split_at_dash(line: &str, dash_idx: usize) -> (&str, String) {
    let spec = line[..dash_idx].trim_end();
    let desc = line[dash_idx + 1..].trim_start().to_string();
    (spec, desc)
}

/// Find the byte offset of a ` = ` (space-equals-space) entry separator in
/// `line`, if any — [`find_dash_separator`]'s twin for a headed command
/// table that uses `=` instead of `-` (`wpa_cli`'s `commands:` block:
/// `status [verbose] = get current WPA/EAPOL/EAP status`). Same shape,
/// same reasoning: a token's own internal `=` (`payload=<hex dump of
/// payload>`, `dialog=<token>`) never matches, because it has no space on
/// at least one side, so only a genuine surrounding-space separator is
/// found. Distinct from [`find_equals_separator_gap`], which is
/// deliberately restricted to flag rows and a stricter "every token
/// before `=` is value-spec-shaped" test — see that function's doc
/// comment for why a bare-word block must never reach it. This one is
/// reached only from [`scan_bare_command_table`], itself gated on already
/// being headed for command emission (see that function's own doc
/// comment for the gate).
pub(super) fn find_bare_equals_separator_gap(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut i = 1;
    while i + 1 < bytes.len() {
        if bytes[i] == b'=' && bytes[i - 1] == b' ' && bytes[i + 1] == b' ' {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Split `line` at a ` = ` separator found by [`find_bare_equals_separator_gap`]:
/// `eq_idx` is the `=`'s own byte offset, so the name field is everything
/// before the space preceding it and the description is everything after
/// the space following it — mirrors [`split_at_dash`] exactly, substituting
/// the separator character.
pub(super) fn split_at_bare_equals(line: &str, eq_idx: usize) -> (&str, String) {
    let name_field = line[..eq_idx].trim_end();
    let desc = line[eq_idx + 1..].trim_start().to_string();
    (name_field, desc)
}

/// Find the byte offset of the first column gap in `line`, if any, after
/// some non-whitespace content — a run of 2+ spaces, or any run containing
/// a tab.
///
/// A tab counts on its own because it is never decoration: it advances to
/// the next 8-column stop, so a single one already separates columns by at
/// least as much as the two spaces required of a space run. Ignoring tabs
/// meant a tab-aligned table had no gap at all and every row collapsed to
/// a name with no description. Measured on `mokutil --help`, which writes
/// `  --list-enrolled\t\t\t\tList the enrolled keys`: **38 flags, 0
/// described**, while the descriptions were sitting right there in the
/// output.
pub(super) fn find_description_gap(line: &str) -> Option<usize> {
    if let Some(col) = find_multi_space_gap(line) {
        return Some(col);
    }
    // Only ever consulted when the rule above found nothing anywhere in
    // the line: a lone `=` token standing in for a column gap
    // (`update-xmlcatalog`'s `--verbose = be verbose`, `wpa_supplicant`'s
    // `-b = optional bridge interface name`) — see
    // `find_equals_separator_gap`'s own doc comment.
    if let Some(col) = find_equals_separator_gap(line) {
        return Some(col);
    }
    // Only ever consulted when the rules above found nothing anywhere in
    // the line: a colon standing in for a column gap, either as its own
    // token (`sg_emc_trespass`'s `-d : output debug`) or glued straight
    // onto the spec (`-hr: Set Honor Reservation bit`) — see
    // `find_colon_separator_gap`'s own doc comment.
    if let Some(col) = find_colon_separator_gap(line) {
        return Some(col);
    }
    // Only ever consulted when the rules above found nothing anywhere in
    // the line — see `find_placeholder_boundary_gap`'s own doc comment.
    if let Some(col) = find_placeholder_boundary_gap(line) {
        return Some(col);
    }
    // Same "no aligned column anywhere" precondition, one shape further
    // out: no placeholder either, just a sentence. See
    // `find_sentence_start_gap`.
    find_sentence_start_gap(line)
}

/// Second fallback for a flag row with no aligned column at all: the
/// description simply starts one space after the spec, and it is
/// recognizable because it starts an **English sentence** rather than
/// naming a value.
///
/// The shape is what a long flag name does to a fixed-width option table —
/// the name overruns the description column, and the formatter emits one
/// space instead of the padding it can no longer supply:
///
/// ```text
///   --md5 Control MD5 generation                    (apt-ftparchive)
///   --no-delink Enable delinking debug mode         (apt-ftparchive)
///   --allow-multiple-definition Allow multiple definitions of symbols   (ld)
///   -print-multi-os-directory Display the relative path to OS libraries (gcc)
/// ```
///
/// Without this, `find_description_gap` returns `None`, the *whole line*
/// is handed to `grammar::parse_flag_spec` as the spec, and its
/// ` VALUE` arm takes the first word of the prose as a `value_name` and
/// **discards the rest of the sentence** — `--md5` acquires the value
/// `Control` and the words "MD5 generation" are lost from the tree
/// entirely. Measured on `apt-ftparchive --help`: 9 flags, 2 of them
/// carrying another flag's job in their `value_name` and no description.
///
/// **Only ever consulted when both gap-finders above found nothing
/// anywhere in the line**, exactly as [`find_placeholder_boundary_gap`]
/// is, so no already-working split can move — this only recovers text
/// that would otherwise be dropped.
///
/// The predicate is deliberately narrow, because the inverse case is the
/// whole risk: ` VALUE` is a real and common shape (`--class-path PATH`,
/// `--release 7|8|9|…|17`, `--manifest-path <manifest-path>`), and reading
/// a value name as prose would delete a real field. Three conditions, all
/// required:
///
/// 1. The line starts with a flag (`-`). Bare-word blocks — subcommand
///    tables, enum-value lists — never reach this at all.
/// 2. The candidate token [`starts_a_sentence`]: an initial ASCII
///    uppercase letter followed by nothing but ASCII lowercase ones
///    (`Control`, `Enable`, `Display`). Every value placeholder shape this
///    project has measured fails that test — `PATH` and `MD5` are all-caps,
///    `<path>` and `[=WHEN]` are bracketed, `7|8|9` is punctuated.
/// 3. At least one more word follows it, so a lone trailing token
///    (`-v Verbose`, `--md5 Control`) is still read as a value. A single
///    word is genuinely ambiguous and stays with the pre-existing reading.
///
/// Scanning stops at the first token that is neither sentence-shaped nor
/// [`is_value_spec_token`] — so a *lowercase* metavar ends the search
/// rather than letting a capitalized word deeper in the line become a
/// false boundary (`--opt value do a Thing here` splits nowhere, instead
/// of splitting before `Thing`).
pub(super) fn find_sentence_start_gap(line: &str) -> Option<usize> {
    if !line.trim_start().starts_with('-') {
        return None;
    }
    let bytes = line.as_bytes();
    let mut i = 0usize;
    let mut token_count = 0usize;
    let mut previous_token_end: Option<usize> = None;
    // A spec that already carries its own value (`--init-command=name`,
    // `--color[=WHEN]`) cannot take another one, so the boundary is fixed
    // at that first token and every word after it is description — even a
    // value-shaped one. Without this, `mariadb`'s
    // `--init-command=name SQL Command to execute ...` splits after `SQL`
    // instead, and the word "SQL" is dropped from the tree: the spec keeps
    // its `=name` value (first value wins) and nothing ever reads `SQL`
    // back out. Never discard a word the tool wrote.
    let mut spec_is_closed = false;
    while i < bytes.len() {
        if bytes[i] == b' ' || bytes[i] == b'\t' {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && bytes[i] != b' ' && bytes[i] != b'\t' {
            i += 1;
        }
        // `start` and `i` only ever land on ASCII space/tab boundaries or
        // the ends of the string, so neither can fall inside a multi-byte
        // character — `get` rather than `[..]` regardless (AGENTS.md §2).
        let token = line.get(start..i)?;
        if token_count > 0 {
            let more_words_follow = line
                .get(i..)
                .is_some_and(|rest| rest.split_whitespace().next().is_some());
            if more_words_follow && starts_a_sentence(token) {
                return previous_token_end;
            }
            if !is_value_spec_token(token) {
                return None;
            }
        } else {
            spec_is_closed = token.contains('=') || token.contains('[');
        }
        // The spelling run's own token always sets the boundary; a closed
        // spec then freezes it there.
        if token_count == 0 || !spec_is_closed {
            previous_token_end = Some(i);
        }
        token_count += 1;
    }
    None
}

/// True if `token` reads as the first word of an English sentence rather
/// than as a value placeholder: an ASCII uppercase letter followed by at
/// least one, and nothing but, ASCII lowercase letters.
///
/// Checked against every distinct occurrence of the shape in this box's
/// 2,301 captured `--help` documents: all 108 of them are verbs
/// (`Use`, `Do`, `Disable`, `Allow`, `Print`, `Enable`, `Display`, …) and
/// none is a metavar. All-caps (`PATH`), mixed (`MD5`, `IPv6`) and
/// punctuated (`7|8|9`) tokens are excluded by construction.
pub(super) fn starts_a_sentence(token: &str) -> bool {
    let mut chars = token.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_uppercase() {
        return false;
    }
    let mut saw_rest = false;
    for c in chars {
        if !c.is_ascii_lowercase() {
            return false;
        }
        saw_rest = true;
    }
    saw_rest
}

/// True if `token` can plausibly still be part of a flag *spec* rather
/// than of its description — used by [`find_sentence_start_gap`] to decide
/// how far it may keep looking for a sentence boundary.
///
/// Anything that names a spelling, a placeholder, or a metavar qualifies:
/// a leading `-`, any notation punctuation, a digit or a `-`/`_`/`.`
/// inside the word, or an all-caps run. A bare all-lowercase alphabetic
/// word does *not* — that is where the scan stops, which is what keeps a
/// capitalized word in the middle of a description from being mistaken for
/// its start.
pub(super) fn is_value_spec_token(token: &str) -> bool {
    if token.starts_with('-') {
        return true;
    }
    if token.chars().any(|c| {
        matches!(
            c,
            '<' | '>' | '[' | ']' | '{' | '}' | '(' | ')' | '=' | '|' | ',' | '/' | ':'
        )
    }) {
        return true;
    }
    if token
        .chars()
        .any(|c| c.is_ascii_digit() || c == '-' || c == '_' || c == '.')
    {
        return true;
    }
    token.chars().all(|c| c.is_ascii_uppercase())
}

/// The fewest consecutive spaces that separate a row's columns rather than
/// merely decorating it — the boundary [`find_multi_space_gap`] cuts at.
///
/// Named rather than left as a literal `2` because it is what once put a
/// whole shape out of the flag-spec grammar's reach: `jdeprscan`'s
/// `  -l    --list` writes its two spellings four spaces apart, so the long
/// form arrived as a *description* and no fragment ever named both. A
/// detector that declares that shape out of its scope has to cite this
/// constant to say so structurally (`xtask`'s
/// `detector::Ground::AcrossDescriptionColumn`), and a retyped copy of the
/// value could drift away from the splitter it claims to describe.
///
/// **That shape is now recovered** — see [`spelling_run`], which reads a
/// second cell that is nothing but another spelling as the option's other
/// spelling rather than as its description. This constant still marks the
/// boundary the naive splitter cuts at; it is no longer the end of the
/// story for a row whose second column is itself a spelling.
pub const MIN_COLUMN_GAP_SPACES: usize = 2;

/// The original heuristic, unchanged: a run of two or more spaces, or any
/// run containing a tab, after some non-whitespace content.
pub(super) fn find_multi_space_gap(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut i = 0;
    let mut seen_content = false;
    while i < bytes.len() {
        if bytes[i] == b' ' || bytes[i] == b'\t' {
            let mut j = i;
            let mut had_tab = false;
            while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') {
                had_tab |= bytes[j] == b'\t';
                j += 1;
            }
            if seen_content && (had_tab || j - i >= MIN_COLUMN_GAP_SPACES) {
                return Some(i);
            }
            i = j;
        } else {
            seen_content = true;
            i += 1;
        }
    }
    None
}

/// Fallback for a line with no aligned column at all — some tools (`curl
/// --help all`, spec §6 rule 2b's own fixture, `corpus/curl/8.5.0-all`,
/// is what surfaced this) right-pad *short* specs to a fixed width but
/// simply run a single space after a *long* one:
///
/// ```text
///      --abstract-unix-socket <path> Connect via abstract Unix domain socket
///  -a, --append      Append to target file when uploading
/// ```
///
/// The second row has real column padding and [`find_multi_space_gap`]
/// finds it; the first has none at all, so without this fallback the
/// whole line — placeholder and description together — reads as the flag
/// spec with an empty description, and a real, present description is
/// silently lost (curl's `--help all` measured 25.2% described before this
/// fix, almost entirely from short flags that happened to have padding).
///
/// **Only ever consulted when [`find_multi_space_gap`] found no gap
/// anywhere in the line at all** — every line with a real aligned column
/// keeps taking that path completely unchanged, so this cannot move where
/// an already-working split happens; it only recovers a description that
/// would otherwise be lost entirely.
///
/// Splits right after the first `>` or `]` that closes a value-placeholder
/// -shaped token (`<value>`, `[value]` — the two spellings this project's
/// own flag grammar, `grammar.rs`, already recognizes) when it is
/// immediately followed by exactly one space and then more content.
/// Content-keyed on that closing-bracket shape, never on a tool name —
/// the same discipline every other layout heuristic in this file follows.
/// A `]` that closes a bracket *inside* a placeholder (`<[%]name=...>`)
/// is never mistaken for the boundary: nothing follows it but more of the
/// placeholder, never a single space, so it fails the "immediately
/// followed by exactly one space" test and scanning continues to the
/// placeholder's real closing `>`.
/// Fallback for a flag row with no aligned column at all, that separates
/// spec from description with a lone `=` token instead of whitespace or a
/// dash: `update-xmlcatalog`'s `--verbose = be verbose` and
/// `wpa_supplicant`'s `-b = optional bridge interface name` (spec §6, the
/// `=`-as-separator family). Before this fallback neither line has a 2+
/// space gap anywhere, so the whole row fell into
/// `grammar::parse_flag_spec` and the description was lost outright —
/// measured on `wpa_supplicant`, whose ~28 flags dropped to 9% parsed.
///
/// **Only ever consulted when [`find_multi_space_gap`] found no gap
/// anywhere in the line**, so an aligned row like `--file <file>       =
/// a local filename` keeps taking that path unchanged; this function never
/// even runs for it. That row's leftover `= ` prefix on the description is
/// [`strip_equals_separator`]'s job, not this one's.
///
/// Restricted to flag rows (`line.trim_start()` starts with `-`) — a
/// bare-word block using the same `name = description` shape
/// (`wpa_supplicant`'s `drivers:` block, `nl80211 = Linux
/// nl80211/cfg80211`) has no `-` anchor to key off of and is deliberately
/// left alone: that block never reaches [`find_description_gap`] through
/// this path anyway ([`scan_bare_block`] uses a different splitter), but
/// the guard also protects the (untested) day this function gets reused
/// from a different call site.
///
/// Scans tokens left to right. Every token before the `=` must satisfy
/// [`is_value_spec_token`] — the same predicate [`find_sentence_start_gap`]
/// uses to tell a still-open spec from prose — so `--file <file> = ...`
/// qualifies (`<file>` is a placeholder) but `--foo Set X = Y` does not:
/// `Set` is a bare lowercase-free word that stops the scan, and the
/// function returns `None` rather than treating `Set`'s `=` as the
/// separator. The candidate token itself must be **exactly** `=` — never
/// `=x`, `x=`, or `x=y` — which the whitespace tokenizer guarantees is
/// already surrounded by whitespace on both sides. Real in-spec and
/// in-description `=` usage (`--opt=VALUE`, `ffplay`'s "0 = disable, 1 =
/// enable", `bugpoint-18`'s "(default = off)") never reaches this function
/// bare: either `find_multi_space_gap` already cut the line, or the `=`
/// sits inside a token rather than standing alone as one.
///
/// Requires at least one non-whitespace character after the `=` — an
/// empty tail (`--flag =`) returns `None`, leaving today's behaviour (no
/// description) unchanged.
///
/// Returns the byte offset of the `=` character itself, so
/// [`split_at_column`] yields `spec = "--verbose"` and
/// `desc = "= be verbose"` — the leading `= ` is stripped afterward by
/// [`strip_equals_separator`], the same function symptom 2 uses.
pub(super) fn find_equals_separator_gap(line: &str) -> Option<usize> {
    if !line.trim_start().starts_with('-') {
        return None;
    }
    let bytes = line.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b' ' || bytes[i] == b'\t' {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && bytes[i] != b' ' && bytes[i] != b'\t' {
            i += 1;
        }
        // `start` and `i` only ever land on ASCII space/tab boundaries or
        // the ends of the string, so neither can fall inside a multi-byte
        // character — `get` rather than `[..]` regardless (AGENTS.md §2).
        let token = line.get(start..i)?;
        if token == "=" {
            let tail_has_content = line.get(i..).is_some_and(|rest| !rest.trim().is_empty());
            return if tail_has_content { Some(start) } else { None };
        }
        if !is_value_spec_token(token) {
            return None;
        }
    }
    None
}

/// Strip a flag row description's leading `= ` separator token: if `desc`
/// starts with `=` immediately followed by ASCII whitespace, drop the `=`
/// and that whitespace; otherwise return it unchanged.
///
/// Two call sites need this, both symptoms of the same `=`-as-separator
/// layout: [`find_equals_separator_gap`] deliberately returns the `=`
/// character's own offset (so [`split_at_column`] keeps the separator
/// attached to whichever side already worked), and a row whose column
/// *is* aligned (`--file <file>       = a local filename`) already splits
/// correctly at the space run **before** the `=` — [`find_multi_space_gap`]
/// gets there first — leaving the same leading `= ` on the description.
/// One strip serves both paths.
///
/// Only the separator token is removed. A second `=` inside the
/// description proper is text, not punctuation, and is left alone:
/// `--root              = the root XML catalog (= /etc/xml/catalog)`
/// strips to `the root XML catalog (= /etc/xml/catalog)`, keeping the
/// parenthetical's own `=`.
///
/// Apply this only where a flag row's description is produced — never to
/// a bare-word block's entries, where `name = description` names the
/// value itself (`wpa_supplicant`'s `drivers:` block) and stripping would
/// fabricate a different name.
pub(super) fn strip_equals_separator(desc: &str) -> &str {
    match desc.strip_prefix('=') {
        Some(rest) if rest.starts_with(|c: char| c.is_ascii_whitespace()) => rest.trim_start(),
        _ => desc,
    }
}

/// Fallback for a flag row with no aligned column, no `=`/`:` separator
/// token, no bracketed placeholder, and no capitalized sentence-starting
/// description word: an isolated `-` token standing in for a column gap,
/// found the identical way [`find_equals_separator_gap`] finds a lone `=`
/// — walk whitespace-delimited tokens, requiring every one before the
/// separator to be spec-shaped ([`is_value_spec_token`]).
///
/// `ar`'s own `--target=BFDNAME - specify the target object format as
/// BFDNAME` is the specimen: the value is glued on with `=`, so no
/// bracket exists for [`find_placeholder_boundary_gap`] to anchor on, and
/// the description starts with a lowercase word ("specify"), so
/// [`find_sentence_start_gap`]'s capitalized-sentence check never fires
/// either — before this fallback, the entire line (spec and description
/// both) fell through as one ungapped spec to
/// [`super::super::grammar::parse_flag_spec`], which still recovered the
/// right `value_name` from the leading `--target=BFDNAME` token but threw
/// the rest of the sentence away with nowhere to land it, and `ar`
/// reported `--target`/`--output` as flags with no description at all
/// despite documenting one.
///
/// The same [`is_value_spec_token`] gate [`find_sentence_start_gap`]
/// already uses is what keeps this from reaching a row shaped `--flag
/// WORD rest of a sentence`, where `WORD` is the first word of prose
/// rather than a value: a bare lowercase word fails `is_value_spec_token`
/// and the walk gives up before ever reaching a `-` token, exactly as it
/// already does for [`find_equals_separator_gap`]/
/// [`find_sentence_start_gap`] today.
///
/// **Deliberately not folded into [`find_description_gap`]'s own chain.**
/// That chain is consulted by callers that must not admit a `-` token as
/// a column at all (a bare-word block's `-` is data), so this finder is
/// tried out of band, only once [`find_description_gap`] has found nothing
/// — see [`super::spelling::split_single_column_entry`]'s call site. The
/// separator it leaves attached to the description is stripped by
/// [`split_at_column`] itself, the same way for every finder: `ar`'s
/// aligned `--thin       - make a thin archive` and its overrun
/// `--target=BFDNAME - specify …` are rows of one table and render the
/// same, dash gone from both.
pub(super) fn find_dash_token_separator_gap(line: &str) -> Option<usize> {
    if !line.trim_start().starts_with('-') {
        return None;
    }
    let bytes = line.as_bytes();
    let mut i = 0usize;
    let mut token_count = 0usize;
    while i < bytes.len() {
        if bytes[i] == b' ' || bytes[i] == b'\t' {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && bytes[i] != b' ' && bytes[i] != b'\t' {
            i += 1;
        }
        // `start` and `i` only ever land on ASCII space/tab boundaries or
        // the ends of the string, so neither can fall inside a multi-byte
        // character — `get` rather than `[..]` regardless (AGENTS.md §2).
        let token = line.get(start..i)?;
        if token == "-" && token_count > 0 {
            let tail_has_content = line.get(i..).is_some_and(|rest| !rest.trim().is_empty());
            return if tail_has_content { Some(start) } else { None };
        }
        if !is_value_spec_token(token) {
            return None;
        }
        token_count += 1;
    }
    None
}

/// Strip a flag row description's leading `- ` separator token, the
/// [`find_dash_token_separator_gap`] counterpart of
/// [`strip_equals_separator`]/[`strip_colon_separator`]: that gap finder
/// deliberately returns the `-` character's own offset, so
/// [`split_at_column`] keeps it attached to the description side.
///
/// Only the separator token is removed. A dash elsewhere in the
/// description — an em-dash-style aside, a hyphenated word — is text, not
/// punctuation, and is untouched: `strip_dash_token_separator` only ever
/// matches a leading `-` immediately followed by whitespace.
pub(super) fn strip_dash_token_separator(desc: &str) -> &str {
    match desc.strip_prefix('-') {
        Some(rest) if rest.starts_with(|c: char| c.is_ascii_whitespace()) => rest.trim_start(),
        _ => desc,
    }
}

/// Fallback for a flag row with no aligned column at all, that separates
/// spec from description with a **colon** instead of whitespace, `=`, or a
/// dash: `sg_emc_trespass`'s real `-d : output debug` (spaced) and
/// `-hr: Set Honor Reservation bit` / `-V: print version string then exit`
/// (glued, no space at all between the spec and the colon). Before this
/// fallback none of those rows have a 2+ space gap or a lone `=` token
/// anywhere, so the whole row fell into `grammar::parse_flag_spec`, whose
/// ` VALUE` arm took the colon (or the colon-terminated remainder of the
/// spec) as a required value — `-d` acquired the fabricated value `":"`,
/// and `-hr` was split into a fabricated `-h` carrying the value `"r:"`,
/// destroying the genuine two-character switch entirely.
///
/// **Only ever consulted when [`find_multi_space_gap`] and
/// [`find_equals_separator_gap`] both found no gap anywhere in the
/// line**, so any already-aligned table (or one using `=` as its
/// separator) keeps taking that path completely unchanged; this function
/// never even runs for it.
///
/// This is [`find_equals_separator_gap`]'s direct sibling, and it has to be
/// **tighter**, not looser: a colon is far more common in ordinary English
/// prose than a bare `=` ever is (`"(default: long)"`, `"Notes: see
/// below"`, `12:30`, `http://…`), so admitting every colon in the line
/// would read straight through a real sentence and invent a split inside
/// it. Two shapes are recognized, both requiring every token scanned
/// before them to satisfy [`is_value_spec_token`] — exactly the guard
/// [`find_equals_separator_gap`] uses, so prose can never be reached:
///
/// 1. **Spaced**: a lone `:` token, standing alone between whitespace —
///    `-d : output debug`. Identical in shape to the equals rule's lone
///    `=` token.
/// 2. **Glued**: a token whose *last* character is `:` and whose remainder
///    (the token with that trailing colon stripped) is itself
///    [`is_value_spec_token`]-shaped — `-hr:`, `-V:`, `-o,` followed by
///    `--output:`. This is the shape the equals rule has no analogue for,
///    because `=VALUE` never gets glued onto the *end* of a spelling the
///    way a colon separator does, and it is the riskier of the two: a
///    prose word that happens to end a sentence right before an inline
///    colon (`Options:`, `Notes:`) also "ends with a colon", so the
///    stripped remainder is checked against the very same predicate that
///    keeps the scan out of prose in the first place — `"Options"` is not
///    [`is_value_spec_token`]-shaped (no dash, no digit, no bracket, not
///    all-uppercase) and is refused, while `"-hr"` and `"-V"` are (they
///    start with `-`) and are accepted. A token that merely *contains* a
///    colon without ending in one (`<hh:mm>`, `0:30`, `http://host:port`)
///    never reaches either arm: it isn't `":"` and doesn't end with `:`,
///    so it is scanned past via the trailing [`is_value_spec_token`] check
///    like any other spec-shaped token, exactly as `find_equals_separator_
///    gap` scans past `<file>` on its way to a real `=`.
///
/// Both shapes require at least one non-whitespace character after the
/// colon — an empty tail (`--flag:`) returns `None`, leaving today's
/// behaviour (no description) unchanged, the same requirement
/// [`find_equals_separator_gap`] makes of `=`.
///
/// Returns the byte offset of the colon character itself (never the
/// character after it), so [`split_at_column`] keeps the separator
/// attached to the description side and [`strip_colon_separator`] removes
/// it afterward — the same two-step [`strip_equals_separator`] already
/// uses for `=`.
pub(super) fn find_colon_separator_gap(line: &str) -> Option<usize> {
    if !line.trim_start().starts_with('-') {
        return None;
    }
    let bytes = line.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b' ' || bytes[i] == b'\t' {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && bytes[i] != b' ' && bytes[i] != b'\t' {
            i += 1;
        }
        // `start` and `i` only ever land on ASCII space/tab boundaries or
        // the ends of the string, so neither can fall inside a multi-byte
        // character — `get` rather than `[..]` regardless (AGENTS.md §2).
        let token = line.get(start..i)?;
        if token == ":" {
            let tail_has_content = line.get(i..).is_some_and(|rest| !rest.trim().is_empty());
            return if tail_has_content { Some(start) } else { None };
        }
        if let Some(head) = token.strip_suffix(':') {
            // A bare `:` with nothing before it in the same token is
            // already handled above; an empty head here would only occur
            // if `token` were exactly `":"`, which can't reach this branch.
            if head.is_empty() || !is_value_spec_token(head) {
                return None;
            }
            let colon_offset = start + head.len();
            let tail_has_content = line.get(i..).is_some_and(|rest| !rest.trim().is_empty());
            return if tail_has_content {
                Some(colon_offset)
            } else {
                None
            };
        }
        if !is_value_spec_token(token) {
            return None;
        }
    }
    None
}

/// Strip a flag row description's leading `: ` separator token, the colon
/// analog of [`strip_equals_separator`]: if `desc` starts with `:`
/// immediately followed by ASCII whitespace, drop the `:` and that
/// whitespace; otherwise return it unchanged.
///
/// [`find_colon_separator_gap`] deliberately returns the `:` character's
/// own offset (so [`split_at_column`] keeps the separator attached to
/// whichever side already worked), leaving it on the front of the
/// description; this removes it.
///
/// Only the separator token is removed. A second `:` inside the
/// description proper is text, not punctuation, and is left alone:
/// `sg_emc_trespass`'s own `Send Short Trespass Command page (default:
/// long) (for FC series)` is untouched — it never had a leading `: ` to
/// begin with, since its separator was the spaced lone-`:` token, already
/// consumed by [`split_at_column`] on the spec side.
pub(super) fn strip_colon_separator(desc: &str) -> &str {
    match desc.strip_prefix(':') {
        Some(rest) if rest.starts_with(|c: char| c.is_ascii_whitespace()) => rest.trim_start(),
        _ => desc,
    }
}

/// Fallback for a flag row with no aligned column at all, one placeholder
/// notation further out than [`find_multi_space_gap`]: the description
/// starts one space after a spec that ends in a bracketed or angle-bracket
/// value (`--size N[bcwkMG]`, `--path <manifest-path>`), recognized by the
/// boundary the closing `]`/`>` itself draws.
///
/// **The line must start with a flag (`-`)**, discovered by the same
/// fabrication [`find_sentence_start_gap`]'s own doc comment (clause 1)
/// already names for its sibling fallback: without the check, a bare-word
/// block's row is read by this too, and a bracketed *operand* placeholder
/// inside an otherwise ` - `-separated command/operation row (llvm-ar's
/// `d - delete [files] from the archive`) is misread as *this* row's own
/// value boundary — splitting `"d - delete [files]"` from `"from the
/// archive"` and handing `emit_subcommands` a name that fails the
/// command-name shape test, dropping the entry outright even though
/// [`find_dash_separator`] would have split it correctly. Found while
/// wiring spec §7 Tier B rule 1's "operations" heading extension through
/// to the fixture it was measured against: `llvm-ar-18`'s `OPERATIONS:`
/// table lost `d`, `m`, `p`, `q`, `r` and `x` (every row naming `[files]`)
/// and kept only `s` and `t` (the two rows with no bracket at all) until
/// this guard was added. [`find_equals_separator_gap`] and
/// [`find_colon_separator_gap`] already carry the identical guard for the
/// identical reason; this was the one fallback in the chain missing it.
pub(super) fn find_placeholder_boundary_gap(line: &str) -> Option<usize> {
    if !line.trim_start().starts_with('-') {
        return None;
    }
    let bytes = line.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b != b'>' && b != b']' {
            continue;
        }
        let after = i + 1;
        if bytes.get(after) != Some(&b' ') {
            continue;
        }
        // Exactly one space: a second space here would mean
        // `find_multi_space_gap` already matched above, so reaching this
        // function at all guarantees no run of 2+ spaces exists anywhere
        // in `line` — no need to re-check that this isn't a longer run.
        let desc_start = after + 1;
        if matches!(bytes.get(desc_start), Some(c) if *c != b' ') {
            return Some(after);
        }
    }
    None
}

pub(super) fn split_at_column(line: &str, col: Option<usize>) -> (&str, String) {
    match col {
        Some(col) if col < line.len() => {
            let spec = line[..col].trim_end();
            // A lone `-` token opening the description side is the table's
            // own column separator, whichever finder located the column:
            // `ar` writes ` - ` on every row of its tables, and the rows
            // whose names overrun the column (`--target=BFDNAME - specify
            // …`) reach the same `- ` through `find_dash_token_separator_gap`
            // while the aligned ones (`--thin       - make a thin archive`)
            // reach it through `find_multi_space_gap`. One table, one rule —
            // stripping it only on the fallback path rendered `--target`
            // without the dash and `--thin` with it, side by side. See
            // `strip_dash_token_separator` for what does and does not count.
            let desc = strip_dash_token_separator(line[col..].trim_start()).to_string();
            (spec, desc)
        }
        _ => (line.trim(), String::new()),
    }
}

/// Usage-synopsis tokens that stand in for the tool's **own option list**
/// rather than naming an operand, matched case-insensitively after the
/// notation wrapper (`<>`, `[]`, `...`) is stripped.
///
/// A synopsis says where the flags go the same way it says where the
/// operands go, and it says it with a word: `tar [OPTION...] [FILE]...`,
/// `pkgconf [OPTIONS] [LIBRARIES]`, `dpkg-statoverride [<option> ...]
/// <command>`, `vim [arguments] [file ..]`. Only the *second* token in each
/// of those pairs is an argument the user supplies; the first is the
/// synopsis pointing at its own options table. Reading it as a positional
/// invents an operand no tool has — the fabrication class spec §7 Tier B
/// forbids, arrived at by a plausible-looking rule rather than by
/// mis-parsing anything.
///
/// **The anchor case is `vim`,** confirmed with the maintainer on
/// 2026-08-13: in `Usage: vim [arguments] [file ..]`, `[file ..]` is a real
/// variadic operand and `[arguments]` is the flag list. Today `arguments`
/// is skipped only incidentally — [`extract_positionals`] happens not to
/// accept bare lowercase words — so widening that rule for any reason at
/// all would silently start fabricating it. Naming the shape here makes the
/// exclusion survive such a change instead of depending on it not happening.
///
/// **`args`/`arg` are deliberately absent.** `git`'s `[<args>]` (the
/// arguments forwarded to the chosen subcommand) and every `sh -c
/// command_string [args]`-shaped synopsis use it as a genuine operand, so
/// excluding it would delete real structure to prevent a defect it does not
/// have. The list holds only words that name an option list and nothing
/// else.
pub(in crate::help_text) const OPTION_LIST_PLACEHOLDERS: &[&str] =
    &["option", "options", "flag", "flags", "arguments"];

/// True when `name` (already unwrapped from its notation) is one of
/// [`OPTION_LIST_PLACEHOLDERS`].
pub fn is_option_list_placeholder(name: &str) -> bool {
    OPTION_LIST_PLACEHOLDERS
        .iter()
        .any(|p| name.eq_ignore_ascii_case(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Issue #3: `apt-get --help`'s real subcommands sit under a
    /// recognized heading (`"Most used commands:"`) in single-space
    /// `name - description` form, e.g. `"update - Retrieve new lists of
    /// packages"`. Before this fix the 2+-space column-gap requirement
    /// (needed to keep the regression above green) meant the whole line
    /// read as one ungapped field, failed the name-shape test, and was
    /// dropped — none of apt-get's real subcommands were recovered at
    /// all. This asserts they now are, *with* their descriptions.
    #[test]
    fn apt_get_dash_separated_commands_are_recovered_with_descriptions() {
        let parsed = parse(APT_GET_HELP);
        let names: Vec<&str> = parsed.subcommands.iter().map(|c| c.name.as_str()).collect();
        for want in [
            "update",
            "upgrade",
            "install",
            "remove",
            "purge",
            "autoremove",
            "dist-upgrade",
            "clean",
            "source",
        ] {
            assert!(names.contains(&want), "expected {want:?} among {names:?}");
        }
        let update = parsed
            .subcommands
            .iter()
            .find(|c| c.name == "update")
            .expect("update recovered");
        assert_eq!(
            update.summary.as_ref().map(|s| s.as_str()),
            Some("Retrieve new lists of packages")
        );
        let dist_upgrade = parsed
            .subcommands
            .iter()
            .find(|c| c.name == "dist-upgrade")
            .expect("dist-upgrade recovered — its own internal hyphen must not be mistaken for the separator");
        assert_eq!(
            dist_upgrade.summary.as_ref().map(|s| s.as_str()),
            Some("Distribution upgrade, see apt-get(8)")
        );
    }

    /// A bare ` - ` inside ordinary prose (not under a recognized command
    /// heading) must never manufacture a subcommand — the exact class of
    /// regression the column-gap rule was originally introduced to stop
    /// (spec [M-10]), just via this separator instead of that one.
    #[test]
    fn dash_separator_is_not_recognized_outside_a_command_heading() {
        let raw = "Usage: widget [OPTIONS]\n\nAbout this tool:\n  widget - a small utility for widgets\n  it does not have a Commands section at all - just prose\n";
        let parsed = parse(raw);
        assert!(
            parsed.subcommands.is_empty(),
            "expected zero subcommands from prose, got {:?}",
            parsed
                .subcommands
                .iter()
                .map(|c| &c.name)
                .collect::<Vec<_>>()
        );
    }

    /// A tab-aligned entry table is a table. `find_description_gap` looked
    /// only for runs of 2+ *spaces*, so a tool separating its columns with
    /// tabs appeared to have no description column at all — measured on
    /// `mokutil --help`: **38 flags, 0 described**, with the descriptions
    /// plainly present in the output.
    #[test]
    fn tab_separated_entries_have_their_descriptions_recovered() {
        let help = "Usage: mokutil [options]\n\nOptions:\n  \
                    --list-enrolled\t\t\tList the enrolled keys\n  \
                    --import\t\t\t\tImport a key\n";
        let parsed = parse(help);
        let described: Vec<(&str, &str)> = parsed
            .flags
            .iter()
            .map(|f| {
                (
                    f.long().unwrap_or(""),
                    f.description.as_ref().map(|d| d.as_str()).unwrap_or(""),
                )
            })
            .collect();
        assert!(
            described.contains(&("list-enrolled", "List the enrolled keys")),
            "{described:?}"
        );
        assert!(
            described.contains(&("import", "Import a key")),
            "{described:?}"
        );
    }

    /// The single-space fallback (spec §6 rule 2b's own fixture,
    /// `corpus/curl/8.5.0-all`, is what surfaced this): a tool that
    /// right-pads *short* specs to a fixed column but simply runs one
    /// space after a *long* one has no aligned column at all for those
    /// rows, so the original 2+-space/tab rule found nothing and the
    /// whole line — placeholder and description together — was read as
    /// the flag spec with an empty description. Measured on real
    /// `curl --help all`: 25.2% described before this fix, 77.1% after.
    #[test]
    fn a_single_space_after_a_value_placeholder_recovers_the_description() {
        let help = "Usage: curl [options...] <url>\n\
                    Options:\n  \
                    --abstract-unix-socket <path> Connect via abstract Unix domain socket\n  \
                    --anyauth     Pick any authentication method\n";
        let parsed = parse(help);
        let described: Vec<(&str, &str)> = parsed
            .flags
            .iter()
            .map(|f| {
                (
                    f.long().unwrap_or(""),
                    f.description.as_ref().map(|d| d.as_str()).unwrap_or(""),
                )
            })
            .collect();
        assert!(
            described.contains(&(
                "abstract-unix-socket",
                "Connect via abstract Unix domain socket"
            )),
            "{described:?}"
        );
        // The ordinary, already-working padded row must be unaffected.
        assert!(
            described.contains(&("anyauth", "Pick any authentication method")),
            "{described:?}"
        );
    }

    /// A description one space after the spec, with no placeholder to key
    /// off of — the shape a long flag name forces on a fixed-width table
    /// when the name overruns the description column. Real
    /// `apt-ftparchive --help`: `--md5` used to arrive carrying
    /// `value_name: Control`, with the words "MD5 generation" discarded
    /// from the tree entirely.
    #[test]
    fn a_sentence_one_space_after_the_spec_is_a_description_not_a_value() {
        let help = "Usage: apt-ftparchive [options] command\n\nOptions:\n  \
                    -h    This help text\n  \
                    --md5 Control MD5 generation\n  \
                    --no-delink Enable delinking debug mode\n  \
                    --contents  Control contents file generation\n";
        let parsed = parse(help);
        let by_long = |name: &str| {
            parsed
                .flags
                .iter()
                .find(|f| f.long() == Some(name))
                .unwrap_or_else(|| panic!("--{name} must be recovered"))
        };
        for (name, text) in [
            ("md5", "Control MD5 generation"),
            ("no-delink", "Enable delinking debug mode"),
        ] {
            let flag = by_long(name);
            assert_eq!(
                flag.description.as_ref().map(|d| d.as_str()),
                Some(text),
                "--{name}"
            );
            assert_eq!(flag.value_name, None, "--{name} takes no value");
            assert_eq!(flag.value_kind, ValueKind::None, "--{name} takes no value");
        }
        // The already-working padded rows must be untouched.
        assert_eq!(
            by_long("contents").description.as_ref().map(|d| d.as_str()),
            Some("Control contents file generation")
        );
        assert_eq!(
            parsed
                .flags
                .iter()
                .find(|f| f.short() == Some('h'))
                .and_then(|f| f.description.as_ref())
                .map(|d| d.as_str()),
            Some("This help text")
        );
    }

    /// The inverse case, and the reason the predicate is what it is: a
    /// genuine ` VALUE` spec must keep parsing as a value. Every shape
    /// here is a real one this project already gets right —
    /// `jdeprscan`'s uppercase `PATH` and pipe-alternation
    /// `7|8|9|…`, `cargo-fmt`'s `<manifest-path>` — plus a lowercase
    /// metavar followed by a capitalized word deeper in the line, which
    /// must not be split at that word.
    #[test]
    fn a_real_value_placeholder_is_never_read_as_a_description() {
        let help = "Usage: tool [options]\n\nOptions:\n  \
                    --class-path PATH\n  \
                    --release 7|8|9|10|11\n  \
                    --manifest-path <manifest-path>\n  \
                    --opt value do a Thing here\n  \
                    --quiet Quiet\n";
        let parsed = parse(help);
        let by_long = |name: &str| {
            parsed
                .flags
                .iter()
                .find(|f| f.long() == Some(name))
                .unwrap_or_else(|| panic!("--{name} must be recovered"))
        };
        for (name, value) in [
            ("class-path", "PATH"),
            ("release", "7|8|9|10|11"),
            ("manifest-path", "<manifest-path>"),
        ] {
            let flag = by_long(name);
            assert_eq!(flag.value_name.as_deref(), Some(value), "--{name}");
            assert_eq!(flag.description, None, "--{name} has no description");
        }
        // A lowercase metavar ends the scan: no split may happen at the
        // capitalized `Thing` three words later.
        assert_eq!(by_long("opt").value_name.as_deref(), Some("value"));
        assert_eq!(by_long("opt").description, None);
        // A lone capitalized trailing token is ambiguous and keeps the
        // pre-existing value reading — one word is not a sentence.
        assert_eq!(by_long("quiet").value_name.as_deref(), Some("Quiet"));
    }

    /// A spec that already carries its own value cannot take another one,
    /// so nothing between it and the sentence may be swallowed as a second
    /// value name and then discarded. Real `mariadb --help`:
    /// `--init-command=name SQL Command to execute ...` must keep the word
    /// "SQL", which the spec has no room for and which nothing downstream
    /// would ever read back out.
    #[test]
    fn a_self_valued_spec_keeps_every_word_of_its_description() {
        let help = "Usage: mariadb [OPTIONS]\n\nOptions:\n  \
                    --init-command=name SQL Command to execute when connecting to server.\n";
        let parsed = parse(help);
        let flag = parsed
            .flags
            .iter()
            .find(|f| f.long() == Some("init-command"))
            .expect("--init-command must be recovered");
        assert_eq!(flag.value_name.as_deref(), Some("name"));
        assert_eq!(
            flag.description.as_ref().map(|d| d.as_str()),
            Some("SQL Command to execute when connecting to server.")
        );
    }

    /// The fallback must never fire when an ordinary aligned gap already
    /// exists — it is consulted only when [`find_multi_space_gap`] finds
    /// nothing anywhere in the line, so a `>`/`]` that happens to sit
    /// inside an already-correctly-split spec (value placeholder) must
    /// not move where the real split lands.
    #[test]
    fn the_single_space_fallback_never_overrides_an_existing_aligned_gap() {
        let help = "Usage: tool [options]\n\nOptions:\n  \
                    -o, --output <file>          Write to file instead of stdout\n";
        let parsed = parse(help);
        let flag = parsed
            .flags
            .iter()
            .find(|f| f.long() == Some("output"))
            .expect("--output must be recovered");
        assert_eq!(
            flag.description.as_ref().map(|d| d.as_str()),
            Some("Write to file instead of stdout")
        );
    }

    /// A closing `]` that sits *inside* a placeholder (`[%]` as part of a
    /// larger `<[%]name=...>` token) must never be mistaken for the real
    /// boundary — nothing follows it but more of the placeholder, never a
    /// single space, so scanning must continue to the placeholder's real
    /// closing `>`.
    #[test]
    fn a_bracket_nested_inside_a_placeholder_is_not_mistaken_for_the_boundary() {
        let help = "Usage: curl [options...] <url>\n\
                    Options:\n  \
                    --variable <[%]name=text/@file> Set variable\n";
        let parsed = parse(help);
        let flag = parsed
            .flags
            .iter()
            .find(|f| f.long() == Some("variable"))
            .expect("--variable must be recovered");
        assert_eq!(
            flag.value_name.as_deref(),
            Some("<[%]name=text/@file>"),
            "the nested `]` must not truncate the placeholder"
        );
        assert_eq!(
            flag.description.as_ref().map(|d| d.as_str()),
            Some("Set variable")
        );
    }

    /// Symptom 1 (`update-xmlcatalog`, `wpa_supplicant`): a bare `= `
    /// separator with only one space on each side has no 2+-space gap
    /// anywhere on the line, so before `find_equals_separator_gap` the
    /// whole row fell into `grammar::parse_flag_spec` and the description
    /// was lost outright. Measured on `wpa_supplicant --help`: "low
    /// confidence: 9% parsed" across its ~28 flags.
    #[test]
    fn a_lone_equals_token_with_single_spacing_recovers_the_description() {
        let help = "Options:\n    \
                    --verbose = be verbose\n    \
                    --sort = sorts the manipulated catalog content\n";
        let parsed = parse(help);
        assert_eq!(
            flag_named(&parsed, "verbose")
                .description
                .as_ref()
                .map(|d| d.as_str()),
            Some("be verbose")
        );
        assert_eq!(
            flag_named(&parsed, "sort")
                .description
                .as_ref()
                .map(|d| d.as_str()),
            Some("sorts the manipulated catalog content")
        );
        for flag in &parsed.flags {
            let desc = flag.description.as_ref().map(|d| d.as_str()).unwrap_or("");
            assert!(!desc.starts_with('='), "separator leaked into: {desc:?}");
        }
    }

    /// Same shape, short-flag form (`wpa_supplicant`'s `options:` block):
    /// `-b = optional bridge interface name`.
    #[test]
    fn a_lone_equals_token_recovers_a_short_flags_description() {
        let help = "options:\n  \
                    -b = optional bridge interface name\n  \
                    -B = run daemon in the background\n";
        let parsed = parse(help);
        let b = parsed
            .flags
            .iter()
            .find(|f| f.short() == Some('b'))
            .expect("-b must be recovered");
        assert_eq!(
            b.description.as_ref().map(|d| d.as_str()),
            Some("optional bridge interface name")
        );
    }

    /// Symptom 2 (`update-xmlcatalog`'s `With:` block): the column *is*
    /// aligned, so `find_multi_space_gap` already cuts correctly, but the
    /// description keeps its leading `= ` (`= a local filename`). This is
    /// `strip_equals_separator`'s job, applied after the split rather than
    /// changing where the split happens.
    #[test]
    fn an_aligned_column_still_strips_its_leading_equals_separator() {
        let help = "With:\n    \
                    --file <file>       = a local filename\n    \
                    --id <id>           = catalog entry idenitifier\n";
        let parsed = parse(help);
        let file = parsed
            .flags
            .iter()
            .find(|f| f.long() == Some("file"))
            .expect("--file must be recovered");
        assert_eq!(
            file.description.as_ref().map(|d| d.as_str()),
            Some("a local filename")
        );
    }

    /// Only the *separator* `=` is stripped; a second `=` inside the
    /// description proper is text and must survive verbatim.
    /// `update-xmlcatalog`: `--root ... = the root XML catalog (=
    /// /etc/xml/catalog)`.
    #[test]
    fn a_second_equals_inside_the_description_is_left_alone() {
        let help = "With:\n    \
                    --root              = the root XML catalog (= /etc/xml/catalog)\n";
        let parsed = parse(help);
        let root = flag_named(&parsed, "root");
        assert_eq!(
            root.description.as_ref().map(|d| d.as_str()),
            Some("the root XML catalog (= /etc/xml/catalog)")
        );
    }

    /// `--flag =` with nothing after the separator keeps today's behaviour
    /// (no description invented from an empty tail).
    #[test]
    fn an_equals_separator_with_an_empty_tail_invents_no_description() {
        let help = "Options:\n  --flag =\n";
        let parsed = parse(help);
        let flag = flag_named(&parsed, "flag");
        assert_eq!(flag.description, None);
    }

    /// Inverse case: `=` deep inside a description, not a separator at
    /// all. `ffprobe --help`/`ffplay --help`:
    /// `-http_seekable <boolean> .D... Use HTTP partial requests, 0 =
    /// disable, 1 = enable, -1 = auto (default auto)`. A real aligned
    /// column gap exists before any `=`, so `find_multi_space_gap` must
    /// keep winning and `find_equals_separator_gap` must never run.
    #[test]
    fn equals_signs_inside_a_sentence_are_not_mistaken_for_a_separator() {
        // `-http_seekable` is one of the underscored single-dash long
        // options `repair_single_dash_long_options` recovers, so the name
        // is whole and the whole value-spec column stays in the
        // description. What matters *here* is orthogonal to both: the `=`
        // signs inside the sentence must not be mistaken for the glued
        // `=value` separator and cut the row short.
        let help = "Options:\n  \
                    -http_seekable     <boolean>    .D......... Use HTTP partial requests, 0 = disable, 1 = enable, -1 = auto (default auto)\n";
        let parsed = parse(help);
        let flag = parsed
            .flags
            .iter()
            .find(|f| f.long() == Some("http_seekable"))
            .expect("-http_seekable must be recovered as one single-dash long option");
        assert!(flag.single_dash());
        assert_eq!(
            flag.description.as_ref().map(|d| d.as_str()),
            Some("<boolean> .D......... Use HTTP partial requests, 0 = disable, 1 = enable, -1 = auto (default auto)")
        );
    }

    /// Inverse case: `llc-18`/`opt-18`/`bugpoint-18`'s
    /// `--enable-gvn-hoist ... - Enable the GVN hoisting pass (default =
    /// off)`. A huge aligned gap, then a ` - ` dash separator; the `=`
    /// inside `(default = off)` is deep in the description and must not
    /// move the cut. The ` - ` itself is LLVM's column separator, written
    /// on every row of its tables, and is stripped by `split_at_column`
    /// like `ar`'s — the description is the sentence, not the furniture.
    #[test]
    fn equals_signs_after_a_dash_separator_are_left_in_the_description() {
        let help = "Options:\n  \
                    --enable-gvn-hoist                                                    - Enable the GVN hoisting pass (default = off)\n";
        let parsed = parse(help);
        let flag = flag_named(&parsed, "enable-gvn-hoist");
        assert_eq!(
            flag.description.as_ref().map(|d| d.as_str()),
            Some("Enable the GVN hoisting pass (default = off)")
        );
    }

    /// Inverse case: `ntfswipe --help`'s
    /// `-c num   --count num   Number of times to write(default = 1)` —
    /// an aligned multi-column row (two spellings, then description) whose
    /// description itself contains `=`.
    #[test]
    fn equals_signs_in_a_two_column_rows_description_are_untouched() {
        let help = "Options:\n    \
                    -c num   --count num   Number of times to write(default = 1)\n    \
                    -b list  --bytes list  List of values to write(default = 0)\n";
        let parsed = parse(help);
        let flag = flag_named(&parsed, "count");
        let desc = flag.description.as_ref().map(|d| d.as_str()).unwrap_or("");
        assert!(
            desc.contains("(default = 1)"),
            "description lost its own `=`: {desc:?}"
        );
    }

    /// Inverse case: `lvmpolld --help`'s
    /// `-t|--timeout     Time to wait in seconds before shutdown on idle
    /// (missing or 0 = inifinite)`.
    #[test]
    fn equals_signs_in_a_piped_alias_rows_description_are_untouched() {
        let help = "Options:\n   \
                    -t|--timeout     Time to wait in seconds before shutdown on idle (missing or 0 = inifinite)\n";
        let parsed = parse(help);
        let desc = parsed
            .flags
            .iter()
            .find_map(|f| f.description.as_ref().map(|d| d.as_str()))
            .unwrap_or("");
        assert!(
            desc.contains("(missing or 0 = inifinite)"),
            "description lost its own `=`: {desc:?}"
        );
    }

    /// Inverse case: `systemd --help`'s `--dump-core[=BOOL]          Dump
    /// core on crash` — the `=` is inside the spec's own bracket notation,
    /// never a standalone token, and a real aligned gap follows it anyway.
    #[test]
    fn a_bracketed_equals_inside_the_spec_is_not_a_separator() {
        let help = "Options:\n     \
                    --dump-core[=BOOL]          Dump core on crash\n";
        let parsed = parse(help);
        let flag = flag_named(&parsed, "dump-core");
        assert_eq!(flag.value_kind, ValueKind::Optional);
        assert_eq!(
            flag.description.as_ref().map(|d| d.as_str()),
            Some("Dump core on crash")
        );
    }

    /// `man --help`'s deeply-indented continuation line
    /// (`corpus`/queue-capture `man/0.stdout`):
    ///
    /// ```text
    ///   -X, --gxditview[=RESOLUTION]   use groff and display through gxditview
    ///                              (X11):
    ///                              -X = -TX75, -X100 = -TX100, -X100-12 = -TX100-12
    ///   -Z, --ditroff              use groff and force it to produce ditroff
    /// ```
    ///
    /// The `-X = -TX75, ...` line starts with `-` and would qualify for
    /// `find_equals_separator_gap` in isolation, but `scan_flags_block`
    /// never offers it that function: its indent (29) is far past
    /// `-X, --gxditview`'s own entry indent (2) plus
    /// `ENTRY_INDENT_TOLERANCE`, so it is read as a **continuation** line
    /// and appended verbatim to `--gxditview`'s description
    /// (`entries.last_mut()` in `scan_flags_block`) — it never reaches
    /// `split_single_column_entry` or `find_description_gap` at all.
    /// Confirmed unchanged by this fix: the continuation text, `=` signs
    /// included, survives byte-for-byte in the recovered description both
    /// before and after.
    #[test]
    fn mans_deeply_indented_continuation_line_is_unaffected() {
        let help = "Options:\n  \
                    -X, --gxditview[=RESOLUTION]   use groff and display through gxditview\n                             \
                    (X11):\n                             \
                    -X = -TX75, -X100 = -TX100, -X100-12 = -TX100-12\n  \
                    -Z, --ditroff              use groff and force it to produce ditroff\n";
        let parsed = parse(help);
        let gxditview = flag_named(&parsed, "gxditview");
        assert_eq!(
            gxditview.description.as_ref().map(|d| d.as_str()),
            Some(
                "use groff and display through gxditview (X11): -X = -TX75, -X100 = -TX100, -X100-12 = -TX100-12"
            ),
            "the continuation text (and its own `=` signs) must survive verbatim"
        );
    }

    #[test]
    fn find_equals_separator_gap_unit_cases() {
        assert_eq!(
            find_equals_separator_gap("  --verbose = be verbose"),
            Some("  --verbose ".len())
        );
        assert_eq!(
            find_equals_separator_gap("  -b = optional bridge interface name"),
            Some("  -b ".len())
        );
        // No content after the separator: unchanged behaviour.
        assert_eq!(find_equals_separator_gap("  --flag ="), None);
        // A token other than a bare spec/value-spec before the `=` stops
        // the scan (`mariadb`-style prose before an `=`).
        assert_eq!(find_equals_separator_gap("  --foo Set X = Y"), None);
        // Not a flag row at all (no leading `-`): never matched here.
        assert_eq!(
            find_equals_separator_gap("nl80211 = Linux nl80211/cfg80211"),
            None
        );
        // `=` glued to another character is not a lone separator token.
        assert_eq!(find_equals_separator_gap("  --foo =bar"), None);
        assert_eq!(find_equals_separator_gap("  --foo bar= baz"), None);
    }

    #[test]
    fn strip_equals_separator_unit_cases() {
        assert_eq!(strip_equals_separator("= be verbose"), "be verbose");
        assert_eq!(strip_equals_separator("=\tbe verbose"), "be verbose");
        assert_eq!(
            strip_equals_separator("the root XML catalog (= /etc/xml/catalog)"),
            "the root XML catalog (= /etc/xml/catalog)"
        );
        // No leading `=`: unchanged.
        assert_eq!(
            strip_equals_separator("no separator here"),
            "no separator here"
        );
        // `=` not followed by whitespace is not the separator shape.
        assert_eq!(strip_equals_separator("=bar"), "=bar");
        assert_eq!(strip_equals_separator("="), "=");
    }

    /// `ar`'s own real rows, byte-exact from its own capture
    /// (`corpus/ar/audit-seed2/help.txt`): the value is glued on with `=`
    /// (no bracket for `find_placeholder_boundary_gap` to anchor on) and
    /// the description starts lowercase (so `find_sentence_start_gap`'s
    /// capitalized-sentence check never fires either) — exactly the shape
    /// none of the other four fallbacks reach.
    #[test]
    fn find_dash_token_separator_gap_unit_cases() {
        assert_eq!(
            find_dash_token_separator_gap(
                "  --target=BFDNAME - specify the target object format as BFDNAME"
            ),
            Some("  --target=BFDNAME ".len())
        );
        assert_eq!(
            find_dash_token_separator_gap(
                "  --output=DIRNAME - specify the output directory for extraction operations"
            ),
            Some("  --output=DIRNAME ".len())
        );
        // A multi-alias row: every token before the isolated `-` must be
        // spec-shaped, exactly as `find_equals_separator_gap` already
        // requires of its own lone `=` token.
        assert_eq!(
            find_dash_token_separator_gap("-o, --output - the output file"),
            Some("-o, --output ".len())
        );
        // No content after the separator: unchanged behaviour, the same
        // requirement `find_equals_separator_gap` makes of `=`.
        assert_eq!(find_dash_token_separator_gap("  --flag -"), None);
        // Not a flag row at all (no leading `-`): never matched here.
        assert_eq!(
            find_dash_token_separator_gap("nl80211 - Linux nl80211/cfg80211"),
            None
        );
        // `-` glued to another character is not a lone separator token.
        assert_eq!(find_dash_token_separator_gap("  --foo -bar"), None);
        // THE HAZARD (maintainer, round 7): a row shaped `--flag WORD rest
        // of a sentence`, where `WORD` might be read as part of the spec
        // rather than the first word of prose. A bare lowercase word
        // fails `is_value_spec_token` and the walk gives up before ever
        // reaching the `-`, so this never matches — the same gate
        // `find_equals_separator_gap`/`find_sentence_start_gap` already
        // rely on for the identical reason.
        assert_eq!(
            find_dash_token_separator_gap("--mode auto - selects mode automatically"),
            None
        );
        // The boundary of that gate, noted rather than fixed here: an
        // ALL-UPPERCASE prose word *does* pass `is_value_spec_token`
        // (`token.chars().all(|c| c.is_ascii_uppercase())`), so a
        // hypothetical `--flag NOTE this does X - a real dash-description`
        // would still let the walk continue past `NOTE` as if it were a
        // value. This is pre-existing behaviour `is_value_spec_token`
        // already had before this fallback (`find_equals_separator_gap`
        // and `find_sentence_start_gap` share the exact same exposure),
        // not something this change introduces or fixes — see this
        // function's own doc comment.
        assert!(is_value_spec_token("NOTE"));
    }

    #[test]
    fn strip_dash_token_separator_unit_cases() {
        assert_eq!(
            strip_dash_token_separator("- specify the target object format as BFDNAME"),
            "specify the target object format as BFDNAME"
        );
        assert_eq!(
            strip_dash_token_separator("-\tspecify with a tab"),
            "specify with a tab"
        );
        // No leading `-`: unchanged.
        assert_eq!(
            strip_dash_token_separator("no separator here"),
            "no separator here"
        );
        // `-` not followed by whitespace is not the separator shape.
        assert_eq!(strip_dash_token_separator("-bar"), "-bar");
        assert_eq!(strip_dash_token_separator("-"), "-");
        // A dash elsewhere in the description — a hyphenated word or an
        // em-dash-style aside — is text, not punctuation, and untouched.
        assert_eq!(
            strip_dash_token_separator("well-formed input only"),
            "well-formed input only"
        );
    }

    /// `sg_emc_trespass`'s real rows, byte-exact from its own capture: a
    /// spaced lone `:` token and two glued colon-terminated specs.
    #[test]
    fn find_colon_separator_gap_unit_cases() {
        assert_eq!(
            find_colon_separator_gap("-d : output debug"),
            Some("-d ".len())
        );
        assert_eq!(
            find_colon_separator_gap("-hr: Set Honor Reservation bit"),
            Some("-hr".len())
        );
        assert_eq!(
            find_colon_separator_gap("-V: print version string then exit"),
            Some("-V".len())
        );
        // A multi-alias row: the colon may sit on a later token, as long as
        // every token before it is spec-shaped.
        assert_eq!(
            find_colon_separator_gap("-o, --output: the output file"),
            Some("-o, --output".len())
        );
        // No content after the separator: unchanged behaviour, the same
        // requirement `find_equals_separator_gap` makes of `=`.
        assert_eq!(find_colon_separator_gap("--flag:"), None);
        assert_eq!(find_colon_separator_gap("--flag :"), None);
        // Not a flag row at all (no leading `-`): never matched here.
        assert_eq!(
            find_colon_separator_gap("Options: pick one of the below"),
            None
        );
        // A word-shaped token ending in `:` is prose (a heading-like
        // word), not a spec — refused, and the scan stops rather than
        // reading through it for a later colon.
        assert_eq!(
            find_colon_separator_gap("-a, --long Options: description"),
            None
        );
        // A colon *inside* a token, not terminating it, is never read as
        // the separator: `find_equals_separator_gap`'s own placeholder and
        // ratio/URL counter-examples, mirrored for `:`.
        assert_eq!(
            find_colon_separator_gap("--host <hh:mm> descriptive text"),
            None
        );
        assert_eq!(find_colon_separator_gap("--ratio 0:30 more text"), None);
        assert_eq!(
            find_colon_separator_gap("--proxy http://host:port do the thing"),
            None
        );
        // The real description's own inline colon (`(default: long)`) is
        // never reached: the scan already returns at the earlier, genuine
        // separator.
        assert_eq!(
            find_colon_separator_gap(
                "-s : Send Short Trespass Command page (default: long) (for FC series)"
            ),
            Some("-s ".len())
        );
    }

    #[test]
    fn strip_colon_separator_unit_cases() {
        assert_eq!(strip_colon_separator(": output debug"), "output debug");
        assert_eq!(
            strip_colon_separator(": Set Honor Reservation bit"),
            "Set Honor Reservation bit"
        );
        // No leading `:`: unchanged.
        assert_eq!(
            strip_colon_separator("no separator here"),
            "no separator here"
        );
        // A second `:` deeper in the description is text, not punctuation.
        assert_eq!(
            strip_colon_separator("Send Short Trespass Command page (default: long)"),
            "Send Short Trespass Command page (default: long)"
        );
        // `:` not followed by whitespace is not the separator shape.
        assert_eq!(strip_colon_separator(":bar"), ":bar");
        assert_eq!(strip_colon_separator(":"), ":");
    }

    /// `sg_emc_trespass --help`'s real capture, byte-exact, replayed
    /// end to end. Before this fix: `-d`/`-h`/`-s`/`-V` all fabricated a
    /// colon-shaped value (`-h` doubly so, since the colon glue also split
    /// the genuine two-character `-hr` switch into a fabricated `-h`), and
    /// the two prose sentences after the synopsis were folded into it and
    /// mined for three fabricated required positionals (`LUN`, `SP`,
    /// `EMC`). After: every flag is a clean boolean with its real
    /// description, and `DEVICE` is the only positional.
    #[test]
    fn sg_emc_trespasss_colon_rows_and_prose_synopsis_tail_are_recovered() {
        let help = concat!(
            "Unrecognized switch: --help\n",
            "Usage:  sg_emc_trespass [-d] [-hr] [-s] [-V] DEVICE\n",
            "  Change ownership of a LUN from another SP to this one.\n",
            "  EMC CLARiiON CX-/AX-family + FC5300/FC4500/FC4700.\n",
            "    -d : output debug\n",
            "    -hr: Set Honor Reservation bit\n",
            "    -s : Send Short Trespass Command page (default: long)\n",
            "         (for FC series)\n",
            "    -V: print version string then exit\n",
            "     DEVICE   sg or block device (latter in lk 2.6 or lk 3 series)\n",
            "        Example: sg_emc_trespass /dev/sda\n",
        );
        let parsed = parse_named(help, "sg_emc_trespass");

        // No fabricated `LUN`/`SP`/`EMC` operands; `DEVICE` survives as the
        // one real positional the usage line actually names.
        let positional_names: Vec<&str> = parsed
            .positionals
            .iter()
            .map(|p| p.primary_name())
            .collect();
        assert_eq!(positional_names, vec!["DEVICE"], "{positional_names:?}");

        let flag = |short: char| {
            parsed
                .flags
                .iter()
                .find(|f| f.short() == Some(short))
                .unwrap_or_else(|| {
                    panic!(
                        "no flag short=={short:?} in {:?}",
                        parsed
                            .flags
                            .iter()
                            .map(|f| f.spelling())
                            .collect::<Vec<_>>()
                    )
                })
                .clone()
        };

        let d = flag('d');
        assert_eq!(d.value_name, None, "-d must not fabricate a value");
        assert_eq!(
            d.description.as_ref().map(|t| t.as_str()),
            Some("output debug")
        );

        let s = flag('s');
        assert_eq!(s.value_name, None, "-s must not fabricate a value");
        assert_eq!(
            s.description.as_ref().map(|t| t.as_str()),
            Some("Send Short Trespass Command page (default: long) (for FC series)")
        );

        let v = flag('V');
        assert_eq!(v.value_name, None, "-V must not fabricate a value");
        // The `DEVICE` operand row and its trailing example sit one column
        // deeper than `-V`'s own entry indent, so `scan_flags_block`'s
        // (unrelated, pre-existing) indentation-based continuation rule
        // still folds them into `-V`'s description — exactly as the task's
        // own "before" tree already showed for this row. Neither of this
        // fix's two causes touches that rule; asserted here only so a
        // future change to it is forced to notice this fixture.
        assert_eq!(
            v.description.as_ref().map(|t| t.as_str()),
            Some(
                "print version string then exit DEVICE sg or block device (latter in lk 2.6 or lk 3 series) Example: sg_emc_trespass /dev/sda"
            )
        );

        // `-hr` is the two-character switch this help text documents.
        // `Flag::short` can only ever hold one character, and the
        // remaining swallowed text (`"r"`, one character) sits below
        // `repair_single_dash_long_options`'s own `MIN_SWALLOWED_NAME_CHARS`
        // floor (2) — the same deliberate ambiguity that leaves `-Ss`,
        // `-ac` and `-it` unmerged elsewhere in this file, so this must
        // not be special-cased into a merge here either. What the colon
        // fix buys is that `-hr` no longer carries the punctuation-mangled
        // value `"r:"`: `-h`'s value is now the clean `"r"`, and its
        // spelling `-h` plus `-hr` (the fallback
        // `xtask::existence::short_candidates` reconstruction) both occur
        // literally in the raw text, so the fleet existence oracle no
        // longer reports it as invented.
        let h = flag('h');
        assert_eq!(h.value_name.as_deref(), Some("r"));
        assert_eq!(
            h.description.as_ref().map(|t| t.as_str()),
            Some("Set Honor Reservation bit")
        );
    }
}
