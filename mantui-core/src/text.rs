//! [`Text`]: the single point through which untrusted, tool-produced strings
//! enter mantui's intermediate representation.
//!
//! See spec §4.1. Every string mantui did not author itself — help output,
//! man page prose, completion script comments, catalog descriptions — must be
//! wrapped in [`Text::sanitize`] before it can reach a widget. The type is
//! deliberately awkward to construct any other way: its field is private and
//! there is no `From<String>` or `From<&str>` impl.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Hard cap on sanitized text length, in `char`s. Applied after all other
/// normalization. Generous enough for any legitimate flag description or
/// man page section, small enough that a pathological multi-megabyte string
/// from a misbehaving tool cannot make its way into a render buffer.
pub const MAX_TEXT_CHARS: usize = 8192;

/// Sanitized, display-safe text.
///
/// Constructing a `Text` always goes through [`Text::sanitize`] (directly, or
/// indirectly via `Deserialize`), which strips control characters and
/// terminal escape sequences, resolves backspace-overstrike, expands tabs,
/// collapses whitespace runs, normalizes newlines (preserving paragraph
/// breaks), and truncates to [`MAX_TEXT_CHARS`]. Widgets and other consumers
/// may assume a `Text` is safe to place directly into a rendering surface.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Text(String);

impl Text {
    /// The only way to build a `Text` from raw, untrusted input.
    ///
    /// Pipeline (see spec §4.1 and §13.3 for the adversarial cases this
    /// must survive):
    /// 1. Strip ANSI/OSC/other terminal escape sequences.
    /// 2. Resolve backspace-overstrike (`_\bX`, `X\bX`, and any stray `\b`).
    /// 3. Strip remaining C0 control characters and DEL.
    /// 4. Expand tabs to spaces at 8-column stops.
    /// 5. Normalize line endings to `\n`; collapse 3+ consecutive newlines
    ///    down to exactly 2 (a paragraph break).
    /// 6. Collapse runs of horizontal whitespace to a single space.
    /// 7. Trim leading/trailing whitespace.
    /// 8. Truncate to [`MAX_TEXT_CHARS`] characters, at a char boundary.
    pub fn sanitize(raw: &str) -> Text {
        let no_escapes = strip_escapes(raw);
        let overstruck = resolve_backspace(&no_escapes);
        let no_control = strip_c0(&overstruck);
        let tabs_expanded = expand_tabs(&no_control, 8);
        let newlines_normalized = normalize_newlines(&tabs_expanded);
        let collapsed = collapse_horizontal_whitespace(&newlines_normalized);
        let trimmed = trim_lines_and_whole(&collapsed);
        let truncated = truncate_chars(&trimmed, MAX_TEXT_CHARS);
        Text(truncated)
    }

    /// Borrow the sanitized string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// True if the sanitized text is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Collapse to a single display line (paragraph breaks and internal
    /// newlines become a single space), for contexts like tree rows that
    /// have no room for multi-line text. The tree pane is expected to call
    /// this at render time rather than store a second copy of the text.
    pub fn single_line(&self) -> String {
        let mut out = String::with_capacity(self.0.len());
        let mut last_was_space = false;
        for ch in self.0.chars() {
            let c = if ch == '\n' { ' ' } else { ch };
            if c == ' ' {
                if !last_was_space && !out.is_empty() {
                    out.push(' ');
                }
                last_was_space = true;
            } else {
                out.push(c);
                last_was_space = false;
            }
        }
        out.trim_end().to_string()
    }
}

impl fmt::Display for Text {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for Text {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Text {
    /// Deserialization re-runs [`Text::sanitize`] rather than trusting the
    /// stored bytes verbatim. This keeps the invariant airtight even when a
    /// `Text` is round-tripped through the on-disk cache (spec §11): a
    /// tampered or corrupted cache file cannot smuggle unsanitized bytes
    /// back into the IR. `sanitize` is idempotent, so this costs nothing
    /// extra for cache entries that were already clean.
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Ok(Text::sanitize(&raw))
    }
}

/// Strip ANSI CSI/OSC/DCS escape sequences and other `ESC`-prefixed
/// sequences. Hand-written state machine rather than a regex crate
/// dependency; the grammar is small and well-known.
fn strip_escapes(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some('[') => {
                // CSI: ESC [ ... final-byte in 0x40..=0x7E
                chars.next();
                for c2 in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c2) {
                        break;
                    }
                }
            }
            Some(']') => {
                // OSC: ESC ] ... BEL or ESC \
                chars.next();
                loop {
                    match chars.next() {
                        None => break,
                        Some('\u{07}') => break,
                        Some('\u{1b}') => {
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                            }
                            break;
                        }
                        Some(_) => continue,
                    }
                }
            }
            Some('P') | Some('X') | Some('^') | Some('_') => {
                // DCS / SOS / PM / APC: ESC x ... ESC \
                chars.next();
                loop {
                    match chars.next() {
                        None => break,
                        Some('\u{1b}') => {
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                            }
                            break;
                        }
                        Some(_) => continue,
                    }
                }
            }
            Some(_) => {
                // Two-character escape (e.g. charset selection ESC ( B).
                chars.next();
            }
            None => {}
        }
    }
    out
}

