//! Flag-row mining: where an option table starts, how one physical line
//! splits into entries (packed rows, BNF alternations, bracket groups),
//! and where a nested entry table interrupts a row's description.

use super::*;

/// True when `line` opens a BNF grammar production — `LABEL := ...`, with
/// or without a leading `where` keyword: iproute2's own convention,
/// `where OBJECT := { address | addrlabel | ... }` and
/// `OPTIONS := { -V[ersion] | ... }`.
///
/// Without this, a column-0 `:=` line reads as ordinary leading prose —
/// `ip`'s and `vdpa`'s entire node `description` used to be exactly this
/// one line. The label before `:=` (after stripping a leading `where`)
/// must be short, plain words, the same shape [`is_section_heading_line`]
/// trusts for an ordinary heading, so a stray `:=` deep in prose is never
/// mistaken for one. See docs/shapes.md S-042.
pub(super) fn looks_like_bnf_production_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    let trimmed = trimmed
        .strip_prefix("where")
        .map_or(trimmed, str::trim_start);
    let Some(op) = trimmed.find(":=") else {
        return false;
    };
    let label = trimmed[..op].trim_end();
    !label.is_empty()
        && label
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ' ' || c == '-')
}

/// Scan a flags block starting at `lines[start]` (already confirmed to
/// look like a flag entry). Returns the index just past the block and the
/// `(spec, description)` pairs recovered.
///
/// Classifies each line as a new entry or a continuation of the previous
/// one (spec §7 Tier B rule 2) using the line's shape combined with how
/// far it's indented relative to the *shallowest* entry seen so far in
/// this block, not a single shared indent floor — real `--help` output
/// routinely mixes two entry depths in one block (a short+long flag at
/// column 2, a long-only flag at column 6). A deeply-indented dash-led
/// continuation (tar's own wrapped `--occurrence` description, which
/// mentions `--delete, --diff, --extract`) still counts as a continuation
/// rather than a new entry. See docs/shapes.md S-045.
///
/// Where a flags block actually begins at or after `start`, or `None` if
/// this section is not a flags block at all.
///
/// Normally that is `start` itself. The exception: a tool that documents a
/// positional as the first row of its options table (`kill --help`'s
/// `<pid> [...]` row, which used to cost the whole table). See
/// docs/shapes.md S-044.
/// True if `line`'s left-hand token can open neither a flag entry nor a
/// command entry — it starts with a character that is neither a flag
/// prefix (`-`, `+`) nor the start of a name (alphanumeric).
///
/// Such a row is structurally undecidable: `[c]`, `[l <text> ]`,
/// `@<file>` and `<pid>` are not flag spellings and not command names, so
/// they carry no evidence about which kind of block they sit in.
pub(super) fn cannot_open_an_entry(line: &str) -> bool {
    match line.trim_start().chars().next() {
        Some(c) => !(c.is_ascii_alphanumeric() || c == '-' || c == '+'),
        None => true,
    }
}

pub(super) fn flags_block_start(lines: &[&str], start: usize) -> Option<usize> {
    /// How many non-flag rows may precede the first flag row.
    const MAX_SKIPPED_LEADING_ROWS: usize = 3;

    // Bounded deliberately, because "look harder for flags" is how
    // fabrication starts. A row is skipped only when it sits at the
    // block's own indent (deeper lines are that row's own description),
    // only MAX_SKIPPED_LEADING_ROWS of them, and there must still be a
    // real `-`-leading row at that same indent — a bare-word command
    // table has no such row and stays unaffected. See docs/shapes.md
    // S-046.
    if looks_like_flag_start(lines[start]) || looks_like_bracket_flag_row(lines[start]) {
        return Some(start);
    }
    let base = leading_whitespace(lines[start]);
    let mut skipped = 0;
    for (offset, line) in lines.iter().enumerate().skip(start + 1) {
        if line.trim().is_empty() {
            return None;
        }
        let indent = leading_whitespace(line);
        if indent > base {
            continue; // the previous row's own wrapped description
        }
        if indent < base {
            return None; // dedented out of the block
        }
        if looks_like_flag_start(line) || looks_like_bracket_flag_row(line) {
            return Some(offset);
        }
        // A row whose left token could not be either kind of entry does
        // not decide what kind of block this is, so it does not spend the
        // budget for finding out — binutils `ar` opens its `generic
        // modifiers:` block with eight such rows before its first real
        // flag. A bare-word command table's rows still charge, so a block
        // of them never becomes a flags block. See docs/shapes.md S-046.
        if cannot_open_an_entry(line) {
            continue;
        }
        skipped += 1;
        if skipped > MAX_SKIPPED_LEADING_ROWS {
            return None;
        }
    }
    None
}

// --- Multi-column option tables (spec §7 Tier B) ------------------------
//
// Some tools (`lsof`, `unzip`, `infocmp`, `zipinfo`) pack two or three
// flag+description pairs onto one physical line. Reading one description
// column per line misattributes the later flags' text onto the first —
// fabricated documentation at full confidence. This section detects the
// shape from the block's own recurring column alignment and splits each
// row into its real per-flag pairs before `emit_flags` sees it. See
// docs/shapes.md S-036.
//
// The vocabulary functions ([`is_flag_shaped`], [`is_flag_char`],
// [`first_word`], [`cells`], [`MIN_COLUMN_RECURRENCE`],
// [`is_value_placeholder_only`]) are `pub` and re-exported so
// `xtask/src/misattribution.rs` imports these rather than restating them —
// a prior restatement drifted and produced 200 of 656 fleet-wide false
// positives. [`fields_in_line`] itself is deliberately **not** shared: the
// misattribution copy is an advisory metric that can under-suppress, this
// one cannot (see its own doc comment). If this splitter's fold rule
// changes, check `misattribution::fields_in_line` by hand — it will not
// change with it.

/// True if `token` opens a new packed entry: a dash immediately followed
/// by an ASCII letter. Narrower than [`looks_like_flag_start`] (which also
/// accepts a bare `-` and a `{...}` alternation) since this is asked of
/// one token, many times per line, rather than a whole line once — a bare
/// trailing `-` or a brace group never opens a second entry mid-line here.
/// See docs/shapes.md S-047.
pub(super) fn token_opens_packed_entry(token: &str) -> bool {
    let mut chars = token.chars();
    matches!(chars.next(), Some('-')) && matches!(chars.next(), Some(c) if c.is_ascii_alphabetic())
}

/// True if `token`, found between two packed entries, reads as the
/// previous entry's own operand rather than as prose — otherwise the line
/// is not this shape at all (see [`try_split_packed_row`]).
/// [`is_value_placeholder_only`] covers a value placeholder cell (`FILE`,
/// `N[bcwkMG]`); `find`'s own `-exec`/`-execdir`/`-ok`/`-okdir` command
/// terminators (`;`, `+`, `{}`) are added on top since none of them is
/// upper-case or bracket-wrapped. See docs/shapes.md S-047.
pub(super) fn token_is_packed_operand(token: &str) -> bool {
    is_value_placeholder_only(token) || matches!(token, ";" | "+" | "{}")
}

/// Split one physical line into the packed `(spelling, operand)` entries
/// it carries — never a description, since this shape has none. Returns
/// `None` the moment a token is neither a new entry's opening dash nor the
/// previous entry's operand: real prose is present and the line is not
/// this shape, so the caller falls back to the ordinary single-column
/// reading rather than guess. A line with only one entry still returns
/// `Some` with one entry; [`block_is_packed_flag_rows`] is what requires
/// at least one line in the block to carry two or more. See
/// docs/shapes.md S-047.
pub(super) fn try_split_packed_row(line: &str) -> Option<Vec<(String, String)>> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    if !tokens.first().is_some_and(|t| token_opens_packed_entry(t)) {
        return None;
    }
    let mut entries: Vec<(String, String)> = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        if !token_opens_packed_entry(tokens[i]) {
            return None;
        }
        let spelling = tokens[i].to_string();
        i += 1;
        let mut operand = String::new();
        while i < tokens.len() && !token_opens_packed_entry(tokens[i]) {
            if !token_is_packed_operand(tokens[i]) {
                return None;
            }
            if !operand.is_empty() {
                operand.push(' ');
            }
            operand.push_str(tokens[i]);
            i += 1;
        }
        entries.push((spelling, operand));
    }
    Some(entries)
}

/// True when `trimmed` is a continuation of a BNF alternation group whose
/// own line leads with the operator instead of a fresh flag spelling —
/// `dcb --help`'s `OPTIONS := [ ... ]` production wraps as
///
/// ```text
///        OPTIONS := [ -V | --Version | -i | --iec | -j | --json
///                   | -N | --Numeric | -p | --pretty
///                   | -s | --statistics | -v | --verbose]
/// ```
///
/// where the second and third lines open on the `|` rather than repeating
/// a flag spelling the way every sibling in this family (`ip`, `vdpa`,
/// `bridge`, `rdma`, `devlink`) does. Without this, a `|`-led line glues
/// as raw, unparsed text onto the previous entry's description instead of
/// becoming the flags it names. Requires flag-shaped content after the
/// `|`, not just the character itself, so an unrelated line starting with
/// `|` (a table border) is not swept in. See docs/shapes.md S-043.
pub(super) fn looks_like_bnf_continuation_row(trimmed: &str) -> bool {
    trimmed
        .strip_prefix('|')
        .is_some_and(|rest| looks_like_flag_start(rest.trim_start()))
}

/// True when `token`, trimmed, is a short flag spelling and nothing else —
/// no abbreviation bracket, no value, no alias of its own
/// ([`is_bare_flag_spelling`]) — and specifically the *short* half of that
/// shape (a long spelling fails the "one character" arm of that predicate
/// on its own).
pub(super) fn is_unadorned_short(token: &str) -> bool {
    let t = token.trim();
    !t.starts_with("--") && is_bare_flag_spelling(t)
}

/// The long-spelling counterpart to [`is_unadorned_short`]: `--name` and
/// nothing else.
pub(super) fn is_unadorned_long(token: &str) -> bool {
    let t = token.trim();
    t.starts_with("--") && is_bare_flag_spelling(t)
}

/// The opening delimiter that would match a given closing one.
pub(super) fn matching_open(close: char) -> char {
    match close {
        '}' => '{',
        ')' => '(',
        _ => '[',
    }
}

