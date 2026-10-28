//! Sorting the two panes with `s` (spec §2, §9.1, §9.3).
//!
//! The detail pane cycles through three states and the commands pane
//! through two. Both are session state: they stay while the selection
//! moves to another command.

use crate::glyphs::Glyphs;
use mandible_core::Entity;
use std::cell::RefCell;

/// How the detail pane orders the rows of its list sections.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DetailSort {
    /// The order the tool printed them in.
    #[default]
    Printed,
    /// A to Z inside each group; the group headings stay.
    Sections,
    /// A to Z over the whole section; the group headings go.
    Overall,
}

impl DetailSort {
    /// The state `s` moves to.
    pub fn next(self) -> DetailSort {
        match self {
            DetailSort::Printed => DetailSort::Sections,
            DetailSort::Sections => DetailSort::Overall,
            DetailSort::Overall => DetailSort::Printed,
        }
    }

    /// What the pane title says about this state; nothing for the tool's
    /// own order.
    pub fn indicator(self, glyphs: Glyphs) -> Option<String> {
        match self {
            DetailSort::Printed => None,
            DetailSort::Sections => Some(format!("{} in sections", glyphs.a_to_z)),
            DetailSort::Overall => Some(glyphs.a_to_z.to_string()),
        }
    }
}

/// The sort state both panes and the search ranking read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Sorts {
    /// The detail pane's state.
    pub detail: DetailSort,
    /// Whether the commands pane is sorted A to Z.
    pub tree: bool,
}

/// The title tail for the commands pane: ` · A–Z`.
pub fn tree_indicator(sorted: bool, glyphs: Glyphs) -> Option<String> {
    sorted.then(|| glyphs.a_to_z.to_string())
}

/// The key rows sort by: the bare long name when the row has one, else the
/// short letter, else the first name it has, in lower case.
pub fn sort_name(entity: &Entity) -> String {
    entity
        .long()
        .or_else(|| entity.short_spelling().map(|s| s.name.as_str()))
        .unwrap_or_else(|| entity.primary_name())
        .to_lowercase()
}

/// Where a row stands in its page: the section's place in the page and the
/// row's place in the tool's own order within that section.
pub(crate) type RowId = (usize, usize);

/// The row at the top of the detail pane, and how far the pane is scrolled
/// into it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TopRow {
    pub(crate) path: Vec<String>,
    pub(crate) id: RowId,
    pub(crate) within: usize,
}

/// What the renderer and the key handler pass to each other so a page that
/// is re-ordered keeps the row that was at its top.
#[derive(Debug, Default)]
pub(crate) struct SortHold {
    seen: RefCell<Option<TopRow>>,
    held: RefCell<Option<TopRow>>,
}

impl SortHold {
    /// The renderer: the row at the top of the page just drawn.
    pub(crate) fn see(&self, top: Option<TopRow>) {
        *self.seen.borrow_mut() = top;
    }

    /// The key handler: keep the row last seen for the next draw.
    pub(crate) fn hold(&self) {
        let seen = self.seen.borrow().clone();
        *self.held.borrow_mut() = seen;
    }

    /// The renderer: the row to keep, once.
    pub(crate) fn take(&self) -> Option<TopRow> {
        self.held.borrow_mut().take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandible_core::{Provenance, Source};

    fn prov() -> Provenance {
        Provenance::single(Source::HelpText)
    }

    #[test]
    fn the_three_states_cycle() {
        let mut s = DetailSort::Printed;
        s = s.next();
        assert_eq!(s, DetailSort::Sections);
        s = s.next();
        assert_eq!(s, DetailSort::Overall);
        s = s.next();
        assert_eq!(s, DetailSort::Printed);
    }

    #[test]
    fn a_long_name_outranks_a_short_letter() {
        let e = Entity::flag_spelled(Some('z'), Some("alpha".into()), false, false, prov());
        assert_eq!(sort_name(&e), "alpha");
        assert_eq!(sort_name(&Entity::flag_short('Q', prov())), "q");
    }

    #[test]
    fn the_no_form_and_the_dashes_are_not_in_the_key() {
        let e = Entity::flag_spelled(None, Some("Squash".into()), false, true, prov());
        assert_eq!(sort_name(&e), "squash");
    }

    #[test]
    fn the_indicator_uses_the_glyph_set() {
        assert_eq!(
            DetailSort::Sections
                .indicator(crate::glyphs::UNICODE)
                .as_deref(),
            Some("A–Z in sections")
        );
        assert_eq!(
            DetailSort::Overall
                .indicator(crate::glyphs::ASCII)
                .as_deref(),
            Some("A-Z")
        );
        assert_eq!(DetailSort::Printed.indicator(crate::glyphs::ASCII), None);
        assert_eq!(
            tree_indicator(true, crate::glyphs::ASCII).as_deref(),
            Some("A-Z")
        );
        assert_eq!(tree_indicator(false, crate::glyphs::UNICODE), None);
    }
}
