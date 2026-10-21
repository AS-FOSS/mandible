//! Small terminals: the three questions every size-dependent behaviour
//! answers, and the only place a width or height meets a number.
//!
//! 1. SCREEN: do two panes fit side by side? ([`screen`])
//! 2. ROOM: does a description get at least [`MIN_DESC_ROOM`] columns?
//!    ([`has_room`], [`section_room`])
//! 3. USER CHOICE: wrap or scroll sideways (`w`, held on the app).
//!
//! When space runs out the order is the same everywhere: normal layout,
//! re-flow, cut with `…` only where the full text is readable elsewhere,
//! scroll. Markers for all of it come from [`marker`] (docs/design.md,
//! "Small terminals").

use crate::glyphs::Glyphs;
use ratatui::layout::Rect;

/// Below this total width the tree and detail panes stack and `Tab`
/// switches which one is visible.
pub const STACK_BREAKPOINT: u16 = 50;

/// The smallest terminal width every screen still renders sanely at.
pub const MIN_WIDTH: u16 = 30;

/// The smallest terminal height every screen still renders sanely at.
pub const MIN_HEIGHT: u16 = 10;

/// The narrowest a description may be before its section is laid out as a
/// man page instead of a table, and before a tree summary is dropped.
pub const MIN_DESC_ROOM: usize = 20;

/// The tree pane's width when both panes show, floor.
pub const TREE_MIN_WIDTH: u16 = 24;

/// Where a man-layout description starts, past its row's left edge.
pub const MAN_DESC_INDENT: usize = 8;

/// Columns a continuation row hangs past its first row (USAGE, EXAMPLES,
/// a man-layout head longer than the pane).
pub const USAGE_HANG: usize = 4;

/// Columns one `←`/`→` press scrolls the detail pane.
pub const HSCROLL_STEP: usize = 8;

/// A spelling wider than this share of the pane does not set the shared
/// description column (spec §9.1a).
pub const DESC_COLUMN_CAP_PERCENT: usize = 45;

/// The share of a section's heads the shared description column is
/// fitted to (spec §9.3).
pub const SHARED_COLUMN_PERCENTILE: usize = 90;

/// The summary column of the tree is capped at this share of the pane.
pub const SUMMARY_COLUMN_CAP_PERCENT: usize = 40;

/// What a terminal of a given size can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    /// Under the minimum: only the "too small" line is drawn.
    TooSmall,
    /// One pane at a time.
    Stacked,
    /// Tree and detail side by side.
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

/// The columns a description gets in a pane `pane_width` wide when the
/// section's description column starts at `column` (the p90 head plus the
/// gap after it).
pub fn section_room(pane_width: usize, column: usize) -> usize {
    pane_width.saturating_sub(column)
}

/// The one marker family. `Cut` is the ellipsis; the others say more
/// content sits above, below, left or right.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Marker {
    /// Text was cut and is readable in full elsewhere.
    Cut,
    /// More content above.
    Up,
    /// More content below.
    Down,
    /// More content to the left.
    Left,
    /// More content to the right.
    Right,
}

/// The glyph for `marker` in the active glyph set (`…` `▲` `▼` `◀` `▶`,
/// or `...` `^` `v` `<` `>` in ASCII).
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
