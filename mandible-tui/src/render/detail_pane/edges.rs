//! What the detail pane draws on its border and in its gutters: the title,
//! and the markers for rows and columns that are out of view.

use super::*;

/// The pane title for `path`, cut from the left to `budget` columns so the
/// current command's name stays visible: leading segments give way to a
/// `…` first, and the name itself is cut from its left only when it alone
/// does not fit.
pub(super) fn breadcrumb_title(path: &[String], glyphs: Glyphs, budget: usize) -> String {
    let sep = format!(" {} ", glyphs.breadcrumb);
    let cut = fit::marker(&glyphs, fit::Marker::Cut);
    let whole = format!(" {} ", path.join(&sep));
    if display_width(&whole) <= budget {
        return whole;
    }
    for skip in 1..path.len() {
        let tail = path[skip..].join(&sep);
        let title = format!(" {cut}{sep}{tail} ");
        if display_width(&title) <= budget {
            return title;
        }
    }
    let name = path.last().map_or("", String::as_str);
    let room = budget.saturating_sub(display_width(cut) + 2);
    let tail: String = name
        .chars()
        .rev()
        .scan(0usize, |used, c| {
            *used += UnicodeWidthChar::width(c).unwrap_or(0);
            (*used <= room).then_some(c)
        })
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!(" {cut}{tail} ")
}

/// The pane title: the breadcrumb, then the sort indicator while the page
/// is sorted. The indicator is the first thing to go: it shows only when
/// the whole breadcrumb and the indicator fit `budget` columns, so the
/// breadcrumb is cut the same way with or without it (spec §9.4).
pub(super) fn detail_title(app: &App, path: &[String], budget: usize) -> Line<'static> {
    let crumb = breadcrumb_title(path, app.glyphs, budget);
    let sorted =
        !app.showing_verbatim() && app.selected_node().is_some_and(|n| !n.same_as_ancestor);
    let tag = app.sorts.detail.indicator(app.glyphs).filter(|_| sorted);
    let Some(tag) = tag else {
        return Line::from(crumb);
    };
    let sep = format!(" {} ", app.glyphs.breadcrumb);
    let head = format!(" {}", path.join(&sep));
    let tail = format!(" {} {tag} ", app.glyphs.dot);
    if crumb != format!("{head} ") || display_width(&head) + display_width(&tail) > budget {
        return Line::from(crumb);
    }
    Line::from(vec![Span::raw(head), Span::styled(tail, Style::reset())])
}

/// The style of the detail pane's border, and of every marker on it.
pub(super) fn border_style(app: &App) -> Style {
    if app.focus == Focus::Detail {
        style::accent(app.color_enabled)
    } else {
        Style::default()
    }
}

/// Draw `▲` and `▼` on the border for rows hidden above or below, through
/// the one marker function. Drawn regardless of pane focus, since a silently
/// clipped edge is worse than a marker pointing at an unfocused pane.
pub(super) fn draw_edge_markers(frame: &mut Frame, area: Rect, app: &App) {
    let set = style::border_set(app.glyphs);
    let style = border_style(app);
    let buf = frame.buffer_mut();
    if app.detail_can_scroll_up() {
        let args = (&app.glyphs, fit::Marker::Up, style, set.horizontal_top);
        fit::draw_vertical_marker(buf, area, args.0, args.1, args.2, args.3);
    }
    if app.detail_can_scroll_down() {
        let args = (&app.glyphs, fit::Marker::Down, style, set.horizontal_bottom);
        fit::draw_vertical_marker(buf, area, args.0, args.1, args.2, args.3);
    }
}

/// Draw the per-line clip markers into the padding gutters either side of
/// `inner` — the one blank column `Block::padding` leaves between border
/// and content. `clip_rows` holds `(line index, clipped_left,
/// clipped_right)` in the same index space the paragraph's vertical
/// `scroll` uses, so a marker follows its line when the pane scrolls
/// vertically and disappears with it off-screen.
pub(super) fn draw_clip_marker_rails(
    frame: &mut Frame,
    inner: Rect,
    app: &App,
    clip_rows: &[(usize, bool, bool)],
    vscroll: usize,
) {
    if inner.x == 0 || clip_rows.is_empty() {
        return;
    }
    let left_x = inner.x - 1;
    let right_x = inner.x + inner.width;
    let border = border_style(app);
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
            fit::draw_marker(buf, (left_x, y), &app.glyphs, fit::Marker::Left, border);
        }
        if right {
            fit::draw_marker(buf, (right_x, y), &app.glyphs, fit::Marker::Right, border);
        }
    }
}
