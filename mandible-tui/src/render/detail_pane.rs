//! The detail pane: breadcrumb header, description, flags grouped by
//! [`mandible_core::Entity::group`] (inherited flags in a final dimmed group),
//! and a provenance footer (spec §2, §9, §9.2).
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
use crate::sanitize::{defensive_single_line, display_width};
use crate::style;
use mandible_core::{CommandNode, Entity, FlagKey, Spelling, ValueKind};
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
        app.color_enabled,
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
    let scroll = built
        .target_flag_line
        .unwrap_or_else(|| app.clamped_detail_scroll()) as u16;
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
/// everything itself) — this is preformatted output, and re-wrapping it
/// would silently edit the tool author's own text. `h`/`l`/`←`/`→` scroll
/// it horizontally instead (spec §9: preformatted detail-pane content
/// scrolls rather than wraps); the important safety property — content
/// never reflows, and can therefore never smear into the pane border the
/// way an unsanitized newline once did (spec §9) — holds regardless of
/// which offset is showing. Safe to hand
/// straight to a `Span` because every `Text` reaching here already went
/// through one of `mandible_core::Text`'s sanitizing constructors —
/// `Text::sanitize` for `node.unparsed` (level-3 degradation), or
/// `Text::sanitize_preserving_layout` for the verbatim view's own lines
/// (`mandible-extract`'s `help_text::raw_help*`) — both of which guarantee
/// no embedded control characters or newlines reach here; they differ only
/// in whether whitespace/indentation is collapsed, never in that safety
/// property. The heading and the recognizable unverified-subcommand notice
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
            lines.push(Line::from(text));
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
}

fn build_lines(
    node: &CommandNode,
    show_hidden: bool,
    width: usize,
    color_enabled: bool,
    target_flag: Option<&FlagKey>,
    glyphs: Glyphs,
    app: &App,
) -> BuiltLines {
    let mut lines = Vec::new();
    let mut target_flag_line = None;
    let mut clip_rows: Vec<(usize, bool, bool)> = Vec::new();
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
        lines.push(Line::default());
    }

    if let Some(description) = &node.description {
        lines.push(heading_line_ruled(
            "DESCRIPTION",
            width,
            color_enabled,
            glyphs,
        ));
        for paragraph_text in description.as_str().split("\n\n") {
            for chunk in wrap_words(paragraph_text, width) {
                lines.push(Line::from(chunk));
            }
            lines.push(Line::default());
        }
    }

    if !node.usage.is_empty() {
        lines.push(heading_line_ruled("USAGE", width, color_enabled, glyphs));
        // Indented as a block, the way API documentation sets a signature
        // apart from its prose.
        let indent = "  ";
        if app.horizontal_scroll_enabled {
            // A synopsis is preformatted — spacing inside it is part of
            // its meaning, so spec §9 has it scroll rather than wrap. One
            // `Line` per usage form, never re-flowed; `h`/`l` reveal the rest instead
            // of the old greedy word-wrap eating it into a ragged block.
            let usage_lines: Vec<String> = node
                .usage
                .iter()
                .map(|u| format!("{indent}{}", usage_signature(&node.name, u.as_str())))
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
            for u in &node.usage {
                let full = usage_signature(&node.name, u.as_str());
                let avail = width.saturating_sub(display_width(indent)).max(1);
                for chunk in wrap_words(&full, avail) {
                    lines.push(Line::from(format!("{indent}{chunk}")));
                }
            }
        }
        lines.push(Line::default());
    }

    let visible_flags: Vec<&Entity> = node
        .flags()
        .filter(|f| show_hidden || (!f.hidden && f.deprecated.is_none()))
        .collect();

    if !visible_flags.is_empty() {
        lines.push(heading_line_ruled("FLAGS", width, color_enabled, glyphs));
        let (flag_lines_out, target) =
            flag_lines(&visible_flags, width, color_enabled, target_flag, glyphs);
        let base = lines.len();
        if let Some(t) = target {
            target_flag_line = Some(base + t);
        }
        lines.extend(flag_lines_out);
        lines.push(Line::default());
    }

    // Provenance is not rendered here at all any more: it describes where
    // this node's data came from, which belongs beside the pane rather than
    // inside its content. See `render::status_bar`.

    BuiltLines {
        lines,
        target_flag_line,
        clip_rows,
    }
}

