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
/// normalizes newlines, reflows each paragraph — unwrapping prose, keeping
/// the breaks the author marked as structure — and truncates to
/// [`MAX_TEXT_CHARS`]. Widgets and other consumers may assume a `Text` is
/// safe to place directly into a rendering surface: every newline it holds
/// is one `sanitize` put there, a paragraph break or a preserved structural
/// break, never a raw one from tool output.
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
    /// 6. Reflow each paragraph ([`reflow_structured`]): flowing prose
    ///    unwraps to one logical line so a later re-wrap at the pane's
    ///    actual width produces clean lines instead of re-wrapping
    ///    already-short, pre-broken ones raggedly, while a line the author
    ///    marked as structure — indented deeper than its paragraph, a list
    ///    row, or an example invocation — keeps its own break and its
    ///    relative indentation. `\n\n` stays a paragraph break.
    /// 7. Collapse runs of horizontal whitespace within a line to a single
    ///    space, and trim leading/trailing whitespace (step 6 does both,
    ///    per logical line, so that a preserved line's own indentation
    ///    survives).
    /// 8. Truncate to [`MAX_TEXT_CHARS`] characters, at a char boundary.
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
        let reflowed = reflow_structured(&newlines_normalized);
        truncate_chars(&reflowed, MAX_TEXT_CHARS)
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

/// How far a preserved structural line may be indented, in columns,
/// relative to its own paragraph.
///
/// The relative indent is what carries "this row sits under that one", and
/// a couple of levels of it is all any `--help` text expresses. The number
/// is a bound on the damage a pathological source can do, not a style
/// choice: a tool that documents a description inside a 30-column table
/// would otherwise hand the detail pane a 30-column indent to honour, and
/// at the pane's real width (41 columns in a 90-column terminal, spec
/// §9.3) that leaves a description no room to be read in. Deeper source
/// indentation is clamped to this, so nesting still reads as nesting and
/// the prose keeps its width.
const MAX_STRUCTURAL_INDENT: usize = 8;

/// Reflow each paragraph, keeping the structure its author marked and
/// unwrapping everything else (spec §4.1's prose tier).
///
/// A `--help` description is two kinds of text at once, and the previous
/// pass only modelled one of them. Prose is hard-wrapped to whatever width
/// the tool's author happened to write for; the pane re-wraps it to its
/// own width, so those breaks are noise and joining them is what stops a
/// re-wrap from coming out ragged. But a bullet list, an indented block,
/// and an example invocation are deliberate: `grep --help`'s
///
/// ```text
/// Search for PATTERNS in each FILE.
/// Example: grep -i 'hello world' menu.h main.c
/// PATTERNS can contain multiple patterns separated by newlines.
/// ```
///
/// has all three lines at column 0 with no blank line anywhere, so a pass
/// that reads structure out of leading whitespace alone saw one paragraph
/// and rendered `Example: grep -i 'hello world' menu.h main.c PATTERNS can
/// contain multiple patterns...` — the example smeared into the sentence
/// after it.
///
/// So structure is recognized per line, against the paragraph it sits in:
///
/// - **Deeper than its paragraph's base indent.** The base is the smallest
///   indentation any line in the paragraph has, so a *uniformly* indented
///   block is ordinary prose (it reflows, which the old leading-whitespace
///   test would not let it do) and only a line that is indented *within*
///   its block counts as structure.
/// - **A list row** ([`is_list_row`]): `- `, `* `, `+ `, `• `, `1. `, `1) `.
/// - **An example invocation** ([`is_example_row`]): an `Example:`/`e.g.`
///   label followed by command-shaped text.
///
/// A structural line keeps its own break and its indentation relative to
/// the paragraph's base (clamped to [`MAX_STRUCTURAL_INDENT`]); the line
/// after one starts fresh rather than joining onto it. Everything else
/// joins to the flowing line above it with a single space. Paragraph
/// breaks (`\n\n`) survive, runs of blank lines collapse to one break, and
/// each logical line has its internal whitespace collapsed and its ends
/// trimmed — which is why this pass subsumes the separate collapse/trim
/// passes it replaced: doing them globally would erase the very
/// indentation it just decided to keep.
fn reflow_structured(input: &str) -> String {
    let mut paragraphs: Vec<String> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for line in input.split('\n') {
        if line.trim().is_empty() {
            if !current.is_empty() {
                paragraphs.push(reflow_paragraph(&current));
                current.clear();
            }
            continue;
        }
        current.push(line);
    }
    if !current.is_empty() {
        paragraphs.push(reflow_paragraph(&current));
    }
    paragraphs.retain(|p| !p.is_empty());
    paragraphs.join("\n\n")
}