/// Trims a trailing closing bracket (`}`/`)`/`]`) that has no opening
/// counterpart earlier in the same segment — the residue of an *enclosing*
/// BNF alternation group's own closer landing on the row's last
/// alternative once the row has been split on `|`: `vdpa`'s `-p[retty] }`
/// and `dcb`'s glued `--verbose]` are the same shape at two distances.
///
/// The no-matching-opener test tells this apart from a bracket that really
/// belongs to the segment: `-b[atch] [filename]`'s trailing `]` closes a
/// `[` earlier in the same segment and is left untouched. See
/// docs/shapes.md S-043.
pub(super) fn strip_trailing_stray_bracket(segment: &str) -> &str {
    let trimmed = segment.trim_end();
    let Some(last) = trimmed.chars().next_back() else {
        return trimmed;
    };
    if !matches!(last, '}' | ')' | ']') {
        return trimmed;
    }
    let before = &trimmed[..trimmed.len() - last.len_utf8()];
    if before.contains(matching_open(last)) {
        trimmed
    } else {
        before.trim_end()
    }
}

/// Read a flag-table row as a BNF alternation group listing several
/// distinct flags on one physical line — iproute2's shared `OPTIONS := {
/// ... }` production, once [`split_shared_heading_row`] has already
/// separated the row from its heading (`ip`, `vdpa`, `bridge`, `rdma`,
/// `devlink`, `dcb`). Returns one `(spec, description)` pair per recovered
/// alternative, description always empty, or `None` when the row does not
/// conform closely enough to split without risking a fabrication. See
/// docs/shapes.md S-043.
///
/// # Two shapes inside one row, told apart by a pairing rule
///
/// `ip` spells a flag's long form as a bracketed suffix glued onto the
/// same token (`-V[ersion]`), so every top-level `|`-segment is already a
/// complete flag. `dcb` spells short and long as two adjacent alternatives
/// (`-V | --Version`) instead — the ordinary alias-list convention
/// [`parse_flag_spec`] reads via a comma, spelled with `|` here. A bare
/// short immediately followed by a bare long
/// ([`is_unadorned_short`]/[`is_unadorned_long`]) is folded back into one
/// alias-list segment first, so `dcb`'s six pairs become six flags rather
/// than twelve fragments.
///
/// # The false-positive guard: every segment must fully, cleanly consume
///
/// A top-level `|`-split alone is not sufficient evidence: `sg_sanitize`'s
/// `--count=OC|-c OC  OC is overwrite count` is one flag with an alias and
/// a shared value, not two — kept together by [`parse_flag_spec`]'s own
/// alias-continuation grammar. Checked per segment after pairing:
///
/// 1. [`looks_like_flag_start`] on the segment alone.
/// 2. `parse_flag_spec` must report `fully_consumed` — `sg_sanitize`'s
///    second segment leaves `"is overwrite count"` unconsumed and fails.
/// 3. A recovered value must not itself start with `-` (`devlink`'s
///    un-piped `-v[erbose] -s[tatistics] -[he]x` tail would otherwise
///    fabricate a value onto `-v`), and must sit at a real whitespace
///    boundary (`ip`'s glued multi-letter abbreviations like `-iec` have
///    none).
/// 4. An unpaired segment may carry only one flag-shaped word — `rdma`'s
///    `-p[retty] -r[aw]` packs two flags into one `|`-segment with a bare
///    space, which `parse_flag_spec`'s alias loop would otherwise silently
///    swallow with no trace, passing `fully_consumed` regardless.
///
/// Any segment failing any check refuses the whole row — never a partial
/// split, which would leave some flags recovered and others glued into
/// whichever segment happened to parse.
pub(super) fn split_bnf_alternation_row(line: &str) -> Option<Vec<(String, String)>> {
    let trimmed = line.trim();
    let raw_segments = split_alternatives(trimmed);
    if raw_segments.len() < 2 {
        return None;
    }
    // Cleaned before the pairing decision below, not after: `dcb`'s last
    // pair (`-v`, `--verbose]`) only reads as a bare short/long pair once
    // the glued group closer is gone.
    let segments: Vec<&str> = raw_segments
        .iter()
        .map(|s| strip_trailing_stray_bracket(s))
        .collect();

    // `(text, was_paired)` — `was_paired` marks a group joined from a bare
    // short and long below (legitimately two flag-shaped words); every
    // other group is one raw `|`-segment and must carry exactly one.
    let mut groups: Vec<(String, bool)> = Vec::new();
    let mut idx = 0;
    while idx < segments.len() {
        let seg = segments[idx];
        if is_unadorned_short(seg)
            && segments
                .get(idx + 1)
                .is_some_and(|next| is_unadorned_long(next))
        {
            groups.push((format!("{} | {}", seg, segments[idx + 1]), true));
            idx += 2;
        } else {
            groups.push((seg.to_string(), false));
            idx += 1;
        }
    }
    if groups.len() < 2 {
        return None;
    }

    let mut entries = Vec::with_capacity(groups.len());
    for (candidate, was_paired) in &groups {
        if !looks_like_flag_start(candidate) {
            return None;
        }
        // See condition 4 in this function's own doc comment: an unpaired
        // group with more than one flag-shaped word is refused rather than
        // quietly losing a real flag.
        if !*was_paired
            && candidate
                .split_whitespace()
                .filter(|w| looks_like_flag_start(w))
                .count()
                > 1
        {
            return None;
        }
        let spec = parse_flag_spec(candidate);
        if !spec.fully_consumed {
            return None;
        }
        if let Some(value) = &spec.value_name {
            if value.starts_with('-') || !candidate.contains(char::is_whitespace) {
                return None;
            }
        }
        entries.push((candidate.clone(), String::new()));
    }
    Some(entries)
}

/// True when every entry row in a flags block splits cleanly via
/// [`try_split_packed_row`] and at least one row actually packs two or
/// more entries — proof this is the dense shape and not an ordinary
/// one-flag-per-line block with no description. Consulted only after
/// [`block_is_multi_column`] and the aligned-spelling check have both
/// declined the block, never in front of either. See docs/shapes.md
/// S-047.
pub(super) fn block_is_packed_flag_rows(entry_lines: &[&str]) -> bool {
    let mut any_multi = false;
    for line in entry_lines {
        match try_split_packed_row(line) {
            Some(entries) => any_multi |= entries.len() >= 2,
            None => return false,
        }
    }
    any_multi
}

/// One raw row within a flags block, before it's split into `(spec,
/// description)` — kept as a whole `&str` because the splitting decision
/// (one column vs. several, see [`block_is_multi_column`]) needs every
/// entry row in the block at once, not just this one.
pub(super) enum FlagsBlockRow<'a> {
    /// Looks like the start of a new flag entry.
    Entry(&'a str),
    /// A `+`/`+<placeholder>` row admitted only by
    /// [`has_flag_shaped_plus_neighbor`] — see docs/shapes.md S-095.
    PlusSigil(&'a str),
    /// A continuation of the previous entry's description (`trim_end`ed
    /// text only — the row's own indentation has already done its job).
    Continuation(&'a str),
}

// --- S-095: the neighbor-gated `+`/`+<placeholder>` option row ---------
//
// `is_flag_shaped`/`looks_like_flag_start` stay deaf to a bare `+`
// (neither function can see neighbouring rows, and a `+` line alone does
// not separate vim's `+`/`+<lnum>` option rows from git-lfs's AsciiDoc
// list-continuation marker or date's `%`-conversion-modifier row). The
// gate lives here instead, in the one pass that already holds the whole
// document's lines: `scan_flags_block` admits the row only beside a
// flag-shaped neighbour, mirroring `xtask::plus_prefixed_option`'s own
// `has_flag_shaped_neighbor` (kept as a separate copy — extract and xtask
// do not share code).

/// True when `token` is a `+`-prefixed option spelling this family
/// claims: bare `+`, or `+` followed by a bracketed placeholder
/// (`+<lnum>`, `+<cmd>`) — never `++` or a token with a real letter
/// straight after the sigil (`+d`, which [`is_flag_shaped`] already
/// reads). See docs/shapes.md S-095.
pub(super) fn is_claimed_plus_token(token: &str) -> bool {
    let Some(rest) = token.strip_prefix('+') else {
        return false;
    };
    rest.is_empty() || rest.starts_with('<')
}

/// True when `line`'s own leading token is flag-shaped evidence for the
/// plus-sigil gate: a real `-`-prefixed flag, the bare `--` marker, or
/// this same family's own claimed `+`-token shape. See docs/shapes.md
/// S-095.
fn plus_neighbor_row_is_flag_shaped(line: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.is_empty() || trimmed == line {
        return false;
    }
    let Some(token) = trimmed.split_whitespace().next() else {
        return false;
    };
    let token = token.trim_end_matches(',');
    if let Some(rest) = token.strip_prefix("--") {
        return rest.is_empty() || rest.chars().next().is_some_and(|c| c.is_ascii_alphabetic());
    }
    if let Some(rest) = token.strip_prefix('-') {
        return rest
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric());
    }
    is_claimed_plus_token(token)
}

/// Whether the nearest non-blank line above or below `lines[i]` is itself
/// flag-shaped — the positive option-table evidence required before a
/// `+`-claimed row (see [`is_claimed_plus_token`]) is read as an entry.
/// Refuses git-lfs's bare `+` (prose neighbours on both sides) and date's
/// `+` row (surrounded by `0`, `^`, `_`, `#` modifier rows, none
/// flag-shaped). See docs/shapes.md S-095.
fn has_flag_shaped_plus_neighbor(lines: &[&str], i: usize) -> bool {
    let above = lines[..i].iter().rev().find(|l| !l.trim().is_empty());
    let below = lines[i + 1..].iter().find(|l| !l.trim().is_empty());
    above.is_some_and(|l| plus_neighbor_row_is_flag_shaped(l))
        || below.is_some_and(|l| plus_neighbor_row_is_flag_shaped(l))
}

/// Parse a neighbor-gated plus-sigil entry's spec text (`"+"`,
/// `"+<lnum>"`, `"+<cmd>, -c <cmd>"`) into a [`FlagSpec`]. Reachable only
/// from a row [`scan_flags_block`] already admitted through
/// [`has_flag_shaped_plus_neighbor`] — never through the ordinary
/// [`parse_flag_spec`]/`try_bare_sigil` grammar, which stays deaf to a
/// bare `+` everywhere else (S-096's own reason: a fabricated `+` alias
/// on `as`'s real `--gstabs+` once came from exactly this kind of
/// unscoped widening). The leading `+` is stripped by hand as the bare
/// sigil spelling; whatever follows (a `<placeholder>` value, a
/// comma-joined alias like `-c <cmd>`) is read by the ordinary grammar,
/// which already knows that shape. See docs/shapes.md S-095.
pub(super) fn parse_plus_sigil_spec(spec_text: &str) -> FlagSpec {
    let trimmed = spec_text.trim();
    let Some(rest) = trimmed.strip_prefix('+') else {
        return FlagSpec::default();
    };
    if !(rest.is_empty() || rest.starts_with('<')) {
        return FlagSpec::default();
    }
    let mut spec = FlagSpec {
        spellings: vec![Spelling::bare("+")],
        ..FlagSpec::default()
    };
    if rest.is_empty() {
        spec.fully_consumed = true;
        return spec;
    }
    let tail = parse_flag_spec(rest);
    spec.value_name = tail.value_name;
    spec.value_kind = tail.value_kind;
    spec.spellings.extend(tail.spellings);
    spec.fully_consumed = tail.fully_consumed;
    spec
}

/// One recovered flag-table row: its spec text, its description, and any
/// enumerated values nested directly under it (`llvm-ar`'s bare `=value`
/// sub-rows, see [`choices_sub_row_value`]; ffmpeg/ffplay's described
/// AVOption sub-rows, see [`choice_description_sub_row`]). A choice
/// entry's second element is `None` for the bare-name shape,
/// `Some(description)` for the described one.
pub(super) type FlagRowEntry = (String, String, Vec<(String, Option<String>)>);

/// The value placeholder when `trimmed` opens an "argfile" row — `jmod`'s
/// `@<filename>`, `ar`'s `@<file>`, `nm`'s `@FILE` — the GNU-binutils/LLVM/
/// JDK convention for splicing a file's contents into argv (spec §4.5).
///
/// The shape: the row's first token (never a later one, which is how
/// `user@host` in a description column is refused) is `@` immediately
/// followed by a bracketed placeholder or an all-uppercase word and
/// nothing else in that token. Returns the placeholder text verbatim, or
/// `None` if `trimmed` does not open this way. Recognized structurally,
/// never by tool name. See docs/shapes.md S-021.
///
/// This is neither a flag nor prose continuing the entry above it, and
/// [`scan_flags_block`] must never fold it into either — before this
/// shape was recognized, an argfile row could corrupt the previous flag's
/// description whenever an earlier same-block row had pulled the block's
/// minimum entry indent down far enough to misclassify it as a
/// continuation.
pub(super) fn argfile_row_value_name(trimmed: &str) -> Option<&str> {
    let first = trimmed.split_whitespace().next()?;
    let rest = first.strip_prefix('@')?;
    if rest.len() > 2 && rest.starts_with('<') && rest.ends_with('>') {
        return Some(rest);
    }
    if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_uppercase()) {
        return Some(rest);
    }
    None
}