/// A section heading followed by a rule to the pane's edge.
///
/// The rule is what gives the pane hierarchy: without it, a bold word and
/// the body text beneath it are two lines of similar weight, and the eye
/// has nothing to anchor a section boundary to. Drawn in the muted style so
/// it separates without competing, and through the glyph set so a
/// non-UTF-8 terminal gets `-` rather than tofu.
fn heading_line_ruled(
    text: &str,
    width: usize,
    color_enabled: bool,
    glyphs: Glyphs,
) -> Line<'static> {
    let heading = text.to_string();
    let used = display_width(&heading) + 1;
    let rule_width = width.saturating_sub(used);
    let mut spans = vec![Span::styled(heading, style::muted_bold(color_enabled))];
    if rule_width > 0 {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            glyphs.rule.repeat(rule_width),
            style::muted(color_enabled),
        ));
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
fn usage_signature(node_name: &str, usage: &str) -> String {
    let name = defensive_single_line(node_name);
    let mut text = defensive_single_line(usage);

    // Drop a leading `usage:` label, case-insensitively — the heading says
    // it.
    let trimmed = text.trim_start();
    if trimmed.len() >= 6 && trimmed[..6].eq_ignore_ascii_case("usage:") {
        text = trimmed[6..].trim_start().to_string();
    }

    if name.is_empty() || usage_names_the_node(&text, &name) {
        text
    } else {
        format!("{name} {text}")
    }
}

/// Whether `name` already appears among `text`'s leading run of bare
/// command-path words — see [`usage_signature`] for why the search covers
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

/// Strip a group heading's trailing colon and normalize its casing, so
/// carapace-sourced groups (already often plain, e.g. `"main"`) and
/// help-text-sourced groups (raw heading text, e.g. `"GLOBAL OPTIONS:"`
/// or `"Main operation mode:"`) render identically for the same logical
/// grouping instead of carrying their source's formatting quirks into the
/// UI verbatim.
fn normalize_group_heading(raw: &str) -> String {
    defensive_single_line(raw)
        .trim()
        .trim_end_matches(':')
        .trim()
        .to_uppercase()
}

/// Group flags by [`Flag::group`], with un-grouped flags first (under no
/// heading) and inherited flags always last as their own muted group,
/// regardless of their source `group` value (spec §9). Returns the lines
/// plus, if `target_flag` matched one of `flags`, the index of its line.
/// A spelling wider than this fraction of the pane does not get to set the
/// shared column — it hangs instead (see [`FlagLayout::Table`]). One
/// 40-character flag name in a list of short ones used to push every
/// description in the list against the right-hand edge. Mirrors the tree
/// pane's summary-column rule (spec §9.1).
const DESC_COLUMN_CAP_PERCENT: usize = 45;

/// Prose narrower than this reads as a shredded column rather than a
/// sentence, so a table that cannot leave this much room becomes a
/// [`FlagLayout::Stacked`] list instead.
///
/// Measured against real output rather than picked: at 20 columns
/// `docker pull`'s `--platform` description breaks as "Set / platform /
/// if server / is / multi-pla… / capable" — six lines, one of them
/// truncated mid-word, for six words of text. 28 is the point either side
/// of which the table and the stacked list swap places on legibility.
const MIN_DESC_WIDTH: usize = 28;

/// Leading indent for every flag row, and (in stacked mode) the extra
/// indent that subordinates a description to the spelling above it.
const FLAG_INDENT: &str = "  ";
const STACKED_DESC_INDENT: usize = 6;

/// How a whole flag list is arranged. Chosen once for the list, never per
/// row — a per-row decision is exactly what made this ragged.
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FlagLayout {
    /// Spelling, value placeholder and description in three aligned
    /// columns.
    ///
    /// Three rather than two, because a value placeholder is a different
    /// *kind* of thing from a spelling — `--env` and `list` answer "what do
    /// I type" and "what does it take". Run together as `--env list` they
    /// read as one token; in their own columns the whole list can be
    /// scanned down either one, which is what a parameter table in API
    /// documentation is for.
    ///
    /// Both columns are invariant for the list: a row too wide for them
    /// hangs its description onto the next line rather than pushing the
    /// column right for itself alone.
    Table { value: usize, description: usize },
    /// Spelling and value on one line, description indented underneath.
    ///
    /// What every narrow-terminal help renderer falls back to, and for the
    /// same reason: it gives prose the full width of the pane and keeps a
    /// perfectly straight left edge, neither of which a table can do once
    /// the columns eat more than half the room.
    Stacked,
}

