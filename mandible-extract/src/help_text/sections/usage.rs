//! The synopsis: recognizing a usage line or an unlabelled stanza head, and
//! mining the synopsis itself for positionals and for flags no option
//! table documents.

use super::*;

/// True if `t` starts with `"usage:"`, case-insensitively.
///
/// Compares raw bytes via `[u8]::get` rather than slicing the `str`, which
/// can panic when a multi-byte character (e.g. a box-drawing glyph) lands
/// off a UTF-8 boundary at the slice point.
pub fn starts_with_usage_prefix(t: &str) -> bool {
    t.as_bytes()
        .get(..6)
        .map(|b| b.eq_ignore_ascii_case(b"usage:"))
        .unwrap_or(false)
}

/// True if `t` starts with `"or:"`, case-insensitively — GNU coreutils'
/// marker for a genuine *alternative* invocation form, distinct from a
/// wrapped continuation of the form above it. Without it, joining every
/// more-indented usage line onto its predecessor would swallow `or:`'s
/// alternative form too. See S-037 and corpus/du/9.4/help.txt.
pub fn starts_with_or_marker(t: &str) -> bool {
    t.as_bytes()
        .get(..3)
        .map(|b| b.eq_ignore_ascii_case(b"or:"))
        .unwrap_or(false)
}

/// True if `t` (already trimmed of leading whitespace) begins with `name`
/// at a word boundary. Lets a tool that repeats its own name across lines
/// with no `or:`/`usage:` marker read as two entries rather than one
/// continuation swallowing the other. Word-boundary checked so `git`
/// doesn't also claim `gitk` or `git-foo`. See S-037.
pub fn starts_with_tool_name(t: &str, name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    match t.strip_prefix(name) {
        Some(rest) => rest.is_empty() || rest.starts_with(char::is_whitespace),
        None => false,
    }
}

/// True if `t` (already trimmed) is the C `fprintf(stderr, "%s: Usage:
/// ...", argv[0])` idiom's line: the tool's own name, a literal `": "`,
/// then `usage:` case-insensitively. [`starts_with_usage_prefix`] alone
/// misses this since it only tests the line's start. Kept tight: the
/// `usage:` must be preceded by *only* the name and `": "`, never scanned
/// for elsewhere in the line. See S-001 and corpus/nfsidmap/audit-seed/help.txt.
pub fn starts_with_name_prefixed_usage(t: &str, name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    t.strip_prefix(name)
        .and_then(|rest| rest.strip_prefix(": "))
        .is_some_and(starts_with_usage_prefix)
}

/// True if `t` opens with `name` at a word boundary and its remainder
/// reads as usage-synopsis grammar rather than prose — the unlabelled
/// synopsis convention (`wpa_cli --help` opens `wpa_cli [-p<path>]
/// [-i<ifname>] ...` with no `Usage:` marker at all). A name match alone
/// is not evidence (`"tar is an archiving program..."` starts with `tar`
/// too), so both must hold: the remainder contains a docopt group
/// delimiter (spec §7 Tier B: `[`, `<`, `{`), and it does not read as an
/// English sentence ([`is_prose_sentence`]). See S-001 and wpa_cli's help.
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
/// notation on the line itself — whose very next physical line is
/// unambiguous flag-row evidence: [`looks_like_bracket_flag_row`] or
/// [`looks_like_paren_alternation_open`]. Shared by the unlabelled-
/// synopsis entry point and the multi-stanza continuation check below, so
/// a second copy can't drift. Never keyed on tool name alone — a name-only
/// line counts only with next-row evidence. See S-005 and vgck/vgchange.
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
/// --updatemetadata VG`, no bracket group at all) or is followed by
/// [`looks_like_bracket_flag_row`] evidence, as
/// [`looks_like_bare_synopsis_head`] requires for the first stanza. A
/// second stanza's own flag token is stronger evidence than a lookahead
/// row, so it is accepted here though not at the entry point — that
/// point's name-only line must stay stricter since it scans the whole
/// document fresh, while this only ever runs inside an already-open
/// synopsis. See S-005 and vgck's second stanza, adduser, pydoc3.
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
    // Refuse a stanza whose own description wraps across more than one
    // physical line: the continuation loop below only recognizes a single
    // line as "prose to drop" (must end in a period), so a two-line
    // description's interior line would otherwise be read as more usage
    // notation and mined for fabricated positionals. pydoc3's `-p`/`-b`/
    // `-w` forms are exactly this case. See S-005 and pydoc3's help text.
    match next {
        None => true,
        Some(n) if n.trim().is_empty() => true,
        Some(n) if looks_like_bracket_flag_row(n) || looks_like_usage_fragment(n) => true,
        Some(n) if is_prose_sentence(n) => true,
        _ => false,
    }
}

