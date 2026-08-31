//! The synopsis: recognizing a usage line or an unlabelled stanza head, and
//! mining the synopsis itself for positionals and for flags no option
//! table documents.

use super::*;

/// True if `t` starts with `"usage:"`, case-insensitively.
///
/// Deliberately compares raw bytes rather than doing `&t[..6]` on the
/// `str` (which panics if byte offset 6 doesn't land on a UTF-8 character
/// boundary — a real crash the coverage harness found: some real-world
/// `--help` output puts a multi-byte character, e.g. a box-drawing glyph,
/// early in the first line). `[u8]::get` is bounds-checked and never
/// panics, and comparing ASCII bytes needs no UTF-8 decoding at all.
pub fn starts_with_usage_prefix(t: &str) -> bool {
    t.as_bytes()
        .get(..6)
        .map(|b| b.eq_ignore_ascii_case(b"usage:"))
        .unwrap_or(false)
}

/// True if `t` starts with `"or:"`, case-insensitively — GNU coreutils'
/// marker for a genuine *alternative* invocation form (`du`'s `Usage: du
/// [OPTION]... [FILE]...` / `  or:  du [OPTION]... --files0-from=F`), as
/// distinct from a wrapped continuation of the form above it. This is the
/// over-join guard: without recognizing this marker, a rule that joins
/// every more-indented line in the usage block onto the entry above it
/// (correct for a wrapped synopsis) would also swallow `or:`'s alternative
/// form, silently merging two real invocations into one and losing the
/// fact that `du` can be invoked either way.
///
/// Same bounds-checked byte comparison as [`starts_with_usage_prefix`], for
/// the same reason: never slice a `&str` derived from tool output at a raw
/// offset.
pub fn starts_with_or_marker(t: &str) -> bool {
    t.as_bytes()
        .get(..3)
        .map(|b| b.eq_ignore_ascii_case(b"or:"))
        .unwrap_or(false)
}

/// True if `t` (already trimmed of leading whitespace) begins with `name`
/// at a word boundary — either exactly `name`, or `name` followed by
/// whitespace. The "starts with the tool's own name" half of the
/// usage-block continuation discriminator (see the block's own comment):
/// a tool that lists alternative invocation forms *without* an `or:`/
/// `usage:` marker, by literally repeating itself —
///
/// ```text
/// Usage: prog foo
///        prog bar
/// ```
///
/// — must still read as two entries, not one continuation swallowing the
/// other.
///
/// Word-boundary checked (via `str::strip_prefix`, not a raw byte slice)
/// so a tool named `git` doesn't also claim a line that happens to start
/// with `gitk` or `git-foo`.
pub fn starts_with_tool_name(t: &str, name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    match t.strip_prefix(name) {
        Some(rest) => rest.is_empty() || rest.starts_with(char::is_whitespace),
        None => false,
    }
}

/// True if `t` (already trimmed of leading whitespace) is the C
/// `fprintf(stderr, "%s: Usage: ...", argv[0])` idiom's line: the tool's
/// own name, then a literal `": "`, then `usage:` case-insensitively —
/// `nfsidmap`'s `nfsidmap: Usage: nfsidmap [-vh] [-c || ...]`.
///
/// [`starts_with_usage_prefix`] tests the line's *start*, so a `usage:`
/// preceded by the program's own name (this ordinary `fprintf`
/// convention, framework-general rather than per-tool) is invisible to
/// it, and the whole document was previously rendered `verbatim` with
/// zero flags recovered.
///
/// Kept deliberately tight, per this fix's own hazard warning: the
/// `usage:` must be preceded by *only* the tool's own name and `": "` —
/// not scanned for anywhere inside the line, and not satisfied by the
/// name alone (an ordinary sentence starting `nfsidmap: ` followed by
/// prose must never match).
pub fn starts_with_name_prefixed_usage(t: &str, name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    t.strip_prefix(name)
        .and_then(|rest| rest.strip_prefix(": "))
        .is_some_and(starts_with_usage_prefix)
}

/// True if `t` (already trimmed of leading whitespace) opens with `name`
/// at a word boundary and its remainder reads as usage-synopsis grammar
/// rather than English prose — the **unlabelled** synopsis convention some
/// tools use in place of any `Usage:` line at all: `wpa_cli --help` simply
/// opens `wpa_cli [-p<path to ctrl sockets>] [-i<ifname>] [-hvBr] ...`,
/// with no marker anywhere.
///
/// This predicate is the entire risk this fix carries (its own doc
/// comment at the call site explains the guardrails around *where* it is
/// tried). A name match alone is not evidence of a synopsis — `"tar is an
/// archiving program that creates..."` starts with `tar` too — so two
/// independent, purely notational signals are both required:
///
/// - The remainder must contain at least one of the docopt-style group
///   delimiters spec §7 names (`[`, `<`, `{`) — the same notation
///   [`looks_like_usage_fragment`] keys on for usage-block continuation.
///   Prose describing a tool essentially never carries these characters;
///   a synopsis is built almost entirely out of them.
/// - The remainder must not read as an English sentence, reusing
///   [`is_prose_sentence`]'s own test (period-terminated, several words,
///   no multi-space column gap) — so a tool whose leading sentence
///   happens to mention a bracketed aside is still refused.
///
/// Measured on a full-`PATH` sweep before landing (see this fix's PR
/// description for the exact tool list this predicate moves).
pub fn looks_like_unlabeled_synopsis_line(t: &str, name: &str) -> bool {
    let Some(rest) = t.strip_prefix(name) else {
        return false;
    };
    if !(rest.is_empty() || rest.starts_with(char::is_whitespace)) {
        return false;
    }
    let rest = rest.trim_start();
    if rest.is_empty() {
        return false;
    }
    rest.contains(['[', '<', '{']) && !is_prose_sentence(rest)
}

/// True if `lines[idx]` is a bare own-name invocation line — no bracket
/// notation on the line itself — whose *very next* physical line is
/// unambiguous flag-row evidence: either [`looks_like_bracket_flag_row`]
/// (`vgck`'s `[ -d|--debug ]`) or [`looks_like_paren_alternation_open`]
/// (`vgchange`'s `( -l|--logicalvolume Number,`, the "any one of these is
/// required" convention). Shared by the unlabelled-synopsis entry point
/// (this file's `unlabelled_synopsis_start`) and the multi-stanza
/// continuation check in the usage-block loop below it — both need exactly
/// this one test, and a second copy would drift.
///
/// Narrow and structural, never keyed on any tool's name: a name-only line
/// only counts when the next line is itself unambiguous flag-row notation,
/// never merely because it comes right after a name match. LVM's own
/// emitter (`vgck`, `vgck --updatemetadata VG`, `vgextend VG PV ...`,
/// `vgchange`'s own paren-alternation first stanza) is the specimen this
/// was measured against, not a special case for it.
pub(super) fn looks_like_bare_synopsis_head(lines: &[&str], idx: usize, name: &str) -> bool {
    let t = lines[idx].trim_start();
    starts_with_tool_name(t, name)
        && lines.get(idx + 1).is_some_and(|next| {
            let next = next.trim_start();
            looks_like_bracket_flag_row(next) || looks_like_paren_alternation_open(next)
        })
}

/// True if `lines[idx]` continues an already-open unlabelled synopsis into
/// a **later stanza**: a line opening with the tool's own name whose
/// remainder either carries a bare flag token directly (`vgck
/// --updatemetadata VG` — the flag notation itself, no bracket group at
/// all) or is followed by [`looks_like_bracket_flag_row`] evidence the same
/// way [`looks_like_bare_synopsis_head`] requires for the *first*
/// stanza. LVM's own emitter is the specimen throughout this doc comment;
/// nothing here keys on it — `adduser` and `pydoc3` hit this same
/// predicate on their own multi-stanza shapes (see this function's tests).
///
/// The first-stanza test alone cannot see this shape: `vgck`'s own second
/// stanza reads `vgck --updatemetadata VG` immediately followed by `[
/// COMMON_OPTIONS ]` — a placeholder token, not a flag row
/// ([`looks_like_bracket_flag_row`] requires the bracketed content to
/// *start* with `-`), so the next-line lookahead alone finds nothing here.
/// But the stanza head carries its own flag inline, which is strictly
/// stronger evidence than a lookahead row ever was — [`extract_usage_flags`]
/// already knows how to read a bare `--flag` sitting directly in a usage
/// line (the [M-15] grammar), so once this predicate admits the line as a
/// synopsis head, the existing machinery below recovers `--updatemetadata`
/// with no further change.
///
/// Deliberately a separate predicate from [`looks_like_bare_synopsis_head`]
/// rather than a widened copy of it: the entry point that opens the usage
/// block in the first place must stay exactly as measured (a name-only
/// line is accepted only on unambiguous *next-row* evidence), and loosening
/// it to "or carries its own flag token" would let it fire on a bare
/// tool-name-prefixed sentence that happens to mention a flag in prose —
/// a hazard the entry point's own doc comment already warns about for the
/// weaker, notation-only test. The continuation site carries no such risk:
/// it only ever runs immediately after a blank line inside an *already
/// open* unlabelled synopsis, never as a fresh scan of the whole document.
pub(super) fn looks_like_stanza_continuation_head(lines: &[&str], idx: usize, name: &str) -> bool {
    let t = lines[idx].trim_start();
    let Some(rest) = t.strip_prefix(name) else {
        return false;
    };
    if !(rest.is_empty() || rest.starts_with(char::is_whitespace)) {
        return false;
    }
    let next = lines.get(idx + 1).map(|l| l.trim_start());
    let has_flag_token = rest.split_whitespace().any(is_bare_flag_token);
    let next_is_bracket_row = next.is_some_and(looks_like_bracket_flag_row);
    if !(has_flag_token || next_is_bracket_row) {
        return false;
    }
    // Guard against admitting a stanza whose own description **wraps
    // across more than one physical line**: the shared continuation loop
    // just below only ever recognizes a single physical line as "prose to
    // drop" ([`is_prose_sentence`] requires the line itself to end in a
    // period), so a description spanning two or more lines has its
    // interior line silently read as more usage notation instead — never
    // caught, because nothing about an unterminated mid-sentence line
    // says "not notation". `pydoc3`'s `-p`/`-b`/`-w` forms each carry a
    // two-line description ("Start an HTTP server on the given port on
    // the local machine.  Port" / "number 0 can be used ..."), and the
    // first line, not ending in a period, was silently glued onto the
    // synopsis text and mined for two fabricated positionals (`HTTP`,
    // `HTML`) before this guard existed. Refuse the stanza outright
    // rather than admit it and let it corrupt: whatever immediately
    // follows the head must be either absent/blank (no description at
    // all), itself more notation ([`looks_like_bracket_flag_row`] /
    // [`looks_like_usage_fragment`]), or a complete one-line sentence the
    // loop already knows how to drop — never an unterminated prose
    // fragment. `pydoc3`'s own `-k`/`-n` forms (each a single-line
    // description) pass this exactly the way `vgck`'s `[ COMMON_OPTIONS
    // ]` continuation and `lvextend`'s bracket rows do.
    match next {
        None => true,
        Some(n) if n.trim().is_empty() => true,
        Some(n) if looks_like_bracket_flag_row(n) || looks_like_usage_fragment(n) => true,
        Some(n) if is_prose_sentence(n) => true,
        _ => false,
    }
}

/// True if `t` (already trimmed of leading whitespace) opens with one of
/// the docopt-style usage grammar's own group delimiters — spec §7 Tier B:
/// "`[OPTIONS]`, `<required>`, `[optional]`, `...` for repetition, `|` for
/// alternatives, `{a|b|c}` for choices." A line that opens this way still
/// reads as more invocation syntax, not the next section starting.
///
/// This is the content-shape half of the usage-block continuation
/// discriminator, needed because indentation alone cannot separate a
/// genuine same-indent continuation (`lsof`'s `[-F [f]] [-g [s]] ...`,
/// wrapped at the exact same column as its own `usage:` marker) from a
/// same-indent line that legitimately ends the block (`du`'s trailing
/// "Summarize device usage of the set of FILEs..." sentence, immediately
/// after its `or:` line with no blank separator) — both sit at or below
/// the block's base indent, and neither is a marker or the tool's own
/// name, so only their content tells them apart: one is bracket/
/// angle-bracket syntax, the other is an English sentence starting with a
/// capital word.
pub(super) fn looks_like_usage_fragment(t: &str) -> bool {
    matches!(t.as_bytes().first(), Some(b'[') | Some(b'<') | Some(b'{'))
}

/// Recover the mode-selecting flag a stanza head line itself names —
/// `grammar::looks_like_stanza_head_flag`'s shape — in addition to the
/// heading text still becoming the stanza's `group` label exactly as
/// before this change (`meaningful_flag_group`, called here with the same
/// heading text the flags-block scan beside this call site already passes
/// it, so a stanza's own head flag and its bracket-row flags always agree
/// on `group`).
///
/// `None` for a bare invocation naming no flag at all (`vgchange` alone),
/// for a heading that is not the tool's own name at a word boundary
/// (`starts_with_tool_name`), and for an ignorable heading
/// (`is_ignorable_heading` — an `Examples:`-shaped block's own rows must
/// never be mined for a flag merely because one of them happens to repeat
/// the tool's name). `tool_name` absent (framework or generic parse with no
/// resolved name) also yields `None`; this recovery has no other way to
/// know the head line's own name is the *tool's* name rather than
/// coincidence.
///
/// Never fabricates required-ness: the flag is emitted exactly like any
/// other recovered flag (`required: false`), even though LVM's own prose
/// ("any one is required") makes it semantically mandatory — the IR has no
/// per-group "choose exactly one of N" relation to spend on that.
pub(super) fn recover_stanza_head_flag(heading: &str, tool_name: Option<&str>) -> Option<Entity> {
    let name = tool_name?;
    if is_ignorable_heading(heading) || !starts_with_tool_name(heading, name) {
        return None;
    }
    // `starts_with_tool_name` already confirmed `heading` opens with `name`
    // at a word boundary, so `strip_prefix` here cannot fail — used instead
    // of a raw byte-offset slice (AGENTS.md's UTF-8 boundary rule) even
    // though `name` is always ASCII in practice.
    let rest = heading.strip_prefix(name)?;
    let rest = rest.trim_start();
    if !looks_like_stanza_head_flag(rest) {
        return None;
    }
    let spec = parse_flag_spec(rest);
    if spec.spellings.is_empty() {
        return None;
    }
    let mut flag = Entity::new(EntityKind::Flag, Provenance::single(Source::HelpText));
    flag.spellings = spec.spellings;
    flag.value_name = spec.value_name;
    flag.value_kind = spec.value_kind;
    flag.group = meaningful_flag_group(heading.to_string());
    Some(flag)
}

/// Fewest whitespace-separated words the line above a stanza head must
/// carry before [`stanza_description_above`] adopts it as that stanza's
/// label.
///
/// Three, deliberately not [`MIN_PROSE_SENTENCE_WORDS`]'s five. The two
/// numbers answer different questions and are not interchangeable. Five
/// answers "is this indentation-promoted line prose rather than a section
/// heading?", asked of any line anywhere in a document, where a two- or
/// three-word *heading* is the thing that must never be claimed. This one
/// is asked in a single, fully-bracketed slot — a lone line between a
/// blank and a confirmed stanza head — where a heading and a description
/// both name the block below and either is a better label than the
/// invocation line. What the floor still has to keep out is a one- or
/// two-word fragment, and three is the shortest real specimen in the
/// measured family: `vgchange`'s own `Activate or deactivate LVs.` is
/// four words, and a five-word floor would leave that one stanza — the
/// tool's most-used mode — labelled by its head line while its five
/// siblings carried their descriptions.
pub(super) const MIN_STANZA_DESCRIPTION_WORDS: usize = 3;