impl FlagLayout {
    /// Where descriptions begin under this layout.
    fn description_column(self) -> usize {
        match self {
            FlagLayout::Table { description, .. } => description,
            FlagLayout::Stacked => STACKED_DESC_INDENT,
        }
    }
}

/// Choose the layout for `flags` in a pane `width` columns wide.
fn flag_layout(flags: &[&Entity], width: usize) -> FlagLayout {
    let cap = width * DESC_COLUMN_CAP_PERCENT / 100;
    let lead = display_width(FLAG_INDENT);
    let gap = 2;

    // Outliers are excluded from the measurement rather than clamped. A
    // clamped column is a column the outlier still misses; an excluded one
    // is a column it can hang below while every other row stays aligned.
    let fits = |w: usize| lead + w + gap <= cap;
    let widest_spec = flags
        .iter()
        .map(|f| display_width(&flag_name_spec(f)))
        .filter(|w| fits(*w))
        .max()
        .unwrap_or(0);
    let widest_value = flags
        .iter()
        .filter_map(|f| flag_value_text(f))
        .map(|v| display_width(&v))
        .filter(|w| fits(*w))
        .max()
        .unwrap_or(0);

    let value = lead + widest_spec + gap;
    // When nothing in this list takes a value the column collapses, rather
    // than leaving a blank strip down the pane.
    let description = value
        + if widest_value == 0 {
            0
        } else {
            widest_value + gap
        };

    if width.saturating_sub(description) < MIN_DESC_WIDTH {
        return FlagLayout::Stacked;
    }
    FlagLayout::Table { value, description }
}

fn flag_lines(
    flags: &[&Entity],
    width: usize,
    color_enabled: bool,
    target_flag: Option<&FlagKey>,
    glyphs: Glyphs,
) -> (Vec<Line<'static>>, Option<usize>) {
    let layout = flag_layout(flags, width);
    // Groups keep the order the tool printed them in, which is editorial:
    // `tar --help` leads with "Main operation mode" because that is what you
    // need first, and its 17 groups are sequenced deliberately. A BTreeMap
    // here sorted them alphabetically, so "Archive format selection" came
    // first and the author's ordering was silently discarded.
    let mut group_order: Vec<Option<String>> = Vec::new();
    let mut own_groups: HashMap<Option<String>, Vec<&Entity>> = HashMap::new();
    let mut inherited: Vec<&Entity> = Vec::new();

    for f in flags {
        if f.inherited {
            inherited.push(f);
        } else {
            let key = f.group.as_ref().map(|g| normalize_group_heading(g));
            if !own_groups.contains_key(&key) {
                group_order.push(key.clone());
            }
            own_groups.entry(key).or_default().push(f);
        }
    }

    let mut out = Vec::new();
    let mut target_line = None;
    let mut note_if_target = |out: &[Line<'static>], f: &Entity| {
        if target_line.is_none() && target_flag.is_some_and(|k| f.matches_key(k)) {
            target_line = Some(out.len());
        }
    };

    // Ungrouped flags first, with no heading, then each group in the order
    // the tool introduced it.
    if let Some(ungrouped) = own_groups.remove(&None) {
        for f in ungrouped {
            note_if_target(&out, f);
            out.extend(flag_line(f, false, width, color_enabled, layout));
        }
    }
    for key in group_order {
        let Some(flags) = own_groups.remove(&key) else {
            continue;
        };
        if let Some(group) = key {
            out.push(heading_line_owned(group, color_enabled));
        }
        for f in flags {
            note_if_target(&out, f);
            out.extend(flag_line(f, false, width, color_enabled, layout));
        }
    }

    if !inherited.is_empty() {
        out.push(heading_line_ruled(
            "INHERITED",
            width,
            color_enabled,
            glyphs,
        ));
        for f in inherited {
            note_if_target(&out, f);
            out.extend(flag_line(f, true, width, color_enabled, layout));
        }
    }

    (out, target_line)
}

