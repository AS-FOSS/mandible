//! The search box's dim suggestion and `Tab` to take it (spec §10).

use super::*;
use crate::search_ghost;

impl App {
    /// The suggestion to draw after the typed text: only while typing, and
    /// only when it was worked out for exactly the input in the box.
    pub(crate) fn ghost(&self) -> Option<&Ghost> {
        self.ghost
            .as_ref()
            .filter(|g| self.focus == Focus::Search && g.input == self.search_query)
    }

    /// The match list has a first jump target: suggest the rest of its
    /// spelling when that spelling starts with the input.
    pub(super) fn set_completion(&mut self, first: Option<&NodeRef>) {
        self.ghost = match (first, self.active_filter()) {
            (Some(first), Some(query)) => search_ghost::completion(&self.root, first, query),
            _ => None,
        };
    }

    /// Nothing matched: suggest the closest name, if one is near.
    pub(super) fn set_closest(&mut self) {
        self.ghost = self
            .active_filter()
            .and_then(|query| search_ghost::closest(&self.root, query, self.sorts));
    }

    /// `Tab` in the search box: put the suggested spelling in the box and
    /// search for it. Nothing happens without a suggestion.
    pub fn accept_ghost(&mut self) {
        let Some(accept) = self.ghost().map(|g| g.accept.clone()) else {
            return;
        };
        self.search_query = accept;
        self.search_index.set_query(&self.search_query);
        self.mark_dirty();
    }
}