/// The description sentence a multi-variant tool writes directly above a
/// usage stanza's head line, when `lines[head_idx]` is such a head and the
/// line above it is such a sentence — LVM's own emitter, one stanza per
/// invocation form:
///
/// ```text
///   Start the lockspace of a shared VG in lvmlockd.
///   vgchange --lockstart
/// \t[ -S|--select String ]
/// \t[ COMMON_OPTIONS ]
/// ```
///
/// # The defect
///
/// The section loop reads `vgchange --lockstart` as the heading governing
/// the bracket rows beneath it, so every flag in the stanza takes that
/// head line as its [`mandible_core::Entity::group`] and the pane draws
/// `Vgchange --lockstart ─────`. The sentence above it — the only
/// human-meaningful thing the tool says about this invocation form — is
/// consumed by nothing and dropped outright. A divider that repeats the
/// spelling already printed on the row beneath it names the group with
/// information the reader can already see, while the sentence that would
/// have told them what the mode *does* is not in the tree at all.
///
/// # The rule, and what each clause keeps out
///
/// The head is the anchor, and it is the strongest one available:
/// [`recover_stanza_head_flag`] must already have accepted
/// `lines[head_idx]` as a stanza head — the tool's own name at a word
/// boundary followed by exactly one bare flag token
/// ([`looks_like_stanza_head_flag`]), never an ignorable heading. Nothing
/// here is tried against an ordinary heading, so the question is only ever
/// asked about a line that is already known to open an invocation form.
/// Given that anchor, the line above must:
///
/// - **Sit at the head's own column.** A more-indented line is the head's
///   own content and a less-indented one governs the head rather than
///   describing it; only a line the author set flush with the head is
///   writing about that head.
/// - **Stand alone** — the line above *it* is blank, or absent. This is
///   the anti-paragraph clause: a description that hard-wraps, or a
///   trailing sentence of an unrelated paragraph that happens to end just
///   above a stanza, has a non-blank neighbour above it, and adopting its
///   last physical line would label the group with half a sentence.
///   Refuse the whole shape rather than take the fragment.
/// - **End in a full stop, and not an ellipsis** — the same terminator
///   test [`is_prose_sentence`] uses, for the same two reasons: a label
///   the author wrote as a label does not end in one, and a trailing
///   `...` is docopt repetition notation rather than a sentence.
/// - **Be a single field** ([`find_multi_space_gap`]) — a line with an
///   aligned column is a table row, not a sentence.
/// - **Not open with the tool's own name** ([`starts_with_tool_name`]) —
///   so a stanza head that happens to end in a period can never become
///   the label of the stanza beneath it, and neither can a worked-example
///   invocation.
/// - **Not open with flag or usage notation** — belt and braces beside
///   the terminator test, so a bracket row or a flag line is refused on
///   its shape as well as its punctuation.
/// - **Carry at least [`MIN_STANZA_DESCRIPTION_WORDS`] words**, and not
///   be an [`is_ignorable_heading`] marker.
///
/// Returns the sentence exactly as the tool wrote it, terminator included
/// — the display layer strips a label's source punctuation (spec §9.3),
/// the same way it already strips a heading's trailing colon.
pub(super) fn stanza_description_above<'a>(
    lines: &[&'a str],
    head_idx: usize,
    tool_name: Option<&str>,
) -> Option<&'a str> {
    let name = tool_name?;
    if head_idx == 0 {
        return None;
    }
    recover_stanza_head_flag(lines[head_idx].trim(), Some(name))?;
    if head_idx >= 2 && !lines[head_idx - 2].trim().is_empty() {
        return None;
    }
    let raw = lines[head_idx - 1];
    if leading_whitespace(raw) != leading_whitespace(lines[head_idx]) {
        return None;
    }
    let text = raw.trim();
    if text.is_empty() || !text.ends_with('.') || text.ends_with("...") {
        return None;
    }
    if text.split_whitespace().count() < MIN_STANZA_DESCRIPTION_WORDS {
        return None;
    }
    if find_multi_space_gap(raw).is_some() {
        return None;
    }
    if starts_with_tool_name(text, name)
        || looks_like_flag_start(text)
        || looks_like_usage_fragment(text)
        || is_ignorable_heading(text)
    {
        return None;
    }
    Some(text)
}

/// Pull placeholder tokens (`<value>`, bare `UPPERCASE` words not preceded
/// by `-`) out of usage lines as positionals. Best-effort: usage-line
/// grammar is genuinely varied (docopt-style `[OPTIONS]`, `<required>`,
/// `...`, `|`, `{a|b|c}`), so this recognizes the common placeholder
/// shapes rather than fully parsing the grammar.
///
/// What it recognizes is *inference from notation*, so it stays narrow, and
/// [`OPTION_LIST_PLACEHOLDERS`] carves out the one family of tokens whose
/// notation is indistinguishable from an operand's while its meaning is the
/// opposite. The declarative counterpart — a framework's own positional
/// block, which needs no inference at all — is
/// [`FrameworkProfile::positional_heading_markers`] and
/// [`emit_declared_positionals`].
/// The byte-offsets into `usage_lines` of every physical line that was
/// folded into the first recovered usage *entry* carrying real invocation
/// content — skipping a bare `Usage:`/`or:` label with nothing after the
/// colon on its own line (util-linux's `renice`: entry 0 is the literal
/// string `Usage:`, entry 1 — one physical line — is `renice
/// [-n|--priority|--relative] <priority> ...`), and following a wrapped
/// entry across every physical line it spans (`sg_sanitize`'s five-line
/// synopsis is one entry, entry 0, so all five lines qualify here).
///
/// `line_entry_index[i]` is the `entries` index physical line `i` was
/// folded into ([`parse_with_profile`]'s own bookkeeping, threaded through
/// because `extract_positionals` only ever sees the flattened
/// `usage_lines`, not which entry each line belongs to).
///
/// This is [`extract_positionals`]'s anchor for *where* the self-closed-
/// bracket-group refinement below is allowed to run — see that function's
/// own doc comment for why the refinement is scoped to exactly these lines
/// rather than every line.
pub(super) fn primary_synopsis_lines(
    entries: &[String],
    line_entry_index: &[usize],
    line_count: usize,
) -> std::collections::HashSet<usize> {
    let primary_entry = entries.iter().position(|e| {
        let bare_label = e.trim().trim_end_matches(':').eq_ignore_ascii_case("usage")
            || e.trim().trim_end_matches(':').eq_ignore_ascii_case("or");
        !bare_label
    });
    let Some(primary_entry) = primary_entry else {
        return std::collections::HashSet::new();
    };
    (0..line_count)
        .filter(|&i| line_entry_index.get(i) == Some(&primary_entry))
        .collect()
}

/// `usage_lines`: every physical line of the recovered usage block, in
/// source order. `primary_lines`: [`primary_synopsis_lines`]'s pick of
/// which of them (by index) make up the tool's primary invocation form —
/// the self-closed-bracket-group refinement below only ever runs on one of
/// those; see its own comment for why.
pub(super) fn extract_positionals(
    usage_lines: &[String],
    primary_lines: std::collections::HashSet<usize>,
) -> Vec<Entity> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for (line_idx, line) in usage_lines.iter().enumerate() {
        // A value-shaped token immediately following a bare flag token
        // (`-C <path>`, `-c <name>=<value>`, argparse's `--config FILE`)
        // is that flag's *argument*, not a positional — a property of
        // usage-synopsis notation generally, true for every framework that
        // writes an option's value right after it rather than gluing it on
        // with `=`. `prev_cleaned` tracks the immediately preceding
        // token's cleaned spelling so the loop below can tell the two
        // apart; it resets every physical line, since a usage line's
        // tokens never continue onto the next one.
        let mut prev_cleaned: Option<&str> = None;
        // Whether the *immediately preceding raw token* was itself a
        // complete, self-closed bracket group (`[-v]`, `[-h]`) — opened
        // and closed with nothing else inside. A flag written that way has
        // already said everything about itself the notation can say; it is
        // never still waiting on a following token the way a *still-open*
        // group (`[-C`, expecting `<path>]` next) or a bare flag with no
        // brackets at all (`-C`, expecting `<path>` next) is.
        //
        // `sg_emc_trespass`'s own synopsis is the case that forced this:
        // `[-d] [-hr] [-s] [-V] DEVICE` closes `[-V]` completely before
        // `DEVICE` ever appears, so `DEVICE` is a real positional, not
        // `-V`'s argument — but the untracked version of this rule stripped
        // brackets off both ends of every token alike, so `[-v]` and a
        // genuinely value-expecting `-C` looked identical, and `DEVICE`
        // was silently swallowed as a fabricated flag argument (dropped
        // outright, no `positionals` entry at all, on a tool with no
        // declared positional block to fall back on).
        //
        // **Scoped to [`primary_synopsis_lines`], deliberately.**
        // A fleet sweep applying this fleet-wide moved two other shapes
        // this fix is not entitled to claim, both real:
        //
        // 1. A *later* alternate invocation form that repeats the tool's
        //    own name (`jps`'s second line, `jps [-q] [-mlvV] [<hostid>]`,
        //    under a first line that already carried its own content —
        //    `Usage: jps [--help]`). `xtask`'s existence oracle attests
        //    operands only from a line it recognizes as synopsis grammar,
        //    and it does not (yet) read a same-name repeat under that
        //    shape as one — a real, narrower gap than this fix owns.
        // 2. An **unlabelled** synopsis (no `Usage:`/`or:` anywhere in the
        //    document at all — `lvreduce`'s bare `lvreduce -L|--size
        //    [-]Size[m|UNIT] LV`, `dbus-cleanup-sockets`'s bare
        //    `dbus-cleanup-sockets [--version] [--help] <socketdir>`). The
        //    oracle's synopsis scanner does not read this convention at
        //    all yet, labelled or not, so *any* operand recovered from it
        //    — by this fix or any other — currently reports as invented.
        //
        // Both were measured on a full-`PATH` sweep (9 tools) and are
        // deliberately left as they were before this fix rather than
        // shipped as new false alarms in `xtask`'s own oracle; the primary
        // line is the one shape the oracle already attests correctly, and
        // it is also every real case this fix was written for
        // (`sg_emc_trespass`, `scsi_ready`'s whole `sg3-utils` family,
        // `lzgrep`/`xzgrep`, and `renice`'s own primary line, which is
        // entry 1 there, not entry 0 — see [`primary_synopsis_lines`]).
        let self_closed_recovery_applies = primary_lines.contains(&line_idx);
        let mut prev_was_self_closed_group = false;
        for token in line.split_whitespace() {
            let cleaned = token.trim_matches(|c| c == '[' || c == ']' || c == '.');
            // A flag already carrying its value inline (`--git-dir=<path>`)
            // has an `=` in `cleaned` and does not expect a following
            // token; a bare flag (`-C`, `-Zscript`) does — unless it was
            // already closed as its own complete bracket group, on the
            // one line this refinement is scoped to.
            let consumed_by_prior_flag = prev_cleaned
                .is_some_and(|p| p.starts_with('-') && !p.contains('='))
                && !(self_closed_recovery_applies && prev_was_self_closed_group);
            prev_cleaned = Some(cleaned);
            prev_was_self_closed_group = token.starts_with('[') && token.ends_with(']');

            if cleaned.starts_with('-') || consumed_by_prior_flag {
                continue;
            }
            let (name, variadic) = if let Some(stripped) = cleaned.strip_prefix('<') {
                // The *nearest* closing `>`, not the outermost one:
                // `<name>=<value>` (git's `-c <name>=<value>`, when not
                // already excluded above as a flag's own argument) must
                // yield `name`, not `name>=<value` from stripping only the
                // token's very last `>`.
                match stripped.find('>') {
                    Some(end) => (stripped[..end].to_string(), token.ends_with("...")),
                    None => continue,
                }
            } else if cleaned.chars().all(|c| c.is_uppercase() || c == '_') && cleaned.len() > 1 {
                (cleaned.to_string(), token.ends_with("..."))
            } else {
                continue;
            };
            if name.is_empty() || is_option_list_placeholder(&name) || !seen.insert(name.clone()) {
                continue;
            }
            let required = !token.contains('[') && !line.contains(&format!("[{token}"));
            let mut positional = Entity::positional(name, Provenance::single(Source::HelpText));
            positional.required = required;
            positional.repeatable = variadic;
            out.push(positional);
        }
    }
    out
}

