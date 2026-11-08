//! A flag row whose description follows its spelling after one space, in
//! lowercase (`-h print this message`), and a row whose spelling ends in the
//! row colon (`-n: dry run`). docs/shapes.md S-262 and S-263.

use super::*;

/// `line` split at the row colon that ends its first token (`-n:`,
/// `--hosts=LIST:`) when text follows it. The colon ends the spelling and
/// is never a value.
pub(super) fn split_row(line: &str) -> Option<(String, String)> {
    if let Some(split) = split_comma_gap_row(line) {
        return Some(split);
    }
    let cut = row_colon_gap(line)?;
    let spec = line.get(..cut)?.trim();
    let desc = line.get(cut..)?.trim();
    let desc = desc.strip_prefix(':').map_or(desc, str::trim_start);
    Some((spec.to_string(), desc.to_string()))
}

/// `-p,     --purge         Remove ...`: the gap after a short's comma
/// separates two spellings, so the description starts at the next gap.
fn split_comma_gap_row(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim_start();
    let first = trimmed.split_whitespace().next()?;
    let short = first.strip_suffix(',')?;
    let rest = trimmed.get(first.len()..)?;
    let after_gap = rest.trim_start();
    let gap = rest.len() - after_gap.len();
    if short.len() != 2 || !short.starts_with('-') || gap < 2 || !after_gap.starts_with("--") {
        return None;
    }
    let long_end = after_gap.find([' ', '\t'])?;
    let long = after_gap.get(..long_end)?;
    let tail = after_gap.get(long_end..)?;
    let desc = tail.trim_start();
    (tail.len() - desc.len() >= 2 && !desc.is_empty() && !long.contains(['=', '[']))
        .then(|| (format!("{short} {long}"), desc.trim_end().to_string()))
}

/// True for a flag row written `--name:  text`, which the flag scan owns.
pub(super) fn is_colon_flag_row(line: &str) -> bool {
    row_colon_gap(line).is_some()
}

fn row_colon_gap(line: &str) -> Option<usize> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('-') {
        return None;
    }
    let start = line.len() - trimmed.len();
    let token = trimmed.split([' ', '\t']).next()?;
    let head = token.strip_suffix(':')?;
    let rest = line.get(start + token.len()..)?;
    (head.len() > 1 && !head.contains(':') && !rest.trim().is_empty()).then_some(start + head.len())
}

/// The words after the spellings and the value of the one row that writes
/// `flag` as `<spellings> <value> <words...>`.
fn words_after_value<'a>(
    flag: &Entity,
    value: &str,
    lines: &[&'a str],
) -> Option<(Vec<&'a str>, bool)> {
    let spellings: Vec<String> = flag.spellings.iter().map(Spelling::render).collect();
    let mut found = lines.iter().filter_map(|line| {
        let mut tokens = line.split_whitespace();
        let mut next = 0;
        while next < spellings.len() {
            let token = tokens.next()?;
            for part in token.split(',').filter(|p| !p.is_empty()) {
                if spellings.get(next).map(String::as_str) != Some(part) {
                    return None;
                }
                next += 1;
            }
        }
        let single_spaced = !line.trim().contains("  ") && !line.contains('\t');
        (tokens.next() == Some(value)).then(|| (tokens.collect::<Vec<_>>(), single_spaced))
    });
    let words = found.next()?;
    found.next().is_none().then_some(words)
}

/// A described row the grammar read with its first description word as the
/// value (`-h print this message`): with no description and two or more
/// words after a plain lowercase value, the words are the description. A
/// value followed by a parenthesised remark keeps the value and takes the
/// remark as its description, and so does a value followed by `--`. Runs after S-176, which reads the value from
/// the usage line where one names it.
pub(super) fn recover_one_space_descriptions(flags: &mut [Entity], lines: &[&str]) {
    for flag in flags.iter_mut() {
        if flag.value_kind != ValueKind::Required
            || !flag.provenance.sources.contains(&Source::HelpText)
            || flag.provenance.sources.contains(&Source::HelpTextSynopsis)
        {
            continue;
        }
        let Some(value) = flag.value_name.clone() else {
            continue;
        };
        let plain = value.len() >= 2 && value.chars().all(|c| c.is_ascii_lowercase());
        let Some((words, single_spaced)) = words_after_value(flag, &value, lines) else {
            continue;
        };
        let remark = words.join(" ");
        let own = flag
            .description
            .as_ref()
            .map_or("", |d| d.as_str())
            .to_string();
        // `-h long -- print more options`: the value, then a `--` separator.
        if let Some(after) = words
            .first()
            .filter(|w| matches!(**w, "--" | "-"))
            .and(words.get(1..))
        {
            if flag.description.is_none() && !after.is_empty() {
                flag.description = non_empty_text(&after.join(" "));
            }
            continue;
        }
        let caps = value.len() >= 2
            && value
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
        let lower_sentence = words
            .first()
            .is_some_and(|w| w.starts_with(|c: char| c.is_ascii_lowercase()));
        if caps && lower_sentence && words.len() >= 2 && own.is_empty() {
            flag.description = non_empty_text(&remark);
            continue;
        }
        if !plain {
            continue;
        }
        // `--state-dir bar Set the state directory`: a capitalised sentence
        // after a lowercase value keeps the value.
        let sentence = words.first().is_some_and(|w| starts_a_sentence(w));
        if sentence
            && words.len() >= 2
            && (own.is_empty() || single_spaced && !own.contains(&remark))
        {
            flag.description = non_empty_text(&format!("{remark} {own}").trim().to_string());
            continue;
        }
        let remark = words.join(" ");
        let own = flag
            .description
            .as_ref()
            .map_or("", |d| d.as_str())
            .to_string();
        let joined = |head: &str| format!("{head} {own}").trim().to_string();
        if remark.starts_with('(') && remark.ends_with(')') && own.is_empty() {
            flag.description = non_empty_text(&remark);
        } else if words.len() >= 2 && (own.is_empty() || single_spaced && !own.contains(&remark)) {
            flag.description = non_empty_text(&joined(&format!("{value} {remark}")));
            flag.value_name = None;
            flag.value_kind = ValueKind::None;
            flag.optional_join = None;
        }
    }
}

/// `-s foo, --src-dir  foo  Set the src dir`: a placeholder written after
/// each spelling of a row is the value once, so the description does not
/// open with its repeat.
pub(super) fn strip_repeated_value(spec: &FlagSpec, spec_text: &str, description: &mut String) {
    let Some(value) = spec.value_name.as_deref() else {
        return;
    };
    if !spec_text.contains(&format!(" {value}, -")) || spec_text.trim_end().ends_with(value) {
        return;
    }
    if let Some(rest) = description
        .strip_prefix(value)
        .and_then(|r| r.strip_prefix(' '))
    {
        *description = rest.trim_start().to_string();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_row_colon_ends_the_first_token() {
        assert_eq!(row_colon_gap("-n: dry run"), Some(2));
        assert_eq!(row_colon_gap("  --hosts=LIST:  Scan"), Some(15));
        assert_eq!(row_colon_gap("-n:"), None);
        assert_eq!(row_colon_gap("note: text"), None);
    }
}