/// Build the argfile row's own [`FlagRowEntry`] once
/// [`argfile_row_value_name`] has matched: the placeholder text as the
/// "spec" slot (never fed to [`super::super::grammar::parse_flag_spec`] —
/// [`super::emit::emit_argfile_flag`] builds the sigil entity directly)
/// and the row's own column gap, through the same
/// [`super::entry::split_at_column`] every other row uses, so `ar`'s ` - `
/// separator strips the same way here as elsewhere in the same table. See
/// docs/shapes.md S-021.
pub(super) fn argfile_flag_entry(line: &str, value_name: &str) -> FlagRowEntry {
    let (_, desc) = super::entry::split_at_column(line, find_description_gap(line));
    (value_name.to_string(), desc, Vec::new())
}

/// True when `spec`'s own text still carries an unclosed `<...>`
/// placeholder, meaning a wrapped continuation line below it may still
/// belong to the placeholder itself rather than to the description.
///
/// A placeholder can only open at a token boundary — `<` immediately
/// preceded by whitespace or the start of the spec — never glued onto an
/// earlier character. That is what tells jmod's real placeholder open
/// (`--target-platform <String: target-`) apart from `msgcat`'s short flag
/// spelled with the literal character `<` (`-<, --less-than=NUMBER`, whose
/// `<` is glued onto the leading `-` with no token boundary before it). A
/// naive raw `<`-vs-`>` count conflated the two. See docs/shapes.md
/// S-048.
pub(super) fn placeholder_left_open(spec: &str) -> bool {
    let bytes = spec.as_bytes();
    let mut open_idx: Option<usize> = None;
    let mut at_token_start = true;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b' ' || b == b'\t' {
            at_token_start = true;
            continue;
        }
        if at_token_start && b == b'<' {
            open_idx = Some(i);
        }
        at_token_start = false;
    }
    open_idx.is_some_and(|i| !spec[i..].contains('>'))
}

/// True when `trimmed` is `llvm-ar`'s own enumerated-value sub-row shape —
/// `=default            -   default` — nested directly under a flag row
/// (`--format`) to name one of that flag's possible values. Returns the
/// bare value name when the row matches, `None` otherwise.
///
/// Two conditions, both required, so an ordinary wrapped-prose
/// continuation is never mistaken for this: the row opens with `=`
/// immediately followed by a bare word (letters, digits, `-`, `_`), and a
/// real column/dash separator ([`find_description_gap`]) follows it. See
/// docs/shapes.md S-049.
pub(super) fn choices_sub_row_value(trimmed: &str) -> Option<&str> {
    let rest = trimmed.strip_prefix('=')?;
    let end = rest
        .find(|c: char| !(c.is_alphanumeric() || c == '-' || c == '_'))
        .unwrap_or(rest.len());
    if end == 0 {
        return None;
    }
    find_description_gap(trimmed)?;
    Some(&rest[..end])
}

/// True when `trimmed` is ffmpeg/ffplay's own AVOption sub-table shape — a
/// bare constant name, a real column gap, then that value's scope flags and
/// explanation kept verbatim as the choice's own description:
///
/// ```text
///   -flags             <flags>      ED.VAS..... (default 0)
///      unaligned                    .D.V....... allow decoders to produce unaligned output
/// ```
///
/// Returns `(name, description)` when the row matches, `None` otherwise.
/// Two conditions, both required, so an ordinary wrapped-prose continuation
/// is never misread as a described choice: the row opens with a bare
/// [`is_command_name_shaped`] token (no `=` prefix — tried only after
/// [`choices_sub_row_value`] has refused), and a genuine aligned column gap
/// ([`find_multi_space_gap`], deliberately not the full
/// [`find_description_gap`] chain, whose colon/equals/sentence heuristics
/// are for flag rows) separates it from the rest of the row. See
/// docs/shapes.md S-015.
///
/// The scope columns and any numeric value column are the tool's own text
/// and stay verbatim inside the returned description — no parser for them.
pub(super) fn choice_description_sub_row(trimmed: &str) -> Option<(&str, &str)> {
    if trimmed.starts_with('=') {
        return None;
    }
    let gap = find_multi_space_gap(trimmed)?;
    let name = &trimmed[..gap];
    if !is_command_name_shaped(name) {
        return None;
    }
    let desc = trimmed[gap..].trim();
    if desc.is_empty() {
        return None;
    }
    Some((name, desc))
}