/// True if `t` opens with a docopt-style group delimiter (spec §7 Tier B:
/// `[`, `<`, `{`) — still more invocation syntax, not the next section
/// starting. Content-shape half of the usage-block continuation
/// discriminator: indentation alone can't separate a same-indent
/// continuation (lsof's wrapped `[-F [f]] ...`) from a same-indent line
/// that legitimately ends the block (du's trailing prose sentence). See
/// S-037.
pub(super) fn looks_like_usage_fragment(t: &str) -> bool {
    matches!(t.as_bytes().first(), Some(b'[') | Some(b'<') | Some(b'{'))
}

/// Recover the mode-selecting flag a stanza head line itself names
/// (`grammar::looks_like_stanza_head_flag`'s shape). `None` for a bare
/// invocation naming no flag (`vgchange` alone), a heading that isn't the
/// tool's own name, an ignorable heading (an `Examples:` block must never
/// be mined this way), or an unresolved `tool_name`. Never fabricates
/// required-ness — LVM's "any one is required" prose has no IR field to
/// spend on. See S-005.
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
/// label. Three, deliberately not [`MIN_PROSE_SENTENCE_WORDS`]'s five: that
/// floor guards against claiming a two/three-word heading anywhere in the
/// document, while this one only ever runs in the single slot between a
/// blank line and a confirmed stanza head, where vgchange's own four-word
/// `Activate or deactivate LVs.` is the shortest real specimen measured.
/// See S-012.
pub(super) const MIN_STANZA_DESCRIPTION_WORDS: usize = 3;

