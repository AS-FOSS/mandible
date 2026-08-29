//! [`Text`]: the single point through which untrusted, tool-produced strings
//! enter mandible's intermediate representation.
//!
//! See spec §4.1. Every string mandible did not author itself — help output,
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
    /// 5. Normalize line endings to `\n`.
    /// 6. Unwrap hard-wrapped paragraphs: a single `\n` inside a paragraph
    ///    joins to a space (so a later re-wrap at the pane's actual width
    ///    produces clean lines instead of re-wrapping already-short,
    ///    pre-broken lines raggedly); `\n\n` stays a paragraph break;
    ///    indented/code-like lines and list items (`- `, `* `, `1. `) are
    ///    never joined to a neighbor, preserving block structure.
    /// 7. Collapse runs of horizontal whitespace to a single space.
    /// 8. Trim leading/trailing whitespace.
    /// 9. Truncate to [`MAX_TEXT_CHARS`] characters, at a char boundary.
    pub fn sanitize(raw: &str) -> Text {
        let no_escapes = strip_escapes(raw);
        Text(Self::finish_pipeline(&no_escapes))
    }

    /// Like [`Text::sanitize`], but for text known to originate as
    /// markdown-flavored prose (carapace-spec's `description`/
    /// `documentation` fields, which use `[label](uri)` links — including
    /// custom schemes like `man://` and `cmd://` — plus inline code,
    /// `**bold**`, and `*em*`/`_em_` markers).
    ///
    /// This is a conservative, targeted normalizer, not a general markdown
    /// parser: it recognizes exactly those four constructs and leaves
    /// anything else untouched. In particular it does not touch `[value]`
    /// usage-string brackets (no following `(...)`), and it requires
    /// non-word characters immediately outside `*em*`/`_em_` delimiters so
    /// it doesn't misfire on identifiers like `GIT_DIR` or globs.
    /// Recognized markup is replaced with its inner text; the surrounding
    /// URI/delimiters are discarded (plain-text fallback, since the detail
    /// pane doesn't yet render hyperlinks).
    pub fn sanitize_markdown(raw: &str) -> Text {
        let no_escapes = strip_escapes(raw);
        let normalized = normalize_markdown(&no_escapes);
        Text(Self::finish_pipeline(&normalized))
    }

    /// The tail of the sanitization pipeline, shared by [`Text::sanitize`]
    /// and [`Text::sanitize_markdown`] (which differ only in what happens
    /// to the text *before* this point: markdown normalization, if any,
    /// always runs immediately after escape-stripping and before anything
    /// else, so it never has to reason about tabs/backspace/control chars).
    fn finish_pipeline(after_escapes: &str) -> String {
        let overstruck = resolve_backspace(after_escapes);
        let no_control = strip_c0(&overstruck);
        let tabs_expanded = expand_tabs(&no_control, 8);
        let newlines_normalized = normalize_newlines(&tabs_expanded);
        let unwrapped = unwrap_paragraphs(&newlines_normalized);
        let collapsed = collapse_horizontal_whitespace(&unwrapped);
        let trimmed = trim_lines_and_whole(&collapsed);
        truncate_chars(&trimmed, MAX_TEXT_CHARS)
    }

    /// Like [`Text::sanitize`], but for text whose layout is the tool's own
    /// rather than mandible's (spec §4.1's second tier): the raw-help pane
    /// (`t`), whose entire job is showing a tool's own bytes as they
    /// arrived, and the usage synopses in `CommandNode::usage`, where the
    /// spacing that lines a tool's alternative invocation forms up is part
    /// of what the author wrote. `Text::sanitize` is the wrong gate for
    /// both: its steps 6-8 (unwrap hard-wrapped paragraphs, collapse
    /// whitespace runs, trim leading/trailing whitespace) are exactly what
    /// destroy column alignment, and column alignment is the one thing a
    /// side-by-side "does this match the raw pane's ground truth" review
    /// depends on.
    ///
    /// This still neutralizes terminal control sequences — the one thing
    /// the raw pane cannot safely pass through, since ANSI/OSC/DCS escapes,
    /// stray carriage returns, and other C0 controls could scramble the
    /// reader's terminal or misrepresent what arrived — and nothing else:
    ///
    /// 1. Strip ANSI/OSC/DCS escape sequences (shares [`strip_escapes`]
    ///    with [`Text::sanitize`] — same hazard, same fix).
    /// 2. Strip remaining C0 control characters and DEL, **including a
    ///    stray `\r`** — callers pass one already-line-split string at a
    ///    time (see below), so any `\r` still present did not terminate a
    ///    line and is exactly the "carriage return that lies about what's
    ///    on screen" hazard, not useful structure.
    /// 3. Expand tabs to spaces at 8-column stops. This is a neutralization
    ///    too, not a formatting choice: `ratatui` does not interpret `\t`
    ///    as a tab stop the way a real terminal does (`unicode-width`
    ///    gives it zero display width), so a raw tab left in would
    ///    *misalign* columns in the pane relative to what the reader's own
    ///    terminal shows for the same bytes — the opposite of this
    ///    function's purpose.
    /// 4. Truncate to [`MAX_TEXT_CHARS`], the same bound [`Text::sanitize`]
    ///    applies, so a pathological single line cannot blow up the pane.
    ///
    /// Deliberately **not** applied: unwrapping, whitespace-collapsing,
    /// trimming, or paragraph-break normalization — indentation and
    /// internal column alignment are preserved exactly as fetched, and
    /// blank lines are whatever the caller's own line-splitting already
    /// produced.
    ///
    /// Callers pass one already-line-split string at a time: a raw-help
    /// line, or one logical usage entry (the `--help` parser joins a
    /// wrapped synopsis's continuations before it ever gets here). Every
    /// other consumer of a `--help` probe — descriptions above all — keeps
    /// going through [`Text::sanitize`], which reflows prose. This is the
    /// layout tier of that split, not a redefinition of it.
    pub fn sanitize_preserving_layout(raw: &str) -> Text {
        let no_escapes = strip_escapes(raw);
        let no_control = strip_c0_keep_tabs(&no_escapes);
        let tabs_expanded = expand_tabs(&no_control, 8);
        Text(truncate_chars(&tabs_expanded, MAX_TEXT_CHARS))
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
///
/// `pub` (fix/usage-synopsis) so `help_text::sections` can run the same
/// stripping over a whole raw `--help` document *before* sectioning, not
/// only per-field at [`Text::sanitize`] emission time. Escapes reaching
/// the sectioning pass corrupt layout analysis that has nothing to do
/// with display: `systemd-creds --help` writes `[0mCommands:`, and
/// with the escape still in the string `mentions_commands_word` sees one
/// alphanumeric run, `0mCommands`, never `Commands` — the heading is
/// never recognized as introducing a command list, and the whole block
/// silently fails to become subcommands. AGENTS.md and
/// `help_text::mod`'s re-export block both record what a second,
/// drifting copy of a shared predicate has already cost this project, so
/// this reuses the exact function [`Text::sanitize`] already calls rather
/// than duplicating the state machine.
pub fn strip_escapes(input: &str) -> String {
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

/// Like [`strip_c0`], but for [`Text::sanitize_preserving_layout`]: strips
/// every C0 control character and DEL **except** `\t` (kept so
/// [`expand_tabs`] can still turn it into alignment-preserving spaces
/// afterward). Unlike `strip_c0`, `\n` and `\r` are *not* kept — this
/// function's only caller passes one already-line-split string at a time,
/// so a `\n`/`\r` reaching here did not terminate a line and is exactly the
/// "control character that could scramble the terminal" hazard
/// [`Text::sanitize_preserving_layout`] exists to neutralize, not
/// structure worth preserving.
fn strip_c0_keep_tabs(input: &str) -> String {
    input
        .chars()
        .filter(|&c| {
            let is_c0 = ('\u{0}'..='\u{1f}').contains(&c);
            !(is_c0 && c != '\t') && c != '\u{7f}'
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

/// Unwrap hard-wrapped paragraphs: within a block of text (separated by
/// blank lines), a `\n` that merely continues a sentence is replaced with a
/// space, so a later re-wrap at the render width produces clean lines
/// instead of re-wrapping already-short, pre-broken lines raggedly. Blank
/// lines (paragraph breaks) are preserved. Lines that look like list items
/// (`- `, `* `, `1. `) or that are indented (leading whitespace — treated
/// as code-like) are never joined to a neighboring line in either
/// direction, so genuine block structure survives.
///
/// Must run before [`collapse_horizontal_whitespace`], which would
/// otherwise erase the leading-whitespace signal this function uses to
/// detect indented/code-like lines.
fn unwrap_paragraphs(input: &str) -> String {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Prev {
        /// Start of input, or immediately after a blank line: the next
        /// line always starts fresh, no join decision to make.
        Fresh,
        /// The previous line was ordinary prose that may continue.
        Joinable,
        /// The previous line was a list item or indented/code-like line.
        Standalone,
    }

    let mut out = String::with_capacity(input.len());
    let mut prev = Prev::Fresh;
    for line in input.split('\n') {
        if line.trim().is_empty() {
            out.push_str("\n\n");
            prev = Prev::Fresh;
            continue;
        }
        let standalone = is_standalone_line(line);
        match prev {
            Prev::Fresh => out.push_str(line),
            Prev::Joinable if !standalone => {
                out.push(' ');
                out.push_str(line.trim_start());
            }
            Prev::Joinable | Prev::Standalone => {
                out.push('\n');
                out.push_str(line);
            }
        }
        prev = if standalone {
            Prev::Standalone
        } else {
            Prev::Joinable
        };
    }
    out
}

/// A line that should never be joined to a neighbor when unwrapping
/// paragraphs: indented (code-like), or a list item (`- `, `* `, `+ `, or
/// `N. `).
fn is_standalone_line(line: &str) -> bool {
    if line.starts_with(' ') || line.starts_with('\t') {
        return true;
    }
    if line.starts_with("- ") || line.starts_with("* ") || line.starts_with("+ ") {
        return true;
    }
    if let Some(dot) = line.find(". ") {
        if dot > 0 && line.as_bytes()[..dot].iter().all(|b| b.is_ascii_digit()) {
            return true;
        }
    }
    false
}

/// Recognize and normalize the small, closed set of markdown constructs
/// spec.md's carapace mapping notes call out: `[label](uri)` links (any
/// scheme, including `man://`/`cmd://`), inline `` `code` ``, `**bold**`,
/// and `*em*`/`_em_`. Anything else is left untouched — this is
/// deliberately not a general markdown parser (see [`Text::sanitize_markdown`]).
fn normalize_markdown(input: &str) -> String {
    let s = strip_markdown_links(input);
    let s = strip_paired_delim(&s, "`");
    let s = strip_paired_delim(&s, "**");
    let s = strip_emphasis_single_char(&s, '*');
    strip_emphasis_single_char(&s, '_')
}

/// Replace `[label](uri)` with `label`. Narrow by construction: the label
/// must be non-empty with no nested `[`/newline, and the uri must be
/// non-empty with no whitespace/newline/nested `(` — so this never
/// misfires on `[value]` usage-string brackets that aren't followed by
/// `(...)`.
fn strip_markdown_links(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '[' {
            if let Some((label, next_i)) = try_parse_link(&chars, i) {
                out.push_str(&label);
                i = next_i;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// If a valid `[label](uri)` starts at `chars[start]` (which must be
/// `'['`), return the label text and the index just past the closing `)`.
fn try_parse_link(chars: &[char], start: usize) -> Option<(String, usize)> {
    let mut j = start + 1;
    while j < chars.len() && chars[j] != ']' {
        if chars[j] == '\n' || chars[j] == '[' {
            return None;
        }
        j += 1;
    }
    if j >= chars.len() || j == start + 1 {
        return None;
    }
    if chars.get(j + 1) != Some(&'(') {
        return None;
    }
    let mut k = j + 2;
    while k < chars.len() && chars[k] != ')' {
        if chars[k] == '\n' || chars[k] == '(' || chars[k].is_whitespace() {
            return None;
        }
        k += 1;
    }
    if k >= chars.len() || k == j + 2 {
        return None;
    }
    let label: String = chars[start + 1..j].iter().collect();
    Some((label, k + 1))
}

/// Replace occurrences of `delim` + content + `delim` with just the
/// content, where content is non-empty and contains neither `delim` nor a
/// newline (the newline restriction is what keeps this from spanning a
/// multi-line fenced code block by accident). Used for backtick code spans
/// and `**bold**`.
fn strip_paired_delim(input: &str, delim: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    loop {
        let Some(open_idx) = rest.find(delim) else {
            out.push_str(rest);
            break;
        };
        let after_open = &rest[open_idx + delim.len()..];
        if let Some(close_rel) = after_open.find(delim) {
            let content = &after_open[..close_rel];
            if !content.is_empty() && !content.contains('\n') {
                out.push_str(&rest[..open_idx]);
                out.push_str(content);
                rest = &after_open[close_rel + delim.len()..];
                continue;
            }
        }
        out.push_str(&rest[..open_idx + delim.len()]);
        rest = &rest[open_idx + delim.len()..];
    }
    out
}

/// Replace `*em*`/`_em_`-style single-character emphasis with its inner
/// text, requiring a non-word character (or start/end of text)
/// immediately outside each delimiter. That boundary rule is what keeps
/// this from misfiring on `SNAKE_CASE_IDENTIFIERS` (an underscore inside a
/// word is never treated as an opening delimiter) or on stray asterisks in
/// glob-like text.
fn strip_emphasis_single_char(input: &str, delim: char) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == delim && (i == 0 || !is_word_char(chars[i - 1])) {
            if let Some((content, after)) = try_parse_emphasis(&chars, i, delim) {
                out.push_str(&content);
                i = after;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn try_parse_emphasis(chars: &[char], open: usize, delim: char) -> Option<(String, usize)> {
    let mut j = open + 1;
    while j < chars.len() && chars[j] != '\n' {
        if chars[j] == delim {
            let content: String = chars[open + 1..j].iter().collect();
            let after_ok = chars.get(j + 1).map(|c| !is_word_char(*c)).unwrap_or(true);
            let content_ok = !content.is_empty()
                && !content.starts_with(char::is_whitespace)
                && !content.ends_with(char::is_whitespace)
                && !content.contains(delim);
            return if after_ok && content_ok {
                Some((content, j + 1))
            } else {
                None
            };
        }
        j += 1;
    }
    None
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
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
mod markdown_tests {
    use super::*;

    #[test]
    fn strips_link_keeping_label() {
        let t = Text::sanitize_markdown("See [gittutorial](man://gittutorial/7) to start");
        assert_eq!(t.as_str(), "See gittutorial to start");
    }

    #[test]
    fn strips_link_with_https_scheme() {
        let t = Text::sanitize_markdown("visit [docs](https://example.com/docs) now");
        assert_eq!(t.as_str(), "visit docs now");
    }

    #[test]
    fn strips_link_with_cmd_scheme() {
        let t = Text::sanitize_markdown("use [gh pr create](cmd://gh/pr/create) instead");
        assert_eq!(t.as_str(), "use gh pr create instead");
    }

    #[test]
    fn does_not_touch_bracket_without_following_paren() {
        // Usage-string bracket syntax must survive untouched.
        let t = Text::sanitize_markdown("[OPTIONS] COMMAND [ARG...]");
        assert_eq!(t.as_str(), "[OPTIONS] COMMAND [ARG...]");
    }

    #[test]
    fn strips_inline_code_backticks() {
        let t = Text::sanitize_markdown("run `git bisect start` to begin");
        assert_eq!(t.as_str(), "run git bisect start to begin");
    }

    #[test]
    fn strips_bold() {
        let t = Text::sanitize_markdown("- **Configured providers** defined here");
        assert_eq!(t.as_str(), "- Configured providers defined here");
    }

    #[test]
    fn strips_single_asterisk_emphasis() {
        let t = Text::sanitize_markdown("changed *any* property of the project");
        assert_eq!(t.as_str(), "changed any property of the project");
    }

    #[test]
    fn strips_underscore_emphasis() {
        let t = Text::sanitize_markdown("run with _<cmd>_ and _<arg>_ should exit");
        assert_eq!(t.as_str(), "run with <cmd> and <arg> should exit");
    }

    #[test]
    fn does_not_touch_snake_case_identifiers() {
        let t = Text::sanitize_markdown("sync from $ANDROID_PRODUCT_OUT to the device");
        assert_eq!(t.as_str(), "sync from $ANDROID_PRODUCT_OUT to the device");
    }

    #[test]
    fn does_not_touch_multiple_underscore_env_vars_in_backticks() {
        let t = Text::sanitize_markdown("Use `GH_TOKEN` and `GH_DEBUG` for auth and logging");
        assert_eq!(t.as_str(), "Use GH_TOKEN and GH_DEBUG for auth and logging");
    }

    #[test]
    fn leaves_unpaired_delimiters_alone() {
        let t = Text::sanitize_markdown("this * has an unmatched asterisk");
        assert_eq!(t.as_str(), "this * has an unmatched asterisk");
    }

    #[test]
    fn does_not_span_multiline_code_fence() {
        let raw = "before\n```\nsome\ncode\n```\nafter";
        let t = Text::sanitize_markdown(raw);
        // Must not collapse the whole fenced block into one "code span";
        // backticks with a newline between them are left alone.
        assert!(t.as_str().contains('`'));
    }

    #[test]
    fn markdown_sanitize_is_idempotent() {
        let raw = "See [x](man://x/1) and `code` and **bold** and *em* and _em_";
        let once = Text::sanitize_markdown(raw);
        let twice = Text::sanitize_markdown(once.as_str());
        assert_eq!(once, twice);
    }

    #[test]
    fn unwrap_preserves_list_items() {
        let raw = "Intro line one\nIntro line two\n\n- item one\n- item two\n- item three";
        let t = Text::sanitize_markdown(raw);
        assert_eq!(
            t.as_str(),
            "Intro line one Intro line two\n\n- item one\n- item two\n- item three"
        );
    }

    #[test]
    fn unwrap_preserves_indented_lines() {
        let raw = "some prose\n    code line one\n    code line two\nmore prose";
        let t = Text::sanitize(raw);
        // Indented lines stay on their own line, not joined to neighbors.
        assert!(t.as_str().contains("some prose\n"));
        assert!(t.as_str().contains("code line one\n"));
    }

    #[test]
    fn hard_wrapped_paragraph_reflows_to_one_line() {
        let raw = "Git is a fast, scalable, distributed revision\ncontrol system with an\nunusually rich command set.";
        let t = Text::sanitize(raw);
        assert_eq!(
            t.as_str(),
            "Git is a fast, scalable, distributed revision control system with an unusually rich command set."
        );
    }
}

#[cfg(test)]
mod fixture_tests {
    use super::*;
    use std::collections::HashMap;

    fn fixtures() -> HashMap<String, String> {
        let json = include_str!("../tests/fixtures/carapace_markdown_samples.json");
        serde_json::from_str(json).expect("fixture file is valid JSON")
    }

    /// Defect A: raw markup must never leak into rendered text. Checked
    /// against every real fixture pulled from the vendored catalog (git,
    /// gh, adb, crush), not just synthetic strings.
    #[test]
    fn no_fixture_leaks_raw_markdown_link_syntax() {
        for (name, raw) in fixtures() {
            let sanitized = Text::sanitize_markdown(raw.as_str());
            assert!(
                !sanitized.as_str().contains("]("),
                "fixture {name:?} leaked raw markdown link syntax: {:?}",
                sanitized.as_str()
            );
        }
    }

    #[test]
    fn git_root_doc_links_become_plain_labels() {
        let fixtures = fixtures();
        let raw = &fixtures["git_root"];
        let sanitized = Text::sanitize_markdown(raw);
        let s = sanitized.as_str();
        assert!(
            s.contains("gittutorial"),
            "label text should survive: {s:?}"
        );
        assert!(
            !s.contains("man://"),
            "raw URI scheme should not leak: {s:?}"
        );
        assert!(!s.contains("]("), "{s:?}");
    }

    #[test]
    fn genuine_emphasis_fixture_strips_markers_without_mangling_identifiers() {
        // This fixture (git's `bisect` documentation) is long enough to
        // exceed MAX_TEXT_CHARS on its own, so the underscore-emphasized
        // placeholders near the end (`_<cmd>_`) may legitimately be
        // truncated away — this test checks the part that's guaranteed to
        // survive (early in the doc) plus that nothing panics on the much
        // messier surrounding text (headings, fenced code, asciidoc-style
        // definition lists).
        let fixtures = fixtures();
        let raw = &fixtures["genuine_emphasis"];
        let sanitized = Text::sanitize_markdown(raw);
        let s = sanitized.as_str();
        assert!(s.contains("git bisect picks a commit"), "{s:?}");
        assert!(
            s.contains("any property of your project"),
            "em marker around 'any' should be stripped: {s:?}"
        );
        assert!(s.chars().count() <= MAX_TEXT_CHARS);
    }

    #[test]
    fn underscore_emphasis_survives_when_not_truncated_away() {
        // Isolate just the tail fragment (well under the char cap) to
        // directly verify the `_<cmd>_`/`_<arg>_` markers are stripped.
        let raw = "Note that _<cmd>_ run with _<arg>_  should exit\nwith code 0";
        let sanitized = Text::sanitize_markdown(raw);
        let s = sanitized.as_str();
        assert!(s.contains("<cmd>"), "{s:?}");
        assert!(s.contains("<arg>"), "{s:?}");
        assert!(!s.contains('_'), "{s:?}");
    }

    #[test]
    fn snake_case_fixture_is_untouched_by_emphasis_stripping() {
        let fixtures = fixtures();
        let raw = &fixtures["snake_case_false_positive"];
        let sanitized = Text::sanitize_markdown(raw);
        assert!(sanitized.as_str().contains("ANDROID_PRODUCT_OUT"));
    }

    #[test]
    fn env_var_fixture_backticks_stripped_underscores_preserved() {
        let fixtures = fixtures();
        let raw = &fixtures["gh_env_vars"];
        let sanitized = Text::sanitize_markdown(raw);
        let s = sanitized.as_str();
        assert!(s.contains("GH_TOKEN"), "{s:?}");
        assert!(s.contains("GH_DEBUG"), "{s:?}");
        assert!(!s.contains('`'), "backticks should be stripped: {s:?}");
    }

    #[test]
    fn bold_list_fixture_strips_bold_and_links_keeps_list_structure() {
        let fixtures = fixtures();
        let raw = &fixtures["bold_sample"];
        let sanitized = Text::sanitize_markdown(raw);
        let s = sanitized.as_str();
        assert!(s.contains("Configured providers"), "{s:?}");
        assert!(!s.contains("**"), "{s:?}");
        assert!(!s.contains("]("), "{s:?}");
        // List item lines survive as their own lines.
        assert!(s.contains("\n- Configured providers"), "{s:?}");
        assert!(s.contains("\n- Known providers"), "{s:?}");
    }

    /// Defect B: a hard-wrapped source paragraph reflows into one logical
    /// line per paragraph (ready for the render-time re-wrap), rather than
    /// keeping its original ragged short lines.
    #[test]
    fn hard_wrapped_git_archive_doc_reflows_paragraphs() {
        let fixtures = fixtures();
        let raw = &fixtures["git_archive_hardwrap"];
        let sanitized = Text::sanitize_markdown(raw);
        let s = sanitized.as_str();
        // The original has "...the tree\nstructure for the named tree..."
        // hard-wrapped mid-sentence; after unwrapping there must be no
        // newline between "tree" and "structure".
        assert!(s.contains("tree structure for the named tree"), "{s:?}");
        // Paragraph breaks (blank line in the source) must still exist.
        assert!(s.contains("\n\n"), "paragraph break should survive: {s:?}");
    }

    #[test]
    fn list_items_fixture_keeps_each_bullet_on_its_own_line() {
        let fixtures = fixtures();
        let raw = &fixtures["list_items_sample"];
        let sanitized = Text::sanitize_markdown(raw);
        let s = sanitized.as_str();
        let bullet_lines: Vec<&str> = s.lines().filter(|l| l.starts_with("- ")).collect();
        assert!(
            bullet_lines.len() >= 3,
            "expected multiple preserved bullet lines, got {bullet_lines:?} in {s:?}"
        );
    }
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
        // Single \r\n / \r within a paragraph are, after normalization to
        // \n, subject to the same hard-wrap unwrapping as any other single
        // newline (see unwraps_single_newlines_within_a_paragraph below) —
        // this test only asserts CRLF/CR are normalized to LF, using
        // list-item lines so unwrap_paragraphs doesn't join them and mask
        // what's being tested.
        let t = Text::sanitize("- a\r\n- b\r- c");
        assert_eq!(t.as_str(), "- a\n- b\n- c");
    }

    #[test]
    fn unwraps_single_newlines_within_a_paragraph() {
        let t = Text::sanitize("a\nb\nc");
        assert_eq!(t.as_str(), "a b c");
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

    // --- sanitize_preserving_layout: the raw-help display path ---

    #[test]
    fn preserving_layout_keeps_leading_indentation() {
        // The defect this function exists to fix: `Text::sanitize` would
        // trim this to "-a, --all  write counts for all files".
        let t = Text::sanitize_preserving_layout("  -a, --all  write counts for all files");
        assert_eq!(t.as_str(), "  -a, --all  write counts for all files");
    }

    #[test]
    fn preserving_layout_keeps_internal_column_gaps() {
        // `Text::sanitize` would collapse the multi-space gap between the
        // flag spelling and its description to a single space.
        let t = Text::sanitize_preserving_layout("--block-size=SIZE    scale sizes by SIZE");
        assert_eq!(t.as_str(), "--block-size=SIZE    scale sizes by SIZE");
    }

    #[test]
    fn preserving_layout_still_strips_ansi_escapes() {
        let t = Text::sanitize_preserving_layout("\x1b[31mred\x1b[0m text");
        assert_eq!(t.as_str(), "red text");
    }

    #[test]
    fn preserving_layout_strips_osc_sequence() {
        let t = Text::sanitize_preserving_layout("\x1b]0;window title\x07visible");
        assert_eq!(t.as_str(), "visible");
    }

    #[test]
    fn preserving_layout_strips_stray_carriage_return() {
        // A `\r` mid-line (progress-bar style) would otherwise scramble a
        // real terminal by moving the cursor back to column 0; the raw
        // pane must not pass that through.
        let t = Text::sanitize_preserving_layout("done\rDONE");
        assert_eq!(t.as_str(), "doneDONE");
        assert!(!t.as_str().contains('\r'));
    }

    #[test]
    fn preserving_layout_strips_other_c0_controls() {
        let t = Text::sanitize_preserving_layout("hello\x01\x02world");
        assert_eq!(t.as_str(), "helloworld");
    }

    #[test]
    fn preserving_layout_expands_tabs_instead_of_leaving_them_raw() {
        // ratatui gives `\t` zero display width, so leaving it raw would
        // misalign columns rather than preserve them — expansion is the
        // neutralization that keeps this function's own promise.
        let t = Text::sanitize_preserving_layout("a\tb");
        assert_eq!(t.as_str(), "a       b");
        assert!(!t.as_str().contains('\t'));
    }

    #[test]
    fn preserving_layout_does_not_trim_or_collapse_whitespace() {
        let t = Text::sanitize_preserving_layout("   a    b   ");
        assert_eq!(t.as_str(), "   a    b   ");
    }

    #[test]
    fn preserving_layout_bounds_pathological_length() {
        let raw = "x".repeat(10 * 1024 * 1024);
        let t = Text::sanitize_preserving_layout(&raw);
        assert!(t.as_str().chars().count() <= MAX_TEXT_CHARS);
    }

    #[test]
    fn preserving_layout_is_idempotent() {
        let raw = "\x1b[1mBold\x1b[0m\t  text  with\rstray CR";
        let once = Text::sanitize_preserving_layout(raw);
        let twice = Text::sanitize_preserving_layout(once.as_str());
        assert_eq!(once, twice);
    }
}
