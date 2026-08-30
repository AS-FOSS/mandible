//! Flag-row mining: where an option table starts, how one physical line
//! splits into entries (packed rows, BNF alternations, bracket groups),
//! and where a nested entry table interrupts a row's description.

use super::*;

/// True when `line` opens a BNF grammar production — `LABEL := ...`, with
/// or without a leading `where` keyword: iproute2's own convention,
/// `where OBJECT := { address | addrlabel | ... }` and
/// `OPTIONS := { -V[ersion] | ... }`.
///
/// This is what a column-0 line carrying a bare `:=` actually is once the
/// usage block above it has ended — grammar, not the tool's own
/// description — and without this test it silently became one: `ip`'s and
/// `vdpa`'s entire node `description` was exactly this one line
/// (`where OBJECT := { address | addrlabel | amt | ... }`), because it is
/// the first column-0 line in the document once `Usage:`'s block closes,
/// and nothing distinguished it from a genuine leading-prose sentence. The
/// production's own *wrapped continuation* lines never reach this test at
/// all — they sit indented under it, so the description-paragraph scan
/// (only ever column-0 lines) never considers them either; this closes the
/// one line that would otherwise have been read as prose.
///
/// The label before `:=` (after stripping a leading `where`) must be
/// short, plain words — the same shape [`is_section_heading_line`] already
/// trusts for an ordinary heading — so a sentence that merely happens to
/// contain a stray `:=` deep in prose (an environment-variable assignment
/// quoted in an example, say) is never mistaken for one.
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
/// one (spec §7 Tier B rule 2) using the *shape* of the line (does it look
/// like a flag?) combined with how far it's indented relative to the
/// shallowest entry seen so far in this block — not a single shared
/// indent floor. Real `--help` output routinely mixes two entry depths in
/// one block (a short+long flag at column 2, a long-only flag at column 6,
/// because the formatter aligns long names under where the long form
/// would start after a short one), and a floor derived from whichever
/// entry happened to come first breaks whichever depth didn't come first.
/// Once a `-`-led token has been seen deep inside the continuation zone
/// (tar's `--occurrence` description references `--delete, --diff,
/// --extract` while wrapped, itself dash-led), shape alone would
/// misclassify it as a new entry — the indent check catches that: a
/// deeply-indented dash-led line is still closer to the description
/// column than to any entry's name column, so it stays a continuation.
/// Where a flags block actually begins at or after `start`, or `None` if
/// this section is not a flags block at all.
///
/// Normally that is `start` itself. The exception this exists for: a tool
/// that documents a **positional** as the first row of its options table.
/// `kill --help` opens its `Options:` section with
///
/// ```text
///  <pid> [...]            send signal to every <pid> listed
///  -q, --queue <value>    integer value to be sent with the signal
/// ```
///
/// Deciding flags-vs-bare-words from row one alone sent that whole block
/// to the bare-word path, and `kill` reported **zero flags** — measured,
/// and confirmed by deleting just that row from the help text, after which
/// the same build read 6 flags at 100% described.
///
/// Bounded deliberately, because "look harder for flags" is how
/// fabrication starts. A row is skipped only when it sits at the block's
/// own indent (deeper lines are that row's own description) and only
/// [`MAX_SKIPPED_LEADING_ROWS`] of them, and there must still be a real
/// `-`-leading row at that same indent. A bare-word command table contains
/// no such row, so it is unaffected — the discriminator stays the `-`
/// marker, which is self-identifying in a way bare words never are.
/// True if `line`'s left-hand token can open neither a flag entry nor a
/// command entry — it starts with a character that is neither a flag
/// prefix (`-`, `+`) nor the start of a name (alphanumeric).
///
/// Such a row is structurally *undecidable*: `[c]`, `[l <text> ]`,
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
        // A row whose left token could not be *either* kind of entry does
        // not decide what kind of block this is, so it does not spend the
        // budget for finding out. binutils `ar` opens its ` generic
        // modifiers:` block with eight `[c]`/`[l <text> ]`/`@<file>` rows
        // before the first `--target=BFDNAME`, and charging those eight
        // against a budget of three lost every long flag in the section.
        // The guard this budget exists for is untouched: a bare-word
        // command table's rows *are* possible command names, so they still
        // charge, and a block of them still never becomes a flags block.
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