/// Resolve backspace-overstrike sequences as emitted by rendered man pages
/// (`_\bX` for underline, `X\bX` for bold). A backspace deletes the
/// previously emitted character; whatever follows becomes the visible glyph.
/// This also silently absorbs any stray backspace with nothing to delete.
fn resolve_backspace(input: &str) -> String {
    let mut out: Vec<char> = Vec::with_capacity(input.len());
    for c in input.chars() {
        if c == '\u{8}' {
            out.pop();
        } else {
            out.push(c);
        }
    }
    out.into_iter().collect()
}

/// Strip remaining C0 control characters and DEL, preserving `\t`, `\n`,
/// `\r` for the later tab/newline passes.
fn strip_c0(input: &str) -> String {
    input
        .chars()
        .filter(|&c| {
            let is_c0 = ('\u{0}'..='\u{1f}').contains(&c);
            let keep = c == '\t' || c == '\n' || c == '\r';
            !(is_c0 && !keep) && c != '\u{7f}'
        })
        .collect()
}

/// Expand tabs to spaces at fixed-width stops, tracking column position
/// relative to the last newline.
fn expand_tabs(input: &str, stop: usize) -> String {
    let mut out = String::with_capacity(input.len());
    let mut col = 0usize;
    for c in input.chars() {
        match c {
            '\t' => {
                let spaces = stop - (col % stop);
                for _ in 0..spaces {
                    out.push(' ');
                }
                col += spaces;
            }
            '\n' => {
                out.push('\n');
                col = 0;
            }
            _ => {
                out.push(c);
                col += 1;
            }
        }
    }
    out
}

/// Normalize `\r\n` and lone `\r` to `\n`.
fn normalize_newlines(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            out.push('\n');
        } else {
            out.push(c);
        }
    }
    out
}

/// Collapse runs of horizontal whitespace (spaces, after tab expansion) to a
/// single space, and collapse runs of 3+ newlines down to exactly 2 so a
/// `\n\n` paragraph break survives while pathological vertical whitespace
/// does not.
fn collapse_horizontal_whitespace(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut space_run = false;
    let mut newline_run = 0usize;
    for c in input.chars() {
        match c {
            ' ' => {
                space_run = true;
                newline_run = 0;
            }
            '\n' => {
                if space_run {
                    // Trailing spaces before a newline are dropped, not kept.
                    space_run = false;
                }
                newline_run += 1;
                if newline_run <= 2 {
                    out.push('\n');
                }
            }
            _ => {
                if space_run {
                    out.push(' ');
                    space_run = false;
                }
                newline_run = 0;
                out.push(c);
            }
        }
    }
    if space_run {
        out.push(' ');
    }
    out
}

/// Trim leading/trailing whitespace on each line and on the whole text.
fn trim_lines_and_whole(input: &str) -> String {
    let lines: Vec<&str> = input.lines().map(|l| l.trim_end_matches(' ')).collect();
    lines.join("\n").trim().to_string()
}

