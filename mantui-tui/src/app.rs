//! Application state: the tree, expand/collapse state, selection, search,
//! focus, and scroll offsets. Pure state — no rendering and no terminal
//! I/O, so it's fully testable without a tty.

use crate::tree::{flatten, TreeRow};
use mantui_core::{resolve, CommandNode};
use std::collections::HashSet;

/// Which pane currently receives keyboard input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    /// The search bar.
    Search,
    /// The tree pane.
    Tree,
    /// The detail pane.
    Detail,
}

/// A request the app can't fulfill itself (needs the terminal or the OS
/// clipboard), surfaced to the caller's event loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Copy this text to the clipboard.
    Copy(String),
    /// Re-extract the current tool, bypassing the cache (`r` key).
    Refresh,
    /// Quit the application.
    Quit,
}

/// All mutable UI state for one tool's tree view.
pub struct App {
    /// The tool name as invoked, e.g. `"git"`.
    pub tool: String,
    /// The merged extraction tree.
    pub root: CommandNode,
    /// Paths the user has explicitly expanded.
    expanded: HashSet<Vec<String>>,
    /// The flattened, currently-visible row list. Rebuilt only when
    /// `dirty` (spec §9: "The flattened row list is cached").
    rows: Vec<TreeRow>,
    dirty: bool,
    /// Index into `rows` of the currently selected tree row.
    pub selected: usize,
    /// Live search box contents.
    pub search_query: String,
    /// The filter kept active after `Esc` leaves the search box (spec §2:
    /// "`Esc` Leave search, keeping the filter pinned; `Esc` again clears
    /// it").
    pub search_pinned: Option<String>,
    /// Which pane has input focus.
    pub focus: Focus,
    /// Tree pane vertical scroll offset, in rows.
    pub tree_scroll: usize,
    /// Detail pane vertical scroll offset, in lines.
    pub detail_scroll: usize,
    /// Whether the `?` keybinding overlay is showing.
    pub show_help: bool,
    /// Whether hidden/deprecated items are shown (toggled with `.`).
    pub show_hidden: bool,
    /// A short-lived status line message (e.g. "copied: --interactive"),
    /// shown in the status bar until the next action replaces it.
    pub status_message: Option<String>,
    /// True if this tree came from cache rather than a fresh extraction,
    /// for the "cached 3d ago" footer (spec §11).
    pub from_cache: bool,
}

impl App {
    /// Build a new app over an already-extracted (and merged) tree, with
    /// the root expanded so the first screen isn't empty.
    pub fn new(tool: String, root: CommandNode) -> App {
        let mut expanded = HashSet::new();
        expanded.insert(vec![root.name.clone()]);
        let mut app = App {
            tool,
            root,
            expanded,
            rows: Vec::new(),
            dirty: true,
            selected: 0,
            search_query: String::new(),
            search_pinned: None,
            focus: Focus::Tree,
            tree_scroll: 0,
            detail_scroll: 0,
            show_help: false,
            show_hidden: false,
            status_message: None,
            from_cache: false,
        };
        app.ensure_rows_fresh();
        app
    }

    /// The currently active filter text, if any: the live query while
    /// typing, else the pinned filter.
    fn active_filter(&self) -> Option<&str> {
        if !self.search_query.is_empty() {
            Some(self.search_query.as_str())
        } else {
            self.search_pinned.as_deref()
        }
    }

    /// Recompute `rows` if a structural change (expand/collapse, search,
    /// hidden-toggle) has happened since the last build. Cheap no-op
    /// otherwise. Callers must call this before reading `rows` — the event
    /// loop calls it once per iteration, right before rendering.
    pub fn ensure_rows_fresh(&mut self) {
        if !self.dirty {
            return;
        }
        let filter = self.active_filter().map(|s| s.to_string());
        self.rows = flatten(
            &self.root,
            &self.expanded,
            filter.as_deref(),
            self.show_hidden,
        );
        if self.selected >= self.rows.len() {
            self.selected = self.rows.len().saturating_sub(1);
        }
        self.dirty = false;
    }

    /// The current visible rows. Panics-free even if stale would be
    /// visually wrong but not unsafe; callers should call
    /// `ensure_rows_fresh` first.
    pub fn rows(&self) -> &[TreeRow] {
        &self.rows
    }