pub(super) fn scan_flags_block<'a>(
    lines: &[&'a str],
    start: usize,
    heading_is_bnf: bool,
) -> (
    usize,
    Vec<FlagRowEntry>,
    bool,
    Option<FlagRowEntry>,
    Vec<bool>,
) {
    const ENTRY_INDENT_TOLERANCE: usize = 10;
    let mut i = start;
    let mut rows: Vec<FlagsBlockRow<'a>> = Vec::new();
    let mut min_entry_indent: Option<usize> = None;
    let mut current_entry_line: Option<&'a str> = None;
    // The argfile row (`@<file>`/`@FILE`), captured separately from `rows`
    // — see the `break` below and `argfile_row_value_name`'s doc comment.
    // At most one per block. See docs/shapes.md S-021.
    let mut argfile_entry: Option<FlagRowEntry> = None;

    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() {
            i += 1;
            continue;
        }
        let indent = leading_whitespace(line);
        let trimmed = line.trim_start();

        // An argfile row is neither a flag entry nor a continuation of the
        // one above it (see `argfile_row_value_name`). Ending the block
        // here re-routes rather than drops: the caller resumes its scan at
        // this line, and the row is captured into `argfile_entry`, outside
        // `rows`, so it can't be misread as a continuation or feed the
        // block's packed/multi-column decisions below. See docs/shapes.md
        // S-021.
        if let Some(value_name) = argfile_row_value_name(trimmed) {
            argfile_entry = Some(argfile_flag_entry(line, value_name));
            break;
        }

        let is_entry_start = (looks_like_flag_start(trimmed)
            || looks_like_bracket_flag_row(trimmed)
            // Gated like `split_bnf_alternation_row`: a leading `|`
            // introduces a BNF continuation (`dcb`'s wrapped `OPTIONS :=`),
            // but a bare `|`-led line is also how `sg_write_x` wraps a
            // single alias onto its own line, where the `|` is a
            // continuation marker, not grammar — reading it as a fresh
            // entry there fabricates a second flag. Only a `:=`-shaped
            // heading may read the leading-`|` shape as anything but an
            // ordinary continuation. See docs/shapes.md S-043.
            || (heading_is_bnf && looks_like_bnf_continuation_row(trimmed)))
            && min_entry_indent.is_none_or(|min| indent <= min + ENTRY_INDENT_TOLERANCE);

        // The neighbor-gated `+`/`+<placeholder>` row (S-095): indented
        // (a heading has none), the claimed shape, and beside a
        // flag-shaped neighbor — see `has_flag_shaped_plus_neighbor`.
        // Checked only once the ordinary shapes above have refused the
        // row, and independently of the indent-tolerance gate above (a
        // block's own plus row may open no more indented than its first
        // real flag).
        let is_plus_sigil_start = !is_entry_start
            && indent > 0
            && is_claimed_plus_token(first_word(trimmed).trim_end_matches(','))
            && min_entry_indent.is_none_or(|min| indent <= min + ENTRY_INDENT_TOLERANCE)
            && has_flag_shaped_plus_neighbor(lines, i);

        if is_entry_start || is_plus_sigil_start {
            rows.push(if is_plus_sigil_start {
                FlagsBlockRow::PlusSigil(line)
            } else {
                FlagsBlockRow::Entry(line)
            });
            min_entry_indent = Some(min_entry_indent.map_or(indent, |m| m.min(indent)));
            current_entry_line = Some(line);
            i += 1;
            continue;
        }

        let is_continuation = !rows.is_empty() && min_entry_indent.is_some_and(|m| indent > m);
        if is_continuation {
            let entry_has_own_description =
                current_entry_line.is_some_and(entry_row_carries_own_description);
            if entry_has_own_description
                && (nested_entry_table_starts_at(lines, i, indent)
                    || (is_section_heading_line(trimmed) && is_ignorable_heading(trimmed))
                    || example_block_starts_at(lines, i))
            {
                break;
            }
            rows.push(FlagsBlockRow::Continuation(trimmed.trim_end()));
            i += 1;
            continue;
        }

        // Neither a new entry nor a continuation of one: this line dedents
        // back to (or below) the block's own entries without looking like
        // a flag — a genuinely new heading. Stop here.
        break;
    }

    // Whether this block packs several flag+description pairs per line
    // (spec §7 Tier B, `lsof`'s options table) is a property of the block,
    // decided once from every entry row together — never per line, which
    // would let an ordinary single-column row get split as if a bare
    // second word were a second flag. See docs/shapes.md S-036.
    let entry_lines: Vec<&str> = rows
        .iter()
        .filter_map(|r| match r {
            FlagsBlockRow::Entry(l) => Some(*l),
            FlagsBlockRow::PlusSigil(_) | FlagsBlockRow::Continuation(_) => None,
        })
        .collect();
    let multi_column = block_is_multi_column(&entry_lines);
    // Independent of, and subordinate to, `multi_column`: a block can pack
    // several flag+description pairs per line (that decision) *or* spell
    // one option across aligned spelling columns (this one). Only a block
    // the first decision did not claim consults the second.
    let aligned_spellings = !multi_column && block_has_aligned_spelling_column(&entry_lines);
    // Subordinate to both of the above, for the reason each of their own
    // doc comments gives for the other: a block already claimed by either
    // shape never reaches the packed-row reader, and the packed reader
    // itself refuses (see `block_is_packed_flag_rows`) rather than compete
    // for a row a working splitter already owns.
    let packed = !multi_column && !aligned_spellings && block_is_packed_flag_rows(&entry_lines);

    let mut entries: Vec<FlagRowEntry> = Vec::new();
    // Parallel to `entries`, one entry per element: `true` where that
    // entry came from a `FlagsBlockRow::PlusSigil` row, so
    // `emit_flags`/`parse_plus_sigil_spec` is the only place that ever
    // reads a bare `+` as a flag spelling — never the ordinary
    // `parse_flag_spec`/`try_bare_sigil` grammar every other entry goes
    // through. See docs/shapes.md S-095.
    let mut is_plus_sigil: Vec<bool> = Vec::new();
    for row in rows {
        let plus_sigil_row = matches!(row, FlagsBlockRow::PlusSigil(_));
        let before = entries.len();
        match row {
            FlagsBlockRow::PlusSigil(line) => {
                let (spec, desc) = split_single_column_entry(line);
                entries.push((spec, desc, Vec::new()));
            }
            FlagsBlockRow::Entry(line) => {
                // A docopt bracket-group row (LVM's `[ -d|--debug ]`) is
                // one flag with no description column at all, so it's read
                // directly rather than through either column splitter
                // below. See docs/shapes.md S-005.
                if let Some(content) = bracket_flag_row_content(line.trim()) {
                    entries.push((content.to_string(), String::new(), Vec::new()));
                    continue;
                }
                // The packed shape (`find --help`'s Tests/Actions tables,
                // see the block comment above `block_is_packed_flag_rows`):
                // several bare entries per line, never a description.
                // `block_is_packed_flag_rows` already proved every entry
                // line splits cleanly, so a `None` here degrades to the
                // single-column path below rather than dropping the row.
                // See docs/shapes.md S-047.
                if packed {
                    if let Some(subs) = try_split_packed_row(line) {
                        entries.extend(subs.into_iter().map(|(s, d)| (s, d, Vec::new())));
                        continue;
                    }
                }
                // A BNF alternation group naming several distinct flags on
                // one line (iproute2's `OPTIONS := { ... }`) — see
                // `split_bnf_alternation_row`. Gated on `heading_is_bnf`
                // (true only when this block's own heading came from the
                // `:=`-operator clause): a bare `|` alone is not sufficient
                // evidence, since `btrfsck`'s `-E|--subvol-extents` and the
                // `lv*`/`vg*`/`pv*` family's `-A|--autobackup y|n` both pass
                // every per-segment check yet are one flag, not this shape.
                // Tried after the bracket-row and packed-row cases and
                // before the ordinary column splitters. See docs/shapes.md
                // S-043.
                if heading_is_bnf {
                    if let Some(alternatives) = split_bnf_alternation_row(line.trim()) {
                        entries.extend(alternatives.into_iter().map(|(s, d)| (s, d, Vec::new())));
                        continue;
                    }
                }
                // `fields_in_line` can come back empty on a line
                // `looks_like_flag_start` accepted but whose leading token
                // isn't `is_flag_shaped` (a stricter, narrower class).
                // Never silently drop the row: fall back to the ordinary
                // single-column split instead.
                let split = multi_column
                    .then(|| fields_in_line(line))
                    .filter(|f| !f.is_empty());
                match split {
                    Some(fields) => {
                        for field in fields {
                            entries.push((
                                field.tokens.join(", "),
                                field.trailing.trim().to_string(),
                                Vec::new(),
                            ));
                        }
                    }
                    None if aligned_spellings => {
                        let (s, d) = split_aligned_spelling_entry(line);
                        entries.push((s, d, Vec::new()));
                    }
                    None => {
                        let (s, d) = split_single_column_entry(line);
                        entries.push((s, d, Vec::new()));
                    }
                }
            }
            FlagsBlockRow::Continuation(text) => {
                if let Some(last) = entries.last_mut() {
                    // A continuation that completes an unclosed `<...>`
                    // placeholder opened on the entry row above (jmod's
                    // `--target-platform <String: target-` / `platform>`)
                    // joins the placeholder itself, not the description —
                    // see `placeholder_left_open`. Joined with no space
                    // inserted, since the wrap lands mid-word right after
                    // the hyphen the source already carries. See
                    // docs/shapes.md S-048.
                    if placeholder_left_open(&last.0) {
                        last.0.push_str(text);
                    } else if let Some((name, desc)) = choice_description_sub_row(text) {
                        // ffmpeg/ffplay's described AVOption sub-rows —
                        // see `choice_description_sub_row`. Tried before
                        // the bare `=name` shape below, which only matches
                        // an `=`-prefixed row this one already refused.
                        // See docs/shapes.md S-015.
                        let name = name.to_string();
                        if !last.2.iter().any(|(n, _)| n == &name) {
                            last.2.push((name, Some(desc.to_string())));
                        }
                    } else if let Some(choice) = choices_sub_row_value(text) {
                        // llvm-ar's own `=value` sub-rows — see
                        // `choices_sub_row_value`. llvm-ar never documents
                        // a per-value explanation on this shape, so there
                        // is nothing to attach here. See docs/shapes.md
                        // S-049.
                        let choice = choice.to_string();
                        if !last.2.iter().any(|(n, _)| n == &choice) {
                            last.2.push((choice, None));
                        }
                    } else {
                        last.1.push(' ');
                        last.1.push_str(text);
                    }
                }
            }
        }
        is_plus_sigil.resize(entries.len(), plus_sigil_row);
        debug_assert!(
            !plus_sigil_row || entries.len() == before + 1,
            "a PlusSigil row must produce exactly one entry"
        );
    }
    (i, entries, packed, argfile_entry, is_plus_sigil)
}

/// The fewest name/description pairs a deeper-indented run must show before
/// [`nested_entry_table_starts_at`] reads it as a table rather than an
/// ordinary wrapped description. Two, same as
/// [`scan_same_indent_entry_table`]'s `MIN_ROWS`: one ragged continuation
/// line must not trip this on its own; only repetition is evidence of a
/// table.
pub(super) const MIN_NESTED_TABLE_ROWS: usize = 2;

/// The fewest consecutive invocation-shaped lines
/// [`example_block_starts_at`] demands before reading a deeper-indented
/// run as a worked-example block rather than an ordinary wrapped
/// description. Two, the same floor [`MIN_NESTED_TABLE_ROWS`] uses, for
/// the same reason: one line that happens to start with `./` is cheap to
/// produce by accident, a run of them is a worked example.
pub(super) const MIN_EXAMPLE_BLOCK_LINES: usize = 2;

/// A leading dash whose next char is not a digit — excludes a bare
/// negative number (`-1`), a choices sub-table's own value column
/// (ffplay's `all -1 ... all`), from [`looks_like_invocation_line`]'s
/// flag check. See docs/shapes.md S-126.
fn is_flag_like_token(w: &str) -> bool {
    let mut chars = w.chars();
    chars.next() == Some('-') && chars.next().is_some_and(|c| !c.is_ascii_digit())
}

/// One line of a shell worked example: a `./`-prefixed leading word, plus
/// a real flag token or a `#` comment marker further on. Deliberately
/// `./`-only, not "any bare word" — `ls`'s own `--time` description reads
/// "with -l, WORD determines..." mid-sentence, a bare word (`with`)
/// immediately followed by a real flag (`-l`), and a looser rule swallowed
/// it whole. See docs/shapes.md S-126.
fn looks_like_invocation_line(trimmed: &str) -> bool {
    let mut words = trimmed.split_whitespace();
    let Some(first) = words.next() else {
        return false;
    };
    if !(first.starts_with("./") && first[2..].starts_with(|c: char| c.is_ascii_alphanumeric())) {
        return false;
    }
    words.any(is_flag_like_token) || trimmed.contains(" # ") || trimmed.trim_end().ends_with('#')
}

/// Look ahead from a candidate continuation line at `lines[start]` for an
/// unheaded worked-example block (`nfsslower-bpfcc`'s five `./nfsslower
/// ...  # ...` lines right after its last flag's description, separated
/// by a blank line and sitting one indent deeper than the flag rows):
/// every non-blank line up to the next blank line or dedent must read as
/// [`looks_like_invocation_line`], and there must be at least
/// [`MIN_EXAMPLE_BLOCK_LINES`] of them. Refuses (returns `false`) the
/// moment one line fails the shape test, so a genuine wrapped description
/// that merely opens with a name-shaped word is never swallowed by this
/// rule. See docs/shapes.md S-126.
pub(super) fn example_block_starts_at(lines: &[&str], start: usize) -> bool {
    let mut n = 0usize;
    let mut j = start;
    while let Some(line) = lines.get(j) {
        if line.trim().is_empty() {
            break;
        }
        if !looks_like_invocation_line(line.trim_start()) {
            return false;
        }
        n += 1;
        j += 1;
    }
    n >= MIN_EXAMPLE_BLOCK_LINES
}

