//! The detail pane: a breadcrumb header over **one scrollable document of
//! sections** — `DESCRIPTION`, `USAGE`, `POSITIONALS`, `FLAGS`,
//! `MODIFIERS`, `ENVIRONMENT`, in that order and only when non-empty
//! (spec §9.3).
//!
//! The four list sections are driven purely by
//! [`mandible_core::EntityKind`]: one loop over [`LIST_SECTIONS`] renders
//! all four, so every kind is rendered through the same code and no
//! section can acquire behaviour of its own — proven twice over, first
//! when the parser began emitting modifiers with no change to this file,
//! then again for environment variables. Within a section,
//! [`mandible_core::Entity::group`] renders as a divider rule and
//! inherited entities land in a final dimmed `Inherited` group
//! (spec §9, §9.3).
//!
//! **Every line handed to the `Paragraph` is already wrapped to the
//! pane's exact width before it gets there** — both the description
//! prose and each flag's spelling/description — rather than leaning on
//! `ratatui`'s own `Wrap` to do it. Two reasons:
//!
//! 1. A flag's description continuation must hang-indent under the
//!    description column, not restart at column 0 (spec-adjacent
//!    feedback: `ratatui::widgets::Wrap` re-wraps a `Line` with no memory
//!    of where useful content started, so a flag line handed to it as one
//!    long `Span` run comes back flush-left on continuation — the single
//!    biggest readability problem the pane had).
//! 2. Search selecting a flag needs to scroll the pane to *that exact
//!    on-screen row* (spec §10's "closes the loop" requirement). That's
//!    only possible if the `Line` index we compute during layout is the
//!    same index the `Paragraph` actually renders at — which requires
//!    controlling 100% of the wrapping ourselves, not delegating part of
//!    it to a widget whose reflow decisions happen after this function
//!    returns.
//!
//! `Wrap` stays enabled on the `Paragraph` purely as a defensive
//! fallback (spec §9's border-corruption lesson: untrusted text reaching
//! a `Span` unclipped is how that happened before) — every line we
//! construct should already fit, so it should never need to act.

use crate::app::{App, Focus};
use crate::glyphs::Glyphs;
use crate::sanitize::{defensive_single_line, display_width, truncate_to_width_marker};
use crate::style;
use mandible_core::{CommandNode, Dashes, Entity, EntityKind, FlagKey, Spelling, Text, ValueKind};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Padding, Paragraph, Wrap};
use ratatui::Frame;
use std::collections::HashMap;
use unicode_width::UnicodeWidthChar;

/// Render the detail pane into `area`.
pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let focused = app.focus == Focus::Detail;
    let border_style = if focused {
        style::accent(app.color_enabled)
    } else {
        Style::default()
    };

    let breadcrumb = app
        .selected_row()
        .map(|r| {
            r.path
                .iter()
                .map(|s| defensive_single_line(s))
                .collect::<Vec<_>>()
                .join(&format!(" {} ", app.glyphs.breadcrumb))
        })
        .unwrap_or_default();
    let title = format!(" {breadcrumb} ");

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_set(style::border_set(app.glyphs))
        .border_style(border_style)
        // A column of breathing room either side, so prose and flag rows
        // don't butt against the border. `Block::padding` takes it out of
        // the inner rect, so every width calculation downstream — wrapping,
        // the description column, truncation — accounts for it without
        // knowing it exists.
        .padding(Padding::horizontal(1));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let Some(node) = app.selected_node() else {
        let paragraph = Paragraph::new("Nothing selected.");
        frame.render_widget(paragraph, inner);
        return;
    };

    // The user asked to see the tool's own bytes (`t`). Checked before the
    // parsed rendering and before the degradation check below, because it
    // is an override of both: the whole point is to see past whatever
    // mandible decided.
    if let Some(raw) = app.raw_help_for_selected() {
        render_raw_mode(frame, inner, app, raw);
        draw_hscroll_affordance(frame, area, app);
        return;
    }

    // Level 3 of spec §7 Tier B's staged degradation (batch 6 part 4): no
    // parse produced anything structurally plausible for this node, so it
    // carries the tool's own raw `--help` text instead of invented
    // structure. This is a fundamentally different rendering, not a
    // variant of the structured one below — see `render_verbatim`.
    if !node.unparsed.is_empty() {
        render_verbatim(
            frame,
            inner,
            app,
            &format!("unparsed {} showing raw --help output", app.glyphs.absent),
            node.unparsed.iter().map(|t| t.as_str().to_string()),
        );
        draw_hscroll_affordance(frame, area, app);
        return;
    }

    let width = inner.width as usize;
    let built = build_lines(
        node,
        app.show_hidden,
        width,
        app.palette,
        app.selected_flag.as_ref(),
        app.glyphs,
        app,
    );
    // Search selecting a flag scrolls straight to it (spec §10): the line
    // index is exact because every line above was pre-wrapped by us, not
    // by the widget's own `Wrap` after the fact. Falls back to the user's
    // own scroll position once nothing is flag-targeted.
    // Tell `App` how far this content can scroll, so `↓` stops at the end
    // instead of pushing it off the top into blank space.
    app.set_detail_extent(built.lines.len(), inner.height as usize);
    let scroll = match built.target_flag_line {
        Some(line) => target_scroll(&built, line, inner.height as usize),
        None => app.clamped_detail_scroll(),
    } as u16;
    let paragraph = Paragraph::new(built.lines)
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));
    frame.render_widget(paragraph, inner);
    draw_clip_marker_rails(
        frame,
        inner,
        app.color_enabled,
        &built.clip_rows,
        scroll as usize,
    );
    draw_hscroll_affordance(frame, area, app);
}

/// Where to scroll so a search-selected entity is on screen (spec §10:
/// selecting a flag "closes the loop" by showing it).
///
/// Scroll math over **logical rows** (spec §9.3), which is what makes the
/// two edge cases below expressible at all — a row is a place in the
/// document with a height, not a line index:
///
/// - A row already wholly visible from the top scrolls **nothing**.
///   Scrolling a flag to the top of the pane when it was on screen anyway
///   throws away the `DESCRIPTION` and `USAGE` above it to no purpose.
/// - The offset is clamped to the document's own extent, so targeting the
///   last flag of a long list cannot scroll past the end into blank space.
///   That clamp is the same bound `App::set_detail_extent` puts on the
///   user's own scrolling, and this path used to bypass it — the
///   unbounded-detail-pane-scroll bug class, reached through search
///   instead of through `↓`.
fn target_scroll(built: &BuiltLines, first_line: usize, viewport: usize) -> usize {
    let row_end = built
        .rows
        .iter()
        .find(|r| r.first_line == first_line)
        .map_or(first_line + 1, |r| r.first_line + r.lines);
    if row_end <= viewport {
        return 0;
    }
    let max = built.lines.len().saturating_sub(viewport);
    first_line.min(max)
}

/// Draw the horizontal-scroll overflow affordance in the detail pane's
/// border — a single glyph on the top edge, the one row that already
/// legitimately carries something other than the plain rule character (the
/// breadcrumb title, spec §2) and so the one row `border_integrity.rs`
/// checks less strictly than the other three. Placed a couple of cells
/// inside the top-right corner, never
/// on it, so a very long breadcrumb can never push this into corner
/// territory and `border_integrity.rs`'s exact-corner-glyph assertion never
/// sees anything but the rounded/ASCII corner it expects.
///
/// A no-op with the config toggle off — `app.horizontal_scroll_enabled` is
/// exactly the same guard the scroll keys use (`App::detail_hscroll_left`
/// /`_right`), so "off" never draws a marker for an offset the user has no
/// way to have created.
///
/// **Deliberately drawn regardless of which pane has focus**, even though
/// `h`/`l`/`←`/`→` only reach [`App::detail_hscroll_left`]/`_right` while
/// `Focus::Detail` (`event::handle_detail_key`) — with the tree focused,
/// this can promise more content on a side no keypress currently reaches.
/// That was a conscious choice, not an oversight: the alternative is a pane
/// that silently clips a USAGE line or raw `--help` text with no sign
/// anything is missing until the reader happens to `Tab` over and press
/// `l`, and a wrong-but-honest "there's more, go focus this pane to see it"
/// is a smaller failure than that silent clipping — the same asymmetry
/// spec §9's border-corruption lesson already treats as the more dangerous
/// direction (content quietly doing something the reader can't see beats
/// content quietly *not* telling them there's more of it). Revisit this if
/// user feedback says the marker reads as broken rather than as "Tab over
/// for more" — the fix then is gating on `app.focus == Focus::Detail` here,
/// not changing what the marker itself draws.
fn draw_hscroll_affordance(frame: &mut Frame, area: Rect, app: &App) {
    if !app.horizontal_scroll_enabled || area.width < 6 || area.height == 0 {
        return;
    }
    let can_left = app.detail_hscroll_can_go_left();
    let can_right = app.detail_hscroll_can_go_right();
    if !can_left && !can_right {
        return;
    }
    let buf = frame.buffer_mut();
    let y = area.y;
    if can_right {
        buf[(area.x + area.width - 2, y)].set_char(app.glyphs.more_right);
    }
    if can_left {
        buf[(area.x + area.width - 3, y)].set_char(app.glyphs.more_left);
    }
}

/// Render the verbatim view (`t`): the tool's own `--help` output for the
/// selected node, whatever mandible made of it.
///
/// The three states are all rendered, not just the successful one. A view
/// whose purpose is "show me what you were actually given" cannot answer a
/// refused or failed probe with a blank pane, because blank is also what a
/// tool that prints nothing looks like, and telling those apart is the
/// entire reason someone pressed the key.
fn render_raw_mode(frame: &mut Frame, inner: Rect, app: &App, raw: &crate::app::RawHelp) {
    // Named from the argv actually run, never a hardcoded spelling — see
    // `RawHelp::Ready`. Only `Ready` knows it; the other two states have no
    // output to attribute, so they stay generic.
    let heading = match raw {
        crate::app::RawHelp::Ready(_, argv) => {
            format!("verbatim {} output of `{argv}`", app.glyphs.absent)
        }
        _ => format!("verbatim {} the tool's own help output", app.glyphs.absent),
    };
    match raw {
        crate::app::RawHelp::Pending => {
            render_verbatim(
                frame,
                inner,
                app,
                &heading,
                std::iter::once("running the probe…".to_string()),
            );
        }
        crate::app::RawHelp::Ready(lines, _) => {
            render_verbatim(
                frame,
                inner,
                app,
                &heading,
                lines.iter().map(|t| t.as_str().to_string()),
            );
        }
        crate::app::RawHelp::Failed(reason) => {
            render_verbatim(frame, inner, app, &heading, std::iter::once(reason.clone()));
        }
    }
}

/// Render preformatted text under a muted heading: the tool's own bytes,
/// never re-flowed.
///
/// Shared by the verbatim view (`t`) and by level-3 degradation, which want
/// the same treatment for the same reason and differ only in their label.
///
/// Originally written for a node whose parse degraded to level 3 (spec §7
/// Tier B step 3, batch 6 part 4): `node.unparsed`, one preformatted line
/// per entry, labelled so it reads as "the author's own text", not a
/// mandible parse.
///
/// Tool-authored body lines are deliberately **not** run through
/// [`wrap_words`] and are not given `Paragraph::wrap` the way every other
/// block in this pane is (see this
/// module's top doc comment on why the rest of the pane pre-wraps
/// everything itself) — this is preformatted output, and re-flowing it
/// would silently edit the tool author's own text. `h`/`l`/`←`/`→` scroll
/// it horizontally instead (spec §9: preformatted detail-pane content
/// scrolls rather than wraps); the important safety property — content
/// never reflows, and can therefore never smear into the pane border the
/// way an unsanitized newline once did (spec §9) — holds regardless of
/// which offset is showing.
///
/// With `[ui] horizontal_scroll` off there is no offset to move, so a line
/// wider than the pane goes to [`wrap_preformatted`] instead: it keeps
/// every column the author drew and continues the overflow on the next
/// row rather than dropping it. Off means *wrap*, never *clip* — this
/// pane is the one view whose job is showing the reader exactly what the
/// tool printed, and a pane that quietly ends a line at the border tells
/// them nothing is missing. Safe to hand
/// straight to a `Span` because both bodies reaching here were built by
/// `mandible_core::Text::sanitize_preserving_layout` (spec §4.1's layout
/// tier), one line at a time: the verbatim view's own lines in
/// `mandible-extract`'s `help_text::format_streams`, and `node.unparsed`
/// in that module's `verbatim_node`. It guarantees no control characters
/// and no embedded newline reach here, and it leaves indentation and
/// column alignment exactly as the tool printed them — which is the
/// point of both views, since a fallback that reformats the document it
/// is showing is no longer showing it. The heading and the recognizable unverified-subcommand notice
/// prefix are different: they are prose owned by mandible, so they wrap at the
/// current inner width while the tool lines after them stay untouched.
fn render_verbatim(
    frame: &mut Frame,
    inner: Rect,
    app: &App,
    heading: &str,
    body: impl Iterator<Item = String>,
) {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let width = inner.width as usize;
    for chunk in wrap_words(heading, width) {
        lines.push(Line::from(Span::styled(
            chunk,
            style::muted_bold(app.color_enabled),
        )));
    }
    lines.push(Line::default());
    let body: Vec<String> = body.collect();
    let wrapped_prefix_lines = unverified_notice_prefix_len(&body);

    // Everything past the mandible-authored notice prefix is the tool's
    // own preformatted bytes (see this function's doc comment above) —
    // exactly the content spec §9 wants scrolled rather than wrapped. The
    // notice prefix itself stays prose: it's mandible's own text, already
    // wrapped by `push_wrapped_notice`, and immune to the horizontal
    // offset the same way DESCRIPTION/FLAGS stay immune in the structured
    // view below.
    let hoffset = if app.horizontal_scroll_enabled {
        let max_width = body
            .iter()
            .skip(wrapped_prefix_lines)
            .map(|line| display_width(line))
            .max()
            .unwrap_or(0);
        app.set_detail_hextent(max_width, width);
        app.clamped_detail_hscroll()
    } else {
        0
    };

    let mut clip_rows: Vec<(usize, bool, bool)> = Vec::new();
    for (index, text) in body.into_iter().enumerate() {
        if index < wrapped_prefix_lines {
            push_wrapped_notice(&mut lines, &text, width);
        } else if app.horizontal_scroll_enabled {
            let (line, left, right) = hscroll_line(&text, hoffset, width);
            if left || right {
                clip_rows.push((lines.len(), left, right));
            }
            lines.push(line);
        } else {
            for row in wrap_preformatted(&text, width) {
                lines.push(Line::from(row));
            }
        }
    }
    app.set_detail_extent(lines.len(), inner.height as usize);
    let scroll = app.clamped_detail_scroll();
    let paragraph = Paragraph::new(lines).scroll((scroll as u16, 0));
    frame.render_widget(paragraph, inner);
    draw_clip_marker_rails(frame, inner, app.color_enabled, &clip_rows, scroll);
}

