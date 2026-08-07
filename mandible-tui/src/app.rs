//! Application state: the tree, expand/collapse state, selection, search,
//! focus, and scroll offsets. Pure state — no rendering and no terminal
//! I/O, so it's fully testable without a tty.

use crate::tree::{flatten, TreeRow};
use mandible_core::{resolve, CommandNode, NodeRef};
use mandible_search::SearchIndex;
use std::collections::HashSet;

/// What the search box matches against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchMode {
    /// Names only — command names and flag spellings. The default,
    /// because it is the mode whose results are self-explanatory: every
    /// row shown has the query visible in its name.
    Name,
    /// Names *and* summaries, descriptions, and flag values. Finds far
    /// more, at the cost of rows whose reason for matching isn't visible
    /// in the name column (searching `branch` in `git` surfaces `switch`
    /// via "Switch branches"), which is exactly why it isn't the default.
    Wide,
}

impl SearchMode {
    /// The label the search bar shows, so the active mode is never
    /// something the user has to remember.
    pub fn label(self) -> &'static str {
        match self {
            SearchMode::Name => "names",
            SearchMode::Wide => "names+text",
        }
    }

    fn toggled(self) -> SearchMode {
        match self {
            SearchMode::Name => SearchMode::Wide,
            SearchMode::Wide => SearchMode::Name,
        }
    }
}

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
    /// Re-extract the current tool from scratch (`r` key). There is no
    /// cache to bypass (spec §11) — this discards the in-memory tree
    /// (including anything lazily filled so far) and re-runs the pipeline,
    /// useful when the tool itself changed mid-session (a plugin installed,
    /// a new alias defined).
    Refresh,
    /// Quit the application.
    Quit,
    /// Lazily fill this node's children (spec §5.2 step 3) — it was just
    /// expanded but isn't known-complete yet. The caller should call
    /// [`App::mark_pending`], run the extraction, then
    /// [`App::splice_filled_node`] with the result.
    Fill(Vec<String>),
}

/// All mutable UI state for one tool's tree view.
pub struct App {
    /// The tool name as invoked, e.g. `"git"`.
    pub tool: String,
    /// The merged extraction tree.
    pub root: CommandNode,
    /// Paths the user has explicitly expanded.
    expanded: HashSet<Vec<String>>,
    /// Paths currently being lazily filled (spec §5.2 step 3) — a node
    /// whose `children_filled` was false when expanded, now being
    /// re-probed. Drives the spinner/placeholder row (spec §9). Populated
    /// and drained by the caller (the binary's event loop), which owns
    /// the actual extraction I/O; `App` itself never spawns anything.
    pending: HashSet<Vec<String>>,
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
    /// What the search box matches against. `/` enters search in
    /// [`SearchMode::Name`]; pressing `/` again toggles to
    /// [`SearchMode::Wide`].
    pub search_mode: SearchMode,
    /// Tree pane vertical scroll offset, in rows.
    pub tree_scroll: usize,
    /// How many tree rows the last rendered frame could show at once.
    /// Written by the renderer each frame and read by
    /// [`Self::follow_selection`] so keyboard navigation can scroll the
    /// viewport to keep the selection visible — without that, `↓` past
    /// the bottom row moved an invisible selection and the pane only
    /// caught up when the mouse wheel was used, which made the tool
    /// unusable from the keyboard alone. Zero until the first frame is
    /// drawn, which `follow_selection` treats as "viewport unknown, leave
    /// the scroll offset alone".
    pub tree_viewport: usize,
    /// Detail pane vertical scroll offset, in lines.
    pub detail_scroll: usize,
    /// Whether the `?` keybinding overlay is showing.
    pub show_help: bool,
    /// Whether hidden/deprecated items are shown (toggled with `.`).
    pub show_hidden: bool,
    /// A short-lived status line message (e.g. "copied: --interactive"),
    /// shown in the status bar until the next action replaces it.
    pub status_message: Option<String>,
    /// The `nucleo`-backed fuzzy index over this tree's commands and flags
    /// (spec §10). Populated from `root` at construction and whenever the
    /// tree's structure changes; queried (not text-matched directly) by
    /// [`Self::ensure_rows_fresh`] to compute which tree paths are
    /// currently showing as matches.
    search_index: SearchIndex,
    /// Whether rendering may use color at all (spec §9.2: respect
    /// `NO_COLOR`). Read from the environment once at construction — a
    /// terminal-color preference isn't something that changes mid-run —
    /// but kept as a plain `pub` field rather than re-read from `std::env`
    /// on every frame, so tests can set it directly without mutating
    /// process-wide environment state (which is unsound across Rust's
    /// parallel test runner).
    pub color_enabled: bool,
    /// When a search result is a flag (not a command), the flag's key
    /// within its parent command — set alongside `selected` so the detail
    /// pane can scroll to and highlight that specific flag instead of
    /// just landing on its parent command with the pane at the top (spec
    /// §10: "Selecting one selects the parent command and scrolls the
    /// detail pane to that flag"). Cleared on any navigation that isn't a
    /// search-result selection.
    pub selected_flag: Option<mandible_core::FlagKey>,
}