// --- Multi-column option tables (spec §7 Tier B, [M-10]'s sibling defect,
// `corpus/lsof/4.95.0`) --------------------------------------------------
//
// Some tools (`lsof`, `unzip`, `infocmp`, `zipinfo`) pack two or three
// flag+description *pairs* onto one physical line instead of one:
//
// ```text
//   -?|-h list help          -a AND selections (OR)     -b avoid kernel blocks
// ```
//
// Reading one description column per line here doesn't just lose `-a` and
// `-b` (under-extraction) — it attributes their descriptions to `-?`
// instead (misattribution), which is fabricated documentation at full
// confidence. This section detects that shape from the block's own layout
// — column alignment recurring across several rows, never a tool name or a
// framework — and splits each row into its real per-flag pairs before
// `emit_flags` ever sees it.
//
// The detection mechanism (cells → fields → recurring offsets) mirrors
// `xtask/src/misattribution.rs`'s `DefinitionIndex`, which was built and
// measured against this exact bug first and already carries the hardening
// against the false-positive classes below.
//
// The vocabulary functions — [`is_flag_shaped`], [`is_flag_char`],
// [`first_word`], [`cells`], [`MIN_COLUMN_RECURRENCE`], and
// [`is_value_placeholder_only`] — are `pub` and re-exported from
// `help_text::mod` (same pattern as [`pick_stream`](super::pick_stream))
// precisely so `xtask/src/misattribution.rs` imports these instead of
// restating them: that restatement was tried once, for `pick_stream`, and
// silently drifted past the openssl stream fix (spec §13.1c's K2 table),
// producing 200 of 656 fleet-wide fabrications from an oracle that no
// longer agreed with the parser it was auditing. `is_flag_shaped`/`cells`/
// `is_value_placeholder_only` are exactly the same hazard: if the splitter
// here and the misattribution oracle disagree on what counts as a
// flag-shaped token or a bare placeholder, the oracle stops measuring this
// parser and starts measuring its own, different guess at the same
// question.
//
// [`fields_in_line`] itself is **not** shared, and this is a real,
// load-bearing behavioral difference, not an oversight: `misattribution`'s
// copy is an *advisory* metric a human reads, so it can afford to
// under-suppress (its own doc comment names `arptables`'s `-A chain` as a
// known, accepted residual false positive). A splitter's mistakes are not
// advisory — they fabricate a flag that was never in the tool's own text —
// so the copy below is strictly more conservative: it never starts a new
// field on top of one that hasn't yet earned real description text of its
// own (see its doc comment), which is exactly what keeps `-A chain`/`-p
// NUM`-shaped rows (a value placeholder standing in for real trailing text,
// lower-case so `is_value_placeholder_only` can't recognize it as one) from
// being read as a second, independent flag. If this splitter's fold rule
// changes, `misattribution::fields_in_line` will not — check both, by hand,
// on a real change here.

/// True if `token` opens a new packed entry: a dash immediately followed
/// by an ASCII letter. Narrower than [`looks_like_flag_start`] (which also
/// accepts a bare `-` and a `{...}` alternation) because this is asked of
/// one whitespace-delimited token, many times per line, rather than of a
/// whole physical line once — a bare trailing `-` or a brace group never
/// opens a second entry mid-line in this shape, and admitting either here
/// would risk splitting a real operand token in two.
pub(super) fn token_opens_packed_entry(token: &str) -> bool {
    let mut chars = token.chars();
    matches!(chars.next(), Some('-')) && matches!(chars.next(), Some(c) if c.is_ascii_alphabetic())
}

/// True if `token`, found between two packed entries, reads as the
/// previous entry's own operand rather than as prose — which would mean
/// the line is not this shape at all (see [`try_split_packed_row`]).
/// [`is_value_placeholder_only`] already recognizes a value placeholder
/// cell (`FILE`, `[bcdpflsD]`, an upper-case name with bracket decoration
/// like `N[bcwkMG]`); the three bare tokens `-exec`/`-execdir`/`-ok`/
/// `-okdir`'s own command-terminator convention writes with no other
/// decoration (`;`, `+`, `{}`) are added on top because none of them is
/// upper-case or bracket-wrapped as a whole token, and no other function in
/// this module already names them.
pub(super) fn token_is_packed_operand(token: &str) -> bool {
    is_value_placeholder_only(token) || matches!(token, ";" | "+" | "{}")
}

