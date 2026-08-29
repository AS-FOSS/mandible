//! Translating terminal input into [`App`] mutations (spec §2 "Interaction
//! model").

use crate::app::{App, Effect, Focus};
use crate::layout::Regions;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

/// Handle a key event. Returns `Some(Effect)` when the caller (the binary's
/// event loop) needs to do something outside pure state — copy to the
/// clipboard, re-extract, or quit.
pub fn handle_key(app: &mut App, key: KeyEvent) -> Option<Effect> {
    // Ctrl-C always quits, regardless of focus.
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        return Some(Effect::Quit);
    }

    if app.show_help {
        // Any key closes the overlay except `?` itself, which is handled
        // uniformly below (toggling closes it too).
        if !matches!(key.code, KeyCode::Char('?')) {
            app.show_help = false;
            return None;
        }
    }

    if app.focus == Focus::Search {
        return handle_search_key(app, key);
    }

    match key.code {
        KeyCode::Char('q') => return Some(Effect::Quit),
        KeyCode::Char('?') => app.toggle_help(),
        KeyCode::Char('/') => app.focus_search(),
        KeyCode::Tab => app.toggle_focus(),
        KeyCode::Char('.') => app.toggle_show_hidden(),
        KeyCode::Char('t') => return app.toggle_raw_mode(),
        KeyCode::Char('r') => return Some(Effect::Refresh),
        KeyCode::Esc => app.escape_search(),
        _ => {
            return match app.focus {
                Focus::Tree => handle_tree_key(app, key),
                Focus::Detail => handle_detail_key(app, key),
                Focus::Search => unreachable!("handled above"),
            }
        }
    }
    None
}

fn handle_search_key(app: &mut App, key: KeyEvent) -> Option<Effect> {
    match key.code {
        KeyCode::Esc => app.escape_search(),
        KeyCode::Enter => app.focus = Focus::Tree,
        KeyCode::Backspace => app.search_backspace(),
        // Arrows move the (filtered) tree selection without leaving the
        // search box, so the result the user is about to land on is
        // visible while still typing — Esc/Enter to actually move focus
        // to the tree remain available for anyone who wants that instead.
        // Letters go to search_input_char below, not here, so typing
        // "j"/"k" always searches rather than navigating.
        KeyCode::Down => app.move_down(),
        KeyCode::Up => app.move_up(),
        // `/` while already in the box toggles what search matches
        // against, rather than typing a slash — command names never
        // contain one, so the keystroke is free, and it puts the mode
        // switch on the same key that opened the box.
        KeyCode::Char('/') => app.cycle_search_mode(),
        KeyCode::Char(c) => app.search_input_char(c),
        _ => {}
    }
    None
}

fn handle_tree_key(app: &mut App, key: KeyEvent) -> Option<Effect> {
    match key.code {
        KeyCode::Down | KeyCode::Char('j') => app.move_down(),
        KeyCode::Up | KeyCode::Char('k') => app.move_up(),
        KeyCode::Right | KeyCode::Enter | KeyCode::Char('l') => {
            return app.expand_selected().map(Effect::Fill)
        }
        KeyCode::Left | KeyCode::Char('h') => app.collapse_or_jump_to_parent(),
        KeyCode::Char('y') => return copy_text_for_selection(app).map(Effect::Copy),
        _ => {}
    }
    None
}

fn handle_detail_key(app: &mut App, key: KeyEvent) -> Option<Effect> {
    match key.code {
        KeyCode::Down | KeyCode::Char('j') => app.detail_scroll_down(),
        KeyCode::Up | KeyCode::Char('k') => app.detail_scroll_up(),
        // `h`/`l` and the arrow keys mean something different per pane
        // (spec §2): in the tree they collapse/expand, here they scroll
        // preformatted content horizontally. Scoped to this function, which
        // only runs while `Focus::Detail` — `handle_tree_key` keeps its own
        // meaning for the same keys untouched.
        KeyCode::Left | KeyCode::Char('h') => app.detail_hscroll_left(),
        KeyCode::Right | KeyCode::Char('l') => app.detail_hscroll_right(),
        KeyCode::Char('y') => return copy_text_for_selection(app).map(Effect::Copy),
        _ => {}
    }
    None
}

/// `y`: the selected flag's spelling if the detail pane has one implicitly
/// focused (batch 1 keeps this simple — flag-level selection within the
/// detail pane is a later refinement), otherwise the node's full command
/// path (spec §2: "Copy: the selected flag's spelling, or the node's full
/// command path").
fn copy_text_for_selection(app: &App) -> Option<String> {
    app.selected_row().map(|row| row.path.join(" "))
}

