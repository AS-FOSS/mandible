//! Two shapes issue #135 named, both about a usage-synopsis token or
//! bracket group carrying more than one real word (docs/shapes.md S-131,
//! S-132). Split out of `usage.rs` to keep that file under its own size
//! ceiling (AGENTS.md §2); both functions are called from there and rely
//! on several of its `pub(super)` helpers.

use super::*;

/// A usage-synopsis bracket-group member that is a single flag spelling
/// followed by two or more plain words with nothing else on the group
/// (`caffeinate`'s `-w Process ID`) — a value placeholder that itself
/// holds a space. [`grammar::try_value`]'s bare-value read only ever takes
/// the first word, so the rest used to fall through
/// [`extract_positionals`]'s own ALL-CAPS scan and get invented as its own
/// positional. Refuses when any trailing word is not plain prose. See
/// docs/shapes.md S-131 and corpus/caffeinate/26.6.2.
pub(super) fn multi_word_value_group(member: &str) -> Option<FlagSpec> {
    let mut words = member.split_whitespace();
    let flag_tok = words.next()?;
    if !flag_tok.starts_with('-') || flag_tok.len() < 2 {
        return None;
    }
    let rest: Vec<&str> = words.collect();
    if rest.len() < 2 {
        return None;
    }
    let mut value = String::new();
    for w in &rest {
        if w.is_empty()
            || !w.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
            || w.chars()
                .any(|c| matches!(c, '-' | '[' | ']' | '(' | ')' | '<' | '>' | '|' | '='))
        {
            return None;
        }
        if !value.is_empty() {
            value.push(' ');
        }
        value.push_str(w);
    }
    let mut spec = parse_flag_spec(flag_tok);
    if spec.spellings.is_empty() {
        return None;
    }
    spec.value_name = Some(value);
    spec.value_kind = ValueKind::Required;
    spec.fully_consumed = true;
    Some(spec)
}

