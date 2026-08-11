//! Application state: the tree, expand/collapse state, selection, search,
//! focus, and scroll offsets. Pure state — no rendering and no terminal
//! I/O, so it's fully testable without a tty.

use crate::tree::{flatten, TreeRow};
use mandible_core::{resolve, CommandNode, NodeRef, Text};
use mandible_search::SearchIndex;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// What the search box matches against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchMode {
    /// Command names only, matched as a literal substring. The default,
    /// and the mode whose results need no explanation: every row shown has
    /// the query visible in its own name, and the rows around it are gone.
    Name,
    /// Everything: command names, flag spellings, summaries, descriptions
    /// and flag values, matched fuzzily. This is where you find a flag by
    /// its spelling, or a command by what it does. Rows appear whose names
    /// don't contain the query — that is the point of the mode, and why it
    /// isn't the default.
    Wide,
}

impl SearchMode {
    /// The label the search bar shows, so the active mode is never
    /// something the user has to remember.
    pub fn label(self) -> &'static str {
        match self {
            SearchMode::Name => "names",
            SearchMode::Wide => "everything",
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
    /// Fetch this node's raw `--help` output for the verbatim view (`t`).
    /// The caller should run the probe and hand the result back through
    /// [`App::set_raw_help`].
    FetchRaw(Vec<String>),
}

/// One node's raw `--help` text, as far as the verbatim view has got.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawHelp {
    /// The probe was requested but hasn't returned yet.
    Pending,
    /// The tool's own output, one sanitized line per entry, paired with
    /// the argv that produced it (e.g. `"git commit -h"`).
    ///
    /// The argv is carried, not assumed, because since [M-16] a
    /// subcommand's text may come from `--help` *or* `-h`, and this view's
    /// only job is telling the reader exactly what we were given. A pane
    /// that hardcodes one spelling while showing the other's bytes states
    /// something untrue in the one place a reader comes to check us.
    Ready(Vec<Text>, String),
    /// The probe failed or was refused (spec §6 rule 0). Carries the
    /// reason, which is shown in place of the text — a verbatim view that
    /// silently shows nothing would be indistinguishable from a tool that
    /// prints nothing.
    Failed(String),
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
    /// Whether the detail pane is showing the tool's own `--help` bytes
    /// instead of mandible's reading of them (`t`).
    ///
    /// A *mode*, not a per-node flag: once you have started distrusting a
    /// parse you are usually comparing several nodes, and having the view
    /// snap back to parsed on every arrow key would make that comparison
    /// impossible. Moving the selection while it is on fetches the new
    /// node's raw text instead.
    pub raw_mode: bool,
    /// Raw `--help` text per node path, as fetched. Only ever populated
    /// for nodes the user actually asked to see verbatim, which is why
    /// this is a map rather than a field on every `CommandNode`.
    raw_help: HashMap<Vec<String>, RawHelp>,
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
    /// How far the detail pane can usefully scroll: its rendered line count
    /// minus the visible height, written by the renderer each frame.
    ///
    /// Interior mutability because rendering takes `&App`, and this is the
    /// one fact only the renderer knows — it is the built line count, which
    /// depends on wrapping at the current width. Without it `↓` incremented
    /// the offset forever: holding the key on a description that already
    /// fit scrolled the content off the top into blank space, and getting
    /// back required as many presses up as had gone down.
    detail_max_scroll: std::cell::Cell<usize>,
    /// Whether the `?` keybinding overlay is showing.
    pub show_help: bool,
    /// Whether hidden/deprecated items are shown (toggled with `.`).
    pub show_hidden: bool,
    /// A short-lived status line message (e.g. "copied: --interactive").
    ///
    /// Genuinely short-lived: it used to sit in the footer until some
    /// *other* action happened to overwrite it, so a single `y` replaced
    /// the keybinding hints for the rest of the session. See
    /// [`Self::expire_status`].
    pub status_message: Option<String>,
    /// When [`Self::status_message`] should disappear. `None` whenever
    /// there is no message.
    status_expires_at: Option<Instant>,
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
    /// The glyph set this terminal can actually draw, chosen once from the
    /// locale (see [`crate::glyphs`]). A plain field rather than a lookup
    /// per frame for the same reason `color_enabled` is: it cannot change
    /// mid-run, and tests need to set it directly without mutating
    /// process-wide environment state.
    pub glyphs: crate::glyphs::Glyphs,
    /// When a search result is a flag (not a command), the flag's key
    /// within its parent command — set alongside `selected` so the detail
    /// pane can scroll to and highlight that specific flag instead of
    /// just landing on its parent command with the pane at the top (spec
    /// §10: "Selecting one selects the parent command and scrolls the
    /// detail pane to that flag"). Cleared on any navigation that isn't a
    /// search-result selection.
    pub selected_flag: Option<mandible_core::FlagKey>,
}