/// Subsequence match of `query` against `name`, case-insensitive — the
/// same shape of match the fuzzy index does, just restricted to the name
/// so `SearchMode::Name` stays consistent with what the tree pane
/// underlines. A plain `contains` would reject `gco` matching `checkout`
/// while the index accepted it, which would look like the filter
/// disagreeing with itself.
fn name_matches(name: &str, query: &str) -> bool {
    let mut haystack = name.chars().flat_map(char::to_lowercase);
    query
        .chars()
        .flat_map(char::to_lowercase)
        .all(|needle| haystack.any(|c| c == needle))
}

/// Same test against a flag's own spellings (`-v`, `--verbose`), so
/// name-mode search finds flags by how they're typed, never by their
/// description.
fn flag_key_matches(key: &mandible_core::FlagKey, query: &str) -> bool {
    let trimmed = query.trim_start_matches('-');
    match key {
        mandible_core::FlagKey::Long(l) => name_matches(l, trimmed),
        mandible_core::FlagKey::Short(c) => name_matches(&c.to_string(), trimmed),
    }
}

impl App {
    /// Build a new app over an already-extracted (and merged) tree, with
    /// the root expanded so the first screen isn't empty.
    pub fn new(tool: String, root: CommandNode) -> App {
        let mut expanded = HashSet::new();
        expanded.insert(vec![root.name.clone()]);
        let mut search_index = SearchIndex::new();
        search_index.populate(&root);
        let mut app = App {
            tool,
            root,
            expanded,
            pending: HashSet::new(),
            rows: Vec::new(),
            dirty: true,
            selected: 0,
            search_query: String::new(),
            search_pinned: None,
            focus: Focus::Tree,
            search_mode: SearchMode::Name,
            tree_scroll: 0,
            tree_viewport: 0,
            detail_scroll: 0,
            show_help: false,
            show_hidden: false,
            status_message: None,
            search_index,
            color_enabled: crate::style::color_enabled_from_env(),
            selected_flag: None,
        };
        app.ensure_rows_fresh();
        app
    }

    /// The currently active filter text, if any: the live query while
    /// typing, else the pinned filter. Also used by the tree pane to
    /// compute which characters within a row's name matched, for
    /// underlining (spec §9.2).
    pub fn active_filter(&self) -> Option<&str> {
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
        let matching_paths = self.compute_matching_paths();
        self.rows = flatten(
            &self.root,
            &self.expanded,
            matching_paths.as_ref(),
            self.show_hidden,
            &self.pending,
        );
        if self.selected >= self.rows.len() {
            self.selected = self.rows.len().saturating_sub(1);
        }
        self.sync_selected_flag_to_top_search_result();
        self.dirty = false;
        // Filtering/expanding can move the selection under the viewport
        // just as arrow keys can, so the same rule applies here.
        self.follow_selection();
    }