/// Look ahead from a candidate continuation line at `lines[start]` (indent
/// `indent`, already deeper than the flags block's own entries) for a
/// nested entry table — command rows with their own one-level-deeper
/// descriptions — rather than an ordinary wrapped description of the flag
/// above it.
///
/// Indentation alone cannot tell the two apart: `btrfs --help`'s
/// `Options for the main command only:` has an ordinary flag row directly
/// above a deeper-indented command table, and reading every line of that
/// table as more of the flag's own description folds the whole table into
/// one flag. See docs/shapes.md S-050.
///
/// # The rule
///
/// A row is counted when a non-blank line sits at exactly `indent`, is not
/// [`looks_like_flag_start`] (a real flag row at this indent is business
/// as usual, not a nested table), and is immediately followed by a
/// non-blank line indented deeper still. The lookahead stops the moment a
/// non-blank line dedents past `indent`. At least [`MIN_NESTED_TABLE_ROWS`]
/// such rows makes it a table.
///
/// Returning `true` tells [`scan_flags_block`] to `break` at `start` —
/// re-routes rather than drops, the same contract [`bare_block_end`]'s
/// flag-row break uses: the caller resumes its scan at exactly this line.
pub(super) fn nested_entry_table_starts_at(lines: &[&str], start: usize, indent: usize) -> bool {
    let mut rows = 0usize;
    let mut j = start;
    while j < lines.len() {
        let line = lines[j];
        if line.trim().is_empty() {
            j += 1;
            continue;
        }
        let line_indent = leading_whitespace(line);
        if line_indent < indent {
            break;
        }
        if line_indent == indent && !looks_like_flag_start(line.trim_start()) {
            if let Some(next) = lines.get(j + 1) {
                if !next.trim().is_empty() && leading_whitespace(next) > indent {
                    rows += 1;
                    // Nothing past the floor can change the answer, and
                    // this scan runs once per candidate continuation line:
                    // returning as soon as it is decided keeps a positive
                    // match from walking the rest of a long table (the
                    // "never call an O(n) function from inside a loop"
                    // hazard in AGENTS.md §2). The negative case is
                    // already short — it stops at the first line that
                    // dedents past `indent`, which in an ordinary flags
                    // block is the very next entry row.
                    if rows >= MIN_NESTED_TABLE_ROWS {
                        return true;
                    }
                }
            }
        }
        j += 1;
    }
    rows >= MIN_NESTED_TABLE_ROWS
}