/// Truncate to at most `max_chars` characters, respecting char boundaries.
fn truncate_chars(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        return input.to_string();
    }
    input.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_c0_controls() {
        let t = Text::sanitize("hello\x01\x02world");
        assert_eq!(t.as_str(), "helloworld");
    }

    #[test]
    fn strips_ansi_csi() {
        let t = Text::sanitize("\x1b[31mred\x1b[0m text");
        assert_eq!(t.as_str(), "red text");
    }

    #[test]
    fn strips_osc_sequence() {
        let t = Text::sanitize("\x1b]0;window title\x07visible");
        assert_eq!(t.as_str(), "visible");
    }

    #[test]
    fn strips_osc_sequence_st_terminated() {
        let t = Text::sanitize("\x1b]8;;http://example.com\x1b\\link\x1b]8;;\x1b\\");
        assert_eq!(t.as_str(), "link");
    }

    #[test]
    fn resolves_underline_overstrike() {
        // "_\bH_\be_\bl_\bl_\bo" -> "Hello"
        let raw = "_\u{8}H_\u{8}e_\u{8}l_\u{8}l_\u{8}o";
        let t = Text::sanitize(raw);
        assert_eq!(t.as_str(), "Hello");
    }

    #[test]
    fn resolves_bold_overstrike() {
        let raw = "H\u{8}He\u{8}el\u{8}ll\u{8}lo\u{8}o";
        let t = Text::sanitize(raw);
        assert_eq!(t.as_str(), "Hello");
    }

    #[test]
    fn stray_backspace_is_absorbed() {
        let t = Text::sanitize("\u{8}\u{8}\u{8}hello");
        assert_eq!(t.as_str(), "hello");
    }

    #[test]
    fn tab_becomes_whitespace_then_collapses_like_any_other_run() {
        // Tabs are expanded to column-aligned spaces, but the subsequent
        // whitespace-collapse pass (spec §4.1) then reduces that run to a
        // single space, same as any other run of horizontal whitespace.
        // `Text` renders prose, not columnar layout, so this is correct:
        // preserving tab-stop alignment would only matter for structural
        // (pre-sanitization) parsing of raw tool output, which happens
        // upstream of `Text::sanitize`, not on already-segmented fields.
        let t = Text::sanitize("a\tb");
        assert_eq!(t.as_str(), "a b");
    }

    #[test]
    fn tabs_do_not_leak_through_as_raw_characters() {
        let t = Text::sanitize("col1\tcol2\tcol3");
        assert!(!t.as_str().contains('\t'));
    }

    #[test]
    fn collapses_whitespace_runs() {
        let t = Text::sanitize("a     b");
        assert_eq!(t.as_str(), "a b");
    }

    #[test]
    fn normalizes_crlf() {
        let t = Text::sanitize("a\r\nb\rc");
        assert_eq!(t.as_str(), "a\nb\nc");
    }

    #[test]
    fn keeps_paragraph_breaks() {
        let t = Text::sanitize("para one\n\npara two");
        assert_eq!(t.as_str(), "para one\n\npara two");
    }

    #[test]
    fn collapses_excess_newlines_to_paragraph_break() {
        let t = Text::sanitize("para one\n\n\n\n\npara two");
        assert_eq!(t.as_str(), "para one\n\npara two");
    }

    #[test]
    fn trims_whole_text() {
        let t = Text::sanitize("   hello world   ");
        assert_eq!(t.as_str(), "hello world");
    }

    #[test]
    fn truncates_pathological_length() {
        let raw = "x".repeat(10 * 1024 * 1024);
        let t = Text::sanitize(&raw);
        assert!(t.as_str().chars().count() <= MAX_TEXT_CHARS);
    }

    #[test]
    fn truncates_at_char_boundary_with_multibyte() {
        let raw = "\u{1F600}".repeat(MAX_TEXT_CHARS + 100);
        let t = Text::sanitize(&raw);
        assert!(t.as_str().chars().count() <= MAX_TEXT_CHARS);
        // Must still be valid UTF-8 (guaranteed by String) and not panic.
        assert!(t.as_str().chars().all(|c| c == '\u{1F600}'));
    }

    #[test]
    fn preserves_cjk_and_emoji() {
        let t = Text::sanitize("日本語 emoji 🎉 test");
        assert_eq!(t.as_str(), "日本語 emoji 🎉 test");
    }

    #[test]
    fn single_line_collapses_newlines() {
        let t = Text::sanitize("line one\nline two\n\nline three");
        assert_eq!(t.single_line(), "line one line two line three");
    }

    #[test]
    fn sanitize_is_idempotent() {
        let raw = "\x1b[1mBold\x1b[0m\ttext\r\nwith\n\n\n\nparagraphs   and   spaces  ";
        let once = Text::sanitize(raw);
        let twice = Text::sanitize(once.as_str());
        assert_eq!(once, twice);
    }

    #[test]
    fn deserialize_sanitizes() {
        let json = "\"\\u001b[31mred\\u0007\"";
        let t: Text = serde_json::from_str(json).unwrap();
        assert_eq!(t.as_str(), "red");
    }

    #[test]
    fn serialize_roundtrip() {
        let t = Text::sanitize("hello world");
        let json = serde_json::to_string(&t).unwrap();
        let back: Text = serde_json::from_str(&json).unwrap();
        assert_eq!(t, back);
    }
}
