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
/// Entries are distinguished from continuation lines by indentation: a line
/// at or near the block's own baseline indent starts a new entry, while a
/// line indented well past it continues the previous entry's description.
///
/// `allow_dash_separator`: when true, a new-entry line with no 2+-space
/// column gap falls back to splitting on the first ` - ` run instead —
/// apt-get's `"update - Retrieve new lists of packages"` style. The column
/// gap always wins when present. See docs/shapes.md S-053.
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
/// 2+-space column gap first, falling back to a ` - ` separator only when
/// `allow_dash_separator` is set and no column gap was found.
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
/// Some tools lay out two columns of flags rather than flag-and-prose
/// (`awk`'s POSIX short options beside their GNU long equivalents). Reading
/// the second column as a description would fabricate documentation the
/// tool never wrote. Deliberately narrow: only a lone token counts, so a
/// real description that merely starts with a dash (`-1 means unlimited`)
/// is untouched. See docs/shapes.md S-054.
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
/// gap separator that apt-get-style `name - description` listings use. A
/// name's own internal hyphens (`dist-upgrade`) never match, since they
/// have no space on at least one side. See docs/shapes.md S-053.
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

/// Split `line` at a ` - ` separator found by [`find_dash_separator`]: the
/// name is everything before the space preceding the dash, the description
/// everything after the space following it.
pub(super) fn split_at_dash(line: &str, dash_idx: usize) -> (&str, String) {
    let spec = line[..dash_idx].trim_end();
    let desc = line[dash_idx + 1..].trim_start().to_string();
    (spec, desc)
}

/// Find the byte offset of a ` = ` (space-equals-space) entry separator in
/// `line`, if any — [`find_dash_separator`]'s twin for a headed command
/// table using `=` (`wpa_cli`'s `commands:` block). Same reasoning: a
/// token's own internal `=` never matches, since it has no space on at
/// least one side. Distinct from [`find_equals_separator_gap`], which is
/// restricted to flag rows; this one is reached only from
/// [`scan_bare_command_table`]. See docs/shapes.md S-017.
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

/// Split `line` at a ` = ` separator found by [`find_bare_equals_separator_gap`],
/// mirroring [`split_at_dash`] with the separator character substituted.
pub(super) fn split_at_bare_equals(line: &str, eq_idx: usize) -> (&str, String) {
    let name_field = line[..eq_idx].trim_end();
    let desc = line[eq_idx + 1..].trim_start().to_string();
    (name_field, desc)
}

/// Find the byte offset of the first column gap in `line`, if any, after
/// some non-whitespace content — a run of 2+ spaces, or any run containing
/// a tab.
///
/// A tab counts on its own since it already advances at least as far as a
/// two-space run. `mokutil --help` tab-aligns its table and was measured at
/// 38 flags, 0 described, before tabs counted. See docs/shapes.md S-055.
pub(super) fn find_description_gap(line: &str) -> Option<usize> {
    if let Some(col) = find_multi_space_gap(line) {
        return Some(col);
    }
    // Only consulted when the rule above found nothing: a lone `=` token
    // standing in for a column gap. See docs/shapes.md S-057.
    if let Some(col) = find_equals_separator_gap(line) {
        return Some(col);
    }
    // Only consulted when the rules above found nothing: a colon standing
    // in for a column gap, spaced or glued onto the spec. See
    // docs/shapes.md S-003.
    if let Some(col) = find_colon_separator_gap(line) {
        return Some(col);
    }
    // Only consulted when the rules above found nothing. See
    // docs/shapes.md S-059.
    if let Some(col) = find_placeholder_boundary_gap(line) {
        return Some(col);
    }
    // Same "no aligned column anywhere" precondition, one shape further
    // out: no placeholder either, just a sentence. See
    // `find_sentence_start_gap`.
    find_sentence_start_gap(line)
}

