//! The USAGE section's own line shaping: strip the redundant label and
//! program word, and decide when a leading word IS this program.
//! docs/shapes.md S-108, S-150, S-151, S-167, S-252.

use crate::sanitize::defensive_single_line;
use mandible_core::{reconstruct_abbrev_word, Text};
use ratatui::style::Style;
use ratatui::text::{Line, Span};

/// Columns a wrapped usage form's later lines sit right of its first line.
pub(super) const USAGE_HANG: usize = 4;

/// One rendered usage row: `lead`, then `chunk`, with the next `path_left`
/// non-space characters (the form's command path, counted across wrapped
/// rows) in `accent`. `path_left` drops by the characters this row used.
pub(super) fn usage_line(
    lead: &str,
    chunk: &str,
    path_left: &mut usize,
    accent: Style,
) -> Line<'static> {
    if *path_left == 0 {
        return Line::from(format!("{lead}{chunk}"));
    }
    let mut cut = chunk.len();
    for (i, c) in chunk.char_indices() {
        if *path_left == 0 {
            cut = i;
            break;
        }
        if !c.is_whitespace() {
            *path_left -= 1;
        }
    }
    Line::from(vec![
        Span::raw(lead.to_string()),
        Span::styled(chunk[..cut].to_string(), accent),
        Span::raw(chunk[cut..].to_string()),
    ])
}

/// The label-stripped text of one usage line and its indentation column.
fn stripped_form(usage: &str) -> (usize, String) {
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
    (column, text)
}

/// The first word of each form that has a word before the node's own name
/// (`git` for `git log [<options>]` under node `log`): the program a form
/// of this node is written from.
fn leading_programs(node_name: &str, usage: &[Text]) -> Vec<String> {
    let name = defensive_single_line(node_name);
    if name.is_empty() {
        return Vec::new();
    }
    usage
        .iter()
        .filter_map(|u| {
            let (_, text) = stripped_form(u.as_str());
            let (start, _) = usage_naming_span(&text, &name)?;
            let first = text[..start].split_whitespace().next()?;
            (!looks_like_option_or_placeholder(first)).then(|| first.to_string())
        })
        .collect()
}

/// One usage line with its redundant prefix stripped (the `Usage:`/`or:`
/// label and the program word the heading already names), for a node whose
/// forms name `programs` before their own name. A form that opens with one
/// of those programs but names a different command path (`git log`'s
/// `or: git show [<options>]`) is printed as written: the node's name is not
/// prepended to it. Returns the form's column, its text, and the byte length
/// of its leading command path (`git rebase` in `git rebase [-i]`), which is
/// `0` for a form led by a different program.
pub(super) fn usage_form_led_by(
    node_name: &str,
    usage: &str,
    programs: &[String],
) -> (usize, String, usize) {
    let name = defensive_single_line(node_name);
    let (column, text) = stripped_form(usage);
    let (text, path_len) = if name.is_empty() {
        (text, 0)
    } else if let Some((start, end)) = usage_naming_span(&text, &name) {
        // The help text already names the node, by a path (`cp`'s
        // `/usr/bin/cp`) or a prefix (`vim.basic`'s `vim`) rather than by
        // an exact match, so it is replaced with the node's own name.
        let path_len = start + name.len();
        (
            format!("{}{name}{}", &text[..start], &text[end..]),
            path_len,
        )
    } else if let Some((start, end)) = foreign_program_word_span(&text, &name) {
        // The form's own leading word is a program name, just not this
        // node's own spelling or stem (`gcc-ranlib-13`'s own form opens
        // with `/usr/bin/ranlib`), so the same replacement applies.
        let path_len = start + name.len();
        (
            format!("{}{name}{}", &text[..start], &text[end..]),
            path_len,
        )
    } else if text
        .split_whitespace()
        .next()
        .is_some_and(|w| programs.iter().any(|p| p == w))
    {
        (text, 0)
    } else {
        let path_len = name.len();
        (format!("{name} {text}"), path_len)
    };
    (column, text, path_len)
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

/// Every usage line of a node, each shaped by [`usage_form_led_by`] as
/// `(pad, text, path_len)`, with `pad` relative to the first form.
pub(super) fn usage_forms(node_name: &str, usage: &[Text]) -> Vec<(usize, String, usize)> {
    let mut programs = leading_programs(node_name, usage);
    if programs.is_empty() {
        programs.extend(shared_foreign_program(node_name, usage));
    }
    let forms: Vec<(usize, String, usize)> = usage
        .iter()
        .map(|u| usage_form_led_by(node_name, u.as_str(), &programs))
        .collect();
    let shift = forms.first().map(|(column, _, _)| *column).unwrap_or(0);
    forms
        .into_iter()
        .map(|(column, text, path_len)| (column.saturating_sub(shift), text, path_len))
        .collect()
}

/// The program word every form opens with, when no form names the node:
/// `bpftrace` for `tcpretrans.bt`, whose forms all read `bpftrace [options]
/// ...`. It must be a lowercase program-like word that is not the node's
/// own name, so the forms are the program's own and are printed as written.
/// See S-252.
fn shared_foreign_program(node_name: &str, usage: &[Text]) -> Option<String> {
    let name = defensive_single_line(node_name);
    let mut shared: Option<String> = None;
    for u in usage {
        let (_, text) = stripped_form(u.as_str());
        let first = text.split_whitespace().next()?;
        if shared.as_deref().is_some_and(|s| s != first) {
            return None;
        }
        shared = Some(first.to_string());
    }
    let word = shared?;
    (word != name && is_program_word(&word)).then_some(word)
}

/// A bare lowercase-led word of program-name characters.
fn is_program_word(word: &str) -> bool {
    !looks_like_option_or_placeholder(word)
        && word.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+'))
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
        // The node's own word followed by its alias list (`brew search,
        // -S [options]`) still names the node; the comma stays in place.
        let word = token.strip_suffix(',').unwrap_or(token);
        if word_names_node(word, name) {
            return Some((start, start + word.len()));
        }
        cursor = end;
    }
    None
}

/// Whether `word` names the node: an exact match, its basename after the
/// last `/` (`cp`'s `/usr/bin/cp`), the node name's own prefix before its
/// first `.` (`vim.basic`'s own `vim`), or S-167's own bracket-abbreviated
/// spelling read back to the whole word it names (`g[dbserver]` for node
/// `gdbserver`) — without this, the usage line's own leading word never
/// matches the node under `usage_naming_span`, so `usage_form` falls
/// through to prepending a second, redundant `gdbserver` in front of text
/// that already names both the tool and the subcommand.
fn word_names_node(word: &str, name: &str) -> bool {
    let basename = word.rsplit('/').next().unwrap_or(word);
    if basename == name {
        return true;
    }
    if reconstruct_abbrev_word(basename).as_deref() == Some(name) {
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
