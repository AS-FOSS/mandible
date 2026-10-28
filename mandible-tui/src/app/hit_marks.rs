//! What the active search matched, kept for marking the panes and for
//! stepping through a page's matching entities with `n`/`p` (spec §10).

use super::*;
use std::rc::Rc;

/// The matches of the finished search, by command path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct HitMarks {
    /// The keyword the hits were computed for.
    pub(crate) query: String,
    /// Commands that matched by their own name.
    pub(crate) commands: HashSet<Vec<String>>,
    /// Matching entities, by the command that owns them.
    pub(crate) entities: HashMap<Vec<String>, Vec<mandible_core::FlagKey>>,
}

impl HitMarks {
    pub(crate) fn from_hits(hits: &[NodeRef], query: &str) -> HitMarks {
        let mut marks = HitMarks {
            query: query.to_string(),
            ..HitMarks::default()
        };
        for hit in hits {
            match hit {
                NodeRef::Command(path) => {
                    marks.commands.insert(path.clone());
                }
                NodeRef::Flag { path, key } => marks
                    .entities
                    .entry(path.clone())
                    .or_default()
                    .push(key.clone()),
            }
        }
        marks
    }

    /// Whether a row is shown because it matched, not only to keep the
    /// tree's shape.
    pub(crate) fn is_hit(&self, path: &[String]) -> bool {
        self.commands.contains(path) || self.entities.contains_key(path)
    }

    /// How many entities of the command at `path` matched.
    pub(crate) fn entity_count(&self, path: &[String]) -> usize {
        self.entities.get(path).map_or(0, Vec::len)
    }
}

/// One matching entity row on the selected command's page, as the last
/// frame laid it out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MatchRow {
    pub(crate) key: mandible_core::FlagKey,
    pub(crate) first_line: usize,
    pub(crate) lines: usize,
    /// The row the last jump or `n`/`p` landed on: the only one on the bar.
    pub(crate) current: bool,
}

impl App {
    /// Replace the hit marks; the detail page is rebuilt when they change.
    pub(super) fn set_hit_marks(&mut self, marks: HitMarks) {
        if self.hit_marks != marks {
            self.hit_marks = marks;
            self.hit_marks_version += 1;
        }
    }

    /// The marks the detail page of the selected command is built with.
    pub(crate) fn hit_marks(&self) -> &HitMarks {
        &self.hit_marks
    }

    /// Bumps whenever [`Self::hit_marks`] changes; part of the page key.
    pub(crate) fn hit_marks_version(&self) -> u64 {
        self.hit_marks_version
    }

    /// Record the matching rows of the page the renderer just laid out.
    pub(crate) fn set_page_matches(&self, rows: Rc<Vec<MatchRow>>) {
        let path = self.selected_path().unwrap_or_default();
        *self.page_matches.borrow_mut() = (path, rows);
    }

    /// `n`/`p`: select the next or previous matching entity on the selected
    /// command's page, wrapping at the ends. Nothing happens without a
    /// search or without a match.
    pub fn step_match(&mut self, forward: bool) {
        if self.no_match || self.active_filter().is_none() {
            return;
        }
        let (path, rows) = self.page_matches.borrow().clone();
        if rows.is_empty() || Some(&path) != self.selected_path().as_ref() {
            return;
        }
        let len = rows.len();
        let current = self
            .selected_flag
            .as_ref()
            .and_then(|key| rows.iter().position(|m| &m.key == key));
        let target = match current {
            Some(i) if forward => (i + 1) % len,
            Some(i) => (i + len - 1) % len,
            None => {
                let top = self.clamped_detail_scroll();
                if forward {
                    rows.iter().position(|m| m.first_line > top).unwrap_or(0)
                } else {
                    rows.iter()
                        .rposition(|m| m.first_line < top)
                        .unwrap_or(len - 1)
                }
            }
        };
        self.selected_flag = Some(rows[target].key.clone());
        self.detail_hscroll = 0;
    }
}
