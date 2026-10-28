//! Keeping the row at the top of the pane when `s` re-orders the page.

use super::*;
use crate::sort::TopRow;

/// The scroll to draw with. When `s` re-ordered the page since the last
/// draw, the row that was at the top is put at the top again, scrolled into
/// by as many lines as before. A search match the page follows, or a page
/// that no longer has the row, keeps `scroll`.
pub(super) fn keep_top_row(
    app: &App,
    built: &BuiltLines,
    (total, viewport): (usize, usize),
    follow_target: bool,
    scroll: usize,
) -> usize {
    let Some(held) = app.sort_hold.take() else {
        return scroll;
    };
    let path = app.selected_row().map(|r| r.path.clone());
    if follow_target || path.as_ref() != Some(&held.path) {
        return scroll;
    }
    let Some(at) = built.ids.iter().position(|&id| id == held.id) else {
        return scroll;
    };
    let row = &built.rows[at];
    let target = row.first_line + held.within.min(row.lines.saturating_sub(1));
    let target = target.min(total.saturating_sub(viewport));
    app.place_detail_scroll(target);
    target
}

/// Record the row at the top of the page drawn at `scroll`, if the top line
/// belongs to a row.
pub(super) fn note_top_row(app: &App, built: &BuiltLines, scroll: usize) {
    let next = built.rows.partition_point(|r| r.first_line <= scroll);
    let top = next.checked_sub(1).and_then(|i| {
        let row = &built.rows[i];
        (scroll < row.first_line + row.lines).then(|| TopRow {
            path: app
                .selected_row()
                .map(|r| r.path.clone())
                .unwrap_or_default(),
            id: built.ids[i],
            within: scroll - row.first_line,
        })
    });
    app.sort_hold.see(top);
}
