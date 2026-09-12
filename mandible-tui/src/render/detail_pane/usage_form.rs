//! The USAGE section's own line shaping: strip the redundant label and
//! program word, and decide when a leading word IS this program.
//! docs/shapes.md S-108, S-150, S-151.

use crate::sanitize::defensive_single_line;
use mandible_core::Text;

/// One usage line with its redundant prefix stripped: the `Usage:` or
/// `or:` label, and the program word the heading already names. Returns
/// the indentation column the author gave the line. See S-108, S-150.
pub(super) fn usage_form(node_name: &str, usage: &str) -> (usize, String) {
    let name = defensive_single_line(node_name);
    let raw = defensive_single_line(usage);

    // Tabs are already expanded to spaces at 8-column stops
    // (`Text::sanitize_preserving_layout`), so counting leading spaces
    // counts columns.
    let mut text = raw.trim_start_matches(' ').to_string();
    let mut column = raw.chars().count() - text.chars().count();

    // Drop a leading `usage:` label, or else a continuation's own `or:`/
    // `or` marker, case-insensitively; charge its width to the column so
    // the forms keep the alignment the tool author drew them with.
    if text
        .get(..6)
        .is_some_and(|p| p.eq_ignore_ascii_case("usage:"))
    {
        let after = text[6..].trim_start().to_string();
        column += text.chars().count() - after.chars().count();
        text = after;
    } else {
        let marker_len = or_marker_len(&text);
        if marker_len > 0 {
            let after = text[marker_len..].trim_start().to_string();
            column += text.chars().count() - after.chars().count();
            text = after;
        }
    }

    let text = if name.is_empty() {
        text
    } else if let Some((start, end)) = usage_naming_span(&text, &name) {
        // The help text already names the node, by a path (`cp`'s
        // `/usr/bin/cp`) or a prefix (`vim.basic`'s `vim`) rather than by
        // an exact match, so it is replaced with the node's own name.
        format!("{}{name}{}", &text[..start], &text[end..])
    } else if let Some((start, end)) = foreign_program_word_span(&text, &name) {
        // The form's own leading word is a program name, just not this
        // node's own spelling or stem (`gcc-ranlib-13`'s own form opens
        // with `/usr/bin/ranlib`), so the same replacement applies.
        format!("{}{name}{}", &text[..start], &text[end..])
    } else {
        format!("{name} {text}")
    };
    (column, text)
}

/// The span of a leading program word that is not this node's own name or
/// stem (`gcc-ranlib-13`'s form opens `/usr/bin/ranlib`). A path always
/// qualifies; a bare word only when it prefixes the node's name, so a
/// sibling (`egrep` under node `grep`) is never replaced. See S-151.
pub(super) fn foreign_program_word_span(text: &str, name: &str) -> Option<(usize, usize)> {
    let first = text.split_whitespace().next()?;
    if looks_like_option_or_placeholder(first) {
        return None;
    }
    // A bare word that CONTAINS the node's own name is a sibling program,
    // not this one under another spelling (`egrep` under node `grep`), and
    // replacing it would delete a real word. A path always qualifies.
    if !first.contains('/') && !name.starts_with(first) {
        return None;
    }
    // "Lowercase-led": the token's own first *alphabetic* character is
    // lowercase, checked past any leading path separators so an absolute
    // path (`/usr/bin/ranlib`) still qualifies.
    let first_alpha = first.chars().find(|c| c.is_alphabetic())?;
    if !first_alpha.is_ascii_lowercase() {
        return None;
    }
    if !first
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+' | '/'))
    {
        return None;
    }
    let start = text.find(first)?;
    Some((start, start + first.len()))
}

/// The byte length of a leading `or:`/`or ` continuation marker, or `0`.
/// Reads through `str::get`, never a raw index, so a multi-byte character
/// straddling the offset degrades to "no marker" instead of panicking.
pub(super) fn or_marker_len(text: &str) -> usize {
    if text.get(..3).is_some_and(|p| p.eq_ignore_ascii_case("or:")) {
        return 3;
    }
    if text.as_bytes().get(2) == Some(&b' ')
        && text.get(..2).is_some_and(|p| p.eq_ignore_ascii_case("or"))
    {
        return 2;
    }
    0
}

/// Every usage line of a node, each shaped by [`usage_form`].
pub(super) fn usage_forms(node_name: &str, usage: &[Text]) -> Vec<(usize, String)> {
    let forms: Vec<(usize, String)> = usage
        .iter()
        .map(|u| usage_form(node_name, u.as_str()))
        .collect();
    let shift = forms.first().map(|(column, _)| *column).unwrap_or(0);
    forms
        .into_iter()
        .map(|(column, text)| (column.saturating_sub(shift), text))
        .collect()
}

/// The span of a leading word that IS this node under another spelling: a
/// resolved path (`/usr/bin/cp` for `cp`) or a dotted stem (`vim` for
/// `vim.basic`). Replaced with the node's own name. See S-108.
pub(super) fn usage_naming_span(text: &str, name: &str) -> Option<(usize, usize)> {
    let mut cursor = 0;
    for token in text.split_whitespace() {
        let start = cursor + text[cursor..].find(token)?;
        let end = start + token.len();
        if looks_like_option_or_placeholder(token) {
            return None;
        }
        if word_names_node(token, name) {
            return Some((start, end));
        }
        cursor = end;
    }
    None
}

/// Whether `word` names the node: an exact match, its basename after the
/// last `/` (`cp`'s `/usr/bin/cp`), or the node name's own prefix before
/// its first `.` (`vim.basic`'s own `vim`).
fn word_names_node(word: &str, name: &str) -> bool {
    let basename = word.rsplit('/').next().unwrap_or(word);
    if basename == name {
        return true;
    }
    match name.split_once('.') {
        Some((prefix, _)) if !prefix.is_empty() => basename == prefix,
        _ => false,
    }
}

/// True for an option (`-v`), a bracketed or angled placeholder, or a bare
/// ALL-CAPS metavar: any of them ends a leading command-path run.
pub(super) fn looks_like_option_or_placeholder(word: &str) -> bool {
    if word.starts_with(['-', '[', '<']) {
        return true;
    }
    let has_letter = word.chars().any(|c| c.is_alphabetic());
    has_letter && !word.chars().any(|c| c.is_lowercase())
}