/// Push the naive column gap past a second spelling the word `or`
/// introduces (`-h  or  --help`), so the alias is not read as
/// description. Never moves the gap earlier, and only when the word after
/// `or` is a whole spelling standing alone.
/// See docs/shapes.md S-099 and `corpus/vim.basic/audit-seed4/help.txt`.
pub(super) fn extend_gap_past_or_joined_alias(line: &str, naive_gap: usize) -> usize {
    let Some(after) = line.get(naive_gap..) else {
        return naive_gap;
    };
    let after_trimmed = after.trim_start();
    let leading_ws = after.len() - after_trimmed.len();
    let Some(or_stripped) = after_trimmed.strip_prefix("or") else {
        return naive_gap;
    };
    if !or_stripped.starts_with(|c: char| c.is_ascii_whitespace()) {
        return naive_gap;
    }
    let second = or_stripped.trim_start();
    let mid_ws = or_stripped.len() - second.len();
    let tok2 = first_word(second);
    if tok2.is_empty() || !tok2.starts_with('-') || !is_flag_shaped(tok2) {
        return naive_gap;
    }
    let tail = &second[tok2.len()..];
    if !tail.is_empty() && !tail.starts_with('\t') && !tail.starts_with("  ") {
        return naive_gap;
    }
    naive_gap + leading_ws + "or".len() + mid_ws + tok2.len()
}