/// The visible window of `s` for horizontal scrolling of preformatted
/// content: `offset` display-width columns trimmed off the left, then
/// capped to at most `width` columns of what remains.
///
/// The cap matters as much as the trim. The structured detail pane's
/// `Paragraph` keeps `Wrap` enabled as a defensive fallback (this module's
/// top doc comment) for content this function's caller does *not* produce —
/// every other line here is already pre-wrapped to fit. A preformatted
/// line handed over wider than `width` is exactly the shape `Wrap` exists
/// to catch, so without the cap it silently re-wraps a synopsis this
/// feature deliberately stopped wrapping, restarting the continuation flush
/// left with no memory of the line's own indent — precisely the reflowed,
/// meaning-scrambling failure spec §9 introduced this feature to remove,
/// just reintroduced one layer up. Found by rendering `ip` through a real pty
/// rather than trusting `TestBackend` (AGENTS.md §3.2): a synthetic fixture
/// narrow enough to need the cap was never in the corpus.
///
/// Character-by-character, never a raw byte slice (AGENTS.md: never slice
/// tool-derived text at a byte offset — this project has shipped a
/// UTF-8-boundary panic from exactly that shortcut once already). A
/// double-width character that straddles either boundary is dropped whole
/// rather than split — splitting it would emit half a cell and misalign
/// every column after it, which is worse than losing one character's width
/// of the scroll.
/// The visible window of one preformatted line at a horizontal offset,
/// plus whether content was clipped off each edge. The flags feed
/// [`draw_clip_marker_rails`], which draws vim-style `<`/`>` markers
/// (`listchars extends:>,precedes:<`) — in the pane's one-column padding
/// gutter against the border, NOT inside the text: the maintainer's call,
/// so the marker reads as a rail on the wall and the text keeps its full
/// width and natural columns. A line that ends exactly at the edge hides
/// nothing and reports no clip; a line entirely behind the offset renders
/// empty with no stray left marker.
fn hscroll_line(s: &str, offset: usize, width: usize) -> (Line<'static>, bool, bool) {
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

/// Draw the per-line clip markers into the padding gutters either side of
/// `inner` — the one blank column `Block::padding` leaves between border
/// and content. `clip_rows` holds `(line index, clipped_left,
/// clipped_right)` in the same index space the paragraph's vertical
/// `scroll` uses, so a marker follows its line when the pane scrolls
/// vertically and disappears with it off-screen.
fn draw_clip_marker_rails(
    frame: &mut Frame,
    inner: Rect,
    color_enabled: bool,
    clip_rows: &[(usize, bool, bool)],
    vscroll: usize,
) {
    if inner.x == 0 || clip_rows.is_empty() {
        return;
    }
    let left_x = inner.x - 1;
    let right_x = inner.x + inner.width;
    let marker = style::muted(color_enabled);
    let buf = frame.buffer_mut();
    for &(idx, left, right) in clip_rows {
        let Some(row) = idx.checked_sub(vscroll) else {
            continue;
        };
        if row >= inner.height as usize {
            continue;
        }
        let y = inner.y + row as u16;
        if left {
            buf[(left_x, y)].set_symbol("<").set_style(marker);
        }
        if right {
            buf[(right_x, y)].set_symbol(">").set_style(marker);
        }
    }
}

/// Wrap one preformatted line to `width` display columns, losing nothing.
///
/// The wrap-mode counterpart of [`hscroll_line`]: with `[ui]
/// horizontal_scroll` off there is no horizontal offset for the reader to
/// move, so a line wider than the pane has to arrive on more than one row
/// or it does not arrive at all. It used to arrive on exactly one, and the
/// `Paragraph` this pane's verbatim path builds carries no `Wrap` (that is
/// the whole point of the scrolling path) — so everything past the pane's
/// last column was silently dropped, in the one view whose purpose is
/// showing the reader what the tool actually printed.
///
/// Not [`wrap_words`], which is prose wrapping: it splits on whitespace
/// and rejoins with single spaces, so `ar`'s padded command table would
/// come back as `d - delete file(s) from the archive` with the column the
/// author aligned on collapsed away — the exact reformatting spec §4.1's
/// layout tier exists to stop. Here instead:
///
/// - a line that fits is returned byte-identical, which is every line of
///   most tools' help output;
/// - an over-wide line is cut at a whitespace boundary when there is one
///   in the window and hard-cut between characters when there is not, so a
///   single unbroken 5,000-column token still survives whole;
/// - each cut keeps the text either side of it exactly as written —
///   interior runs of spaces inside a row are never touched;
/// - continuation rows carry the line's own leading indent, so a wrapped
///   table row stays visibly part of that row rather than drifting to
///   column 0. The indent is dropped when it would take half the pane or
///   more, since a 38-column indent in a 40-column pane turns one line
///   into a tall column of two-character rows.
///
/// Character-by-character, never a raw byte slice at a computed column
/// (AGENTS.md's byte-slicing rule) — [`width_prefix_end`] only ever
/// returns a real `char` boundary.
fn wrap_preformatted(line: &str, width: usize) -> Vec<String> {
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
fn width_prefix_end(s: &str, width: usize) -> usize {
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

fn hscroll_window(s: &str, offset: usize, width: usize) -> std::borrow::Cow<'_, str> {
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

/// Recognize only the display-only prose prepended by `not_attested_fallback`.
///
/// The first sentence is always present. A successful safe root-help fallback
/// adds a blank, mandible's explanatory label, and another blank before the
/// tool-authored bytes. Keeping this recognition here avoids changing the raw
/// help API or tree model solely to carry presentation metadata.
fn unverified_notice_prefix_len(body: &[String]) -> usize {
    use mandible_core::notice::{ROOT_HELP_FALLBACK_LABEL, UNVERIFIED_SUBCOMMAND_NOTICE_PREFIX};

    if !body
        .first()
        .is_some_and(|line| line.starts_with(UNVERIFIED_SUBCOMMAND_NOTICE_PREFIX))
    {
        return 0;
    }

    if body.get(1).is_some_and(String::is_empty)
        && body
            .get(2)
            .is_some_and(|line| line == ROOT_HELP_FALLBACK_LABEL)
        && body.get(3).is_some_and(String::is_empty)
    {
        4
    } else {
        1
    }
}

/// Add one mandible-authored notice paragraph with a stable block indent.
///
/// The raw-help body after this prefix is intentionally preformatted; only
/// prose created by mandible takes this path. [`wrap_words`] supplies the
/// display-width-aware long-token splitting used by the structured detail
/// pane, so CJK and emoji cannot turn a character count into a border overrun.
fn push_wrapped_notice(lines: &mut Vec<Line<'static>>, text: &str, width: usize) {
    if text.is_empty() {
        lines.push(Line::default());
        return;
    }

    const INDENT: &str = "  ";
    let indent = if width > display_width(INDENT) {
        INDENT
    } else {
        ""
    };
    let available = width.saturating_sub(display_width(indent)).max(1);
    for chunk in wrap_words(text, available) {
        lines.push(Line::from(format!("{indent}{chunk}")));
    }
}

/// One rendered entity, as **one logical row** however many screen lines
/// its description wrapped onto (spec §9.3).
///
/// This is the type that keeps the distinction honest. Selection addresses
/// `first_line` — never a continuation line — and the scroll extent is
/// taken from the *rendered* line count, never from a count of rows, so
/// neither number can drift into the other. Conflating them is the
/// unbounded-detail-pane-scroll bug class: a pane that scrolls in rows
/// while it renders in lines runs off the end of its own content by
/// exactly the number of wraps on screen, and
/// `a_wrapped_entry_is_one_logical_row` pins it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EntryRow {
    /// Index of the row's first line in [`BuiltLines::lines`].
    first_line: usize,
    /// How many lines the row occupies, always at least one.
    lines: usize,
}

/// The rendered detail-pane content plus where a search-targeted flag
/// landed, if any.
struct BuiltLines {
    lines: Vec<Line<'static>>,
    /// The line index [`Flag`] `app.selected_flag` starts at, if it was
    /// found on `node`.
    target_flag_line: Option<usize>,
    /// `(line index, clipped_left, clipped_right)` for each horizontally
    /// clipped preformatted line, consumed by [`draw_clip_marker_rails`].
    clip_rows: Vec<(usize, bool, bool)>,
    /// One entry per rendered entity, in document order — see [`EntryRow`].
    rows: Vec<EntryRow>,
}

/// The list sections of spec §9.3, in render order: the [`EntityKind`]
/// each holds, its heading, and the indent its rows are inset by.
///
/// The whole of the per-kind knowledge in this pane, and it is *data*.
/// There is no branch on kind anywhere below: a section renders because its
/// kind has entities, and every kind goes through exactly the same code.
/// `Modifier` and `EnvVar` are what that claim bought: the parser began
/// emitting modifier letters (spec §7 Tier B, "Modifier tables") and later
/// environment variables (spec §7 Tier B, "Environment sections"), and this
/// pane rendered a MODIFIERS section for `ar`/`llvm-ar` and an ENVIRONMENT
/// section for `bpftrace`/`node`/`fzf` with no change to a line of it.
/// `DESCRIPTION` and `USAGE` are not here because they are node prose, not
/// entity lists — they carry no count and take no shared column.
/// POSITIONALS is the one section carrying an indent (spec §9.3). Its rows
/// are bare names with no dashes to start them, so at the content edge a
/// run of them reads as loose text against the pane border rather than as a
/// list; the flag-shaped sections keep the edge, where the short and long
/// columns are structure the eye follows down the section. The indent is
/// [`POSITIONAL_INDENT`], a number of its own rather than a share of the
/// flag columns.
const LIST_SECTIONS: [(EntityKind, &str, usize); 4] = [
    (EntityKind::Positional, "POSITIONALS", POSITIONAL_INDENT),
    (EntityKind::Flag, "FLAGS", 0),
    (EntityKind::Modifier, "MODIFIERS", 0),
    (EntityKind::EnvVar, "ENVIRONMENT", 0),
];

fn build_lines(
    node: &CommandNode,
    show_hidden: bool,
    width: usize,
    palette: style::Palette,
    target_flag: Option<&FlagKey>,
    glyphs: Glyphs,
    app: &App,
) -> BuiltLines {
    let mut lines = Vec::new();
    let mut target_flag_line = None;
    let mut clip_rows: Vec<(usize, bool, bool)> = Vec::new();
    let mut rows: Vec<EntryRow> = Vec::new();
    // Reset every frame, not just when a USAGE section is present below —
    // otherwise a node with no USAGE at all would leave the previous
    // node's extent sitting in the `Cell`, and the overflow affordance
    // would draw for content that isn't even on screen anymore.
    if app.horizontal_scroll_enabled {
        app.set_detail_hextent(0, width);
    }

    if let Some(summary) = &node.summary {
        for chunk in wrap_words(summary.as_str(), width) {
            lines.push(Line::from(Span::styled(
                chunk,
                Style::default().add_modifier(ratatui::style::Modifier::BOLD),
            )));
        }
    }

    if let Some(description) = &node.description {
        open_block(&mut lines, SECTION_BLANKS);
        lines.push(heading_line_ruled(
            "DESCRIPTION",
            None,
            width,
            palette,
            glyphs,
        ));
        // One blank row *between* paragraphs and none after the last: the
        // separator that closes the section belongs to whatever section
        // opens next, and is `open_section`'s to place. An empty paragraph
        // — what a source `\n\n\n\n` splits into — is skipped rather than
        // rendered as a second blank row.
        let mut first = true;
        for paragraph_text in description.as_str().split("\n\n") {
            if paragraph_text.trim().is_empty() {
                continue;
            }
            if !first {
                lines.push(Line::default());
            }
            first = false;
            // A single `\n` inside a paragraph is structure the author
            // marked and `Text::sanitize` kept — a bullet, an indented
            // row, an example invocation (spec §4.1). Flowing prose has
            // already been joined into one logical line by that point, so
            // every break here is deliberate and each logical line is
            // wrapped on its own, at its own indent, rather than being
            // fed to one `split_whitespace` that would erase it. Wrapping
            // the whole paragraph as one string is what put `grep`'s
            // `Example:` line back in the middle of the sentence after it,
            // after the IR had correctly separated them.
            for logical in paragraph_text.split('\n') {
                let indent = logical.len() - logical.trim_start().len();
                let indent_str = " ".repeat(indent);
                let avail = width.saturating_sub(indent).max(1);
                for chunk in wrap_words(logical, avail) {
                    lines.push(Line::from(format!("{indent_str}{chunk}")));
                }
            }
        }
    }

    if !node.usage.is_empty() {
        open_block(&mut lines, SECTION_BLANKS);
        lines.push(heading_line_ruled("USAGE", None, width, palette, glyphs));
        // Indented as a block, the way API documentation sets a signature
        // apart from its prose.
        let indent = "  ";
        let forms = usage_forms(&node.name, &node.usage);
        if app.horizontal_scroll_enabled {
            // A synopsis is preformatted — spacing inside it is part of
            // its meaning, so spec §9 has it scroll rather than wrap. One
            // `Line` per usage form, never re-flowed; `h`/`l` reveal the rest instead
            // of the old greedy word-wrap eating it into a ragged block.
            let usage_lines: Vec<String> = forms
                .iter()
                .map(|(pad, text)| format!("{indent}{}{text}", " ".repeat(*pad)))
                .collect();
            let max_width = usage_lines
                .iter()
                .map(|line| display_width(line))
                .max()
                .unwrap_or(0);
            app.set_detail_hextent(max_width, width);
            let hoffset = app.clamped_detail_hscroll();
            for line in usage_lines {
                let (built, left, right) = hscroll_line(&line, hoffset, width);
                if left || right {
                    clip_rows.push((lines.len(), left, right));
                }
                lines.push(built);
            }
        } else {
            // Wrapping mode keeps the same left edge for a form's own
            // continuation rows, so a form that has to wrap still reads as
            // one form rather than drifting back to the block indent.
            for (pad, text) in &forms {
                let lead = format!("{indent}{}", " ".repeat(*pad));
                let avail = width.saturating_sub(display_width(&lead)).max(1);
                for chunk in wrap_words(text, avail) {
                    lines.push(Line::from(format!("{lead}{chunk}")));
                }
            }
        }
    }

    // The four list sections, in spec §9.3's order, from one loop. An
    // empty section renders nothing at all — not a heading over blank
    // space — which is what keeps a tool with only a description and flags
    // looking exactly as it did before this section model existed.
    for (kind, label, indent) in LIST_SECTIONS {
        let visible: Vec<&Entity> = node
            .entities_of(kind)
            .filter(|e| show_hidden || (!e.hidden && e.deprecated.is_none()))
            .collect();
        if visible.is_empty() {
            continue;
        }
        open_block(&mut lines, SECTION_BLANKS);
        lines.push(heading_line_ruled(
            label,
            Some(visible.len()),
            width,
            palette,
            glyphs,
        ));
        let base = lines.len();
        let section = section_lines(&visible, width, indent, palette, target_flag, glyphs);
        if target_flag_line.is_none() {
            target_flag_line = section.target.map(|t| base + t);
        }
        rows.extend(section.rows.iter().map(|r| EntryRow {
            first_line: base + r.first_line,
            lines: r.lines,
        }));
        lines.extend(section.lines);
    }

    // Provenance is not rendered here at all any more: it describes where
    // this node's data came from, which belongs beside the pane rather than
    // inside its content. See `render::status_bar`.

    BuiltLines {
        lines,
        target_flag_line,
        clip_rows,
        rows,
    }
}

/// Blank rows above a section header: two, because a section is the
/// pane's outer level and reads as a chapter (spec §9.3).
const SECTION_BLANKS: usize = 2;

/// Blank rows above a ruled group divider: one. A group is a subdivision
/// of the section containing it, so it gets less air than the section
/// does — the gap sizes are the container hierarchy made visible, which
/// is why they are two constants and not one number used twice.
const GROUP_BLANKS: usize = 1;

/// Put exactly `blanks` blank rows between whatever is already on the page
/// and the heading about to be pushed (spec §9.3) — never fewer, never
/// more, and never any at the very top of the document.
///
/// The separator belongs to the block that *opens*, not to the one that
/// closes. Every section used to push its own trailing blank instead,
/// which made the boundary the sum of two independent decisions: a section
/// that wrapped its last row, ended on a group, or ran a paragraph split
/// out into an empty trailer contributed a different number of blanks from
/// its neighbour, and the page's rhythm changed with the content. One
/// caller per level, one rule, and whatever blank rows the block above
/// left behind are absorbed here rather than counted on — which is what
/// makes the count exact rather than merely a minimum.
///
/// Nothing is pushed *below* a heading: the header's or divider's own rule
/// is already a horizontal line separating it from its rows, and a blank
/// under it would set the label adrift from the list it names.
fn open_block(lines: &mut Vec<Line<'static>>, blanks: usize) {
    while lines.last().is_some_and(line_is_blank) {
        lines.pop();
    }
    if lines.is_empty() {
        return;
    }
    for _ in 0..blanks {
        lines.push(Line::default());
    }
}

/// Whether a built line would render as an empty row: no spans, or nothing
/// but whitespace in them.
fn line_is_blank(line: &Line<'static>) -> bool {
    line.spans.iter().all(|s| s.content.trim().is_empty())
}

/// A section heading followed by a rule to the pane's edge, with the
/// section's entity count for the list sections: `FLAGS (41)`.
///
/// The rule is what gives the pane hierarchy: without it, a word and the
/// body text beneath it are two lines of similar weight, and the eye has
/// nothing to anchor a section boundary to. Label and rule alike are drawn
/// in [`style::section_rule`] — the middle of the pane's three neutral
/// steps, a clear step below the borders around it and a clear step above
/// the group divider that subdivides it — and the rule goes through the
/// glyph set so a non-UTF-8 terminal gets `-` rather than tofu.
///
/// Shape, not styling, is what separates this from a group divider
/// ([`group_divider_line`]): both are label-first with a rule running to
/// the pane's edge, and this one is CAPS with a count against the
/// divider's mixed case without one. Spec §9.2 forbids making an
/// attribute the sole distinction between two kinds of text, because
/// several terminals ignore attributes — so the two must still read
/// differently with every one stripped, and they do.
fn heading_line_ruled(
    text: &str,
    count: Option<usize>,
    width: usize,
    palette: style::Palette,
    glyphs: Glyphs,
) -> Line<'static> {
    let heading = match count {
        Some(n) => format!("{text} ({n})"),
        None => text.to_string(),
    };
    let used = display_width(&heading) + 1;
    let rule_width = width.saturating_sub(used);
    // Label and rule are one piece of furniture, so the label is drawn in
    // exactly the rule's style — same color, and no bold, which brightens
    // a foreground on many terminals and would recreate the mismatch
    // through an attribute rather than a color. CAPS and the count are
    // what mark this out as the outer level, not weight.
    let shade = style::section_rule(palette);
    let mut spans = vec![Span::styled(heading, shade)];
    if rule_width > 0 {
        spans.push(Span::styled(" ", shade));
        spans.push(Span::styled(glyphs.rule.repeat(rule_width), shade));
    }
    Line::from(spans)
}

/// One usage line, with the redundancy stripped.
///
/// The raw string frequently already carries both a `Usage:` label and the
/// tool's own command path — `tar --help` yields `Usage: tar [OPTION...]`,
/// and prepending the node name to that produced `tar Usage: tar
/// [OPTION...]`, with the name twice and a label the `USAGE` heading
/// directly above already supplies.
///
/// The old guard only checked the usage text's *first* word, which is why
/// `docker import --help`'s `Usage:  docker import [OPTIONS] file|URL|-
/// [REPOSITORY[:TAG]]` rendered as `import docker import [OPTIONS]
/// file|URL|- [REPOSITORY[:TAG]]`: cobra prints the *full* command path
/// (`docker import`), not just the leaf name, so the first word is
/// `docker` and the check missed. `smokecli columns outlier` (argparse,
/// which does the same thing) has the identical shape: `usage: smokecli
/// columns outlier [-h] ...`.
///
/// So the check now scans the whole run of bare, word-shaped tokens at the
/// front of the usage text — stopping at the first token that looks like
/// an option or placeholder (`-...`, `[...`, `<...`, or a bare ALL-CAPS
/// metavar like `FILE`) — and prepends the name only when it is absent
/// from that whole run, not just its first entry. That run *is* the
/// tool's own command-path prefix; if the node's name shows up anywhere in
/// it the line already names the command. Tools that print no command name
/// at all still work: `Usage: [OPTIONS] FILE` has an empty leading run (the
/// very first token is a placeholder), so nothing is found there and the
/// name still gets prepended — which is what keeps a bare pattern like
/// `[OPTIONS] <url>` a complete, copy-pasteable invocation.
/// One usage form, as the column its text began at in the tool's own
/// output and the text itself.
///
/// The column is the form's leading indentation plus whatever a `Usage:`
/// label occupied in front of it, because both are width the author put
/// there and only one of them survives into the rendered line. It is what
/// [`usage_forms`] compensates with.
fn usage_form(node_name: &str, usage: &str) -> (usize, String) {
    let name = defensive_single_line(node_name);
    let raw = defensive_single_line(usage);

    // The author's own indentation. Tabs are already expanded to spaces at
    // 8-column stops by `Text::sanitize_preserving_layout`, so counting
    // leading spaces is counting columns.
    let mut text = raw.trim_start_matches(' ').to_string();
    let mut column = raw.chars().count() - text.chars().count();

    // Drop a leading `usage:` label, case-insensitively — the heading says
    // it — and charge its width to the column, since the text that
    // followed it started that far in.
    if text.len() >= 6 && text[..6].eq_ignore_ascii_case("usage:") {
        let after = text[6..].trim_start().to_string();
        column += text.chars().count() - after.chars().count();
        text = after;
    }

    let text = if name.is_empty() || usage_names_the_node(&text, &name) {
        text
    } else {
        format!("{name} {text}")
    };
    (column, text)
}

/// Every usage form, each as the padding it renders behind and its text —
/// the tool's own alignment, compensated for the label the first form no
/// longer shows (spec §4.1).
///
/// A tool draws its alternative invocations lined up under each other, and
/// it lines them up against the `Usage: ` label it printed in front of the
/// first one:
///
/// ```text
/// Usage: ip [ OPTIONS ] OBJECT { COMMAND | help }
///        ip [ -force ] -batch filename
/// ```
///
/// The `USAGE` heading already says "usage", so the label is dropped — and
/// dropping it moves the first form seven columns left while the second
/// stays where the author put it, which is worse than not preserving the
/// indentation at all. So every form shifts left by the first form's own
/// content column: form one lands at the block indent, and the rest keep
/// their positions *relative to it*, which is the alignment the author
/// actually drew.
///
/// A form indented less than that shift — `du`'s `  or:  du ...`, whose
/// two columns are fewer than the seven `Usage: ` occupied — clamps at the
/// block indent rather than going negative. It cannot be aligned as drawn
/// once the label it was drawn against is gone, and the honest fallback is
/// the left edge.
fn usage_forms(node_name: &str, usage: &[Text]) -> Vec<(usize, String)> {
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

/// Whether `name` already appears among `text`'s leading run of bare
/// command-path words — see [`usage_form`] for why the search covers
/// the whole run rather than only the first token.
fn usage_names_the_node(text: &str, name: &str) -> bool {
    text.split_whitespace()
        .take_while(|word| !looks_like_option_or_placeholder(word))
        .any(|word| word == name)
}

/// A token that ends a usage line's leading command-path run: an option
/// (`-v`, `--verbose`), a bracketed/angled placeholder (`[OPTIONS]`,
/// `<url>`), or a bare ALL-CAPS metavar (`FILE`, `URL`) — docopt-style
/// convention for "this is a slot to fill in", never a literal word of the
/// command path.
fn looks_like_option_or_placeholder(word: &str) -> bool {
    if word.starts_with(['-', '[', '<']) {
        return true;
    }
    let has_letter = word.chars().any(|c| c.is_alphabetic());
    has_letter && !word.chars().any(|c| c.is_lowercase())
}

/// Greedy word-wrap of `text` to at most `width` display columns per
/// line, never breaking a word unless it alone exceeds `width` — in which
/// case it is broken across as many lines as it takes (see
/// [`break_overlong_word`]) rather than truncated. A token that is lost
/// once truncated is unrecoverable from the parsed view: `smokecli
/// unbreakable url` prints a ~150-character URL that used to render as
/// `https://registry.example.com/v2/org…` in a 46-column pane, with
/// everything past `/v2/org` gone. Always returns at least one (possibly
/// empty) chunk.
fn wrap_words(text: &str, width: usize) -> Vec<String> {
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

/// Break a single token wider than `width` display columns into as many
/// width-limited chunks as it takes, so the token survives intact across
/// multiple lines instead of being lost to an ellipsis truncation.
///
/// Splits are placed between characters, chosen by summing each
/// character's [`unicode_width`] — never by byte index (a raw byte offset
/// can land mid-character and panic, the exact failure AGENTS.md's
/// byte-slicing rule documents for parsed tool output) and never by
/// `char` count (a `char`-count split can put a double-width CJK or emoji
/// character right at the boundary and let it overflow the line by one
/// cell, the same border-overflow failure display-width truncation exists
/// to prevent elsewhere in this pane). A lone character wider than `width`
/// itself (a 2-wide emoji in a 1-column budget) still cannot be split —
/// it gets its own chunk and that chunk is allowed to exceed `width` by
/// the unavoidable minimum, since no cut point inside a character exists.
fn break_overlong_word(word: &str, width: usize) -> Vec<String> {
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

/// A group heading with its source's punctuation stripped: single-lined,
/// trimmed, and without the trailing terminator a help-text heading
/// carries — the colon of `"GLOBAL OPTIONS:"`, and the full stop of a
/// group whose label is a whole sentence the tool wrote
/// (`"Start the lockspace of a shared VG in lvmlockd."`, LVM's own
/// per-stanza description, which spec §7 Tier B makes that stanza's
/// group label).
///
/// A divider is furniture: its label runs straight into the rule beside
/// it, and a terminator stranded between the two reads as a mark on the
/// line rather than as the end of a sentence — the same reason the colon
/// goes. One trailing stop only, and never an ellipsis, which is docopt
/// repetition notation rather than punctuation (a group named
/// `"FILE..."` keeps its meaning).
///
/// The casing is left exactly as the tool wrote it — that is
/// [`group_label`]'s and [`group_key`]'s business, and they want different
/// answers.
fn strip_group_punctuation(raw: &str) -> String {
    let single = defensive_single_line(raw);
    let trimmed = single.trim().trim_end_matches(':').trim();
    let unterminated = match trimmed.strip_suffix('.') {
        Some(head) if !trimmed.ends_with("...") => head.trim_end(),
        _ => trimmed,
    };
    unterminated.to_string()
}

/// The identity a group is collected under: case-folded, so that
/// carapace-sourced groups (already often plain, e.g. `"main"`) and
/// help-text-sourced ones (raw heading text, e.g. `"GLOBAL OPTIONS:"`)
/// collapse into one group rather than rendering the same logical grouping
/// twice under two spellings of its name.
fn group_key(raw: &str) -> String {
    strip_group_punctuation(raw).to_uppercase()
}

/// The label a group divider displays: **mixed case, never CAPS**
/// (spec §9.3).
///
/// CAPS is the section header's shape, and a group that also shouted would
/// be indistinguishable from one on a terminal that drops the dimming
/// (spec §9.2). So a heading the tool wrote in screaming caps
/// (`"GLOBAL OPTIONS"`) is set in sentence case, while one that already
/// carries the author's own casing (`"Main operation mode"`) keeps it —
/// mixed case is information when the author chose it and noise when the
/// help-text format imposed it. Either way the first character is
/// capitalized, so `"main"` and `"Main"` render alike.
fn group_label(raw: &str) -> String {
    let stripped = strip_group_punctuation(raw);
    let shouted = !stripped.chars().any(char::is_lowercase);
    let mut label = String::with_capacity(stripped.len());
    for (i, c) in stripped.chars().enumerate() {
        if i == 0 {
            label.extend(c.to_uppercase());
        } else if shouted {
            label.extend(c.to_lowercase());
        } else {
            label.push(c);
        }
    }
    label
}

/// A group divider within a section (spec §9.3): the group's label at
/// column 0 with a rule running from it to the pane's edge,
/// `Operation ──────…`.
///
/// The rows beneath it stay at the section's normal margin — grouping is
/// drawn, never indented, so it costs no width.
///
/// Label-first, like the section header above it ([`heading_line_ruled`]).
/// The divider used to open with a single rule cell before its label, on
/// the theory that leading with the rule marked it as subordinate — but a
/// one-cell stub of a line is not a level, it is a decoration, and it cost
/// the pane its one straight left edge: every heading, every divider and
/// every ungrouped row starts at column 0, so the eye reads the document
/// down one margin. What actually separates the two levels is the shade of
/// the rule and the shape of the label — CAPS with a count against mixed
/// case without one — neither of which needs a cell of furniture in front
/// of the words to carry it.
///
/// Rule and label are both drawn in [`style::group_rule`], the dimmest of
/// the pane's three neutral steps and a clear step below the section
/// header's [`style::section_rule`], so a divider reads as subordinate to
/// the header above it rather than as its equal. The two spans are one
/// style because they are one piece of furniture: a label in a different
/// shade from the line running out of it reads as two unrelated marks
/// that happen to share a row. The difference in weight belongs between
/// the levels, never inside one of them — which is also why no label
/// anywhere in this pane is bold.
///
/// `ruled` is false for a divider that opens its section — see
/// [`group_divider_lead_line`], which is what that case renders instead.
///
/// The label is tool-authored text of unbounded length, so it is truncated
/// to the pane rather than trusted to fit — a divider that wrapped would
/// stop being a rule, and one that overflowed would reach the border (spec
/// §9's border-corruption lesson).
fn group_divider_line(
    label: &str,
    width: usize,
    palette: style::Palette,
    glyphs: Glyphs,
    ruled: bool,
) -> Line<'static> {
    let shade = style::group_rule(palette);
    if !ruled {
        return group_divider_lead_line(label, width, palette, glyphs);
    }
    // A space behind the label and at least one rule cell after it: the
    // budget the label has to fit inside.
    let furniture = display_width(glyphs.rule) + 1;
    let label = truncate_to_width_marker(label, width.saturating_sub(furniture), glyphs.ellipsis);
    let rule_width = width.saturating_sub(display_width(&label) + 1);
    let mut spans = vec![Span::styled(label, shade)];
    if rule_width > 0 {
        spans.push(Span::styled(" ", shade));
        spans.push(Span::styled(glyphs.rule.repeat(rule_width), shade));
    }
    Line::from(spans)
}

/// The divider that **opens** a section: its label alone, at column 0,
/// with no rule at all (spec §9.3).
///
/// A section header already draws a full-width rule, and a ruled divider
/// on the very next line draws a second one directly beneath it. Two
/// full-width rules one above the other read as a single doubled line —
/// the header's own rule stops looking like a boundary and the group's
/// stops looking like a subdivision of it. The header's rule is the
/// boundary; the group only needs to be named, and naming it at column 0
/// under a heading that also starts at column 0 is what a sub-heading
/// looks like.
///
/// Distinguishable from the section header with every attribute stripped
/// (spec §9.2): the header is CAPS with a count and a rule running to the
/// pane's edge, this is mixed case with neither.
fn group_divider_lead_line(
    label: &str,
    width: usize,
    palette: style::Palette,
    glyphs: Glyphs,
) -> Line<'static> {
    let label = truncate_to_width_marker(label, width, glyphs.ellipsis);
    Line::from(Span::styled(label, style::group_rule(palette)))
}

/// A spelling wider than this fraction of the pane does not get to set the
/// shared column — it runs on past it instead, pushing its own first
/// description line and nothing else (see [`SectionLayout`]). One
/// 40-character flag name in a list of short ones used to push every
/// description in the list against the right-hand edge. Mirrors the tree
/// pane's summary-column rule (spec §9.1).
const DESC_COLUMN_CAP_PERCENT: usize = 45;

/// The share of a section's entities the shared column is fitted to
/// (spec §9.3: "roughly the p90 spelling width — the majority, not the
/// outliers").
///
/// Not the maximum. A column fitted to the widest spelling in the section
/// is a column one entity chose for every other one, and the wider that
/// entity is the less room the rest of the section's prose gets. Fitting
/// the ninetieth percentile spends one extra line on the widest tenth and
/// gives the width back to the other nine.
const SHARED_COLUMN_PERCENTILE: usize = 90;

/// The narrowest a description is allowed to be. A section's shared column
/// is clamped down until this much of the pane is left for prose, however
/// wide the section's heads are (spec §9.3).
///
/// Measured against real output rather than picked: at 20 columns
/// `docker pull`'s `--platform` description breaks as "Set / platform /
/// if server / is / multi-pla… / capable" — six lines, one of them
/// truncated mid-word, for six words of text. At 28 the same description
/// reads as prose. In a 90-column terminal the detail pane is 41 columns
/// wide, which puts the clamp at column 13 — enough for a short-and-long
/// pair, with wider heads pushing their own first line right.
///
/// Clamping the column is not the same as letting a wide head clamp its
/// own description: the column moves for the whole section, so every
/// description in it still begins in the same place.
const MIN_DESC_WIDTH: usize = 28;

/// Where a short spelling starts: the true left edge of the content area
/// (spec §9.3). There is no uniform margin on a list section — the row's
/// own shape decides which of the two columns it starts at.
const SHORT_COLUMN: usize = 0;

/// Where a long spelling starts, whether or not a short precedes it
/// (spec §9.3): the display width of a short prefix, `-X, `.
///
/// A row that has a short renders it at [`SHORT_COLUMN`] and its long
/// lands here by arithmetic; a row with no short is preindented to the
/// same place. That is the whole point — the eye follows the longs down
/// one column without having to know which rows happen to have a short
/// letter as well.
const LONG_COLUMN: usize = "-X, ".len();

/// The indent POSITIONALS rows are inset by (spec §9.3).
///
/// Two columns: enough to set a loose list of bare names in from the pane's
/// edge, and not so much that it costs the descriptions width. Its own
/// number, deliberately **not** [`LONG_COLUMN`] — the inset exists to keep
/// a run of dashless names off the pane's border, which is a question about
/// this section alone, and tying it to the flag columns would couple two
/// layouts that have no reason to move together.
///
/// MODIFIERS and ENVIRONMENT are bare-name sections too, but they are laid
/// out like FLAGS — one tight list against the content edge — and stay
/// there.
const POSITIONAL_INDENT: usize = 2;

/// The column an entity's spellings start at within a section indented by
/// `indent` (spec §9.3).
///
/// Shape decides it, never kind and never the section: a row whose first
/// documented spelling is a short (or a dashless name — a positional, a
/// modifier letter, a variable) starts at the content edge, and a row that
/// has only long spellings is preindented so its first long lands in the
/// same column a short row's long does.
///
/// A row documenting more than two spellings (`-h, -?, -help, --help`)
/// flows from the short column whatever its first spelling is. It is the
/// natural exception: there is no single "the long" in such a row to align,
/// so preindenting it would push a list of names right for no column, and
/// its length already marks it out.
fn spelling_column(entity: &Entity, indent: usize) -> usize {
    indent + bare_spelling_column(entity)
}

/// [`spelling_column`] before the section's own indent is added.
fn bare_spelling_column(entity: &Entity) -> usize {
    if entity.spellings.len() > 2 {
        return SHORT_COLUMN;
    }
    if entity
        .spellings
        .iter()
        .any(|s| matches!(s.dashes, Dashes::None))
    {
        return SHORT_COLUMN;
    }
    if entity.short_spelling().is_some() {
        return SHORT_COLUMN;
    }
    LONG_COLUMN
}

/// How a whole section is arranged. Chosen once **per section**, never per
/// row — a per-row decision is exactly what made this ragged.
///
/// Per section rather than per pane (spec §9.3): positionals, flags,
/// modifiers and environment variables have nothing to say to each other's
/// widths, and one column shared across all four would be set by whichever
/// section happens to hold the longest name.
///
/// The pane is not wide enough for a three-column table at every terminal
/// size, and the previous code did not admit that. It computed one shared
/// description column, capped it at 45% of the pane, and then let any row
/// too wide for the cap start its description wherever its own text
/// happened to end. At 120 columns almost nothing exceeded the cap and the
/// table looked right; at 90 columns `docker`'s global flags rendered with
/// descriptions starting at three different columns (19, 24 and 28), which
/// is not a table at all. The cap was silently setting a target that most
/// rows then missed individually.
///
/// A wide row does start its own first line past the column today, which
/// looks like that defect and is not it: it is one line of one row, one
/// space past that row's head, and every other line of that description —
/// and every line of every other row — is on the column. What made the old
/// behaviour ragged was that a row's *whole* description moved, so the
/// section had no column at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SectionLayout {
    /// The section's shared description column: where every description
    /// line in the section begins.
    ///
    /// The one number the whole section shares. The placeholder is part of
    /// what the reader types, so it is measured as part of the spelling
    /// rather than given an aligned slot of its own (spec §9.3): a slot
    /// has to be wide enough for the section's widest placeholder, which
    /// is width every row pays and one row needs — `grep`'s `-e,
    /// --regexp PATTERNS` ran past the description column on a placeholder
    /// alone.
    description: usize,
    /// Columns every row in the section is inset by — [`POSITIONAL_INDENT`]
    /// for POSITIONALS, zero for the flag-shaped sections.
    indent: usize,
}

/// The width that fits [`SHARED_COLUMN_PERCENTILE`] of `widths` — the
/// smallest number at least that share of the entries are within.
///
/// Zero for an empty section, and the maximum for a section small enough
/// that the percentile lands on it: a list of three flags aligns all three,
/// because "the majority, not the outliers" only has anything to exclude
/// once there is a tail to be an outlier in.
fn percentile_width(widths: impl Iterator<Item = usize>) -> usize {
    let mut widths: Vec<usize> = widths.collect();
    if widths.is_empty() {
        return 0;
    }
    widths.sort_unstable();
    let n = widths.len();
    let rank = (n * SHARED_COLUMN_PERCENTILE).div_ceil(100).max(1);
    widths[rank - 1]
}

/// The layout for one section's `entities` in a pane `width` columns wide,
/// inset by `indent`.
///
/// Three bounds on the shared column, and it is the *lowest* of them:
///
/// 1. **The percentile** (spec §9.3): fitted to the majority, so the widest
///    tenth pushes its own first line right rather than setting a column
///    for everyone else.
/// 2. **The pane cap** (spec §9.1a): a head past
///    [`DESC_COLUMN_CAP_PERCENT`] of the pane gets no vote at all, however
///    many of its kind there are — a section where *most* heads are
///    enormous must still leave prose a readable width.
/// 3. **The clamp** (spec §9.3): whatever the first two say, the column
///    comes down until [`MIN_DESC_WIDTH`] columns are left for prose. This
///    is what a narrow pane degrades by — the column moves, and the
///    section stays one layout with one column rather than swapping to a
///    second one at some threshold width.
///
/// …and one floor under all three: the column never comes further left
/// than two past the deepest column a spelling can start at. The area left
/// of the column is reserved for heads, so a column inside it would put
/// descriptions to the *left* of the preindented longs they belong to,
/// where a description stops reading as one.
///
/// Outliers are excluded from the measurement rather than clamped to it. A
/// clamped column is a column the outlier still misses; an excluded one is
/// a column it starts one space past while every other row stays aligned.
fn section_layout(entities: &[&Entity], width: usize, indent: usize) -> SectionLayout {
    let cap = width * DESC_COLUMN_CAP_PERCENT / 100;
    let gap = 2;

    // One measured width per row, from the pane's own left edge to the end
    // of the row's placeholder: a preindented long is measured where it
    // actually starts, and a placeholder is measured as part of the
    // spelling it belongs to rather than against a slot of its own.
    let fits = |w: usize| w + gap <= cap;
    let fitting = entities
        .iter()
        .map(|e| entity_head_width(e, indent))
        .filter(|w| fits(*w));

    let floor = indent + LONG_COLUMN + gap;
    let description = (percentile_width(fitting) + gap)
        .min(width.saturating_sub(MIN_DESC_WIDTH))
        .max(floor);
    SectionLayout {
        description,
        indent,
    }
}

/// One section's rendered body: its lines, its logical rows, and where a
/// search-targeted entity landed within it. All three index from the
/// section's own first line; [`build_lines`] rebases them onto the pane.
struct SectionBody {
    lines: Vec<Line<'static>>,
    rows: Vec<EntryRow>,
    target: Option<usize>,
}

/// The label the final dimmed group of inherited entities renders under
/// (spec §9): mixed case, like every other group divider, because it is
/// one.
const INHERITED_GROUP: &str = "Inherited";

/// Render one section's entities: ungrouped first under no divider, then
/// each [`Entity::group`] behind its divider, then inherited entities last
/// as their own dimmed group, whatever their source `group` said
/// (spec §9, §9.3).
fn section_lines(
    entities: &[&Entity],
    width: usize,
    indent: usize,
    palette: style::Palette,
    target_flag: Option<&FlagKey>,
    glyphs: Glyphs,
) -> SectionBody {
    let layout = section_layout(entities, width, indent);
    // Groups keep the order the tool printed them in, which is editorial:
    // `tar --help` leads with "Main operation mode" because that is what you
    // need first, and its 17 groups are sequenced deliberately. A BTreeMap
    // here sorted them alphabetically, so "Archive format selection" came
    // first and the author's ordering was silently discarded.
    let mut group_order: Vec<Option<String>> = Vec::new();
    let mut labels: HashMap<String, String> = HashMap::new();
    let mut own_groups: HashMap<Option<String>, Vec<&Entity>> = HashMap::new();
    let mut inherited: Vec<&Entity> = Vec::new();

    for e in entities {
        if e.inherited {
            inherited.push(e);
            continue;
        }
        let raw = e.group.as_deref().unwrap_or_default();
        let key = Some(group_key(raw)).filter(|k| !k.is_empty());
        if !own_groups.contains_key(&key) {
            group_order.push(key.clone());
            if let Some(k) = &key {
                // First occurrence names the group: two spellings of one
                // heading collapse under `group_key`, and the label the
                // reader sees is the one the tool introduced it with.
                labels.insert(k.clone(), group_label(raw));
            }
        }
        own_groups.entry(key).or_default().push(e);
    }

    let mut body = SectionBody {
        lines: Vec::new(),
        rows: Vec::new(),
        target: None,
    };

    // Ungrouped entities first, under no divider, then each group in the
    // order the tool introduced it.
    if let Some(ungrouped) = own_groups.remove(&None) {
        for e in ungrouped {
            push_entity(
                &mut body,
                e,
                false,
                width,
                palette.color,
                layout,
                target_flag,
            );
        }
    }
    for key in group_order {
        let Some(group) = own_groups.remove(&key) else {
            continue;
        };
        if let Some(key) = key {
            let label = labels.get(&key).map_or(key.as_str(), String::as_str);
            // A divider that opens its section drops its rule: the section
            // header a line above already drew one, and two full-width
            // rules in a row read as one doubled line (spec §9.3).
            let ruled = !body.lines.is_empty();
            // …and for the same reason it takes no blank row either. The
            // label belongs to the header directly above it, the way a
            // sub-heading does; a gap there would separate the two things
            // that go together and leave the section header floating.
            // Every *later* divider genuinely ends one run of rows and
            // starts another, so it gets its one row of air.
            if ruled {
                open_block(&mut body.lines, GROUP_BLANKS);
            }
            body.lines
                .push(group_divider_line(label, width, palette, glyphs, ruled));
        }
        for e in group {
            push_entity(
                &mut body,
                e,
                false,
                width,
                palette.color,
                layout,
                target_flag,
            );
        }
    }

    if !inherited.is_empty() {
        let ruled = !body.lines.is_empty();
        if ruled {
            open_block(&mut body.lines, GROUP_BLANKS);
        }
        body.lines.push(group_divider_line(
            INHERITED_GROUP,
            width,
            palette,
            glyphs,
            ruled,
        ));
        for e in inherited {
            push_entity(
                &mut body,
                e,
                true,
                width,
                palette.color,
                layout,
                target_flag,
            );
        }
    }

    body
}

/// Render one entity into `body` as **one logical row**, however many
/// lines it wraps onto (spec §9.3).
///
/// The single place an [`EntryRow`] is created, and it records the row's
/// first line — so a search landing on a wrapped entity scrolls to its
/// spelling, not into the middle of its description.
fn push_entity(
    body: &mut SectionBody,
    entity: &Entity,
    dim: bool,
    width: usize,
    color_enabled: bool,
    layout: SectionLayout,
    target_flag: Option<&FlagKey>,
) {
    let first_line = body.lines.len();
    if body.target.is_none() && target_flag.is_some_and(|k| entity.matches_key(k)) {
        body.target = Some(first_line);
    }
    let rendered = entity_line(entity, dim, width, color_enabled, layout);
    body.rows.push(EntryRow {
        first_line,
        lines: rendered.len(),
    });
    body.lines.extend(rendered);
}

/// An entity's spellings, e.g. `-i, --interactive` for a flag or
/// `pathspec` for a positional — with a repeatable positional's `...`
/// (spec §9.3).
fn entity_name_spec(flag: &Entity) -> String {
    // Every documented spelling, comma-separated, each reconstructed by
    // `Spelling::render`: the dashes and the getopt_long `--[no-]foo`
    // convention are display metadata, because the IR stores every name
    // bare (`"help"`, never `"-help"`; `"foo"`, never `"[no-]foo"`) so
    // that what a user searches and copies has no punctuation smuggled
    // into it. This is the one place that spelling comes back together —
    // and it is a *list*, so a row documenting four spellings
    // (`-h, -?, -help, --help`) renders all four rather than the two a
    // short/long pair could hold.
    let spellings = flag
        .spellings
        .iter()
        .map(Spelling::render)
        .collect::<Vec<_>>()
        .join(", ");
    // `repeatable` is one fact spelled two ways by the two kinds'
    // notation (`Entity::repeatable`): a flag says it by being accepted
    // again (`-v -v -v`), a positional by the POSIX synopsis ellipsis the
    // parser read it from (`FILE...`). Only the positional's notation is
    // a suffix on the name, so only POSITIONALS gets one back — putting
    // `...` on `-v` would invent a spelling nobody can type. Required/no
    // marker is untouched; the ellipsis says "more than one", not
    // "at least one".
    if flag.kind == EntityKind::Positional && flag.repeatable {
        return format!("{spellings}...");
    }
    spellings
}

/// How wide one entity's head runs: from the pane's left edge, through the
/// section's indent and the column its shape starts it at, to the end of
/// its value placeholder.
///
/// The single width the section's shared column is fitted to (spec §9.3).
/// It has to be one number, because a placeholder measured separately is a
/// placeholder the row is not charged for: `grep`'s `-e, --regexp
/// PATTERNS` fits a column measured over `-e, --regexp` and overruns the
/// one it is rendered against.
fn entity_head_width(entity: &Entity, indent: usize) -> usize {
    let mut width = spelling_column(entity, indent) + display_width(&entity_name_spec(entity));
    if let Some(v) = entity_value_text(entity) {
        let gap = if spelling_is_sigil(entity) { 0 } else { 1 };
        width += gap + display_width(&v);
    }
    width
}

/// An entity's value placeholder, e.g. `FILE` or `[FILE]` when optional.
/// `None` when it takes no value.
fn entity_value_text(flag: &Entity) -> Option<String> {
    flag.value_name
        .as_ref()
        .and_then(|name| match flag.value_kind {
            ValueKind::Required => Some(name.clone()),
            ValueKind::Optional => Some(format!("[{name}]")),
            ValueKind::None => None,
        })
}

/// True when this entity's spelling column ends in a sigil rather than a
/// name, so its value placeholder glues directly onto it with **no**
/// space — the argfile sigil flag's own row-verbatim shape, `@<file>`
/// (spec §4.5), rather than the ordinary `--output FILE` gap every other
/// valued flag renders with (spec §9.3).
///
/// Decided by shape, not by the literal spelling `"@"`, so any future
/// sigil-shaped entity this fleet turns up gets the same treatment for
/// free: a single spelling whose first character is not alphanumeric.
/// Every ordinary flag — `-i`, `--interactive`, even the punctuation-heavy
/// `-?`/`-<` this fleet has seen (which take no value) — has an
/// alphanumeric spelling, so this is a no-op for every row but the sigil
/// one.
fn spelling_is_sigil(flag: &Entity) -> bool {
    flag.spellings.len() == 1
        && flag.spellings[0]
            .name
            .chars()
            .next()
            .is_some_and(|c| !c.is_alphanumeric())
}

/// One entity's spellings, value placeholder, and description — each
/// styled per spec §9.2's table (spelling: accent; value placeholder:
/// muted; description: default foreground) — laid out against the
/// section's shared column.
///
/// Every description line starts at that column, first and continuation
/// alike, so the left of the section is heads and the right is prose. The
/// one exception is a head that reaches the column: it keeps its own line
/// and its first description line starts one space past where it ends,
/// with every continuation back at the column (spec §9.3).
///
/// The returned lines are one logical row (spec §9.3); the caller records
/// that as an [`EntryRow`].
fn entity_line(
    flag: &Entity,
    dim: bool,
    width: usize,
    color_enabled: bool,
    // Where the value and description columns begin, shared across the
    // whole section — see `section_layout`.
    layout: SectionLayout,
) -> Vec<Line<'static>> {
    let name_spec = entity_name_spec(flag);
    let value_text = entity_value_text(flag);

    // Two columns, chosen by the row's own shape (spec §9.3), behind the
    // section's own indent: shorts at the content edge, longs preindented
    // so every long in the section starts in the same place whether or not
    // a short precedes it.
    let head_column = spelling_column(flag, layout.indent);
    let leading = " ".repeat(head_column);
    let leading = leading.as_str();
    let spelling_style = if dim {
        style::muted(color_enabled)
    } else {
        style::accent(color_enabled)
    };
    // Muted, not italic. Italic is unreliable — spec §9.2 lists it among
    // the modifiers many terminals silently ignore, and where it *is*
    // honoured the glyphs frequently overflow their cell and leave
    // artefacts behind (reported on a `--log-level` value rendering
    // `error|info|debug`). It was also redundant the moment values moved
    // into their own column: position now carries the distinction, which
    // is the more robust signal anyway.
    let value_style = style::muted(color_enabled);
    let desc_style = if dim {
        style::muted(color_enabled)
    } else {
        Style::default()
    };

    let mut first_line_spans = vec![Span::styled(
        format!("{leading}{name_spec}"),
        spelling_style,
    )];
    let mut prefix_width = display_width(leading) + display_width(&name_spec);
    if let Some(v) = &value_text {
        // One space after the spelling, never a pad to a slot of its own
        // (spec §9.3) — except the argfile sigil flag (spec §4.5), whose
        // row-verbatim shape is `@<file>` with no space at all between the
        // sigil and its placeholder. `spelling_is_sigil` decides this by
        // shape (a lone non-alphanumeric-led spelling), not by checking for
        // `"@"` literally, so it never touches any other flag's rendering.
        // The placeholder is part of what the reader types, and a slot for
        // it is width every row in the section pays so that the widest
        // placeholder can line up — which is how a row whose first line was
        // mostly empty ended up hanging its description. The distinction
        // between name and placeholder is carried by the style, which
        // costs nothing.
        let gap = if spelling_is_sigil(flag) { "" } else { " " };
        first_line_spans.push(Span::raw(gap));
        first_line_spans.push(Span::styled(v.clone(), value_style));
        prefix_width += display_width(gap) + display_width(v);
    }

    // A head wider than the pane is broken across lines here, rather than
    // handed over for `Paragraph`'s defensive `Wrap` to reflow.
    //
    // This module pre-wraps everything precisely so that fallback never
    // has to act (see the module doc), and where it did act the result was
    // visibly wrong: `vgchange --alloc`'s value placeholder,
    // `contiguous|cling|cling_by_tags|normal|anywhere|inherit`, is one
    // 55-column token, and `Wrap` restarted it at column 0 with no memory
    // of the row's indent — a value placeholder rendered flush against the
    // pane's left edge, two rows below the spelling it belongs to. Found
    // by rendering `vgchange` through a real pty (AGENTS.md §3.2); no
    // synthetic fixture in the corpus had a placeholder that wide.
    let mut head: Vec<Line<'static>> = Vec::new();
    if prefix_width > width {
        // Budget the wrap at the row's own column, so every line of the
        // head sits in the area reserved for heads rather than drifting
        // into the description's.
        let budget = width.saturating_sub(head_column).max(1);
        let indent = leading.to_string();
        for (i, chunk) in wrap_words(&name_spec, budget).into_iter().enumerate() {
            let text = if i == 0 {
                format!("{leading}{chunk}")
            } else {
                format!("{indent}{chunk}")
            };
            head.push(Line::from(Span::styled(text, spelling_style)));
        }
        if let Some(v) = &value_text {
            for chunk in wrap_words(v, budget) {
                head.push(Line::from(Span::styled(
                    format!("{indent}{chunk}"),
                    value_style,
                )));
            }
        }
    }

    let deprecated_tag = flag
        .deprecated
        .as_ref()
        .map(|_| " (deprecated)".to_string());

    let mut description_text = flag.description.as_ref().map(|d| d.single_line());

    if let Some(tag) = &deprecated_tag {
        description_text = Some(match description_text {
            Some(d) => format!("{d}{tag}"),
            None => tag.trim_start().to_string(),
        });
    }
    let description_text = description_text.filter(|d| !d.is_empty());

    // The IR carries a flag's permitted values (spec §7 Tier B rule 4:
    // `gnu`/`oldgnu`/`pax`/`posix` under `tar --format=` are enum values,
    // which is why they are *not* subcommands). This does not join into
    // `description_text`: description text must stay the tool's own prose,
    // and the spelling column must stay verbatim — `tar --format` carries
    // both a `FORMAT` placeholder and `choices`, and folding an enumeration
    // into either would corrupt it. A derived enumeration gets its own
    // labeled line instead, indented two columns past the description
    // column, in the section's derived-metadata style.
    // A flag whose choices carry no per-value description (the common
    // case — `tar --quoting-style`'s bare `literal`/`shell`/`c`/...) still
    // gets the round-6 single summary line. A flag whose choices carry
    // their own text (ffmpeg/ffplay's AVOption constants, spec §7
    // recognition rule) render one indented `name  description` line per
    // choice instead — see `choice_detail_lines` below.
    let has_choice_descriptions = flag.choices.iter().any(|c| c.description.is_some());
    let values_line = (!flag.choices.is_empty() && !has_choice_descriptions).then(|| {
        let joined = flag
            .choices
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        format!("values: {joined}")
    });

    if description_text.is_none() && values_line.is_none() && !has_choice_descriptions {
        if !head.is_empty() {
            return head;
        }
        return vec![Line::from(first_line_spans)];
    }

    // One description column for the entire section, not one per entity.
    // That is what makes a parameter list read as a table — the defining
    // visual element of API documentation — and it only holds if it is
    // *always* the same number. It previously wasn't: the column was a
    // target, and any row too wide for it silently started its description
    // at its own width instead, so a list could show three different
    // "columns" at once.
    //
    // Every description line therefore starts at the column, and the left
    // of the section is reserved for heads. A head that reaches the column
    // is the one exception (spec §9.3): it cannot be truncated to fit
    // (spec §9.1's rule for the tree applies here too) and it must not
    // move the column for the rest of the section, so it keeps its line
    // and its *first* description line starts one space past where it
    // ends. Every later line of that same description is back at the
    // column, which is what keeps the exception a per-row nudge rather
    // than a second layout.
    let column = layout.description;
    let rest_width = width.saturating_sub(column).max(1);

    let mut lines = Vec::new();
    let mut remainder = description_text.clone().unwrap_or_default();
    if head.is_empty() {
        if description_text.is_some() {
            // One space past the head, or the column — whichever is
            // further right. They coincide for every row whose head fits.
            let start = column.max(prefix_width + 1);
            let first = width
                .checked_sub(start)
                .and_then(|room| leading_words(&remainder, room));
            if let Some((first_chunk, rest)) = first {
                first_line_spans.push(Span::raw(" ".repeat(start - prefix_width)));
                first_line_spans.push(Span::styled(first_chunk, desc_style));
                remainder = rest;
            }
        }
        // No description: the head is the whole first line, and a
        // choices-only flag's values line still lands on the next one.
        lines.push(Line::from(first_line_spans));
    } else {
        // A head broken across lines has taken the pane's whole width for
        // itself; its description begins on the line after it, at the
        // column like any other.
        lines.extend(head);
    }

    let indent_str = " ".repeat(column);
    if !remainder.is_empty() {
        for chunk in wrap_words(&remainder, rest_width) {
            lines.push(Line::from(Span::styled(
                format!("{indent_str}{chunk}"),
                desc_style,
            )));
        }
    }

    // The values line sits two columns past the description column (spec
    // §9.3): a further indent, not a fresh column, is what marks it as
    // subordinate to the description rather than a second row of the same
    // kind. It wraps at the pane's own width like any other pane line, and
    // renders even when the flag carries no description at all.
    if let Some(values_line) = values_line {
        let values_column = column + 2;
        let values_width = width.saturating_sub(values_column).max(1);
        let values_indent = " ".repeat(values_column);
        let values_style = style::muted(color_enabled);
        for chunk in wrap_words(&values_line, values_width) {
            lines.push(Line::from(Span::styled(
                format!("{values_indent}{chunk}"),
                values_style,
            )));
        }
    } else if has_choice_descriptions {
        lines.extend(choice_detail_lines(flag, column, width, color_enabled));
    }

    lines
}