/// How long a status message stays in the footer before the keybinding
/// hints come back. Long enough to read a copied flag spelling, short
/// enough that the hints are never gone when someone looks up needing
/// them.
const STATUS_MESSAGE_TTL: Duration = Duration::from_secs(4);

/// Case-insensitive **substring** match of `query` against `name`.
///
/// Deliberately literal, not a subsequence, because that is the whole
/// point of `SearchMode::Name` existing as a separate mode. Subsequence
/// matching is what made the narrow mode feel broken: searching `run` in
/// `docker` surfaced `ps` and `build`, because `--no-trunc` contains
/// r…u…n in order and a matching flag surfaces its parent command. Every
/// hit was technically correct and none of them looked it.
///
/// So the two modes are now genuinely different kinds of search rather
/// than the same search over different fields:
///
/// - [`SearchMode::Name`] — literal substring, over names and flag
///   spellings. Every row shown contains what you typed.
/// - [`SearchMode::Wide`] — the fuzzy index over names, summaries,
///   descriptions and flag values, where `gco` still finds `checkout`.
///
/// One keystroke apart, and the search bar says which is active.
fn name_matches(name: &str, query: &str) -> bool {
    name.to_lowercase().contains(&query.to_lowercase())
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
            raw_mode: false,
            raw_help: HashMap::new(),
            search_mode: SearchMode::Name,
            tree_scroll: 0,
            tree_viewport: 0,
            detail_scroll: 0,
            detail_max_scroll: std::cell::Cell::new(0),
            show_help: false,
            show_hidden: false,
            status_message: None,
            status_expires_at: None,
            search_index,
            color_enabled: crate::style::color_enabled_from_env(),
            glyphs: crate::glyphs::from_env(),
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
                NodeRef::Flag { path, .. } => {
                    // A flag match surfaces its *parent command*, which in
                    // name mode means rows whose own names don't contain
                    // the query: searching `run` in `docker` returned `ps`,
                    // because `--no-trunc` contains "run". Correct, and
                    // indistinguishable from a broken filter. Name mode is
                    // now exactly what its label says — command names — and
                    // flags are found in `Wide`, one keystroke away.
                    if self.search_mode == SearchMode::Name {
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

    /// The selected row's path from the root, e.g. `["git", "rebase"]`.
    pub fn selected_path(&self) -> Option<Vec<String>> {
        Some(self.selected_row()?.path.clone())
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

    /// Swap in a freshly extracted tree (`r`), keeping the user where they
    /// were.
    ///
    /// Refresh used to be `*app = App::new(tool, root)`, which threw away
    /// every expanded node, the selection, the scroll position, the search
    /// filter and the view mode. That is a poor trade for a key whose whole
    /// purpose is "the tool changed under me": you press it *because* you
    /// want to keep looking at the thing you are looking at, and it dumped
    /// you back at the root.
    ///
    /// The selection is restored by path rather than by row index, since a
    /// re-extract can change how many rows precede it. A node that no
    /// longer exists in the new tree leaves the selection clamped to the
    /// nearest valid row instead of pointing past the end.
    pub fn reload(&mut self, root: CommandNode) {
        let previously_selected = self.selected_path();

        self.root = root;
        // Fills queued against the old tree are abandoned by the caller,
        // so nothing is still pending for this one.
        self.pending.clear();
        // Re-probed on demand. The point of a re-extract is that the tool
        // may have changed, which makes cached raw output exactly as stale
        // as the tree it came from.
        self.raw_help.clear();

        self.search_index = SearchIndex::new();
        self.search_index.populate(&self.root);
        let filter = self.active_filter().unwrap_or("").to_string();
        self.search_index.set_query(&filter);

        self.mark_dirty();
        self.ensure_rows_fresh();

        self.selected = previously_selected
            .and_then(|path| self.rows.iter().position(|row| row.path == path))
            .unwrap_or_else(|| self.selected.min(self.rows.len().saturating_sub(1)));
        self.detail_scroll = 0;
    }

    /// `t`: toggle the verbatim view — the tool's own `--help` bytes
    /// instead of mandible's reading of them.
    ///
    /// This is the escape hatch for the one failure the rest of the
    /// pipeline cannot signal. A node that degraded to verbatim already
    /// says so, and a thin parse carries a low-confidence warning; what
    /// neither covers is a grammar that produced a confident, well-formed,
    /// *wrong* tree, which looks exactly like a correct one. Rather than
    /// asking the user to trust a number, this shows them the source text
    /// and lets them check.
    ///
    /// Returns the fetch the caller must run, if the text isn't already
    /// in hand. Turning the mode *off* never needs one.
    pub fn toggle_raw_mode(&mut self) -> Option<Effect> {
        self.raw_mode = !self.raw_mode;
        // Scroll offsets don't carry between two completely different
        // renderings of the same node: line 40 of a flag table is not line
        // 40 of the raw text, so keeping the offset lands the reader
        // somewhere arbitrary in whichever view they just switched to.
        self.detail_scroll = 0;
        self.raw_fetch_needed()
    }

    /// The fetch required to render the current selection verbatim, if the
    /// mode is on and this node's text is neither in hand nor already in
    /// flight.
    ///
    /// Called both by [`Self::toggle_raw_mode`] and by the event loop after
    /// every event, which is what makes the mode survive moving the
    /// selection: each newly-selected node gets fetched once, on arrival.
    pub fn raw_fetch_needed(&self) -> Option<Effect> {
        if !self.raw_mode {
            return None;
        }
        let path = self.selected_path()?;
        if self.raw_help.contains_key(&path) {
            return None;
        }
        Some(Effect::FetchRaw(path))
    }

    /// Mark a raw fetch as in flight, so the renderer can say so and
    /// [`Self::raw_fetch_needed`] doesn't queue it a second time.
    pub fn mark_raw_pending(&mut self, path: Vec<String>) {
        self.raw_help.insert(path, RawHelp::Pending);
    }

    /// Hand back the result of a [`Effect::FetchRaw`].
    pub fn set_raw_help(&mut self, path: Vec<String>, result: RawHelp) {
        self.raw_help.insert(path, result);
    }

    /// The verbatim text for the current selection, if the mode is on.
    /// `None` means "render the parsed view", so a failed fetch still
    /// returns `Some(RawHelp::Failed)` rather than silently falling back —
    /// switching views must never look like it did nothing.
    pub fn raw_help_for_selected(&self) -> Option<&RawHelp> {
        if !self.raw_mode {
            return None;
        }
        self.raw_help.get(&self.selected_path()?)
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
        let max = self.detail_max_scroll.get();
        self.detail_scroll = self.detail_scroll.saturating_add(1).min(max);
    }

    /// Record how far the detail pane can scroll, from the renderer.
    /// Clamps any offset already past the new limit, so a resize that makes
    /// the pane taller (or a move to a shorter node) doesn't leave the view
    /// stranded below its own content.
    pub fn set_detail_extent(&self, content_lines: usize, viewport_lines: usize) {
        self.detail_max_scroll
            .set(content_lines.saturating_sub(viewport_lines));
    }

    /// The current scroll offset, clamped to what the last frame could
    /// actually show.
    pub fn clamped_detail_scroll(&self) -> usize {
        self.detail_scroll.min(self.detail_max_scroll.get())
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
        self.set_status_at(message, Instant::now());
    }

    /// [`Self::set_status`] with an explicit clock, so the expiry is
    /// testable without sleeping.
    pub fn set_status_at(&mut self, message: impl Into<String>, now: Instant) {
        self.status_message = Some(message.into());
        self.status_expires_at = Some(now + STATUS_MESSAGE_TTL);
    }

    /// Drop the status message once its time is up, restoring the
    /// keybinding hints. Called from the event loop, which already wakes
    /// every 100ms to poll for input, so no extra timer is needed.
    pub fn expire_status(&mut self, now: Instant) {
        if self.status_expires_at.is_some_and(|at| now >= at) {
            self.status_message = None;
            self.status_expires_at = None;
        }
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
    fn a_status_message_expires_and_restores_the_hints() {
        let mut app = App::new("git".to_string(), sample_tree());
        let t0 = Instant::now();
        app.set_status_at("copied: --interactive", t0);
        assert!(app.status_message.is_some());

        // Still visible just before the deadline...
        app.expire_status(t0 + STATUS_MESSAGE_TTL - Duration::from_millis(1));
        assert!(app.status_message.is_some(), "expired too early");

        // ...and gone at it. Previously nothing cleared it at all, so one
        // `y` replaced the keybinding hints for the rest of the session.
        app.expire_status(t0 + STATUS_MESSAGE_TTL);
        assert!(app.status_message.is_none(), "should have expired");
    }

    #[test]
    fn searching_a_flag_spelling_selects_its_parent_command() {
        // Spec §10: "Selecting one selects the parent command..." — since
        // flags aren't tree rows, a flag match must force-expand and
        // surface its *parent* in the filtered tree.
        //
        // In `SearchMode::Wide`, which is where flags are searched. Name
        // mode deliberately matches command names only: a flag match there
        // surfaced parents whose own names didn't contain the query, which
        // is correct and reads as a broken filter.
        let mut root = sample_tree();
        let mut autosquash =
            mandible_core::Flag::long("autosquash", Provenance::single(Source::HelpText));
        autosquash.description = Some(mandible_core::Text::sanitize(
            "Automatically squash commits",
        ));
        root.subcommands[1].flags.push(autosquash); // rebase

        let mut app = App::new("git".to_string(), root);
        app.focus_search();
        app.cycle_search_mode(); // Name -> Wide
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
        app.cycle_search_mode(); // Name -> Wide
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

    /// Re-extract used to be `*app = App::new(...)`, which dumped the
    /// user back at the root with everything collapsed. You press `r`
    /// *because* you want to keep looking at what you are looking at.
    #[test]
    fn reload_keeps_the_selection_on_the_same_node() {
        let mut app = App::new("git".to_string(), sample_tree());
        app.selected = 2; // rebase
        assert_eq!(app.selected_node().unwrap().name, "rebase");

        app.reload(sample_tree());

        assert_eq!(
            app.selected_node().unwrap().name,
            "rebase",
            "selection is restored by path, not by row index"
        );
    }

    /// A re-extract can legitimately remove a node. That must clamp, not
    /// leave the selection pointing past the end of the row list.
    #[test]
    fn reload_clamps_when_the_selected_node_is_gone() {
        let mut app = App::new("git".to_string(), sample_tree());
        app.selected = 2; // rebase

        let mut shrunk = CommandNode::new("git", Provenance::single(Source::HelpText));
        shrunk.subcommands.push(CommandNode::new(
            "add",
            Provenance::single(Source::HelpText),
        ));
        app.reload(shrunk);

        assert!(app.selected < app.rows().len(), "selection ran off the end");
        assert!(app.selected_node().is_some());
    }

    /// Expansion survives, so a re-extract does not collapse a tree the
    /// user spent time opening.
    #[test]
    fn reload_keeps_expanded_nodes_expanded() {
        // The known-complete fixture, so expanding actually reveals the
        // child rather than requesting a fill for it.
        let mut app = App::new("git".to_string(), sample_tree_known_complete());
        app.selected = 2; // rebase
        app.expand_selected();
        // `rows()` is a cache; the event loop refreshes it each iteration.
        app.ensure_rows_fresh();
        let expanded_rows = app.rows().len();
        assert!(expanded_rows > 3, "precondition: rebase opened a child");

        app.reload(sample_tree_known_complete());

        assert_eq!(app.rows().len(), expanded_rows);
    }

    /// Cached verbatim text describes the tree that was just thrown away,
    /// which makes it exactly as stale as the tree itself.
    #[test]
    fn reload_drops_cached_raw_help() {
        let mut app = App::new("git".to_string(), sample_tree());
        app.toggle_raw_mode();
        app.set_raw_help(
            vec!["git".to_string()],
            RawHelp::Ready(vec![Text::sanitize("stale")], "t --help".to_string()),
        );
        assert!(app.raw_help_for_selected().is_some());

        app.reload(sample_tree());

        assert!(
            app.raw_help_for_selected().is_none(),
            "must re-probe rather than show output from the previous tree"
        );
        assert!(app.raw_mode, "the view mode itself is the user's choice");
    }

    /// Fills queued against the old tree are abandoned by the caller, so
    /// nothing should still render as loading afterwards.
    #[test]
    fn reload_clears_pending_fills() {
        let mut app = App::new("git".to_string(), sample_tree());
        app.mark_pending(vec!["git".to_string(), "rebase".to_string()]);
        app.reload(sample_tree());
        assert!(!app.is_pending(&["git".to_string(), "rebase".to_string()]));
    }

    /// `t` asks for the text it doesn't have, and the fetch names the
    /// selected node rather than the root.
    #[test]
    fn toggling_raw_mode_requests_the_selected_nodes_help_text() {
        let mut app = App::new("git".to_string(), sample_tree());
        app.selected = 2; // rebase
        assert_eq!(
            app.toggle_raw_mode(),
            Some(Effect::FetchRaw(vec![
                "git".to_string(),
                "rebase".to_string()
            ]))
        );
        assert!(app.raw_mode);
    }

    /// Turning the mode off asks for nothing, and — the part worth
    /// asserting — leaves the parsed view showing even for a node whose
    /// raw text is still cached from earlier.
    #[test]
    fn leaving_raw_mode_needs_no_fetch_and_hides_cached_text() {
        let mut app = App::new("git".to_string(), sample_tree());
        app.toggle_raw_mode();
        app.set_raw_help(
            vec!["git".to_string()],
            RawHelp::Ready(vec![Text::sanitize("usage: git")], "git --help".to_string()),
        );
        assert!(app.raw_help_for_selected().is_some());

        assert_eq!(app.toggle_raw_mode(), None);
        assert!(!app.raw_mode);
        assert!(app.raw_help_for_selected().is_none());
    }

    /// The mode survives moving the selection: each newly-selected node
    /// gets its own fetch, which is what `raw_fetch_needed` exists for.
    #[test]
    fn moving_the_selection_in_raw_mode_fetches_the_new_node() {
        let mut app = App::new("git".to_string(), sample_tree());
        app.toggle_raw_mode();
        app.set_raw_help(
            vec!["git".to_string()],
            RawHelp::Ready(Vec::new(), "git --help".to_string()),
        );
        assert_eq!(app.raw_fetch_needed(), None, "root is already in hand");

        app.move_down(); // add
        assert_eq!(
            app.raw_fetch_needed(),
            Some(Effect::FetchRaw(vec!["git".to_string(), "add".to_string()]))
        );
    }

    /// An in-flight fetch must not be queued a second time on every loop
    /// iteration — the event loop calls `raw_fetch_needed` after *every*
    /// event, so a `Pending` entry that didn't suppress the request would
    /// re-probe the tool at the poll rate.
    #[test]
    fn a_pending_fetch_is_not_requested_again() {
        let mut app = App::new("git".to_string(), sample_tree());
        app.toggle_raw_mode();
        app.mark_raw_pending(vec!["git".to_string()]);
        assert_eq!(app.raw_fetch_needed(), None);
    }

    /// A refused or failed probe stays visible instead of silently
    /// reverting to the parsed view. Pressing `t` on `kill` must say why
    /// nothing is shown, since a blank pane is also what a tool that
    /// prints nothing looks like.
    #[test]
    fn a_failed_fetch_is_still_rendered_as_raw() {
        let mut app = App::new("kill".to_string(), sample_tree());
        app.toggle_raw_mode();
        app.set_raw_help(
            vec!["git".to_string()],
            RawHelp::Failed("refused: never probed".to_string()),
        );
        assert!(matches!(
            app.raw_help_for_selected(),
            Some(RawHelp::Failed(_))
        ));
    }

    /// Line 40 of a flag table is not line 40 of the raw text, so carrying
    /// the offset across the switch lands the reader somewhere arbitrary.
    #[test]
    fn switching_views_resets_the_detail_scroll() {
        let mut app = App::new("git".to_string(), sample_tree());
        app.detail_scroll = 40;
        app.toggle_raw_mode();
        assert_eq!(app.detail_scroll, 0);
        app.detail_scroll = 12;
        app.toggle_raw_mode();
        assert_eq!(app.detail_scroll, 0);
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
