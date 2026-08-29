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
    /// A scroll *proportion* (0.0..=1.0) carried across a raw/rendered
    /// toggle, resolved against the next view's extent. Line offsets are
    /// meaningless across the two renderings of one node, but the reader's
    /// approximate place in the document is not — this is what lets `t`
    /// flip between them for comparison without snapping to the top.
    /// A `Cell` because the renderer (`&App`) is the first to know the new
    /// extent; key handlers (`&mut App`) materialize it into
    /// [`Self::detail_scroll`] before applying their own movement.
    pending_detail_fraction: std::cell::Cell<Option<f64>>,
    /// Detail pane horizontal scroll offset, in display columns. Only ever
    /// nonzero for preformatted content (the raw `--help` view and
    /// USAGE-section synopsis lines, spec §9) — prose is always wrapped to
    /// the pane width and has nothing to scroll to.
    detail_hscroll: usize,
    /// How far the detail pane's widest *preformatted* line extends past
    /// the current viewport width, written by the renderer each frame —
    /// same `Cell`-for-interior-mutability reasoning as
    /// [`Self::detail_max_scroll`]: rendering takes `&App`, and the content
    /// width at the current pane width is a fact only the renderer knows.
    /// Zero when nothing on screen is preformatted (plain prose has no
    /// horizontal extent worth scrolling to), which both clamps `h`/`l` to
    /// no-ops and tells the renderer not to draw the overflow affordance.
    detail_max_hscroll: std::cell::Cell<usize>,
    /// The horizontal twin of [`Self::pending_detail_fraction`]: a scroll
    /// proportion carried across a raw/rendered toggle, resolved against
    /// the next view's horizontal extent. Same lifecycle: set by the
    /// toggle, shown by [`Self::clamped_detail_hscroll`], materialized by
    /// the first `h`/`l` press, dropped on selection change.
    pending_detail_hfraction: std::cell::Cell<Option<f64>>,
    /// Whether `h`/`l`/`←`/`→` scroll the detail pane horizontally instead
    /// of doing nothing there, and whether preformatted content (raw view,
    /// USAGE lines) is left unwrapped in the first place.
    ///
    /// Defaults to `true` here — [`App::new`] is a pure constructor with no
    /// filesystem I/O, so it never itself reads
    /// `~/.config/mandible/config.toml`. Reading that file is the
    /// composition root's job: `mandible/src/app_runner.rs`'s `new_app`
    /// calls `mandible_core::config::load` once and overwrites this field
    /// on the freshly-built `App`, the same way every other startup concern
    /// already lives there rather than in `App::new`. A constructor that
    /// touches the real filesystem made every test (and every other
    /// embedder of this crate) hostage to whichever `config.toml` happened
    /// to exist on the machine running them — a `horizontal_scroll = false`
    /// left over from someone's own use of mandible silently flipped this
    /// field's default in the test suite and made "off reproduces today's
    /// behavior" pass for the wrong reason.
    ///
    /// A plain `pub` field, like [`Self::color_enabled`] and
    /// [`Self::glyphs`], so tests can set it directly without any global
    /// state to isolate.
    pub horizontal_scroll_enabled: bool,
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
    /// Set when the tree's structure changed but the index has not been
    /// rebuilt from it yet. Consumed by `Self::sync_search_index`, which
    /// [`Self::tick_search`] drives once per event-loop iteration — so a
    /// whole batch of warmed nodes costs one rebuild instead of one each.
    search_index_stale: bool,
    /// When the index was last rebuilt, for `Self::sync_search_index`'s
    /// throttle. `None` means "never throttle the next one" (nothing has
    /// been rebuilt since construction/reload, where the index is already
    /// current).
    search_index_rebuilt_at: Option<Instant>,
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
    /// `Some` for the duration of one tool's session inside `mandible
    /// --review`, `None` in the ordinary `mandible <tool>` path. Carries the
    /// tool's stratum, pre-tag suggestions, and sample progress for the
    /// review overlay to display, plus any in-progress verdict draft — see
    /// [`crate::app_review`]. `App` itself never sets this; the caller
    /// (`mandible/src/app_runner.rs`'s `run_review`) attaches it right
    /// after building the app for each sampled tool.
    pub review: Option<crate::app_review::ReviewOverlay>,
}