/// Render a flag's choices as a `values:` header followed by one indented
/// `name  description` row per choice (spec §9.3/§4.1's render note),
/// dim/derived style matching the bare `values:` line this replaces.
///
/// Only called when at least one choice carries a description — the mixed
/// case (some choices described, some bare, e.g. ffmpeg's `-bug`, whose
/// `autodetect` value has no text of its own) still renders every choice
/// through this path so the list stays one table rather than splitting
/// into two.
fn choice_detail_lines(
    flag: &Entity,
    column: usize,
    width: usize,
    color_enabled: bool,
) -> Vec<Line<'static>> {
    let values_column = column + 2;
    let values_indent = " ".repeat(values_column);
    let values_style = style::muted(color_enabled);

    let choice_column = values_column + 2;
    let choice_indent = " ".repeat(choice_column);
    let choice_width = width.saturating_sub(choice_column).max(1);
    let name_width = flag
        .choices
        .iter()
        .map(|c| display_width(&c.name))
        .max()
        .unwrap_or(0);

    let mut lines = vec![Line::from(Span::styled(
        format!("{values_indent}values:"),
        values_style,
    ))];

    for choice in &flag.choices {
        let desc = choice
            .description
            .as_ref()
            .map(|d| d.single_line())
            .filter(|d| !d.is_empty());
        let Some(desc) = desc else {
            lines.push(Line::from(Span::styled(
                format!("{choice_indent}{}", choice.name),
                values_style,
            )));
            continue;
        };
        let head = format!("{:<name_width$}  ", choice.name);
        let head_width = display_width(&head);
        let first_room = choice_width.saturating_sub(head_width);
        match leading_words(&desc, first_room) {
            Some((first_chunk, rest)) => {
                lines.push(Line::from(Span::styled(
                    format!("{choice_indent}{head}{first_chunk}"),
                    values_style,
                )));
                // `leading_words` returns `rest == ""` when the whole
                // description fit on the first line — `wrap_words` always
                // returns at least one (possibly empty) chunk for callers
                // that need one, which is exactly wrong here: an empty
                // `rest` means there is no continuation to render, not one
                // blank line's worth. Skipping the call entirely (rather
                // than filtering its output) is what keeps a choice whose
                // description merely happens to be blank-after-wrapping
                // indistinguishable from one that never had a remainder —
                // there is no such case, `rest` is only ever the literal
                // empty string when nothing is left.
                if !rest.is_empty() {
                    let cont_indent = " ".repeat(choice_column + head_width);
                    let cont_width = width.saturating_sub(choice_column + head_width).max(1);
                    for chunk in wrap_words(&rest, cont_width) {
                        lines.push(Line::from(Span::styled(
                            format!("{cont_indent}{chunk}"),
                            values_style,
                        )));
                    }
                }
            }
            None => {
                lines.push(Line::from(Span::styled(
                    format!("{choice_indent}{}", choice.name),
                    values_style,
                )));
                for chunk in wrap_words(&desc, choice_width) {
                    lines.push(Line::from(Span::styled(
                        format!("{choice_indent}{chunk}"),
                        values_style,
                    )));
                }
            }
        }
    }

    lines
}

