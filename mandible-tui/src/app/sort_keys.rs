//! `s`: sorting the focused pane (spec §2, §9.1, §9.3).

use super::*;

impl App {
    /// `s`: sort the pane that has focus. The detail pane cycles through
    /// the tool's order, A to Z in each section, and A to Z overall; the
    /// commands pane switches between the tool's order and A to Z.
    pub fn sort_focused(&mut self) {
        match self.focus {
            Focus::Tree => self.sort_tree(),
            Focus::Detail => self.sort_detail(),
            Focus::Search => {}
        }
    }

    fn sort_tree(&mut self) {
        self.ensure_rows_fresh();
        let was = self.selected_path();
        self.sorts.tree = !self.sorts.tree;
        self.rebuild_in_place(was);
    }

    fn sort_detail(&mut self) {
        if self.showing_verbatim() || self.selected_node().is_none_or(|n| n.same_as_ancestor) {
            self.set_status("Nothing to sort in this view");
            return;
        }
        self.ensure_rows_fresh();
        let was = self.selected_path();
        self.sort_hold.hold();
        self.sorts.detail = self.sorts.detail.next();
        self.rebuild_in_place(was);
    }

    /// Rebuild the rows and the ranking after a sort changed, leaving the
    /// selected command selected and in view and the search where it was.
    fn rebuild_in_place(&mut self, was: Option<Vec<String>>) {
        self.keep_jump = true;
        self.mark_dirty();
        self.ensure_rows_fresh();
        self.keep_jump = false;
        if let Some(idx) = was
            .as_ref()
            .and_then(|path| self.rows.iter().position(|r| &r.path == path))
        {
            self.selected = idx;
        }
        self.follow_selection();
    }
}