/// A plain prose word: lowercase-led, only letters/digits/`-`/`_`.
fn plain_word(w: &str) -> bool {
    !w.is_empty()
        && w.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && w.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// True when a bracket group's own stripped content (brackets removed) is
/// the [`multi_word_value_group`] shape: one flag spelling followed by
/// two or more plain-prose words. Same word rule that function uses
/// (alphabetic-led, so `Process`/`ID` both qualify) — deliberately looser
/// than [`plain_word`] above, which requires a lowercase lead and would
/// wrongly refuse `ID`.
fn looks_like_multiword_value_group(stripped: &str) -> bool {
    let mut words = stripped.split_whitespace();
    let Some(flag) = words.next() else {
        return false;
    };
    if !flag.starts_with('-') || flag.len() < 2 {
        return false;
    }
    let rest: Vec<&str> = words.collect();
    rest.len() >= 2
        && rest.iter().all(|w| {
            !w.is_empty()
                && w.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
                && !w
                    .chars()
                    .any(|c| matches!(c, '-' | '[' | ']' | '(' | ')' | '<' | '>' | '|' | '='))
        })
}

/// The primary synopsis's own trailing bracket group of two or more real
/// words, with or without an ellipsis marker (docs/shapes.md S-132).
/// Requires another group on the line to already show the identical
/// multi-word shape as a flag's own value
/// ([`looks_like_multiword_value_group`]): otherwise "two prose words in
/// a bracket" cannot be told apart from a generic "pass this program's
/// own extra options through" aside. See this function's own tests for
/// both directions.
pub(super) fn recover_trailing_multiword_operand(
    usage_lines: &[String],
    primary_lines: &std::collections::HashSet<usize>,
) -> Vec<Entity> {
    let line_idx = match primary_lines.len() {
        1 => match primary_lines.iter().next() {
            Some(&i) => i,
            None => return Vec::new(),
        },
        _ => return Vec::new(),
    };
    let Some(line) = usage_lines.get(line_idx) else {
        return Vec::new();
    };
    let lower = line.to_ascii_lowercase();
    let Some(idx) = lower.find("usage:") else {
        return Vec::new();
    };
    let after = &line[idx + "usage:".len()..];
    let before_desc = cut_before_description_gap(after);
    let groups = group_synopsis_tokens(before_desc.trim());
    // The program name, at least one group ahead of the trailing one, and
    // the trailing group itself.
    if groups.len() < 3 {
        return Vec::new();
    }
    let has_sibling_multiword_value = groups[1..groups.len() - 1]
        .iter()
        .any(|g| looks_like_multiword_value_group(g.trim_matches(|c| c == '[' || c == ']')));
    if !has_sibling_multiword_value {
        return Vec::new();
    }
    let last = groups.last().expect("len checked above");
    let stripped = last.trim_matches(|c| c == '[' || c == ']');
    let mut words = stripped.split_whitespace();
    let Some(raw_first) = words.next() else {
        return Vec::new();
    };
    let first = raw_first.trim_end_matches('.');
    if !plain_word(first) || is_option_list_placeholder(first) {
        return Vec::new();
    }
    let mut name = first.to_string();
    let mut repeatable = token_marks_repetition(raw_first);
    let mut real_word_count = 1usize;
    for w in words {
        if w.chars().all(|c| c == '.') {
            // A separate ellipsis-only token marks the *previous* word
            // repeatable; it is never a word of its own. See S-101.
            repeatable = true;
            continue;
        }
        // Only the group's own *first* word is checked against
        // `OPTION_LIST_PLACEHOLDERS` — a later word in a compound name
        // (`command arguments`) is real text, not the author's generic
        // "any argument" placeholder standing alone.
        let w_clean = w.trim_end_matches('.');
        if !plain_word(w_clean) {
            return Vec::new();
        }
        name.push(' ');
        name.push_str(w_clean);
        repeatable = token_marks_repetition(w) || repeatable;
        real_word_count += 1;
    }
    if real_word_count < 2 {
        return Vec::new();
    }
    let mut positional = Entity::positional(name, Provenance::single(Source::HelpText));
    positional.required = !last.starts_with('[');
    positional.repeatable = repeatable;
    vec![positional]
}

/// One synopsis group read as a brace alternation naming exactly one
/// operand, `(name, required)` — `cache_repair`'s `{device|file}`. The
/// alternation's own source spelling, braces included, is the operand's
/// name, the same rule S-097 already applies to a glued optional-value
/// run: decomposing the notation would lose the fact that exactly one of
/// the members is meant, not all of them in sequence. `None` unless the
/// group (brackets trimmed) is nothing but one brace run with at least
/// one `|` inside and no nested brace. See docs/shapes.md S-153.
pub(super) fn parse_brace_alternation_group(group: &str) -> Option<(String, bool)> {
    let required = !group.starts_with('[');
    let stripped = group.trim_matches(|c| c == '[' || c == ']');
    if !stripped.starts_with('{') || !stripped.ends_with('}') {
        return None;
    }
    let inner = stripped.get(1..stripped.len() - 1)?;
    if inner.is_empty() || inner.contains(['{', '}']) || !inner.contains('|') {
        return None;
    }
    Some((stripped.to_string(), required))
}

/// The bar-separated members named inside a brace alternation's own
/// spelling (`"{device|file}"` -> `["device", "file"]`), for the
/// resulting positional's `choices` — the same members a reader would
/// type. `None` when `name` is not that shape, so a caller never has to
/// re-check what [`parse_brace_alternation_group`] already established.
pub(super) fn brace_alternation_members(name: &str) -> Option<Vec<String>> {
    let inner = name.strip_prefix('{')?.strip_suffix('}')?;
    Some(inner.split('|').map(str::to_string).collect())
}

/// Split `word`'s own trailing run of ASCII digits off as an integer,
/// `None` when there is none or the whole word is digits. Local to this
/// module; `xtask`'s `numbered-variadic-usage-tail` detector keeps its own
/// independent copy rather than importing this one, by that detector's
/// own design (it measures the parser, so it must not share code with
/// it). See docs/shapes.md S-136.
fn split_trailing_integer(word: &str) -> Option<(&str, u64)> {
    let digit_count = word.chars().rev().take_while(char::is_ascii_digit).count();
    if digit_count == 0 || digit_count == word.len() {
        return None;
    }
    let (stem, digits) = word.split_at(word.len() - digit_count);
    digits.parse::<u64>().ok().map(|n| (stem, n))
}

/// The recovered run's own trailing two entries, read as `X1 [X2 ...]`
/// (docs/shapes.md S-136, issue #141): both plain-word operands (never a
/// brace alternation), the first required and not itself repeatable, the
/// second optional and repeatable, both sharing an alphabetic stem and
/// the second's trailing integer exactly the first's own plus one. The
/// numbering is the evidence a bare two-word tail (S-109) lacks, so this
/// collapses the pair to one repeatable operand named by the shared stem
/// — the same shape `[file...]` already produces (S-101) — rather than
/// two fixed operands.
pub(super) fn collapse_numbered_variadic_tail(
    first: &(String, bool, bool, bool, bool),
    second: &(String, bool, bool, bool, bool),
) -> Option<(String, bool, bool, bool, bool)> {
    let (name1, required1, repeat1, brace1, _) = first;
    let (name2, required2, repeat2, brace2, _) = second;
    if *brace1 || *brace2 || !*required1 || *repeat1 || *required2 || !*repeat2 {
        return None;
    }
    let (stem1, n1) = split_trailing_integer(name1)?;
    let (stem2, n2) = split_trailing_integer(name2)?;
    if stem1.is_empty() || stem1 != stem2 || n2 != n1 + 1 {
        return None;
    }
    Some((stem1.to_string(), true, true, false, true))
}

/// A bracket group that opened on a bare ALL-CAPS word (`extract_positionals`'s
/// own token loop), closed. `words` pairs each raw token with its own
/// cleaned (bracket/dot-trimmed) spelling, in source order. Every word
/// ALL-CAPS and none an option-list placeholder (`mknod`'s `[MAJOR
/// MINOR]`, `gdk-pixbuf-thumbnailer`'s `[INPUT FILE]`) collapses to one
/// multi-word operand; otherwise this is exactly the per-word reading a
/// lone-word ALL-CAPS bracket already gets (`udevadm`'s `[COMMAND
/// OPTIONS]` keeps `COMMAND`, declines `OPTIONS`) — a batch of the same
/// tokens behind one bracket, not a new rule, so this never removes a
/// reading the lone-word branch already gave. See docs/shapes.md S-154.
pub(super) fn finalize_bare_bracket_group(words: &[(String, String)], line: &str) -> Vec<Entity> {
    let is_real_word = |w: &str| w.chars().all(|c| c.is_uppercase() || c == '_') && w.len() > 1;
    if words.len() >= 2
        && words
            .iter()
            .all(|(_, w)| is_real_word(w) && !is_option_list_placeholder(w))
    {
        let name = words
            .iter()
            .map(|(_, w)| w.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let last_raw = &words.last().expect("length checked above").0;
        let mut positional = Entity::positional(name, Provenance::single(Source::HelpText));
        positional.repeatable = token_marks_repetition(last_raw);
        return vec![positional];
    }
    words
        .iter()
        .filter(|(_, w)| is_real_word(w) && !is_option_list_placeholder(w))
        .map(|(raw, word)| {
            let mut positional =
                Entity::positional(word.clone(), Provenance::single(Source::HelpText));
            positional.required = !raw.contains('[') && !line.contains(&format!("[{raw}"));
            positional.repeatable = token_marks_repetition(raw);
            positional
        })
        .collect()
}

/// The primary synopsis's own trailing run of operands — atlas S-041
/// (one operand) generalized to a run of two or more, promoted from
/// `xtask`'s `unparsed-tail-operand` detector (`xtask/src/tail_operand.rs`).
/// The token loop above only ever promotes a `<value>` or an `ALL-CAPS`
/// metavariable; a usage line's own trailing operands (`file`,
/// `[member-name] [count] archive-file file...`) are neither shape and
/// went unrecovered even though the synopsis plainly names them. Fires
/// only when the loop above found nothing, and only for a
/// one-physical-line primary entry ([`primary_synopsis_lines`]) — a
/// wrapped synopsis is a harder shape this does not attempt. Each refusal
/// is documented at its own check, deliberately stricter than the
/// detector it promotes.
pub(super) fn recover_primary_tail_operands(
    usage_lines: &[String],
    primary_lines: &std::collections::HashSet<usize>,
) -> Vec<Entity> {
    let line_idx = match primary_lines.len() {
        1 => match primary_lines.iter().next() {
            Some(&i) => i,
            None => return Vec::new(),
        },
        _ => return Vec::new(),
    };
    let Some(line) = usage_lines.get(line_idx) else {
        return Vec::new();
    };
    let lower = line.to_ascii_lowercase();
    let Some(idx) = lower.find("usage:") else {
        return Vec::new();
    };
    let after = &line[idx + "usage:".len()..];
    let before_desc = cut_before_description_gap(after);
    let mut groups = group_synopsis_tokens(before_desc.trim());
    if groups.len() < 2 {
        return Vec::new();
    }
    groups.remove(0); // the program name itself

    // Walk backward from the tail, collecting a run of operand-shaped
    // groups in reverse source order. A separate ellipsis-only group
    // (lessecho's bare `file ...`) marks the *next* group popped — the
    // operand immediately before it in source order — repeatable, same
    // as a dots suffix glued straight onto a word. See S-101. The fourth
    // field is true only for a brace-alternation operand
    // ([`parse_brace_alternation_group`], S-153's `cache_repair` shape);
    // the fifth is true only once [`collapse_numbered_variadic_tail`] has
    // replaced a numbered pair with its own single repeatable operand
    // (S-136), never set here.
    let mut collected: Vec<(String, bool, bool, bool, bool)> = Vec::new();
    let mut pending_repeat = false;
    while let Some(last) = groups.last() {
        let bare = last.trim_matches(|c| c == '[' || c == ']');
        if !bare.is_empty() && bare.chars().all(|c| c == '.') {
            pending_repeat = true;
            groups.pop();
            continue;
        }
        if let Some((word, required, marker_repeat)) = parse_operand_group(last) {
            collected.push((
                word,
                required,
                marker_repeat || pending_repeat,
                false,
                false,
            ));
            pending_repeat = false;
            groups.pop();
            continue;
        }
        let Some((name, required)) = parse_brace_alternation_group(last) else {
            break;
        };
        collected.push((name, required, pending_repeat, true, false));
        pending_repeat = false;
        groups.pop();
    }
    if collected.is_empty() {
        return Vec::new();
    }
    collected.reverse(); // restore source order

    // A trailing `X1 [X2 ...]` pair (S-136, issue #141) collapses to one
    // repeatable operand named by the shared stem before either guard
    // below runs, since the numbering is itself the evidence that removes
    // both ambiguities: `apt-extracttemplates` has no earlier group at
    // all, and `apt-sortpkgs`'s lone `[options]` is the same shape the
    // "`[options] command`" guard would otherwise decline.
    if collected.len() >= 2 {
        let tail = collected.len() - 2;
        if let Some(collapsed) =
            collapse_numbered_variadic_tail(&collected[tail], &collected[tail + 1])
        {
            collected.truncate(tail);
            collected.push(collapsed);
        }
    }

    // At least one real group must stand between the program name and the
    // run: a lone bracket group right after the program name (`true`'s
    // `Usage: true [ignored command line arguments]`) is prose describing
    // the tool's forgiving argument handling, not a flag list licensing a
    // trailing operand, and [`parse_operand_group`] must not read its
    // first word as one. A numbered-variadic tail is exempt: its own
    // numbering already licenses it with no earlier group at all
    // (`apt-extracttemplates`).
    if groups.is_empty() && !collected.iter().all(|c| c.4) {
        return Vec::new();
    }
    // Every group ahead of the run must read as either an option-list
    // placeholder or a plain flag spelling — `apt-ftparchive`'s lone
    // `[options]`, or `bashbug`/`lessecho`'s real flag lists. A group
    // carrying more than a plain flag spelling — an explicit value word
    // (`-d xy`), a brace-value placeholder (`-b{blocksize}[KMG]`), or a
    // nested alternation (`[-c|-C] cmd`) — is grammar this rule declines
    // to reason about, even when the run itself looks clean, and refuses
    // the whole line rather than guess a boundary.
    let mut earlier_all_placeholder = true;
    for earlier in &groups {
        let earlier_stripped = earlier.trim_matches(|c| c == '[' || c == ']');
        if ends_with_option_list_placeholder(earlier_stripped) {
            continue;
        }
        // `is_understood_flag_context`'s own leniency (an angle-bracket
        // metavar pair, a glued no-whitespace cluster) only ever licenses
        // a *bracketed* group: `btrfs-select-super`'s bare, unbracketed
        // `-s number dev` glues a required value onto `-s` with no
        // brackets anywhere on the line, and a bare flag token cannot be
        // told apart from a genuinely boolean one without that notation —
        // the same ambiguity `-d xy` carries inside a bracket. See
        // `a_bare_unbracketed_flag_never_licenses_the_run_behind_it`.
        if earlier.starts_with('[') && is_understood_flag_context(earlier_stripped) {
            earlier_all_placeholder = false;
            continue;
        }
        return Vec::new();
    }
    // `[options] command`'s shape: a lone placeholder group ahead of a
    // bare, required first operand reads as easily as "provide a
    // subcommand" as "provide an operand" — see the doc comment above.
    // Only the earliest operand in the run sits directly behind the
    // ambiguous context, so only its own required-ness is checked. A
    // brace alternation or a numbered-variadic tail is exempt: neither
    // notation can be mistaken for a bare subcommand name.
    if earlier_all_placeholder && collected[0].1 && !collected[0].3 && !collected[0].4 {
        return Vec::new();
    }
    collected
        .into_iter()
        .map(|(word, required, repeatable, is_brace, _is_numbered)| {
            let mut positional =
                Entity::positional(word.clone(), Provenance::single(Source::HelpText));
            positional.required = required;
            positional.repeatable = repeatable;
            // A brace alternation's own members become the positional's
            // `choices`, when the IR carries choices on a positional (it
            // does — `Entity::choices` is not flag-only). The name keeps
            // its source spelling regardless, per S-097's own rule.
            if is_brace {
                if let Some(members) = brace_alternation_members(&word) {
                    positional.choices = members.into_iter().map(Choice::bare).collect();
                }
            }
            positional
        })
        .collect()
}