    /// Spec §10: "Selecting one [a flag search result] selects the parent
    /// command and scrolls the detail pane to that flag." The best-ranked
    /// (first) live search result stands in for "the selected result" —
    /// there's no separate cursor over the result list itself, only over
    /// the resulting filtered tree — so when it's a flag, jump the tree
    /// selection to its owning command and remember which flag to scroll
    /// the detail pane to; when it's a command, or there's no active
    /// filter at all, there's no flag target.
    fn sync_selected_flag_to_top_search_result(&mut self) {
        self.selected_flag = None;
        if self.active_filter().is_none() {
            return;
        }
        let Some(NodeRef::Flag { path, key }) = self.search_index.results().into_iter().next()
        else {
            return;
        };
        if let Some(idx) = self.rows.iter().position(|r| r.path == path) {
            self.selected = idx;
        }
        self.selected_flag = Some(key);
    }

    /// The set of command paths currently matching the active filter, if
    /// any, derived from [`SearchIndex::results`] — a
    /// [`mandible_core::NodeRef::Flag`] match contributes its *parent*
    /// command's path, since flags aren't tree rows (spec §2) but a flag
    /// match should still force-expand and highlight the command that
    /// owns it (spec §10: "Selecting one selects the parent command").
    fn compute_matching_paths(&self) -> Option<HashSet<Vec<String>>> {
        let filter = self.active_filter()?;
        if filter.trim().is_empty() {
            return None;
        }
        // The index matches one combined haystack per item (name plus
        // summary, description and flag value), which is what
        // `SearchMode::Wide` wants. `SearchMode::Name` narrows that same
        // result set down to items whose *name* actually matches, rather
        // than maintaining a second index: nucleo has already done the
        // expensive part, and this filter runs over matches only.
        let mut paths = HashSet::new();
        for node_ref in self.search_index.results() {
            match node_ref {
                NodeRef::Command(path) => {
                    if self.search_mode == SearchMode::Name
                        && !name_matches(path.last().map(String::as_str).unwrap_or(""), filter)
                    {
                        continue;
                    }
                    paths.insert(path);
                }
                NodeRef::Flag { path, key } => {
                    if self.search_mode == SearchMode::Name && !flag_key_matches(&key, filter) {
                        continue;
                    }
                    paths.insert(path);
                }
            }
        }
        Some(paths)
    }

    /// Drive the search index's background matcher forward (spec §10
    /// "Threading": must be called from the event loop's own poll timeout,
    /// never a blocking spin inside a keystroke handler). Marks the row
    /// list dirty if the result set changed, so the next
    /// [`Self::ensure_rows_fresh`] picks up fresh matches.
    pub fn tick_search(&mut self, timeout_ms: u64) {
        if self.search_index.tick(timeout_ms) {
            self.mark_dirty();
        }
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
        self.selected_flag = None;
        self.follow_selection();
    }