/// Handle a mouse event. `regions` must be the layout computed for the
/// frame currently on screen, so hit-testing lines up with what the user
/// sees. Tree rows are rendered at fixed column offsets (spec §9: "chevron
/// is hit when `col == 2*depth`"), which is what makes this arithmetic
/// rather than guesswork.
pub fn handle_mouse(app: &mut App, mouse: MouseEvent, regions: &Regions) -> Option<Effect> {
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            return handle_click(app, mouse.column, mouse.row, regions)
        }
        MouseEventKind::ScrollDown => handle_scroll(app, mouse.column, mouse.row, regions, 1),
        MouseEventKind::ScrollUp => handle_scroll(app, mouse.column, mouse.row, regions, -1),
        _ => {}
    }
    None
}

fn handle_click(app: &mut App, col: u16, row: u16, regions: &Regions) -> Option<Effect> {
    if let Some(tree_rect) = regions.tree {
        if rect_contains(tree_rect, col, row) {
            app.focus = Focus::Tree;
            // Inside the border: row 0 is the top border, so content
            // starts at tree_rect.y + 1; likewise column 0 is the left
            // border.
            let inner_row = row.saturating_sub(tree_rect.y + 1);
            let inner_col = col.saturating_sub(tree_rect.x + 1);
            app.ensure_rows_fresh();
            let idx = app.tree_scroll + inner_row as usize;
            if idx < app.rows().len() {
                let depth = app.rows()[idx].depth;
                let chevron_col = 2 * depth;
                if app.rows()[idx].has_children && inner_col as usize == chevron_col {
                    let path = app.rows()[idx].path.clone();
                    return app.toggle_expand_path(&path).map(Effect::Fill);
                } else {
                    // `select_index` resets the detail pane itself, and
                    // only when the click actually landed on a different
                    // row. A second unconditional reset here would undo
                    // that guard for the mouse alone.
                    app.select_index(idx);
                }
            }
            return None;
        }
    }
    if let Some(detail_rect) = regions.detail {
        if rect_contains(detail_rect, col, row) {
            app.focus = Focus::Detail;
        }
    }
    None
}

fn handle_scroll(app: &mut App, col: u16, row: u16, regions: &Regions, direction: i32) {
    if let Some(tree_rect) = regions.tree {
        if rect_contains(tree_rect, col, row) {
            if direction > 0 {
                app.tree_scroll = app.tree_scroll.saturating_add(1);
            } else {
                app.tree_scroll = app.tree_scroll.saturating_sub(1);
            }
            return;
        }
    }
    if let Some(detail_rect) = regions.detail {
        if rect_contains(detail_rect, col, row) {
            if direction > 0 {
                app.detail_scroll_down();
            } else {
                app.detail_scroll_up();
            }
        }
    }
}