/// Split one physical line into the packed `(spelling, operand)` entries
/// it carries — never a description, because this shape has none. Returns
/// `None` the moment a token is neither a new entry's own opening dash nor
/// the previous entry's operand: that means real prose is present and the
/// line is not this shape, so the caller must fall back to the ordinary
/// single-column reading rather than guess. A line with only one entry and
/// nothing following it (an ordinary lone boolean flag) still returns
/// `Some` with one entry — [`block_is_packed_flag_rows`] is what requires
/// at least one line in the block to carry two or more before any of them
/// is read this way, so a block that happens to wrap one flag onto its own
/// line is not refused outright just for that line.
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
/// where the second and third physical lines open on the `|` that
/// separates them from the line above, not on a flag — every sibling in
/// this family (`ip`, `vdpa`, `bridge`, `rdma`, `devlink`) instead repeats
/// a flag spelling at the start of every wrapped line, which
/// [`looks_like_flag_start`] already recognizes as its own entry. Without
/// this, a `|`-led line is neither an entry (fails `looks_like_flag_start`,
/// which never accepts a leading `|`) nor read as a continuation of useful
/// shape — [`scan_flags_block`]'s continuation branch would still glue its
/// raw text onto the previous entry's `description`, but a BNF grammar row
/// carries no description to append to, so the text would sit there
/// unparsed rather than becoming the extra flags it names.
///
/// Requires flag-shaped content after the leading `|` and whitespace, not
/// just the `|` itself, so an unrelated line that happens to start a
/// physical line with `|` for some other reason (a table border, a
/// "pipe-or" example) is never swept in on the character alone.
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
/// counterpart earlier in the *same* segment — the residue of an
/// *enclosing* BNF alternation group's own closer landing on the row's
/// last alternative once the row has been split on `|`: `vdpa`'s
/// `-p[retty] }` (space-separated from its own abbreviation bracket) and
/// `dcb`'s `--verbose]` (glued straight onto the long spelling, closing the
/// `[` that `split_shared_heading_row` already consumed when it recognized
/// the block's opening line) are the same shape at two different
/// distances.
///
/// The no-matching-opener test is what tells this apart from a bracket
/// that really does belong to the segment: `-b[atch] [filename]`'s own
/// trailing `]` closes a `[` two tokens earlier in the *same* segment, so
/// it is left untouched, while `-c[olor]`'s own abbreviation bracket
/// (already closed by [`grammar::strip_short_abbrev_suffix`] before this
/// ever runs) never reaches this function without a group-closer glued
/// past it in the first place.
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
/// distinct flags on one physical line — the shape iproute2's shared help
/// emitter writes for its `OPTIONS := { ... }` production, once
/// [`split_shared_heading_row`] has already separated the row from the
/// heading it opened on (`ip`, `vdpa`, `bridge`, `rdma`, `devlink`; `dcb`
/// opens its group with `[` rather than `{`, which that function already
/// treats the same way — see its own doc comment). Returns one `(spec,
/// description)` pair per recovered alternative, description always empty
/// — a BNF grammar row carries no prose at all, only spellings — or `None`
/// when the row does not conform closely enough to split without risking a
/// fabrication.
///
/// # Two shapes inside one row, told apart by a pairing rule
///
/// `ip`'s convention spells a flag's long form as a bracketed suffix
/// glued onto the same token (`-V[ersion]`), so every top-level
/// `|`-segment is already a complete, self-contained flag. `dcb` never
/// abbreviates this way and instead spells the short and long forms as two
/// adjacent alternatives (`-V | --Version`) — the ordinary alias-list
/// convention [`parse_flag_spec`] already reads via a comma, just spelled
/// with `|` here. A bare short immediately followed by a bare long —
/// neither carrying a bracket, value, or anything else of its own
/// ([`is_unadorned_short`]/[`is_unadorned_long`]) — is folded back into one
/// alias-list segment before anything else runs, so `dcb`'s six pairs
/// become six flags rather than twelve one-spelling fragments.
///
/// # The false-positive guard: every segment must fully, cleanly consume
///
/// A top-level `|`-split alone is not sufficient evidence: `sg_sanitize`'s
/// `--count=OC|-c OC  OC is overwrite count` also splits into two dash-led
/// segments, and the two are one flag with an alias and a shared value, not
/// two flags — kept together by [`parse_flag_spec`]'s own alias-
/// continuation grammar precisely so a naive splitter doesn't take them
/// apart (see `alias_follows`'s doc comment). Checked per segment after the
/// pairing step above:
///
/// 1. [`looks_like_flag_start`] — a segment that fails even to look like a
///    flag row on its own is never something worth guessing at.
/// 2. `parse_flag_spec` must report `fully_consumed`: nothing left over
///    once its own spelling/value grammar has run. `sg_sanitize`'s second
///    segment leaves `"is overwrite count"` unconsumed and fails this.
/// 3. A recovered value must (a) not itself start with `-` — a value is
///    never another flag's spelling, which is what `devlink`'s un-piped
///    `-v[erbose] -s[tatistics] -[he]x` tail would otherwise fabricate onto
///    `-v` — and (b) only exist where the segment actually has a
///    whitespace boundary before it: `ip`'s own un-bracketed multi-letter
///    abbreviations (`-iec`, `-ts[hort]`) glue a bare word directly onto
///    the short letter with nothing between them, and reading that glued
///    run as a value is exactly the fabrication
///    [`grammar::strip_short_abbrev_suffix`]'s own doc comment already
///    warns about one layer in.
/// 4. An *unpaired* segment (one raw `|`-fragment, not the short/long join
///    above) may carry only one flag-shaped word. Without this, `rdma`'s
///    `-p[retty] -r[aw]` — one `|`-segment, two flags run together by a
///    bare space, the "missing separator" shape this reader deliberately
///    stays out of — would not fail condition 2 at all:
///    [`parse_flag_spec`]'s alias loop silently *consumes* `-r[aw]` as a
///    discarded extra spelling rather than leaving it as leftover text, so
///    `fully_consumed` comes back `true` with no value and no visible sign
///    `-r` was ever there. Counting flag-shaped words is what catches a
///    swallow that leaves no other trace.
///
/// Any segment failing any of the three refuses the **whole row** — this
/// reader never partially splits a line, which would leave some of a row's
/// flags recovered as separate entries and others still glued into
/// whichever one happened to parse, with nothing distinguishing the two
/// outcomes for a reviewer. A row this conservative about is one where
/// `ip`'s own multi-letter-abbreviation defect (`corpus/ip/6.1.0/meta.toml`)
/// already stops the whole line from benefiting — reported, not silently
/// patched over.
pub(super) fn split_bnf_alternation_row(line: &str) -> Option<Vec<(String, String)>> {
    let trimmed = line.trim();
    let raw_segments = split_alternatives(trimmed);
    if raw_segments.len() < 2 {
        return None;
    }
    // Clean each segment's own trailing stray bracket *before* the pairing
    // decision below, not after: `dcb`'s last pair (`-v`, `--verbose]`)
    // only reads as a bare short/long pair once `--verbose]`'s glued group
    // closer is gone — done any later, `-v` and `--verbose` would each end
    // up a separate one-spelling flag instead of one flag with both.
    let segments: Vec<&str> = raw_segments
        .iter()
        .map(|s| strip_trailing_stray_bracket(s))
        .collect();

    // `(text, was_paired)` — `was_paired` marks a group built by joining a
    // bare short and its bare long below, which legitimately carries two
    // flag-shaped words on purpose; every other group is one raw `|`-
    // segment and must carry exactly one.
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
        // A missing-separator hazard one layer finer than the top-level
        // `|`-split catches: `rdma`'s `-p[retty] -r[aw]` is a *single*
        // `|`-segment (no pipe inside it at all) whose two flags are
        // instead run together with a bare space — the exact shape
        // `parse_flag_spec`'s alias loop silently swallows, discarding
        // `-r[aw]` with no value, no leftover text, and therefore no
        // `fully_consumed` failure to catch it on. Refusing whenever an
        // *unpaired* group carries more than one flag-shaped word is what
        // stops that swallow from reaching this reader at all — the row
        // is refused rather than quietly losing a real flag.
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
/// [`try_split_packed_row`] (no row anywhere carries real prose this shape
/// would otherwise fabricate a boundary inside) **and** at least one row
/// actually packs two or more entries — proof this is genuinely the dense
/// shape and not just an ordinary one-flag-per-line block that happens to
/// have no description. Consulted only after [`block_is_multi_column`] and
/// the aligned-spelling check have both already declined the block, the
/// same subordination [`block_is_multi_column`]'s own doc comment
/// describes for that pair — never in front of either, since both are
/// already-working shapes this one must never compete with.
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
/// description)` — kept as a whole `&str` because the *splitting* decision
/// (one column vs. several — see [`block_is_multi_column`]) can't be made
/// per-line; it needs every entry row in the block at once.
pub(super) enum FlagsBlockRow<'a> {
    /// Looks like the start of a new flag entry.
    Entry(&'a str),
    /// A continuation of the previous entry's description (`trim_end`ed
    /// text only — the row's own indentation has already done its job by
    /// this point).
    Continuation(&'a str),
}

pub(super) fn scan_flags_block<'a>(
    lines: &[&'a str],
    start: usize,
    heading_is_bnf: bool,
) -> (usize, Vec<(String, String)>, bool) {
    const ENTRY_INDENT_TOLERANCE: usize = 10;
    let mut i = start;
    let mut rows: Vec<FlagsBlockRow<'a>> = Vec::new();
    let mut min_entry_indent: Option<usize> = None;
    let mut current_entry_line: Option<&'a str> = None;

    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() {
            i += 1;
            continue;
        }
        let indent = leading_whitespace(line);
        let trimmed = line.trim_start();

        let is_entry_start = (looks_like_flag_start(trimmed)
            || looks_like_bracket_flag_row(trimmed)
            // Gated the same way `split_bnf_alternation_row` is, and for
            // the identical reason: a leading `|` introduces a BNF
            // alternation's own wrapped continuation (`dcb`'s `| -N |
            // --Numeric | ...`) in exactly one shape this fleet has, but a
            // bare `|`-led line is also how `sg_write_x` wraps a single
            // alias onto its own line (`--generation=EOG,NOG` / `    |-G
            // EOG,NOG    and New ORWgeneration field to NOG`) — there the
            // leading `|` is a continuation *marker*, not grammar, and
            // reading it as a fresh entry split a real flag's own
            // continuation into a second, fabricated `-G`. Only a heading
            // known to be `:=`-shaped may read this leading-`|` shape as
            // anything other than the ordinary ("more indented, glue it
            // onto the entry above") continuation rule already handles.
            || (heading_is_bnf && looks_like_bnf_continuation_row(trimmed)))
            && min_entry_indent.is_none_or(|min| indent <= min + ENTRY_INDENT_TOLERANCE);

        if is_entry_start {
            rows.push(FlagsBlockRow::Entry(line));
            min_entry_indent = Some(min_entry_indent.map_or(indent, |m| m.min(indent)));
            current_entry_line = Some(line);
            i += 1;
            continue;
        }

        let is_continuation = !rows.is_empty() && min_entry_indent.is_some_and(|m| indent > m);
        if is_continuation {
            let entry_has_own_description =
                current_entry_line.is_some_and(entry_row_carries_own_description);
            if entry_has_own_description && nested_entry_table_starts_at(lines, i, indent) {
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

    // Whether this block packs more than one flag+description pair per
    // physical line (spec §7 Tier B, `lsof`'s options table) is a property
    // of the *block*, decided once from every entry row together — never
    // per line, which would let a block's ordinary single-column rows
    // (`lsof`'s own `-i select IPv[46] files`) get split as if a bare
    // second word were a second flag.
    let entry_lines: Vec<&str> = rows
        .iter()
        .filter_map(|r| match r {
            FlagsBlockRow::Entry(l) => Some(*l),
            FlagsBlockRow::Continuation(_) => None,
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

    let mut entries: Vec<(String, String)> = Vec::new();
    for row in rows {
        match row {
            FlagsBlockRow::Entry(line) => {
                // A docopt bracket-group row (LVM's `[ -d|--debug ]`) is
                // one flag and nothing else — no description column
                // exists on the row at all. Neither the multi-column
                // splitter nor the aligned-spelling-column splitter below
                // is the right tool for a shape with no second column to
                // find, so this row is read directly: the content inside
                // the brackets is exactly what `split_single_column_entry`
                // would otherwise try to recover by looking for a
                // whitespace gap that isn't there.
                if let Some(content) = bracket_flag_row_content(line.trim()) {
                    entries.push((content.to_string(), String::new()));
                    continue;
                }
                // The packed shape (`find --help`'s "Tests"/"Actions"
                // tables — see the block comment above
                // `block_is_packed_flag_rows`): several bare entries per
                // line, never a description. `block_is_packed_flag_rows`
                // already proved every entry line in this block splits
                // cleanly, so this can only be `None` for a line that
                // reached here despite that (never for `find` itself) —
                // and even then it degrades to the single-column path
                // below rather than panicking or dropping the row.
                if packed {
                    if let Some(subs) = try_split_packed_row(line) {
                        entries.extend(subs);
                        continue;
                    }
                }
                // A BNF alternation group naming several distinct flags on
                // one physical line (iproute2's shared `OPTIONS := { ... }`
                // emitter) — see `split_bnf_alternation_row`'s own doc
                // comment for the shape and the false-positive guard that
                // keeps this from ever taking `sg_sanitize`'s alias-plus-
                // value `|` apart the same way.
                //
                // **Gated on `heading_is_bnf`.** A bare `|` alone is not
                // sufficient evidence, full stop — `btrfsck`'s own
                // `-E|--subvol-extents <subvolid>` (a normal short/long
                // pair, `|` as its plain alias separator, no grammar
                // anywhere near it) and the whole `lv*`/`vg*`/`pv*` family's
                // `-A|--autobackup y|n` convention both satisfy every
                // per-segment check below on their own — each half
                // independently `fully_consumed` — and only the *document*
                // says these are one flag, not the BNF shape this reader
                // targets. Measured full-fleet before this gate existed: 8
                // tools outside the iproute2 family (`btrfsck`, `dpkg`,
                // `mkfs.btrfs`, `pvchange`, `sg_get_config`, `sg_write_x`,
                // `update-java-alternatives`, `vgchange`) had a real,
                // previously-correct short/long pair torn into two
                // half-flags. `heading_is_bnf` is `true` only when *this*
                // block's own heading was produced by
                // `split_shared_heading_row`'s `:=`-operator clause — the
                // one piece of evidence that actually distinguishes the two
                // shapes — never inherited from context or guessed at from
                // the row's own text.
                //
                // Never both at once in practice: `packed` requires *every*
                // entry line in the block to split via `try_split_packed_row`,
                // which treats a bare `|` token as neither a new packed
                // entry (`token_opens_packed_entry` requires a dash) nor a
                // valid operand (`token_is_packed_operand`) — so a real BNF
                // alternation row (which always contains a `|`) fails
                // `try_split_packed_row` on sight and `packed` comes back
                // `false` for the whole block. The `if packed` branch above
                // is what would run first if a block ever did satisfy both,
                // but that block does not occur in the measured fleet.
                //
                // Tried after the bracket-row and packed-row cases above
                // (never competes with LVM's shape: that row's whole content
                // sits inside one outer bracket, so it has no *top-level*
                // `|` for this to find) and before the ordinary column
                // splitters below, which would otherwise read the row's
                // first flag only and bury every alternative after it in
                // that flag's own `description`.
                if heading_is_bnf {
                    if let Some(alternatives) = split_bnf_alternation_row(line.trim()) {
                        entries.extend(alternatives);
                        continue;
                    }
                }
                // `fields_in_line` can come back empty on a line
                // `looks_like_flag_start` accepted (bare `-` test) but
                // whose leading token isn't `is_flag_shaped` (a stricter,
                // narrower class — see that function). Never silently drop
                // the row when that happens: fall back to the ordinary
                // single-column split, same as a block that was never
                // multi-column at all.
                let split = multi_column
                    .then(|| fields_in_line(line))
                    .filter(|f| !f.is_empty());
                match split {
                    Some(fields) => {
                        for field in fields {
                            entries
                                .push((field.tokens.join(", "), field.trailing.trim().to_string()));
                        }
                    }
                    None if aligned_spellings => entries.push(split_aligned_spelling_entry(line)),
                    None => entries.push(split_single_column_entry(line)),
                }
            }
            FlagsBlockRow::Continuation(text) => {
                if let Some(last) = entries.last_mut() {
                    last.1.push(' ');
                    last.1.push_str(text);
                }
            }
        }
    }
    (i, entries, packed)
}

/// The fewest name/description pairs a deeper-indented run must show before
/// [`nested_entry_table_starts_at`] will read it as a table rather than an
/// ordinary wrapped description.
///
/// Two, for the same reason [`scan_same_indent_entry_table`]'s `MIN_ROWS`
/// is two: one ragged continuation line that happens to be followed by a
/// still-deeper line is unremarkable prose, and must not trip this on its
/// own. Only repetition — the same name/description shape recurring — is
/// evidence of a table.
pub(super) const MIN_NESTED_TABLE_ROWS: usize = 2;

/// Look ahead from a candidate continuation line at `lines[start]` (indent
/// `indent`, already known to be deeper than the flags block's own entries)
/// for a **nested entry table** — command rows with their own
/// one-level-deeper descriptions — rather than an ordinary wrapped
/// description of the flag above it.
///
/// # Why this exists
///
/// [`scan_flags_block`]'s continuation rule used to be indentation alone:
/// *any* line deeper than the block's entries continues the previous
/// entry's description. That is right for a wrapped sentence and wrong for
/// a nested table, and nothing about indentation tells them apart — both
/// are "deeper than the flag rows above."
///
/// `btrfs --help` (`corpus/btrfs/audit-seed2/help.txt`) has both, back to
/// back. `Options for the main command only:` holds two ordinary flag rows
/// at indent 2 (`--help`, `--version`). A blank line later, at indent 4, a
/// large command table begins — `btrfs balance start [options] <path>`,
/// each row followed by its own description one indent deeper (indent 8):
///
/// ```text
///   --version         print version string
///
///     btrfs balance start [options] <path>
///         Balance chunks across the devices
///     btrfs balance pause <path>
///         Pause running balance
/// ```
///
/// Indentation alone reads every line of that table, and every one of its
/// descriptions, as more of `--version`'s own description — the whole
/// command table folds into one flag.
///
/// # The rule
///
/// A row is counted when a non-blank line sits at exactly `indent` and (a)
/// is not [`looks_like_flag_start`] — a real flag row at this indent is
/// business as usual for the block, not a nested table — and (b) is
/// immediately followed by a non-blank line indented deeper still. The
/// lookahead continues across blank lines and any line at or below
/// `indent`'s depth, stopping the moment a non-blank line dedents past it
/// (structure below `indent` belongs to whatever comes after the table, not
/// to this scan). At least [`MIN_NESTED_TABLE_ROWS`] such rows makes it a
/// table.
///
/// Returning `true` tells [`scan_flags_block`] to `break` at `start` — it
/// **re-routes rather than drops**, the same contract [`bare_block_end`]'s
/// flag-row break uses: the caller resumes its own scan at exactly this
/// line, so a wrong call here loses nothing, it just leaves the text where
/// it was for a later pass to read.
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
/// its own, non-empty description **on its own line**?
///
/// # Why this exists
///
/// [`nested_entry_table_starts_at`] cannot tell apart two shapes that look
/// identical from indentation alone: a nested table that does not belong to
/// the flag above it (break away — `btrfs --help`, whose
/// `--version         print version string` already has its description
/// inline, and the command table below it is not that description), and a
/// value-choice list or keyword list that **is** the flag's description
/// (never break — pngfix's `--strip=[none|crc|unsafe|unused|...]:`
/// and pod2man's `--guesswork=rule[,rule...]` both carry nothing on their
/// own line; everything below, including any run that happens to look
/// table-shaped, is the only description that flag will ever have).
/// Breaking there does not mis-split, it deletes: `--strip` and
/// `--guesswork` both lost their entire description this way before this
/// gate existed, `--guesswork` also fabricating a bogus `choices` list from
/// whatever the parser found past the wrongly-ended block.
///
/// # The rule
///
/// The entry row already has real description text of its own only when a
/// conservative single-column split of that one line ([`split_single_column_entry`],
/// the same split an ungated single-column block would use) yields a
/// non-empty description. A row that instead looks multi-column-shaped
/// ([`fields_in_line`] finds more than one field) is read conservatively as
/// *not* having settled its description yet — this file's block-wide
/// multi-column decision ([`block_is_multi_column`]) isn't available yet
/// mid-scan, so a single-line probe here can't be trusted to say which
/// field, if any, is real; refusing the break is always safe, since the
/// worst case is the same as the pre-fix behaviour these two regressions
/// need to keep.
///
/// Evaluated once per candidate continuation line from the *entry row's own
/// text*, never from the continuation rows accumulated so far — so it gives
/// the same answer at the first continuation line and at the fiftieth, and
/// a description already underway can never be truncated part-way through.
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

    /// `btrfs --help`'s shape (`corpus/btrfs/audit-seed2/help.txt`): a
    /// flags block at indent 2 (`--help`, `--version`), followed by a blank
    /// line and then a nested command table at indent 4 whose own rows are
    /// each followed by a description one indent deeper (indent 8). Before
    /// this test's fix, [`scan_flags_block`]'s continuation rule only
    /// checked "is this line indented past the block's entries?" — true for
    /// every row and every description in the table below — so the entire
    /// table folded into `--version`'s description instead of ending the
    /// flags block.
    ///
    /// Three table groups, deliberately more than the
    /// [`MIN_NESTED_TABLE_ROWS`] floor of two: a single ragged continuation
    /// followed by one deeper line must never trip this detector, only
    /// genuine repetition may.
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

    /// `pngfix --strip`'s shape (`corpus/pngfix/*/help.txt`): the flag row
    /// carries **no inline description at all** — it just ends in `:` — and
    /// everything below it, at one indent deeper, *is* that flag's
    /// description: a value-choice list whose own rows wrap onto a second,
    /// still-deeper physical line for the longer choices (`unsafe`,
    /// `unused`). That wrap is what [`nested_entry_table_starts_at`] misreads
    /// as table rows: two choices whose explanation happens to overflow onto
    /// a deeper-indented continuation line is exactly the "row at `indent`
    /// followed by something deeper" shape it looks for, even though this is
    /// ordinary wrapped prose, not a nested command table. Before the entry-
    /// row gate, the detector fired here too, breaking the flags block right
    /// at the first continuation line and leaving `--strip` with **no
    /// description at all** — the whole choice list, gone, not merely
    /// mis-split. Since the entry row itself has nothing on its own line,
    /// there is nowhere else for this text to go: the break must never
    /// trigger when the row being continued is bare like this.
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

    /// `pod2man --guesswork`'s shape (`corpus/pod2man/*/help.txt`): the flag
    /// row also carries no inline description — an ordinary wrapped
    /// paragraph follows, then (this is the part that trips the detector) a
    /// genuine bare-word keyword list (`functions`, `manref`, `quoting`,
    /// `variables`), each keyword followed by its own explanation one indent
    /// deeper. That keyword list is real repetition, so
    /// [`nested_entry_table_starts_at`] is right that *something* table-
    /// shaped is down there — it is simply wrong that it's a *different*
    /// entry's table rather than more of `--guesswork`'s own description.
    /// Before the entry-row gate this broke the flags block at the very
    /// first continuation line, so `--guesswork` lost its paragraph *and*
    /// its keyword list both — its entire description, gone.
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

    /// sed's `--help` has no `Options:`/`Flags:` heading at all — the
    /// output starts directly with `-n, --quiet, --silent`. This must
    /// still be recovered as a (headingless) flags block, own-line
    /// descriptions and all, not silently dropped or misread as commands.
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

    /// A positional documented as the *first* row of an options table must
    /// not cost the whole table. `kill --help` opens `Options:` with
    /// `<pid> [...]`, and because the flags-vs-bare-words decision read
    /// only that first row, every flag below it was thrown away — measured
    /// at **0 flags**, confirmed by deleting just that row, after which
    /// the same build read 6 flags at 100% described.
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

    /// `ip`'s own `OPTIONS := { -V[ersion] | -s[tatistics] | -d[etails] |
    /// -r[esolve] |` row: every top-level `|`-segment already carries its
    /// own abbreviation bracket, so each one is a complete, self-contained
    /// flag and all four are recovered.
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
    /// alternatives rather than one abbreviated token. A bare short
    /// immediately followed by a bare long folds back into one flag with
    /// both spellings, never two one-spelling fragments.
    #[test]
    fn a_bnf_alternation_row_pairs_bare_short_and_long_spellings() {
        let entries = split_bnf_alternation_row("-V | --Version | -i | --iec | -j | --json")
            .expect("row splits");
        assert_eq!(entries.len(), 3);
        let short = parse_flag_spec(&entries[0].0);
        assert_eq!(short.short, Some('V'));
        assert_eq!(short.long, Some("Version".to_string()));
    }

    /// `sg_sanitize`'s real `--count=OC|-c OC` (from `corpus/sg_sanitize`):
    /// splits into two top-level `|`-segments, and the two are one flag —
    /// an alias plus a shared value — never two, because the second
    /// segment (`-c OC  OC is overwrite count`) leaves real prose
    /// unconsumed and fails `fully_consumed` on its own, independent of
    /// any outer gating.
    ///
    /// This function alone cannot refuse every non-BNF `|`-joined pair,
    /// though — `btrfsck`'s real `-E|--subvol-extents <subvolid>` DOES
    /// split cleanly at this level (both halves are, on their own, a
    /// perfectly good `fully_consumed` flag), which is exactly why the
    /// per-segment checks here are not the whole guard: the caller
    /// ([`scan_flags_block`]) never invokes this function at all unless
    /// the block's own heading came from a `:=` production. See
    /// `a_plain_pipe_joined_flag_row_survives_parse_with_profile_unsplit`
    /// below for that gate exercised end to end, on the real shape that
    /// caught this before the gate existed.
    #[test]
    fn a_row_gluing_one_flags_alias_and_value_through_a_pipe_is_never_split() {
        assert_eq!(
            split_bnf_alternation_row("--count=OC|-c OC  OC is overwrite count"),
            None
        );
    }

    /// `rdma`'s real `-p[retty] -r[aw]}` and `devlink`'s real
    /// `-v[erbose] -s[tatistics] -[he]x`: two or three flags run together
    /// by a bare space inside one `|`-segment (the "missing separator"
    /// shape this reader deliberately refuses). Without the
    /// more-than-one-flag-shaped-word guard, `parse_flag_spec`'s own alias
    /// loop silently swallows the second flag with no leftover text at
    /// all, so `fully_consumed` alone cannot catch it.
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

    /// `vdpa`'s real closing brace, one space after the last flag's own
    /// abbreviation bracket (`-p[retty] }`), and `dcb`'s real closing
    /// bracket glued directly onto a bare long spelling with no space at
    /// all (`--verbose]`) — the enclosing group's own closer landing on
    /// the row's last alternative two different ways. Both must vanish
    /// without being read as that flag's value.
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

    /// End-to-end regression for the false-positive sweep
    /// `split_bnf_alternation_row`'s own doc comment describes: a real
    /// short/long pair joined by `|` outside any `:=` production must
    /// come through `parse_with_profile` completely unsplit, whether or
    /// not the document elsewhere contains a real BNF heading.
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

    /// Reproduces the corruption this shape reader fixes: before it
    /// existed, `find_placeholder_boundary_gap` misread `-size N[bcwkMG]`'s
    /// own bracketed unit suffix as a description boundary and handed
    /// `-wholename` the front of the *next* entries on the line
    /// (`-true -type [bcdpflsD] -uid N`) as a fabricated description.
    /// `-wholename` must come out with its real value (`PATTERN`) and no
    /// description at all — never text belonging to a different flag.
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
}
