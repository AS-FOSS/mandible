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
    // See the sibling comment in `recover_primary_tail_operands`: a bare
    // usage label (S-150) may already be gone by the time this line
    // reaches `usage_lines`, so a missing `usage:` falls back to the
    // whole line rather than refusing.
    let lower = line.to_ascii_lowercase();
    let after: &str = match lower.find("usage:") {
        Some(idx) => &line[idx + "usage:".len()..],
        None => line.as_str(),
    };
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