/// The words of `text` that fit in `width` display columns, and what is
/// left of it — `None` when not even the first word fits.
///
/// The greedy first line of a description whose head pushed it right
/// (spec §9.3). A word too wide for the room beside the head is not broken
/// there: `None` sends the whole description to the next line, where the
/// section's column gives it the room to break in. Splitting it twice —
/// once against a few leftover cells, once against the column — is how a
/// pushed row ends up less readable than a plain one.
fn leading_words(text: &str, width: usize) -> Option<(String, String)> {
    let mut first = String::new();
    let mut used = 0usize;
    let mut words = text.split_whitespace();
    let mut rest: Vec<&str> = Vec::new();

    for word in words.by_ref() {
        let candidate = used + usize::from(used > 0) + display_width(word);
        if candidate > width {
            rest.push(word);
            break;
        }
        if used > 0 {
            first.push(' ');
        }
        first.push_str(word);
        used = candidate;
    }
    if first.is_empty() {
        return None;
    }
    rest.extend(words);
    Some((first, rest.join(" ")))
}

/// The provenance footer (spec §2, §4.2): which sources contributed, and
/// whether structure and prose each came from a trusted source.
/// Where a node's data came from, e.g. `help-text + cobra-dunder-complete`.
///
/// Rendered in the status row under the detail pane rather than inside the
/// pane itself: it describes the pane's *subject*, not its content, and
/// inside the pane it pushed the documentation down by a line on every
/// command to say the same thing each time.
pub fn provenance_summary(node: &CommandNode) -> String {
    node.provenance
        .sources
        .iter()
        .map(|s| s.label())
        .collect::<Vec<_>>()
        .join(" + ")
}

/// Confidence below this is a warning; at or above it, silence.
///
/// 0.5 is exactly the cap Tier B applies when no framework was identified
/// but the generic engine parsed cleanly — `git`, `curl`, `apt-get` and
/// `openssl` all sit there and are fine. What is worth warning about is
/// well below it: `find` and `ip` both measure well under it (real
/// samples, mostly unclean), meaning the grammar recognised almost nothing
/// and what is on screen is a guess.
///
/// `node.provenance.confidence` is `mandible-extract`'s
/// `sections::compute_confidence`: clean/total over the option-table rows
/// the block scanner found, not a statement about the whole document (that
/// is `--doctor`'s separate `flag_description_ratio`, described/describable
/// over every flag — see `mandible/src/doctor.rs`'s header comment). The
/// two disagreeing is not automatically a bug — they measure different
/// things — but `ssh-keygen --help` (pure usage synopsis, zero real
/// option-table rows) used to read `0.0` here while `--doctor` reported
/// 100%: the block scanner's own curl-shaped-flags guard correctly handed
/// the wrapped final continuation line of `ssh-keygen`'s last usage form
/// (`-n namespace -s signature_file [-r krl_file] [-O option]`, which
/// opens with a dash) to the generic flags scanner, which read it as
/// exactly one option-table row, failed to parse it cleanly (it is not
/// one), and `0 / 1` reported a confident total failure from a sample of
/// one. Fixed at the source (`sections::compute_confidence`'s
/// `MIN_MEANINGFUL_SAMPLE`): a sample of zero *or one* row is folded into
/// the same "no real sample" fallback, not divided. `find`'s 19-row and
/// `ip`'s 11-row samples are untouched by that fix and still read as real,
/// low scores here.
const LOW_CONFIDENCE: f32 = 0.5;

