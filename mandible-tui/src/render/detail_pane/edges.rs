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

/// Write `text` on the top or bottom border, ending `from_right` cells before
/// the right-hand corner, but only over cells that are still plain rule: a title or a
/// corner is never overwritten.
fn stamp_border(frame: &mut Frame, area: Rect, app: &App, top: bool, from_right: u16, text: &str) {
    let set = style::border_set(app.glyphs);
    let (rule, y) = if top {
        (set.horizontal_top, area.y)
    } else {
        (
            set.horizontal_bottom,
            area.y + area.height.saturating_sub(1),
        )
    };
    let Ok(len) = u16::try_from(display_width(text)) else {
        return;
    };
    let end = area.x + area.width;
    let Some(start) = end.checked_sub(from_right + len) else {
        return;
    };
    let buf = frame.buffer_mut();
    if start <= area.x || (start..start + len).any(|x| buf[(x, y)].symbol() != rule) {
        return;
    }
    let mut x = start;
    for ch in text.chars() {
        buf[(x, y)].set_char(ch);
        x += u16::try_from(UnicodeWidthChar::width(ch).unwrap_or(1)).unwrap_or(1);
    }
}

/// Draw what the pane is hiding onto its border, through the one marker
/// family: `▲`/`▼ more` for rows above or below, `◀`/`▶` for columns off
/// either side. Drawn regardless of pane focus, since a silently clipped
/// edge is worse than a marker pointing at an unfocused pane (spec §9).
pub(super) fn draw_edge_markers(frame: &mut Frame, area: Rect, app: &App) {
    let up = fit::marker(&app.glyphs, fit::Marker::Up);
    let down = fit::marker(&app.glyphs, fit::Marker::Down);
    if app.detail_can_scroll_up() {
        stamp_border(frame, area, app, true, 4, &format!(" {up} more "));
    }
    if app.detail_can_scroll_down() {
        stamp_border(frame, area, app, false, 2, &format!(" {down} more "));
    }
    if !app.sideways() {
        return;
    }
    if app.detail_hscroll_can_go_right() {
        let right = fit::marker(&app.glyphs, fit::Marker::Right);
        stamp_border(frame, area, app, true, 1, right);
    }
    if app.detail_hscroll_can_go_left() {
        let left = fit::marker(&app.glyphs, fit::Marker::Left);
        stamp_border(frame, area, app, true, 2, left);
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
    let marker = style::muted(app.color_enabled);
    let left_glyph = fit::marker(&app.glyphs, fit::Marker::Left);
    let right_glyph = fit::marker(&app.glyphs, fit::Marker::Right);
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
            buf[(left_x, y)].set_symbol(left_glyph).set_style(marker);
        }
        if right {
            buf[(right_x, y)].set_symbol(right_glyph).set_style(marker);
        }
    }
}