/// [`reflow_structured`] for one paragraph's worth of non-blank lines.
fn reflow_paragraph(lines: &[&str]) -> String {
    let base = lines
        .iter()
        .map(|l| leading_spaces(l))
        .min()
        .unwrap_or_default();

    // Each entry is one logical line: its indent relative to `base`, and
    // its whitespace-collapsed content.
    let mut logical: Vec<(usize, String)> = Vec::new();
    let mut prev_flows = false;
    for line in lines {
        let content = collapse_spaces(line.trim());
        if content.is_empty() {
            continue;
        }
        let relative = leading_spaces(line).saturating_sub(base);
        let structural = relative > 0 || is_list_row(&content) || is_example_row(&content);
        if !structural && prev_flows {
            if let Some((_, last)) = logical.last_mut() {
                last.push(' ');
                last.push_str(&content);
            }
        } else {
            let indent = if structural {
                relative.min(MAX_STRUCTURAL_INDENT)
            } else {
                0
            };
            logical.push((indent, content));
        }
        prev_flows = !structural;
    }

    logical
        .into_iter()
        .map(|(indent, content)| format!("{}{content}", " ".repeat(indent)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Leading space count. Tabs are already expanded to spaces by this point
/// in the pipeline, so a column count and a space count are the same
/// number.
fn leading_spaces(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

/// Collapse every run of spaces in an already-trimmed line to one space.
fn collapse_spaces(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut in_run = false;
    for c in line.chars() {
        if c == ' ' {
            in_run = true;
            continue;
        }
        if in_run && !out.is_empty() {
            out.push(' ');
        }
        in_run = false;
        out.push(c);
    }
    out
}

/// A list row: a bullet (`- `, `* `, `+ `, `• `) or an enumerator
/// (`1. `, `1) `). Shape only — the marker and the space after it, never a
/// word.
fn is_list_row(line: &str) -> bool {
    for marker in ["- ", "* ", "+ ", "\u{2022} "] {
        if line.starts_with(marker) {
            return true;
        }
    }
    for sep in [". ", ") "] {
        if let Some(at) = line.find(sep) {
            if at > 0 && line.as_bytes()[..at].iter().all(|b| b.is_ascii_digit()) {
                return true;
            }
        }
    }
    false
}

/// An example row: a label (`Example:`, `Examples:`, `e.g.`, `eg:`, `For
/// example:`) followed by text shaped like a command invocation.
///
/// Recognized by **shape, never by name** (AGENTS.md §1): the label is a
/// fixed vocabulary of English documentation words, and what follows it is
/// judged by [`looks_like_invocation`], which knows nothing about any
/// particular tool. `Example: grep -i 'hello world' menu.h main.c` is an
/// example row for exactly the same reason `Example: mytool --dry-run x`
/// is — a bare word followed by an option — and `Example: the second form
/// is usually what you want` is not one, for the same reason either way.
fn is_example_row(line: &str) -> bool {
    example_label_rest(line).is_some_and(looks_like_invocation)
}

/// The text after an example label, or `None` if the line does not open
/// with one. Case-insensitive, and the label must start the line.
fn example_label_rest(line: &str) -> Option<&str> {
    for label in ["for example:", "examples:", "example:", "e.g.", "eg:"] {
        if let Some(head) = line.get(..label.len()) {
            if head.eq_ignore_ascii_case(label) {
                let rest = line[label.len()..].trim();
                return (!rest.is_empty()).then_some(rest);
            }
        }
    }
    None
}

/// Whether `text` is shaped like a command someone could type: a bare
/// command word, then at least one option or shell operator.
///
/// The second half is what makes this safe to key structure off. Without
/// it, "the second form is usually what you want" passes — its first word
/// is bare and word-shaped like any command name — and every prose
/// sentence after an `Example:` label would be pulled out of the flow it
/// belongs in. Requiring a `-x`/`--xyz` token or a shell operator is a
/// deliberately strict rule that misses `Example: cp src dst` rather than
/// admit a sentence, per AGENTS.md §5: a recognizer that fires on prose
/// cannot be used to decide layout.
fn looks_like_invocation(text: &str) -> bool {
    let mut tokens = text.split_whitespace();
    let Some(head) = tokens.next() else {
        return false;
    };
    if !is_command_word(head) {
        return false;
    }
    tokens.clone().any(is_option_token)
        || ["|", ">", "<", "&&", "$("]
            .iter()
            .any(|op| text.contains(op))
}

/// A bare command word: `grep`, `git`, `foo.sh`, `./run`, `dpkg-query`.
/// Starts with a letter, a dot or a slash, ends alphanumeric (so a word
/// carrying sentence punctuation — `files,` or `it.` — is not one), and
/// holds nothing but the characters a program name is spelled with.
fn is_command_word(word: &str) -> bool {
    let mut chars = word.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '.' || first == '/') {
        return false;
    }
    if !word.ends_with(|c: char| c.is_ascii_alphanumeric()) {
        return false;
    }
    word.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/' | '+'))
}

/// An option token: a dash followed by at least one more option character.
/// `-i`, `--dry-run`, `--`-prefixed long forms; never a bare `-` (stdin)
/// and never an em-dash-looking run of hyphens with nothing after it.
fn is_option_token(word: &str) -> bool {
    let Some(rest) = word.strip_prefix('-') else {
        return false;
    };
    let rest = rest.strip_prefix('-').unwrap_or(rest);
    rest.starts_with(|c: char| c.is_ascii_alphanumeric())
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

    // --- the structure recognizer (spec §4.1's prose tier) ---

    /// The defect this exists for, in `grep --help`'s own words: three
    /// column-0 lines, no blank line anywhere, and the middle one is an
    /// example invocation that used to be smeared into the sentence after
    /// it.
    #[test]
    fn example_row_keeps_its_own_line_between_two_prose_sentences() {
        let raw = "Search for PATTERNS in each FILE.\n\
                   Example: grep -i 'hello world' menu.h main.c\n\
                   PATTERNS can contain multiple patterns separated by newlines.";
        let t = Text::sanitize(raw);
        assert_eq!(
            t.as_str(),
            "Search for PATTERNS in each FILE.\n\
             Example: grep -i 'hello world' menu.h main.c\n\
             PATTERNS can contain multiple patterns separated by newlines."
        );
    }

    /// The anti-case, and the reason the recognizer wants an option or a
    /// shell operator rather than just a bare first word: an `Example:`
    /// label introducing a *sentence* is prose, and prose reflows.
    #[test]
    fn example_label_introducing_prose_still_reflows() {
        let raw = "Consider the second form.\n\
                   Example: the second form is usually what you want here\n\
                   and it takes no options at all.";
        let t = Text::sanitize(raw);
        assert_eq!(
            t.as_str(),
            "Consider the second form. Example: the second form is usually what you want here and it takes no options at all."
        );
        assert!(!t.as_str().contains('\n'));
    }

    #[test]
    fn eg_label_with_an_invocation_is_an_example_row() {
        let raw = "Filters the output.\ne.g. mytool --dry-run build\nThe filter is applied last.";
        let t = Text::sanitize(raw);
        assert_eq!(
            t.as_str(),
            "Filters the output.\ne.g. mytool --dry-run build\nThe filter is applied last."
        );
    }

    #[test]
    fn example_row_is_recognized_by_a_shell_operator_too() {
        let raw = "Reads stdin.\nExample: mytool build | tee log.txt\nOutput goes to stdout.";
        let t = Text::sanitize(raw);
        assert!(
            t.as_str()
                .contains("\nExample: mytool build | tee log.txt\n"),
            "{:?}",
            t.as_str()
        );
    }

    #[test]
    fn bullet_rows_keep_their_lines_at_the_paragraph_base() {
        let raw = "Choose one of:\n- alpha does a thing\n* beta does another\n\u{2022} gamma\n1. delta\n2) epsilon";
        let t = Text::sanitize(raw);
        assert_eq!(
            t.as_str(),
            "Choose one of:\n- alpha does a thing\n* beta does another\n\u{2022} gamma\n1. delta\n2) epsilon"
        );
    }

    #[test]
    fn a_line_indented_deeper_than_its_paragraph_keeps_break_and_relative_indent() {
        let raw = "Modes are:\n    fast   skip the checks\n    safe   run every check\nPick one.";
        let t = Text::sanitize(raw);
        assert_eq!(
            t.as_str(),
            "Modes are:\n    fast skip the checks\n    safe run every check\nPick one."
        );
    }

    /// The other anti-case, and the behaviour change the base-indent rule
    /// buys: a paragraph that is *uniformly* indented is ordinary
    /// hard-wrapped prose, and must unwrap. The old leading-whitespace
    /// test read every one of these lines as code-like and left the
    /// pane re-wrapping already-short lines raggedly.
    #[test]
    fn uniformly_indented_hard_wrapped_prose_still_unwraps() {
        let raw = "    Summarize device usage of the set of\n    FILEs, recursively for\n    directories.";
        let t = Text::sanitize(raw);
        assert_eq!(
            t.as_str(),
            "Summarize device usage of the set of FILEs, recursively for directories."
        );
    }

    #[test]
    fn relative_indent_is_clamped_rather_than_honoured_to_any_depth() {
        let deep = " ".repeat(30);
        let raw = format!("Table:\n{deep}value  what it means");
        let t = Text::sanitize(&raw);
        assert_eq!(t.as_str(), "Table:\n        value what it means");
    }

    #[test]
    fn structure_preservation_survives_a_second_sanitize() {
        let raw = "Search for PATTERNS in each FILE.\n\
                   Example: grep -i 'hello world' menu.h main.c\n\
                   - or use a list row\n\
                   PATTERNS can contain multiple patterns.";
        let once = Text::sanitize(raw);
        let twice = Text::sanitize(once.as_str());
        assert_eq!(once, twice);
    }

    #[test]
    fn single_line_collapses_preserved_structure_for_the_tree_pane() {
        let t = Text::sanitize("Search for PATTERNS.\nExample: grep -i x menu.h\n- and a bullet");
        assert_eq!(
            t.single_line(),
            "Search for PATTERNS. Example: grep -i x menu.h - and a bullet"
        );
        assert!(!t.single_line().contains('\n'));
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