/// Second fallback for a flag row with no aligned column at all: the
/// description starts one space after the spec, recognizable because it
/// starts an English sentence rather than naming a value.
///
/// The shape is what a long flag name does to a fixed-width table — the
/// name overruns the description column and the formatter emits one space
/// instead of padding:
///
/// ```text
///   --md5 Control MD5 generation                    (apt-ftparchive)
/// ```
///
/// Without this, the whole line is handed to the grammar as the spec, and
/// its `VALUE` arm takes the first prose word as a `value_name`, discarding
/// the rest of the sentence. See docs/shapes.md S-031.
///
/// **Only ever consulted when both gap-finders above found nothing
/// anywhere in the line**, so no already-working split can move.
///
/// Three conditions, all required, since the inverse case — a real ` VALUE`
/// spec (`--class-path PATH`, `--release 7|8|9`) — must keep parsing as a
/// value:
///
/// 1. The line starts with a flag (`-`). Bare-word blocks never reach this.
/// 2. The candidate token [`starts_a_sentence`]: initial uppercase then
///    only lowercase (`Control`, `Enable`). Every measured placeholder
///    shape fails that test.
/// 3. At least one more word follows it, so a lone trailing token stays
///    read as a value.
///
/// Scanning stops at the first token that is neither sentence-shaped nor
/// [`is_value_spec_token`], so a lowercase metavar ends the search rather
/// than letting a capitalized word deeper in the line become a false
/// boundary.
pub(super) fn find_sentence_start_gap(line: &str) -> Option<usize> {
    if !line.trim_start().starts_with('-') {
        return None;
    }
    let bytes = line.as_bytes();
    let mut i = 0usize;
    let mut token_count = 0usize;
    let mut previous_token_end: Option<usize> = None;
    // A spec that already carries its own value (`--init-command=name`)
    // cannot take another one, so the boundary is fixed at that first
    // token and every later word is description, even a value-shaped one
    // — `mariadb`'s own `SQL Command to execute` must not be dropped. See
    // docs/shapes.md S-031.
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
/// than as a value placeholder: an uppercase letter followed by at least
/// one, and nothing but, lowercase letters. All-caps (`PATH`), mixed
/// (`MD5`) and punctuated (`7|8|9`) tokens are excluded by construction.
/// See docs/shapes.md S-031.
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

/// True if `token` can plausibly still be part of a flag spec rather than
/// its description — used by [`find_sentence_start_gap`] to decide how far
/// it may keep looking for a sentence boundary: a leading `-`, notation
/// punctuation, a digit or `-`/`_`/`.` inside the word, or an all-caps run.
/// A bare all-lowercase word does not qualify, which is what stops a
/// capitalized word mid-description from being mistaken for its start.
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
/// Named rather than left as a literal `2` because a smaller gap put a
/// whole shape out of reach: `jdeprscan`'s `  -l    --list` writes its two
/// spellings four spaces apart, so the long form arrived as a description.
/// [`spelling_run`] now recovers that shape by reading a second cell that
/// is itself a spelling as the option's alias rather than its description
/// — this constant still marks where the naive splitter cuts. See
/// docs/shapes.md S-082.
pub const MIN_COLUMN_GAP_SPACES: usize = 2;

/// The original heuristic: a run of two or more spaces, or any run
/// containing a tab, after some non-whitespace content.
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
/// --help all`, spec §6 rule 2b, `corpus/curl/8.5.0-all`) right-pad short
/// specs to a fixed width but run just a single space after a long one:
///
/// ```text
///      --abstract-unix-socket <path> Connect via abstract Unix domain socket
///  -a, --append      Append to target file when uploading
/// ```
///
/// The second row has real padding and [`find_multi_space_gap`] finds it;
/// the first has none, so without this fallback the whole line reads as
/// the flag spec with an empty description. See docs/shapes.md S-056.
///
/// **Only consulted when [`find_multi_space_gap`] found no gap anywhere**,
/// so an already-working split never moves.
///
/// Splits right after the first `>` or `]` that closes a value-placeholder
/// token (`<value>`, `[value]`) when immediately followed by exactly one
/// space and then more content. A `]` that closes a bracket *inside* a
/// placeholder (`<[%]name=...>`) is never mistaken for the boundary: it is
/// never followed by exactly one space, so scanning continues to the
/// placeholder's real closing `>`.
/// Fallback for a flag row with no aligned column, separating spec from
/// description with a lone `=` token instead of whitespace or a dash:
/// `update-xmlcatalog`'s `--verbose = be verbose`. Restricted to flag rows
/// (`-`-led) — a bare-word block using the same shape has no `-` anchor
/// and is deliberately left to [`scan_bare_block`]'s own splitter instead.
/// See docs/shapes.md S-057.
///
/// **Only consulted when [`find_multi_space_gap`] found no gap.** Every
/// token before the `=` must satisfy [`is_value_spec_token`] (`--foo Set X
/// = Y` stops at `Set`), and the candidate token must be exactly `=`,
/// never `=x`/`x=`/`x=y`, with at least one non-whitespace character after
/// it. Returns the `=`'s own byte offset, so [`split_at_column`] keeps it
/// attached to the description side and [`strip_equals_separator`] removes
/// it afterward.
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

/// Strip a flag row description's leading `= ` separator token; otherwise
/// return it unchanged. Serves both [`find_equals_separator_gap`] (which
/// deliberately returns the `=`'s own offset) and an aligned row whose
/// column split lands before the `=` on its own. Only the separator token
/// is removed — a second `=` inside the description proper is text:
/// `update-xmlcatalog`'s `= the root XML catalog (= /etc/xml/catalog)`
/// keeps its own parenthetical `=`. Never applied to a bare-word block's
/// entries, where `name = description` names the value itself.
pub(super) fn strip_equals_separator(desc: &str) -> &str {
    match desc.strip_prefix('=') {
        Some(rest) if rest.starts_with(|c: char| c.is_ascii_whitespace()) => rest.trim_start(),
        _ => desc,
    }
}

/// Fallback for a flag row with no aligned column, no `=`/`:` separator, no
/// bracketed placeholder, and no capitalized sentence-starting description
/// word: an isolated `-` token standing in for a column gap, found the same
/// way [`find_equals_separator_gap`] finds a lone `=`.
///
/// `ar`'s `--target=BFDNAME - specify the target object format as BFDNAME`
/// is the specimen: the value is glued on with `=`, so no bracket exists
/// for [`find_placeholder_boundary_gap`], and the description starts
/// lowercase, so [`find_sentence_start_gap`] never fires either — before
/// this fallback the whole line fell through ungapped and the description
/// was lost. See docs/shapes.md S-058.
///
/// The same [`is_value_spec_token`] gate keeps this from reaching a row
/// shaped `--flag WORD rest of a sentence`: a bare lowercase word fails the
/// gate and the walk gives up before reaching a `-` token.
///
/// **Deliberately not folded into [`find_description_gap`]'s own chain** —
/// that chain's callers must not admit a `-` token as a column at all (a
/// bare-word block's `-` is data), so this finder is tried out of band by
/// [`super::spelling::split_single_column_entry`], only once
/// [`find_description_gap`] has found nothing. The separator it leaves is
/// stripped by [`split_at_column`] itself, same as every other finder.
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
/// [`strip_equals_separator`]/[`strip_colon_separator`]. Only the separator
/// token is removed — a dash elsewhere in the description (a hyphenated
/// word, an em-dash aside) is text and untouched.
pub(super) fn strip_dash_token_separator(desc: &str) -> &str {
    match desc.strip_prefix('-') {
        Some(rest) if rest.starts_with(|c: char| c.is_ascii_whitespace()) => rest.trim_start(),
        _ => desc,
    }
}

/// Fallback for a flag row with no aligned column, that separates spec from
/// description with a colon instead of whitespace, `=`, or a dash:
/// `sg_emc_trespass`'s `-d : output debug` (spaced) and `-hr: Set Honor
/// Reservation bit` (glued). Before this fallback such rows fell into the
/// grammar, whose `VALUE` arm took the colon as a required value, mangling
/// the spelling itself. See docs/shapes.md S-003.
///
/// **Only consulted when [`find_multi_space_gap`] and
/// [`find_equals_separator_gap`] both found nothing.**
///
/// Tighter than [`find_equals_separator_gap`], since a colon is far more
/// common in ordinary prose than a bare `=` (`"(default: long)"`, `12:30`,
/// `http://…`) — admitting every colon would invent a split inside a real
/// sentence. Two shapes, both requiring every token scanned before them to
/// satisfy [`is_value_spec_token`]:
///
/// 1. **Spaced**: a lone `:` token — `-d : output debug`.
/// 2. **Glued**: a token ending in `:` whose remainder is itself
///    [`is_value_spec_token`]-shaped — `-hr:`, `-V:`. A prose word ending a
///    sentence right before a heading colon (`"Options:"`) is refused the
///    same way, since its stripped remainder fails the predicate. A token
///    that merely *contains* a colon without ending in one (`<hh:mm>`,
///    `http://host:port`) never reaches either arm.
///
/// Both shapes require at least one non-whitespace character after the
/// colon — an empty tail (`--flag:`) returns `None`.
///
/// Returns the byte offset of the colon character itself, so
/// [`split_at_column`] keeps it attached to the description side and
/// [`strip_colon_separator`] removes it afterward.
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
/// analog of [`strip_equals_separator`]. Only the separator token is
/// removed — a second `:` inside the description proper
/// (`sg_emc_trespass`'s `(default: long)`) is text and untouched.
pub(super) fn strip_colon_separator(desc: &str) -> &str {
    match desc.strip_prefix(':') {
        Some(rest) if rest.starts_with(|c: char| c.is_ascii_whitespace()) => rest.trim_start(),
        _ => desc,
    }
}

/// Fallback for a flag row with no aligned column, one placeholder notation
/// further out than [`find_multi_space_gap`]: the description starts one
/// space after a spec ending in a bracketed or angle-bracket value
/// (`--size N[bcwkMG]`, `--path <manifest-path>`), recognized by the
/// boundary the closing `]`/`>` itself draws.
///
/// **The line must start with a flag (`-`)** — without the check, a
/// bare-word block's bracketed *operand* placeholder inside a ` - `
/// -separated row (`llvm-ar`'s `d - delete [files] from the archive`) is
/// misread as this row's own value boundary, dropping the entry. See
/// docs/shapes.md S-059.
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
            // A lone `-` token opening the description side is the
            // table's own column separator, whichever finder located the
            // column: `ar` writes ` - ` on every row of its tables. See
            // `strip_dash_token_separator` and docs/shapes.md S-058.
            let desc = strip_dash_token_separator(line[col..].trim_start()).to_string();
            (spec, desc)
        }
        _ => (line.trim(), String::new()),
    }
}

