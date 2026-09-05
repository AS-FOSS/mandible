//! Width-aware wrapping and horizontal-scroll windowing for one line.

use super::*;

/// The visible window of `s` for horizontal scrolling of preformatted
/// content: `offset` display-width columns trimmed off the left, capped to
/// `width` columns of what remains (else it falls through to `Paragraph`'s
/// defensive `Wrap` and gets reflowed, spec §9). Indexes character-by-
/// character, never a byte offset (AGENTS.md); a double-width character
/// straddling a boundary is dropped whole. Regression fixture: `ip`
/// rendered through a real pty (AGENTS.md §3.2). Returns the window plus
/// per-edge clip flags, fed to [`draw_clip_marker_rails`].
pub(super) fn hscroll_line(s: &str, offset: usize, width: usize) -> (Line<'static>, bool, bool) {
    let total = display_width(s);
    if offset == 0 && total <= width {
        return (Line::from(s.to_string()), false, false);
    }
    if total <= offset || width == 0 {
        return (Line::default(), false, false);
    }
    let clipped_left = offset > 0;
    let clipped_right = total > offset + width;
    let line = Line::from(hscroll_window(s, offset, width).into_owned());
    (line, clipped_left, clipped_right)
}

/// Wrap one preformatted line to `width` display columns, losing nothing
/// (wrap-mode counterpart of [`hscroll_line`], used with `[ui]
/// horizontal_scroll` off, spec §9). Not [`wrap_words`]: that collapses
/// interior spacing, destroying a tool's own column-aligned table (spec
/// §4.1). A fitting line returns byte-identical; an over-wide one cuts at
/// a whitespace boundary when one exists, else hard-cuts between
/// characters; continuation rows carry the line's own indent, dropped
/// past half the pane. Character-by-character, never a byte slice
/// (AGENTS.md); [`width_prefix_end`] only returns real `char` boundaries.
pub(super) fn wrap_preformatted(line: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    if display_width(line) <= width {
        return vec![line.to_string()];
    }
    let indent_width = display_width(&line[..line.len() - line.trim_start().len()]);
    let hang = if indent_width * 2 < width {
        " ".repeat(indent_width)
    } else {
        String::new()
    };

    let mut rows: Vec<String> = Vec::new();
    let mut rest = line;
    while !rest.is_empty() {
        let prefix = if rows.is_empty() { "" } else { hang.as_str() };
        let avail = width.saturating_sub(display_width(prefix)).max(1);
        let mut cut = width_prefix_end(rest, avail);
        if cut == 0 {
            // A single character wider than the whole budget: take it
            // anyway and overflow by the unavoidable minimum, exactly as
            // `break_overlong_word` does, rather than loop forever.
            cut = rest
                .chars()
                .next()
                .map_or(rest.len(), |c: char| c.len_utf8());
        }
        if cut == rest.len() {
            rows.push(format!("{prefix}{rest}"));
            break;
        }
        // Prefer a whitespace boundary inside the window, so a word is not
        // split when it did not have to be — but never one that would emit
        // a row with no content on it.
        let mut end = cut;
        if let Some(pos) = rest[..cut].rfind(char::is_whitespace) {
            if !rest[..pos].trim().is_empty() {
                end = pos;
            }
        }
        rows.push(format!("{prefix}{}", &rest[..end]));
        rest = if end < cut {
            // Broke at whitespace: that run was the break, not content.
            rest[end..].trim_start()
        } else {
            &rest[end..]
        };
    }
    rows
}

/// The byte index ending the longest prefix of `s` that fits `width`
/// display columns — always a `char` boundary, so slicing at it cannot
/// panic on multi-byte input.
pub(super) fn width_prefix_end(s: &str, width: usize) -> usize {
    let mut used = 0usize;
    for (idx, ch) in s.char_indices() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + w > width {
            return idx;
        }
        used += w;
    }
    s.len()
}

pub(super) fn hscroll_window(s: &str, offset: usize, width: usize) -> std::borrow::Cow<'_, str> {
    if offset == 0 && display_width(s) <= width {
        return std::borrow::Cow::Borrowed(s);
    }
    let mut remaining = offset;
    let mut budget = width;
    let mut result = String::new();
    let mut trimming = offset > 0;
    for ch in s.chars() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if trimming {
            if w <= remaining {
                remaining -= w;
                continue;
            }
            trimming = false;
        }
        if w > budget {
            break;
        }
        budget -= w;
        result.push(ch);
    }
    std::borrow::Cow::Owned(result)
}