/// How long a status message stays in the footer before the keybinding
/// hints come back. Long enough to read a copied flag spelling, short
/// enough that the hints are never gone when someone looks up needing
/// them.
const STATUS_MESSAGE_TTL: Duration = Duration::from_secs(4);

/// Shortest gap between two search-index rebuilds while nobody is
/// searching (see `App::sync_search_index` for the "nobody is searching"
/// caveat, which is the whole reason this is safe).
///
/// The batching alone already turns one rebuild per warmed node into one
/// per event-loop iteration, but the loop wakes every 100ms and a rebuild
/// is O(whole tree), so a long cascade would still rebuild a growing tree
/// ten times a second for its whole duration. 250ms bounds that to four,
/// which is far below the threshold at which a user notices a tree they
/// are not currently searching having settled — and every rebuild skipped
/// here is skipped only because a later one will supersede it: the stale
/// flag stays set, so no change is ever dropped.
const REBUILD_MIN_INTERVAL: Duration = Duration::from_millis(250);

/// Columns moved per `h`/`l`/`←`/`→` press in the detail pane. Larger than
/// the vertical scroll's one-line-per-press step deliberately: preformatted
/// content this feature targets (long USAGE synopses, wide `--help` tables)
/// tends to overflow by tens of columns at once, and a one-column step would
/// take many presses to reveal anything.
const DETAIL_HSCROLL_STEP: usize = 8;

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
            pending_detail_fraction: std::cell::Cell::new(None),
            detail_hscroll: 0,
            detail_max_hscroll: std::cell::Cell::new(0),
            pending_detail_hfraction: std::cell::Cell::new(None),
            horizontal_scroll_enabled: true,
            show_help: false,
            show_hidden: false,
            status_message: None,
            status_expires_at: None,
            search_index,
            search_index_stale: false,
            search_index_rebuilt_at: None,
            color_enabled: crate::style::color_enabled_from_env(),
            glyphs: crate::glyphs::from_env(),
            selected_flag: None,
            review: None,
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
        // `compute_matching_paths` below reads the index, and key handlers
        // call this method directly rather than going through the event
        // loop, so give the same throttle a chance to run here instead of
        // relying on `tick_search` having been the most recent caller. With
        // a query active the throttle is a no-op and the rows below are
        // computed against a fully current index.
        self.sync_search_index(Instant::now());
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
        self.sync_search_index(Instant::now());
        if self.search_index.tick(timeout_ms) {
            self.mark_dirty();
        }
    }

    /// Rebuild the search index if the tree changed since the last rebuild,
    /// subject to the throttle described on [`REBUILD_MIN_INTERVAL`].
    ///
    /// `now` is a parameter rather than read here so a test can drive the
    /// throttle deterministically instead of sleeping.
    fn sync_search_index(&mut self, now: Instant) {
        if !self.search_index_stale {
            return;
        }
        // Staleness is user-visible the moment there is a query: spec §5.2
        // has the background warm existing precisely so whole-tree search
        // is honest, and a result set that lags the tree by a quarter of a
        // second while someone types is exactly the dishonesty it was
        // added to remove. So the throttle applies only while nobody is
        // searching — which is the case for the whole warm cascade in the
        // common path, and is where all the wasted work was.
        let searching = self.active_filter().is_some() || self.focus == Focus::Search;
        if !searching
            && self
                .search_index_rebuilt_at
                .is_some_and(|last| now.duration_since(last) < REBUILD_MIN_INTERVAL)
        {
            return;
        }
        self.rebuild_search_index(now);
    }

    /// Rebuild the search index now if the tree changed since the last
    /// rebuild, ignoring the throttle. For callers that are not the event
    /// loop and so cannot rely on a later [`Self::tick_search`] to pick the
    /// change up.
    pub fn flush_search_index(&mut self) {
        if self.search_index_stale {
            self.rebuild_search_index(Instant::now());
        }
    }

    fn rebuild_search_index(&mut self, now: Instant) {
        self.search_index.populate(&self.root);
        self.search_index_stale = false;
        self.search_index_rebuilt_at = Some(now);
        self.mark_dirty();
    }

    /// How many times the search index has been rebuilt from the tree.
    /// A test seam for the "one rebuild per batch of warmed nodes, not one
    /// per node" property — see [`mandible_search::SearchIndex::populate_count`].
    pub fn search_populate_count(&self) -> u64 {
        self.search_index.populate_count()
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
        self.detail_hscroll = 0;
        self.pending_detail_hfraction.set(None);
        self.follow_selection();
    }

    /// Move the tree selection up one row.
    pub fn move_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
        self.selected_flag = None;
        self.detail_hscroll = 0;
        self.pending_detail_hfraction.set(None);
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
        // The tree's structure (and searchable content) just changed, so
        // the search index needs rebuilding from it — but *not* here.
        // `SearchIndex::populate` is O(whole tree) and this method is
        // called once per node the background warmer fills (~255 for
        // docker, up to spec §5.2's 4,096-node cap), so rebuilding inline
        // is O(n²) and pegged every core for the entire warm. Record the
        // need instead; `flush_search_index`, driven from the event loop,
        // collapses a whole arrival batch into one rebuild.
        self.search_index_stale = true;
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
        self.detail_hscroll = 0;
        self.pending_detail_hfraction.set(None);
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
        self.detail_hscroll = 0;
        self.pending_detail_hfraction.set(None);
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
        // Freshly built from this very tree: nothing pending, and the next
        // splice's rebuild must not be throttled against a rebuild that
        // belongs to the previous tree.
        self.search_index_stale = false;
        self.search_index_rebuilt_at = None;
        let filter = self.active_filter().unwrap_or("").to_string();
        self.search_index.set_query(&filter);

        self.mark_dirty();
        self.ensure_rows_fresh();

        self.selected = previously_selected
            .and_then(|path| self.rows.iter().position(|row| row.path == path))
            .unwrap_or_else(|| self.selected.min(self.rows.len().saturating_sub(1)));
        self.reset_detail_scroll();
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
        // A line offset doesn't carry between two completely different
        // renderings of the same node: line 40 of a flag table is not line
        // 40 of the raw text. The reader's *proportional* place does —
        // flipping `t` to compare the parse against the source shouldn't
        // throw them back to the top — so the fraction is carried and the
        // offset resolved once the new view's extent is known.
        // A fraction still pending from the *previous* toggle carries
        // through unchanged — a proportion is view-independent. Computing
        // it fresh from `detail_scroll` here would read the zero the last
        // toggle left behind, which is precisely the rapid `t`-`t`-`t`
        // comparison this feature exists for.
        let fraction = self.pending_detail_fraction.take().or_else(|| {
            let max = self.detail_max_scroll.get();
            (max > 0).then(|| self.detail_scroll.min(max) as f64 / max as f64)
        });
        self.pending_detail_fraction.set(fraction);
        self.detail_scroll = 0;
        // The horizontal offset carries as a proportion too. An absolute
        // column is meaningless across the two renderings, but when the
        // reader has scrolled into the wide part of a synopsis, the raw
        // text's wide part is the region they are comparing against —
        // resetting to column zero on every `t` made that comparison
        // impossible (maintainer-reported, same failure as the vertical
        // reset this method already fixes).
        let hfraction = self.pending_detail_hfraction.take().or_else(|| {
            let max = self.detail_max_hscroll.get();
            (max > 0).then(|| self.detail_hscroll.min(max) as f64 / max as f64)
        });
        self.pending_detail_hfraction.set(hfraction);
        self.detail_hscroll = 0;
        self.raw_fetch_needed()
    }

    /// The horizontal twin of
    /// [`Self::materialize_pending_detail_scroll`].
    fn materialize_pending_detail_hscroll(&mut self) {
        if let Some(fraction) = self.pending_detail_hfraction.take() {
            let max = self.detail_max_hscroll.get();
            self.detail_hscroll = (fraction * max as f64).round() as usize;
        }
    }

    /// Resolve a fraction carried across a raw/rendered toggle into a line
    /// offset for the current view, and clear it. Key handlers call this
    /// before moving so their movement starts from where the reader sees
    /// the pane, not from the stale offset underneath it.
    fn materialize_pending_detail_scroll(&mut self) {
        if let Some(fraction) = self.pending_detail_fraction.take() {
            let max = self.detail_max_scroll.get();
            self.detail_scroll = (fraction * max as f64).round() as usize;
        }
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
        self.materialize_pending_detail_scroll();
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
        let max = self.detail_max_scroll.get();
        // A fraction still pending from a raw/rendered toggle takes
        // precedence: the extent it needs arrives from the renderer one
        // frame after the toggle, so this is where it first takes effect
        // on screen. It stays pending (peek, not take) until a key handler
        // materializes it — this method takes `&self`.
        match self.pending_detail_fraction.get() {
            Some(fraction) => (fraction * max as f64).round() as usize,
            None => self.detail_scroll.min(max),
        }
    }

    /// Detail pane scroll up.
    pub fn detail_scroll_up(&mut self) {
        self.selected_flag = None;
        self.materialize_pending_detail_scroll();
        self.detail_scroll = self.detail_scroll.saturating_sub(1);
    }

    /// Reset detail scroll — called on selection change so the pane
    /// doesn't stay scrolled into a different node's content.
    pub fn reset_detail_scroll(&mut self) {
        self.detail_scroll = 0;
        // A new node's content has no relation to the old node's place;
        // a fraction carried across a view toggle must not survive into it.
        self.pending_detail_fraction.set(None);
        self.detail_hscroll = 0;
        self.pending_detail_hfraction.set(None);
    }

    /// `h`/`←`: scroll the detail pane left, when it has focus (spec §9:
    /// preformatted detail-pane content scrolls horizontally rather than
    /// wrapping). A no-op when the config toggle is off, or already at the
    /// left edge — `saturating_sub` alone would still be correct here, but
    /// the early return also means a disabled toggle never touches
    /// [`Self::detail_hscroll`] at all, which is one less thing for the
    /// "config off reproduces today's output exactly" property to depend
    /// on holding elsewhere.
    pub fn detail_hscroll_left(&mut self) {
        if !self.horizontal_scroll_enabled {
            return;
        }
        self.materialize_pending_detail_hscroll();
        self.detail_hscroll = self.detail_hscroll.saturating_sub(DETAIL_HSCROLL_STEP);
    }

    /// `l`/`→`: scroll the detail pane right, clamped to
    /// [`Self::detail_max_hscroll`] — the widest preformatted line on
    /// screen minus the viewport width, written by the renderer each frame
    /// exactly like [`Self::detail_max_scroll`] is for vertical scroll.
    pub fn detail_hscroll_right(&mut self) {
        if !self.horizontal_scroll_enabled {
            return;
        }
        self.materialize_pending_detail_hscroll();
        let max = self.detail_max_hscroll.get();
        self.detail_hscroll = self
            .detail_hscroll
            .saturating_add(DETAIL_HSCROLL_STEP)
            .min(max);
    }

    /// Record how far the detail pane's preformatted content can usefully
    /// scroll horizontally, from the renderer. Called every frame the
    /// detail pane draws preformatted content, with `0` when there is
    /// none on screen — that both clamps `h`/`l` to a no-op and prevents a
    /// stale nonzero extent from a previous node leaking into this frame's
    /// overflow affordance (see [`Self::detail_hscroll_can_go_right`]).
    pub fn set_detail_hextent(&self, max_line_width: usize, viewport_width: usize) {
        self.detail_max_hscroll
            .set(max_line_width.saturating_sub(viewport_width));
    }

    /// The current horizontal offset, clamped to what the last frame could
    /// actually show — same shape as [`Self::clamped_detail_scroll`].
    pub fn clamped_detail_hscroll(&self) -> usize {
        let max = self.detail_max_hscroll.get();
        // Peek, don't take: rendering takes `&self`; a key handler
        // materializes it. Mirrors [`Self::clamped_detail_scroll`].
        match self.pending_detail_hfraction.get() {
            Some(fraction) => (fraction * max as f64).round() as usize,
            None => self.detail_hscroll.min(max),
        }
    }

    /// Whether there is preformatted content hidden off the left edge —
    /// i.e. whether the overflow affordance belongs on that side.
    pub fn detail_hscroll_can_go_left(&self) -> bool {
        self.clamped_detail_hscroll() > 0
    }

    /// Whether there is preformatted content hidden off the right edge.
    pub fn detail_hscroll_can_go_right(&self) -> bool {
        self.clamped_detail_hscroll() < self.detail_max_hscroll.get()
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

    /// The reader's proportional place survives a raw/rendered toggle:
    /// halfway down a 200-line rendering must land halfway down a 50-line
    /// raw text, keep showing there before any key is pressed, and a later
    /// keypress must move from that place — while a selection change drops
    /// the carried place entirely.
    #[test]
    fn raw_toggle_keeps_proportional_scroll() {
        let mut app = App::new("git".to_string(), sample_tree());
        app.set_detail_extent(220, 20); // max 200
        app.detail_scroll = 100;
        app.toggle_raw_mode();
        // Renderer reports the raw view's extent on the next frame.
        app.set_detail_extent(70, 20); // max 50
        assert_eq!(app.clamped_detail_scroll(), 25);
        app.detail_scroll_down();
        assert_eq!(app.clamped_detail_scroll(), 26);

        // Toggling back carries the place in the other direction too.
        app.toggle_raw_mode();
        app.set_detail_extent(220, 20);
        assert_eq!(app.clamped_detail_scroll(), 104); // 26/50 of 200

        // A new selection has unrelated content: nothing carries.
        app.toggle_raw_mode();
        app.reset_detail_scroll();
        app.set_detail_extent(70, 20);
        assert_eq!(app.clamped_detail_scroll(), 0);
    }

    /// The horizontal offset survives the raw/rendered toggle the same
    /// way the vertical one does: proportionally, through repeated
    /// flips, materialized by the first h/l press, and dropped on a
    /// selection change. (Maintainer-reported gap after the vertical fix
    /// shipped alone.)
    #[test]
    fn raw_toggle_keeps_proportional_horizontal_scroll() {
        let mut app = App::new("git".to_string(), sample_tree());
        assert!(app.horizontal_scroll_enabled, "default is on");
        app.set_detail_hextent(140, 40); // max 100
        app.detail_hscroll = 50;
        app.toggle_raw_mode();
        app.set_detail_hextent(90, 40); // raw view: max 50
        assert_eq!(app.clamped_detail_hscroll(), 25);

        // Second toggle with no key press in between: still half-way.
        app.toggle_raw_mode();
        app.set_detail_hextent(140, 40);
        assert_eq!(app.clamped_detail_hscroll(), 50);

        // A key press materializes and moves from there.
        app.detail_hscroll_right();
        assert_eq!(app.clamped_detail_hscroll(), 50 + DETAIL_HSCROLL_STEP);

        // Selection change drops the carried place entirely.
        app.toggle_raw_mode();
        app.reset_detail_scroll();
        app.set_detail_hextent(90, 40);
        assert_eq!(app.clamped_detail_hscroll(), 0);
    }

    /// The rapid `t`-`t`-`t` comparison, with no scroll key pressed in
    /// between: the place must survive every flip, not just the first.
    /// (The first version computed the second toggle's fraction from the
    /// zeroed offset the first toggle left behind — reset to top on the
    /// second press, found by the maintainer in real use.)
    #[test]
    fn repeated_raw_toggle_without_scrolling_keeps_place() {
        let mut app = App::new("git".to_string(), sample_tree());
        app.set_detail_extent(220, 20); // rendered: max 200
        app.detail_scroll = 150;
        app.toggle_raw_mode();
        app.set_detail_extent(70, 20); // raw: max 50
        assert_eq!(app.clamped_detail_scroll(), 38); // 0.75 of 50, rounded
        app.toggle_raw_mode();
        app.set_detail_extent(220, 20);
        assert_eq!(app.clamped_detail_scroll(), 150);
        app.toggle_raw_mode();
        app.set_detail_extent(70, 20);
        assert_eq!(app.clamped_detail_scroll(), 38);
    }

    /// Drive the (real, async, `nucleo`-backed) search index until its
    /// results stop changing for a few consecutive polls, bounded overall
    /// so a bug can't hang the test suite. Mirrors how the real event loop
    /// calls `tick_search` from its own poll timeout (spec §10
    /// "Threading") rather than assuming a single call finishes matching.
    fn settle_search(app: &mut App) {
        // The event loop's own `tick_search` does this first; a test that
        // spliced nodes in would otherwise settle a match against the index
        // as it was before the splice.
        app.flush_search_index();
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
            mandible_core::Entity::flag_long("autosquash", Provenance::single(Source::HelpText));
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
            mandible_core::Entity::flag_long("autosquash", Provenance::single(Source::HelpText));
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

    /// Splice `count` distinct children under the root, the way the
    /// background warmer's drain loop does: one call per filled node.
    fn splice_warmed_children(app: &mut App, count: usize) {
        for i in 0..count {
            let name = format!("warmed{i}");
            let mut child = CommandNode::new(&name, Provenance::single(Source::HelpText));
            child.children_filled = true;
            child.summary = Some(mandible_core::Text::sanitize("a warmed subcommand"));
            app.root.subcommands.push(child.clone());
            app.splice_filled_node(&["git".to_string(), name], child);
        }
    }

    /// The rebuild storm that pegged every core during `mandible docker`'s
    /// warm: `splice_filled_node` rebuilt the whole index per arrival, so
    /// docker's ~255 warmed nodes cost ~255 full restarts and
    /// re-injections of a growing item set. The property that fixes it is
    /// countable and nothing else observable distinguishes the two, hence
    /// the `populate_count` seam.
    #[test]
    fn splicing_many_nodes_rebuilds_the_index_once() {
        let mut app = App::new("git".to_string(), sample_tree());
        let after_construction = app.search_populate_count();

        splice_warmed_children(&mut app, 32);
        assert_eq!(
            app.search_populate_count(),
            after_construction,
            "splicing must not rebuild the index at all; the event loop does that"
        );

        app.tick_search(0);
        assert_eq!(
            app.search_populate_count(),
            after_construction + 1,
            "32 arrivals drained in one event-loop iteration must cost exactly one rebuild"
        );
    }

    /// The batch rebuild is not allowed to make search lag the tree: spec
    /// §5.2 has warming exist so whole-tree search is honest, so a node
    /// that just arrived must be findable without waiting on the throttle.
    #[test]
    fn an_active_query_rebuilds_immediately_and_finds_the_new_node() {
        let mut app = App::new("git".to_string(), sample_tree());
        app.focus_search();
        for c in "warmed7".chars() {
            app.search_input_char(c);
        }
        settle_search(&mut app);
        let before = app.search_populate_count();

        splice_warmed_children(&mut app, 8);
        // No `now` advance at all: the throttle must not apply here.
        app.sync_search_index(Instant::now());
        assert_eq!(
            app.search_populate_count(),
            before + 1,
            "a search is active, so the arrival batch must be indexed at once"
        );

        settle_search(&mut app);
        app.mark_dirty();
        app.ensure_rows_fresh();
        assert!(
            app.rows().iter().any(|r| r.name == "warmed7"),
            "a freshly warmed node must be findable by an already-active query: {:?}",
            app.rows().iter().map(|r| &r.name).collect::<Vec<_>>()
        );
    }

    /// With nobody searching, rebuilds are capped at one per
    /// [`REBUILD_MIN_INTERVAL`] — but a skipped rebuild is deferred, never
    /// dropped: the stale flag survives so the next eligible tick picks the
    /// change up.
    #[test]
    fn the_rebuild_throttle_defers_a_change_without_dropping_it() {
        let mut app = App::new("git".to_string(), sample_tree());
        let t0 = Instant::now();

        splice_warmed_children(&mut app, 4);
        app.sync_search_index(t0);
        let after_first = app.search_populate_count();

        // A second cascade arriving inside the interval is held.
        splice_warmed_children(&mut app, 4);
        app.sync_search_index(t0 + REBUILD_MIN_INTERVAL / 2);
        assert_eq!(
            app.search_populate_count(),
            after_first,
            "a rebuild inside the throttle interval must be skipped"
        );
        assert!(app.search_index_stale, "the pending change must be kept");

        // …and picked up as soon as the interval has passed, with no
        // further splice to prompt it.
        app.sync_search_index(t0 + REBUILD_MIN_INTERVAL);
        assert_eq!(
            app.search_populate_count(),
            after_first + 1,
            "the deferred change must be indexed once the interval elapses"
        );
        assert!(!app.search_index_stale);
    }

    /// `r` throws the tree away and rebuilds the index from the new one, so
    /// it must also drop any staleness owed to the *old* tree — and must
    /// not leave the next arrival throttled against a rebuild that belongs
    /// to a tree that no longer exists.
    #[test]
    fn reload_leaves_the_index_current_and_unthrottled() {
        let mut app = App::new("git".to_string(), sample_tree());
        splice_warmed_children(&mut app, 4);
        assert!(app.search_index_stale);

        app.reload(sample_tree());
        assert!(
            !app.search_index_stale,
            "reload repopulates from the new tree, so nothing is owed"
        );
        let after_reload = app.search_populate_count();

        let mut child = CommandNode::new("add", Provenance::single(Source::HelpText));
        child.children_filled = true;
        app.splice_filled_node(&["git".to_string(), "add".to_string()], child);
        app.sync_search_index(Instant::now());
        assert_eq!(
            app.search_populate_count(),
            after_reload + 1,
            "the first arrival after a reload must not be throttled"
        );
    }

    // --- detail pane horizontal scroll ---

    #[test]
    fn detail_hscroll_clamps_at_both_ends() {
        let mut app = App::new("git".to_string(), sample_tree());
        app.horizontal_scroll_enabled = true;
        app.set_detail_hextent(50, 20); // max 30

        for _ in 0..10 {
            app.detail_hscroll_right();
        }
        assert_eq!(
            app.clamped_detail_hscroll(),
            30,
            "must never exceed max_line_width - viewport_width"
        );

        for _ in 0..10 {
            app.detail_hscroll_left();
        }
        assert_eq!(app.clamped_detail_hscroll(), 0, "must never go negative");
    }

    #[test]
    fn detail_hscroll_is_a_noop_with_the_config_toggle_off() {
        let mut app = App::new("git".to_string(), sample_tree());
        app.horizontal_scroll_enabled = false;
        app.set_detail_hextent(50, 20);
        app.detail_hscroll_right();
        app.detail_hscroll_right();
        assert_eq!(
            app.clamped_detail_hscroll(),
            0,
            "off must not scroll at all"
        );
    }

    #[test]
    fn detail_hscroll_resets_on_selection_change() {
        let mut app = App::new("git".to_string(), sample_tree());
        app.set_detail_hextent(50, 20);
        app.detail_hscroll_right();
        assert!(app.clamped_detail_hscroll() > 0, "precondition");

        app.move_down();
        assert_eq!(
            app.clamped_detail_hscroll(),
            0,
            "a new selection has unrelated content to scroll through"
        );
    }

    #[test]
    fn detail_hscroll_resets_on_collapse_or_jump_to_parent() {
        let mut app = App::new("git".to_string(), sample_tree());
        app.selected = 2; // rebase
        app.expand_selected();
        app.ensure_rows_fresh();
        app.selected = 3; // --onto-helper
        app.set_detail_hextent(50, 20);
        app.detail_hscroll_right();
        assert!(app.clamped_detail_hscroll() > 0, "precondition");

        app.collapse_or_jump_to_parent();
        assert_eq!(app.clamped_detail_hscroll(), 0);
    }

    /// Originally pinned a reset-to-zero on toggle; the maintainer
    /// overruled that (the wide region being compared corresponds across
    /// the two views), so the toggle now carries the offset
    /// proportionally, like the vertical scroll.
    #[test]
    fn detail_hscroll_carries_proportionally_across_raw_mode_toggle() {
        let mut app = App::new("git".to_string(), sample_tree());
        app.set_detail_hextent(50, 20); // max 30
        app.detail_hscroll_right();
        let before = app.clamped_detail_hscroll();
        assert!(before > 0, "precondition");

        app.toggle_raw_mode();
        app.set_detail_hextent(50, 20); // same extent: same place
        assert_eq!(app.clamped_detail_hscroll(), before);
    }

    #[test]
    fn detail_hscroll_overflow_flags_track_the_current_offset() {
        let mut app = App::new("git".to_string(), sample_tree());
        app.set_detail_hextent(50, 20); // max 30
        assert!(!app.detail_hscroll_can_go_left());
        assert!(app.detail_hscroll_can_go_right());

        app.detail_hscroll_right(); // step 8: offset 8
        assert!(app.detail_hscroll_can_go_left());
        assert!(app.detail_hscroll_can_go_right());

        for _ in 0..10 {
            app.detail_hscroll_right();
        }
        assert_eq!(app.clamped_detail_hscroll(), 30);
        assert!(app.detail_hscroll_can_go_left());
        assert!(
            !app.detail_hscroll_can_go_right(),
            "fully scrolled right: nothing more to reveal"
        );
    }
}