fn heading_line_owned(text: String, color_enabled: bool) -> Line<'static> {
    Line::from(Span::styled(text, style::muted_bold(color_enabled)))
}

/// One flag's spelling, value placeholder, and description — each styled
/// per spec §9.2's table (spelling: accent; value placeholder: muted
/// italic; description: default foreground) — wrapped so a multi-line
/// description hangs indented under where it started rather than
/// restarting at column 0.
/// A flag's spelling, e.g. `-i, --interactive`.
fn flag_name_spec(flag: &Entity) -> String {
    // Every documented spelling, comma-separated, each reconstructed by
    // `Spelling::render`: the dashes and the getopt_long `--[no-]foo`
    // convention are display metadata, because the IR stores every name
    // bare (`"help"`, never `"-help"`; `"foo"`, never `"[no-]foo"`) so
    // that what a user searches and copies has no punctuation smuggled
    // into it. This is the one place that spelling comes back together —
    // and it is a *list*, so a row documenting four spellings
    // (`-h, -?, -help, --help`) renders all four rather than the two a
    // short/long pair could hold.
    flag.spellings
        .iter()
        .map(Spelling::render)
        .collect::<Vec<_>>()
        .join(", ")
}

/// A flag's value placeholder as its own column entry, e.g. `FILE` or
/// `[FILE]` when optional. `None` when the flag takes no value.
fn flag_value_text(flag: &Entity) -> Option<String> {
    flag.value_name
        .as_ref()
        .and_then(|name| match flag.value_kind {
            ValueKind::Required => Some(name.clone()),
            ValueKind::Optional => Some(format!("[{name}]")),
            ValueKind::None => None,
        })
}