/// Greedy word-wrap of `text` to at most `width` display columns per
/// line, never breaking a word unless it alone exceeds `width`, in which
/// case it breaks across as many lines as it takes ([`break_overlong_word`])
/// rather than truncating: a truncated token is unrecoverable from the
/// parsed view. Fixture: `smokecli unbreakable url`. Always returns at
/// least one (possibly empty) chunk.
pub(super) fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;

    for word in text.split_whitespace() {
        let word_width = display_width(word);
        let sep_width = usize::from(!current.is_empty());
        if current_width + sep_width + word_width <= width {
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(word);
            current_width += sep_width + word_width;
            continue;
        }
        if !current.is_empty() {
            lines.push(std::mem::take(&mut current));
            current_width = 0;
        }
        if word_width > width {
            lines.extend(break_overlong_word(word, width));
        } else {
            current.push_str(word);
            current_width = word_width;
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// True when `s` (a single word, no surrounding punctuation) reads as an
/// enumerator's own key: a decimal run (`"0"`, `"12"`) or a `0x`-prefixed
/// hex run (`"0x10"`) — `sg_luns --select`'s continuation lines number
/// their entries both ways in the same list. Never a bare `x`/`0x` with
/// nothing after it.
fn is_enumerator_key(s: &str) -> bool {
    match s.strip_prefix("0x") {
        Some(hex) => !hex.is_empty() && hex.chars().all(|c| c.is_ascii_hexdigit()),
        None => !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()),
    }
}

/// `text` split into `(byte_offset, word)` pairs at whitespace boundaries —
/// [`str::split_whitespace`] plus the offsets it doesn't give, needed to
/// cut `text` at a word start rather than re-tokenizing it after the fact.
fn word_offsets(text: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some(&(start, c)) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        let mut end = start;
        while let Some(&(idx, c)) = chars.peek() {
            if c.is_whitespace() {
                break;
            }
            end = idx + c.len_utf8();
            chars.next();
        }
        out.push((start, &text[start..end]));
    }
    out
}

/// True when `word` (at position `i` in `words`) opens an enumerated item:
/// a token shaped `N ->`, `N:`, a bare `-`, or a bare `*`, where `N` is
/// [`is_enumerator_key`]-shaped.
fn opens_enumerated_item(words: &[(usize, &str)], i: usize) -> bool {
    let word = words[i].1;
    matches!(word, "-" | "*")
        || (is_enumerator_key(word) && words.get(i + 1).is_some_and(|(_, w)| *w == "->"))
        || (word.len() > 1 && word.ends_with(':') && is_enumerator_key(&word[..word.len() - 1]))
}

/// Byte offsets in `text` where an enumerated item opens (spec §16's
/// enumerator-continuation rule), never the very first word — that item
/// already opens its own segment by construction.
///
/// Empty unless at least *two* items open somewhere in `text` (counting
/// the first word too): a lone `N ->` sitting in otherwise ordinary prose
/// (`sg_luns --maxlen`'s `"(def: 0 -> 8192 bytes)"`) is a range, not a
/// list, and forcing a break there would make an already-fitting line
/// ragged for nothing. `sg_luns --select`'s six `N ->` openers are what
/// this rule exists for.
pub(super) fn enumerator_breaks(text: &str) -> Vec<usize> {
    let words = word_offsets(text);
    let opens: Vec<usize> = (0..words.len())
        .filter(|&i| opens_enumerated_item(&words, i))
        .collect();
    if opens.len() < 2 {
        return Vec::new();
    }
    opens
        .into_iter()
        .filter(|&i| i > 0)
        .map(|i| words[i].0)
        .collect()
}

/// [`wrap_words`], but a description never folds two enumerated items onto
/// one wrapped line (spec §16, `sg_luns --select`'s `0 -> ... 1 -> ...`
/// paragraph): `text` is first cut at every [`enumerator_breaks`] offset,
/// each resulting segment wrapped independently, so an item's continuation
/// still wraps at `width` but a new item always starts its own line. No
/// text is dropped or reordered — a description with no such token wraps
/// exactly as [`wrap_words`] already did.
pub(super) fn wrap_description(text: &str, width: usize) -> Vec<String> {
    let breaks = enumerator_breaks(text);
    if breaks.is_empty() {
        return wrap_words(text, width);
    }
    let mut lines = Vec::new();
    let mut start = 0;
    for cut in breaks.into_iter().chain(std::iter::once(text.len())) {
        let segment = text[start..cut].trim();
        if !segment.is_empty() {
            lines.extend(wrap_words(segment, width));
        }
        start = cut;
    }
    lines
}

/// Break a single token wider than `width` display columns into as many
/// width-limited chunks as it takes, so the token survives intact rather
/// than being lost to an ellipsis truncation. Splits by summed
/// [`unicode_width`], never byte index (AGENTS.md) and never `char` count
/// (a double-width CJK/emoji character could overflow the line by one
/// cell). A single character wider than `width` gets its own oversized
/// chunk; no cut point inside a character exists.
pub(super) fn break_overlong_word(word: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_width = 0usize;

    for c in word.chars() {
        let c_width = UnicodeWidthChar::width(c).unwrap_or(0);
        if current_width + c_width > width && !current.is_empty() {
            chunks.push(std::mem::take(&mut current));
            current_width = 0;
        }
        current.push(c);
        current_width += c_width;
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}
