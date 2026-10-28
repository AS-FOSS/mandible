//! Small terminals: the three questions every size-dependent behaviour
//! answers, and the one place a width or height meets a number
//! (docs/design.md §9.4): SCREEN ([`screen`]), ROOM ([`has_room`]) and
//! USER CHOICE (`w`, held on the app). Markers come from [`marker`].

use crate::glyphs::Glyphs;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

/// Below this width the panes stack and `Tab` switches between them.
pub const STACK_BREAKPOINT: u16 = 50;

/// Narrowest terminal drawn at all.
pub const MIN_WIDTH: u16 = 30;

/// Shortest terminal drawn at all.
pub const MIN_HEIGHT: u16 = 10;

/// Columns a description needs before its section stays a table.
pub const MIN_DESC_ROOM: usize = 20;

/// The tree pane's floor when both panes show.
pub const TREE_MIN_WIDTH: u16 = 24;

/// Where a man-layout description starts, past the section's indent.
pub const MAN_DESC_INDENT: usize = 4;

/// Columns a man-layout head's wrapped pieces hang past its first line.
pub const MAN_HEAD_HANG: usize = 2;

/// Columns a continuation row hangs past its first row.
pub const USAGE_HANG: usize = 4;

/// Columns one `←`/`→` press scrolls.
pub const HSCROLL_STEP: usize = 8;

/// A head wider than this share of the pane gets no vote on the column.
pub const DESC_COLUMN_CAP_PERCENT: usize = 45;

/// The share of heads the shared description column is fitted to.
pub const SHARED_COLUMN_PERCENTILE: usize = 90;

/// The tree's summary column is capped at this share of the pane.
pub const SUMMARY_COLUMN_CAP_PERCENT: usize = 40;

/// Columns of the tree's match-count column: a count of at most `99+`,
/// a space and `found`.
pub const MATCH_COUNT_WIDTH: usize = 9;

/// Columns between the match-count column and the pane's border.
pub const MATCH_COUNT_MARGIN: usize = 1;

/// Columns between the tree's summary and its match-count column.
pub const MATCH_COUNT_GAP: usize = 2;

/// What a terminal of a given size can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    TooSmall,
    Stacked,
    Split,
}

/// Question 1: do two panes fit side by side?
pub fn screen(area: Rect) -> Screen {
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        Screen::TooSmall
    } else if area.width < STACK_BREAKPOINT {
        Screen::Stacked
    } else {
        Screen::Split
    }
}

/// The line drawn instead of the UI on a [`Screen::TooSmall`] terminal.
pub fn too_small_message() -> String {
    format!("Terminal too small (need {MIN_WIDTH}×{MIN_HEIGHT}). Enlarge the window or zoom out.")
}

/// Question 2: is `room` enough columns for a description?
pub fn has_room(room: usize) -> bool {
    room >= MIN_DESC_ROOM
}

/// Columns a description gets in a pane `pane_width` wide when the p90 of
/// every head in its section (uncapped) is `head_width`.
pub fn section_room(pane_width: usize, head_width: usize) -> usize {
    pane_width.saturating_sub(head_width + 1)
}

/// The marker family: cut, more above, below, left, right.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Marker {
    Cut,
    Up,
    Down,
    Left,
    Right,
}

/// The glyph for `marker`, with its ASCII fallback from the glyph set.
pub fn marker(glyphs: &Glyphs, marker: Marker) -> &'static str {
    match marker {
        Marker::Cut => glyphs.ellipsis,
        Marker::Up => glyphs.more_up,
        Marker::Down => glyphs.more_down,
        Marker::Left => glyphs.more_left,
        Marker::Right => glyphs.more_right,
    }
}

/// Draw `which` at `(x, y)` in `border`, the style of the frame it belongs
/// to: a marker is part of its pane's frame, so it is the accent when the
/// pane is focused and plain when not, and never a colour of its own.
pub fn draw_marker(
    buf: &mut Buffer,
    (x, y): (u16, u16),
    glyphs: &Glyphs,
    which: Marker,
    border: Style,
) {
    buf[(x, y)]
        .set_symbol(marker(glyphs, which))
        .set_style(border);
}

/// Draw the `▲` (top) or `▼` (bottom) of a frame occupying `area`, one rule
/// cell in from the right corner (`─▲─╮`). Only a plain rule cell is drawn
/// over, so a title is never overwritten.
pub fn draw_vertical_marker(
    buf: &mut Buffer,
    area: Rect,
    glyphs: &Glyphs,
    which: Marker,
    border: Style,
    rule: &str,
) {
    let y = match which {
        Marker::Up => area.y,
        Marker::Down => area.y + area.height.saturating_sub(1),
        _ => return,
    };
    let Some(x) = (area.x + area.width).checked_sub(VERTICAL_MARKER_FROM_RIGHT) else {
        return;
    };
    if x > area.x && buf[(x, y)].symbol() == rule {
        draw_marker(buf, (x, y), glyphs, which, border);
    }
}

/// Columns from a frame's right edge to its vertical marker's cell.
pub const VERTICAL_MARKER_FROM_RIGHT: u16 = 3;

/// Columns a top border's title must leave free for the vertical marker,
/// counted from the title's start to the right edge.
pub const TITLE_CLEARANCE: u16 = VERTICAL_MARKER_FROM_RIGHT + 1;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_answers_split_stacked_or_too_small() {
        assert_eq!(screen(Rect::new(0, 0, 50, 24)), Screen::Split);
        assert_eq!(screen(Rect::new(0, 0, 49, 24)), Screen::Stacked);
        assert_eq!(screen(Rect::new(0, 0, MIN_WIDTH - 1, 24)), Screen::TooSmall);
        assert_eq!(
            screen(Rect::new(0, 0, 80, MIN_HEIGHT - 1)),
            Screen::TooSmall
        );
    }

    #[test]
    fn room_is_twenty_columns() {
        assert!(has_room(20));
        assert!(!has_room(19));
        assert_eq!(section_room(40, 19), 20);
    }

    #[test]
    fn markers_have_an_ascii_fallback() {
        for m in [
            Marker::Cut,
            Marker::Up,
            Marker::Down,
            Marker::Left,
            Marker::Right,
        ] {
            assert!(marker(&crate::glyphs::ASCII, m).is_ascii());
        }
        assert_eq!(marker(&crate::glyphs::UNICODE, Marker::Right), "▶");
    }
}