/// Extract flag spellings from a usage-synopsis block (spec [M-15]:
/// "378 of 1,895 `ok` tools carry no flags at all", because usage-only
/// options — `git --help`'s `[-p | --paginate | -P | --no-pager]` and
/// friends — were never mined at all; [`extract_positionals`] (above)
/// reads the same block for positionals only, and nothing else reads it
/// for anything.
///
/// **The anti-fabrication property this relies on: a synopsis token
/// becomes a flag only if it starts with `-`.** That single character
/// class is the whole guard — there is no heading to misjudge, no
/// column-alignment ambiguity, no bare-word block that might be prose
/// (the failure mode [M-10] came in through four different ways in the
/// section-block scanner above). Prose cannot enter through a `-` prefix,
/// so this stays resistant to [M-10] by construction. Do not relax it to
/// recognize more shapes; spec §7 Tier B's rule is unconditional: never
/// fabricate.
///
/// Flags recovered here carry **no description** — a usage line documents
/// spellings and value shapes, never prose, and inventing one (by copying
/// the usage line's own text, or a neighbouring flag's description) is
/// exactly the fabrication spec §7 Tier B forbids. Reconciling a
/// same-spelling flag that *does* have a description (from an `Options:`-
/// style block elsewhere in the same output) is [`parse_with_profile`]'s
/// job, via [`flag_spelling_already_present`] — see that function's doc
/// comment for why a duplicate is *dropped* rather than merged.
pub(super) fn extract_usage_flags(usage_lines: &[String]) -> Vec<Entity> {
    let mut out: Vec<Entity> = Vec::new();
    // Running depth of an open parenthesized alternation group (LVM's
    // "for options listed in parentheses, any one is required" convention),
    // tracked over these same physical lines the same way `parse_body`'s
    // own usage-block loop already tracked it while collecting them
    // (`grammar::paren_depth_delta`) — re-derived here rather than passed
    // in because `usage_lines` alone determines the same open/close
    // boundaries deterministically. Needed because a member row routinely
    // opens with `-` itself (`-p|--maxphysicalvolumes Number,`), which must
    // be read via `paren_alternation_member_content`, not the ordinary
    // segment walk below — that walk has no notion of a comma-terminated
    // alternative and would mis-tokenize it (`,` is not a recognized
    // separator anywhere in `usage_segments`).
    let mut paren_group_depth: i32 = 0;
    for line in usage_lines {
        let trimmed = line.trim();
        if paren_group_depth > 0 || looks_like_paren_alternation_open(trimmed) {
            paren_group_depth += paren_depth_delta(trimmed);
            if paren_group_depth < 0 {
                paren_group_depth = 0;
            }
            if let Some(content) = paren_alternation_member_content(trimmed) {
                push_usage_flag(&mut out, parse_flag_spec(content));
            }
            continue;
        }
        // A whole line that is one docopt bracket-group flag row (LVM's
        // `[ -A|--autobackup y|n ]`) is read directly by
        // `bracket_flag_row_content` + `parse_flag_spec`, never by the
        // generic segment walk below. `usage_segments` splits a group's
        // content on every top-level `|` unconditionally
        // (`split_top_level_pipe`), which is right for an alternation of
        // whole flags (`{-v|--version}`) but wrong here: it would read
        // `-A|--autobackup y|n` as three alternatives — `-A`,
        // `--autobackup y`, `n` — losing `--autobackup`'s real value
        // `y|n` down to just `y`. `parse_flag_spec` already resolves this
        // exact alias-vs-value ambiguity correctly (see
        // `bracket_flag_row_content`'s own doc comment), so this row
        // shape is diverted to it before the segment walk ever sees it.
        if let Some(content) = bracket_flag_row_content(line.trim()) {
            push_usage_flag(&mut out, parse_flag_spec(content));
            continue;
        }
        let segments = usage_segments(line);
        let mut seg_idx = 0usize;
        while seg_idx < segments.len() {
            if out.len() >= MAX_RECOVERED_ENTRIES {
                return out;
            }
            let segment = segments[seg_idx].clone();
            seg_idx += 1;
            match segment {
                UsageSegment::Group(members) => {
                    let mut flaggy: Vec<&str> = Vec::new();
                    for m in members {
                        if m.starts_with('-') {
                            flaggy.push(m);
                            continue;
                        }
                        // A member that is *itself* a delimited alternation
                        // of flag spellings, optionally followed by one
                        // shared operand — `xfs_io`'s `[[-c|-C] cmd]...`,
                        // whose outer group has exactly this one member.
                        // Neither `-c` nor `-C` reached the tree at all
                        // before this arm existed: the member does not start
                        // with `-`, so the filter above dropped it whole.
                        for spec in nested_alternation_specs(m) {
                            if out.len() >= MAX_RECOVERED_ENTRIES {
                                return out;
                            }
                            push_usage_flag(&mut out, spec);
                        }
                    }
                    // spec [M-15]'s conservative-pairing rule: within one
                    // bracket group, pair a short with a long only when the
                    // group has exactly one of each. `[-v | --version]`
                    // qualifies; `[-p | --paginate | -P | --no-pager]`
                    // (four alternatives) does not, and every spelling in
                    // it is emitted on its own rather than guessing which
                    // short goes with which long. A wrong pairing asserts a
                    // false equivalence a user would act on — worse than an
                    // unpaired entry, which is merely incomplete.
                    // A bundle is never one half of an alternation pair:
                    // `pair_short_and_long` would happily take `-2CDlNuVv`
                    // as the "short" side (it has a short and no long) and
                    // silently discard seven flags, so the cluster question
                    // is asked first.
                    if flaggy.len() == 2 && flaggy.iter().all(|m| parse_bundled_shorts(m).is_none())
                    {
                        let a = parse_flag_spec(flaggy[0]);
                        let b = parse_flag_spec(flaggy[1]);
                        if let Some(paired) = pair_short_and_long(a, b) {
                            push_usage_flag(&mut out, paired);
                            continue;
                        }
                    }
                    for m in flaggy {
                        push_usage_token(&mut out, m);
                    }
                }
                UsageSegment::Bare(tok) => {
                    if tok.starts_with('-') {
                        // A mandatory flag some tool's synopsis writes
                        // unbracketed (`ssh-keygen -D pkcs11`, `-M generate`,
                        // `-I certificate_identity`) is two bare tokens in a
                        // row: the flag, then its own required value with no
                        // separating group at all. `usage_segments` already
                        // pairs a flag with its value when a `[...]` group
                        // holds both (`bracket_flag_row_content`, the `Group`
                        // arm above); outside a group each bare token used to
                        // stand alone, so the flag's own value token was read
                        // as an unrelated, silently-dropped bare word and the
                        // flag came out looking like a boolean it isn't.
                        //
                        // Attaching is refused when `tok` is itself a
                        // recognized bundle of single-character switches
                        // (`parse_bundled_shorts`) — a bundle's members are
                        // booleans by construction, and the word that
                        // follows one in a synopsis is the next independent
                        // token, never a shared value. It is also refused
                        // when the next segment is missing, empty, or is
                        // itself flag-shaped (`-k -f krl_file`: `-k` stays a
                        // bare boolean because what follows it is another
                        // flag, not a value). It is also refused when the
                        // next word opens a parenthetical aside —
                        // `iptables`'s own `iptables -h (print this help
                        // information)` measured this exactly: `-h` takes no
                        // argument at all, and `(print` is the first word of
                        // a parenthetical explanation, not a value. No real
                        // value-placeholder convention this grammar
                        // recognizes anywhere else opens with `(` (`<value>`,
                        // `[value]`, `{a|b}`, or a bare word are the whole
                        // set), so refusing it costs no real recall.
                        let attach_value = parse_bundled_shorts(tok).is_none()
                            && matches!(
                                segments.get(seg_idx),
                                Some(UsageSegment::Bare(value))
                                    if !value.is_empty()
                                        && !value.starts_with('-')
                                        && !value.starts_with('(')
                            );
                        if attach_value {
                            if let Some(UsageSegment::Bare(value)) = segments.get(seg_idx) {
                                push_usage_flag(
                                    &mut out,
                                    parse_flag_spec(&format!("{tok} {value}")),
                                );
                            }
                            seg_idx += 1;
                            continue;
                        }
                        push_usage_token(&mut out, tok);
                    }
                }
            }
        }
    }
    out
}

/// The fewest alternatives a *nested* group must carry before
/// [`nested_alternation_specs`] will read it as one.
///
/// Two, and unlike `grammar::looks_like_flag_start`'s floor of one this is
/// not a judgment call about ambiguity — it is a statement about who already
/// owns the shape. A one-member nested group is `[[-v] file]`, an ordinary
/// optional flag inside an outer group, and [`usage_segments`] plus the
/// pairing rule above already read it correctly. Claiming it here would put
/// two rules on one shape for no recall at all.
pub(super) const MIN_NESTED_ALTERNATIVES: usize = 2;

/// Read one member of a usage-synopsis group as a nested alternation of
/// flag spellings sharing a single operand — `xfs_io`'s `[[-c|-C] cmd]...`,
/// where the outer group's only member is the string `[-c|-C] cmd`.
///
/// Returns one [`FlagSpec`] per alternative, each carrying the shared
/// operand as a required value, or the *paired* single spec when the
/// alternatives are exactly one short and one long (spec [M-15]'s
/// conservative-pairing rule, the same one the caller applies to a flat
/// group — `{-i|--input} <file>` and `[-i|--input] <file>` must not disagree
/// about whether they name one flag or two). Empty when the member is not
/// this shape.
///
/// **The operand is refused unless it is one clean token.** `cmd` is taken;
/// anything with a second word, or that is itself flag-shaped, yields no
/// value rather than a guessed one — the alternatives are still emitted,
/// because their spellings are in the tool's own text either way, and
/// dropping real flags to avoid an unsure value spec would be the wrong
/// trade in the other direction. What is never done is inventing a value
/// name out of text this function could not read.
pub(super) fn nested_alternation_specs(member: &str) -> Vec<FlagSpec> {
    let Some(alt) = parse_flag_alternation(member) else {
        return Vec::new();
    };
    if alt.members.len() < MIN_NESTED_ALTERNATIVES {
        return Vec::new();
    }
    let shared_value = shared_operand(&alt.rest);
    let specs: Vec<FlagSpec> = alt
        .members
        .iter()
        .map(|m| {
            let mut spec = parse_flag_spec(m);
            if let Some(value) = &shared_value {
                spec.value_name = Some(value.clone());
                spec.value_kind = ValueKind::Required;
            }
            spec
        })
        .collect();
    if let [a, b] = specs.as_slice() {
        if let Some(paired) = pair_short_and_long(a.clone(), b.clone()) {
            return vec![paired];
        }
    }
    specs
}

/// The operand a nested alternation's members share, when the text after
/// the group is one clean value token and nothing else.
///
/// `None` for empty text, for anything with a second word, and for a
/// flag-shaped token (`[[-a|-b] -c]` is not an operand, whatever it is).
pub(super) fn shared_operand(rest: &str) -> Option<String> {
    let trimmed = rest.trim();
    if trimmed.is_empty() || trimmed.split_whitespace().nth(1).is_some() {
        return None;
    }
    if is_flag_shaped(trimmed) {
        return None;
    }
    Some(trimmed.to_string())
}

/// True if `candidate` shares a spelling — its short letter, or its long
/// name — with any flag already in `existing`.
///
/// **Deliberately loose in `existing`'s favor.** `candidate` is always a
/// usage-derived flag here (see the one call site in [`parse_with_profile`]),
/// so it never has a description to lose; matching on *either* spelling
/// (not requiring the same combination [`mandible_core::merge::flag_identity`]
/// would key on, which prefers a long name over a short one) is what
/// catches `arptables`' `--insert, -I` row against a bare `-I` mentioned
/// standalone elsewhere in the synopsis — a real duplicate that a stricter,
/// identity-string equality check would miss and add a second, spellingless
/// (no long, no description) time. The cost of the looser match is a
/// forgone enrichment (a usage flag's value shape is never folded into an
/// existing entry that lacks one) in exchange for the guarantee this
/// function exists to provide: an existing flag, right or wrong, is never
/// altered by anything found here — only ever left alone or joined by a
/// new one.
///
/// A one-letter abbreviation bracket also counts as a short-letter match:
/// `ip`'s usage line writes a bare `[ -force ]`, which the ordinary short
/// path reads as `-f` glued to a value `"orce"`; the *table* documents the
/// same flag as `-f[amily]`, which the abbreviation model correctly reads
/// as long-like (`long() == Some("family")`, `short() == None`) — without
/// this arm the table entity's `short()` no longer agrees with the
/// synopsis candidate's `-f`, and the glued-value noise leaks into the
/// tree as a second, spurious flag.
pub(super) fn flag_spelling_already_present(candidate: &Entity, existing: &[Entity]) -> bool {
    existing.iter().any(|f| {
        (candidate.long().is_some() && f.long() == candidate.long())
            || (candidate.short().is_some() && f.short() == candidate.short())
            || (candidate.short().is_some()
                && f.spellings.iter().any(|s| {
                    matches!(s.dashes, Dashes::Single)
                        && s.abbrev == Some(1)
                        && s.name.chars().next() == candidate.short()
                }))
    })
}

/// Push the flag(s) one synopsis token names: either a bundle of
/// single-character boolean switches, one [`Flag`] per member, or — for
/// every other shape — the single flag [`parse_flag_spec`] reads.
///
/// The bundle question is asked *here*, on the synopsis path only, and
/// never inside [`parse_flag_spec`]: an option-*table* row of the identical
/// shape is the GCC/Clang single-dash convention (`-fdump-scos`, `-Wall`,
/// `-Idirectory`), where the glued text genuinely is a value — thousands of
/// correct parses fleet-wide that splitting would destroy. Only a usage
/// synopsis writes a getopt cluster, so only this caller asks. See
/// [`parse_bundled_shorts`] for the five conditions and the two families
/// (single-dash long options, repeated-character flags) that share the
/// cluster's structural fingerprint and must not be split.
///
/// Members are emitted as bare booleans — no value, no description — which
/// is what they are: `[-2CDlNuVv]` says `-2`, `-C`, `-D`, `-l`, `-N`, `-u`,
/// `-V` and `-v` are eight switches and says nothing else about any of
/// them. Fabricating a description from the usage line's own text is the
/// same spec §7 Tier B violation [`extract_usage_flags`] forbids.
pub(super) fn push_usage_token(out: &mut Vec<Entity>, token: &str) {
    if let Some(members) = parse_bundled_shorts(token) {
        for member in members {
            if out.len() >= MAX_RECOVERED_ENTRIES {
                return;
            }
            push_usage_flag(
                out,
                FlagSpec {
                    spellings: vec![Spelling::short(member)],
                    fully_consumed: true,
                    ..FlagSpec::default()
                },
            );
        }
        return;
    }
    push_usage_flag(out, parse_flag_spec(token));
}

/// Turn a [`FlagSpec`] into a [`Flag`] and push it, unless the spec
/// recognized nothing (`short`/`long` both `None` — a stray token like a
/// bare `-` or `--` option terminator). Mirrors `emit_flags`'s field
/// defaults exactly, except `group`/`description` are always `None`: see
/// [`extract_usage_flags`]'s doc comment for why a usage-derived flag must
/// never carry a description. Provenance is [`Source::HelpTextSynopsis`],
/// not the plain [`Source::HelpText`] `emit_flags` uses — same authority
/// (spec §4.4 is unaffected), but a distinct source so spec §13's
/// `pct_flags_with_text` can tell a structurally-undescribable flag apart from
/// one that merely wasn't described.
pub(super) fn push_usage_flag(out: &mut Vec<Entity>, spec: FlagSpec) {
    if spec.spellings.is_empty() {
        return;
    }
    let mut flag = Entity::new(
        EntityKind::Flag,
        Provenance::single(Source::HelpTextSynopsis),
    );
    flag.spellings = spec.spellings;
    flag.value_name = spec.value_name;
    flag.value_kind = spec.value_kind;
    out.push(flag);
}

/// True when `spec`'s `long()`-shaped spelling is a genuine double-dash
/// long option, as opposed to the single-dash abbreviation-bracket shape
/// (`-p[d]`, name `"pd"`) that [`FlagSpec::long`] reads as long-like by
/// the same rule every other consumer of `FlagSpec` shares.
///
/// [`pair_short_and_long`] restricts its "long" side to this — see that
/// function's doc comment for why.
fn long_is_double_dash(spec: &FlagSpec) -> bool {
    spec.spellings
        .iter()
        .any(|s| matches!(s.dashes, Dashes::Double))
}

/// Pair a short-only and a long-only [`FlagSpec`] into one, or refuse
/// (`None`) if they are not exactly complementary (spec [M-15]'s
/// conservative pairing rule, applied by the caller to a bracket group
/// already known to have exactly one flaggy member of each kind).
///
/// The "long" side must be a genuine double-dash spelling
/// ([`long_is_double_dash`]). M-15's rule was measured and validated only
/// against that shape (`[-v | --version]`, one flag's two spellings); the
/// single-dash abbreviation-bracket convention (`-p[d]`, [`FlagSpec::long`]
/// treats it as long-like too) came later and was never part of that
/// evidence. `pppdump`'s own `[-h | -p[d]]` is the counter-example: `-h`
/// prints the dump in hex and `-p[d]` prints it as printable characters
/// (`p`) or both (`pd`) — two semantically distinct flags the tool
/// happened to write as a usage-synopsis alternation, not one flag's two
/// spellings, and pairing them fabricated a merge across distinct flags
/// exactly like `pod2html`'s `--quiet`/`--noquiet`/`--verbose`/`--noverbose`
/// row (the alias-loop defect this same pairing rule was never at risk of
/// on its own, until the abbreviation model widened what `long()` accepts).
/// Refusing here costs no recall: the caller still emits each alternative
/// as its own flag when pairing is refused, so the two real spellings
/// (`-h`, `-p[d]`) reach the tree correctly named, just not merged.
pub(super) fn pair_short_and_long(a: FlagSpec, b: FlagSpec) -> Option<FlagSpec> {
    let (short_spec, long_spec) = if a.short().is_some()
        && a.long().is_none()
        && b.short().is_none()
        && b.long().is_some()
        && long_is_double_dash(&b)
    {
        (a, b)
    } else if b.short().is_some()
        && b.long().is_none()
        && a.short().is_none()
        && a.long().is_some()
        && long_is_double_dash(&a)
    {
        (b, a)
    } else {
        return None;
    };
    let long_had_value = long_spec.value_name.is_some();
    // Short first, then long — the display order every other spelling
    // list in this crate keeps (`-i, --interactive`).
    let mut spellings = short_spec.spellings;
    spellings.extend(long_spec.spellings);
    Some(FlagSpec {
        spellings,
        value_kind: if long_had_value {
            long_spec.value_kind
        } else {
            short_spec.value_kind
        },
        value_name: long_spec.value_name.or(short_spec.value_name),
        fully_consumed: short_spec.fully_consumed && long_spec.fully_consumed,
    })
}