    /// The currently selected row, if any (empty tree edge case aside,
    /// there's always at least the root).
    pub fn selected_row(&self) -> Option<&TreeRow> {
        self.rows.get(self.selected)
    }

    /// The `CommandNode` the selected row addresses.
    pub fn selected_node(&self) -> Option<&CommandNode> {
        let row = self.selected_row()?;
        resolve(&self.root, &row.path)
    }

    fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Move the tree selection down one row.
    pub fn move_down(&mut self) {
        self.ensure_rows_fresh();
        if self.selected + 1 < self.rows.len() {
            self.selected += 1;
        }
    }

    /// Move the tree selection up one row.
    pub fn move_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    /// `→`/`Enter`/`l`: expand the selected row if it has children and
    /// isn't already expanded. (Lazy-extraction trigger is a later-batch
    /// concern; Tier A trees are always fully filled.)
    pub fn expand_selected(&mut self) {
        self.ensure_rows_fresh();
        if let Some(row) = self.rows.get(self.selected) {
            if row.has_children && !self.expanded.contains(&row.path) {
                self.expanded.insert(row.path.clone());
                self.mark_dirty();
            }
        }
    }

    /// `←`/`h`: collapse the selected row if it's expanded, else jump
    /// selection to its parent.
    pub fn collapse_or_jump_to_parent(&mut self) {
        self.ensure_rows_fresh();
        let Some(row) = self.rows.get(self.selected).cloned() else {
            return;
        };
        if row.has_children && self.expanded.contains(&row.path) {
            self.expanded.remove(&row.path);
            self.mark_dirty();
            self.ensure_rows_fresh();
            if let Some(idx) = self.rows.iter().position(|r| r.path == row.path) {
                self.selected = idx;
            }
        } else if row.path.len() > 1 {
            let parent_path = row.path[..row.path.len() - 1].to_vec();
            if let Some(idx) = self.rows.iter().position(|r| r.path == parent_path) {
                self.selected = idx;
            }
        }
    }

    /// Click/Enter/`l`/`→` shorthand: toggle expand state on the given row
    /// path (used by mouse chevron clicks, which address a row directly
    /// rather than "the selection").
    pub fn toggle_expand_path(&mut self, path: &[String]) {
        if self.expanded.contains(path) {
            self.expanded.remove(path);
        } else {
            self.expanded.insert(path.to_vec());
        }
        self.mark_dirty();
    }

    /// Select the row at flattened index `idx`, if in range (used by mouse
    /// row clicks).
    pub fn select_index(&mut self, idx: usize) {
        self.ensure_rows_fresh();
        if idx < self.rows.len() {
            self.selected = idx;
        }
    }

    /// `/`: focus the search box.
    pub fn focus_search(&mut self) {
        self.focus = Focus::Search;
    }

    /// A character typed into the search box.
    pub fn search_input_char(&mut self, c: char) {
        self.search_query.push(c);
        self.mark_dirty();
    }

    /// Backspace in the search box.
    pub fn search_backspace(&mut self) {
        self.search_query.pop();
        self.mark_dirty();
    }

    /// `Esc`: leave the search box, pinning the current query as the
    /// active filter. A second `Esc` (called while already out of the
    /// search box, i.e. this is called again with an empty live query)
    /// clears the pinned filter entirely.
    pub fn escape_search(&mut self) {
        if self.focus == Focus::Search {
            if !self.search_query.is_empty() {
                self.search_pinned = Some(std::mem::take(&mut self.search_query));
            }
            self.focus = Focus::Tree;
        } else if self.search_pinned.is_some() {
            self.search_pinned = None;
            self.search_query.clear();
            self.mark_dirty();
        }
    }

