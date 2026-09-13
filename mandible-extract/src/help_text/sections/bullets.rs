//! lowdown's man-page-like `--help` rendering (nix/Lix, issue #138): every
//! entry, command or option alike, is written as a `·`-led bullet row
//! rather than a plain indented one, and a group label can wrap onto a
//! second physical line. Rewriting both away before the generic layout
//! engine ever sees the text lets every later stage (heading recognition,
//! `scan_bare_block`, `scan_flags_block`) read this shape as the ordinary
//! indented rows it already understands. See docs/shapes.md S-143, S-144.

use super::*;

/// The bullet glyph lowdown emits for a list item: U+00B7 MIDDLE DOT,
/// never U+2022 BULLET.
const BULLET: char = '\u{b7}';

/// Strip a `·` list marker from one physical line, if `line`'s first
/// non-space character is the bullet followed by exactly one space. The
/// row's own text then starts at the same column a continuation line of
/// the same entry already sits at, so [`bare_block_end`]/[`split_entries`]
/// read the un-marked block exactly the way they read any other
/// multi-line bare-word entry — no new continuation-folding rule needed.
/// Leaves every other line untouched, byte for byte.
fn strip_bullet_marker(line: &str) -> Option<(&str, &str)> {
    let indent_len = line.len() - line.trim_start().len();
    let indent = &line[..indent_len];
    let rest = &line[indent_len..];
    let mut chars = rest.chars();
    if chars.next() != Some(BULLET) {
        return None;
    }
    let after_bullet = chars.as_str();
    let text = after_bullet.strip_prefix(' ')?;
    Some((indent, text))
}

/// S-143's own gate: `text` (a bullet row's own text, marker already
/// stripped) opens with the tool's own name, one more
/// [`is_command_name_shaped`] word, and a ` - ` separator. `tool_name` is
/// already known to the parser for every probe (the same parameter
/// [`starts_with_tool_name`] reads); reading it back out of the row is not
/// per-tool logic, since any tool naming itself this way is read the same
/// way. Returns the text with the `"<name> "` prefix removed, so what's
/// left (`"help - show help..."`) is the ordinary `name - description`
/// row [`split_entries`] already knows how to fold.
fn strip_command_row_tool_prefix<'a>(text: &'a str, tool_name: &str) -> &'a str {
    let Some(rest) = text.strip_prefix(tool_name) else {
        return text;
    };
    let Some(rest) = rest.strip_prefix(' ') else {
        return text;
    };
    let Some((word, tail)) = rest.split_once(' ') else {
        return text;
    };
    if !is_command_name_shaped(word) || !tail.trim_start().starts_with("- ") {
        return text;
    }
    rest
}

/// True when `label` (already checked colon-terminated and heading-shaped
/// on its own) independently names a command section — the extra guard
/// [`join_wrapped_heading_line`] requires before it will ever join two
/// physical lines, so the rule can only ever fire on the one shape it was
/// built for.
fn names_a_command_section(label: &str) -> bool {
    label
        .split(|c: char| !c.is_alphanumeric())
        .map(|w| w.to_lowercase())
        .any(|w| matches!(w.as_str(), "command" | "commands"))
}

/// True when `word` is plain alphabetic prose with no notation — the
/// shape a heading's own wrapped first physical line must have for
/// [`join_wrapped_heading_line`] to consider it at all.
fn is_plain_word(word: &str) -> bool {
    !word.is_empty() && word.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Joins a heading wrapped across two physical lines at one indent, so
/// later heading tests read one label. Both lines must hold plain words
/// only, and the joined label must itself name a command section.
/// corpus/nix/2.95.2-lix. docs/shapes.md S-143.
fn join_wrapped_heading_line<'a>(first: &'a str, second: &'a str) -> Option<String> {
    let first_indent = first.len() - first.trim_start().len();
    let first_trim = first.trim();
    if first_trim.is_empty() || first_trim.ends_with(':') {
        return None;
    }
    if !first_trim.split_whitespace().all(is_plain_word) {
        return None;
    }
    let second_indent = second.len() - second.trim_start().len();
    if second_indent != first_indent {
        return None;
    }
    let second_trim = second.trim();
    let label = second_trim.strip_suffix(':')?;
    if label.is_empty() || !label.split_whitespace().all(is_plain_word) {
        return None;
    }
    let joined = format!("{first_trim} {second_trim}");
    names_a_command_section(&joined).then_some(joined)
}

