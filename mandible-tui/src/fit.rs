//! Small terminals: the three questions every size-dependent behaviour
//! answers, and the one place a width or height meets a number
//! (docs/design.md §9.4): SCREEN ([`screen`]), ROOM ([`has_room`]) and
//! USER CHOICE (`w`, held on the app). Markers come from [`marker`].

use crate::glyphs::Glyphs;
use ratatui::layout::Rect;

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
pub const MAN_DESC_INDENT: usize = 8;

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

/// Columns a description gets in a pane `pane_width` wide when the
/// description column starts at `column`.
pub fn section_room(pane_width: usize, column: usize) -> usize {
    pane_width.saturating_sub(column)
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
        assert_eq!(section_room(40, 20), 20);
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