    /// `Tab`: move focus between tree and detail pane. (Search has its own
    /// entry/exit via `/` and `Esc`.)
    pub fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Tree => Focus::Detail,
            Focus::Detail => Focus::Tree,
            Focus::Search => Focus::Tree,
        };
    }

    /// `?`: toggle the keybinding overlay.
    pub fn toggle_help(&mut self) {
        self.show_help = !self.show_help;
    }

    /// `.`: toggle showing hidden/deprecated items.
    pub fn toggle_show_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
        self.mark_dirty();
    }

    /// Detail pane scroll down.
    pub fn detail_scroll_down(&mut self) {
        self.detail_scroll = self.detail_scroll.saturating_add(1);
    }

    /// Detail pane scroll up.
    pub fn detail_scroll_up(&mut self) {
        self.detail_scroll = self.detail_scroll.saturating_sub(1);
    }

    /// Reset detail scroll — called on selection change so the pane
    /// doesn't stay scrolled into a different node's content.
    pub fn reset_detail_scroll(&mut self) {
        self.detail_scroll = 0;
    }

    /// Set a status bar message (e.g. after a copy).
    pub fn set_status(&mut self, message: impl Into<String>) {
        self.status_message = Some(message.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mantui_core::{Provenance, Source};

    fn sample_tree() -> CommandNode {
        let mut root = CommandNode::new("git", Provenance::single(Source::HelpText));
        root.subcommands.push(CommandNode::new(
            "add",
            Provenance::single(Source::HelpText),
        ));
        let mut rebase = CommandNode::new("rebase", Provenance::single(Source::HelpText));
        rebase.subcommands.push(CommandNode::new(
            "--onto-helper",
            Provenance::single(Source::HelpText),
        ));
        root.subcommands.push(rebase);
        root
    }

    #[test]
    fn starts_with_root_expanded() {
        let app = App::new("git".to_string(), sample_tree());
        assert_eq!(app.rows().len(), 3); // git, add, rebase
    }

    #[test]
    fn expand_then_collapse_round_trips_row_count() {
        let mut app = App::new("git".to_string(), sample_tree());
        app.selected = 2; // rebase
        app.expand_selected();
        app.ensure_rows_fresh();
        assert_eq!(app.rows().len(), 4); // + --onto-helper

        app.collapse_or_jump_to_parent();
        app.ensure_rows_fresh();
        assert_eq!(app.rows().len(), 3);
    }

    #[test]
    fn collapse_on_leaf_jumps_to_parent() {
        let mut app = App::new("git".to_string(), sample_tree());
        app.selected = 2;
        app.expand_selected();
        app.ensure_rows_fresh();
        app.selected = 3; // --onto-helper, a leaf
        app.collapse_or_jump_to_parent();
        assert_eq!(app.selected_row().unwrap().name, "rebase");
    }

    #[test]
    fn rows_are_not_rebuilt_on_pure_navigation() {
        let mut app = App::new("git".to_string(), sample_tree());
        app.ensure_rows_fresh();
        let before = app.rows().to_vec();
        app.move_down();
        app.move_down();
        app.move_up();
        // ensure_rows_fresh should be a no-op (dirty flag never set by
        // move_up/move_down), so identical Vec contents prove no rebuild
        // silently changed anything.
        app.ensure_rows_fresh();
        assert_eq!(app.rows(), before.as_slice());
    }

    #[test]
    fn search_pins_on_escape_and_clears_on_second_escape() {
        let mut app = App::new("git".to_string(), sample_tree());
        app.focus_search();
        for c in "onto".chars() {
            app.search_input_char(c);
        }
        app.ensure_rows_fresh();
        assert!(app.rows().iter().any(|r| r.name == "--onto-helper"));

        app.escape_search();
        assert_eq!(app.focus, Focus::Tree);
        assert_eq!(app.search_pinned.as_deref(), Some("onto"));
        app.ensure_rows_fresh();
        assert!(
            app.rows().iter().any(|r| r.name == "--onto-helper"),
            "filter stays pinned"
        );

        app.escape_search();
        assert!(app.search_pinned.is_none());
        app.ensure_rows_fresh();
        assert_eq!(
            app.rows().len(),
            3,
            "filter cleared, back to expanded-root view"
        );
    }

    #[test]
    fn selected_node_resolves_via_noderef() {
        let mut app = App::new("git".to_string(), sample_tree());
        app.selected = 1; // add
        assert_eq!(app.selected_node().unwrap().name, "add");
    }

    #[test]
    fn toggle_focus_cycles_tree_and_detail() {
        let mut app = App::new("git".to_string(), sample_tree());
        assert_eq!(app.focus, Focus::Tree);
        app.toggle_focus();
        assert_eq!(app.focus, Focus::Detail);
        app.toggle_focus();
        assert_eq!(app.focus, Focus::Tree);
    }
}
