//! Responsive layout (spec §2 "Layout").
//!
//! Vertical: search bar (3 rows) / body (fill) / status bar (1 row).
//! Body horizontal: tree pane `Min(24)` / detail pane fills the remainder —
//! not a percentage split (a 35% tree pane at 80 columns leaves ~20 usable
//! cells after borders and indentation, not enough for a name plus a
//! summary — spec [M-7]). Which of "too small", "one pane at a time" and
//! "both panes" applies is [`crate::fit::screen`]'s answer.

use crate::app::Focus;
use crate::fit::{self, Screen};
use ratatui::layout::{Constraint, Direction, Layout, Rect};

/// The computed screen regions for one frame.
#[derive(Debug, Clone, Copy)]
pub struct Regions {
    /// The search bar's area.
    pub search: Rect,
    /// The tree pane's area, if visible this frame.
    pub tree: Option<Rect>,
    /// The detail pane's area, if visible this frame.
    pub detail: Option<Rect>,
    /// The status bar's area.
    pub status: Rect,
    /// True when the terminal is under the minimum size: draw only the
    /// "too small" line.
    pub too_small: bool,
}

/// Compute this frame's layout for a terminal of size `area`, given which
/// pane has focus (used to decide which pane to show when stacked).
pub fn compute(area: Rect, focus: Focus) -> Regions {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);
    let search = vertical[0];
    let body = vertical[1];
    let status = vertical[2];

    let screen = fit::screen(area);
    let (tree, detail) = match screen {
        Screen::Split => {
            let cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Min(fit::TREE_MIN_WIDTH), Constraint::Fill(1)])
                .split(body);
            (Some(cols[0]), Some(cols[1]))
        }
        _ => match focus {
            Focus::Detail => (None, Some(body)),
            _ => (Some(body), None),
        },
    };

    Regions {
        search,
        tree,
        detail,
        status,
        too_small: screen == Screen::TooSmall,
    }
}

/// A centered popup rect for the `?` keybinding overlay, `percent_x` /
/// `percent_y` of `area`.
pub fn centered_popup(area: Rect, percent_x: u16, percent_y: u16) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(w: u16, h: u16) -> Rect {
        Rect::new(0, 0, w, h)
    }

    #[test]
    fn wide_terminal_shows_both_panes_side_by_side() {
        let regions = compute(area(100, 30), Focus::Tree);
        assert!(regions.tree.is_some());
        assert!(regions.detail.is_some());
        assert!(!regions.too_small);
    }

    #[test]
    fn narrow_terminal_stacks_and_shows_focused_pane_only() {
        let regions = compute(area(40, 30), Focus::Tree);
        assert!(regions.tree.is_some());
        assert!(regions.detail.is_none());

        let regions = compute(area(40, 30), Focus::Detail);
        assert!(regions.tree.is_none());
        assert!(regions.detail.is_some());
    }

    #[test]
    fn medium_terminal_keeps_both_panes() {
        let regions = compute(area(55, 30), Focus::Tree);
        assert!(regions.tree.is_some());
        assert!(regions.detail.is_some());
    }

    #[test]
    fn a_terminal_under_the_minimum_is_too_small() {
        assert!(compute(area(fit::MIN_WIDTH - 1, 30), Focus::Tree).too_small);
        assert!(compute(area(80, fit::MIN_HEIGHT - 1), Focus::Tree).too_small);
        assert!(!compute(area(fit::MIN_WIDTH, fit::MIN_HEIGHT), Focus::Tree).too_small);
    }

    #[test]
    fn tree_pane_has_minimum_width() {
        let regions = compute(area(200, 30), Focus::Tree);
        assert!(regions.tree.unwrap().width >= 24);
    }
}
