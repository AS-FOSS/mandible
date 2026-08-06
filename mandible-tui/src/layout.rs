//! Responsive layout (spec §2 "Layout").
//!
//! Vertical: search bar (3 rows) / body (fill) / status bar (1 row).
//! Body horizontal: tree pane `Min(24)` / detail pane fills the remainder —
//! not a percentage split (a 35% tree pane at 80 columns leaves ~20 usable
//! cells after borders and indentation, not enough for a name plus a
//! summary — spec [M-7]). Below 60 columns, tree rows drop their summary
//! text. Below 50 columns, the panes stack vertically and `Tab` switches
//! which one is visible.

use crate::app::Focus;
use ratatui::layout::{Constraint, Direction, Layout, Rect};

/// The minimum total terminal width, in columns, below which the tree and
/// detail panes stack vertically instead of sitting side by side.
pub const STACK_BREAKPOINT: u16 = 50;

/// The minimum total terminal width, in columns, below which tree rows
/// drop their summary text (name only).
pub const SUMMARY_BREAKPOINT: u16 = 60;

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
    /// True if tree row summaries should be omitted for width reasons.
    pub hide_summaries: bool,
}

/// Compute this frame's layout for a terminal of size `area`, given which
/// pane has focus (used to decide which pane to show when stacked below
/// [`STACK_BREAKPOINT`]).
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

    let hide_summaries = area.width < SUMMARY_BREAKPOINT;

    let (tree, detail) = if area.width < STACK_BREAKPOINT {
        match focus {
            Focus::Detail => (None, Some(body)),
            _ => (Some(body), None),
        }
    } else {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(24), Constraint::Fill(1)])
            .split(body);
        (Some(cols[0]), Some(cols[1]))
    };

    Regions {
        search,
        tree,
        detail,
        status,
        hide_summaries,
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
        assert!(!regions.hide_summaries);
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
    fn medium_terminal_hides_summaries_but_keeps_both_panes() {
        let regions = compute(area(55, 30), Focus::Tree);
        assert!(regions.tree.is_some());
        assert!(regions.detail.is_some());
        assert!(regions.hide_summaries);
    }

    #[test]
    fn tree_pane_has_minimum_width() {
        let regions = compute(area(200, 30), Focus::Tree);
        assert!(regions.tree.unwrap().width >= 24);
    }
}