/// A caveat about *this* node, or nothing at all.
///
/// The footer used to read `help-text · structure ✓ · prose ✓` under every
/// command of every tool. Both axes have authority for every tool
/// measured, so the ticks were always ticks; the tier list was the same
/// string on every node. It was decoration, and it crowded out the one
/// thing in this area that carries information — how much of the help text
/// the grammar actually understood.
///
/// So it now appears only when there is a caveat. Silence means "nothing
/// to flag", which is a stronger signal than a tick that is always
/// present, and it is the same reasoning that moved the framework out of
/// here: repeated identical metadata is noise, not provenance.
pub fn provenance_caveat(node: &CommandNode, glyphs: Glyphs) -> Option<String> {
    // Spec §5.4: this node's name came off a filename on `PATH`, not out of
    // the parent's own help text, so the strongest caveat available about
    // it is that the command may not exist at all. Checked before
    // everything below, including the verbatim exemption: how well the
    // binary's own help parsed says nothing about whether the parent
    // dispatches to it, and naming the binary is what lets the reader
    // settle that themselves.
    if let Some(binary) = &node.discovered_binary {
        return Some(format!(
            "unverified: not in the parent's help; found on PATH as `{binary}`"
        ));
    }

    // A node rendered verbatim is not a bad parse — it is the designed
    // honest fallback (spec §7 Tier B step 3), it carries confidence 0.0
    // by construction, and the pane already says so in its own words. Every
    // `git` subcommand lands here, because `git clone --help` renders
    // GIT-CLONE(1) and the man-page guard correctly refuses to mine roff
    // prose for structure. Reporting that as "0% parsed" made a deliberate
    // outcome read as a failure on every node of the tool.
    if !node.unparsed.is_empty() {
        return None;
    }

    // Spec §6 rule 2b: the tool's own text said this document is
    // incomplete, and mandible could not (or did not) follow it — an
    // unrecognised word/shape, a failed probe, or a rule 0 refusal.
    // Checked ahead of the confidence caveat below and unconditionally
    // (not gated on confidence at all): a node can parse *perfectly* —
    // every flag on this page correctly recognized and described — and
    // still be the wrong page, which is exactly curl's `--help` before
    // this feature existed. A *followed* confession (`followed: true`)
    // says nothing here; the tree already reflects the expanded document
    // and there is nothing left to flag.
    if let Some(confession) = &node.confession {
        if !confession.followed {
            return Some(format!(
                "incomplete: this tool's help said more is available (`{} {}`)",
                confession.flag, confession.word
            ));
        }
    }

    let confidence = node.provenance.confidence?;
    if confidence >= LOW_CONFIDENCE {
        return None;
    }

    // Terse on purpose: this shares one row with the controls, and the
    // long form ("… understood little of this tool's help text; treat the
    // structure as a guess") ran past the width available and pushed them
    // off. The percentage is the information; the reader can infer the
    // rest, and `--doctor` has the detail.
    let pct = (confidence * 100.0).round() as u32;
    let _ = glyphs;
    Some(format!("low confidence: {pct}% parsed"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandible_core::{Choice, Provenance, Source, Text};

    fn node_with_flags() -> CommandNode {
        let mut n = CommandNode::new(
            "rebase",
            Provenance::single(Source::KnownSpec {
                provider: "carapace".to_string(),
            }),
        );
        n.summary = Some(Text::sanitize("Reapply commits on top of another base tip"));
        let mut f1 = Entity::flag_long(
            "interactive",
            Provenance::single(Source::KnownSpec {
                provider: "carapace".to_string(),
            }),
        );
        f1.spellings.insert(0, Spelling::short('i'));
        f1.description = Some(Text::sanitize("Make a list of commits"));
        let mut f2 = Entity::flag_long(
            "help",
            Provenance::single(Source::KnownSpec {
                provider: "carapace".to_string(),
            }),
        );
        f2.inherited = true;
        f2.description = Some(Text::sanitize("Show help"));
        n.set_flags(vec![f1, f2]);
        n
    }

    /// A minimal `App` for tests that only need `build_lines`'s `app`
    /// parameter for its horizontal-scroll bookkeeping — the config
    /// defaults to on, matching a real run with no `config.toml`.
    fn test_app() -> App {
        App::new(
            "test".to_string(),
            CommandNode::new("test", Provenance::single(Source::HelpText)),
        )
    }

    fn text_of(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn inherited_flags_are_grouped_last() {
        let node = node_with_flags();
        let flags: Vec<&Entity> = node.flags().collect();
        let lines = section_lines(
            &flags,
            80,
            0,
            style::Palette::extended(),
            None,
            crate::glyphs::UNICODE,
        )
        .lines;
        let text: Vec<String> = lines.iter().map(text_of).collect();
        // A group divider, not a section header (spec §9.3): `INHERITED`
        // in caps would read as a section of its own, and inherited flags
        // are a group *within* FLAGS.
        let inherited_pos = text
            .iter()
            .position(|l| l.contains(INHERITED_GROUP))
            .unwrap();
        let help_pos = text.iter().position(|l| l.contains("--help")).unwrap();
        assert!(help_pos > inherited_pos);
        assert!(
            !text[inherited_pos].contains("INHERITED"),
            "the inherited group must not shout: {:?}",
            text[inherited_pos]
        );
    }

    /// A break `Text::sanitize` kept inside a description paragraph is
    /// structure, and the pane must render it as one (spec §4.1, §9.3).
    /// Wrapping the whole paragraph through one `split_whitespace` put
    /// `grep`'s `Example:` line straight back into the middle of the
    /// sentence after it, undoing at render time what the IR had just got
    /// right — the defect was visible in the pane while every IR-level
    /// test passed.
    #[test]
    fn a_preserved_description_break_renders_as_its_own_line() {
        let mut node = node_with_flags();
        node.description = Some(Text::sanitize(
            "Search for PATTERNS in each FILE.\n\
             Example: grep -i 'hello world' menu.h main.c\n\
             PATTERNS can contain multiple patterns separated by newlines.",
        ));
        let built = build_lines(
            &node,
            false,
            80,
            style::Palette::extended(),
            None,
            crate::glyphs::UNICODE,
            &test_app(),
        );
        let text: Vec<String> = built.lines.iter().map(text_of).collect();
        let example = text
            .iter()
            .position(|l| l.contains("Example:"))
            .expect("the example row renders");
        assert_eq!(
            text[example].trim(),
            "Example: grep -i 'hello world' menu.h main.c",
            "the example row must be a line of its own: {:?}",
            text[example]
        );
        assert!(
            text[example + 1].trim().starts_with("PATTERNS can contain"),
            "the sentence after it must start its own line: {:?}",
            text[example + 1]
        );
    }

    /// The anti-case at the render layer: ordinary hard-wrapped prose has
    /// no preserved breaks to honour, so it still reflows to the pane's
    /// width as one paragraph.
    #[test]
    fn hard_wrapped_description_prose_still_reflows_in_the_pane() {
        let mut node = node_with_flags();
        node.description = Some(Text::sanitize(
            "Search for PATTERNS\nin each FILE named on\nthe command line.",
        ));
        let built = build_lines(
            &node,
            false,
            80,
            style::Palette::extended(),
            None,
            crate::glyphs::UNICODE,
            &test_app(),
        );
        let text: Vec<String> = built.lines.iter().map(text_of).collect();
        assert!(
            text.iter()
                .any(|l| l.trim() == "Search for PATTERNS in each FILE named on the command line."),
            "prose must reflow to one line at this width: {text:?}"
        );
    }

    #[test]
    fn hidden_flags_suppressed_by_default() {
        let mut node = node_with_flags();
        node.flags_mut().next().expect("a flag").hidden = true;
        let built = build_lines(
            &node,
            false,
            80,
            style::Palette::extended(),
            None,
            crate::glyphs::UNICODE,
            &test_app(),
        );
        let joined: String = built.lines.iter().map(text_of).collect();
        assert!(!joined.contains("--interactive"));
    }

    #[test]
    fn hidden_flags_shown_when_toggled() {
        let mut node = node_with_flags();
        node.flags_mut().next().expect("a flag").hidden = true;
        let built = build_lines(
            &node,
            true,
            80,
            style::Palette::extended(),
            None,
            crate::glyphs::UNICODE,
            &test_app(),
        );
        let joined: String = built.lines.iter().map(text_of).collect();
        assert!(joined.contains("--interactive"));
    }

    /// Every description starts in the same column, whatever the flag's
    /// spelling is. Descriptions used to be indented by *each flag's own*
    /// width, so a list of options read as ragged prose rather than a
    /// parameter table — the alignment is what makes it look like
    /// documentation.
    #[test]
    fn flag_descriptions_share_one_column() {
        let mk = |short: Option<char>, long: &str, value: Option<&str>, desc: &str| {
            let mut f =
                mandible_core::Entity::flag_long(long, Provenance::single(Source::HelpText));
            if let Some(c) = short {
                f.spellings.insert(0, Spelling::short(c));
            }
            f.value_name = value.map(|v| v.to_string());
            if value.is_some() {
                f.value_kind = ValueKind::Required;
            }
            f.description = Some(Text::sanitize(desc));
            f
        };
        let flags = [
            mk(Some('d'), "detach", None, "zzz detached mode"),
            mk(
                None,
                "detach-keys",
                Some("string"),
                "zzz override the key sequence",
            ),
            mk(
                Some('e'),
                "env",
                Some("list"),
                "zzz set environment variables",
            ),
        ];
        let refs: Vec<&mandible_core::Entity> = flags.iter().collect();
        let lines = section_lines(
            &refs,
            80,
            0,
            style::Palette::extended(),
            None,
            crate::glyphs::UNICODE,
        )
        .lines;

        // Column at which each row's description text begins, located by a
        // marker rather than inferred from runs of whitespace: a row's
        // leading indent and the padding before a value placeholder are
        // both runs of whitespace too, so "the first double space" finds a
        // different thing on different rows and the measurement stops
        // meaning what its name says.
        let starts = description_columns(&lines);

        assert!(starts.len() >= 3, "expected a row per flag, got {starts:?}");
        assert!(
            starts.windows(2).all(|w| w[0] == w[1]),
            "descriptions are not column-aligned: {starts:?}"
        );
    }

    /// `docker --help`'s global flags, which is the list the alignment
    /// actually broke on. The test above uses three short synthetic flags
    /// at one comfortable width, and that is exactly why it kept passing
    /// while real panes rendered ragged: nothing in it was wide enough to
    /// exceed the column cap, so the per-row fallback never fired.
    ///
    /// Every description begins with `zzz` so its column can be located
    /// exactly rather than inferred from runs of whitespace (the value
    /// placeholder is also preceded by a run of whitespace, which is what
    /// makes the inference ambiguous).
    fn docker_global_flags() -> Vec<mandible_core::Entity> {
        let mk = |short: Option<char>, long: &str, value: Option<&str>| {
            let mut f =
                mandible_core::Entity::flag_long(long, Provenance::single(Source::HelpText));
            if let Some(c) = short {
                f.spellings.insert(0, Spelling::short(c));
            }
            f.value_name = value.map(|v| v.to_string());
            if value.is_some() {
                f.value_kind = ValueKind::Required;
            }
            // Every word is the marker, so the column of *every*
            // description line can be located exactly — a continuation
            // line is not distinguishable from a wrapped head by
            // indentation alone, and guessing is how a pin ends up
            // measuring the wrong lines.
            f.description = Some(Text::sanitize(
                "zzz zzz zzz zzz zzz zzz zzz zzz zzz zzz zzz zzz zzz zzz zzz zzz",
            ));
            f
        };
        vec![
            mk(None, "config", Some("string")),
            mk(Some('c'), "context", Some("string")),
            mk(Some('D'), "debug", None),
            mk(Some('H'), "host", Some("string")),
            mk(Some('l'), "log-level", Some("string")),
            mk(None, "tls", None),
            mk(None, "tlscacert", Some("string")),
        ]
    }

    /// The column that every description line in `lines` starts at.
    ///
    /// Located by the `zzz` marker every word of the fixture descriptions
    /// carries, so first lines and continuation lines are both measured
    /// and a head — wrapped or not — is never mistaken for either.
    fn description_columns(lines: &[Line<'static>]) -> Vec<usize> {
        lines
            .iter()
            .filter_map(|line| {
                let text = text_of(line);
                let at = text.find("zzz")?;
                Some(display_width(&text[..at]))
            })
            .collect()
    }

    /// Every description line in `lines`, as `(column, head)` — `head`
    /// being the text of the row's head where the description shares a
    /// line with one, and `None` on a continuation line or a line whose
    /// description starts at the very left edge.
    fn description_starts(lines: &[Line<'static>]) -> Vec<(usize, Option<String>)> {
        lines
            .iter()
            .filter_map(|line| {
                let text = text_of(line);
                let at = text.find("zzz")?;
                let before = &text[..at];
                let head = before.trim_end();
                let head = (!head.is_empty()).then(|| head.to_string());
                Some((display_width(before), head))
            })
            .collect()
    }

    /// The reported defect, at every width rather than one.
    ///
    /// A description never starts at a column its own row chose. It used
    /// to: the shared column was capped at 45% of the pane and any row too
    /// wide for the cap started its description at its own width instead,
    /// so `docker`'s global flags rendered descriptions at three different
    /// columns (19, 24 and 28) in a 90-column terminal — with `--log-level
    /// string` also losing the gap that separates a spelling from its
    /// value, so the two ran together as one token.
    ///
    /// Spec §9.3 states the rule as one layout with one column and one
    /// per-row exception, and this pins it line by line, which is stronger
    /// than the "at most two distinct columns" it supersedes: a
    /// continuation line is *always* at the section's column, and a first
    /// line is either at that column or exactly one space past the end of
    /// its own head — never at some third place, and never at a per-row
    /// indent of its own.
    #[test]
    fn every_description_line_starts_at_the_column_or_one_space_past_its_head() {
        let flags = docker_global_flags();
        let refs: Vec<&mandible_core::Entity> = flags.iter().collect();

        for width in 20..=160 {
            let lines = section_lines(
                &refs,
                width,
                0,
                style::Palette::extended(),
                None,
                crate::glyphs::UNICODE,
            )
            .lines;
            let column = section_layout(&refs, width, 0).description;
            let starts = description_starts(&lines);
            assert!(
                !starts.is_empty(),
                "width {width}: no descriptions rendered"
            );
            for (start, head) in starts {
                let Some(head) = head else {
                    assert_eq!(
                        start, column,
                        "width {width}: a continuation line must start at the \
                         section's column {column}"
                    );
                    continue;
                };
                if start == column {
                    continue;
                }
                assert!(
                    start > column,
                    "width {width}: {head:?} started its description at {start}, \
                     left of the section's column {column}"
                );
                assert_eq!(
                    start,
                    display_width(&head) + 1,
                    "width {width}: a head past the column is followed by exactly \
                     one space, not a column of its own: {head:?}"
                );
            }
        }
    }

    /// A pane too narrow for the section's own column brings the column
    /// down rather than shredding prose into a strip: the column is the
    /// thing that degrades, and the section stays one layout.
    ///
    /// At 90 columns `docker pull`'s `--platform` description used to
    /// break as "Set / platform / if server / is / multi-pla… / capable" —
    /// six lines for six words, one truncated mid-word, because the
    /// columns had eaten everything but 9 cells of the pane. This
    /// supersedes the stacked-layout pin: what guarantees the same
    /// legibility now is the clamp, not a second layout.
    #[test]
    fn a_narrow_pane_clamps_the_column_rather_than_shredding_prose() {
        let flags = docker_global_flags();
        let refs: Vec<&mandible_core::Entity> = flags.iter().collect();

        for width in 34..=60 {
            let column = section_layout(&refs, width, 0).description;
            assert!(
                width - column >= MIN_DESC_WIDTH,
                "width {width}: column {column} leaves {} for prose",
                width - column
            );
            // ...and the clamp is a clamp, not a reset: it only ever moves
            // the column left of where the section's own heads put it.
            let unclamped = percentile_width(
                refs.iter()
                    .map(|e| entity_head_width(e, 0))
                    .filter(|w| w + 2 <= width * DESC_COLUMN_CAP_PERCENT / 100),
            ) + 2;
            assert!(
                column <= unclamped.max(LONG_COLUMN + 2),
                "width {width}: the clamp moved the column right, to {column}"
            );
        }

        // Prose really does get that width on the page, measured on the
        // rendered lines rather than restated from the arithmetic above.
        let lines = section_lines(
            &refs,
            38,
            0,
            style::Palette::extended(),
            None,
            crate::glyphs::UNICODE,
        )
        .lines;
        let widest_prose = lines
            .iter()
            .map(text_of)
            .filter(|t| t.contains("zzz"))
            .map(|t| display_width(t.trim()))
            .max()
            .unwrap_or(0);
        assert!(
            widest_prose >= MIN_DESC_WIDTH,
            "prose still shredded: widest description line was {widest_prose}"
        );
    }

    /// One very long spelling must not drag every other row's description
    /// against the right-hand edge — the reason a cap existed at all. It
    /// now pushes only its own first line instead of widening the column,
    /// so the cap's original job is done without the raggedness it used to
    /// cause.
    #[test]
    fn one_overlong_head_pushes_only_its_own_first_line() {
        let mut flags = docker_global_flags();
        // Past the 45% cap at 120 columns, which is the point of the test.
        // A spelling that merely *looks* long is not an outlier: a 49-char
        // name at this width still leaves 59 columns for prose, and the
        // cap admits it deliberately rather than spending a line on it.
        let mut monster = mandible_core::Entity::flag_long(
            "an-extremely-long-option-name-that-nobody-would-ever-type-by-hand",
            Provenance::single(Source::HelpText),
        );
        monster.description = Some(Text::sanitize(
            "zzz zzz zzz zzz zzz zzz zzz zzz zzz zzz zzz zzz zzz zzz zzz zzz",
        ));
        flags.push(monster);
        let refs: Vec<&mandible_core::Entity> = flags.iter().collect();

        let without: Vec<&mandible_core::Entity> = refs[..refs.len() - 1].to_vec();
        let shared = section_layout(&refs, 120, 0).description;
        assert_eq!(
            section_layout(&refs, 120, 0),
            section_layout(&without, 120, 0),
            "an outlier spelling must not set the column for the list"
        );

        let lines = section_lines(
            &refs,
            120,
            0,
            style::Palette::extended(),
            None,
            crate::glyphs::UNICODE,
        )
        .lines;
        let starts = description_starts(&lines);

        // Exactly one line in the section starts past the shared column,
        // and it is the outlier's own first line, one space past its head.
        // This is stronger than the two-numbers pin it supersedes: it says
        // *which* line may miss the column, and by how much.
        let pushed: Vec<&(usize, Option<String>)> =
            starts.iter().filter(|(c, _)| *c != shared).collect();
        assert_eq!(
            pushed.len(),
            1,
            "only the outlier may be pushed: {starts:?}"
        );
        let (start, head) = pushed[0];
        let head = head.as_deref().expect("the outlier's head shares its line");
        assert!(head.contains("an-extremely-long-option-name"), "{head:?}");
        assert_eq!(*start, display_width(head) + 1);

        // ...and its own continuation lines come back to the column, so
        // the push is one line's worth of exception and no more.
        let outlier = lines
            .iter()
            .position(|l| text_of(l).contains("an-extremely-long-option-name"))
            .expect("outlier row missing");
        let continuation = text_of(&lines[outlier + 1]);
        assert!(continuation.contains("zzz"), "{continuation:?}");
        assert_eq!(
            continuation.len() - continuation.trim_start().len(),
            shared,
            "a pushed row's continuation must return to the column: {continuation:?}"
        );
    }

    /// A confidently-parsed node says nothing. Silence is the signal that
    /// there is nothing to flag, and it is a stronger one than a tick that
    /// was present on every node of every tool measured.
    #[test]
    fn a_confident_node_gets_no_caveat() {
        let mut node = node_with_flags();
        node.provenance = Provenance::with_confidence(Source::HelpText, 0.97);
        assert_eq!(provenance_caveat(&node, crate::glyphs::UNICODE), None);

        // Exactly at the threshold is Tier B's "no framework identified but
        // parsed cleanly" cap, where git, curl, apt-get — and, since
        // `sections::compute_confidence`'s `MIN_MEANINGFUL_SAMPLE` fix,
        // ssh-keygen — all sit. Not a warning.
        node.provenance = Provenance::with_confidence(Source::HelpText, LOW_CONFIDENCE);
        assert_eq!(provenance_caveat(&node, crate::glyphs::UNICODE), None);
    }

    /// A node shown verbatim says nothing here: it is the designed
    /// fallback, not a failed parse, and the pane already labels itself
    /// `unparsed`. Every `git` subcommand is one, since `git clone --help`
    /// renders a man page.
    #[test]
    fn a_verbatim_node_gets_no_caveat() {
        let mut node = node_with_flags();
        node.provenance = Provenance::with_confidence(Source::HelpText, 0.0);
        node.unparsed = vec![Text::sanitize("GIT-CLONE(1) Git Manual GIT-CLONE(1)")];
        assert_eq!(provenance_caveat(&node, crate::glyphs::UNICODE), None);
    }

    /// Spec §6 rule 2b: an unfollowed confession is flagged even on an
    /// otherwise perfectly-confident node — curl's `--help` parses every
    /// one of its 12 flags cleanly, and the problem is entirely that it's
    /// the wrong document, which confidence alone can never see.
    #[test]
    fn an_unfollowed_confession_warns_with_the_advertised_argv() {
        let mut node = node_with_flags();
        node.provenance = Provenance::with_confidence(Source::HelpText, 0.97);
        node.confession = Some(mandible_core::Confession::new(
            "all".to_string(),
            "--help".to_string(),
            false,
        ));
        let caveat = provenance_caveat(&node, crate::glyphs::UNICODE)
            .expect("an unfollowed confession must be surfaced even on a confident parse");
        assert!(caveat.contains("--help all"), "{caveat:?}");
    }

    /// A *followed* confession is the success case — the tree already
    /// reflects the expanded document — and must say nothing here.
    #[test]
    fn a_followed_confession_gets_no_caveat() {
        let mut node = node_with_flags();
        node.provenance = Provenance::with_confidence(Source::HelpText, 0.97);
        node.confession = Some(mandible_core::Confession::new(
            "all".to_string(),
            "--help".to_string(),
            true,
        ));
        assert_eq!(provenance_caveat(&node, crate::glyphs::UNICODE), None);
    }

    /// Spec §5.4: a node discovered by the `<parent>-<sub>` PATH convention
    /// names the binary it was found as, so the reader can settle for
    /// themselves whether the parent really dispatches to it.
    #[test]
    fn a_convention_discovered_node_says_it_is_unverified() {
        let mut node = node_with_flags();
        node.provenance = Provenance::with_confidence(Source::HelpText, 0.97);
        node.discovered_binary = Some("cargo-clippy".to_string());
        let caveat = provenance_caveat(&node, crate::glyphs::UNICODE)
            .expect("a convention-discovered node must say so");
        assert!(caveat.contains("unverified"), "{caveat:?}");
        assert!(caveat.contains("cargo-clippy"), "{caveat:?}");
    }

    /// How well the *binary's own* help parsed says nothing about whether
    /// the parent dispatches to it, so the two caveats are not alternatives
    /// and the unverified one is the load-bearing half.
    #[test]
    fn an_unverified_node_says_so_even_when_it_degraded_to_verbatim() {
        let mut node = node_with_flags();
        node.provenance = Provenance::with_confidence(Source::HelpText, 0.0);
        node.unparsed = vec![Text::sanitize("CARGO-CLIPPY(1)")];
        node.discovered_binary = Some("cargo-clippy".to_string());
        let caveat = provenance_caveat(&node, crate::glyphs::UNICODE)
            .expect("verbatim must not swallow the unverified caveat");
        assert!(caveat.contains("unverified"), "{caveat:?}");
    }

    /// A barely-parsed node says so. `find` scores 0.11 and `ip` 0.09 in
    /// practice, and both used to report `structure ✓ · prose ✓`.
    #[test]
    fn a_barely_parsed_node_warns_with_its_score() {
        let mut node = node_with_flags();
        node.provenance = Provenance::with_confidence(Source::HelpText, 0.11);
        let caveat = provenance_caveat(&node, crate::glyphs::UNICODE)
            .expect("low confidence must be surfaced");
        assert!(caveat.contains("11%"), "{caveat:?}");
        assert!(caveat.contains("low confidence"), "{caveat:?}");
        // Short enough to share a row with the controls.
        assert!(caveat.chars().count() <= 32, "too long: {caveat:?}");
    }

    /// The reported defect: a flag description that wraps must hang-
    /// indent under the description column on continuation lines, not
    /// restart at column 0.
    #[test]
    fn a_pushed_description_continues_at_the_shared_column() {
        let mut flag = Entity::flag_long("tlscacert", Provenance::single(Source::HelpText));
        flag.value_name = Some("string".to_string());
        flag.value_kind = ValueKind::Required;
        flag.description = Some(Text::sanitize(
            "Trust certs signed only by this CA (default \"\")",
        ));
        let column = 20;
        let lines = entity_line(
            &flag,
            false,
            40,
            true,
            SectionLayout {
                description: column,
                indent: 0,
            },
        );
        assert!(lines.len() >= 2, "expected wrapping: {lines:?}");

        // This row's spelling plus value runs to 24, past the column, so
        // its first description line is pushed to 25 — one space past its
        // own head, and nowhere else.
        let first = text_of(&lines[0]);
        let head = "    --tlscacert string";
        assert!(first.starts_with(head), "{first:?}");
        assert_eq!(
            first.find("Trust"),
            Some(head.len() + 1),
            "a head past the column is followed by exactly one space: {first:?}"
        );

        // The pin the old hanging-indent assertion carried is kept and
        // strengthened, not dropped: the failure it guarded against was a
        // continuation that clears *this row's own prefix*, which is the
        // per-row indent that made a list of flags render with three
        // different "columns" at once. Every continuation here is checked
        // against the section's own column — one number the whole section
        // shares — and that number is well clear of this row's 24-column
        // prefix.
        for line in &lines[1..] {
            let text = text_of(line);
            let indent_len = text.len() - text.trim_start().len();
            assert_eq!(
                indent_len, column,
                "first={first:?} line={text:?} must continue at the shared column"
            );
            assert!(
                indent_len < display_width(&first),
                "a description must not be indented by its own row's width"
            );
        }
    }

    /// Spec §9.2: the flag spelling is accent-styled, the value
    /// placeholder is muted italic, and the description is default
    /// foreground — three distinct spans, not one undifferentiated run.
    #[test]
    fn flag_line_has_distinctly_styled_spans() {
        let mut flag = Entity::flag_long("output", Provenance::single(Source::HelpText));
        flag.spellings.insert(0, Spelling::short('o'));
        flag.value_name = Some("FILE".to_string());
        flag.value_kind = ValueKind::Required;
        flag.description = Some(Text::sanitize("Write output to FILE"));
        let lines = entity_line(
            &flag,
            false,
            80,
            true,
            SectionLayout {
                description: 20,
                indent: 0,
            },
        );
        let spans = &lines[0].spans;
        assert!(spans.len() >= 3, "{spans:?}");
        // Spelling span carries the accent color.
        assert_eq!(spans[0].style.fg, Some(style::ACCENT));
        // The value sits in its own column, so the padding between the two
        // is its own (unstyled) span and the value follows it.
        let value = spans
            .iter()
            .find(|s| s.content.as_ref() == "FILE")
            .expect("value should be its own span");
        assert_ne!(
            value.style, spans[0].style,
            "value must not read as a spelling"
        );
    }

    /// The argfile sigil flag (spec §4.5) renders row-verbatim-shaped:
    /// `@<file>` with **no** space between the sigil and its placeholder,
    /// unlike every ordinary valued flag's `--output FILE` gap. `§3.4`
    /// plant: comment out the `spelling_is_sigil` check in `entity_line`
    /// (make the `gap` always `" "`) and this test goes red on the
    /// `"@ <file>"` it then renders instead.
    #[test]
    fn argfile_sigil_flag_glues_its_value_with_no_space() {
        let flag = Entity::argfile_sigil("<file>", Provenance::single(Source::HelpText));
        let lines = entity_line(
            &flag,
            false,
            80,
            true,
            SectionLayout {
                description: 20,
                indent: 0,
            },
        );
        let first = text_of(&lines[0]);
        assert!(
            first.trim_start().starts_with("@<file>"),
            "the sigil and its placeholder must glue with no space: {first:?}"
        );
        assert!(
            !first.contains("@ <"),
            "must never render a space between @ and its placeholder: {first:?}"
        );
        // Every ordinary valued flag keeps its one-space gap — the sigil
        // case must not have loosened the general rule.
        let mut ordinary = Entity::flag_long("output", Provenance::single(Source::HelpText));
        ordinary.value_name = Some("FILE".to_string());
        ordinary.value_kind = ValueKind::Required;
        let ordinary_lines = entity_line(
            &ordinary,
            false,
            80,
            true,
            SectionLayout {
                description: 20,
                indent: 0,
            },
        );
        let ordinary_first = text_of(&ordinary_lines[0]);
        assert!(
            ordinary_first.contains("--output FILE"),
            "an ordinary flag must keep its one-space gap: {ordinary_first:?}"
        );
    }

    #[test]
    fn deprecated_flag_gets_a_tag() {
        let mut flag = Entity::flag_long("old-flag", Provenance::single(Source::HelpText));
        flag.deprecated = Some(Text::sanitize("use --new-flag instead"));
        flag.description = Some(Text::sanitize("Old behavior"));
        let lines = entity_line(
            &flag,
            false,
            80,
            true,
            SectionLayout {
                description: 20,
                indent: 0,
            },
        );
        let joined: String = lines.iter().map(text_of).collect();
        assert!(joined.contains("(deprecated)"), "{joined:?}");
    }

    /// Spec §9.3: a flag's `choices` render as their own line under the
    /// description, not folded into it — the description stays the tool's
    /// own prose, and the enumeration gets a `values:` line indented two
    /// columns past the shared description column.
    #[test]
    fn choices_render_on_their_own_line_below_the_description() {
        let mut flag = Entity::flag_long("format", Provenance::single(Source::HelpText));
        flag.description = Some(Text::sanitize("archive format to create"));
        flag.choices = vec![
            Choice::bare("default"),
            Choice::bare("gnu"),
            Choice::bare("darwin"),
            Choice::bare("bsd"),
            Choice::bare("bigarchive"),
        ];
        let layout = SectionLayout {
            description: 20,
            indent: 0,
        };
        let lines = entity_line(&flag, false, 80, true, layout);

        let joined: String = lines.iter().map(text_of).collect::<Vec<_>>().join("\n");
        assert!(
            joined.contains("archive format to create"),
            "the description must stay the tool's own prose: {joined:?}"
        );
        assert!(
            !joined.contains('['),
            "choices must not be bracketed into the description any more: {joined:?}"
        );
        assert!(
            joined.contains("values: default, gnu, darwin, bsd, bigarchive"),
            "choices must render as a labeled values line: {joined:?}"
        );

        let values_line = lines
            .iter()
            .find(|l| text_of(l).contains("values:"))
            .expect("a values line");
        let text = text_of(values_line);
        let indent_len = text.len() - text.trim_start().len();
        assert_eq!(
            indent_len,
            layout.description + 2,
            "the values line must sit two columns past the description column: {text:?}"
        );
        assert_eq!(
            values_line.spans[0].style,
            style::muted(true),
            "the values line must use the pane's derived-metadata style"
        );
    }

    /// A flag can carry `choices` with no description at all (an
    /// undocumented enum flag). The values line must still render, right
    /// under the head, rather than being dropped because there was no
    /// description to attach it to.
    #[test]
    fn choices_render_even_when_the_flag_has_no_description() {
        let mut flag = Entity::flag_long("format", Provenance::single(Source::HelpText));
        flag.choices = vec![Choice::bare("posix"), Choice::bare("windows")];
        let layout = SectionLayout {
            description: 20,
            indent: 0,
        };
        let lines = entity_line(&flag, false, 80, true, layout);

        assert_eq!(
            lines.len(),
            2,
            "one head line and one values line: {lines:?}"
        );
        assert!(
            !text_of(&lines[0]).contains("values:"),
            "the head line must not carry the values line: {:?}",
            text_of(&lines[0])
        );
        let text = text_of(&lines[1]);
        assert!(text.contains("values: posix, windows"), "{text:?}");
        let indent_len = text.len() - text.trim_start().len();
        assert_eq!(indent_len, layout.description + 2, "{text:?}");
    }

    /// A flag whose choices carry their own text — ffplay/ffmpeg's AVOption
    /// constants (spec §7 recognition rule) — render `values:` alone as a
    /// header, then one indented `name  description` row per choice,
    /// instead of the single comma-joined summary line a bare-choices flag
    /// gets. Folding the constants back into one line would just relocate
    /// the smear this round's recognizer exists to fix.
    #[test]
    fn described_choices_render_as_a_name_description_list() {
        let mut flag = Entity::flag_long("flags", Provenance::single(Source::HelpText));
        flag.value_name = Some("<flags>".to_string());
        flag.description = Some(Text::sanitize("ED.VAS..... (default 0)"));
        flag.choices = vec![
            Choice::described(
                "unaligned",
                Text::sanitize(".D.V....... allow decoders to produce unaligned output"),
            ),
            Choice::described(
                "gray",
                Text::sanitize("ED.V....... only decode/encode grayscale"),
            ),
        ];
        let layout = SectionLayout {
            description: 20,
            indent: 0,
        };
        // A wide pane, deliberately: this test is about the choice list's
        // *shape* (a header line, then one row per choice), not about
        // word-wrap, which `a_narrow_pane_clamps_the_column_rather_than_shredding_prose`
        // and friends already cover for the description column this
        // section reuses.
        let lines = entity_line(&flag, false, 200, true, layout);
        let joined: String = lines.iter().map(text_of).collect::<Vec<_>>().join("\n");

        assert!(
            joined.contains("ED.VAS..... (default 0)"),
            "the flag's own description must stay put: {joined:?}"
        );
        assert!(
            !joined.contains("unaligned, gray"),
            "described choices must never fall back to the comma-joined line: {joined:?}"
        );

        let header = lines
            .iter()
            .find(|l| text_of(l).trim() == "values:")
            .expect("a bare 'values:' header line");
        let header_indent = {
            let t = text_of(header);
            t.len() - t.trim_start().len()
        };
        assert_eq!(
            header_indent,
            layout.description + 2,
            "the header sits two columns past the description column, exactly like the bare case"
        );
        assert_eq!(
            header.spans[0].style,
            style::muted(true),
            "the header must use the pane's derived-metadata style"
        );

        let unaligned = lines
            .iter()
            .find(|l| text_of(l).contains("unaligned"))
            .expect("the unaligned choice row");
        let unaligned_text = text_of(unaligned);
        assert!(
            unaligned_text.contains("allow decoders to produce unaligned output"),
            "{unaligned_text:?}"
        );
        let unaligned_indent = unaligned_text.len() - unaligned_text.trim_start().len();
        assert!(
            unaligned_indent > header_indent,
            "each choice row sits deeper than the 'values:' header: {unaligned_indent} vs {header_indent}"
        );

        let gray = lines
            .iter()
            .find(|l| text_of(l).contains("gray"))
            .expect("the gray choice row");
        assert!(
            text_of(gray).contains("only decode/encode grayscale"),
            "{:?}",
            text_of(gray)
        );
    }

    /// A choice whose description fits on the row's first line must never
    /// be followed by a blank line. `wrap_words` always returns at least
    /// one (possibly empty) chunk — a documented guarantee useful to most
    /// of its callers, wrong for a continuation that may legitimately not
    /// exist — so calling it unconditionally on `leading_words`'s `rest`
    /// rendered one spurious blank `Line` per choice whose text fit
    /// entirely on its own row. At a width where `-flags`' real `unaligned`
    /// wraps and its real `gray` does not, this pins both shapes at once:
    /// the render must show exactly six lines (the header, one wrapped
    /// choice across two rows, one single-line choice), never eight.
    #[test]
    fn a_choice_that_fits_on_one_line_is_never_followed_by_a_blank_line() {
        let mut flag = Entity::flag_long("flags", Provenance::single(Source::HelpText));
        flag.value_name = Some("<flags>".to_string());
        flag.description = Some(Text::sanitize("ED.VAS..... (default 0)"));
        flag.choices = vec![
            Choice::described(
                "unaligned",
                Text::sanitize(".D.V....... allow decoders to produce unaligned output"),
            ),
            Choice::described(
                "gray",
                Text::sanitize("ED.V....... only decode/encode grayscale"),
            ),
        ];
        let layout = SectionLayout {
            description: 20,
            indent: 0,
        };
        // Narrow enough that "unaligned"'s longer description wraps, wide
        // enough that "gray"'s shorter one fits on a single line — the
        // exact split the real ffplay pane showed the defect on.
        let lines = entity_line(&flag, false, 82, true, layout);
        let texts: Vec<String> = lines.iter().map(text_of).collect();

        assert!(
            texts.iter().all(|t| !t.trim().is_empty()),
            "no rendered line may be blank: {texts:?}"
        );
        assert_eq!(
            texts,
            vec![
                "    --flags         ED.VAS..... (default 0)".to_string(),
                "                      values:".to_string(),
                "                        unaligned  .D.V....... allow decoders to produce unaligned"
                    .to_string(),
                "                                   output".to_string(),
                "                        gray       ED.V....... only decode/encode grayscale"
                    .to_string(),
            ],
            "{texts:#?}"
        );
    }

    /// `tar --format` carries both a `FORMAT` placeholder in the spelling
    /// column and `choices` (spec §7 Tier B rule 4). The placeholder must
    /// render verbatim, on the head line, exactly as before — the values
    /// line is additional, never a replacement or a rewrite of the head.
    #[test]
    fn a_placeholder_and_choices_together_leave_the_spelling_column_untouched() {
        let mut flag = Entity::flag_long("format", Provenance::single(Source::HelpText));
        flag.value_name = Some("FORMAT".to_string());
        flag.value_kind = ValueKind::Required;
        flag.description = Some(Text::sanitize("archive format to create"));
        flag.choices = vec![
            Choice::bare("gnu"),
            Choice::bare("oldgnu"),
            Choice::bare("pax"),
            Choice::bare("posix"),
        ];
        let layout = SectionLayout {
            description: 20,
            indent: 0,
        };
        let lines = entity_line(&flag, false, 80, true, layout);

        let head = text_of(&lines[0]);
        assert!(
            head.contains("--format") && head.contains("FORMAT"),
            "the spelling and its placeholder must render verbatim, together: {head:?}"
        );
        assert!(
            !head.contains("values:") && !head.contains('['),
            "the head line must carry no trace of choices: {head:?}"
        );

        let joined: String = lines.iter().map(text_of).collect::<Vec<_>>().join("\n");
        assert!(
            joined.contains("values: gnu, oldgnu, pax, posix"),
            "choices still render, just off the head line: {joined:?}"
        );
    }

    /// The coordinator's second reported defect: a group heading must not
    /// carry its source's trailing colon or casing quirks into the UI —
    /// `"GLOBAL OPTIONS:"` and `"Global Options"` must collect into one
    /// group rather than rendering the same logical grouping twice.
    ///
    /// The *key* is what does that collecting, and it stays case-folded.
    /// What changed with spec §9.3 is that the key is no longer what the
    /// reader sees: a displayed CAPS heading is now the section header's
    /// shape, so the label went mixed case and got its own function.
    #[test]
    fn group_keys_strip_trailing_colon_and_fold_case() {
        assert_eq!(group_key("GLOBAL OPTIONS:"), "GLOBAL OPTIONS");
        assert_eq!(group_key("Main operation mode:"), "MAIN OPERATION MODE");
        assert_eq!(group_key("main"), "MAIN");
        // Two spellings of one heading are one group.
        assert_eq!(group_key("Global Options"), group_key("GLOBAL OPTIONS:"));
    }

    /// Spec §9.3: a group divider's label is mixed case, never CAPS —
    /// that shape difference is what keeps it distinguishable from a
    /// section header on a terminal that ignores dimming (spec §9.2).
    ///
    /// A heading the tool shouted is set in sentence case; one that
    /// carries the author's own casing keeps it, because there the mixed
    /// case is information rather than a help-text formatting convention.
    #[test]
    fn group_labels_are_mixed_case_never_caps() {
        assert_eq!(group_label("GLOBAL OPTIONS:"), "Global options");
        assert_eq!(group_label("Main operation mode:"), "Main operation mode");
        assert_eq!(group_label("main"), "Main");
        for raw in ["GLOBAL OPTIONS:", "Main operation mode:", "main"] {
            let label = group_label(raw);
            assert!(
                label.chars().any(char::is_lowercase),
                "a group label must not read as a section header: {label:?}"
            );
        }
    }

    /// A group whose label is a whole sentence the tool wrote — LVM's
    /// per-stanza description, which spec §7 Tier B makes that stanza's
    /// group label — loses its full stop the same way a heading loses its
    /// colon: the label runs straight into the divider's rule, and a
    /// terminator between the two reads as a stray mark rather than as the
    /// end of a sentence. One stop only, and never an ellipsis, which is
    /// repetition notation rather than punctuation.
    #[test]
    fn group_labels_drop_a_sentence_terminator_like_a_heading_colon() {
        assert_eq!(
            group_label("Start the lockspace of a shared VG in lvmlockd."),
            "Start the lockspace of a shared VG in lvmlockd"
        );
        assert_eq!(
            group_label("Activate or deactivate LVs."),
            "Activate or deactivate LVs"
        );
        // Not punctuation: docopt repetition notation stays.
        assert_eq!(group_label("FILE..."), "File...");
        // The colon rule is unchanged, and the two collapse to one key.
        assert_eq!(group_label("Main operation mode:"), "Main operation mode");
        assert_eq!(
            group_key("Activate or deactivate LVs."),
            group_key("Activate or deactivate LVs")
        );
    }

    /// Spec §9.3: a value placeholder is fused onto the spelling it
    /// belongs to and measured with it — it gets no aligned slot of its
    /// own.
    ///
    /// A slot has to be as wide as the section's widest placeholder, and
    /// every row in the section pays that width whether it takes a value
    /// or not. `grep`'s `-e, --regexp PATTERNS` is the case that named
    /// it: the placeholder alone pushed the row past the description
    /// column, so the description hung onto a second line while the first
    /// sat mostly empty.
    #[test]
    fn a_placeholder_is_fused_onto_its_spelling_not_given_a_slot() {
        let mk = |short: char, long: &str, value: Option<&str>| {
            let mut f = Entity::flag_long(long, Provenance::single(Source::HelpText));
            f.spellings.insert(0, Spelling::short(short));
            f.value_name = value.map(str::to_string);
            if value.is_some() {
                f.value_kind = ValueKind::Required;
            }
            f.description = Some(Text::sanitize("zzz what this one does"));
            f
        };
        let flags = [
            mk('e', "regexp", Some("PATTERNS")),
            mk('f', "file", Some("FILE")),
            mk('c', "count", None),
        ];
        let refs: Vec<&Entity> = flags.iter().collect();
        let text: Vec<String> = section_lines(
            &refs,
            60,
            0,
            style::Palette::extended(),
            None,
            crate::glyphs::UNICODE,
        )
        .lines
        .iter()
        .map(text_of)
        .collect();
        let row = |needle: &str| {
            text.iter()
                .find(|t| t.contains(needle))
                .unwrap_or_else(|| panic!("no row for {needle}: {text:?}"))
                .clone()
        };

        // Each placeholder sits one space past its own spelling, so two
        // rows of different spelling widths put theirs in different
        // columns. A slot would line them up, which is exactly the width
        // this stops spending.
        let regexp = row("PATTERNS");
        let file = row("FILE");
        assert_eq!(regexp.find("PATTERNS"), Some("-e, --regexp".len() + 1));
        assert_eq!(file.find("FILE"), Some("-f, --file".len() + 1));
        assert_ne!(
            regexp.find("PATTERNS"),
            file.find("FILE"),
            "placeholders must not share a column"
        );

        // ...and the section's column is measured over the fused width, so
        // the widest of them still reaches its description on row one
        // rather than hanging on a placeholder nobody measured.
        assert!(
            regexp.contains("zzz"),
            "the placeholder's own width must not hang the description: {regexp:?}"
        );
    }

    /// Spec §9.1a: a head past the 45% cap gets no vote on the column,
    /// however many of its kind there are. Where the cap excludes most of
    /// a section, the column the minority sets still serves the whole
    /// section — every row's description continues at it, and the excluded
    /// rows push only their own first lines.
    ///
    /// This supersedes the pin that sent such a section to a stacked
    /// layout. The failure that rule guarded against — most of a section
    /// missing the column it is supposedly aligned on — is now impossible
    /// by construction rather than by choosing a second layout: a row that
    /// cannot reach the column never renders a line at any other one.
    #[test]
    fn a_column_the_minority_fits_still_serves_the_whole_section() {
        let mk = |long: &str| {
            let mut f = Entity::flag_long(long, Provenance::single(Source::HelpText));
            f.description = Some(Text::sanitize(
                "zzz zzz zzz zzz zzz zzz zzz zzz zzz zzz zzz zzz",
            ));
            f
        };
        // Four rows well past 45% of a 60-column pane, one comfortably
        // inside it.
        let mut flags: Vec<Entity> = (0..4)
            .map(|i| mk(&format!("a-considerably-wider-option-{i}")))
            .collect();
        flags.push(mk("x"));
        let refs: Vec<&Entity> = flags.iter().collect();

        let cap = 60 * DESC_COLUMN_CAP_PERCENT / 100;
        let fitting = refs
            .iter()
            .filter(|e| entity_head_width(e, 0) + 2 <= cap)
            .count();
        assert_eq!(fitting, 1, "the fixture must leave a minority fitting");

        let column = section_layout(&refs, 60, 0).description;
        let starts = description_starts(
            &section_lines(
                &refs,
                60,
                0,
                style::Palette::extended(),
                None,
                crate::glyphs::UNICODE,
            )
            .lines,
        );
        assert!(
            starts.len() > refs.len(),
            "the fixture must wrap, or continuation lines prove nothing: {starts:?}"
        );
        for (start, head) in &starts {
            match head {
                None => assert_eq!(start, &column, "a continuation line left the column"),
                Some(head) => assert!(
                    *start == column || *start == display_width(head) + 1,
                    "{head:?} started its description at {start}, neither the \
                     column {column} nor one space past its own head"
                ),
            }
        }
    }

    /// Spec §9.3's two columns: a short spelling starts at the content
    /// edge, and every long starts one short-prefix in — including a long
    /// with no short partner, which is preindented to get there.
    ///
    /// The point of the preindent is that the eye can follow the longs
    /// down a single column without first having to notice which rows
    /// happen to carry a short letter, so what is asserted is the column
    /// each long *lands in*, not the padding each row was given.
    #[test]
    fn shorts_start_at_the_edge_and_longs_align_in_one_column() {
        let mk = |spellings: Vec<Spelling>| {
            let mut e = Entity::new(EntityKind::Flag, Provenance::single(Source::HelpText));
            e.spellings = spellings;
            e.description = Some(Text::sanitize("zzz does a thing"));
            e
        };
        let flags = [
            mk(vec![Spelling::short('d'), Spelling::long("detach")]),
            mk(vec![Spelling::long("detach-keys")]),
            mk(vec![Spelling::short('D')]),
            mk(vec![
                Spelling::short('h'),
                Spelling::single_dash("help"),
                Spelling::long("help"),
            ]),
            mk(vec![
                Spelling::long("one"),
                Spelling::long("two"),
                Spelling::long("three"),
            ]),
        ];
        let refs: Vec<&Entity> = flags.iter().collect();
        let text: Vec<String> = section_lines(
            &refs,
            80,
            0,
            style::Palette::extended(),
            None,
            crate::glyphs::UNICODE,
        )
        .lines
        .iter()
        .map(text_of)
        .collect();
        let row = |needle: &str| {
            text.iter()
                .find(|t| t.contains(needle))
                .unwrap_or_else(|| panic!("no row for {needle}: {text:?}"))
                .clone()
        };

        // A short/long pair leads with the short at the edge, which puts
        // its long exactly one short prefix in.
        let pair = row("--detach ");
        assert!(pair.starts_with("-d, --detach"), "{pair:?}");
        assert_eq!(pair.find("--detach"), Some(LONG_COLUMN), "{pair:?}");

        // A long with no short is preindented to the same column.
        let lone = row("--detach-keys");
        assert_eq!(lone.find("--detach-keys"), Some(LONG_COLUMN), "{lone:?}");
        assert!(
            lone.starts_with(&" ".repeat(LONG_COLUMN)),
            "a lone long is preindented, not padded elsewhere: {lone:?}"
        );

        // A short with no long stays at the edge rather than reserving a
        // column nothing follows it into.
        let short_only = row("-D ");
        assert!(short_only.starts_with("-D"), "{short_only:?}");

        // More than two spellings flows from the short column: there is no
        // single "the long" in such a row for a column to align.
        for needle in ["-h, -help, --help", "--one, --two, --three"] {
            let many = row(needle);
            assert!(
                many.starts_with(needle),
                "a multi-spelling row flows from the edge: {many:?}"
            );
        }

        // ...and the whole section still shares one description column.
        let starts = description_columns(
            &section_lines(
                &refs,
                80,
                0,
                style::Palette::extended(),
                None,
                crate::glyphs::UNICODE,
            )
            .lines,
        );
        let distinct: std::collections::BTreeSet<usize> = starts.iter().copied().collect();
        assert_eq!(
            distinct.len(),
            1,
            "two columns, one description: {starts:?}"
        );
    }

    /// Spec §9.3: the column never comes further left than the deepest
    /// column a spelling can start at, so a preindented long never has its
    /// own description sitting flush beneath it — or, worse, left of it.
    ///
    /// The old hanging indent could coincide with the column a lone long
    /// starts at, and at that value the row renders as a name with a
    /// sentence directly under it at the same left edge — two lines of
    /// equal rank, with nothing to say the second belongs to the first.
    /// The floor under the clamp is what keeps that from coming back at a
    /// pane width narrow enough to squeeze the column into the head area,
    /// which is the one thing that can now move the column left.
    #[test]
    fn the_column_never_moves_left_of_the_heads_it_serves() {
        let mut flag = Entity::flag_long("config", Provenance::single(Source::HelpText));
        flag.description = Some(Text::sanitize(
            "zzz zzz zzz zzz zzz zzz zzz zzz zzz zzz zzz zzz zzz zzz",
        ));
        assert_eq!(
            spelling_column(&flag, 0),
            LONG_COLUMN,
            "the fixture must be a preindented lone long"
        );
        let refs = [&flag];

        // Every width, including the ones where the clamp is doing all the
        // work: a description that starts at or left of its own spelling
        // has stopped being that spelling's description.
        for width in 10..=120 {
            let layout = section_layout(&refs, width, 0);
            assert!(
                layout.description > spelling_column(&flag, 0),
                "width {width}: column {} is not clear of the long column",
                layout.description
            );
            let lines = entity_line(&flag, false, width, true, layout);
            let head_column = spelling_column(&flag, 0);
            // Only the description's own lines — a head wide enough to
            // wrap contributes lines of its own, and those belong in the
            // head area by design.
            for (start, _) in description_starts(&lines) {
                assert!(
                    start > head_column,
                    "width {width}: a description at {start} is flush under or left \
                     of the spelling it belongs to, at {head_column}"
                );
            }
        }
    }

    /// Spec §9.3, the wiring half: which line draws in which level of the
    /// pane's neutral hierarchy, and — at **both** levels — that a label
    /// is drawn in exactly its own rule's style.
    ///
    /// The label half supersedes a pin that asserted the mismatch as
    /// correct. It checked the match on the group divider and then
    /// asserted the section header's label as `muted_bold` against its
    /// own plainly-styled rule, so the defect at the level the eye reads
    /// first was written into the test as the expected value — which is
    /// why it was reported twice from outside before anything here could
    /// see it. The match is now one property quantified over both levels
    /// rather than a fact stated about one of them, and bold is checked
    /// explicitly, since bold brightens a foreground on many terminals
    /// and recreates the mismatch through an attribute.
    ///
    /// Supersedes, too, the pin that named `Gray` and `DarkGray` directly. Those
    /// two are the whole of what the sixteen named colors offer below a
    /// default foreground, and `Gray` is *at* it — so the section rule
    /// read at the brightness of the pane borders around it and the
    /// hierarchy had two visible levels where it needs three.
    /// [`style`]'s own `the_three_neutral_levels_step_clearly_apart` pins
    /// the shades and their separation; this pins which line gets which.
    ///
    /// Asserted on the styles of the spans rather than on their text,
    /// because the text is identical by construction: both are runs of the
    /// same rule glyph, and only the style tells them apart.
    #[test]
    fn every_rule_label_is_drawn_in_its_own_rules_style() {
        let glyphs = crate::glyphs::UNICODE;
        let palette = style::Palette::extended();
        let header = heading_line_ruled("FLAGS", Some(3), 60, palette, glyphs);
        let divider = group_divider_line("Main operation mode", 60, palette, glyphs, true);

        let rule_style = |line: &Line<'static>| {
            line.spans
                .iter()
                .find(|s| s.content.starts_with(glyphs.rule) && s.content.chars().count() > 1)
                .map(|s| s.style)
                .expect("a rule run")
        };
        let label_style = |line: &Line<'static>, text: &str| {
            line.spans
                .iter()
                .find(|s| s.content.contains(text))
                .map(|s| s.style)
                .unwrap_or_else(|| panic!("a label span for {text:?}"))
        };

        let header_rule = rule_style(&header);
        let divider_rule = rule_style(&divider);
        assert_eq!(header_rule, style::section_rule(palette));
        assert_eq!(divider_rule, style::group_rule(palette));

        // The property, at both levels: a label is its own rule's style.
        for (level, line, text, rule) in [
            ("section header", &header, "FLAGS (3)", header_rule),
            (
                "group divider",
                &divider,
                "Main operation mode",
                divider_rule,
            ),
        ] {
            let label = label_style(line, text);
            assert_eq!(
                label, rule,
                "the {level}'s label must be drawn in its own rule's style"
            );
            assert!(
                !label.add_modifier.contains(ratatui::style::Modifier::BOLD),
                "the {level}'s label is bold, which brightens it away from its rule"
            );
        }

        // The two levels still differ from each other — a matched label is
        // not an excuse for one flat shade over the whole pane.
        assert_ne!(
            header_rule, divider_rule,
            "the two rules must not read as one weight"
        );

        // Neither level is dimmed: the ordering must survive a terminal
        // that drops the attribute entirely (spec §9.2).
        for (name, style) in [("header", header_rule), ("divider", divider_rule)] {
            assert!(
                !style.add_modifier.contains(ratatui::style::Modifier::DIM),
                "the {name} rule leans on DIM"
            );
        }

        // Shape, not weight, is what keeps the two readable apart once
        // every attribute is stripped.
        assert!(text_of(&header).starts_with("FLAGS (3) "));
        assert!(text_of(&divider).starts_with("Main operation mode "));
    }

    /// Spec §9.3: a divider that opens its section renders its label alone
    /// at column 0 — the section header a line above already drew the
    /// rule, and a second full-width rule beneath it reads as one doubled
    /// line.
    #[test]
    fn a_section_opening_divider_carries_no_rule() {
        let mut flags = Vec::new();
        for (group, name) in [("Operation:", "create"), ("Devices:", "file")] {
            let mut f = Entity::flag_long(name, Provenance::single(Source::HelpText));
            f.group = Some(group.to_string());
            f.description = Some(Text::sanitize("does a thing"));
            flags.push(f);
        }
        let refs: Vec<&Entity> = flags.iter().collect();
        let lines = section_lines(
            &refs,
            60,
            0,
            style::Palette::extended(),
            None,
            crate::glyphs::UNICODE,
        )
        .lines;
        let text: Vec<String> = lines.iter().map(text_of).collect();

        assert_eq!(
            text[0], "Operation",
            "the opening divider is its label alone: {text:?}"
        );
        let later = text
            .iter()
            .find(|t| t.contains("Devices"))
            .expect("a second divider");
        assert!(
            later.starts_with("Devices ─") && later.ends_with('─'),
            "a later divider keeps its rule, behind its own label: {later:?}"
        );
    }

    /// Spec §9.3: the shared column is fitted to roughly the p90 spelling
    /// width — "the majority, not the outliers" — so the widest tenth of a
    /// section hangs instead of setting a column for everyone else.
    ///
    /// Measured on the column arithmetic rather than the rendered text,
    /// because the failure this rules out is a column *number* that one
    /// entity chose: nine short spellings and one long one produce the
    /// same column as nine short spellings alone.
    #[test]
    fn the_shared_column_fits_the_majority_not_the_widest() {
        let mk = |long: &str| {
            let mut f = Entity::flag_long(long, Provenance::single(Source::HelpText));
            f.description = Some(Text::sanitize("zzz something worth reading"));
            f
        };
        // Nine spellings of the same modest width, and one much wider —
        // wide enough to matter, narrow enough that the 45% pane cap
        // (spec §9.1a) still admits it, so the percentile is the only
        // thing that can be excluding it.
        let mut flags: Vec<Entity> = (0..9).map(|i| mk(&format!("opt-{i}"))).collect();
        let short_only: Vec<&Entity> = flags.iter().collect();
        let narrow = section_layout(&short_only, 100, 0);

        flags.push(mk("a-considerably-wider-option-name"));
        let with_outlier: Vec<&Entity> = flags.iter().collect();
        // Measured where the row actually starts — these are long-only
        // spellings, so each is preindented to the long column (spec
        // §9.3) and the cap sees that width, not the bare text's.
        let widest = spelling_column(&flags[9], 0) + display_width(&entity_name_spec(&flags[9]));
        assert!(
            widest + 2 <= 100 * DESC_COLUMN_CAP_PERCENT / 100,
            "the outlier must be inside the pane cap, or this measures the cap"
        );
        assert_eq!(
            section_layout(&with_outlier, 100, 0),
            narrow,
            "the widest tenth must not set the column"
        );

        // ...and the exclusion is a percentile, not a rule about single
        // rows: once the wide spellings *are* the majority they win it.
        let mut wide: Vec<Entity> = (0..9)
            .map(|i| mk(&format!("a-considerably-wider-option-{i}")))
            .collect();
        wide.push(mk("opt-0"));
        let wide_refs: Vec<&Entity> = wide.iter().collect();
        assert_ne!(
            section_layout(&wide_refs, 100, 0),
            narrow,
            "a majority of wide spellings must set a wide column"
        );
    }

    /// A repeatable positional renders the POSIX synopsis ellipsis
    /// (spec §9.3). The parser already reads `repeatable` off the `...`
    /// the tool printed; the pane was dropping it, so `grep`'s `FILE` and
    /// a single-file positional looked identical in POSITIONALS.
    #[test]
    fn a_repeatable_positional_renders_its_ellipsis() {
        let mut once = Entity::positional("PATTERNS", Provenance::single(Source::HelpText));
        once.required = true;
        let mut many = Entity::positional("FILE", Provenance::single(Source::HelpText));
        many.repeatable = true;
        assert_eq!(entity_name_spec(&once), "PATTERNS");
        assert_eq!(entity_name_spec(&many), "FILE...");
    }

    /// The anti-case: `repeatable` is one field for two kinds, and only
    /// the positional's notation is an ellipsis on the name. A repeatable
    /// *flag* (`-v -v -v`) must not grow a `...` — that would render a
    /// spelling nobody can type.
    #[test]
    fn a_repeatable_flag_gets_no_ellipsis() {
        let mut verbose = Entity::flag_long("verbose", Provenance::single(Source::HelpText));
        verbose.repeatable = true;
        assert_eq!(entity_name_spec(&verbose), "--verbose");
    }

    /// The ellipsis is measured as part of the head, not drawn past it:
    /// a name the section's column was fitted to must not overrun that
    /// column by the three characters the pane added after measuring.
    #[test]
    fn the_ellipsis_is_charged_to_the_row_that_carries_it() {
        let mut many = Entity::positional("FILE", Provenance::single(Source::HelpText));
        many.repeatable = true;
        let mut once = Entity::positional("FILE", Provenance::single(Source::HelpText));
        once.repeatable = false;
        assert_eq!(
            entity_head_width(&many, POSITIONAL_INDENT),
            entity_head_width(&once, POSITIONAL_INDENT) + 3
        );
    }

    /// Every section computes its own column (spec §9.3): a wide
    /// positional name must not push the flag list's descriptions right.
    ///
    /// The column is read off the flag's own rendered row rather than
    /// through `description_columns`, which also reports the section
    /// headings (they are prose at column 0) — a set-membership assertion
    /// over that helper's output is satisfied by the heading alone and
    /// would pass however the columns were computed.
    #[test]
    fn each_section_computes_its_own_column() {
        /// The column `needle`'s row starts its description at.
        fn column_of(built: &BuiltLines, needle: &str) -> usize {
            let row = built
                .lines
                .iter()
                .map(text_of)
                .find(|t| t.contains(needle) && t.contains("zzz"))
                .unwrap_or_else(|| panic!("no described row for {needle}"));
            let at = row.find("zzz").expect("checked above");
            display_width(&row[..at])
        }

        let mut node = CommandNode::new("tool", Provenance::single(Source::HelpText));
        let mut flag = Entity::flag_long("all", Provenance::single(Source::HelpText));
        flag.description = Some(Text::sanitize("zzz include everything"));
        node.entities.push(flag);
        let flags_only = build_lines(
            &node,
            false,
            80,
            style::Palette::extended(),
            None,
            crate::glyphs::UNICODE,
            &test_app(),
        );

        // Wide enough to move a shared column well clear of the flag
        // section's own, and narrow enough that the 45% pane cap still
        // admits it — a name the cap excludes would be excluded from a
        // shared column too, and this test would then pass either way.
        let mut positional = Entity::positional(
            "a-long-positional-name",
            Provenance::single(Source::HelpText),
        );
        positional.description = Some(Text::sanitize("zzz the thing to operate on"));
        node.entities.insert(0, positional);
        let both = build_lines(
            &node,
            false,
            80,
            style::Palette::extended(),
            None,
            crate::glyphs::UNICODE,
            &test_app(),
        );

        assert_eq!(
            column_of(&both, "--all"),
            column_of(&flags_only, "--all"),
            "the flag section's column moved when a positional was added"
        );
        assert!(
            column_of(&both, "a-long-positional-name") > column_of(&both, "--all"),
            "the fixture must actually have two different columns to tell apart"
        );
    }

    /// Spec §9.3: modifiers render as their own counted section, between
    /// FLAGS and ENVIRONMENT, at the content edge rather than POSITIONALS'
    /// inset — a bare letter, its operand one space behind it, and the
    /// section's own shared description column.
    ///
    /// [`LIST_SECTIONS`] claims this pane needs no per-kind branch, and a
    /// kind arriving from the parser for the first time is when that claim
    /// is either true or isn't. `ar` is the specimen; the letters are its
    /// own.
    #[test]
    fn modifiers_render_as_their_own_section() {
        let mut node = CommandNode::new("ar", Provenance::single(Source::HelpText));
        let mut flag = Entity::flag_long("thin", Provenance::single(Source::HelpText));
        flag.description = Some(Text::sanitize("make a thin archive"));
        node.entities.push(flag);
        for (letter, description) in [('v', "be verbose"), ('S', "do not build a symbol table")] {
            let mut m = Entity::modifier(letter, Provenance::single(Source::HelpText));
            m.description = Some(Text::sanitize(description));
            node.entities.push(m);
        }
        let mut valued = Entity::modifier('l', Provenance::single(Source::HelpText));
        valued.value_name = Some("<text>".into());
        valued.value_kind = mandible_core::ValueKind::Required;
        valued.description = Some(Text::sanitize("specify the dependencies"));
        node.entities.push(valued);

        let built = build_lines(
            &node,
            false,
            80,
            style::Palette::extended(),
            None,
            crate::glyphs::UNICODE,
            &test_app(),
        );
        let text: Vec<String> = built.lines.iter().map(text_of).collect();

        let header = text
            .iter()
            .position(|l| l.starts_with("MODIFIERS (3)"))
            .unwrap_or_else(|| panic!("no MODIFIERS header: {text:#?}"));
        let flags_header = text
            .iter()
            .position(|l| l.starts_with("FLAGS ("))
            .expect("FLAGS header");
        assert!(flags_header < header, "MODIFIERS must follow FLAGS");

        // The letter is bare — no dash invented for it — and sits at the
        // content edge, not POSITIONALS' inset.
        let row = text
            .iter()
            .find(|l| l.contains("be verbose"))
            .unwrap_or_else(|| panic!("no row for [v]: {text:#?}"));
        assert!(
            row.starts_with('v'),
            "modifier row not at the edge: {row:?}"
        );
        assert!(!row.contains("-v"), "a dash was invented: {row:?}");

        // The operand renders one space behind the letter it belongs to.
        assert!(
            text.iter().any(|l| l.contains("l <text>")),
            "operand not rendered with its letter: {text:#?}"
        );

        // One logical row per modifier, same as every other list section.
        assert_eq!(built.rows.len(), 4);
    }

    /// Spec §9.3: environment variables render as their own counted
    /// section, after MODIFIERS, at the content edge rather than
    /// POSITIONALS' inset — the same claim `modifiers_render_as_their_own_section`
    /// pins, exercised for the fourth and last `EntityKind`.
    #[test]
    fn env_vars_render_as_their_own_section() {
        let mut node = CommandNode::new("node", Provenance::single(Source::HelpText));
        let mut flag = Entity::flag_long("version", Provenance::single(Source::HelpText));
        flag.description = Some(Text::sanitize("print node's version"));
        node.entities.push(flag);
        for (name, description) in [
            ("NODE_DEBUG", "list of core modules to debug"),
            ("NO_COLOR", "alias for NODE_DISABLE_COLORS"),
        ] {
            let mut v = Entity::env_var_item(name, Provenance::single(Source::HelpText));
            v.description = Some(Text::sanitize(description));
            node.entities.push(v);
        }

        let built = build_lines(
            &node,
            false,
            80,
            style::Palette::extended(),
            None,
            crate::glyphs::UNICODE,
            &test_app(),
        );
        let text: Vec<String> = built.lines.iter().map(text_of).collect();

        let header = text
            .iter()
            .position(|l| l.starts_with("ENVIRONMENT (2)"))
            .unwrap_or_else(|| panic!("no ENVIRONMENT header: {text:#?}"));
        let flags_header = text
            .iter()
            .position(|l| l.starts_with("FLAGS ("))
            .expect("FLAGS header");
        assert!(flags_header < header, "ENVIRONMENT must follow FLAGS");

        // The name is bare — no dash invented for it — and sits at the
        // content edge, not POSITIONALS' inset.
        let row = text
            .iter()
            .find(|l| l.contains("alias for NODE_DISABLE_COLORS"))
            .unwrap_or_else(|| panic!("no row for NO_COLOR: {text:#?}"));
        assert!(
            row.starts_with("NO_COLOR"),
            "env var row not at the edge: {row:?}"
        );
        assert!(!row.contains("-NO_COLOR"), "a dash was invented: {row:?}");

        assert_eq!(built.rows.len(), 3);
    }

    /// Spec §9.3: a wrapped entry is **one logical row** for selection and
    /// scroll math, however many screen lines its description takes.
    ///
    /// The bug class this pins is the unbounded detail-pane scroll: a pane
    /// that counts rows where it renders lines (or the reverse) runs off
    /// the end of its own content by exactly the number of wraps on
    /// screen. So all three numbers are checked against each other — one
    /// row per entity, every rendered line accounted for by exactly one
    /// row or by the section furniture, and a scroll extent taken from the
    /// lines rather than the rows.
    #[test]
    fn a_wrapped_entry_is_one_logical_row() {
        let mut node = CommandNode::new("tool", Provenance::single(Source::HelpText));
        for i in 0..12 {
            let mut f = Entity::flag_long(
                format!("option-number-{i}"),
                Provenance::single(Source::HelpText),
            );
            // Long enough to wrap several times in a narrow pane, so rows
            // and lines cannot coincidentally agree.
            f.description = Some(Text::sanitize(
                "a description long enough that it has to wrap onto several \
                 further lines before it runs out of words to say",
            ));
            node.entities.push(f);
        }
        let width = 46;
        let app = test_app();
        let built = build_lines(
            &node,
            false,
            width,
            style::Palette::extended(),
            None,
            crate::glyphs::UNICODE,
            &app,
        );

        assert_eq!(built.rows.len(), 12, "one logical row per entity");
        assert!(
            built.rows.iter().any(|r| r.lines > 1),
            "the fixture must actually wrap, or this proves nothing"
        );
        // Rows are disjoint, ordered, and inside the document.
        let mut next = built.rows[0].first_line;
        for row in &built.rows {
            assert_eq!(
                row.first_line, next,
                "rows must not overlap or gap: {row:?}"
            );
            assert!(row.lines >= 1);
            next = row.first_line + row.lines;
        }
        assert!(next <= built.lines.len());

        // Scroll math is in lines, and stops at the end of the content —
        // never at the end of the rows, which would leave the last
        // entity's wrapped tail unreachable, and never past it.
        let viewport = 10;
        let mut scroller = test_app();
        scroller.set_detail_extent(built.lines.len(), viewport);
        for _ in 0..(built.lines.len() + 20) {
            scroller.detail_scroll_down();
        }
        assert_eq!(
            scroller.clamped_detail_scroll(),
            built.lines.len() - viewport,
            "scrolling must stop with the last line on screen"
        );
    }

    /// The scroll a search target produces is bounded by the same extent
    /// the user's own scrolling is (spec §9.3's scroll math). Targeting
    /// the last flag of a long list used to set the offset to that flag's
    /// line with no clamp at all, scrolling the document off the top.
    #[test]
    fn a_search_target_near_the_end_does_not_scroll_past_it() {
        let mut node = CommandNode::new("tool", Provenance::single(Source::HelpText));
        for i in 0..40 {
            let mut f =
                Entity::flag_long(format!("option-{i}"), Provenance::single(Source::HelpText));
            f.description = Some(Text::sanitize("something"));
            node.entities.push(f);
        }
        let target = FlagKey::Long("option-39".to_string());
        let built = build_lines(
            &node,
            false,
            60,
            style::Palette::extended(),
            Some(&target),
            crate::glyphs::UNICODE,
            &test_app(),
        );
        let line = built.target_flag_line.expect("the flag should be found");
        let viewport = 10;
        let scroll = target_scroll(&built, line, viewport);
        assert!(
            scroll <= built.lines.len() - viewport,
            "scrolled past the end: {scroll} of {} lines",
            built.lines.len()
        );
        assert!(
            line >= scroll && line < scroll + viewport,
            "the targeted row must still be on screen"
        );
    }

    /// A row already wholly visible scrolls nothing: throwing away the
    /// DESCRIPTION above a flag that was on screen anyway is a worse
    /// answer than leaving the pane where it is.
    #[test]
    fn a_search_target_already_on_screen_does_not_scroll() {
        let node = node_with_flags();
        let target = FlagKey::Long("interactive".to_string());
        let built = build_lines(
            &node,
            false,
            80,
            style::Palette::extended(),
            Some(&target),
            crate::glyphs::UNICODE,
            &test_app(),
        );
        let line = built.target_flag_line.expect("the flag should be found");
        assert_eq!(target_scroll(&built, line, 40), 0);
    }

    /// Closing spec §10's open item: selecting a flag via search must
    /// scroll the detail pane to that exact flag's line.
    #[test]
    fn selected_flag_reports_its_own_line_index() {
        let node = node_with_flags();
        let built = build_lines(
            &node,
            false,
            80,
            style::Palette::extended(),
            Some(&FlagKey::Long("interactive".to_string())),
            crate::glyphs::UNICODE,
            &test_app(),
        );
        let idx = built.target_flag_line.expect("flag should be found");
        let line_text = text_of(&built.lines[idx]);
        assert!(line_text.contains("--interactive"), "{line_text:?}");
    }

    #[test]
    fn no_target_flag_means_no_scroll_override() {
        let node = node_with_flags();
        let built = build_lines(
            &node,
            false,
            80,
            style::Palette::extended(),
            None,
            crate::glyphs::UNICODE,
            &test_app(),
        );
        assert_eq!(built.target_flag_line, None);
    }

    /// A node carrying all four entity kinds — flag, positional, modifier,
    /// environment variable — used to prove the search-target scroll works
    /// identically across kinds rather than being flag-specific.
    fn node_with_all_entity_kinds() -> CommandNode {
        let mut n = CommandNode::new("ar", Provenance::single(Source::HelpText));
        n.summary = Some(Text::sanitize("create, modify, and extract archives"));

        let mut positional = Entity::positional("archive", Provenance::single(Source::HelpText));
        positional.description = Some(Text::sanitize("the archive file"));
        n.entities.push(positional);

        let mut flag = Entity::flag_long("help", Provenance::single(Source::HelpText));
        flag.description = Some(Text::sanitize("Show help"));
        n.entities.push(flag);

        let mut modifier = Entity::modifier('d', Provenance::single(Source::HelpText));
        modifier.description = Some(Text::sanitize("delete a member from the archive"));
        n.entities.push(modifier);

        let mut env_var =
            Entity::env_var_item("AR_TIMESTAMP", Provenance::single(Source::HelpText));
        env_var.description = Some(Text::sanitize("override the archive's timestamp"));
        n.entities.push(env_var);

        n
    }

    /// Closing spec §10's open item for the dashless kinds: selecting a
    /// *modifier* search result must scroll the detail pane to that exact
    /// modifier's own row in its MODIFIERS section, exactly as a flag
    /// search result does for FLAGS.
    #[test]
    fn selected_modifier_reports_its_own_line_index() {
        let node = node_with_all_entity_kinds();
        let built = build_lines(
            &node,
            false,
            80,
            style::Palette::extended(),
            Some(&FlagKey::Name("d".to_string())),
            crate::glyphs::UNICODE,
            &test_app(),
        );
        let idx = built.target_flag_line.expect("modifier should be found");
        let line_text = text_of(&built.lines[idx]);
        assert!(line_text.contains('d'), "{line_text:?}");
        // The targeted row must actually be the MODIFIERS row, not the
        // FLAGS or POSITIONALS row that happens to render first — proven
        // by checking the row lands after the MODIFIERS heading and before
        // the ENVIRONMENT heading.
        let modifiers_heading = built
            .lines
            .iter()
            .position(|l| text_of(l).contains("MODIFIERS"))
            .expect("MODIFIERS heading should render");
        let environment_heading = built
            .lines
            .iter()
            .position(|l| text_of(l).contains("ENVIRONMENT"))
            .expect("ENVIRONMENT heading should render");
        assert!(
            idx > modifiers_heading && idx < environment_heading,
            "modifier target line {idx} should land within MODIFIERS ({modifiers_heading}..{environment_heading})"
        );
    }

    /// Same as above, for an environment variable landing in its own
    /// ENVIRONMENT section.
    #[test]
    fn selected_env_var_reports_its_own_line_index() {
        let node = node_with_all_entity_kinds();
        let built = build_lines(
            &node,
            false,
            80,
            style::Palette::extended(),
            Some(&FlagKey::Name("AR_TIMESTAMP".to_string())),
            crate::glyphs::UNICODE,
            &test_app(),
        );
        let idx = built.target_flag_line.expect("env var should be found");
        let line_text = text_of(&built.lines[idx]);
        assert!(line_text.contains("AR_TIMESTAMP"), "{line_text:?}");
        let environment_heading = built
            .lines
            .iter()
            .position(|l| text_of(l).contains("ENVIRONMENT"))
            .expect("ENVIRONMENT heading should render");
        assert!(
            idx > environment_heading,
            "env var target line {idx} should land after the ENVIRONMENT heading ({environment_heading})"
        );
    }

    /// Same as above, for a positional landing in its own
    /// POSITIONALS section.
    #[test]
    fn selected_positional_reports_its_own_line_index() {
        let node = node_with_all_entity_kinds();
        let built = build_lines(
            &node,
            false,
            80,
            style::Palette::extended(),
            Some(&FlagKey::Name("archive".to_string())),
            crate::glyphs::UNICODE,
            &test_app(),
        );
        let idx = built.target_flag_line.expect("positional should be found");
        let line_text = text_of(&built.lines[idx]);
        assert!(line_text.contains("archive"), "{line_text:?}");
        let positionals_heading = built
            .lines
            .iter()
            .position(|l| text_of(l).contains("POSITIONALS"))
            .expect("POSITIONALS heading should render");
        let flags_heading = built
            .lines
            .iter()
            .position(|l| text_of(l).contains("FLAGS"))
            .expect("FLAGS heading should render");
        assert!(
            idx > positionals_heading && idx < flags_heading,
            "positional target line {idx} should land within POSITIONALS ({positionals_heading}..{flags_heading})"
        );
    }

    /// A `Long`/`Short` flag key must never accidentally land on a
    /// dashless row, even one whose bare name happens to equal the flag
    /// spelling being searched — e.g. a modifier named `help` would
    /// otherwise be indistinguishable from the `--help` flag by name
    /// alone. Regression coverage for the `matches_key` isolation rule
    /// (spec §10; `mandible-core`'s `entity.rs`).
    #[test]
    fn a_flag_key_does_not_land_on_a_dashless_row_of_the_same_name() {
        let mut node = node_with_all_entity_kinds();
        // Add a positional that happens to share its bare name with the
        // node's flag.
        let mut decoy = Entity::positional("help", Provenance::single(Source::HelpText));
        decoy.description = Some(Text::sanitize("a decoy positional named help"));
        node.entities.push(decoy);

        let built = build_lines(
            &node,
            false,
            80,
            style::Palette::extended(),
            Some(&FlagKey::Long("help".to_string())),
            crate::glyphs::UNICODE,
            &test_app(),
        );
        let idx = built.target_flag_line.expect("flag should be found");
        let flags_heading = built
            .lines
            .iter()
            .position(|l| text_of(l).contains("FLAGS"))
            .expect("FLAGS heading should render");
        let modifiers_heading = built
            .lines
            .iter()
            .position(|l| text_of(l).contains("MODIFIERS"))
            .expect("MODIFIERS heading should render");
        assert!(
            idx > flags_heading && idx < modifiers_heading,
            "the --help flag, not the decoy positional, should be targeted: {idx}"
        );
    }

    /// Render the whole frame in each of the verbatim view's three states.
    ///
    /// The state machine for `t` is unit-tested in `app`, but that proves
    /// only that the right variant is *selected*; this proves it reaches
    /// the screen. The `Failed` case matters most: a refusal that rendered
    /// as an empty pane would be indistinguishable from a tool that prints
    /// nothing, which is the exact confusion the view exists to remove.
    #[test]
    fn raw_mode_renders_each_state_to_the_screen() {
        use crate::app::{App, RawHelp};
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        fn screen(app: &App) -> String {
            let backend = TestBackend::new(80, 24);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| {
                    let area = frame.area();
                    render(frame, area, app);
                })
                .unwrap();
            terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|c| c.symbol())
                .collect::<Vec<_>>()
                .join("")
        }

        // A node that parses perfectly well, so anything verbatim on
        // screen can only have come from the raw path overriding it.
        let mut root = CommandNode::new("tool", Provenance::single(Source::HelpText));
        let mut flag = Entity::flag_long("verbose", Provenance::single(Source::HelpText));
        flag.description = Some(Text::sanitize("PARSED-FLAG-DESCRIPTION"));
        root.entities.push(flag);
        let mut app = App::new("tool".to_string(), root);
        let path = vec!["tool".to_string()];

        let parsed = screen(&app);
        assert!(parsed.contains("PARSED-FLAG-DESCRIPTION"), "{parsed}");

        app.toggle_raw_mode();

        app.mark_raw_pending(path.clone());
        let pending = screen(&app);
        assert!(pending.contains("verbatim"), "{pending}");
        assert!(pending.contains("running the probe"), "{pending}");

        app.set_raw_help(
            path.clone(),
            RawHelp::Ready(
                vec![Text::sanitize("RAW-HELP-LINE-FROM-THE-TOOL")],
                "git --help".to_string(),
            ),
        );
        let ready = screen(&app);
        assert!(ready.contains("RAW-HELP-LINE-FROM-THE-TOOL"), "{ready}");
        assert!(
            !ready.contains("PARSED-FLAG-DESCRIPTION"),
            "the parse must be replaced, not appended: {ready}"
        );

        app.set_raw_help(
            path.clone(),
            RawHelp::Failed("refused: REASON-SHOWN-TO-THE-USER".to_string()),
        );
        let failed = screen(&app);
        assert!(failed.contains("REASON-SHOWN-TO-THE-USER"), "{failed}");
        assert!(
            !failed.contains("PARSED-FLAG-DESCRIPTION"),
            "a refusal must not silently fall back to the parse: {failed}"
        );

        // And back, to prove the override is not one-way.
        app.toggle_raw_mode();
        let restored = screen(&app);
        assert!(restored.contains("PARSED-FLAG-DESCRIPTION"), "{restored}");
    }

    /// Batch 6 part 4 (spec §7 Tier B step 3): a node whose parse degraded
    /// to level 3 must render its `unparsed` text, labelled as such, via
    /// the whole-frame path — not the structured `build_lines` path (which
    /// a node with `unparsed` set should never even reach, since
    /// `unparsed`/`flags`/`subcommands`/`usage` are mutually exclusive by
    /// construction).
    #[test]
    fn unparsed_node_renders_labelled_raw_text() {
        use crate::app::App;
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let mut root = CommandNode::new(
            "mystery",
            Provenance::with_confidence(Source::HelpText, 0.0),
        );
        root.unparsed = vec![
            Text::sanitize("a friendly banner"),
            Text::sanitize("and nothing else"),
        ];
        let app = App::new("mystery".to_string(), root);

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                let area = frame.area();
                render(frame, area, &app);
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let rendered: String = buffer
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect::<Vec<_>>()
            .join("");
        assert!(rendered.contains("unparsed"), "{rendered}");
        assert!(rendered.contains("a friendly banner"), "{rendered}");
        assert!(rendered.contains("and nothing else"), "{rendered}");
    }

    /// The verbatim fallback draws the author's own columns (spec §4.1's
    /// layout tier) and, being preformatted content whose layout is not
    /// mandible's, participates in `[ui] horizontal_scroll` by the same
    /// path the raw view does — no second mechanism.
    ///
    /// `ar`'s shape: `mandible ar`, then any subcommand. Every one of
    /// them degrades to verbatim, and the padded `-` column that lines
    /// the descriptions up was being collapsed away before it reached
    /// here.
    #[test]
    fn verbatim_fallback_draws_aligned_columns_and_scrolls_sideways() {
        use crate::app::App;
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        // Narrow enough that the padded rows overrun the pane, which is
        // the only condition under which there is any horizontal scroll
        // to exercise.
        fn screen(app: &App) -> String {
            let backend = TestBackend::new(40, 24);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| {
                    let area = frame.area();
                    render(frame, area, app);
                })
                .unwrap();
            terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|c| c.symbol())
                .collect::<Vec<_>>()
                .join("")
        }

        let mut root = CommandNode::new("ar", Provenance::with_confidence(Source::HelpText, 0.0));
        root.unparsed = [
            " commands:",
            "  d            - delete file(s) from the archive",
            "  m[ab]        - move file(s) in the archive",
        ]
        .iter()
        .map(|l| Text::sanitize_preserving_layout(l))
        .collect();
        let mut app = App::new("ar".to_string(), root);
        app.horizontal_scroll_enabled = true;

        let rendered = screen(&app);
        assert!(
            rendered.contains("  d            - delete"),
            "the author's own column must survive to the screen: {rendered}"
        );
        assert!(rendered.contains("  m[ab]        - move"), "{rendered}");

        // Same content, scrolled: the fallback flows through the existing
        // preformatted-content path, so `→` moves it rather than doing
        // nothing (which is what a fallback with its own renderer would).
        app.detail_hscroll_right();
        let scrolled = screen(&app);
        assert!(
            !scrolled.contains("  d            - delete"),
            "a scrolled pane must not still show column 0: {scrolled}"
        );
        assert!(
            scrolled.contains("- delete file(s) from the"),
            "the row itself is unchanged, only its window moved: {scrolled}"
        );
    }

    /// Every form's rendered padding, for a tool's own synopsis block.
    fn pads(name: &str, forms: &[&str]) -> Vec<usize> {
        let usage: Vec<Text> = forms
            .iter()
            .map(|f| Text::sanitize_preserving_layout(f))
            .collect();
        usage_forms(name, &usage)
            .into_iter()
            .map(|(pad, _)| pad)
            .collect()
    }

    /// `ip` lines its second invocation form up under the first, against
    /// the `Usage: ` label it printed in front of the first. The heading
    /// supplies that label, so the pane drops it — and dropping it must
    /// take the second form's indentation with it, or the two forms come
    /// out seven columns apart from each other having been drawn flush.
    #[test]
    fn the_label_width_is_compensated_so_forms_stay_as_drawn() {
        assert_eq!(
            pads(
                "ip",
                &[
                    "Usage: ip [ OPTIONS ] OBJECT { COMMAND | help }",
                    "       ip [ -force ] -batch filename",
                ]
            ),
            vec![0, 0],
            "both forms were drawn flush and must render flush"
        );
        let usage: Vec<Text> = ["Usage: ip [ OPTIONS ] OBJECT { COMMAND | help }"]
            .iter()
            .map(|f| Text::sanitize_preserving_layout(f))
            .collect();
        assert_eq!(
            usage_forms("ip", &usage)[0].1,
            "ip [ OPTIONS ] OBJECT { COMMAND | help }",
            "the label itself is still dropped"
        );
    }

    /// A form the author indented *deeper* than the first stays deeper by
    /// exactly that much — the compensation is a shift of the whole block,
    /// never a flattening of it.
    #[test]
    fn a_form_indented_deeper_than_the_first_stays_deeper() {
        assert_eq!(
            pads(
                "prog",
                &[
                    "Usage: prog build [OPTIONS]",
                    "       prog test [OPTIONS]",
                    "           prog test --only NAME",
                ]
            ),
            vec![0, 0, 4]
        );
    }

    /// The clamp. `du` draws its second form against a two-column `  or:`
    /// marker, which is fewer columns than the seven `Usage: ` occupied,
    /// so the compensation would push it negative. It clamps at the block
    /// indent instead of wrapping around or panicking.
    #[test]
    fn a_form_indented_less_than_the_label_clamps_at_zero() {
        assert_eq!(
            pads(
                "du",
                &[
                    "Usage: du [OPTION]... [FILE]...",
                    "  or:  du [OPTION]... --files0-from=F",
                ]
            ),
            vec![0, 0]
        );
    }

    /// A single-form tool is the common case and must be untouched by any
    /// of this: one form, no compensation to make, flush at the block
    /// indent whether or not it carried a label.
    #[test]
    fn a_single_form_tool_renders_flush_at_the_block_indent() {
        assert_eq!(pads("tar", &["Usage: tar [OPTION...] [FILE]..."]), vec![0]);
        assert_eq!(pads("mytool", &["mytool [OPTIONS] FILE"]), vec![0]);
        // Including one the tool itself indented: with nothing to align
        // against, the author's margin is not alignment and the form sits
        // at the block indent like any other.
        assert_eq!(pads("mytool", &["    mytool [OPTIONS] FILE"]), vec![0]);
    }

    /// The reported defect: cobra prints the *full* command path in its
    /// usage line, not just the leaf node's own name — `docker import
    /// --help` yields `Usage:  docker import [OPTIONS] file|URL|-
    /// [REPOSITORY[:TAG]]`. The old guard only checked the usage text's
    /// first word ("docker" ≠ "import"), so it prepended the leaf name
    /// anyway and produced `import docker import [OPTIONS] file|URL|-
    /// [REPOSITORY[:TAG]]` — the name doubled and the real command path
    /// pushed off the front. The correct output is the tool's own line,
    /// byte for byte.
    #[test]
    fn a_usage_form_does_not_prepend_when_the_full_path_already_names_the_node() {
        assert_eq!(
            usage_form(
                "import",
                "docker import [OPTIONS] file|URL|- [REPOSITORY[:TAG]]"
            )
            .1,
            "docker import [OPTIONS] file|URL|- [REPOSITORY[:TAG]]"
        );
        // Same shape, a second real tool (docker pull), so this isn't
        // one coincidental fixture.
        assert_eq!(
            usage_form("pull", "docker pull [OPTIONS] NAME[:TAG|@DIGEST]").1,
            "docker pull [OPTIONS] NAME[:TAG|@DIGEST]"
        );
        // argparse does the same thing, and for a node three levels deep
        // the leading run is three words wide, not one — the fix has to
        // scan the whole run, not just swap which single word it checks.
        assert_eq!(
            usage_form("outlier", "smokecli columns outlier [-h] [-v] [-n]").1,
            "smokecli columns outlier [-h] [-v] [-n]"
        );
    }

    /// The other direction, which is why the fix can't just delete the
    /// prepending: some tools print usage with no command name in it at
    /// all (`Usage: [OPTIONS] FILE`), and mandible adds the name so the
    /// line reads as a complete, copy-pasteable invocation. Here the
    /// node's name genuinely does not appear anywhere in the usage text,
    /// so it must still be prepended.
    #[test]
    fn a_usage_form_still_prepends_when_the_name_is_truly_absent() {
        assert_eq!(
            usage_form("mytool", "[OPTIONS] FILE").1,
            "mytool [OPTIONS] FILE"
        );
        assert_eq!(usage_form("cat", "<url>").1, "cat <url>");
    }

    /// A single over-long token must survive wrapping intact — broken
    /// across as many lines as it takes, never truncated. Concatenating
    /// every chunk this function returns must reconstruct the original
    /// word exactly; losing a suffix here is exactly what shipped as
    /// `smokecli unbreakable url` rendering a ~150-character URL as
    /// `https://registry.example.com/v2/org…` with everything past
    /// `/v2/org` gone from the parsed view.
    #[test]
    fn wrap_words_breaks_an_overlong_token_instead_of_losing_it() {
        let url = "https://registry.example.com/v2/org/repo/blobs/uploads/deadbeefcafefeed0123456789abcdef0123456789abcdef0123456789abcd?query=value&more=stuff";
        let chunks = wrap_words(url, 20);
        assert!(chunks.len() > 1, "expected multiple chunks: {chunks:?}");
        let rejoined: String = chunks.concat();
        assert_eq!(rejoined, url, "the token must survive intact");
        for chunk in &chunks {
            assert!(
                display_width(chunk) <= 20,
                "chunk exceeds the budget: {chunk:?}"
            );
        }
        // Nothing here is a hard-truncation ellipsis marker.
        assert!(!rejoined.contains('…'));
    }

    /// [`break_overlong_word`] must split only at character boundaries —
    /// never mid-character — even when the word is wide/emoji text, so
    /// display-width accounting (not byte or `char` count) is what decides
    /// where a line ends.
    #[test]
    fn break_overlong_word_never_splits_a_multibyte_character() {
        // Each CJK character is 2 columns wide; a budget of 3 must place
        // exactly one character per chunk; the whole string must survive.
        let word = "日本語文字列長い";
        let chunks = break_overlong_word(word, 3);
        let rejoined: String = chunks.concat();
        assert_eq!(rejoined, word);
        for chunk in &chunks {
            // Every chunk parses as valid UTF-8 chars by construction
            // (`String` guarantees it), so the real assertion is the
            // width budget: no chunk may smuggle a whole extra character
            // past it.
            assert!(display_width(chunk) <= 3, "chunk too wide: {chunk:?}");
        }
    }

    /// The wrap-mode rule for preformatted content, stated as the two
    /// halves it has to satisfy at once: a line that fits is untouched,
    /// and a line that does not is continued rather than cut.
    ///
    /// The second half is the defect this function was written for —
    /// `[ui] horizontal_scroll = false` used to hand the raw view's lines
    /// to a `Paragraph` with no `Wrap`, which ended each one at the pane's
    /// last column and dropped the rest with no indication at all.
    #[test]
    fn wrap_preformatted_keeps_short_lines_verbatim_and_continues_long_ones() {
        // Byte-identical when it fits, columns and all: this is `ar`'s
        // padded command table, whose alignment is the author's own.
        let aligned = "  m[ab]        - move file(s) in the archive";
        assert_eq!(wrap_preformatted(aligned, 60), vec![aligned.to_string()]);
        // Exactly the width still fits — the cut is at wider-than, not
        // at as-wide-as.
        assert_eq!(
            wrap_preformatted(aligned, display_width(aligned)),
            vec![aligned.to_string()]
        );

        let rows = wrap_preformatted(aligned, 24);
        assert!(rows.len() > 1, "an over-wide line must continue: {rows:?}");
        for row in &rows {
            assert!(display_width(row) <= 24, "row overruns the pane: {row:?}");
        }
        // Nothing lost: every non-whitespace character comes back in
        // order, whether the break fell on a space or inside a word.
        let squash = |s: &str| -> String { s.chars().filter(|c| !c.is_whitespace()).collect() };
        assert_eq!(squash(&rows.concat()), squash(aligned));
        // The author's own run of spaces survives inside a row rather
        // than being collapsed the way `wrap_words` would collapse it.
        assert!(
            rows[0].contains("m[ab]        -"),
            "interior columns were reflowed: {rows:?}"
        );
        // And the continuation carries the line's own indent.
        assert!(
            rows[1].starts_with("  ") && !rows[1].starts_with("   "),
            "{rows:?}"
        );
    }

    /// A line with no whitespace at all — the shape `wrap_words` handles
    /// with [`break_overlong_word`] — must be cut between characters and
    /// survive whole, never truncated to what fitted.
    #[test]
    fn wrap_preformatted_hard_cuts_a_line_with_nowhere_to_break() {
        let url = "https://registry.example.com/v2/org/repo/blobs/uploads/deadbeefcafefeed0123456789abcdef";
        let rows = wrap_preformatted(url, 20);
        assert!(rows.len() > 1, "{rows:?}");
        assert_eq!(rows.concat(), url, "the line must survive intact");
        for row in &rows {
            assert!(display_width(row) <= 20, "row overruns: {row:?}");
        }
    }

    /// Cuts land between characters, chosen by display width — so a
    /// double-width glyph is never split in half and never allowed to
    /// overflow the row it ends (the same property
    /// [`break_overlong_word`] holds, reached by a different path).
    #[test]
    fn wrap_preformatted_never_splits_a_double_width_character() {
        let cjk = "日本語のテキストで境界を壊すテスト文字列です";
        let rows = wrap_preformatted(cjk, 7);
        assert_eq!(rows.concat(), cjk);
        for row in &rows {
            assert!(display_width(row) <= 7, "row overruns: {row:?}");
        }
    }

    /// Degenerate inputs a real `--help` document contains: a blank line
    /// (which is layout and must keep its row), and an indent so deep it
    /// would leave no room for content, which drops the hanging indent
    /// rather than emitting a tall column of near-empty rows.
    #[test]
    fn wrap_preformatted_handles_blank_lines_and_a_pane_swallowing_indent() {
        assert_eq!(wrap_preformatted("", 20), vec![String::new()]);
        assert_eq!(wrap_preformatted("   ", 20), vec!["   ".to_string()]);

        let deep = format!("{}some text that has to go somewhere", " ".repeat(18));
        let rows = wrap_preformatted(&deep, 20);
        for row in &rows {
            assert!(display_width(row) <= 20, "row overruns: {row:?}");
        }
        assert!(
            rows[1..].iter().all(|row| !row.starts_with("   ")),
            "an indent wider than half the pane must not be carried: {rows:?}"
        );
        let squash = |s: &str| -> String { s.chars().filter(|c| !c.is_whitespace()).collect() };
        assert_eq!(squash(&rows.concat()), squash(&deep));
    }

    /// The end-to-end path for the reported repro: a node whose `usage`
    /// carries an over-long token must still show the whole token
    /// somewhere in the rendered lines, and never emit an ellipsis in its
    /// place.
    ///
    /// Pinned with the horizontal-scroll toggle explicitly **off**: this is
    /// the pre-existing wrapping behavior, and `horizontal_scroll = false`
    /// (spec: the config toggle for this feature) must reproduce it
    /// exactly. The toggle **on** has its own test below, where the same
    /// token stays on one unwrapped line instead.
    #[test]
    fn build_lines_wraps_rather_than_truncates_a_long_usage_token_with_scroll_disabled() {
        let mut node = CommandNode::new("url", Provenance::single(Source::HelpText));
        let long_url = "https://registry.example.com/v2/org/repo/blobs/uploads/deadbeefcafefeed0123456789abcdef0123456789abcdef0123456789abcd";
        node.usage = vec![Text::sanitize(long_url)];

        let mut app = test_app();
        app.horizontal_scroll_enabled = false;
        let built = build_lines(
            &node,
            false,
            46,
            style::Palette::extended(),
            None,
            crate::glyphs::UNICODE,
            &app,
        );
        // Every usage line carries its own 2-space block indent (see the
        // USAGE section of `build_lines`) — strip it per line before
        // rejoining so adjacent chunks of the broken token reassemble
        // without a spurious gap between them.
        let joined: String = built
            .lines
            .iter()
            .map(text_of)
            .map(|t| t.trim_start().to_string())
            .collect();
        // The chunks concatenate back to the original token exactly, so
        // the whole URL — not just a fragment of it — must appear intact
        // somewhere in the rendered output.
        assert!(
            joined.contains(long_url),
            "token was lost, not wrapped: {joined:?}"
        );
        assert!(
            !joined.contains('…'),
            "an over-long token must never be ellipsis-truncated: {joined:?}"
        );
        assert!(
            built.lines.len() > 1,
            "with scrolling disabled the long token should still wrap across lines: {:?}",
            built.lines.iter().map(text_of).collect::<Vec<_>>()
        );
    }

    /// The toggle **on** (the default): a USAGE synopsis is preformatted
    /// and must stay on one line rather than being greedily word-wrapped,
    /// with `h`/`l` revealing the rest instead (spec §9: preformatted
    /// detail-pane content scrolls rather than wraps).
    #[test]
    fn usage_synopsis_stays_on_one_line_when_horizontal_scroll_is_enabled() {
        let mut node = CommandNode::new("url", Provenance::single(Source::HelpText));
        let long_url = "https://registry.example.com/v2/org/repo/blobs/uploads/deadbeefcafefeed0123456789abcdef0123456789abcdef0123456789abcd";
        node.usage = vec![Text::sanitize(long_url)];

        let app = test_app();
        assert!(app.horizontal_scroll_enabled, "default is on");
        let width = 46;
        let built = build_lines(
            &node,
            false,
            width,
            style::Palette::extended(),
            None,
            crate::glyphs::UNICODE,
            &app,
        );
        // Unscrolled, the line shows a `width`-column prefix of the
        // synopsis — the rest reachable with `l`, never reflowed onto a
        // second line the way the disabled path wraps it.
        let usage_lines: Vec<&Line> = built
            .lines
            .iter()
            .filter(|l| text_of(l).trim_start().starts_with("url https"))
            .collect();
        assert_eq!(
            usage_lines.len(),
            1,
            "a preformatted synopsis must not be split across lines: {:?}",
            built.lines.iter().map(text_of).collect::<Vec<_>>()
        );
        let shown = text_of(usage_lines[0]);
        // The clip marker lives in the padding gutter (see
        // `draw_clip_marker_rails`), never inside the text, so the line
        // itself is a clean unbroken prefix at full width.
        assert!(
            long_url.starts_with(shown.trim_start().trim_start_matches("url ")),
            "the visible portion must be an unbroken prefix of the real synopsis: {shown:?}"
        );
        assert!(
            display_width(&shown) <= width,
            "must not overflow the pane and rely on Paragraph::Wrap to save it: {shown:?}"
        );
    }

    /// Scrolling right trims preformatted USAGE content from the left —
    /// the same offset the affordance in the border reflects — while
    /// leaving everything else (here, nothing else on this node) alone.
    /// Two-pass, matching how a real frame works: the first pass tells
    /// `App` how wide the content is (`set_detail_hextent`), only after
    /// which a scroll key has something to clamp against.
    #[test]
    fn usage_synopsis_scrolls_horizontally_when_enabled() {
        let mut node = CommandNode::new("url", Provenance::single(Source::HelpText));
        let long_url = "x".repeat(200);
        node.usage = vec![Text::sanitize(&long_url)];

        let mut app = test_app();
        let _ = build_lines(
            &node,
            false,
            46,
            style::Palette::extended(),
            None,
            crate::glyphs::UNICODE,
            &app,
        );
        app.detail_hscroll_right();
        app.detail_hscroll_right();
        let built = build_lines(
            &node,
            false,
            46,
            style::Palette::extended(),
            None,
            crate::glyphs::UNICODE,
            &app,
        );
        let usage_line = built
            .lines
            .iter()
            .map(text_of)
            .find(|t| t.contains('x'))
            .expect("usage line should still be present");
        assert!(
            !usage_line.trim_start().starts_with("url xxxx"),
            "the line should have scrolled past its own start: {usage_line:?}"
        );
    }
}