fn rect_contains(rect: ratatui::layout::Rect, col: u16, row: u16) -> bool {
    col >= rect.x && col < rect.x + rect.width && row >= rect.y && row < rect.y + rect.height
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout;
    use mandible_core::{CommandNode, Provenance, Source};

    fn app() -> App {
        let mut root = CommandNode::new("git", Provenance::single(Source::HelpText));
        root.subcommands.push(CommandNode::new(
            "add",
            Provenance::single(Source::HelpText),
        ));
        App::new("git".to_string(), root)
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn q_quits() {
        let mut a = app();
        assert_eq!(
            handle_key(&mut a, key(KeyCode::Char('q'))),
            Some(Effect::Quit)
        );
    }

    #[test]
    fn ctrl_c_quits_even_while_in_search() {
        let mut a = app();
        a.focus_search();
        let ev = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(handle_key(&mut a, ev), Some(Effect::Quit));
    }

    #[test]
    fn slash_focuses_search_and_typing_updates_query() {
        let mut a = app();
        handle_key(&mut a, key(KeyCode::Char('/')));
        assert_eq!(a.focus, Focus::Search);
        handle_key(&mut a, key(KeyCode::Char('a')));
        handle_key(&mut a, key(KeyCode::Char('d')));
        assert_eq!(a.search_query, "ad");
    }

    #[test]
    fn arrow_keys_move_selection_while_search_stays_focused() {
        // Regression: arrows were previously a dead key while the search
        // box had focus, which is exactly the mode a user typing a filter
        // most wants to preview results in.
        let mut a = app();
        a.expand_selected();
        a.ensure_rows_fresh();
        a.focus_search();
        assert_eq!(a.selected, 0);
        handle_key(&mut a, key(KeyCode::Down));
        assert_eq!(a.selected, 1, "down arrow should move selection");
        assert_eq!(a.focus, Focus::Search, "focus should remain on search");
        handle_key(&mut a, key(KeyCode::Up));
        assert_eq!(a.selected, 0, "up arrow should move selection back");
        assert_eq!(a.focus, Focus::Search);
    }

    #[test]
    fn y_returns_copy_effect_with_node_path() {
        let mut a = app();
        assert_eq!(
            handle_key(&mut a, key(KeyCode::Char('y'))),
            Some(Effect::Copy("git".to_string()))
        );
    }

    /// `h`/`l` mean something different per pane (spec §2): in the tree
    /// they collapse/jump-to-parent and expand; in the detail pane they
    /// scroll preformatted content horizontally. Neither meaning should
    /// leak into the other pane's focus.
    #[test]
    fn h_and_l_are_scoped_to_the_focused_pane() {
        let mut a = app();
        a.expand_selected();
        a.ensure_rows_fresh();
        a.selected = 1; // "add", a leaf

        // Tree focus: `h` jumps to parent, unaffected by horizontal scroll.
        assert_eq!(a.focus, Focus::Tree);
        handle_key(&mut a, key(KeyCode::Char('h')));
        assert_eq!(a.selected_row().unwrap().name, "git");
        assert_eq!(a.clamped_detail_hscroll(), 0);

        // Detail focus: `l` scrolls horizontally rather than expanding a
        // tree row, and does not move the tree selection at all.
        a.toggle_focus();
        assert_eq!(a.focus, Focus::Detail);
        a.set_detail_hextent(50, 10); // give it something to scroll to
        let selected_before = a.selected;
        handle_key(&mut a, key(KeyCode::Char('l')));
        assert_eq!(a.selected, selected_before, "l must not move the tree");
        assert!(
            a.clamped_detail_hscroll() > 0,
            "l should have scrolled the detail pane right"
        );
        handle_key(&mut a, key(KeyCode::Char('h')));
        assert_eq!(a.clamped_detail_hscroll(), 0, "h should scroll back left");
    }

    #[test]
    fn r_requests_refresh() {
        let mut a = app();
        assert_eq!(
            handle_key(&mut a, key(KeyCode::Char('r'))),
            Some(Effect::Refresh)
        );
    }

    #[test]
    fn arrow_down_moves_selection() {
        let mut a = app();
        a.expand_selected();
        a.ensure_rows_fresh();
        assert_eq!(a.selected, 0);
        handle_key(&mut a, key(KeyCode::Down));
        assert_eq!(a.selected, 1);
    }

    #[test]
    fn help_overlay_closes_on_any_key() {
        let mut a = app();
        a.toggle_help();
        assert!(a.show_help);
        handle_key(&mut a, key(KeyCode::Char('x')));
        assert!(!a.show_help);
    }

    #[test]
    fn click_on_chevron_toggles_expand_not_select() {
        let mut a = app();
        a.ensure_rows_fresh();
        // App::new starts with the root already expanded, so there are 2
        // rows ("git", "add") to begin with.
        assert_eq!(a.rows().len(), 2);

        let regions = layout::compute(ratatui::layout::Rect::new(0, 0, 100, 30), Focus::Tree);
        let tree_rect = regions.tree.unwrap();
        // Root row (depth 0) is the first content row inside the border.
        let click_row = tree_rect.y + 1;
        let click_col = tree_rect.x + 1; // chevron column for depth 0
        handle_mouse(
            &mut a,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: click_col,
                row: click_row,
                modifiers: KeyModifiers::NONE,
            },
            &regions,
        );
        a.ensure_rows_fresh();
        assert_eq!(
            a.rows().len(),
            1,
            "clicking the already-expanded root's chevron should collapse it"
        );
        assert_eq!(
            a.selected, 0,
            "clicking a chevron must not change selection"
        );

        // Click it again: should re-expand.
        handle_mouse(
            &mut a,
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: click_col,
                row: click_row,
                modifiers: KeyModifiers::NONE,
            },
            &regions,
        );
        a.ensure_rows_fresh();
        assert_eq!(a.rows().len(), 2, "clicking again should re-expand");
    }
}