/// Does the flags-block entry row currently being continued already carry
/// its own, non-empty description on its own line?
///
/// [`nested_entry_table_starts_at`] cannot tell apart two shapes that look
/// identical from indentation alone: a nested table that does not belong
/// to the flag above it (break away — `btrfs`'s `--version`), and a
/// value-choice or keyword list that *is* the flag's whole description
/// (never break — pngfix's `--strip=[...]:` and pod2man's
/// `--guesswork=rule[,rule...]` carry nothing on their own line, so
/// breaking there deletes the entire description rather than mis-splitting
/// it). See docs/shapes.md S-051.
///
/// # The rule
///
/// The entry row already has real description text only when a
/// conservative single-column split of that one line
/// ([`split_single_column_entry`]) yields a non-empty description. A row
/// that instead looks multi-column-shaped ([`fields_in_line`] finds more
/// than one field) is read conservatively as not yet settled, since the
/// block-wide multi-column decision isn't available mid-scan; refusing the
/// break is always safe.
///
/// Evaluated once per candidate line from the entry row's own text, never
/// from continuation rows accumulated so far, so a description already
/// underway can never be truncated part-way through.
pub(super) fn entry_row_carries_own_description(entry_line: &str) -> bool {
    if fields_in_line(entry_line).len() > 1 {
        return false;
    }
    let (_, desc) = split_single_column_entry(entry_line);
    !desc.trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `btrfs --help`'s shape: a flags block followed by a deeper-indented
    /// command table whose own rows each have a description one indent
    /// deeper still. Three table groups, deliberately more than
    /// [`MIN_NESTED_TABLE_ROWS`]'s floor of two, since a single ragged
    /// continuation must never trip this detector. See docs/shapes.md
    /// S-050 and corpus/btrfs/audit-seed2/help.txt.
    #[test]
    fn nested_command_table_does_not_swallow_into_a_flag_description() {
        let help = "\
Usage: widget [global] <group> <command> [<args>]

Options for the main command only:
  --help            print condensed help for all subcommands
  --version         print version string

    widget group one start
        Start the first task
    widget group one stop
        Stop the first task
    widget group two run
        Run the second task
";
        let parsed = parse(help);
        let version = flag_named(&parsed, "version");
        assert_eq!(
            version.description.as_ref().map(|t| t.as_str()),
            Some("print version string"),
            "--version's description must not absorb the command table below it"
        );
        for swallowed in [
            "Start the first task",
            "Stop the first task",
            "Run the second task",
        ] {
            assert!(
                !version
                    .description
                    .as_ref()
                    .is_some_and(|d| d.as_str().contains(swallowed)),
                "table row {swallowed:?} leaked into --version's description: {:?}",
                version.description
            );
        }
    }

    /// `pngfix --strip`'s shape: the flag row ends in `:` with no inline
    /// description, and everything one indent deeper — a value-choice list
    /// whose longer choices wrap onto a still-deeper line — is that flag's
    /// whole description. The wrap looks exactly like
    /// [`nested_entry_table_starts_at`]'s own "row followed by something
    /// deeper" shape, so without the entry-row gate this broke the flags
    /// block and deleted the entire choice list. See docs/shapes.md S-051
    /// and corpus/pngfix/*/help.txt.
    #[test]
    fn value_choice_list_with_wrapped_entries_is_not_read_as_a_nested_table() {
        let help = "\
Usage: widget [options] file

OPTIONS
    --strip=[none|crc|unsafe|unused]:
        none (default): Retain all chunks.
        crc: Remove chunks with a bad CRC.
        unsafe: Remove chunks that may be unsafe to retain if the image data
                is modified. This is set automatically if --max is given.
        unused: Remove chunks not used when decoding an image. This retains
                any chunks that might be used by transformations.
    --optimize (-o):
        Find the smallest deflate window size for the compressed data.
";
        let parsed = parse(help);
        let strip = flag_named(&parsed, "strip");
        let desc = strip.description.as_ref().map_or("", |t| t.as_str());
        assert!(
            desc.contains("Retain all chunks"),
            "--strip's own description must not be dropped: {desc:?}"
        );
        assert!(
            desc.contains("Remove chunks not used"),
            "--strip's own description must not be dropped: {desc:?}"
        );
    }

    /// `pod2man --guesswork`'s shape: no inline description, an ordinary
    /// wrapped paragraph, then a genuine bare-word keyword list, each
    /// keyword followed by its own explanation one indent deeper.
    /// [`nested_entry_table_starts_at`] is right that something
    /// table-shaped is down there — wrong that it belongs to a different
    /// entry rather than to `--guesswork`'s own description. See
    /// docs/shapes.md S-051 and corpus/pod2man/*/help.txt.
    #[test]
    fn guesswork_style_keyword_list_is_not_read_as_a_nested_table() {
        let help = "\
Usage: widget [options]

Options:
    --guesswork=rule[,rule...]
        By default, widget applies some default formatting rules based on
        guesswork. This option allows turning all or some of it off.

        Otherwise, the value of this option should be a comma-separated
        list of one or more of the following keywords:

        functions
            Convert function references like foo() to bold even if they
            have no markup.

        manref
            Make the first part of man page references like foo(1) bold
            even if they have no markup.

    --help
        Show this help.
";
        let parsed = parse(help);
        let guesswork = flag_named(&parsed, "guesswork");
        let desc = guesswork.description.as_ref().map_or("", |t| t.as_str());
        assert!(
            desc.contains("default formatting rules"),
            "--guesswork's own description must not be dropped: {desc:?}"
        );
        assert!(
            desc.contains("functions"),
            "--guesswork's keyword list must not be dropped: {desc:?}"
        );
    }

    /// sed's `--help` has no `Options:`/`Flags:` heading at all; must still
    /// be recovered as a headingless flags block. See docs/shapes.md
    /// S-052.
    #[test]
    fn sed_headingless_flags_block_is_recovered() {
        let parsed = parse(SED_HELP);
        let quiet = parsed.flags.iter().find(|f| f.long() == Some("quiet"));
        assert!(
            quiet.is_some(),
            "expected --quiet among {:?}",
            parsed.flags.iter().map(|f| f.long()).collect::<Vec<_>>()
        );
        assert_eq!(quiet.unwrap().short(), Some('n'));
        assert!(quiet
            .unwrap()
            .description
            .as_ref()
            .unwrap()
            .as_str()
            .contains("suppress automatic printing"));
    }

    /// A positional documented as the first row of an options table must
    /// not cost the whole table. `kill --help` opens `Options:` with
    /// `<pid> [...]`. See docs/shapes.md S-044.
    #[test]
    fn a_leading_positional_row_does_not_discard_the_options_table() {
        let help = "Usage:\n kill [options] <pid> [...]\n\nOptions:\n \
                    <pid> [...]            send signal to every <pid> listed\n \
                    -q, --queue <value>    integer value to be sent with the signal\n \
                    -L, --table            list all signal names in a nice table\n";
        let parsed = parse(help);
        let longs: Vec<&str> = parsed.flags.iter().filter_map(|f| f.long()).collect();
        assert!(longs.contains(&"queue"), "{longs:?}");
        assert!(longs.contains(&"table"), "{longs:?}");
        assert!(
            parsed.subcommands.is_empty(),
            "the positional row must not become a subcommand: {:?}",
            parsed
                .subcommands
                .iter()
                .map(|c| &c.name)
                .collect::<Vec<_>>()
        );
    }

    /// `ip`'s own row: every top-level `|`-segment already carries its own
    /// abbreviation bracket, so each is a complete flag. See
    /// docs/shapes.md S-042.
    #[test]
    fn a_bnf_alternation_row_of_self_contained_abbreviated_flags_splits_fully() {
        let entries =
            split_bnf_alternation_row("-V[ersion] | -s[tatistics] | -d[etails] | -r[esolve] |")
                .expect("row splits");
        let specs: Vec<&str> = entries.iter().map(|(s, _)| s.as_str()).collect();
        assert_eq!(
            specs,
            vec!["-V[ersion]", "-s[tatistics]", "-d[etails]", "-r[esolve]"]
        );
        assert!(entries.iter().all(|(_, desc)| desc.is_empty()));
    }

    /// `dcb`'s own convention: short and long spelled as two adjacent
    /// alternatives. A bare short immediately followed by a bare long
    /// folds back into one flag with both spellings. See docs/shapes.md
    /// S-043.
    #[test]
    fn a_bnf_alternation_row_pairs_bare_short_and_long_spellings() {
        let entries = split_bnf_alternation_row("-V | --Version | -i | --iec | -j | --json")
            .expect("row splits");
        assert_eq!(entries.len(), 3);
        let short = parse_flag_spec(&entries[0].0);
        assert_eq!(short.short(), Some('V'));
        assert_eq!(short.long(), Some("Version"));
    }

    /// `sg_sanitize`'s real `--count=OC|-c OC`: one flag, an alias plus a
    /// shared value, never two, since the second segment leaves real
    /// prose unconsumed and fails `fully_consumed` on its own.
    ///
    /// This function alone cannot refuse every non-BNF `|`-joined pair —
    /// `btrfsck`'s `-E|--subvol-extents <subvolid>` DOES split cleanly at
    /// this level — which is why the caller ([`scan_flags_block`]) never
    /// invokes it unless the block's heading came from a `:=` production.
    /// See `a_plain_pipe_joined_flag_row_survives_parse_with_profile_unsplit`
    /// below for that gate end to end, and docs/shapes.md S-043.
    #[test]
    fn a_row_gluing_one_flags_alias_and_value_through_a_pipe_is_never_split() {
        assert_eq!(
            split_bnf_alternation_row("--count=OC|-c OC  OC is overwrite count"),
            None
        );
    }

    /// `rdma`'s `-p[retty] -r[aw]}` and `devlink`'s
    /// `-v[erbose] -s[tatistics] -[he]x`: two or three flags run together
    /// by a bare space inside one `|`-segment, the "missing separator"
    /// shape this reader refuses. See docs/shapes.md S-043.
    #[test]
    fn a_row_whose_segment_runs_two_flags_together_with_a_bare_space_is_refused() {
        assert_eq!(
            split_bnf_alternation_row("-V[ersion] | -d[etails] | -j[son] | -p[retty] -r[aw]}"),
            None
        );
        assert_eq!(
            split_bnf_alternation_row(
                "-V[ersion] | -n[o-nice-names] | -j[son] | -p[retty] | -v[erbose] -s[tatistics] -[he]x }"
            ),
            None
        );
    }

    /// `vdpa`'s closing brace (`-p[retty] }`) and `dcb`'s glued closing
    /// bracket (`--verbose]`) — the enclosing group's closer landing on the
    /// row's last alternative two different ways. Both must vanish without
    /// being read as that flag's value. See docs/shapes.md S-043.
    #[test]
    fn a_trailing_stray_group_closer_never_becomes_a_value() {
        assert_eq!(strip_trailing_stray_bracket("-p[retty] }"), "-p[retty]");
        assert_eq!(strip_trailing_stray_bracket("--verbose]"), "--verbose");
        // A bracket that closes something real in the *same* segment is
        // never touched.
        assert_eq!(
            strip_trailing_stray_bracket("-b[atch] [filename]"),
            "-b[atch] [filename]"
        );
    }

    /// End-to-end regression: a real short/long pair joined by `|` outside
    /// any `:=` production must come through `parse_with_profile`
    /// completely unsplit. See docs/shapes.md S-043.
    #[test]
    fn a_plain_pipe_joined_flag_row_survives_parse_with_profile_unsplit() {
        let raw = "Usage: btrfsck [options] <device>\n\nOptions:\n    -Q|--qgroup-report        print a report on qgroup consistency\n    -E|--subvol-extents <subvolid>\n                              print subvolume extents and sharing state\n";
        let result = parse_with_profile(raw, None, Some("btrfsck"));
        let e = result
            .flags
            .iter()
            .find(|f| f.long() == Some("subvol-extents"))
            .expect("subvol-extents recovered as one flag");
        assert_eq!(e.short(), Some('E'));
        assert_eq!(e.value_name.as_deref(), Some("<subvolid>"));
        assert!(
            !result
                .flags
                .iter()
                .any(|f| f.short().is_none() && f.long().is_none()),
            "no half-flag left behind"
        );
    }

    /// `where OBJECT := { address | ... }` is BNF grammar, not the tool's
    /// own description — the leak `looks_like_bnf_production_line` closes.
    #[test]
    fn a_bnf_production_line_never_becomes_the_description() {
        assert!(looks_like_bnf_production_line(
            "where  OBJECT := { address | addrlabel | amt }"
        ));
        assert!(looks_like_bnf_production_line("OPTIONS := { -V | -s }"));
        // An ordinary sentence that happens to start with the same word
        // must stay eligible to become the description.
        assert!(!looks_like_bnf_production_line(
            "where possible, prefer the short spelling"
        ));
    }

    /// [`try_split_packed_row`] unit cases: the sharp operand-vs-next-entry
    /// boundaries spec calls out for GNU `find --help`'s own shape.
    #[test]
    fn try_split_packed_row_unit_cases() {
        assert_eq!(
            try_split_packed_row("-anewer FILE -atime N"),
            Some(vec![
                ("-anewer".to_string(), "FILE".to_string()),
                ("-atime".to_string(), "N".to_string()),
            ])
        );
        // A prefix-bracket-then-bare-suffix value spec with no separator
        // (`-perm`'s own convention) is kept as one operand, verbatim,
        // rather than decomposed further.
        assert_eq!(
            try_split_packed_row("-perm [-/]MODE -regex PATTERN"),
            Some(vec![
                ("-perm".to_string(), "[-/]MODE".to_string()),
                ("-regex".to_string(), "PATTERN".to_string()),
            ])
        );
        // Two operands on one entry (`-fprintf FILE FORMAT`) are kept
        // together, not split into a second flag.
        assert_eq!(
            try_split_packed_row("-fprintf FILE FORMAT -print"),
            Some(vec![
                ("-fprintf".to_string(), "FILE FORMAT".to_string()),
                ("-print".to_string(), String::new()),
            ])
        );
        // The `-exec`/`-ok` command-terminator convention: bare `;` and
        // `{} +` are operand tokens, never new entries (neither starts
        // with a dash-plus-letter).
        assert_eq!(
            try_split_packed_row("-exec COMMAND ; -exec COMMAND {} +"),
            Some(vec![
                ("-exec".to_string(), "COMMAND ;".to_string()),
                ("-exec".to_string(), "COMMAND {} +".to_string()),
            ])
        );
        // Real prose between two dash-looking tokens is not this shape at
        // all — refuse rather than guess where the entry boundary is.
        assert_eq!(
            try_split_packed_row("-foo Enable the foo behavior -bar"),
            None
        );
        // A lone boolean flag alone on its line still succeeds (one
        // entry, empty operand) — `block_is_packed_flag_rows` is what
        // requires at least one *other* line in the block to pack two or
        // more before this one is read as part of the shape.
        assert_eq!(
            try_split_packed_row("-readable"),
            Some(vec![("-readable".to_string(), String::new())])
        );
    }

    /// `-wholename PATTERN -size N[bcwkMG] -true -type [bcdpflsD] -uid N`:
    /// `-size`'s bracketed unit suffix must never be misread as a
    /// description boundary that hands `-wholename` text belonging to the
    /// entries after it. See docs/shapes.md S-047.
    #[test]
    fn find_style_packed_tests_block_recovers_every_entry_with_no_fabricated_description() {
        let raw = concat!(
            "Usage: find [-H] [-L] [-P] [path...] [expression]\n",
            "\n",
            "Tests (N can be +N or -N or N):\n",
            "      -wholename PATTERN -size N[bcwkMG] -true -type [bcdpflsD] -uid N\n",
        );
        let parsed = parse_named(raw, "find");
        let wholename = parsed
            .flags
            .iter()
            .find(|f| f.long() == Some("wholename"))
            .expect("-wholename recovered");
        assert_eq!(wholename.value_name.as_deref(), Some("PATTERN"));
        assert!(
            wholename.description.is_none(),
            "no description exists in this document; must not be fabricated: {:?}",
            wholename.description
        );
        assert!(wholename.single_dash());
        for name in ["size", "true", "type", "uid"] {
            assert!(
                parsed.flags.iter().any(|f| f.long() == Some(name)),
                "expected {name} to be recovered as its own flag, not folded into -wholename"
            );
        }
    }

    /// GNU find's real `-exec`/`-execdir` write two packed entries under
    /// one spelling (a `;`-terminated form and a `{} +`-terminated form).
    /// They must merge into one `Flag`, not appear twice.
    #[test]
    fn find_style_packed_actions_block_merges_repeated_spellings() {
        let raw = concat!(
            "Actions:\n",
            "      -exec COMMAND ; -exec COMMAND {} + -ok COMMAND ;\n",
        );
        let parsed = parse_named(raw, "find");
        let exec_flags: Vec<_> = parsed
            .flags
            .iter()
            .filter(|f| f.long() == Some("exec"))
            .collect();
        assert_eq!(
            exec_flags.len(),
            1,
            "one -exec flag, not two: {exec_flags:?}"
        );
        assert_eq!(
            exec_flags[0].value_name.as_deref(),
            Some("COMMAND ; | COMMAND {} +")
        );
    }

    /// A block with real prose descriptions must never be read as packed,
    /// even if one of its rows happens to carry two dash-looking tokens.
    #[test]
    fn packed_row_reader_never_claims_a_block_with_real_descriptions() {
        let raw = concat!(
            "Other common options:\n",
            "      --help                   display this help and exit\n",
            "      --version                output version information and exit\n",
        );
        let parsed = parse_named(raw, "find");
        let help = parsed
            .flags
            .iter()
            .find(|f| f.long() == Some("help"))
            .expect("--help recovered");
        assert_eq!(
            help.description.as_ref().map(|t| t.as_str()),
            Some("display this help and exit")
        );
    }

    /// `jmod --help`'s wrapped `--target-platform <String: target-` /
    /// `  platform>`: the continuation completes the placeholder, so it
    /// must join `value_name`, not `description`. See docs/shapes.md
    /// S-048 and corpus/jmod/17.0.20/help.txt.
    #[test]
    fn a_wrapped_placeholder_continuation_joins_the_value_name_not_the_description() {
        let parsed = parse_named(JMOD_HELP, "jmod");
        let flag = flag_named(&parsed, "target-platform");
        assert_eq!(
            flag.value_name.as_deref(),
            Some("<String: target-platform>"),
            "the wrapped placeholder must be joined into value_name whole"
        );
        assert_eq!(
            flag.description.as_ref().map(|t| t.as_str()),
            Some("Target platform"),
            "no placeholder tail must leak into the description"
        );
    }

    /// `msgcat`'s `-<, --less-than=NUMBER` — a short flag literally spelled
    /// with `<` — must never look like an open placeholder: its `<` is
    /// glued onto the leading `-` with no token boundary. See
    /// docs/shapes.md S-048.
    #[test]
    fn a_short_flag_spelled_with_the_literal_angle_bracket_is_never_an_open_placeholder() {
        assert!(!placeholder_left_open("-<, --less-than=NUMBER"));
        // The genuine open-placeholder case must still be recognized.
        assert!(placeholder_left_open("--target-platform <String: target-"));
    }

    /// `jmod --help`'s trailing `@<filename>  Read options from the
    /// specified file` row must never corrupt `--version` above it, and
    /// must become its own dashless flag entity spelled `@`. See
    /// docs/shapes.md S-021 and corpus/jmod/17.0.20/help.txt.
    #[test]
    fn an_argfile_row_never_corrupts_the_entry_above_it() {
        let parsed = parse_named(JMOD_HELP, "jmod");
        let version = flag_named(&parsed, "version");
        assert_eq!(
            version.description.as_ref().map(|t| t.as_str()),
            Some("Version information"),
            "the @<filename> row must not leak into --version's description: {:?}",
            version.description
        );
        assert!(
            !parsed
                .flags
                .iter()
                .any(|f| f.long().is_some_and(|l| l.contains("filename"))),
            "@<filename> must never become a *long-spelled* flag"
        );
        let argfile = parsed
            .flags
            .iter()
            .find(|f| f.primary_name() == "@")
            .expect("the @<filename> row must be recovered as the argfile sigil flag");
        assert_eq!(argfile.kind, mandible_core::EntityKind::Flag);
        assert!(argfile.long().is_none() && argfile.short().is_none());
        assert_eq!(argfile.value_name.as_deref(), Some("<filename>"));
        assert_eq!(argfile.value_kind, ValueKind::Required);
        assert_eq!(
            argfile.description.as_ref().map(|t| t.as_str()),
            Some("Read options from the specified file")
        );
    }

    /// Isolates the argfile guard from the dash-underline guard: the test
    /// above uses jmod, whose block also carries a dash-underline header
    /// row that alone could mask a failure. `size --help`
    /// (`tests/fixtures/help_text/size_help.stdout`) has no dash-underline
    /// row anywhere, so a break here can only be caused by the argfile
    /// guard itself. See docs/shapes.md S-021.
    #[test]
    fn an_argfile_row_never_corrupts_the_entry_above_it_with_no_dash_underline_row_present() {
        let parsed = parse_named(SIZE_HELP, "size");
        let target = flag_named(&parsed, "target");
        assert_eq!(
            target.description.as_ref().map(|t| t.as_str()),
            Some("Set the binary file format"),
            "the @<file> row must not leak into --target's description: {:?}",
            target.description
        );
        assert!(
            !parsed
                .flags
                .iter()
                .any(|f| f.long().is_some_and(|l| l.contains("file>"))),
            "@<file> must never become a *long-spelled* flag"
        );
        // `size --help`'s own `@<file>   Read options from <file>` row
        // recovers exactly like jmod's, confirming the shape generalizes.
        let argfile = parsed
            .flags
            .iter()
            .find(|f| f.primary_name() == "@")
            .expect("size's @<file> row must be recovered as the argfile sigil flag");
        assert_eq!(argfile.value_name.as_deref(), Some("<file>"));
        assert_eq!(argfile.value_kind, ValueKind::Required);
        assert_eq!(
            argfile.description.as_ref().map(|t| t.as_str()),
            Some("Read options from <file>")
        );
    }

    /// `ar --help`'s real `--target=BFDNAME - specify ...` and
    /// `--output=DIRNAME - specify ...` rows recover their descriptions
    /// through the full pipeline, not just `find_dash_token_separator_gap`
    /// in isolation. See docs/shapes.md S-058 and
    /// corpus/ar/audit-seed2/help.txt.
    #[test]
    fn ar_glued_equals_flags_recover_their_lowercase_descriptions() {
        let parsed = parse_named(AR_HELP, "ar");
        let target = flag_named(&parsed, "target");
        assert_eq!(target.value_name.as_deref(), Some("BFDNAME"));
        assert_eq!(
            target.description.as_ref().map(|t| t.as_str()),
            Some("specify the target object format as BFDNAME")
        );
        let output = flag_named(&parsed, "output");
        assert_eq!(output.value_name.as_deref(), Some("DIRNAME"));
        assert_eq!(
            output.description.as_ref().map(|t| t.as_str()),
            Some("specify the output directory for extraction operations")
        );
        // A neighbouring row whose column was found another way
        // (`--record-libdeps`, via `find_placeholder_boundary_gap`) reads
        // the same: `ar` writes ` - ` on every row of this table, and the
        // separator strips whichever finder located the column.
        let record_libdeps = flag_named(&parsed, "record-libdeps");
        assert_eq!(
            record_libdeps.description.as_ref().map(|t| t.as_str()),
            Some("specify the dependencies of this library")
        );
    }

    /// THE HAZARD, end to end rather than at the gap-finder alone: a row
    /// shaped `--flag WORD rest of a sentence` must never let `WORD` be
    /// mistaken for part of the spec. A bare lowercase word is never
    /// spec-shaped, so the dash-token-separator fallback never opens here.
    /// See docs/shapes.md S-058.
    #[test]
    fn a_prose_word_after_the_spec_never_opens_the_dash_token_fallback() {
        let raw = "Usage: widget [OPTIONS]\n\nOptions:\n  --mode auto - selects mode automatically\n  -h, --help  show this help message and exit\n";
        let parsed = parse(raw);
        let mode = flag_named(&parsed, "mode");
        assert_ne!(
            mode.description.as_ref().map(|t| t.as_str()),
            Some("selects mode automatically"),
            "a bare lowercase prose word must never be read as closing the spec"
        );
    }

    /// The GNU-binutils `@FILE` spelling (`nm`/`ld`/`as`) recovers
    /// identically to the bracketed `<file>`/`<filename>` spelling above.
    /// See docs/shapes.md S-021.
    #[test]
    fn the_uppercase_argfile_spelling_recovers_the_same_sigil_flag() {
        let raw = "Usage: nm [OPTION...] [file...]\n\nOptions:\n  -a, --debug-syms       Display debugger-only symbols\n  @FILE                  Read options from FILE\n  -h, --help             Display this information\n";
        let parsed = parse(raw);
        let argfile = parsed
            .flags
            .iter()
            .find(|f| f.primary_name() == "@")
            .expect("@FILE must be recovered as the argfile sigil flag");
        assert_eq!(argfile.value_name.as_deref(), Some("FILE"));
        assert_eq!(
            argfile.description.as_ref().map(|t| t.as_str()),
            Some("Read options from FILE")
        );
        // Its neighbours must be untouched — the sigil row must not have
        // swallowed `--debug-syms` above it or `--help` below it.
        assert!(parsed.flags.iter().any(|f| f.long() == Some("debug-syms")));
        assert!(parsed.flags.iter().any(|f| f.long() == Some("help")));
    }

    /// The false-positive guard (spec §4.5): `@` glued to something not
    /// placeholder-shaped (`user@host`, `jar`'s own `@classes.list`
    /// example) must never be read as the argfile sigil. See
    /// docs/shapes.md S-021.
    #[test]
    fn a_row_opening_with_at_but_not_placeholder_shaped_is_never_the_argfile_flag() {
        assert_eq!(
            argfile_row_value_name("@example.com    contact address"),
            None
        );
        assert_eq!(argfile_row_value_name("@classes.list"), None);
        // Sanity check the two real shapes still pass, so this test would
        // actually fail if the placeholder check were ever loosened away
        // rather than merely not extended.
        assert_eq!(
            argfile_row_value_name("@<file>  read options"),
            Some("<file>")
        );
        assert_eq!(
            argfile_row_value_name("@FILE  Read options from FILE"),
            Some("FILE")
        );
    }

    /// `llvm-ar --help`'s `--format` / `=default - default` / ... sub-rows
    /// are `--format`'s enumerated values and must land in `choices`, not
    /// its description. See docs/shapes.md S-049 and
    /// corpus/llvm-ar-18/18.1.3/help.txt.
    #[test]
    fn llvm_ar_format_sub_rows_become_choices_not_description_text() {
        let parsed = parse_named(LLVM_AR_HELP, "llvm-ar-18");
        let format = flag_named(&parsed, "format");
        let choice_strs: Vec<&str> = format.choices.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            choice_strs,
            vec!["default", "gnu", "darwin", "bsd", "bigarchive"]
        );
        assert_eq!(
            format.description.as_ref().map(|t| t.as_str()),
            Some("archive format to create"),
            "the =value sub-rows must not remain in the description: {:?}",
            format.description
        );
    }

    /// `ffplay -help`'s real `-flags` row: six AVOption constants, each
    /// with its own scope-flags-plus-explanation text, nested one indent
    /// deeper than the flag row. Before this recognizer these fell through
    /// to the default continuation rule and smeared into `-flags`'s own
    /// description. See docs/shapes.md S-015 and
    /// corpus/ffplay/6.1.1-3ubuntu5/help.txt.
    #[test]
    fn ffplay_flags_sub_rows_become_described_choices() {
        let raw = "AVCodecContext AVOptions:\n\
                    \x20 -flags             <flags>      ED.VAS..... (default 0)\n\
                    \x20    unaligned                    .D.V....... allow decoders to produce unaligned output\n\
                    \x20    gray                         ED.V....... only decode/encode grayscale\n\
                    \x20    low_delay                    ED.V....... force low delay\n\
                    \x20    bitexact                     ED.VAS..... use only bitexact functions (except (I)DCT)\n\
                    \x20    output_corrupt               .D.V....... Output even potentially corrupted frames\n\
                    \x20    drop_changed                 .D.VA.....P Drop frames whose parameters differ from first decoded frame\n\
                    \x20 -ar                <int>        ED..A...... set audio sampling rate (in Hz) (from 0 to INT_MAX) (default 0)\n";
        let parsed = parse(raw);
        let flags = flag_named(&parsed, "flags");

        let want: Vec<(&str, &str)> = vec![
            (
                "unaligned",
                ".D.V....... allow decoders to produce unaligned output",
            ),
            ("gray", "ED.V....... only decode/encode grayscale"),
            ("low_delay", "ED.V....... force low delay"),
            (
                "bitexact",
                "ED.VAS..... use only bitexact functions (except (I)DCT)",
            ),
            (
                "output_corrupt",
                ".D.V....... Output even potentially corrupted frames",
            ),
            (
                "drop_changed",
                ".D.VA.....P Drop frames whose parameters differ from first decoded frame",
            ),
        ];
        let got: Vec<(&str, &str)> = flags
            .choices
            .iter()
            .map(|c| {
                (
                    c.name.as_str(),
                    c.description.as_ref().map_or("", |d| d.as_str()),
                )
            })
            .collect();
        assert_eq!(
            got, want,
            "every constant name and description byte must survive, verbatim, in order"
        );

        // The flag's own description shrinks to what actually belongs to
        // it — the constants are gone, not merely duplicated.
        let desc = flags.description.as_ref().map_or("", |t| t.as_str());
        assert!(
            desc.contains("ED.VAS....."),
            "the flag's own scope/default text must survive: {desc:?}"
        );
        for (name, text) in &want {
            assert!(
                !desc.contains(name) && !desc.contains(text),
                "choice {name:?}'s text must leave the flag's own description, not just be duplicated into choices: {desc:?}"
            );
        }

        // The flag row directly after the sub-table must survive untouched.
        // Its own spelling is a separate, pre-existing parser gap
        // (`-ar` reads as short `-a` plus a value named `r`), so this
        // checks the row's content landed somewhere rather than pinning
        // that name.
        assert!(
            parsed.flags.iter().any(|f| f
                .description
                .as_ref()
                .is_some_and(|d| d.as_str().contains("set audio sampling rate"))),
            "the row after the choices sub-table must not be dropped or absorbed: {:?}",
            parsed
                .flags
                .iter()
                .map(|f| f.description.as_ref().map(|d| d.as_str()))
                .collect::<Vec<_>>()
        );
        assert!(
            !flags.choices.iter().any(|c| c.name == "ar"
                || c.description
                    .as_ref()
                    .is_some_and(|d| d.as_str().contains("sampling rate"))),
            "the next flag row must never be absorbed into -flags's own choices: {:?}",
            flags.choices
        );
    }

    /// The same recognizer against the real, full `ffplay --help` capture
    /// rather than a hand-typed excerpt. See docs/shapes.md S-015 and
    /// corpus/ffplay/6.1.1-3ubuntu5/help.txt.
    #[test]
    fn ffplay_help_real_capture_describes_flags_choices() {
        let parsed = parse_named(FFPLAY_HELP, "ffplay");
        let flags = flag_named(&parsed, "flags");
        let names: Vec<&str> = flags.choices.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "unaligned",
                "gray",
                "low_delay",
                "bitexact",
                "output_corrupt",
                "drop_changed"
            ]
        );
        for choice in &flags.choices {
            assert!(
                choice.description.is_some(),
                "every -flags constant in the real capture documents itself: {:?}",
                choice.name
            );
        }
        let unaligned = flags
            .choices
            .iter()
            .find(|c| c.name == "unaligned")
            .unwrap();
        assert_eq!(
            unaligned.description.as_ref().map(|d| d.as_str()),
            Some(".D.V....... allow decoders to produce unaligned output")
        );
    }

    /// A wrapped-description continuation that merely starts with a
    /// lowercase word must never be misread as a described choice: real
    /// prose reflows at a single space, it does not draw the genuine
    /// 2+-space aligned column ffmpeg's AVOption table does. See
    /// docs/shapes.md S-015.
    #[test]
    fn a_single_spaced_prose_continuation_is_never_read_as_a_described_choice() {
        let raw = "Usage: widget [options]\n\nOptions:\n    \
                   --relaxed  a somewhat informal mode that widget falls back to\n        \
                   when strict parsing fails and the input still looks plausible enough to run\n";
        let parsed = parse(raw);
        let relaxed = flag_named(&parsed, "relaxed");
        assert!(
            relaxed.choices.is_empty(),
            "single-spaced prose must never become a choice: {:?}",
            relaxed.choices
        );
        let desc = relaxed.description.as_ref().map_or("", |t| t.as_str());
        assert!(
            desc.contains("strict parsing fails"),
            "the wrapped continuation must still land in the description: {desc:?}"
        );
    }

    // --- S-095: the neighbor-gated `+`/`+<placeholder>` option row -----

    /// `vim.basic --help`, byte-exact
    /// (`corpus/vim.basic/audit-seed4/help.txt` lines 36-42): the bare `+`
    /// row beside 40+ ordinary `-x` rows must become a flag spelled `+`.
    #[test]
    fn vim_basic_bare_plus_row_becomes_a_flag() {
        let raw = "Arguments:\n\
                    \x20  --noplugin\t\tDon't load plugin scripts\n\
                    \x20  -p[N]\t\tOpen N tab pages (default: one for each file)\n\
                    \x20  -O[N]\t\tLike -o but split vertically\n\
                    \x20  +\t\t\tStart at end of file\n\
                    \x20  +<lnum>\t\tStart at line <lnum>\n\
                    \x20  --cmd <command>\tExecute <command> before loading any vimrc file\n";
        let parsed = parse_named(raw, "vim.basic");
        let plus = parsed
            .flags
            .iter()
            .find(|f| {
                f.spellings.len() == 1 && f.spellings[0].name == "+" && f.value_name.is_none()
            })
            .unwrap_or_else(|| panic!("no bare `+` flag in {:?}", parsed.flags));
        assert_eq!(
            plus.description.as_ref().map(|t| t.as_str()),
            Some("Start at end of file")
        );
    }

    /// The row directly below the one above, same fixture: `+<lnum>`
    /// recovers a flag spelled `+` with value `<lnum>`.
    #[test]
    fn vim_basic_plus_lnum_row_recovers_a_value() {
        let raw = "Arguments:\n\
                    \x20  --noplugin\t\tDon't load plugin scripts\n\
                    \x20  -p[N]\t\tOpen N tab pages (default: one for each file)\n\
                    \x20  -O[N]\t\tLike -o but split vertically\n\
                    \x20  +\t\t\tStart at end of file\n\
                    \x20  +<lnum>\t\tStart at line <lnum>\n\
                    \x20  --cmd <command>\tExecute <command> before loading any vimrc file\n";
        let parsed = parse_named(raw, "vim.basic");
        let lnum = parsed
            .flags
            .iter()
            .find(|f| {
                f.spellings.len() == 1 && f.spellings[0].name == "+" && f.value_name.is_some()
            })
            .unwrap_or_else(|| panic!("no `+<lnum>` flag in {:?}", parsed.flags));
        assert_eq!(lnum.value_name.as_deref(), Some("<lnum>"));
        assert_eq!(lnum.value_kind, ValueKind::Required);
        assert_eq!(
            lnum.description.as_ref().map(|t| t.as_str()),
            Some("Start at line <lnum>")
        );
    }

    /// `nvim --help`, byte-exact (`corpus/nvim/0.9.5/help.txt` lines
    /// 6-11): the comma-joined alias row recovers *both* spellings —
    /// `+<cmd>` and `-c <cmd>` — sharing one description, not just the
    /// first. See AGENTS.md S-3.9.
    #[test]
    fn nvim_plus_cmd_alias_row_recovers_both_spellings() {
        let raw = "Options:\n\
                    \x20 --                    Only file names after this\n\
                    \x20 +                     Start at end of file\n\
                    \x20 --cmd <cmd>           Execute <cmd> before any config\n\
                    \x20 +<cmd>, -c <cmd>      Execute <cmd> after config and first file\n\
                    \x20 -l <script> [args...] Execute Lua <script> (with optional args)\n";
        let parsed = parse_named(raw, "nvim");
        let cmd = parsed
            .flags
            .iter()
            .find(|f| f.spellings.iter().any(|s| s.name == "+") && f.value_name.is_some())
            .unwrap_or_else(|| panic!("no `+<cmd>` flag in {:?}", parsed.flags));
        assert_eq!(cmd.value_name.as_deref(), Some("<cmd>"));
        assert_eq!(
            cmd.short(),
            Some('c'),
            "the `-c <cmd>` alias must not be dropped: {:?}",
            cmd.spellings
        );
        assert_eq!(
            cmd.description.as_ref().map(|t| t.as_str()),
            Some("Execute <cmd> after config and first file")
        );
    }

    /// `git-lfs --help`'s real AsciiDoc list-continuation marker, byte-
    /// exact: a bare `+` line with prose neighbors on both sides (a
    /// numbered step above, a shell command below), neither flag-shaped.
    /// Must never become a flag. The false positive a reverted fix once
    /// produced here. See docs/shapes.md S-095.
    #[test]
    fn git_lfs_list_continuation_marker_is_not_a_flag() {
        let raw = ". Setup Git LFS on your system. You only have to do this once per user\n\
                    account:\n\
                    +\n\
                    \n\
                    git lfs install\n";
        let parsed = parse_named(raw, "git-lfs");
        assert!(
            !parsed
                .flags
                .iter()
                .any(|f| f.spellings.iter().any(|s| s.name == "+")),
            "git-lfs's list-continuation marker must not become a `+` flag: {:?}",
            parsed.flags
        );
    }

    /// `date --help`'s real `%`-conversion-modifier table row, byte-exact:
    /// `+` sits among `0`, `^` modifier-character rows, none a real
    /// `-`-prefixed flag, so it has no flag-shaped neighbor. Must never
    /// become a flag. See docs/shapes.md S-095.
    #[test]
    fn date_percent_modifier_plus_row_is_not_a_flag() {
        let raw = "  0  (zero) pad with zeros\n\
                    \x20 +  pad with zeros, and put '+' before future years with >4 digits\n\
                    \x20 ^  use upper case if possible\n";
        let parsed = parse_named(raw, "date");
        assert!(
            !parsed
                .flags
                .iter()
                .any(|f| f.spellings.iter().any(|s| s.name == "+")),
            "date's `%`-modifier row must not become a `+` flag: {:?}",
            parsed.flags
        );
    }
}