/// Usage-synopsis tokens that stand in for the tool's own option list
/// rather than naming an operand, matched case-insensitively after the
/// notation wrapper is stripped: `tar [OPTION...] [FILE]...`,
/// `vim [arguments] [file ..]`. Only the second token in each pair is a
/// real argument. See docs/shapes.md S-060.
///
/// `args`/`arg` are deliberately absent: `git`'s `[<args>]` and `sh -c
/// command_string [args]` use it as a genuine operand.
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

    /// `apt-get --help`'s real subcommands sit under `"Most used
    /// commands:"` in single-space `name - description` form. See
    /// docs/shapes.md S-053.
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
    /// heading) must never manufacture a subcommand ([M-10]).
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

    /// A tab-aligned entry table is a table. See docs/shapes.md S-055.
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

    /// The single-space fallback (`corpus/curl/8.5.0-all`). See
    /// docs/shapes.md S-056.
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
    /// off of — the shape a long flag name forces on a fixed-width table.
    /// See docs/shapes.md S-031.
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

    /// The inverse case: a genuine ` VALUE` spec must keep parsing as a
    /// value (`jdeprscan`'s uppercase `PATH`, `cargo-fmt`'s
    /// `<manifest-path>`), including a lowercase metavar followed by a
    /// capitalized word deeper in the line, which must not be split there.
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

    /// A spec that already carries its own value cannot take another one:
    /// `mariadb`'s `--init-command=name SQL Command to execute ...` must
    /// keep the word "SQL".
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
    /// exists.
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

    /// A closing `]` that sits inside a placeholder must never be mistaken
    /// for the real boundary.
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

    /// A bare `= ` separator with single spacing (`update-xmlcatalog`,
    /// `wpa_supplicant`). See docs/shapes.md S-057.
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

    /// Same shape, short-flag form (`wpa_supplicant`'s `options:` block).
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

    /// The column *is* aligned (`update-xmlcatalog`'s `With:` block), but
    /// the description keeps its leading `= `; stripped after the split
    /// rather than changing where the split happens.
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

    /// Only the separator `=` is stripped; a second `=` inside the
    /// description proper survives verbatim.
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

    /// `--flag =` with nothing after the separator invents no description.
    #[test]
    fn an_equals_separator_with_an_empty_tail_invents_no_description() {
        let help = "Options:\n  --flag =\n";
        let parsed = parse(help);
        let flag = flag_named(&parsed, "flag");
        assert_eq!(flag.description, None);
    }

    /// Inverse case: `=` deep inside a description, not a separator at
    /// all. A real aligned column gap exists before any `=`, so
    /// `find_multi_space_gap` must keep winning.
    #[test]
    fn equals_signs_inside_a_sentence_are_not_mistaken_for_a_separator() {
        // `-http_seekable` is one of the underscored single-dash long
        // options `repair_single_dash_long_options` recovers; what matters
        // here is orthogonal: the `=` signs inside the sentence must not
        // be mistaken for the glued `=value` separator.
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

    /// Inverse case: a huge aligned gap, then a ` - ` dash separator; the
    /// `=` inside `(default = off)` must not move the cut.
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

    /// Inverse case: an aligned multi-column row whose description itself
    /// contains `=`.
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

    /// Inverse case: a piped-alias row's description containing `=`.
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

    /// Inverse case: the `=` sits inside the spec's own bracket notation,
    /// never a standalone token.
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
    /// (`corpus`/queue-capture `man/0.stdout`) sits far past
    /// `ENTRY_INDENT_TOLERANCE` of its entry row, so `scan_flags_block`
    /// reads it as a continuation and never offers it to any gap-finder.
    /// See docs/shapes.md S-061.
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
    /// (`corpus/ar/audit-seed2/help.txt`).
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
        // THE HAZARD (maintainer, round 7): a bare lowercase word fails
        // `is_value_spec_token`, so this never matches.
        assert_eq!(
            find_dash_token_separator_gap("--mode auto - selects mode automatically"),
            None
        );
        // The boundary of that gate, noted rather than fixed here: an
        // ALL-UPPERCASE prose word *does* pass `is_value_spec_token`, a
        // pre-existing exposure `find_equals_separator_gap` and
        // `find_sentence_start_gap` share, not introduced here.
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

    /// `sg_emc_trespass --help`'s real capture, byte-exact, replayed end to
    /// end. See docs/shapes.md S-003.
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
        // still folds them into `-V`'s description.
        assert_eq!(
            v.description.as_ref().map(|t| t.as_str()),
            Some(
                "print version string then exit DEVICE sg or block device (latter in lk 2.6 or lk 3 series) Example: sg_emc_trespass /dev/sda"
            )
        );

        // `-hr` is the two-character switch this help text documents.
        // `Flag::short` can only ever hold one character, and the
        // remaining swallowed text ("r", one character) sits below
        // `repair_single_dash_long_options`'s own `MIN_SWALLOWED_NAME_CHARS`
        // floor (2), the same deliberate ambiguity that leaves `-Ss`,
        // `-ac` and `-it` unmerged elsewhere. What the colon fix buys is
        // that `-hr` no longer carries the punctuation-mangled value
        // `"r:"`.
        let h = flag('h');
        assert_eq!(h.value_name.as_deref(), Some("r"));
        assert_eq!(
            h.description.as_ref().map(|t| t.as_str()),
            Some("Set Honor Reservation bit")
        );
    }
}
