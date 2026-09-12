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