/// The description sentence a multi-variant tool writes directly above a
/// usage stanza's head line, when `lines[head_idx]` is such a head and the
/// line above it is such a sentence — LVM's own one-stanza-per-form
/// emitter:
///
/// ```text
///   Start the lockspace of a shared VG in lvmlockd.
///   vgchange --lockstart
/// \t[ -S|--select String ]
/// ```
///
/// The head must already be accepted by [`recover_stanza_head_flag`]. The
/// line above it must: sit at the head's own column; stand alone (its own
/// predecessor blank or absent — the anti-paragraph clause, so a
/// hard-wrapped or trailing-paragraph sentence is refused whole rather
/// than adopted as a fragment); end in a full stop, not an ellipsis; be a
/// single field ([`find_multi_space_gap`]); not open with the tool's own
/// name or with flag/usage notation; and carry at least
/// [`MIN_STANZA_DESCRIPTION_WORDS`] words while not being an
/// [`is_ignorable_heading`] marker. Returned verbatim, terminator
/// included — the display layer strips it (spec §9.3). See S-012.
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
/// `...`, `|`, `{a|b|c}`), so this recognizes common placeholder shapes
/// rather than fully parsing the grammar. Stays narrow since it infers
/// from notation; [`OPTION_LIST_PLACEHOLDERS`] carves out the token
/// family whose notation is indistinguishable from an operand's while its
/// meaning is the opposite. See S-004.
/// The offsets into `usage_lines` of every physical line folded into the
/// first recovered usage *entry* carrying real invocation content —
/// skipping a bare `Usage:`/`or:` label with nothing after the colon
/// (util-linux's `renice`), and following a wrapped entry across every
/// line it spans (`sg_sanitize`'s five-line synopsis). Anchors
/// [`extract_positionals`]'s self-closed-bracket-group refinement to
/// exactly these lines, not every line. See S-004.
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
/// those. See S-004.
pub(super) fn extract_positionals(
    usage_lines: &[String],
    primary_lines: std::collections::HashSet<usize>,
) -> Vec<Entity> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for (line_idx, line) in usage_lines.iter().enumerate() {
        // A value-shaped token immediately following a bare flag token
        // (`-C <path>`) is that flag's argument, not a positional.
        // `prev_cleaned` tracks the preceding token's cleaned spelling so
        // the loop can tell the two apart; resets every physical line.
        let mut prev_cleaned: Option<&str> = None;
        // Whether the immediately preceding raw token was a complete,
        // self-closed bracket group (`[-v]`) rather than a still-open one
        // (`[-C`, expecting `<path>]`) or a bare flag (`-C`, expecting
        // `<path>`). sg_emc_trespass's `[-d] [-hr] [-s] [-V] DEVICE`
        // closes `[-V]` before `DEVICE` appears, so `DEVICE` is a real
        // positional, not `-V`'s argument. Scoped to
        // [`primary_synopsis_lines`] deliberately: fleet-wide it also
        // claimed a later same-name alternate form (`jps`'s second line)
        // and an unlabelled synopsis (`lvreduce`'s bare form), both of
        // which the existence oracle doesn't yet attest, so recovering
        // operands from them there reports as invented. See S-004 and
        // corpus/sg_emc_trespass/audit-seed2/help.txt.
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

/// Extract flag spellings from a usage-synopsis block: usage-only options
/// (`git --help`'s `[-p | --paginate | -P | --no-pager]`) are otherwise
/// never mined at all (spec [M-15]). [`extract_positionals`] reads the
/// same block for positionals only.
///
/// Anti-fabrication property: a synopsis token becomes a flag only if it
/// starts with `-` — no heading to misjudge, no bare-word block that
/// might be prose, so this stays resistant to [M-10] by construction; do
/// not relax it. Flags recovered here carry no description (a usage line
/// never documents prose); a same-spelling flag that does have one is
/// reconciled by [`parse_with_profile`] via
/// [`flag_spelling_already_present`], which drops the duplicate rather
/// than merging it. See S-088.
pub(super) fn extract_usage_flags(usage_lines: &[String]) -> Vec<Entity> {
    let mut out: Vec<Entity> = Vec::new();
    // Running depth of an open parenthesized alternation group (LVM's
    // "any one of these is required" convention), re-derived here rather
    // than passed in since `usage_lines` alone determines the same
    // open/close boundaries. A member row routinely opens with `-` itself
    // (`-p|--maxphysicalvolumes Number,`) and must go through
    // `paren_alternation_member_content`, not the ordinary segment walk,
    // which has no notion of a comma-terminated alternative. See S-088.
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
        // generic segment walk below: `usage_segments` splits on every
        // top-level `|` unconditionally, which would read
        // `-A|--autobackup y|n` as three alternatives and lose
        // `--autobackup`'s real value `y|n` down to just `y`. See S-088.
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
                        // A member that is itself a delimited alternation
                        // of flag spellings, optionally with a shared
                        // operand — xfs_io's `[[-c|-C] cmd]...`. See S-088.
                        for spec in nested_alternation_specs(m) {
                            if out.len() >= MAX_RECOVERED_ENTRIES {
                                return out;
                            }
                            push_usage_flag(&mut out, spec);
                        }
                    }
                    // spec [M-15]'s conservative-pairing rule: within one
                    // bracket group, pair a short with a long only when
                    // the group has exactly one of each — `[-v |
                    // --version]` qualifies, git's four-way paginate
                    // alternation does not, and a wrong pairing asserts a
                    // false equivalence worse than an unpaired entry. A
                    // bundle (`-2CDlNuVv`) is never one half of a pair, so
                    // the cluster question is asked first. See S-087.
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
                        // A mandatory flag some synopsis writes unbracketed
                        // (`ssh-keygen -D pkcs11`) is two bare tokens: the
                        // flag, then its required value with no group at
                        // all. Outside a `[...]` group each token used to
                        // stand alone, so the value read as an unrelated,
                        // dropped bare word. Attaching is refused when
                        // `tok` is a bundle of single-char switches
                        // (booleans by construction), when the next
                        // segment is missing/empty/flag-shaped (`-k -f
                        // krl_file`: `-k` stays boolean), or when the next
                        // word opens a parenthetical aside — iptables'
                        // `-h (print this help information)`. See S-040.
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
/// [`nested_alternation_specs`] will read it as one. Two: a one-member
/// nested group is `[[-v] file]`, an ordinary optional flag already read
/// correctly by [`usage_segments`] plus the pairing rule. See S-088.
pub(super) const MIN_NESTED_ALTERNATIVES: usize = 2;

/// Read one member of a usage-synopsis group as a nested alternation of
/// flag spellings sharing a single operand — xfs_io's `[[-c|-C] cmd]...`.
/// Returns one [`FlagSpec`] per alternative, each carrying the shared
/// operand as a required value, or the *paired* single spec under
/// spec [M-15]'s conservative-pairing rule. Empty when the member is not
/// this shape. The operand is refused unless it is one clean token — the
/// alternatives are still emitted with no value rather than a guessed
/// one. See S-088.
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
/// the group is one clean value token and nothing else. `None` for empty
/// text, a second word, or a flag-shaped token. See S-088.
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

/// True if `candidate` shares a spelling — short letter or long name —
/// with any flag already in `existing`. Deliberately loose in `existing`'s
/// favor: matching on *either* spelling (not the identity combination
/// [`mandible_core::merge::flag_identity`] keys on) catches arptables'
/// `--insert, -I` row against a bare `-I` mentioned standalone elsewhere,
/// and an existing flag is never altered here, only left alone or joined.
/// A one-letter abbreviation bracket also counts as a short-letter match:
/// `ip`'s `[ -force ]` reads as `-f` glued to `"orce"` on the short path,
/// but the table documents it as `-f[amily]` (long-like). See S-088.
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
/// every other shape — the single flag [`parse_flag_spec`] reads. The
/// bundle question is asked only here, on the synopsis path, never inside
/// [`parse_flag_spec`]: the identical glued shape in an option *table* row
/// is the GCC/Clang single-dash convention (`-Wall`), where it genuinely
/// is a value. Members are emitted as bare booleans — no value, no
/// description — since fabricating one from the usage line's own text is
/// the spec §7 Tier B violation [`extract_usage_flags`] forbids. See
/// S-087.
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
/// recognized nothing (a stray `-`/`--` terminator). `group`/`description`
/// are always `None`. Provenance is [`Source::HelpTextSynopsis`], not
/// plain [`Source::HelpText`] — same authority (spec §4.4 unaffected),
/// but distinct so spec §13's `pct_flags_with_text` can tell a
/// structurally-undescribable flag apart from one merely undescribed. See
/// S-088.
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
/// (`-p[d]`, name `"pd"`) [`FlagSpec::long`] also reads as long-like.
/// [`pair_short_and_long`] restricts its "long" side to this. See S-087.
fn long_is_double_dash(spec: &FlagSpec) -> bool {
    spec.spellings
        .iter()
        .any(|s| matches!(s.dashes, Dashes::Double))
}

/// Pair a short-only and a long-only [`FlagSpec`] into one, or refuse
/// (`None`) if not exactly complementary (spec [M-15]'s conservative
/// pairing rule). The "long" side must be a genuine double-dash spelling
/// ([`long_is_double_dash`]): the single-dash abbreviation-bracket form
/// (`-p[d]`) can name a semantically distinct flag rather than a second
/// spelling of the same one — pppdump's `[-h | -p[d]]` counter-example,
/// `-h` (hex dump) and `-p[d]` (printable) are two real flags, not one's
/// two spellings. Refusing costs no recall: both still reach the tree,
/// just unmerged. See S-087.
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
        // `{` opens a group on the same terms as `[`. Before this, eqn's
        // `usage: eqn {-v | --version}` split into three bare tokens on
        // the spaces around `|`, reading `--version` as carrying the
        // literal value `"}"`. See S-088.
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
/// and its matching close, depth-aware over that one pair, so
/// `[--exec-path[=<path>]]`'s inner `[...]` is consumed as outer-group
/// content rather than closing the group early. `None` when the delimiter
/// is never closed; the caller falls back to an ordinary bare token. See
/// S-088.
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
/// `never}`, emitting a flag whose value was the fragment `{always`. See
/// S-088.
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

    // --- multi-stanza unlabelled synopsis ---

    /// jar's `Examples:` block chains several real flags on one line with
    /// no brackets, structurally indistinguishable from a bare stanza
    /// head. See S-071.
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

    /// The complete OpenJDK examples shape: the prose sentence above each
    /// `Examples:` marker must not own it, example rows must never become
    /// flags, and real sections after must still parse. See S-071.
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

    /// A same-indent, colon-terminated label inside an examples block
    /// (`Input:`/`Output:`) is not by itself a new CLI section, but a
    /// positively identified options section after it must still reopen
    /// normal parsing. See S-071.
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

    /// `vgck --updatemetadata` — a second stanza past the blank line — is
    /// its own usage entry and flag; its prose head lands in neither. See
    /// S-037.
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

    /// git's wrapped hanging-indent synopsis, followed by an unrelated
    /// blank-line-separated paragraph opening with `git` again, must not
    /// be read as a second usage entry. See S-037.
    #[test]
    fn labelled_usage_block_does_not_reopen_on_a_later_blank_line() {
        let help = "Usage: git [--version] [--help] <command> [<args>]\n\n\
                     git clone is used to clone repositories.\n\
                     git clone [--bare] <repo>\n\
                     \t[--depth <n>]\n";
        let parsed = parse_with_profile(help, None, Some("git"));
        assert_eq!(parsed.usage.len(), 1, "usage: {:?}", parsed.usage);
    }

    /// corepack's headingless invocation table (one `<tool> <subcommand>
    /// ...` row per blank-line-separated stanza) must not be reopened as
    /// usage text — that would demote a real recovered subcommand. See
    /// S-005.
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

    /// pydoc3's multi-line stanza description (`-p`) is refused from the
    /// usage block (its wrapped line would otherwise be mined for
    /// fabricated positionals `HTTP`/`HTML`), but `-p`'s flag is still
    /// recovered via the independent `recover_stanza_head_flag` path,
    /// which reads only the head line, never the wrapped description. See
    /// S-005.
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
    // The two tests above already pin corepack's headingless table and
    // pydoc3's multi-line description; both fixtures contain no `(`, so
    // `paren_group_depth` is never reached by them.

    /// A bare vgchange-shaped synopsis whose first continuation opens a
    /// multi-line `(` group, one flag per line, closed by `)` on the last
    /// member's own line. Every member recovers a clean value. See S-088.
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

    /// vgchange's own wrinkle: trailing bracket-row flags sit after a
    /// blank line separating them from the group's closing `)` — still
    /// the same stanza. Pins `just_closed_paren_group`. See S-088.
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
    // LVM's real vgchange --help documents six stanza-head flags this
    // batch of synthetic fixtures pins, one hazard per test. See S-005 and
    // corpus/vgchange/2.03.16.

    /// The multi-alias head: `-a|--activate y|n|ay` reads as one flag, not
    /// three. See S-005.
    #[test]
    fn stanza_head_multi_alias_flag_reads_as_one_flag_with_its_value() {
        // A too-short second-stanza description breaks the usage block's
        // continuation, mirroring vgchange's own shape.
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
        // `Do a thing.` is this stanza's own description, so it becomes
        // the group label and the head line is retained as a usage form.
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

    /// A value-plus-positional stanza head, `--systemid String VG`, leaves
    /// the trailing `VG` alone as a positional, not a second flag. See
    /// S-005.
    #[test]
    fn stanza_head_value_plus_positional_leaves_the_positional_alone() {
        // `[ COMMON_OPTIONS ]` is a placeholder row, never a flags block on
        // its own (its content doesn't start with `-`).
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

    /// A bare stanza head with no flag at all yields no flag. See S-005.
    #[test]
    fn bare_stanza_head_with_no_flag_yields_no_flag() {
        let help = "tool\n\t[ -f|--force ]\n";
        let parsed = parse_with_profile(help, None, Some("tool"));
        assert_eq!(parsed.flags.len(), 1, "flags: {:?}", parsed.flags);
        assert_eq!(parsed.flags[0].long(), Some("force"));
    }

    /// A two-word section heading not named after the tool is never read
    /// as a stanza head flag, even when followed by `[...]` flag rows. See
    /// S-005.
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

    /// bpftrace's `EXAMPLES:` invocation lines (tool name, flag, one-line
    /// description) must never be read as stanza heads — they would
    /// otherwise displace the real, described flags from `OPTIONS:`. See
    /// S-071.
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

    /// vgck --help, byte-exact: an unlabelled synopsis with a
    /// continuation-row flag and 18 headed bracket rows. See S-088.
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

        // 20 flags total: the 18 rows, plus --reportformat from the first
        // stanza's continuation and --updatemetadata from the second
        // stanza's head. See S-088.
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

    /// vgck's `[ COMMON_OPTIONS ]`/`[ VG|Tag ... ]` operand brackets must
    /// never be read as flags. See S-088.
    #[test]
    fn vgck_never_fabricates_a_flag_from_an_operand_bracket() {
        let parsed = parse_with_profile(VGCK_HELP, None, Some("vgck"));
        assert!(parsed
            .flags
            .iter()
            .all(|f| f.long() != Some("COMMON_OPTIONS")));
        assert!(parsed.flags.iter().all(|f| f.long() != Some("VG")));
    }

    /// vgextend's alias-cluster, choice-list, and nested-bracket-value
    /// rows in one richer synopsis. See S-088.
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

    /// ethtool's alternation between two different flags, only one
    /// carrying its own bracketed operands, must be refused whole — not
    /// LVM's shape, since LVM's own alias run never has a bare flag
    /// spelling reappear after the first whitespace gap. See S-088.
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

    /// curl's flag list runs straight into its usage line with no blank
    /// line or heading, and was previously swallowed whole. See S-037.
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

    /// git's five-physical-line wrapped synopsis fabricated a fake extra
    /// invocation before this fix; the fix produces exactly one entry,
    /// joined by a single space (spec §7: usage is kept verbatim). See
    /// S-037.
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

    /// du's `or:` marker keeps two genuine alternative forms separate
    /// despite deeper indentation than the block's base. See S-037.
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

    /// A tool repeating its usage: label per form gets one entry per
    /// label. See S-037.
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

    /// A tool repeating its own name with no marker at all reads as two
    /// entries when the name is known. See S-037.
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
        // Without a known tool name the same repeated line instead reads
        // as one hanging-indent continuation (git's wrapped shape). See
        // S-037.
        let unnamed = parse(raw);
        assert_eq!(unnamed.usage, vec!["Usage: prog foo prog bar".to_string()]);
    }

    /// lsof's three same-indent continuation lines were previously
    /// dropped, losing six documented flags. See S-037.
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
        // The over-join guard's counterpart: a trailing prose sentence at
        // the same column still ends the block. See S-037.
        assert!(!parsed.usage[0].contains("Defaults"), "{:?}", parsed.usage);
        let short_flags: Vec<Option<char>> = parsed.flags.iter().map(|f| f.short()).collect();
        // Spot-check flags documented only in lsof's dropped continuation
        // lines, none of them from line one's own bundled `-?...` blob
        // (which parses as one flag, not fourteen). See S-037.
        for want in ['F', 'g', 'L', 'M', 'r', 'u'] {
            assert!(
                short_flags.contains(&Some(want)),
                "expected -{want} recovered from lsof's continuation lines, got {short_flags:?}"
            );
        }
    }

    /// ip's stderr-only, exit-255 help is checked only for structural
    /// presence, spec [M-8].
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

    /// git's greedy-bracket-match and flag-argument-as-positional
    /// regressions from the corpus xfail fixture: the fix must produce
    /// exactly `command` and `args`. See S-004 and corpus/git/2.43.0/.
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

    /// The general rule behind the git regression, spelled out with a
    /// synthetic usage line: a bare flag's value never becomes a
    /// positional, while a flag with an inline `=` value leaves the next
    /// token free. See S-004.
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

    /// sg_emc_trespass's self-closed `[-V]` leaves `DEVICE` as a real
    /// positional, not `-V`'s argument. See S-004.
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

    /// vim's own `[arguments]` placeholder must never be extracted as an
    /// operand, confirmed with the maintainer. See S-060.
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

    /// Why `args`/`arg` are absent from [`OPTION_LIST_PLACEHOLDERS`]: a
    /// real operand must still be recognized. See S-060.
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

    /// git's own reference example for [M-15]: pairing `[-v|--version]`
    /// but not the four-way paginate alternation. See S-087.
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

    /// pppdump's `-h`/`-p[d]` counter-example, byte-exact, pinning the
    /// abbreviation-bracket pairing refusal. See S-087.
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

    /// Usage-derived flags carry `Source::HelpTextSynopsis` so spec §13's
    /// metric can exclude them from the denominator.
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

    /// `-C <path>`, `--git-dir=<path>`, and `--exec-path[=<path>]`'s
    /// required-versus-optional value shapes. See S-088.
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

    /// ssh-keygen's own unbracketed mandatory values across three synopsis
    /// lines. See S-040.
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

    /// iptables' own `-h (print this help information)` parenthetical
    /// aside, byte-exact from a fleet sweep. See S-040.
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

    /// ssh-keygen's `-k -f krl_file`, a bare flag followed by another flag
    /// stays boolean. See S-040.
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

    /// tmux's real synopsis cluster becomes eight boolean switches
    /// alongside five untouched value-taking flags. See S-087.
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

    /// filefrag's glued value spec beside a cluster stays one valued flag,
    /// the synopsis-only cluster question. See S-087.
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

    /// An options-table row of the identical glued shape, the GCC/Clang
    /// convention, is never split. See S-087.
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

    /// od's synopsis-plus-table bundle must not double-count already-
    /// described cluster members. See S-088.
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

    /// A flag documented in both the synopsis and an Options: block
    /// collapses to one described entry. See S-088.
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

    /// apt-get's genuinely zero-flag positional-only usage line, spec
    /// [M-15].
    #[test]
    fn usage_synopsis_with_no_dash_tokens_yields_zero_flags() {
        let parsed = parse("Usage: mytool [FILE]... <target>\n");
        assert!(parsed.flags.is_empty(), "{:?}", parsed.flags);
    }

    // --- the stanza's own description as its group label ----------------

    /// The one document shape that reaches the stanza-label rule at all,
    /// real vgchange's own layout: a first stanza that anchors the usage
    /// block, then a too-short description that ends it, then the stanzas
    /// this rule labels. See S-012.
    const STANZA_PREAMBLE: &str =
        "tool\n\t[ -x|--xflag ]\n\nDo a thing.\ntool -a|--activate y|n|ay\n\t[ -f|--force ]\n";

    /// A stanza's description labels both its head flag and its bracket
    /// rows, and the head line survives as a usage form. See S-012.
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

    /// The no-description fallback: a stanza with no description keeps its
    /// head line as its own label. See S-012.
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

    /// The anti-paragraph clause: a paragraph's trailing sentence is never
    /// adopted as a stanza label. See S-012.
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

    /// Every remaining refusal clause of the stanza-description
    /// recognizer, one near-miss each. See S-012.
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

    /// A malformed unmatched bracket in a usage line must not panic, never
    /// seen from a real tool.
    #[test]
    fn unmatched_bracket_in_usage_line_does_not_panic() {
        let parsed = parse("usage: widget [--flag <value>\n");
        assert!(parsed.flags.iter().all(|f| f.long() != Some("")));
    }
}