/// A bare word using nothing but ASCII lowercase letters — S-144's own
/// shape for a value name written in plain prose rather than notation
/// (`--option name value Set...`'s `name` and `value`).
fn is_lowercase_bare_word(word: &str) -> bool {
    !word.is_empty() && word.chars().all(|c| c.is_ascii_lowercase())
}

/// S-144's alias half, and S-161's own repair (docs/shapes.md): a
/// `/`-joined second spelling with a space on each side
/// (`"--print-build-logs / -L Print..."`, `cargo-clippy`'s `-W / --warn
/// [LINT]`) is rewritten to the ordinary comma-joined alias (`", -L"`)
/// [`super::grammar::parse_flag_spec`] already reads, so the row's own
/// alias survives instead of being read as an unparsed leftover. Fires
/// only when the token right after the spelling's own run is exactly `/`
/// with one space on each side and what follows is itself flag-shaped, so
/// an ordinary description merely containing a slash (a path,
/// "input/output") is never touched. Ungated on the lowdown bullet marker
/// (S-161 lifted it out of that gate): the same narrow evidence is safe on
/// any option-table row, called from [`super::emit::emit_flags_with`] too.
pub(super) fn join_slash_alias(text: &str) -> String {
    let Some(spec_end) = text.find(' ') else {
        return text.to_string();
    };
    let (spec, rest) = text.split_at(spec_end);
    if !spec.starts_with('-') {
        return text.to_string();
    }
    let Some(after_sep) = rest.strip_prefix(" / ") else {
        return text.to_string();
    };
    if !after_sep.starts_with('-') {
        return text.to_string();
    }
    format!("{spec}, {after_sep}")
}

/// S-144's value half: a run of one or more [`is_lowercase_bare_word`]
/// tokens sitting directly between an option row's own spelling/alias
/// run and its description's sentence start (`--option name value
/// Set...`) is wrapped in `<...>`, the same angle-bracket value notation
/// [`super::entry::find_placeholder_boundary_gap`] and
/// [`super::grammar::parse_flag_spec`] already read (`--manifest-path
/// <manifest-path>`), so the run reaches the tree as one value spec
/// instead of the whole line falling through ungapped. A description
/// starting immediately after the spelling/alias run (`--debug Set...`,
/// zero value names) is left untouched.
fn wrap_option_row_value_names(text: &str) -> String {
    let bytes = text.as_bytes();
    // Walk the spelling/alias run: whitespace- or comma-separated tokens
    // that all start with `-`. `i` ends up at the first token past it.
    let mut i = 0usize;
    loop {
        while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b',') {
            i += 1;
        }
        let word_start = i;
        while i < bytes.len() && bytes[i] != b' ' {
            i += 1;
        }
        if word_start == i {
            break;
        }
        if !text[word_start..i].starts_with('-') {
            i = word_start;
            break;
        }
    }
    // Collect the run of lowercase bare words starting at `i`.
    let mut run_start: Option<usize> = None;
    let mut run_end = i;
    let mut j = i;
    loop {
        while j < bytes.len() && bytes[j] == b' ' {
            j += 1;
        }
        let word_start = j;
        while j < bytes.len() && bytes[j] != b' ' {
            j += 1;
        }
        if word_start == j {
            break;
        }
        if is_lowercase_bare_word(&text[word_start..j]) {
            run_start.get_or_insert(word_start);
            run_end = j;
        } else {
            break;
        }
    }
    let Some(start) = run_start else {
        return text.to_string();
    };
    format!(
        "{}<{}>{}",
        &text[..start],
        &text[start..run_end],
        &text[run_end..]
    )
}

/// Both S-144 repairs, run only on a row this module already knows came
/// from a lowdown `·` marker and reads as an option row (starts with
/// `-`) — never on an ordinary flag row from any other tool. See
/// docs/shapes.md S-144.
fn repair_option_bullet_row(text: &str) -> String {
    if !text.starts_with('-') {
        return text.to_string();
    }
    wrap_option_row_value_names(&join_slash_alias(text))
}

