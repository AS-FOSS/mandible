//! One synopsis group read as an operand. Split out of `usage.rs` to keep
//! that file under its own size ceiling (AGENTS.md §2).

use super::*;

/// Whether the stem of `name` (`COMMAND` of `COMMAND-OPTION`) is a word of
/// `line` ahead of the first word that holds `name`. See docs/shapes.md S-255.
fn extends_earlier_operand(name: &str, line: &str) -> bool {
    let Some((stem, _)) = name.rsplit_once('-') else {
        return false;
    };
    let clean = |w: &str| {
        w.trim_matches(['[', ']', '<', '>', '.', '|', '{', '}'])
            .to_string()
    };
    line.split_whitespace()
        .take_while(|w| !w.contains(name))
        .any(|w| clean(w) == stem)
}

/// An ALL-CAPS operand with a repeat group glued to it, `MODE[,MODE]...`,
/// as the name it keeps (`MODE[,MODE]`; the dots make it repeatable). The
/// group must repeat the word after a separator. See docs/shapes.md S-255.
pub(super) fn glued_repeat_operand(token: &str) -> Option<String> {
    let body = token.trim_start_matches('[');
    let opened = token.len() - body.len();
    let body = body.trim_end_matches('.');
    if !token_marks_repetition(token) {
        return None;
    }
    let (word, group) = body.split_once('[')?;
    let caps = |w: &str| w.len() > 1 && w.chars().all(|c| c.is_ascii_uppercase() || c == '_');
    let (sep, rest) = group.split_once(word)?;
    let closers = rest.strip_prefix(']')?;
    let tidy = closers.len() <= opened && closers.chars().all(|c| c == ']');
    let sep_ok = !sep.is_empty()
        && sep
            .chars()
            .all(|c| !c.is_alphanumeric() && !"[]<>".contains(c) && !c.is_whitespace());
    (caps(word) && tidy && sep_ok).then(|| format!("{word}[{sep}{word}]"))
}

/// The operand that ends every usage form moves behind the operands only a
/// later form adds (`chmod`'s `FILE...` after `MODE` and `OCTAL-MODE`), so
/// the list reads in usage order. See docs/shapes.md S-255.
pub(super) fn move_shared_tail_last(out: &mut Vec<Entity>, forms: &[Vec<String>]) {
    let mut ending = forms.iter().filter_map(|f| f.last());
    let Some(tail) = ending.next() else {
        return;
    };
    let shared = forms.iter().filter(|f| !f.is_empty()).count() >= 2 && ending.all(|l| l == tail);
    let Some(at) = out.iter().position(|e| e.primary_name() == tail) else {
        return;
    };
    let all_named = out
        .iter()
        .all(|e| forms.iter().flatten().any(|n| n == e.primary_name()));
    if shared && all_named && at + 1 < out.len() {
        let moved = out.remove(at);
        out.push(moved);
    }
}

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
/// tool's options; `COMMAND-OPTION` after `COMMAND` extends that operand
/// and stays one), the line holds no synopsis group (`[`, `<` or `{`), or
/// it sits in running prose of three or more lowercase words after the
/// program's own words (`<address> is a passive address like SSL-L`).
pub(super) fn hyphenated_caps_refused(name: &str, line: &str) -> bool {
    let last = name.rsplit('-').next().unwrap_or(name);
    if (is_option_list_placeholder(last) && !extends_earlier_operand(name, line))
        || !line.contains(['[', '<', '{'])
    {
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