fn flag_line(
    flag: &Entity,
    dim: bool,
    width: usize,
    color_enabled: bool,
    // Where the value and description columns begin, shared across the
    // whole flag list — see `flag_columns`.
    layout: FlagLayout,
) -> Vec<Line<'static>> {
    let name_spec = flag_name_spec(flag);
    let value_text = flag_value_text(flag);

    let leading = "  ";
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
        // Padded to its own column, so values line up down the list rather
        // than sitting wherever each spelling happens to end. In stacked
        // mode there is no column to reach, so a single space separates
        // them — the description below is what carries the alignment.
        let pad = match layout {
            FlagLayout::Table { value, .. } => value.saturating_sub(prefix_width).max(1),
            FlagLayout::Stacked => 1,
        };
        first_line_spans.push(Span::raw(" ".repeat(pad)));
        first_line_spans.push(Span::styled(v.clone(), value_style));
        prefix_width += pad + display_width(v);
    }

    let deprecated_tag = flag
        .deprecated
        .as_ref()
        .map(|_| " (deprecated)".to_string());

    let mut description_text = flag.description.as_ref().map(|d| d.single_line());

    // The IR carries a flag's permitted values (spec §7 Tier B rule 4:
    // `gnu`/`oldgnu`/`pax`/`posix` under `tar --format=` are enum values,
    // which is why they are *not* subcommands) and the pane was extracting
    // them and then dropping them on the floor. Knowing that `--format`
    // takes exactly six spellings is precisely the sort of thing you open a
    // reference to find out.
    if !flag.choices.is_empty() {
        let joined = flag
            .choices
            .iter()
            .map(|c| c.as_str().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let rendered = format!("[{joined}]");
        description_text = Some(match description_text {
            Some(d) if !d.is_empty() => format!("{d} {rendered}"),
            _ => rendered,
        });
    }

    if let Some(tag) = &deprecated_tag {
        description_text = Some(match description_text {
            Some(d) => format!("{d}{tag}"),
            None => tag.trim_start().to_string(),
        });
    }

    let Some(description_text) = description_text.filter(|d| !d.is_empty()) else {
        return vec![Line::from(first_line_spans)];
    };

    // One description column for the entire list, not one per flag. That
    // is what makes a parameter list read as a table — the defining visual
    // element of API documentation — and it only holds if it is *always*
    // the same number. It previously wasn't: the column was a target, and
    // any row too wide for it silently started its description at its own
    // width instead, so a list could show three different "columns" at
    // once.
    //
    // So a row that does not fit hangs: its description starts on the next
    // line, at the shared column. The spelling is never truncated to force
    // alignment (spec §9.1's rule for the tree applies here too) and the
    // column never moves — the row costs one extra line, which is the only
    // one of the three that nothing else has to pay for.
    let gap = 2;
    let indent_width = layout.description_column();
    let hangs = prefix_width + gap > indent_width;
    let available = width.saturating_sub(indent_width).max(1);
    let chunks = wrap_words(&description_text, available);

    let mut lines = Vec::new();
    let mut chunks_iter = chunks.into_iter();
    if !hangs {
        if let Some(first_chunk) = chunks_iter.next() {
            first_line_spans.push(Span::raw(" ".repeat(indent_width - prefix_width)));
            first_line_spans.push(Span::styled(first_chunk, desc_style));
        }
    }
    lines.push(Line::from(first_line_spans));

    let indent_str = " ".repeat(indent_width);
    for chunk in chunks_iter {
        lines.push(Line::from(Span::styled(
            format!("{indent_str}{chunk}"),
            desc_style,
        )));
    }
    lines
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
    use mandible_core::{Provenance, Source, Text};

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
        let (lines, _) = flag_lines(&flags, 80, true, None, crate::glyphs::UNICODE);
        let text: Vec<String> = lines.iter().map(text_of).collect();
        let inherited_pos = text.iter().position(|l| l.contains("INHERITED")).unwrap();
        let help_pos = text.iter().position(|l| l.contains("--help")).unwrap();
        assert!(help_pos > inherited_pos);
    }

    #[test]
    fn hidden_flags_suppressed_by_default() {
        let mut node = node_with_flags();
        node.flags_mut().next().expect("a flag").hidden = true;
        let built = build_lines(
            &node,
            false,
            80,
            true,
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
            true,
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
            mk(Some('d'), "detach", None, "Detached mode"),
            mk(
                None,
                "detach-keys",
                Some("string"),
                "Override the key sequence",
            ),
            mk(Some('e'), "env", Some("list"), "Set environment variables"),
        ];
        let refs: Vec<&mandible_core::Entity> = flags.iter().collect();
        let (lines, _) = flag_lines(&refs, 80, true, None, crate::glyphs::UNICODE);

        // Column at which each row's description text begins.
        let starts: Vec<usize> = lines
            .iter()
            .filter_map(|line| {
                let text = text_of(line);
                let trimmed = text.trim_start();
                if trimmed.starts_with('-') {
                    // A flag row: find where the description follows the
                    // spelling and its run of padding.
                    let spec_end = text.find("  ")?;
                    let rest = &text[spec_end..];
                    let pad = rest.len() - rest.trim_start().len();
                    Some(spec_end + pad)
                } else {
                    None
                }
            })
            .collect();

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
            f.description = Some(Text::sanitize(
                "zzz set the thing to the other thing and then keep going for a while",
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
    fn description_columns(lines: &[Line<'static>]) -> Vec<usize> {
        lines
            .iter()
            .filter_map(|line| {
                let text = text_of(line);
                if let Some(at) = text.find("zzz") {
                    return Some(display_width(&text[..at]));
                }
                // A continuation line: prose with no spelling on it.
                let trimmed = text.trim_start();
                if trimmed.is_empty() || trimmed.starts_with('-') {
                    return None;
                }
                Some(text.len() - trimmed.len())
            })
            .collect()
    }

    /// The reported defect, at every width rather than one.
    ///
    /// A shared column is only shared if it is the same number for every
    /// row. It was not: the column was capped at 45% of the pane and any
    /// row too wide for the cap started its description at its own width
    /// instead, so `docker`'s global flags rendered descriptions at three
    /// different columns (19, 24 and 28) in a 90-column terminal — with
    /// `--log-level string` also losing the gap that separates a spelling
    /// from its value, so the two ran together as one token.
    #[test]
    fn descriptions_share_one_column_at_every_width() {
        let flags = docker_global_flags();
        let refs: Vec<&mandible_core::Entity> = flags.iter().collect();

        for width in 20..=160 {
            let (lines, _) = flag_lines(&refs, width, true, None, crate::glyphs::UNICODE);
            let starts = description_columns(&lines);
            assert!(
                !starts.is_empty(),
                "width {width}: no descriptions rendered"
            );
            let distinct: std::collections::BTreeSet<usize> = starts.iter().copied().collect();
            assert_eq!(
                distinct.len(),
                1,
                "width {width}: descriptions start at {distinct:?}, not one shared column"
            );
        }
    }

    /// Below the point where a table can leave prose a readable width, the
    /// list stacks rather than shredding descriptions into a narrow strip.
    ///
    /// At 90 columns `docker pull`'s `--platform` description used to
    /// break as "Set / platform / if server / is / multi-pla… / capable" —
    /// six lines for six words, one truncated mid-word, because the
    /// columns had eaten everything but 9 cells of the pane.
    #[test]
    fn a_narrow_pane_stacks_instead_of_shredding_prose() {
        let flags = docker_global_flags();
        let refs: Vec<&mandible_core::Entity> = flags.iter().collect();

        assert_eq!(flag_layout(&refs, 38), FlagLayout::Stacked);
        let (lines, _) = flag_lines(&refs, 38, true, None, crate::glyphs::UNICODE);
        for start in description_columns(&lines) {
            assert_eq!(start, STACKED_DESC_INDENT, "stacked prose must be flush");
        }
        // The whole point of stacking: prose gets the pane, not a strip.
        // Measured on the rendered lines rather than asserted against the
        // constants, which would only restate the arithmetic above.
        let widest_prose = lines
            .iter()
            .map(text_of)
            .filter(|t| !t.trim_start().starts_with('-'))
            .map(|t| display_width(t.trim()))
            .max()
            .unwrap_or(0);
        assert!(
            widest_prose >= MIN_DESC_WIDTH,
            "stacked prose still shredded: widest line was {widest_prose}"
        );
    }

    /// One very long spelling must not drag every other row's description
    /// against the right-hand edge — the reason a cap existed at all. It
    /// now hangs instead of widening the column, so the cap's original job
    /// is done without the raggedness it used to cause.
    #[test]
    fn one_overlong_spelling_hangs_rather_than_moving_the_column() {
        let mut flags = docker_global_flags();
        // Past the 45% cap at 120 columns, which is the point of the test.
        // A spelling that merely *looks* long is not an outlier: a 49-char
        // name at this width still leaves 59 columns for prose, and the
        // cap admits it deliberately rather than spending a line on it.
        let mut monster = mandible_core::Entity::flag_long(
            "an-extremely-long-option-name-that-nobody-would-ever-type-by-hand",
            Provenance::single(Source::HelpText),
        );
        monster.description = Some(Text::sanitize("zzz does something"));
        flags.push(monster);
        let refs: Vec<&mandible_core::Entity> = flags.iter().collect();

        let without: Vec<&mandible_core::Entity> = refs[..refs.len() - 1].to_vec();
        assert_eq!(
            flag_layout(&refs, 120),
            flag_layout(&without, 120),
            "an outlier spelling must not set the column for the list"
        );

        let (lines, _) = flag_lines(&refs, 120, true, None, crate::glyphs::UNICODE);
        let distinct: std::collections::BTreeSet<usize> =
            description_columns(&lines).into_iter().collect();
        assert_eq!(distinct.len(), 1, "outlier broke the column: {distinct:?}");

        // ...and it hangs: its spelling occupies a line of its own.
        let joined: Vec<String> = lines.iter().map(text_of).collect();
        let row = joined
            .iter()
            .find(|l| l.contains("an-extremely-long-option-name"))
            .expect("outlier row missing");
        assert!(
            !row.contains("zzz"),
            "an over-long spelling should hang its description, not push the column: {row:?}"
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
    fn wrapped_flag_description_hangs_indented_not_flush_left() {
        let mut flag = Entity::flag_long("tlscacert", Provenance::single(Source::HelpText));
        flag.value_name = Some("string".to_string());
        flag.value_kind = ValueKind::Required;
        flag.description = Some(Text::sanitize(
            "Trust certs signed only by this CA (default \"\")",
        ));
        let lines = flag_line(
            &flag,
            false,
            40,
            true,
            FlagLayout::Table {
                value: 18,
                description: 20,
            },
        );
        assert!(lines.len() >= 2, "expected wrapping: {lines:?}");
        let first_text = text_of(&lines[0]);
        // Every description line — the first as well as the continuations
        // — sits at the column the list agreed on, never at column 0 and
        // never at this row's own width.
        //
        // This row's spelling plus value runs to 24, past the column, so
        // it hangs: line 0 is the spelling alone and the description
        // starts on line 1. The earlier assertion here demanded the
        // continuation clear *this row's* prefix, which is precisely the
        // per-row indent that made a list of flags render with three
        // different "columns" at once.
        for line in &lines[1..] {
            let text = text_of(line);
            let indent_len = text.len() - text.trim_start().len();
            assert_eq!(
                indent_len, 20,
                "first={first_text:?} line={text:?} must start at the shared column"
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
        let lines = flag_line(
            &flag,
            false,
            80,
            true,
            FlagLayout::Table {
                value: 18,
                description: 20,
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

    #[test]
    fn deprecated_flag_gets_a_tag() {
        let mut flag = Entity::flag_long("old-flag", Provenance::single(Source::HelpText));
        flag.deprecated = Some(Text::sanitize("use --new-flag instead"));
        flag.description = Some(Text::sanitize("Old behavior"));
        let lines = flag_line(
            &flag,
            false,
            80,
            true,
            FlagLayout::Table {
                value: 18,
                description: 20,
            },
        );
        let joined: String = lines.iter().map(text_of).collect();
        assert!(joined.contains("(deprecated)"), "{joined:?}");
    }

    /// The coordinator's second reported defect: a group heading must not
    /// carry its source's trailing colon or casing quirks into the UI —
    /// `"GLOBAL OPTIONS:"` and `"Global Options"` must render the same.
    #[test]
    fn group_headings_strip_trailing_colon_and_normalize_case() {
        assert_eq!(normalize_group_heading("GLOBAL OPTIONS:"), "GLOBAL OPTIONS");
        assert_eq!(
            normalize_group_heading("Main operation mode:"),
            "MAIN OPERATION MODE"
        );
        assert_eq!(normalize_group_heading("main"), "MAIN");
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
            true,
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
            true,
            None,
            crate::glyphs::UNICODE,
            &test_app(),
        );
        assert_eq!(built.target_flag_line, None);
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
    fn usage_signature_does_not_prepend_when_the_full_path_already_names_the_node() {
        assert_eq!(
            usage_signature(
                "import",
                "docker import [OPTIONS] file|URL|- [REPOSITORY[:TAG]]"
            ),
            "docker import [OPTIONS] file|URL|- [REPOSITORY[:TAG]]"
        );
        // Same shape, a second real tool (docker pull), so this isn't
        // one coincidental fixture.
        assert_eq!(
            usage_signature("pull", "docker pull [OPTIONS] NAME[:TAG|@DIGEST]"),
            "docker pull [OPTIONS] NAME[:TAG|@DIGEST]"
        );
        // argparse does the same thing, and for a node three levels deep
        // the leading run is three words wide, not one — the fix has to
        // scan the whole run, not just swap which single word it checks.
        assert_eq!(
            usage_signature("outlier", "smokecli columns outlier [-h] [-v] [-n]"),
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
    fn usage_signature_still_prepends_when_the_name_is_truly_absent() {
        assert_eq!(
            usage_signature("mytool", "[OPTIONS] FILE"),
            "mytool [OPTIONS] FILE"
        );
        assert_eq!(usage_signature("cat", "<url>"), "cat <url>");
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
        let built = build_lines(&node, false, 46, true, None, crate::glyphs::UNICODE, &app);
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
            true,
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
        let _ = build_lines(&node, false, 46, true, None, crate::glyphs::UNICODE, &app);
        app.detail_hscroll_right();
        app.detail_hscroll_right();
        let built = build_lines(&node, false, 46, true, None, crate::glyphs::UNICODE, &app);
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