/// Rewrite every `·`-marked bullet row in `raw` into the plain indented
/// row the generic layout engine already understands, and join a heading
/// that lowdown wrapped across two physical lines. Run once, before any
/// other layout analysis — mirrors [`super::heading::split_shared_heading_rows`]'s
/// own shape: a raw-text rewrite feeding the same unmodified engine, never
/// a second code path. See docs/shapes.md S-143, S-144.
pub(super) fn rewrite_lowdown_bullets(raw: &str, tool_name: Option<&str>) -> String {
    let lines: Vec<&str> = raw.lines().collect();
    let mut out = String::with_capacity(raw.len());
    let mut i = 0;
    while i < lines.len() {
        if i + 1 < lines.len() {
            if let Some(joined) = join_wrapped_heading_line(lines[i], lines[i + 1]) {
                let indent_len = lines[i].len() - lines[i].trim_start().len();
                out.push_str(&lines[i][..indent_len]);
                out.push_str(&joined);
                out.push('\n');
                i += 2;
                continue;
            }
        }
        match strip_bullet_marker(lines[i]) {
            Some((indent, text)) => {
                let text = match tool_name {
                    Some(name) => strip_command_row_tool_prefix(text, name),
                    None => text,
                };
                let text = repair_option_bullet_row(text);
                out.push_str(indent);
                out.push_str(&text);
            }
            None => out.push_str(lines[i]),
        }
        out.push('\n');
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_a_plain_option_bullet_marker() {
        let raw = "  \u{b7} --debug Set the logging verbosity level to 'debug'.\n";
        let rewritten = rewrite_lowdown_bullets(raw, Some("nix"));
        assert_eq!(
            rewritten,
            "  --debug Set the logging verbosity level to 'debug'.\n"
        );
    }

    #[test]
    fn joins_a_slash_separated_alias_into_a_comma() {
        let raw =
            "  \u{b7} --print-build-logs / -L Print full build logs on\n    standard error.\n";
        let rewritten = rewrite_lowdown_bullets(raw, Some("nix"));
        assert!(
            rewritten.contains("--print-build-logs, -L Print full build logs on"),
            "{rewritten:?}"
        );
    }

    #[test]
    fn wraps_a_run_of_lowercase_value_names_before_the_description() {
        let raw =
            "  \u{b7} --option name value Set the Lix configuration\n    setting name to value (overriding nix.conf).\n";
        let rewritten = rewrite_lowdown_bullets(raw, Some("nix"));
        assert!(
            rewritten.contains("--option <name value> Set the Lix configuration"),
            "{rewritten:?}"
        );
    }

    #[test]
    fn never_wraps_when_the_description_starts_immediately() {
        let text = "--debug Set the logging verbosity level to 'debug'.";
        assert_eq!(wrap_option_row_value_names(text), text);
    }

    #[test]
    fn never_joins_a_slash_inside_an_ordinary_description() {
        let text = "--path input / output description";
        assert_eq!(join_slash_alias(text), text);
    }

    #[test]
    fn strips_the_tool_name_prefix_from_a_command_bullet() {
        let raw = "  \u{b7} nix help - show help about nix or a particular\n    subcommand \n";
        let rewritten = rewrite_lowdown_bullets(raw, Some("nix"));
        assert_eq!(
            rewritten,
            "  help - show help about nix or a particular\n    subcommand \n"
        );
    }

    #[test]
    fn leaves_a_non_command_bullet_alone() {
        let raw = "  \u{b7} Store path \n";
        let rewritten = rewrite_lowdown_bullets(raw, Some("nix"));
        assert_eq!(rewritten, "  Store path \n");
    }

    #[test]
    fn leaves_an_unrelated_bullet_char_inside_prose_alone() {
        // Not a list marker: no leading space run before the dot on its
        // own line, so this must never be mistaken for a row marker.
        let raw = "price is 3\u{b7}50 per unit\n";
        let rewritten = rewrite_lowdown_bullets(raw, Some("nix"));
        assert_eq!(rewritten, raw);
    }

    #[test]
    fn joins_a_heading_wrapped_across_two_lines() {
        let raw = "    Commands for upgrading or troubleshooting your Nix\n    installation:\n\n      \u{b7} nix doctor - check\n";
        let rewritten = rewrite_lowdown_bullets(raw, Some("nix"));
        assert!(rewritten
            .contains("Commands for upgrading or troubleshooting your Nix installation:\n"));
    }

    #[test]
    fn never_joins_an_ordinary_two_line_prose_paragraph() {
        let raw = "    See the manual page for further\n    details:\n\n      more text\n";
        let rewritten = rewrite_lowdown_bullets(raw, Some("nix"));
        assert_eq!(rewritten, raw);
    }

    #[test]
    fn never_joins_when_the_first_line_already_carries_notation() {
        let raw = "    nix [option...] subcommand\n    installation:\n";
        let rewritten = rewrite_lowdown_bullets(raw, Some("nix"));
        assert_eq!(rewritten, raw);
    }
}