    /// Move the tree selection up one row.
    pub fn move_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
        self.selected_flag = None;
        self.follow_selection();
    }

    /// Scroll the tree pane the minimum amount needed to keep
    /// [`Self::selected`] on screen. Called after every operation that
    /// moves the selection, so the tool is fully keyboard-navigable:
    /// selection and viewport must never drift apart, because a selection
    /// the user cannot see is indistinguishable from the app ignoring
    /// their keypress.
    ///
    /// A zero [`Self::tree_viewport`] means no frame has been drawn yet,
    /// so there is no viewport to scroll and the offset is left alone.
    pub fn follow_selection(&mut self) {
        if self.tree_viewport == 0 {
            return;
        }
        if self.selected < self.tree_scroll {
            self.tree_scroll = self.selected;
        } else if self.selected >= self.tree_scroll + self.tree_viewport {
            self.tree_scroll = self.selected + 1 - self.tree_viewport;
        }
    }

    /// `→`/`Enter`/`l`: expand the selected row if it has children and
    /// isn't already expanded. (Lazy-extraction trigger is a later-batch
    /// concern; Tier A trees are always fully filled.)
    /// Returns `Some(path)` if the just-expanded node's children are not
    /// yet known-complete (spec §5.2 step 3: lazy per-node expansion) and
    /// isn't already being filled — the caller (the binary's event loop,
    /// which owns extraction I/O) should mark it pending via
    /// [`Self::mark_pending`], trigger a fill, and eventually call
    /// [`Self::splice_filled_node`] with the result. Also returns the
    /// path when the selected row has no discovered children yet at all
    /// (`!has_children`) but also isn't known-complete — the classic
    /// "never probed this node" case for a Tier-B-only tool.
    pub fn expand_selected(&mut self) -> Option<Vec<String>> {
        self.ensure_rows_fresh();
        let row = self.rows.get(self.selected)?;
        let needs_fill = !row.children_filled && !self.pending.contains(&row.path);
        let path = row.path.clone();
        // `needs_fill` is included deliberately: a node whose children
        // haven't been extracted yet reports `has_children == false`, but
        // the user pressing `→` on it is still a request to expand.
        // Recording that intent here is what lets `splice_filled_node`
        // stay purely mechanical — it expands nothing on its own, so the
        // background warmer filling the whole tree no longer unfolds the
        // whole tree on screen.
        if (row.has_children || needs_fill) && !self.expanded.contains(&path) {
            self.expanded.insert(path.clone());
            self.mark_dirty();
        }
        needs_fill.then_some(path)
    }

    /// Mark `path` as currently being lazily filled, so the tree pane
    /// shows a spinner row for it instead of a static chevron.
    pub fn mark_pending(&mut self, path: Vec<String>) {
        self.pending.insert(path);
        self.mark_dirty();
    }

    /// Clear a path's pending marker (called whether the fill succeeded
    /// or failed — either way, it's no longer in flight).
    pub fn clear_pending(&mut self, path: &[String]) {
        if self.pending.remove(path) {
            self.mark_dirty();
        }
    }

    /// True if `path` is currently being lazily filled.
    pub fn is_pending(&self, path: &[String]) -> bool {
        self.pending.contains(path)
    }

    /// Splice a freshly-filled node into the tree at `path`, replacing
    /// whatever was there, clears its pending marker, and — if
    /// the node now has children and wasn't already user-expanded —
    /// expands it, so the result the user asked for is immediately
    /// visible rather than requiring a second expand press.
    pub fn splice_filled_node(&mut self, path: &[String], node: CommandNode) {
        self.pending.remove(path);
        if let Some(slot) = mandible_core::resolve_mut(&mut self.root, path) {
            *slot = node;
        }
        // Deliberately does *not* expand the node. Every node in the tree
        // is filled in the background now, so auto-expanding on arrival
        // unfolded the entire tree and buried the user in rows they never
        // asked to see. Expansion is user intent, recorded by
        // `expand_selected`; a node the user already expanded is still in
        // `expanded`, so its children appear the moment they arrive.
        // The tree's structure (and searchable content) just changed —
        // keep the search index in sync. `populate` doesn't touch the
        // current query/pattern, only the item set, so an active search
        // simply re-matches against the freshly-filled data on the next
        // tick.
        self.search_index.populate(&self.root);
        self.mark_dirty();
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
        self.selected_flag = None;
        self.follow_selection();
    }

    /// Click/Enter/`l`/`→` shorthand: toggle expand state on the given row
    /// path (used by mouse chevron clicks, which address a row directly
    /// rather than "the selection"). Same fill-needed contract as
    /// [`Self::expand_selected`].
    pub fn toggle_expand_path(&mut self, path: &[String]) -> Option<Vec<String>> {
        let mut needs_fill = None;
        if self.expanded.contains(path) {
            self.expanded.remove(path);
        } else {
            self.expanded.insert(path.to_vec());
            let filled = resolve(&self.root, path)
                .map(|n| n.children_filled)
                .unwrap_or(true);
            if !filled && !self.pending.contains(path) {
                needs_fill = Some(path.to_vec());
            }
        }
        self.mark_dirty();
        needs_fill
    }

    /// Select the row at flattened index `idx`, if in range (used by mouse
    /// row clicks).
    pub fn select_index(&mut self, idx: usize) {
        self.ensure_rows_fresh();
        if idx < self.rows.len() {
            self.selected = idx;
        }
        self.selected_flag = None;
        self.follow_selection();
    }

    /// `/`: focus the search box. Pressing it again while the box is
    /// already focused toggles between matching names only and matching
    /// descriptions too — the narrow mode first, so the first thing a new
    /// user sees is the mode whose results explain themselves.
    pub fn cycle_search_mode(&mut self) {
        self.search_mode = self.search_mode.toggled();
        self.mark_dirty();
    }

    /// `/`: focus the search box.
    pub fn focus_search(&mut self) {
        self.focus = Focus::Search;
    }

    /// A character typed into the search box.
    pub fn search_input_char(&mut self, c: char) {
        self.search_query.push(c);
        self.search_index.set_query(&self.search_query);
        self.mark_dirty();
    }

    /// Backspace in the search box.
    pub fn search_backspace(&mut self) {
        self.search_query.pop();
        self.search_index.set_query(&self.search_query);
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
            self.search_index.set_query("");
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
        // A manual scroll always wins over an outstanding flag-scroll
        // target from a search selection — otherwise the next render
        // would just snap straight back to it.
        self.selected_flag = None;
        self.detail_scroll = self.detail_scroll.saturating_add(1);
    }

    /// Detail pane scroll up.
    pub fn detail_scroll_up(&mut self) {
        self.selected_flag = None;
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
    use mandible_core::{Provenance, Source};
    use std::time::{Duration, Instant};

    /// Drive the (real, async, `nucleo`-backed) search index until its
    /// results stop changing for a few consecutive polls, bounded overall
    /// so a bug can't hang the test suite. Mirrors how the real event loop
    /// calls `tick_search` from its own poll timeout (spec §10
    /// "Threading") rather than assuming a single call finishes matching.
    fn settle_search(app: &mut App) {
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut quiet_polls = 0;
        while Instant::now() < deadline && quiet_polls < 3 {
            let changed = app.search_index.tick(20);
            if changed {
                quiet_polls = 0;
            } else {
                quiet_polls += 1;
            }
        }
    }

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
        settle_search(&mut app);
        app.ensure_rows_fresh();
        assert!(app.rows().iter().any(|r| r.name == "--onto-helper"));

        app.escape_search();
        assert_eq!(app.focus, Focus::Tree);
        assert_eq!(app.search_pinned.as_deref(), Some("onto"));
        settle_search(&mut app);
        app.ensure_rows_fresh();
        assert!(
            app.rows().iter().any(|r| r.name == "--onto-helper"),
            "filter stays pinned"
        );

        app.escape_search();
        assert!(app.search_pinned.is_none());
        settle_search(&mut app);
        app.ensure_rows_fresh();
        assert_eq!(
            app.rows().len(),
            3,
            "filter cleared, back to expanded-root view"
        );
    }

    #[test]
    fn searching_a_flag_spelling_selects_its_parent_command() {
        // Spec §10: "Selecting one selects the parent command..." — since
        // flags aren't tree rows, a flag match must force-expand and
        // surface its *parent* in the filtered tree.
        let mut root = sample_tree();
        let mut autosquash =
            mandible_core::Flag::long("autosquash", Provenance::single(Source::HelpText));
        autosquash.description = Some(mandible_core::Text::sanitize(
            "Automatically squash commits",
        ));
        root.subcommands[1].flags.push(autosquash); // rebase

        let mut app = App::new("git".to_string(), root);
        app.focus_search();
        for c in "autosquash".chars() {
            app.search_input_char(c);
        }
        settle_search(&mut app);
        app.ensure_rows_fresh();

        let names: Vec<&str> = app.rows().iter().map(|r| r.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["git", "rebase"],
            "flag match should surface its parent command, not the flag itself: {names:?}"
        );

        // Closing spec §10's open item: the match also selects the parent
        // row and remembers which flag the detail pane should scroll to.
        assert_eq!(app.rows()[app.selected].name, "rebase");
        assert_eq!(
            app.selected_flag,
            Some(mandible_core::FlagKey::Long("autosquash".to_string()))
        );
    }

    /// Manually moving the tree selection away from a flag search match
    /// must drop the flag scroll target — otherwise the detail pane would
    /// keep snapping back to a flag on a command the user has since
    /// navigated away from.
    #[test]
    fn manual_navigation_clears_the_selected_flag_target() {
        let mut root = sample_tree();
        let mut autosquash =
            mandible_core::Flag::long("autosquash", Provenance::single(Source::HelpText));
        autosquash.description = Some(mandible_core::Text::sanitize(
            "Automatically squash commits",
        ));
        root.subcommands[1].flags.push(autosquash); // rebase

        let mut app = App::new("git".to_string(), root);
        app.focus_search();
        for c in "autosquash".chars() {
            app.search_input_char(c);
        }
        settle_search(&mut app);
        app.ensure_rows_fresh();
        assert!(app.selected_flag.is_some(), "precondition");

        app.move_down();
        assert_eq!(app.selected_flag, None);
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

    // --- lazy fill (spec §5.2 step 3) ---

    fn sample_tree_known_complete() -> CommandNode {
        let mut root = sample_tree();
        root.children_filled = true;
        for child in &mut root.subcommands {
            child.children_filled = true;
            for grandchild in &mut child.subcommands {
                grandchild.children_filled = true;
            }
        }
        root
    }

    #[test]
    fn expanding_a_known_complete_node_needs_no_fill() {
        let mut app = App::new("git".to_string(), sample_tree_known_complete());
        app.selected = 2; // rebase, children_filled: true
        assert_eq!(app.expand_selected(), None);
    }

    #[test]
    fn expanding_an_unfilled_node_reports_the_path_to_fill() {
        // sample_tree() defaults every node to children_filled: false.
        let mut app = App::new("git".to_string(), sample_tree());
        app.selected = 2; // rebase
        let needs_fill = app.expand_selected();
        assert_eq!(
            needs_fill,
            Some(vec!["git".to_string(), "rebase".to_string()])
        );
    }

    #[test]
    fn already_pending_node_is_not_reported_again() {
        let mut app = App::new("git".to_string(), sample_tree());
        app.selected = 2; // rebase
        let path = app.expand_selected().unwrap();
        app.mark_pending(path);
        // Collapsing and re-expanding while still pending must not queue
        // a second fill for the same path.
        app.collapse_or_jump_to_parent();
        let second = app.expand_selected();
        assert_eq!(
            second, None,
            "already-pending node should not be re-requested"
        );
    }

    #[test]
    fn pending_row_is_marked_in_flattened_rows() {
        let mut app = App::new("git".to_string(), sample_tree());
        app.selected = 2; // rebase
        let path = app.expand_selected().unwrap();
        app.mark_pending(path);
        app.ensure_rows_fresh();
        let rebase_row = app.rows().iter().find(|r| r.name == "rebase").unwrap();
        assert!(rebase_row.pending);
    }

    #[test]
    fn splice_filled_node_replaces_node_and_clears_pending() {
        let mut app = App::new("git".to_string(), sample_tree());
        app.selected = 2; // rebase
        let path = app.expand_selected().unwrap();
        app.mark_pending(path.clone());
        assert!(app.is_pending(&path));

        let mut filled = CommandNode::new("rebase", Provenance::single(Source::HelpText));
        filled.children_filled = true;
        filled.summary = Some(mandible_core::Text::sanitize("now filled"));
        app.splice_filled_node(&path, filled);

        assert!(!app.is_pending(&path));
        app.ensure_rows_fresh();
        let rebase_row = app.rows().iter().find(|r| r.name == "rebase").unwrap();
        assert!(!rebase_row.pending);
        assert_eq!(rebase_row.summary.as_deref(), Some("now filled"));
    }

    #[test]
    fn splice_filled_node_with_new_children_auto_expands() {
        let mut app = App::new("git".to_string(), sample_tree());
        app.selected = 2; // rebase
        let path = app.expand_selected().unwrap();
        app.mark_pending(path.clone());

        let mut filled = CommandNode::new("rebase", Provenance::single(Source::HelpText));
        filled.children_filled = true;
        filled.subcommands.push(CommandNode::new(
            "--onto",
            Provenance::single(Source::HelpText),
        ));
        app.splice_filled_node(&path, filled);

        app.ensure_rows_fresh();
        // The newly-discovered child should be visible without a second
        // expand press.
        assert!(app.rows().iter().any(|r| r.name == "--onto"));
    }
}