/// One token-level unit of a usage-synopsis line, as [`usage_segments`]
/// walks it: either a bracketed alternation group (spec [M-15]'s pairing
/// rule operates within one such group) or a bare token outside any
/// bracket.
#[derive(Clone)]
pub(super) enum UsageSegment<'a> {
    /// The members of one top-level `[...]` group, already split on `|` at
    /// that group's own nesting depth — so `--exec-path[=<path>]`'s inner
    /// bracket (an optional value spec) is never mistaken for a second
    /// alternative.
    Group(Vec<&'a str>),
    /// A single top-level token outside any bracket (e.g. git's own
    /// `usage:`/`git` at the very start of the line, or a required flag
    /// some tool's synopsis writes unbracketed).
    Bare(&'a str),
}

/// Walk `line` into [`UsageSegment`]s.
///
/// Every substring boundary here comes from `char_indices`, never a raw
/// byte offset (`AGENTS.md`'s slicing rule) — safe even if a usage line
/// happens to carry a multi-byte character, with no separate UTF-8-
/// boundary reasoning required.
pub(super) fn usage_segments(line: &str) -> Vec<UsageSegment<'_>> {
    let chars: Vec<(usize, char)> = line.char_indices().collect();
    let len = chars.len();
    let mut out = Vec::new();
    let mut idx = 0usize;
    while idx < len {
        let (byte_pos, c) = chars[idx];
        if c.is_whitespace() {
            idx += 1;
            continue;
        }
        // `{` opens a group on exactly the same terms as `[`. `eqn`'s
        // synopsis writes `usage: eqn {-v | --version}`, and while the
        // brackets-only version of this loop was running, the spaces around
        // that `|` split it into three bare tokens — `{-v` (discarded, it
        // does not start with `-`), `|`, and `--version}` (parsed as
        // `--version` carrying the literal value `"}"`). A brace group whose
        // members are *not* flag-shaped is unaffected: `{start|stop}` became
        // a bare token that was skipped before and becomes a group with no
        // flaggy member now, which `extract_usage_flags` skips just the same.
        if let Some(close) = group_close_delimiter(c) {
            if let Some((content_range, close_idx)) = matched_group(&chars, idx, c, close) {
                let content = &line[content_range.0..content_range.1];
                out.push(UsageSegment::Group(split_top_level_pipe(content)));
                idx = close_idx + 1;
                continue;
            }
        }
        let mut j = idx;
        while j < len && !chars[j].1.is_whitespace() {
            j += 1;
        }
        let end_byte = if j < len { chars[j].0 } else { line.len() };
        out.push(UsageSegment::Bare(&line[byte_pos..end_byte]));
        idx = j;
    }
    out
}

/// The closing delimiter that matches `c`, when `c` opens a synopsis group
/// at all. The two pairs a usage line uses for grouping; `<`/`>` is
/// deliberately absent, since it delimits a value placeholder and never a
/// group of alternatives.
pub(super) fn group_close_delimiter(c: char) -> Option<char> {
    match c {
        '[' => Some(']'),
        '{' => Some('}'),
        _ => None,
    }
}

/// Find the byte range of the content strictly between `chars[open_idx]`
/// (an `open` delimiter) and its matching `close`, and the char-index of
/// that close — depth aware over that one pair, so
/// `[--exec-path[=<path>]]`'s inner `[...]` (an optional value spec on the
/// one alternative) is consumed as part of the outer group's content
/// instead of closing the group early, and `[[-c|-C] cmd]`'s inner `]` does
/// not end the outer group either. `None` when `open_idx`'s delimiter is
/// never closed (malformed input); the caller falls back to treating it as
/// an ordinary bare token.
pub(super) fn matched_group(
    chars: &[(usize, char)],
    open_idx: usize,
    open: char,
    close: char,
) -> Option<((usize, usize), usize)> {
    let (open_byte, open_c) = chars[open_idx];
    let content_start = open_byte + open_c.len_utf8();
    let mut depth = 1i32;
    let mut j = open_idx + 1;
    while j < chars.len() {
        let (byte_pos, c) = chars[j];
        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return Some(((content_start, byte_pos), j));
            }
        }
        j += 1;
    }
    None
}

