//! One synopsis group read as an operand. Split out of `usage.rs` to keep
//! that file under its own size ceiling (AGENTS.md §2).

use super::*;

/// One synopsis group, its own outer brackets (if any) still attached, as
/// an operand: `(name, required, repeatable)`. `None` when the group is
/// not operand-shaped — dash-led, an option-list placeholder, an
/// `ALL-CAPS` metavariable (the token loop above's own job, S-041's
/// "out of scope" test), or carrying more than a dots-only suffix after
/// its first word (`true`'s `[ignored command line arguments]` must not
/// read `ignored` as an operand). See S-041 and
/// [`recover_primary_tail_operands`].
pub(super) fn parse_operand_group(group: &str) -> Option<(String, bool, bool)> {
    let stripped = group.trim_matches(|c| c == '[' || c == ']');
    let mut words = stripped.split_whitespace();
    let raw_word = words.next()?;
    // A dots-glued word (`file...`) marks repetition itself. See S-101.
    let marker_repeat = token_marks_repetition(raw_word);
    let word = raw_word.trim_end_matches('.');
    if word.is_empty() || word.starts_with('-') || is_option_list_placeholder(word) {
        return None;
    }
    let mut chars = word.chars();
    let first = chars.next()?;
    if !first.is_ascii_lowercase() {
        return None;
    }
    // `in-file(s)` keeps its `(s)` as written.
    let body = word.strip_suffix("(s)").unwrap_or(word);
    let mut chars = body.chars();
    chars.next();
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return None;
    }
    let mut inline_repeat = false;
    for w in words {
        if w.chars().all(|c| c == '.') {
            inline_repeat = true;
        } else {
            return None;
        }
    }
    let required = !group.starts_with('[');
    Some((word.to_string(), required, marker_repeat || inline_repeat))
}

/// `[[--] word word...]`: an optional end-of-options marker followed by
/// operand words, as `(name, repeatable)` in source order. `None` for any
/// other group. See docs/shapes.md S-153 and S-096.
pub(super) fn parse_separator_operand_group(group: &str) -> Option<Vec<(String, bool)>> {
    let inner = group.strip_prefix("[[--]")?.strip_suffix(']')?;
    let mut out = Vec::new();
    for w in inner.split_whitespace() {
        let (name, _, repeat) = parse_operand_group(w)?;
        out.push((name, repeat));
    }
    (!out.is_empty()).then_some(out)
}

/// Whether an ALL-CAPS name with a hyphen (`INITIAL-ARGS`) is not an operand
/// on `line`: it ends in an option-list word (`SHORT-OPTION` stands for the
/// tool's options), the line holds no synopsis group (`[`, `<` or `{`), or
/// it sits in running prose of three or more lowercase words after the
/// program's own words (`<address> is a passive address like SSL-L`).
pub(super) fn hyphenated_caps_refused(name: &str, line: &str) -> bool {
    let last = name.rsplit('-').next().unwrap_or(name);
    if is_option_list_placeholder(last) || !line.contains(['[', '<', '{']) {
        return true;
    }
    let mut run = 0;
    let mut program_words = true;
    for word in line.split_whitespace() {
        let lower = word.chars().all(|c| c.is_ascii_lowercase());
        program_words &= lower || word.ends_with(':') || word.contains(['/', '.', '_', '-']);
        if lower && !program_words {
            run += 1;
            if run >= 3 {
                return true;
            }
        } else {
            run = 0;
        }
    }
    false
}