/// Split a group's content on `|` at that content's own nesting depth 0, so
/// a nested `[...]`/`{...}` (an optional value spec, or a value alternation,
/// on one of the alternatives) is never itself split on. Empty fragments (a
/// stray leading/trailing `|`, or `||`) are dropped.
///
/// Both delimiter pairs count toward the depth. Counting only brackets read
/// `[--color={always|never}]` as the two alternatives `--color={always` and
/// `never}`, emitting a flag whose value was the fragment `{always`.
pub(super) fn split_top_level_pipe(content: &str) -> Vec<&str> {
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

#[cfg(test)]
mod tests {
    use super::*;

    // --- multi-stanza unlabelled synopsis (fix/multi-stanza-synopsis) ---

    /// `jar --help`'s own `Examples:` block writes a worked, filled-in
    /// invocation with several real flags chained on one line and no
    /// brackets anywhere — `jar --update --file foo.jar --main-class
    /// com.foo.Main --module-version 1.0` — structurally indistinguishable
    /// from a bare stanza head by the flag-count-one shape alone. A plain
    /// English sentence at column 0 earlier in the document (`"restore
    /// individual classes or resources from an archive."`) would otherwise
    /// own the entire more-indented `Examples:` block, hiding the marker
    /// from `is_ignorable_heading`. The obscured-section fence must now
    /// suppress both the chained stanza candidate and its wrapped
    /// `-C foo/ ...` row while preserving the real, described
    /// `--update`/`-u` and `--file`/`-f` rows.
    #[test]
    fn jars_chained_example_invocation_is_never_read_as_a_stanza_head() {
        let help = "jar creates an archive for classes and resources, and can manipulate or\n\
                     restore individual classes or resources from an archive.\n\
                     \n\
                     \x20Examples:\n\
                     \x20jar --update --file foo.jar --main-class com.foo.Main --module-version 1.0\n\
                     \x20\x20\x20\x20-C foo/ module-info.class\n\
                     \n\
                     Main operation mode:\n\
                     \n\
                     \x20-u, --update               Update an existing jar archive\n\
                     \x20-f, --file=FILE             The archive file name\n";
        let parsed = parse_with_profile(help, None, Some("jar"));
        let update_flags: Vec<_> = parsed
            .flags
            .iter()
            .filter(|f| f.long() == Some("update"))
            .collect();
        assert_eq!(update_flags.len(), 1, "flags: {:?}", parsed.flags);
        assert_eq!(update_flags[0].short(), Some('u'));
        assert!(
            update_flags[0].value_name.is_none(),
            "flags: {:?}",
            parsed.flags
        );
        assert!(
            parsed.flags.iter().any(|f| f.long() == Some("file")),
            "flags: {:?}",
            parsed.flags
        );
        assert_eq!(parsed.flags.len(), 2, "flags: {:?}", parsed.flags);
        assert!(
            parsed.flags.iter().all(|flag| flag.short() != Some('C')),
            "flags: {:?}",
            parsed.flags
        );
    }

    /// The complete structural shape around OpenJDK's examples, not only
    /// the chained `--update` row pinned above.  The prose sentence before
    /// each one-space-indented `Examples:` marker must not become the
    /// marker's owner, the wrapped `-C foo/ ...` example rows must never be
    /// emitted as flags, and the real same-indent sections after the second
    /// examples block must still be parsed with their groups intact.
    #[test]
    fn jars_indented_examples_are_contained_before_real_flag_sections() {
        let help = "Usage: jar [OPTION...] [ [--release VERSION] [-C dir] files] ...\n\
                     jar creates an archive for classes and resources, and can manipulate or\n\
                     restore individual classes or resources from an archive.\n\
                     \n\
                     \x20Examples:\n\
                     \x20# Create a modular jar archive, where the module descriptor is located in\n\
                     \x20# classes/module-info.class:\n\
                     \x20jar --create --file foo.jar --main-class com.foo.Main --module-version 1.0\n\
                     \x20\x20\x20\x20\x20-C foo/ classes resources\n\
                     \x20# Update an existing non-modular jar to a modular jar:\n\
                     \x20jar --update --file foo.jar --main-class com.foo.Main --module-version 1.0\n\
                     \x20\x20\x20\x20\x20-C foo/ module-info.class\n\
                     \n\
                     To shorten or simplify the jar command, you can specify arguments in a separate\n\
                     text file and pass it to the jar command with the at sign (@) as a prefix.\n\
                     \n\
                     \x20Examples:\n\
                     \x20# Read additional options and list of class files from the file classes.list\n\
                     \x20jar --create --file my.jar @classes.list\n\
                     \n\
                     \x20Main operation mode:\n\
                     \n\
                     \x20\x20-c, --create               Create the archive\n\
                     \x20\x20-u, --update               Update an existing jar archive\n\
                     \n\
                     \x20Operation modifiers valid in any mode:\n\
                     \n\
                     \x20\x20-C DIR                     Change to the specified directory\n\
                     \x20\x20-f, --file=FILE            The archive file name\n";

        let parsed = parse_with_profile(help, None, Some("jar"));
        assert!(
            parsed.subcommands.is_empty(),
            "nodes: {:?}",
            parsed.subcommands
        );
        assert_eq!(parsed.flags.len(), 4, "flags: {:?}", parsed.flags);

        let change_dir: Vec<_> = parsed
            .flags
            .iter()
            .filter(|flag| flag.short() == Some('C'))
            .collect();
        assert_eq!(change_dir.len(), 1, "flags: {:?}", parsed.flags);
        assert_eq!(change_dir[0].value_name.as_deref(), Some("DIR"));
        assert_eq!(
            change_dir[0].group.as_deref(),
            Some("Operation modifiers valid in any mode:")
        );

        for long in ["create", "update"] {
            let flag = parsed
                .flags
                .iter()
                .find(|flag| flag.long() == Some(long))
                .unwrap_or_else(|| panic!("missing --{long} in {:?}", parsed.flags));
            assert_eq!(flag.group.as_deref(), Some("Main operation mode:"));
        }
        assert!(!parsed.saw_unattributable_content);
        assert_eq!(parsed.confidence, 1.0);
    }

    /// A same-indent, colon-terminated label inside an examples block is
    /// not by itself a new CLI section.  `Input:`/`Output:` are common
    /// worked-example labels and their payload may itself begin with `-`;
    /// reopening on the first generic `X:` line would therefore replace
    /// one fabrication with another.  A positively identified options
    /// section after them must still reopen normal parsing.
    #[test]
    fn labels_inside_indented_examples_do_not_reopen_flag_parsing() {
        let help = "demo explains how its processing pipeline behaves for callers.\n\
                     This sentence introduces the worked examples printed immediately below.\n\
                     \x20Examples:\n\
                     \x20Input:\n\
                     \x20\x20\x20--fake-one VALUE   example input, not a supported option\n\
                     \x20\x20\x20--fake-two VALUE   another example input\n\
                     \x20Output:\n\
                     \x20\x20\x20--fake-result      rendered example output\n\
                     \x20Commands:\n\
                     \x20\x20\x20invented-one       example output, not a subcommand\n\
                     \x20\x20\x20invented-two       another example output\n\
                     \x20Options:\n\
                     \x20\x20\x20--fake-option      one option shown by the example\n\
                     \x20Supported options:\n\
                     \x20\x20\x20--real VALUE       actual supported option\n\
                     \x20\x20\x20--verbose          actual verbosity option\n";

        let parsed = parse_with_profile(help, None, Some("demo"));
        let longs: Vec<_> = parsed.flags.iter().filter_map(|flag| flag.long()).collect();
        assert_eq!(longs, ["real", "verbose"], "flags: {:?}", parsed.flags);
        assert!(
            parsed.subcommands.is_empty(),
            "nodes: {:?}",
            parsed.subcommands
        );
    }

    #[test]
    fn blkids_alternate_usage_form_is_never_read_as_a_stanza_head() {
        let help = "Usage:\n\
                     blkid --label <label> | --uuid <uuid>\n\
                     \n\
                     blkid -p [--match-tag <tag>] [--offset <offset>] [--size <size>]\n\
                     \x20\x20\x20\x20\x20\x20[--output <format>] <dev> ...\n\
                     \n\
                     Low-level probing options:\n\
                     \x20-p, --probe            low-level superblocks probing (bypass cache)\n\
                     \x20-i, --info             gather information about I/O limits\n";
        let parsed = parse_with_profile(help, None, Some("blkid"));
        let p_flags: Vec<_> = parsed
            .flags
            .iter()
            .filter(|f| f.short() == Some('p'))
            .collect();
        assert_eq!(p_flags.len(), 1, "flags: {:?}", parsed.flags);
        assert_eq!(p_flags[0].long(), Some("probe"));
        assert!(
            !parsed.flags.iter().any(|f| f.long() == Some("match-tag")
                || f.value_name.as_deref() == Some("--match-tag <tag>")),
            "flags: {:?}",
            parsed.flags
        );
    }

    /// `vgck --updatemetadata` — a *second* stanza, past the blank line
    /// this fix teaches the usage-block loop to look beyond — is now
    /// present as its own usage entry and its own flag, and the stanza's
    /// own prose head ("Rewrite VG metadata...") lands in neither.
    /// Reuses the existing [`VGCK_HELP`] fixture above (the real capture
    /// at `audit/queue-captures/vgck/0.stdout`) rather than a second copy.
    #[test]
    fn vgck_recovers_the_second_stanza_and_its_updatemetadata_flag() {
        let parsed = parse_with_profile(VGCK_HELP, None, Some("vgck"));
        assert_eq!(parsed.usage.len(), 2, "usage: {:?}", parsed.usage);
        assert!(parsed.usage[1].contains("--updatemetadata"));
        let updatemetadata = flag_named(&parsed, "updatemetadata");
        assert_eq!(updatemetadata.value_name.as_deref(), Some("VG"));
        for u in &parsed.usage {
            assert!(!u.contains("Rewrite VG metadata"));
        }
    }

    /// A tool with a genuine `Usage:` label must be completely unaffected
    /// by the multi-stanza continuation: `git`'s own wrapped hanging-indent
    /// synopsis, followed by an unrelated blank-line-separated paragraph
    /// that happens to open with `git` again, must never be read as a
    /// second usage entry.
    #[test]
    fn labelled_usage_block_does_not_reopen_on_a_later_blank_line() {
        let help = "Usage: git [--version] [--help] <command> [<args>]\n\n\
                     git clone is used to clone repositories.\n\
                     git clone [--bare] <repo>\n\
                     \t[--depth <n>]\n";
        let parsed = parse_with_profile(help, None, Some("git"));
        assert_eq!(parsed.usage.len(), 1, "usage: {:?}", parsed.usage);
    }

    /// A headingless invocation table (`corepack`'s own commander/oclif
    /// style — one `<tool> <subcommand> [flags] ...` row per blank-line-
    /// separated stanza, each followed by its own subcommand description)
    /// must not be reopened into more "usage": that would demote a real
    /// subcommand `scan_headingless_invocation_table` already recovers
    /// into fabricated synopsis text. The `...` ending each row defeats
    /// `is_prose_sentence`'s period check the same way a real LVM
    /// continuation's own remainder would, so this pins the guard that
    /// keeps the two shapes apart at the continuation site specifically
    /// (`looks_like_stanza_continuation_head`, not the wider
    /// `looks_like_unlabeled_synopsis_line`).
    #[test]
    fn headingless_invocation_table_stanzas_are_not_reopened_as_usage() {
        let help = "Corepack - 0.34.6\n\n  $ corepack <command>\n\nGeneral commands\n\n  \
                     corepack disable [--install-directory #0] ...\n    Remove the shims\n\n  \
                     corepack enable [--install-directory #0] ...\n    Add the shims\n";
        let parsed = parse_with_profile(help, None, Some("corepack"));
        // Whatever the pre-existing entry-point behavior does with the
        // first row, the second must never be folded into it as more
        // usage text — this fix must not widen that.
        assert!(
            parsed.usage.iter().all(|u| !u.contains("enable")),
            "usage: {:?}",
            parsed.usage
        );
    }

    /// A stanza whose own description wraps across more than one physical
    /// line (`pydoc3`'s `-p`/`-b`/`-w` forms) must not be *admitted into the
    /// usage block* at all — the shared continuation loop only recognizes a
    /// *single* physical line as prose to drop, so an interior wrapped line
    /// would otherwise be silently read as more usage notation and mined
    /// for fabricated positionals (`HTTP`, `HTML` were invented from
    /// exactly this before the guard existed). A single-line description
    /// (`-k`, `-n`) is unaffected.
    ///
    /// `-p`'s own flag is recovered anyway, by a *different, independent*
    /// path this fix adds (`recover_stanza_head_flag`, via the generic
    /// heading scanner `pydoc3 -p <port>` falls to once the usage block
    /// refuses it) — one that only ever reads the head line's own text,
    /// never the wrapped description beneath it, so it carries none of the
    /// fabrication risk this guard exists for. This is a real, correct
    /// flag (`pydoc3 -p <port>` genuinely starts an HTTP server), not a
    /// regression of the guard: the assertion this test pins is "no
    /// fabricated positional", not "no `-p` flag ever, by any means".
    #[test]
    fn stanza_with_wrapped_multi_line_description_is_refused() {
        let help = "pydoc - the Python documentation tool\n\npydoc3 <name> ...\n    Show text documentation on something.\n\npydoc3 -k <keyword>\n    Search for a keyword in the synopsis lines of all available modules.\n\npydoc3 -p <port>\n    Start an HTTP server on the given port on the local machine.  Port\n    number 0 can be used to get an arbitrary unused port.\n";
        let parsed = parse_with_profile(help, None, Some("pydoc3"));
        assert!(
            parsed.flags.iter().any(|f| f.short() == Some('k')),
            "flags: {:?}",
            parsed.flags
        );
        let p = parsed
            .flags
            .iter()
            .find(|f| f.short() == Some('p'))
            .unwrap_or_else(|| panic!("flags: {:?}", parsed.flags));
        assert_eq!(p.value_name.as_deref(), Some("<port>"));
        assert!(
            !parsed
                .positionals
                .iter()
                .any(|p| p.primary_name() == "HTTP" || p.primary_name() == "HTML"),
            "positionals: {:?}",
            parsed.positionals
        );
    }

    // --- the parenthesized alternation stanza ---------------------------
    //
    // `headingless_invocation_table_stanzas_are_not_reopened_as_usage`
    // (above) and `stanza_with_wrapped_multi_line_description_is_refused`
    // (above) already pin the two hazards fix/multi-stanza-synopsis
    // guarded (`corepack`'s headingless invocation table, `pydoc3`'s own
    // multi-physical-line stanza descriptions) — both fixtures contain no
    // `(` at all, so `paren_group_depth` never leaves zero for either one
    // and this fix's own code path is never reached by them; re-run
    // unchanged by `cargo nextest run --workspace`, not duplicated here.

    /// The positive case: a bare `vgchange`-shaped synopsis whose first
    /// continuation opens a multi-line `(` group, one flag per physical
    /// line, closed by `)` on the last member's own line. Every member is
    /// recovered with a clean value (no leftover `,` from the shape's own
    /// separator) and the alias/value split `bracket_flag_row_content`
    /// already resolves for the bracket-row shape.
    #[test]
    fn paren_alternation_stanza_recovers_every_member_with_clean_values() {
        let help = "tool\n\
                     \t( -a|--aaa Number,\n\
                     \t  -b|--bbb,\n\
                     \t     --ccc y|n )\n";
        let parsed = parse_with_profile(help, None, Some("tool"));
        assert_eq!(parsed.usage.len(), 1, "usage: {:?}", parsed.usage);

        let aaa = parsed
            .flags
            .iter()
            .find(|f| f.long() == Some("aaa"))
            .unwrap_or_else(|| panic!("flags: {:?}", parsed.flags));
        assert_eq!(aaa.short(), Some('a'));
        assert_eq!(aaa.value_name.as_deref(), Some("Number"));

        let bbb = parsed
            .flags
            .iter()
            .find(|f| f.long() == Some("bbb"))
            .unwrap_or_else(|| panic!("flags: {:?}", parsed.flags));
        assert_eq!(bbb.short(), Some('b'));
        assert_eq!(bbb.value_name, None);

        let ccc = parsed
            .flags
            .iter()
            .find(|f| f.long() == Some("ccc"))
            .unwrap_or_else(|| panic!("flags: {:?}", parsed.flags));
        assert_eq!(ccc.value_name.as_deref(), Some("y|n"));

        for f in &parsed.flags {
            if let Some(v) = &f.value_name {
                assert!(
                    !v.ends_with(','),
                    "flag {:?} kept the shape's own trailing comma",
                    f.long()
                );
            }
        }
    }

    /// `vgchange`'s own specimen wrinkle: the group's trailing bracket-row
    /// flag list sits after a blank line separating it from the group's
    /// closing `)` — still the *same* stanza, not a new one. This pins
    /// `just_closed_paren_group`, the narrowest branch this fix adds: the
    /// blank line must fold the bracket rows into the still-open usage
    /// entry rather than requiring fresh `looks_like_stanza_continuation_head`
    /// evidence (a bracket row is never a stanza head).
    #[test]
    fn trailing_bracket_rows_continue_across_the_blank_line_after_the_closing_paren() {
        let help = "tool\n\
                     \t( -a|--aaa Number,\n\
                     \t     --ccc y|n )\n\
                     \n\
                     \t[ -d|--ddd ]\n\
                     \t[ -e|--eee ]\n";
        let parsed = parse_with_profile(help, None, Some("tool"));
        assert_eq!(
            parsed.usage.len(),
            1,
            "the trailing bracket rows must join the one open stanza, not start a second: {:?}",
            parsed.usage
        );
        assert!(
            parsed.usage[0].contains("-d|--ddd") && parsed.usage[0].contains("-e|--eee"),
            "usage: {:?}",
            parsed.usage
        );
        assert!(
            parsed.flags.iter().any(|f| f.long() == Some("ddd")),
            "flags: {:?}",
            parsed.flags
        );
        assert!(
            parsed.flags.iter().any(|f| f.long() == Some("eee")),
            "flags: {:?}",
            parsed.flags
        );
    }

    /// A row using `|` as a plain alias separator with no paren group at
    /// all (an ordinary bracket-row flag list, no `(` anywhere in the
    /// document) must parse exactly as it always did — `paren_group_depth`
    /// stays at zero throughout, so this fix's own code path never fires.
    #[test]
    fn a_plain_alias_separator_row_with_no_paren_group_is_unaffected() {
        let help = "tool\n\
                     \t[ -a|--aaa Number ]\n\
                     \t[ -b|--bbb y|n ]\n";
        let parsed = parse_with_profile(help, None, Some("tool"));
        assert_eq!(parsed.usage.len(), 1, "usage: {:?}", parsed.usage);
        assert!(
            parsed.flags.iter().any(|f| f.long() == Some("aaa")),
            "flags: {:?}",
            parsed.flags
        );
        let bbb = parsed
            .flags
            .iter()
            .find(|f| f.long() == Some("bbb"))
            .unwrap_or_else(|| panic!("flags: {:?}", parsed.flags));
        assert_eq!(bbb.value_name.as_deref(), Some("y|n"));
    }

    // --- the stanza head's own mode-selecting flag ----------------------
    //
    // LVM's real `vgchange --help` (`corpus/vgchange/2.03.16`) documents six
    // stanzas whose head line names a flag never recovered before this fix:
    // `-a|--activate y|n|ay`, `--refresh`, `--systemid String VG`,
    // `--lockstart`, `--lockstop`, `--locktype sanlock|dlm|none VG`. These
    // synthetic fixtures pin the same shapes with a made-up tool name, one
    // hazard per test, independent of that real fixture's own upkeep.

    /// The multi-alias head: `-a|--activate y|n|ay` must read as *one* flag
    /// (short `a`, long `activate`, value `y|n|ay`), not three — the same
    /// `alias_follows`/`take_rest_value_token` guard `bracket_flag_row_content`
    /// already relies on for the identical ambiguity.
    #[test]
    fn stanza_head_multi_alias_flag_reads_as_one_flag_with_its_value() {
        // Mirrors `vgchange`'s own shape exactly: a first stanza whose bare
        // head *does* anchor the usage block (its own bracket row follows
        // immediately), then a second stanza behind a too-short
        // (`MIN_PROSE_SENTENCE_WORDS`-failing, spec §7's own recorded gap)
        // description, which breaks the usage-block's multi-stanza
        // continuation and leaves the second stanza's own head line for
        // the generic heading scanner to find — the exact path that read
        // `vgchange -a|--activate y|n|ay` as a heading and nothing else,
        // before this fix.
        let help = "tool\n\
                     \t[ -x|--xflag ]\n\
                     \n\
                     Do a thing.\n\
                     tool -a|--activate y|n|ay\n\
                     \t[ -f|--force ]\n";
        let parsed = parse_with_profile(help, None, Some("tool"));
        let activate = parsed
            .flags
            .iter()
            .find(|f| f.long() == Some("activate"))
            .unwrap_or_else(|| panic!("flags: {:?}", parsed.flags));
        assert_eq!(activate.short(), Some('a'));
        assert_eq!(activate.value_name.as_deref(), Some("y|n|ay"));
        assert_eq!(activate.value_kind, ValueKind::Required);
        // `Do a thing.` is this stanza's own description, so it is the
        // group's label and the head line is retained as a usage form
        // instead ([`stanza_description_above`]); the assertion the shape
        // this test exists for still makes is that the head flag and its
        // block agree on whatever that label is.
        assert_eq!(activate.group.as_deref(), Some("Do a thing."));
        assert!(
            parsed
                .usage
                .iter()
                .any(|u| u == "tool -a|--activate y|n|ay"),
            "usage: {:?}",
            parsed.usage
        );
        assert!(
            !activate.required,
            "not attempted: no fabricated required-ness"
        );
        assert_eq!(
            parsed
                .flags
                .iter()
                .filter(|f| f.short() == Some('a'))
                .count(),
            1,
            "flags: {:?}",
            parsed.flags
        );
    }

    /// The value-plus-positional head: `--systemid String VG` names one
    /// flag whose value is `String`; the trailing `VG` is a positional, not
    /// a second flag and not swallowed into the value.
    #[test]
    fn stanza_head_value_plus_positional_leaves_the_positional_alone() {
        // `vgchange`'s own `--systemid` stanza, faithfully: `[ COMMON_OPTIONS ]`
        // is a placeholder row, not a flag row (its content does not start
        // with `-`), so `flags_block_start` never recognizes a flags block
        // here at all — this stanza gets no `group`-bearing block from
        // anything else in the engine, and the head line is the *only*
        // place its flag is ever named.
        let help = "tool\n\
                     \n\
                     Change the system ID.\n\
                     tool --systemid String VG\n\
                     \t[ COMMON_OPTIONS ]\n";
        let parsed = parse_with_profile(help, None, Some("tool"));
        let systemid = parsed
            .flags
            .iter()
            .find(|f| f.long() == Some("systemid"))
            .unwrap_or_else(|| panic!("flags: {:?}", parsed.flags));
        assert_eq!(systemid.value_name.as_deref(), Some("String"));
        assert!(
            !parsed.flags.iter().any(|f| f.long() == Some("VG")),
            "flags: {:?}",
            parsed.flags
        );
    }

    /// The bare head (`tool` alone, no flag) must yield no flag and change
    /// nothing — the first stanza of every LVM tool's own synopsis.
    #[test]
    fn bare_stanza_head_with_no_flag_yields_no_flag() {
        let help = "tool\n\t[ -f|--force ]\n";
        let parsed = parse_with_profile(help, None, Some("tool"));
        assert_eq!(parsed.flags.len(), 1, "flags: {:?}", parsed.flags);
        assert_eq!(parsed.flags[0].long(), Some("force"));
    }

    /// A negative case that must **not** be read as a stanza head: a
    /// two-word section heading that is not the tool's own name at all
    /// (`starts_with_tool_name` fails outright), even though it is followed
    /// by indented `[...]` flag rows exactly like a real LVM stanza.
    #[test]
    fn a_heading_not_named_after_the_tool_is_never_read_as_a_stanza_head_flag() {
        let help = "tool\n\t[ -f|--force ]\n\nCommon options:\n\t[ -d|--debug ]\n";
        let parsed = parse_with_profile(help, None, Some("tool"));
        assert!(
            !parsed.flags.iter().any(|f| f.long() == Some("options")),
            "flags: {:?}",
            parsed.flags
        );
        assert!(
            parsed.flags.iter().any(|f| f.long() == Some("debug")),
            "flags: {:?}",
            parsed.flags
        );
    }

    /// `bpftrace --help`'s real `EXAMPLES:` block writes each example as
    /// "the tool's own name, then a bare flag token, then a one-line
    /// description" — `tool -e 'tracepoint:raw_syscalls:sys_enter { ... }'`
    /// — line for line the same shape as a genuine LVM stanza head. Without
    /// `in_ignorable_section` gating `recover_stanza_head_flag`, this
    /// fabricated `-e`/`-l` rows whose value was a fragment of the example
    /// invocation, displacing the real, described `-e`/`-l` rows from the
    /// `OPTIONS:` block above. This pins the fix: the real, described flags
    /// survive untouched and nothing from `EXAMPLES:` is mined at all.
    #[test]
    fn examples_section_invocation_lines_are_never_read_as_stanza_heads() {
        let help = "tool - a tool\n\
                     \n\
                     OPTIONS:\n\
                     \x20\x20\x20\x20-e 'program'   execute this program\n\
                     \x20\x20\x20\x20-l [search]    list probes\n\
                     \n\
                     EXAMPLES:\n\
                     tool -l '*sleep*'\n\
                     \x20\x20\x20\x20list probes containing \"sleep\"\n\
                     tool -e 'tracepoint:raw_syscalls:sys_enter { @[comm] = count(); }'\n\
                     \x20\x20\x20\x20count syscalls by process name\n";
        let parsed = parse_with_profile(help, None, Some("tool"));
        let e_flags: Vec<_> = parsed
            .flags
            .iter()
            .filter(|f| f.short() == Some('e'))
            .collect();
        assert_eq!(e_flags.len(), 1, "flags: {:?}", parsed.flags);
        assert_eq!(e_flags[0].value_name.as_deref(), Some("'program'"));
        assert!(
            e_flags[0].description.is_some(),
            "flags: {:?}",
            parsed.flags
        );

        let l_flags: Vec<_> = parsed
            .flags
            .iter()
            .filter(|f| f.short() == Some('l'))
            .collect();
        assert_eq!(l_flags.len(), 1, "flags: {:?}", parsed.flags);
        assert!(
            l_flags[0].description.is_some(),
            "flags: {:?}",
            parsed.flags
        );

        assert!(
            !parsed.flags.iter().any(|f| f
                .value_name
                .as_deref()
                .is_some_and(|v| v.contains("sleep") || v.contains("tracepoint"))),
            "flags: {:?}",
            parsed.flags
        );
    }

    // --- LVM's docopt bracket-group flag rows ---------------------------

    /// `vgck --help`, byte-exact (minus the `WARNING: Running as a
    /// non-root user` stderr banner, irrelevant here). A bare, unlabelled
    /// synopsis (`vgck` alone, no bracket notation on its own line) whose
    /// only flag — `--reportformat` — is documented on a continuation row,
    /// plus a real `Common options for lvm:` heading whose 18 rows are
    /// each one whole `[...]` group, tab-indented under a heading written
    /// with two spaces.
    const VGCK_HELP: &str = concat!(
        "  vgck - Check the consistency of volume group(s)\n",
        "\n",
        "  Read and display information about a VG.\n",
        "  vgck\n",
        "\t[    --reportformat basic|json ]\n",
        "\t[ COMMON_OPTIONS ]\n",
        "\t[ VG|Tag ... ]\n",
        "\n",
        "  Rewrite VG metadata to correct problems.\n",
        "  vgck --updatemetadata VG\n",
        "\t[ COMMON_OPTIONS ]\n",
        "\n",
        "  Common options for lvm:\n",
        "\t[ -d|--debug ]\n",
        "\t[ -h|--help ]\n",
        "\t[ -q|--quiet ]\n",
        "\t[ -v|--verbose ]\n",
        "\t[ -y|--yes ]\n",
        "\t[ -t|--test ]\n",
        "\t[    --commandprofile String ]\n",
        "\t[    --config String ]\n",
        "\t[    --driverloaded y|n ]\n",
        "\t[    --nolocking ]\n",
        "\t[    --lockopt String ]\n",
        "\t[    --longhelp ]\n",
        "\t[    --profile String ]\n",
        "\t[    --version ]\n",
        "\t[    --devicesfile String ]\n",
        "\t[    --devices PV ]\n",
        "\t[    --nohints ]\n",
        "\t[    --journal String ]\n",
        "\n",
        "  Use --longhelp to show all options and advanced commands.\n",
    );

    #[test]
    fn vgck_recovers_the_synopsis_continuation_flag() {
        let parsed = parse_with_profile(VGCK_HELP, None, Some("vgck"));
        let reportformat = flag_named(&parsed, "reportformat");
        assert_eq!(reportformat.short(), None);
        assert_eq!(reportformat.value_name.as_deref(), Some("basic|json"));
    }

    #[test]
    fn vgck_recovers_every_common_option_from_the_headed_bracket_table() {
        let parsed = parse_with_profile(VGCK_HELP, None, Some("vgck"));
        let debug = flag_named(&parsed, "debug");
        assert_eq!(debug.short(), Some('d'));
        assert_eq!(debug.value_name, None);

        let commandprofile = flag_named(&parsed, "commandprofile");
        assert_eq!(commandprofile.short(), None);
        assert_eq!(commandprofile.value_name.as_deref(), Some("String"));

        let driverloaded = flag_named(&parsed, "driverloaded");
        assert_eq!(driverloaded.value_name.as_deref(), Some("y|n"));

        // Every one of the 18 rows under `Common options for lvm:`, plus
        // `--reportformat` from the first stanza's synopsis continuation
        // and `--updatemetadata` from the *second* stanza's own head
        // (fix/multi-stanza-synopsis: the blank line between the two no
        // longer ends the usage block before the second stanza is read).
        for long in [
            "debug",
            "help",
            "quiet",
            "verbose",
            "yes",
            "test",
            "commandprofile",
            "config",
            "driverloaded",
            "nolocking",
            "lockopt",
            "longhelp",
            "profile",
            "version",
            "devicesfile",
            "devices",
            "nohints",
            "journal",
            "reportformat",
            "updatemetadata",
        ] {
            flag_named(&parsed, long);
        }
        assert_eq!(parsed.flags.len(), 20, "{:#?}", parsed.flags);
    }

    /// The operand cross-references LVM writes in the identical bracket
    /// notation must never be read as flags: `[ COMMON_OPTIONS ]` names no
    /// dash at all, and `[ VG|Tag ... ]` / `[ VG PV ... ]` are positionals.
    #[test]
    fn vgck_never_fabricates_a_flag_from_an_operand_bracket() {
        let parsed = parse_with_profile(VGCK_HELP, None, Some("vgck"));
        assert!(parsed
            .flags
            .iter()
            .all(|f| f.long() != Some("COMMON_OPTIONS")));
        assert!(parsed.flags.iter().all(|f| f.long() != Some("VG")));
    }

    /// `vgextend`'s richer synopsis head (`vgextend VG PV ...`, still no
    /// bracket notation of its own) with the same value-vs-alias-vs-nested-
    /// bracket shapes this fix's own doc comments name: `-A|--autobackup
    /// y|n` (alias cluster plus a choice-list value) and `--metadatasize
    /// Size[m|UNIT]` (a value carrying its own nested brackets).
    #[test]
    fn vgextend_reads_the_alias_choice_and_nested_bracket_value_rows() {
        let raw = concat!(
            "  vgextend - Add physical volumes to a volume group\n",
            "\n",
            "  vgextend VG PV ...\n",
            "\t[ -A|--autobackup y|n ]\n",
            "\t[ -f|--force ]\n",
            "\t[    --metadatasize Size[m|UNIT] ]\n",
            "\t[ COMMON_OPTIONS ]\n",
        );
        let parsed = parse_with_profile(raw, None, Some("vgextend"));

        let autobackup = flag_named(&parsed, "autobackup");
        assert_eq!(autobackup.short(), Some('A'));
        assert_eq!(autobackup.value_name.as_deref(), Some("y|n"));

        let force = flag_named(&parsed, "force");
        assert_eq!(force.short(), Some('f'));
        assert_eq!(force.value_name, None);

        let metadatasize = flag_named(&parsed, "metadatasize");
        assert_eq!(metadatasize.value_name.as_deref(), Some("Size[m|UNIT]"));
    }

    // --- the alternation-with-mismatched-operands hazard ----------------

    /// `ethtool --help`'s real row: an alternation between two *different*
    /// flags, only one of which carries its own bracketed operands. Not
    /// LVM's shape at all — LVM's alias run never has a bare flag
    /// spelling reappear after the first whitespace gap — so
    /// `bracket_flag_row_content` must refuse the whole row rather than
    /// read `--all-groups` as carrying `--groups`'s operand and losing
    /// `--groups` outright (the exact fabrication this fix's own doc
    /// comment on `bracket_flag_row_content` names).
    #[test]
    fn bracket_row_with_a_second_alternatives_operands_is_refused() {
        assert_eq!(
            bracket_flag_row_content(
                "[ --all-groups | --groups [eth-phy] [eth-mac] [eth-ctrl] [rmon] ]"
            ),
            None
        );
    }

    #[test]
    fn ethtool_keeps_both_alternatives_unread_rather_than_fabricating() {
        let raw = concat!(
            "  ethtool DEVNAME\n",
            "\t[ --all-groups | --groups [eth-phy] [eth-mac] [eth-ctrl] [rmon] ]\n",
        );
        let parsed = parse_with_profile(raw, None, Some("ethtool"));
        assert!(parsed.flags.iter().all(|f| f.long() != Some("all-groups")));
    }

    /// Regression: `curl --help` runs its flag list straight into the
    /// usage line — indented one space, no blank line, no `Options:`
    /// heading. The usage block used to consume every indented line that
    /// followed, so all of curl's flags landed in `usage` and the tool
    /// reported *zero* flags while its status stayed `ok` (nothing was
    /// fabricated, so the structure-sanity check couldn't see it either).
    /// This fixture was checked in but never asserted on, which is how it
    /// survived; the assertion is what makes the fix stick.
    #[test]
    fn curl_flags_running_straight_into_the_usage_line_are_not_swallowed() {
        let parsed = parse(CURL_HELP);
        assert!(
            parsed.flags.len() > 100,
            "expected curl's full flag list, got {}",
            parsed.flags.len()
        );
        let longs: Vec<&str> = parsed.flags.iter().filter_map(|f| f.long()).collect();
        assert!(longs.contains(&"append"), "{longs:?}");
        assert!(longs.contains(&"anyauth"), "{longs:?}");
        // The usage block keeps its own line and stops before the flags.
        assert_eq!(parsed.usage.len(), 1);
        assert!(parsed.usage[0].starts_with("Usage: curl"));
        // And it must not have invented subcommands out of the flag rows.
        assert!(parsed.subcommands.is_empty(), "{:?}", parsed.subcommands);
    }

    /// The fabrication bug this module exists to fix: `git --help`'s usage
    /// synopsis is one invocation form wrapped across five physical lines
    /// (four continuations, all indented under `git`, none of them a
    /// marker). The old per-physical-line storage reported five separate
    /// `usage` entries, and the detail pane's `usage_signature` — which
    /// prepends the node name to any entry not already starting with it —
    /// turned the tail fragment into `git [--config-env=<name>=<envvar>]
    /// <command> [<args>]`, a complete-looking invocation git never
    /// documented. The fix must produce exactly one entry, with every
    /// fragment's own text intact and joined by a single space (not
    /// re-flowed — spec §7: usage is kept verbatim).
    #[test]
    fn git_wrapped_usage_synopsis_joins_into_one_entry() {
        let parsed = parse(GIT_HELP);
        assert_eq!(
            parsed.usage.len(),
            1,
            "git's five wrapped lines must join into one logical entry, got {:?}",
            parsed.usage
        );
        assert_eq!(
            parsed.usage[0],
            "usage: git [-v | --version] [-h | --help] [-C <path>] [-c <name>=<value>] \
             [--exec-path[=<path>]] [--html-path] [--man-path] [--info-path] \
             [-p | --paginate | -P | --no-pager] [--no-replace-objects] [--bare] \
             [--git-dir=<path>] [--work-tree=<path>] [--namespace=<name>] \
             [--config-env=<name>=<envvar>] <command> [<args>]"
        );
    }

    /// The over-join guard: `du --help` prints two *genuine* alternative
    /// invocation forms, joined by GNU coreutils' `or:` marker —
    ///
    /// ```text
    /// Usage: du [OPTION]... [FILE]...
    ///   or:  du [OPTION]... --files0-from=F
    /// ```
    ///
    /// — as opposed to one form wrapped across lines. The `or:` line is
    /// indented *more* than the block's base indent (0), so a rule that
    /// joins every more-indented line onto the entry above it — correct
    /// for git's wrapped synopsis above — would also swallow this one,
    /// silently merging two real invocations into a single fabricated
    /// line. The marker check is what keeps `or:` its own entry regardless
    /// of indentation.
    #[test]
    fn du_or_marker_stays_a_separate_usage_entry() {
        let raw = "Usage: du [OPTION]... [FILE]...\n  or:  du [OPTION]... --files0-from=F\nSummarize device usage of the set of FILEs, recursively for directories.\n";
        let parsed = parse(raw);
        assert_eq!(
            parsed.usage,
            vec![
                "Usage: du [OPTION]... [FILE]...".to_string(),
                "  or:  du [OPTION]... --files0-from=F".to_string(),
            ],
            "or: must stay a separate entry, not join onto the line above"
        );
    }

    /// A tool that repeats the `usage:`/`Usage:` label itself for each
    /// form (rather than using `or:`) must also get one entry per label,
    /// not one entry per physical line and not everything joined into one.
    #[test]
    fn repeated_usage_label_at_base_indent_starts_a_new_entry() {
        let raw = "usage: widget run [OPTIONS]\nusage: widget stop [OPTIONS]\n";
        let parsed = parse(raw);
        assert_eq!(
            parsed.usage,
            vec![
                "usage: widget run [OPTIONS]".to_string(),
                "usage: widget stop [OPTIONS]".to_string(),
            ]
        );
    }

    /// The other discriminator spec §7's usage grammar allows for: a tool
    /// that lists alternative forms *without* any `usage:`/`or:` marker, by
    /// literally repeating its own name. This must read as two entries when
    /// the tool's name is known — the counterpart to the marker-based
    /// version just above, exercised via [`parse_named`] rather than
    /// [`parse`] since the discriminator only fires when a name is given.
    #[test]
    fn own_name_repeated_with_no_marker_starts_a_new_entry() {
        let raw = "Usage: prog foo\n       prog bar\n";
        let parsed = parse_named(raw, "prog");
        assert_eq!(
            parsed.usage,
            vec!["Usage: prog foo".to_string(), "       prog bar".to_string(),],
            "{:?}",
            parsed.usage
        );
        // Without a known name, the second line is still more indented
        // than the block's own base (7 spaces vs. 0) — the same hanging-
        // indent shape git's wrapped synopsis uses — so it (reasonably)
        // reads as a continuation instead, joining into one entry. This is
        // exactly why `tool_name` matters: the discriminator that tells
        // these two real shapes apart is the name, not the indent, which
        // looks identical in both.
        let unnamed = parse(raw);
        assert_eq!(unnamed.usage, vec!["Usage: prog foo prog bar".to_string()]);
    }

    /// The regression this batch exists to fix: `lsof -h`'s usage synopsis
    /// wraps across three physical lines, but — unlike `git`'s hanging
    /// indent — every continuation sits at *the same* column as the
    /// `usage:` marker itself (both indented by exactly one space):
    ///
    /// ```text
    ///  usage: [-?abhKlnNoOPRtUvVX] [+|-c c] [+|-d s] [+D D] [+|-E] [+|-e s] [+|-f[gG]]
    ///  [-F [f]] [-g [s]] [-i [i]] [+|-L [l]] [+m [m]] [+|-M] [-o [o]] [-p s]
    ///  [+|-r [t]] [-s [p:s]] [-S [t]] [-T [t]] [-u s] [+|-w] [-x [fl]] [--] [names]
    /// ```
    ///
    /// The indentation-only rule `f5f1183` shipped read `leading_whitespace
    /// <= base_indent` as "block already ended" for every one of these
    /// continuation lines, dropping them — and the six flags documented
    /// only in them (`-F`, `-g`, `+|-L`, `+m`, `+|-M`, `+|-r`, among others)
    /// never reached `extract_usage_flags` — before they ever joined
    /// `result.usage`. This must now recover as one logical entry, with
    /// every continuation-only flag still present.
    #[test]
    fn lsof_same_indent_continuations_join_into_one_entry() {
        let raw = " usage: [-?abhKlnNoOPRtUvVX] [+|-c c] [+|-d s] [+D D] [+|-E] [+|-e s] [+|-f[gG]]\n \
                    [-F [f]] [-g [s]] [-i [i]] [+|-L [l]] [+m [m]] [+|-M] [-o [o]] [-p s]\n \
                    [+|-r [t]] [-s [p:s]] [-S [t]] [-T [t]] [-u s] [+|-w] [-x [fl]] [--] [names]\n\
                    Defaults in parentheses; comma-separated set (s) items; dash-separated ranges.\n";
        let parsed = parse_named(raw, "lsof");
        assert_eq!(
            parsed.usage.len(),
            1,
            "lsof's three same-indent lines must join into one logical entry, got {:?}",
            parsed.usage
        );
        assert_eq!(
            parsed.usage[0],
            "usage: [-?abhKlnNoOPRtUvVX] [+|-c c] [+|-d s] [+D D] [+|-E] [+|-e s] [+|-f[gG]] \
             [-F [f]] [-g [s]] [-i [i]] [+|-L [l]] [+m [m]] [+|-M] [-o [o]] [-p s] \
             [+|-r [t]] [-s [p:s]] [-S [t]] [-T [t]] [-u s] [+|-w] [-x [fl]] [--] [names]"
        );
        // The over-join guard's counterpart: the trailing "Defaults in
        // parentheses..." sentence sits at that same column-0-or-less
        // position (no leading space at all) but is ordinary prose, not a
        // usage-grammar fragment, and must still end the block rather than
        // being swallowed onto the synopsis.
        assert!(!parsed.usage[0].contains("Defaults"), "{:?}", parsed.usage);
        let short_flags: Vec<Option<char>> = parsed.flags.iter().map(|f| f.short()).collect();
        // Spot-check flags documented only in the two (previously dropped)
        // continuation lines — none of these appear in the first line's
        // own groups (verified by hand against `usage_segments`'
        // token-level behavior: `-o`, for instance, appears as a bare
        // character inside line one's bundled `-?abhKlnNoOPRtUvVX` blob,
        // but that whole blob parses as a single flag spelled `-?` with
        // the rest as its value shape, not as fourteen separate flags — so
        // `-o` is only ever actually recovered from continuation line
        // one's own explicit `[-o [o]]` group).
        for want in ['F', 'g', 'L', 'M', 'r', 'u'] {
            assert!(
                short_flags.contains(&Some(want)),
                "expected -{want} recovered from lsof's continuation lines, got {short_flags:?}"
            );
        }
    }

    /// Regression for spec [M-8]: `ip --help` writes only to stderr and
    /// exits 255. `ip`'s usage grammar (`OBJECT := { address | ... }`) is
    /// unusual enough that this just checks *something* structural comes
    /// back, not a specific shape.
    #[test]
    fn ip_stderr_help_produces_a_usage_line() {
        let parsed = parse(IP_HELP);
        assert!(
            !parsed.usage.is_empty(),
            "expected at least a Usage: line from ip's stderr help"
        );
    }

    #[test]
    fn tar_usage_line_recovered() {
        let parsed = parse(TAR_HELP);
        assert!(!parsed.usage.is_empty());
        assert!(parsed.usage[0].to_lowercase().contains("usage:"));
    }

    /// Regression for the two `extract_positionals` defects the
    /// `corpus/git/2.43.0` fixture held open under `[xfail]`: git's root
    /// usage line has `-C <path>` and `-c <name>=<value>` before its two
    /// real positionals, `<command>` and `[<args>]`.
    ///
    /// 1. Greedy bracket match: stripping only the *last* `>` off
    ///    `<name>=<value>` used to land past the value spec's own closing
    ///    bracket, producing a positional literally named `name>=<value`.
    /// 2. Flag arguments read as positionals: `-C <path>` and
    ///    `-c <name>=<value>` are option values, not positionals, but the
    ///    old scan had no awareness of what preceded a token.
    ///
    /// The fix must produce exactly `command` and `args` — neither
    /// `path` nor `name>=<value` (nor `options`/`file`/anything else) may
    /// leak in.
    #[test]
    fn git_root_positionals_are_exactly_command_and_args() {
        let parsed = parse(GIT_HELP);
        let names: Vec<&str> = parsed
            .positionals
            .iter()
            .map(|p| p.primary_name())
            .collect();
        assert_eq!(names, vec!["command", "args"], "{names:?}");
    }

    /// The general shape behind the git-specific regression above, spelled
    /// out with a synthetic usage line so the rule reads as "any flag
    /// followed by its value token", not "git's own bytes": a bare flag's
    /// value — whether `<angle>`-bracketed or a bare `UPPERCASE` word
    /// (argparse's `--config FILE` convention) — must never become a
    /// positional, while a flag that already carries its value inline
    /// (`=`) leaves the *next* token free to be a real positional.
    #[test]
    fn flag_values_in_a_usage_line_are_never_positionals() {
        let parsed = parse("usage: widget [-C <dir>] [--tag=<name>] <target> [--config FILE]\n");
        let names: Vec<&str> = parsed
            .positionals
            .iter()
            .map(|p| p.primary_name())
            .collect();
        assert_eq!(names, vec!["target"], "{names:?}");
    }

    /// The mirror-image case: a token right after a flag *is* a real
    /// positional when that flag was already written as its own complete,
    /// self-closed bracket group (`[-V]`, `[-v]`) rather than one still
    /// waiting on a following value (`[-C`, `-C`). `sg_emc_trespass`'s own
    /// synopsis, `[-d] [-hr] [-s] [-V] DEVICE`, is exactly this shape:
    /// `DEVICE` sits right after the fully-closed `[-V]` and is a real
    /// operand, not `-V`'s fabricated argument.
    #[test]
    fn a_token_after_a_self_closed_bracket_flag_is_a_real_positional() {
        let parsed = parse("Usage:  sg_emc_trespass [-d] [-hr] [-s] [-V] DEVICE\n");
        let names: Vec<&str> = parsed
            .positionals
            .iter()
            .map(|p| p.primary_name())
            .collect();
        assert_eq!(names, vec!["DEVICE"], "{names:?}");

        // The general shape, with more than one self-closed flag ahead of
        // an uppercase operand.
        let parsed = parse("usage: widget [-h] [-v] FILE\n");
        let names: Vec<&str> = parsed
            .positionals
            .iter()
            .map(|p| p.primary_name())
            .collect();
        assert_eq!(names, vec!["FILE"], "{names:?}");
    }

    /// `vim.basic`, the anchor case for [`OPTION_LIST_PLACEHOLDERS`],
    /// confirmed with the maintainer on 2026-08-13: in
    /// `Usage: vim [arguments] [file ..]`, `[arguments]` is the placeholder
    /// for vim's own 45-flag list and `[file ..]` is a real variadic
    /// operand. Extracting `arguments` would be a fabrication, not a recall
    /// gain, and this asserts it stays out no matter which of the two rules
    /// (bare-lowercase, or the placeholder list) is doing the work.
    #[test]
    fn a_usage_lines_option_list_placeholder_is_never_an_operand() {
        for line in [
            "Usage: vim [arguments] [file ..]\n",
            "usage: pkgconf [OPTIONS] [LIBRARIES]\n",
            "Usage: dpkg-statoverride [<option> ...] <command>\n",
            "Usage: tar [OPTION...] [FILE]...\n",
            "USAGE: widget [FLAGS] [OPTIONS] <input>\n",
        ] {
            let names: Vec<String> = parse(line)
                .positionals
                .into_iter()
                .map(|p| p.primary_name().to_string())
                .collect();
            assert!(
                !names
                    .iter()
                    .any(|n| OPTION_LIST_PLACEHOLDERS.contains(&n.to_lowercase().as_str())),
                "{line:?} yielded {names:?}"
            );
        }
    }

    /// The other direction, and the reason `args`/`arg` are absent from
    /// [`OPTION_LIST_PLACEHOLDERS`]: an operand whose name merely *reads*
    /// like a generic word is still an operand, and the guard must not
    /// reach it.
    #[test]
    fn a_real_operand_is_not_mistaken_for_an_option_list_placeholder() {
        let parsed = parse("usage: git [<options>] <command> [<args>]\n");
        let names: Vec<&str> = parsed
            .positionals
            .iter()
            .map(|p| p.primary_name())
            .collect();
        assert_eq!(names, vec!["command", "args"], "{names:?}");
    }

    /// [M-15]'s headline case, straight from the reference example in the
    /// work order: a synopsis with no `Options:`/`Flags:` block at all must
    /// still recover the flags it documents inline. Also exercises the
    /// conservative-pairing rule end to end: `[-v | --version]` (exactly
    /// one short, one long) becomes one flag with both spellings;
    /// `[-p | --paginate | -P | --no-pager]` (two of each) must not guess a
    /// pairing and instead emit all four spellings as separate entries.
    #[test]
    fn usage_synopsis_flags_are_recovered_with_conservative_pairing() {
        let raw = "usage: git [-v | --version] [-h | --help] [-C <path>] \
                   [-p | --paginate | -P | --no-pager] [--git-dir=<path>]\n";
        let parsed = parse(raw);

        let version = parsed
            .flags
            .iter()
            .find(|f| f.long() == Some("version"))
            .expect("--version recovered");
        assert_eq!(
            version.short(),
            Some('v'),
            "exactly one short + one long in a group must pair"
        );

        let help = parsed
            .flags
            .iter()
            .find(|f| f.long() == Some("help"))
            .expect("--help recovered");
        assert_eq!(help.short(), Some('h'));

        // Four alternatives: never guess which short goes with which long.
        // Every spelling is its own unpaired flag, with no cross-pairing.
        let spellings: Vec<(Option<char>, Option<&str>)> =
            parsed.flags.iter().map(|f| (f.short(), f.long())).collect();
        assert!(
            spellings.contains(&(Some('p'), None)),
            "expected an unpaired -p entry, got {spellings:?}"
        );
        assert!(
            spellings.contains(&(None, Some("paginate"))),
            "expected an unpaired --paginate entry, got {spellings:?}"
        );
        assert!(
            spellings.contains(&(Some('P'), None)),
            "expected an unpaired -P entry, got {spellings:?}"
        );
        assert!(
            spellings.contains(&(None, Some("no-pager"))),
            "expected an unpaired --no-pager entry, got {spellings:?}"
        );

        // None of these carry a description — a synopsis has spellings and
        // value shapes only, never prose (spec §7 Tier B: never fabricate).
        assert!(parsed.flags.iter().all(|f| f.description.is_none()));
    }

    /// `pppdump`'s own usage line, byte-exact, is the counter-example to
    /// the conservative-pairing rule's usual `[-v | --version]` reading:
    /// `-h` prints the dump in hexadecimal and `-p[d]` prints it as
    /// printable characters (`p`) or both (`pd`) — two distinct flags the
    /// tool happened to write as a `|`-alternation, not one flag's two
    /// spellings. [`pair_short_and_long`] must refuse to pair a short
    /// spelling with a single-dash abbreviation-bracket "long-like" one
    /// ([`long_is_double_dash`]); pairing here would fabricate a merge
    /// across distinct flags, the same defect class as `pod2html`'s
    /// `--quiet`/`--noquiet`/`--verbose`/`--noverbose` row.
    #[test]
    fn pppdumps_short_and_abbrev_bracket_alternation_is_never_paired() {
        let raw = "Usage: /usr/sbin/pppdump [-h | -p[d]] [-r] [-m mru] [-a] [file ...]\n";
        let parsed = parse(raw);

        let h = parsed
            .flags
            .iter()
            .find(|f| f.short() == Some('h'))
            .expect("-h recovered as its own flag");
        assert_eq!(
            h.long(),
            None,
            "-h must never gain -p[d]'s name through a fabricated pairing"
        );

        let pd = parsed
            .flags
            .iter()
            .find(|f| f.long() == Some("pd"))
            .expect("-p[d] recovered as its own flag, named from its bracket");
        assert_eq!(
            pd.short(),
            None,
            "-p[d] must never gain -h's spelling through a fabricated pairing"
        );
    }

    /// spec §13's metric redefinition rests on this: a usage-synopsis-
    /// derived flag must carry `Source::HelpTextSynopsis`, not the plain
    /// `Source::HelpText` an options-table row gets, so `pct_flags_with_text`
    /// can exclude it from the denominator instead of counting it as an
    /// undescribed flag from a source that could have described it.
    #[test]
    fn usage_derived_flags_carry_the_synopsis_source_table_derived_do_not() {
        let raw =
            "usage: widget [--verbose] [<file>]\n\nOptions:\n  --loud    print extra output\n";
        let parsed = parse(raw);

        let verbose = parsed
            .flags
            .iter()
            .find(|f| f.long() == Some("verbose"))
            .expect("--verbose recovered from the synopsis");
        assert_eq!(
            verbose.provenance.sources.as_slice(),
            [Source::HelpTextSynopsis],
            "usage-only flag must be marked structurally undescribable"
        );
        assert!(!verbose.provenance.describable());

        let loud = parsed
            .flags
            .iter()
            .find(|f| f.long() == Some("loud"))
            .expect("--loud recovered from the Options: block");
        assert_eq!(loud.provenance.sources.as_slice(), [Source::HelpText]);
        assert!(loud.provenance.describable());
    }

    /// Value shapes the usage grammar recognizes: `-C <path>` (space-
    /// separated) and `--git-dir=<path>` (`=`-joined) are both a
    /// *required* value; `--exec-path[=<path>]` (bracketed) is *optional*.
    #[test]
    fn usage_synopsis_flag_value_shapes_are_captured() {
        let raw = "usage: git [-C <path>] [--exec-path[=<path>]] [--git-dir=<path>]\n";
        let parsed = parse(raw);

        let c = parsed
            .flags
            .iter()
            .find(|f| f.short() == Some('C'))
            .expect("-C recovered");
        assert_eq!(c.value_name.as_deref(), Some("<path>"));
        assert_eq!(c.value_kind, mandible_core::ValueKind::Required);

        let exec_path = parsed
            .flags
            .iter()
            .find(|f| f.long() == Some("exec-path"))
            .expect("--exec-path recovered");
        assert_eq!(exec_path.value_kind, mandible_core::ValueKind::Optional);

        let git_dir = parsed
            .flags
            .iter()
            .find(|f| f.long() == Some("git-dir"))
            .expect("--git-dir recovered");
        assert_eq!(git_dir.value_kind, mandible_core::ValueKind::Required);
    }

    /// A mandatory flag some tool's synopsis writes unbracketed —
    /// `ssh-keygen`'s own `-D pkcs11`, `-M generate`, `-I
    /// certificate_identity -s ca_key` — is two bare tokens with no
    /// group around either. Before the bare-token lookahead in
    /// `extract_usage_flags` existed, each flag's own value word was a
    /// second, unrelated `Bare` segment that started with neither `-` nor
    /// anything else this scan reads, so it was silently dropped and the
    /// flag came out looking like a boolean it isn't.
    #[test]
    fn bare_synopsis_flags_recover_their_unbracketed_value() {
        let raw = "usage: ssh-keygen -D pkcs11\n       ssh-keygen -M generate [-O option] output_file\n       ssh-keygen -I certificate_identity -s ca_key [-hU]\n";
        let parsed = parse(raw);

        let d = parsed
            .flags
            .iter()
            .find(|f| f.short() == Some('D'))
            .expect("-D recovered");
        assert_eq!(d.value_name.as_deref(), Some("pkcs11"));
        assert_eq!(d.value_kind, mandible_core::ValueKind::Required);

        let m = parsed
            .flags
            .iter()
            .find(|f| f.short() == Some('M'))
            .expect("-M recovered");
        assert_eq!(m.value_name.as_deref(), Some("generate"));

        let i = parsed
            .flags
            .iter()
            .find(|f| f.short() == Some('I'))
            .expect("-I recovered");
        assert_eq!(i.value_name.as_deref(), Some("certificate_identity"));

        let s = parsed
            .flags
            .iter()
            .find(|f| f.short() == Some('s'))
            .expect("-s recovered");
        assert_eq!(s.value_name.as_deref(), Some("ca_key"));
    }

    /// `iptables --help`'s own synopsis line, byte-exact: `-h` takes no
    /// argument, and `(print` is the first word of a parenthetical aside,
    /// not a value. Measured during this fix's own fleet sweep — without
    /// the `(`-guard, `-h` picked up a fabricated value across the whole
    /// `iptables`/`arptables`/`ip6tables` family.
    #[test]
    fn a_parenthetical_aside_after_a_bare_flag_is_never_read_as_its_value() {
        let raw = "Usage: iptables -h (print this help information)\n";
        let parsed = parse(raw);
        let h = parsed
            .flags
            .iter()
            .find(|f| f.short() == Some('h'))
            .expect("-h recovered");
        assert_eq!(h.value_name, None, "-h must stay boolean");
    }

    /// The lookahead must never attach a value onto a bare flag that is
    /// immediately followed by *another* flag rather than a value —
    /// `ssh-keygen -k -f krl_file`'s own `-k` takes no argument at all, and
    /// what follows it is `-f`, not `-k`'s value.
    #[test]
    fn a_bare_flag_followed_by_another_bare_flag_stays_boolean() {
        let raw = "usage: ssh-keygen -k -f krl_file\n";
        let parsed = parse(raw);
        let k = parsed
            .flags
            .iter()
            .find(|f| f.short() == Some('k'))
            .expect("-k recovered");
        assert_eq!(k.value_name, None, "-k must stay boolean");
    }

    /// The bundled-short-flag collapse, end to end through `parse`:
    /// `tmux`'s real synopsis line, byte-exact. Its `[-2CDlNuVv]` must
    /// become eight boolean switches, and the five genuine value-taking
    /// short flags sharing the same physical line must be untouched — the
    /// only thing separating them from the cluster is a space, so this
    /// asserts both halves together or it asserts nothing useful.
    #[test]
    fn a_synopsis_short_flag_cluster_becomes_one_flag_per_member() {
        let raw = "usage: tmux [-2CDlNuVv] [-c shell-command] [-f file] [-L socket-name]\n            [-S socket-path] [-T features] [command [flags]]\n";
        let parsed = parse(raw);
        for member in "2CDlNuVv".chars() {
            let flag = parsed
                .flags
                .iter()
                .find(|f| f.short() == Some(member))
                .unwrap_or_else(|| panic!("-{member} missing from {:?}", parsed.flags));
            assert_eq!(flag.value_name, None, "-{member} is a boolean switch");
            assert_eq!(
                flag.value_kind,
                mandible_core::ValueKind::None,
                "-{member} takes no value"
            );
            assert_eq!(flag.long(), None);
            assert!(flag.description.is_none(), "a usage line describes nothing");
        }
        for (short, value) in [
            ('c', "shell-command"),
            ('f', "file"),
            ('L', "socket-name"),
            ('S', "socket-path"),
            ('T', "features"),
        ] {
            let flag = parsed
                .flags
                .iter()
                .find(|f| f.short() == Some(short))
                .unwrap_or_else(|| panic!("-{short} missing from {:?}", parsed.flags));
            assert_eq!(flag.value_name.as_deref(), Some(value));
            assert_eq!(flag.value_kind, mandible_core::ValueKind::Required);
        }
    }

    /// The counterweight, and the reason the cluster question is asked on
    /// the *synopsis* path only: `filefrag`'s real usage line carries a
    /// cluster and a glued value spec side by side. `[-b{blocksize}[KMG]]`
    /// is synopsis-sourced and glued exactly like the cluster is, and must
    /// stay one valued flag.
    #[test]
    fn a_glued_value_spec_beside_a_cluster_stays_one_flag() {
        let raw = "Usage: /usr/sbin/filefrag [-b{blocksize}[KMG]] [-BeEksvxX] file ...\n";
        let parsed = parse(raw);
        let b = parsed
            .flags
            .iter()
            .find(|f| f.short() == Some('b'))
            .expect("-b recovered");
        assert_eq!(b.value_name.as_deref(), Some("{blocksize}[KMG]"));
        for member in "BeEksvxX".chars() {
            assert!(
                parsed.flags.iter().any(|f| f.short() == Some(member)),
                "-{member} missing from {:?}",
                parsed.flags
            );
        }
    }

    /// An option-*table* row of the identical shape is the GCC/Clang
    /// single-dash convention and is genuinely one flag with a glued
    /// value. Only the synopsis path splits, so a described row keeps its
    /// description and its value — splitting it would destroy thousands of
    /// correct fleet-wide parses to fix 58 tools.
    #[test]
    fn an_options_block_row_of_the_same_shape_is_never_split() {
        let raw = "Options:\n  -Zscript      run a script\n  -DMACRO       define a macro\n";
        let parsed = parse(raw);
        let z = parsed
            .flags
            .iter()
            .find(|f| f.short() == Some('Z'))
            .expect("-Zscript recovered");
        assert_eq!(z.value_name.as_deref(), Some("script"));
        assert!(
            !parsed.flags.iter().any(|f| f.short() == Some('s')),
            "-Zscript must not have been split: {:?}",
            parsed.flags
        );
    }

    /// A cluster in a synopsis whose members are *also* documented in an
    /// options block must not double-count: `flag_spelling_already_present`
    /// already drops a usage-derived duplicate, and expansion feeds it one
    /// candidate per member rather than one for the whole cluster. `od`'s
    /// real shape — a bundle in the usage line, the same switches described
    /// in a table below it.
    #[test]
    fn cluster_members_already_described_in_a_block_are_not_added_twice() {
        let raw = "Usage: od [-abcdfilosx]... [FILE]...\n\nOptions:\n  -a    named characters\n  -b    octal bytes\n";
        let parsed = parse(raw);
        for member in ['a', 'b'] {
            let matches: Vec<&Entity> = parsed
                .flags
                .iter()
                .filter(|f| f.short() == Some(member))
                .collect();
            assert_eq!(matches.len(), 1, "-{member}: {matches:?}");
            assert!(
                matches[0].description.is_some(),
                "-{member} must keep the described version"
            );
        }
        // ...and the members the table never described are still recovered.
        for member in ['c', 'd', 'f', 'i', 'l', 'o', 's', 'x'] {
            assert!(
                parsed.flags.iter().any(|f| f.short() == Some(member)),
                "-{member} missing from {:?}",
                parsed.flags
            );
        }
    }

    /// Do-not-double-count: a flag documented in *both* the usage synopsis
    /// and an `Options:` block must collapse to one entry, with the
    /// described version's fields — never two `Flag`s for the same
    /// spelling, and never the description dropped in favor of the
    /// synopsis's bare spelling.
    #[test]
    fn usage_and_options_block_duplicates_merge_into_one_described_flag() {
        let raw =
            "usage: widget [--verbose] [<file>]\n\nOptions:\n  --verbose    print extra output\n";
        let parsed = parse(raw);
        let verbose: Vec<&Entity> = parsed
            .flags
            .iter()
            .filter(|f| f.long() == Some("verbose"))
            .collect();
        assert_eq!(
            verbose.len(),
            1,
            "expected exactly one flag, got {verbose:?}"
        );
        assert_eq!(
            verbose[0].description.as_ref().map(|d| d.as_str()),
            Some("print extra output")
        );
    }

    /// A synopsis with nothing dash-led at all (no `[OPTIONS]`-shaped
    /// bracket carries a real flag, just a positional-only usage line)
    /// must recover zero flags — the extractor must not invent structure
    /// from empty input, mirroring `apt-get`'s real-world shape (spec
    /// [M-15]: "apt-get's zero flags is correct").
    #[test]
    fn usage_synopsis_with_no_dash_tokens_yields_zero_flags() {
        let parsed = parse("Usage: mytool [FILE]... <target>\n");
        assert!(parsed.flags.is_empty(), "{:?}", parsed.flags);
    }

    // --- the stanza's own description as its group label ----------------

    /// The one document shape that reaches this rule at all, spelled out
    /// once here because it is not obvious and every test below reuses it:
    /// a stanza is labelled by the **section** loop only after the usage
    /// block has already ended, since a stanza the block itself reaches
    /// becomes a `usage` entry with no group. So the preamble is a first
    /// stanza that anchors the block, then a description too short for
    /// `is_prose_sentence` to skip (`MIN_PROSE_SENTENCE_WORDS`, spec §7's
    /// own recorded gap) which ends it, and only then the stanzas this
    /// rule labels. Real `vgchange` has exactly this shape, and it is why
    /// the two floors are separate numbers.
    const STANZA_PREAMBLE: &str =
        "tool\n\t[ -x|--xflag ]\n\nDo a thing.\ntool -a|--activate y|n|ay\n\t[ -f|--force ]\n";

    /// A stanza reached by the section loop (its own head line read as the
    /// heading governing the bracket rows beneath it) is labelled by the
    /// description sentence above the head, its head flag and its bracket
    /// rows agree on that label, and the head line itself is kept as a
    /// usage form rather than discarded along with the label it used to be.
    #[test]
    fn stanza_group_label_is_the_description_above_its_head() {
        let help = format!(
            "{STANZA_PREAMBLE}\nStart the lockspace of a shared VG in lvmlockd.\ntool --lockstart\n\t[ -S|--select String ]\n\t[ COMMON_OPTIONS ]\n"
        );
        let parsed = parse_with_profile(&help, None, Some("tool"));
        let label = Some("Start the lockspace of a shared VG in lvmlockd.");
        let select = parsed
            .flags
            .iter()
            .find(|f| f.long() == Some("select"))
            .unwrap_or_else(|| panic!("flags: {:?}", parsed.flags));
        assert_eq!(select.group.as_deref(), label, "flags: {:?}", parsed.flags);
        let lockstart = parsed
            .flags
            .iter()
            .find(|f| f.long() == Some("lockstart"))
            .unwrap_or_else(|| panic!("flags: {:?}", parsed.flags));
        assert_eq!(
            lockstart.group.as_deref(),
            label,
            "the stanza's own head flag and its bracket rows must agree"
        );
        assert!(
            parsed.usage.iter().any(|u| u == "tool --lockstart"),
            "the head line must survive the relabel as a usage form: {:?}",
            parsed.usage
        );
    }

    /// The no-description fallback, which this change must not regress: a
    /// stanza with a blank line above its head keeps the head line as its
    /// label exactly as before, and contributes no usage entry.
    #[test]
    fn stanza_without_a_description_keeps_its_head_line_label() {
        let help = format!(
            "{STANZA_PREAMBLE}\ntool --lockstart\n\t[ -S|--select String ]\n\t[ COMMON_OPTIONS ]\n"
        );
        let parsed = parse_with_profile(&help, None, Some("tool"));
        let select = parsed
            .flags
            .iter()
            .find(|f| f.long() == Some("select"))
            .unwrap_or_else(|| panic!("flags: {:?}", parsed.flags));
        assert_eq!(select.group.as_deref(), Some("tool --lockstart"));
        assert!(
            !parsed.usage.iter().any(|u| u == "tool --lockstart"),
            "no relabel, so nothing to retain: {:?}",
            parsed.usage
        );
    }

    /// The anti-paragraph clause. A sentence that is the *last line of a
    /// paragraph* — something above it, no blank between — is not a
    /// description of the stanza beneath it, and adopting it would label
    /// the group with the tail of unrelated prose. Refused whole; the head
    /// line keeps the label.
    #[test]
    fn trailing_line_of_a_paragraph_is_not_adopted_as_a_stanza_label() {
        let help = format!(
            "{STANZA_PREAMBLE}\nThis tool changes volume group attributes and is\ndocumented at length in the lvm manual page.\ntool --lockstart\n\t[ -S|--select String ]\n"
        );
        let parsed = parse_with_profile(&help, None, Some("tool"));
        let select = parsed
            .flags
            .iter()
            .find(|f| f.long() == Some("select"))
            .unwrap_or_else(|| panic!("flags: {:?}", parsed.flags));
        assert_eq!(
            select.group.as_deref(),
            Some("tool --lockstart"),
            "flags: {:?}",
            parsed.flags
        );
    }

    /// Every remaining clause of the recognizer, each refused on its own:
    /// a differently-indented neighbour, a two-column table row, a line
    /// opening with the tool's own name, an ellipsis, and a head that
    /// names no flag at all (a bare `<tool>` invocation, which is not a
    /// stanza head).
    #[test]
    fn stanza_description_recognizer_refuses_every_near_miss() {
        let lines = [
            "",
            "  Start the lockspace of a VG.",
            "  vgchange --lockstart",
        ];
        assert_eq!(
            stanza_description_above(&lines, 2, Some("vgchange")),
            Some("Start the lockspace of a VG.")
        );
        for near_miss in [
            // Not at the head's own column.
            "      Start the lockspace of a VG.",
            // A two-column table row, not a sentence.
            "  lockstart          Start the lockspace of a VG.",
            // The tool's own name: another invocation line, never a label.
            "  vgchange starts the lockspace of a VG.",
            // Repetition notation, not a sentence terminator.
            "  Start the lockspace of a VG...",
            // No terminator at all.
            "  Start the lockspace of a VG",
            // Under the word floor.
            "  Lockstart.",
        ] {
            let lines = ["", near_miss, "  vgchange --lockstart"];
            assert_eq!(
                stanza_description_above(&lines, 2, Some("vgchange")),
                None,
                "adopted {near_miss:?}"
            );
        }
        // A bare own-name head names no mode, so it is not a stanza head
        // and nothing above it is a stanza description.
        let lines = ["", "  Read information about a VG.", "  vgchange"];
        assert_eq!(stanza_description_above(&lines, 2, Some("vgchange")), None);
        // No resolved tool name: the head line cannot be known to be an
        // invocation of *this* tool rather than a coincidence.
        let lines = [
            "",
            "  Start the lockspace of a VG.",
            "  vgchange --lockstart",
        ];
        assert_eq!(stanza_description_above(&lines, 2, None), None);
    }

    /// A malformed/unmatched bracket in a usage line (never seen from a
    /// real tool, but the parser must not panic or misbehave on it) falls
    /// back to treating the stray `[` as part of an ordinary bare token —
    /// still gated by the same "starts with `-`" rule, so it recovers
    /// nothing rather than guessing.
    #[test]
    fn unmatched_bracket_in_usage_line_does_not_panic() {
        let parsed = parse("usage: widget [--flag <value>\n");
        // No panic is the primary assertion; the bracket does eventually
        // close over word boundaries in a way that still yields --flag,
        // since `[--flag` is bare (starts with `-`... actually with `[`)
        // and `<value>` is not flag-shaped. This just documents there is
        // no crash and no fabricated flag from the stray bracket itself.
        assert!(parsed.flags.iter().all(|f| f.long() != Some("")));
    }
}
