//! Application state: the tree, expand/collapse state, selection, search,
//! focus, and scroll offsets. Pure state — no rendering and no terminal
//! I/O, so it's fully testable without a tty.

use crate::probes::{FillEnd, Outcome};
use crate::search_ghost::Ghost;
use crate::search_rank::{hit_path, page_order_within_groups, rank_by_name};
use crate::tree::{flatten, TreeRow};
use mandible_core::{resolve, CommandNode, NodeRef, Text};
use mandible_extract::KeptProbes;
use mandible_search::SearchIndex;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// What the search box matches against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchMode {
    /// Command names, matched as a literal substring, and every entity's
    /// spellings (flag, positional, modifier, env var alike), matched as a
    /// case-insensitive prefix of a `-`/`_`-separated word. The default,
    /// and the mode whose results need no explanation: every row shown has
    /// the query visibly related to its own name, and the rows around it
    /// are gone.
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

/// What the active filter's match came to.
enum SearchHits {
    /// No filter is active.
    Off,
    /// Nothing matched.
    NoMatch,
    /// The ranked result list; the first entry is the jump target.
    Found(Vec<NodeRef>),
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
    /// Reload what the selection is on (`r` key). There is no cache to
    /// bypass (spec §11): on a command it is read again, then its
    /// subcommands; on the tool's root page the whole tree is discarded and
    /// the pipeline re-run, useful when the tool itself changed mid-session
    /// (a plugin installed, a new alias defined).
    Refresh,
    /// Quit the application.
    Quit,
    /// Lazily fill this node's children (spec §5.2 step 3) — it was just
    /// expanded but isn't known-complete yet. The caller should call
    /// [`App::mark_pending`], run the extraction, then
    /// [`App::splice_filled_node`] with the result.
    Fill(Vec<String>),
    /// Show this node's raw `--help` output in the verbatim view (`t`).
    /// The caller serves it from the output kept with the node and hands
    /// it back through [`App::set_raw_help`], or marks it pending and
    /// fetches it in the background when nothing is kept.
    FetchRaw(Vec<String>),
    /// Print this composed command line on stdout and exit (spec §2's
    /// `--print-selection`). Only ever produced while
    /// [`App::print_selection`] is set, which no ordinary `mandible <tool>`
    /// session does.
    PrintSelection(String),
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
    pub root: crate::tree_store::TreeStore,
    /// Paths the user has explicitly expanded.
    expanded: HashSet<Vec<String>>,
    /// Where each command's probe stands and what came of it (spec §5.2
    /// step 3). Drives the spinner row (spec §9) and the page notices.
    /// Written by the caller (the binary's event loop), which owns the
    /// extraction I/O; `App` itself never spawns anything.
    pub probes: crate::probes::Probes,
    /// The flattened, currently-visible row list. Rebuilt only when
    /// `dirty` (spec §9: "The flattened row list is cached").
    rows: Vec<TreeRow>,
    dirty: bool,
    /// Index into `rows` of the currently selected tree row.
    pub selected: usize,
    /// The search keyword: what is typed while the box has focus, and the
    /// filter in effect once `Enter` has moved focus to the list.
    pub search_query: String,
    /// The row the cursor was on when `/` opened the box, so `Esc` while
    /// typing can put it back.
    pre_search_path: Option<Vec<String>>,
    /// True while the active filter has a finished match with no result:
    /// the tree pane shows a notice and `rows` is left as it was.
    no_match: bool,
    /// The dim suggestion for the input the box holds.
    ghost: Option<Ghost>,
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
    /// Visible text lines of the detail pane, written by the renderer; the
    /// size of a `PgUp`/`PgDn` step.
    detail_viewport: std::cell::Cell<usize>,
    /// The offset the last frame drew at when a search-selected flag
    /// positioned the pane, else `None`. Written by the renderer; `↑`/`↓`
    /// continue from it so a manual scroll starts at what is on screen.
    flag_scroll_shown: std::cell::Cell<Option<usize>>,
    /// Rows inside the detail pane's border at the last draw; zero until one.
    /// Decides whether a long description previews.
    pane_height: std::cell::Cell<usize>,
    /// The detail pane's built page, reused until one of the inputs named
    /// in `PageKey` changes. In memory, this run only.
    pub(crate) page_cache: std::cell::RefCell<Option<crate::render::PageCache>>,
    /// What the finished search matched, for marking both panes.
    hit_marks: HitMarks,
    hit_marks_version: u64,
    /// The selected command's matching entity rows as the last frame laid
    /// them out, with the command path they belong to.
    page_matches: std::cell::RefCell<(Vec<String>, std::rc::Rc<Vec<MatchRow>>)>,
    /// The item at the top of the detail pane in the last frame, so a wrap
    /// toggle or a resize can put the same item back at the top.
    pub(crate) reflow_anchor: std::cell::RefCell<Option<crate::render::ReflowAnchor>>,
    /// A scroll the renderer placed after a re-flow, as `(detail_scroll it
    /// was placed over, the placed offset)`; it stands only while
    /// `detail_scroll` still holds that value, and is adopted by the next
    /// scroll the reader makes.
    reflow_scroll: std::cell::Cell<Option<(usize, usize)>>,
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
    /// Where each view left the detail pane, indexed by the [`Self::raw_mode`]
    /// it belongs to (see [`view_slot`]). [`Self::detail_scroll`] and
    /// [`Self::detail_hscroll`] hold the *showing* view's position; this
    /// holds the other one's, so `t` is a swap rather than a computation.
    ///
    /// Both offsets are kept, and kept literally. Nothing is mapped, scaled
    /// or seeded between the views: one node's parse and its `--help` text
    /// place the same flag at unrelated coordinates, so any derived position
    /// is a position neither view was ever showing, and a reader hopping
    /// `t` to compare a spelling has to find their place again by hand
    /// every time. A view nobody has scrolled yet for this node holds
    /// `(0, 0)` and opens at the top-left.
    saved_detail_offsets: [DetailOffsets; 2],
    /// Whether the raw `--help` view and the `unparsed` fallback scroll
    /// sideways (do not wrap). It is the starting value of that view's wrap
    /// state and what `w` flips while that view is showing; the parsed view
    /// has its own ([`Self::parsed_wrap`]). `[ui] horizontal_scroll` sets
    /// both, and both start folded to the pane.
    ///
    /// [`App::new`] does no filesystem I/O, so every embedder and test gets
    /// the same defaults whatever `config.toml` exists on the machine. The
    /// composition root (`mandible/src/app_runner.rs`'s `new_app`) reads the
    /// file and applies it with [`App::apply_config`].
    ///
    /// A plain `pub` field, like [`Self::color_enabled`] and
    /// [`Self::glyphs`], so tests can set it directly.
    pub horizontal_scroll_enabled: bool,
    /// Whether the parsed view wraps (default) or scrolls sideways; `w`
    /// flips it while the parsed view is showing.
    pub parsed_wrap: bool,
    /// Whether the `?` pop-up is showing.
    pub show_help: bool,
    /// The settings tab's state and the values it changes.
    pub settings: crate::settings::Settings,
    /// A line that replaces the footer hints until the next key press: the
    /// problems found in `config.toml` at startup, or a failed save.
    pub notice: Option<String>,
    /// Lines the `?` overlay is scrolled by; only nonzero when its content
    /// is taller than the screen.
    help_scroll: usize,
    /// `(max_scroll, visible_lines)` of the overlay, written by the renderer.
    help_extent: std::cell::Cell<(usize, usize)>,
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
    /// The search index over this tree's commands and flags (spec §10).
    /// Populated from `root` at construction and whenever the tree's
    /// structure changes; queried (not text-matched directly) by
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
    /// What the terminal can be asked to draw, including whether the
    /// xterm-256 palette is available for the detail pane's two rule
    /// shades (spec §9.3). Read once at construction alongside
    /// `color_enabled`, which is this value's `color` field — the two are
    /// set from one [`crate::style::Palette::from_env`] call so they
    /// cannot disagree, and both stay plain `pub` fields so tests can
    /// choose a depth without mutating process-wide environment state.
    pub palette: crate::style::Palette,
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
    /// The row `selected_flag` stands for when the page has several of one
    /// spelling; `None` leaves it to the first.
    pub(crate) selected_flag_row: Option<crate::sort::RowId>,
    /// The query and top search result `selected_flag` was last derived
    /// from. A rows refresh that finds the same pair leaves the target and
    /// the user's scroll alone.
    flag_target_source: Option<(String, NodeRef)>,
    /// Whether `Enter` composes the selection onto the calling shell's
    /// prompt instead of expanding the selected row (spec §2's
    /// `--print-selection`).
    ///
    /// `false` in every ordinary `mandible <tool>` session, and nothing in
    /// this crate ever sets it: the composition root
    /// (`mandible/src/app_runner.rs`'s `new_app`) turns it on for the one
    /// invocation that asked, the same way it applies `config.toml`. With
    /// it off, every key means exactly what it meant before this mode
    /// existed — `→`/`Enter`/`l` all expand — which is the property
    /// `mandible-tui`'s own tests pin.
    pub print_selection: bool,
    /// `Some` for the duration of one tool's session inside `mandible
    /// --review`, `None` in the ordinary `mandible <tool>` path. Carries the
    /// tool's stratum, pre-tag suggestions, and sample progress for the
    /// review overlay to display, plus any in-progress verdict draft — see
    /// [`crate::app_review`]. `App` itself never sets this; the caller
    /// (`mandible/src/app_runner.rs`'s `run_review`) attaches it right
    /// after building the app for each sampled tool.
    pub review: Option<crate::app_review::ReviewOverlay>,
    /// A subcommand path the command line asked to open — `mandible cargo
    /// clippy` sets `["cargo", "clippy"]` (spec §5.4). Cleared by
    /// [`App::settle_requested_path`] as soon as it lands on the node, or
    /// as soon as the tree is complete enough to say the node does not
    /// exist. `None` for a plain `mandible <tool>`.
    ///
    /// It cannot simply be selected at startup: the tree is empty until the
    /// background root fill arrives (spec §5.2 step 1), so this is an intent
    /// held across frames rather than an action taken on one.
    pub requested_path: Option<Vec<String>>,
    /// How `s` has sorted each pane (spec §2). Session state: it stays
    /// while the selection moves to another command.
    pub sorts: crate::sort::Sorts,
    /// The detail pane's top row across a re-order.
    pub(crate) sort_hold: crate::sort::SortHold,
    /// Set while `s` rebuilds the rows, so the sort does not move the
    /// selection to a new first match.
    keep_jump: bool,
}

/// One view's scroll position in the detail pane: a line offset and a
/// display-column offset, exactly as the reader left them.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
struct DetailOffsets {
    vertical: usize,
    horizontal: usize,
}

/// Which half of [`App::saved_detail_offsets`] a view owns.
///
/// A function rather than a bare `as usize` at each call site so the two
/// views can never be indexed by two different conventions — the whole
/// per-view memory is one bit of state read four times.
fn view_slot(raw_mode: bool) -> usize {
    usize::from(raw_mode)
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

/// Rows `PgUp`/`PgDn` move the tree selection.
const TREE_PAGE_ROWS: isize = 10;

impl App {
    /// Build a new app over an already-extracted (and merged) tree, with
    /// the root expanded so the first screen isn't empty.
    pub fn new(tool: String, root: CommandNode) -> App {
        let mut expanded = HashSet::new();
        expanded.insert(vec![root.name.clone()]);
        let mut search_index = SearchIndex::new();
        search_index.populate(&root);
        // One read of the environment for both, so `color_enabled` and
        // `palette.color` cannot disagree about whether color is on.
        let palette = crate::style::Palette::from_env();
        let glyphs = crate::glyphs::from_env();
        let mut app = App {
            tool,
            root: crate::tree_store::TreeStore::new(root),
            expanded,
            probes: crate::probes::Probes::new(crate::probes::KEPT_BUDGET_BYTES),
            rows: Vec::new(),
            dirty: true,
            selected: 0,
            search_query: String::new(),
            pre_search_path: None,
            no_match: false,
            ghost: None,
            focus: Focus::Tree,
            raw_mode: false,
            raw_help: HashMap::new(),
            search_mode: SearchMode::Name,
            tree_scroll: 0,
            tree_viewport: 0,
            detail_scroll: 0,
            detail_max_scroll: std::cell::Cell::new(0),
            detail_viewport: std::cell::Cell::new(0),
            flag_scroll_shown: std::cell::Cell::new(None),
            pane_height: std::cell::Cell::new(0),
            page_cache: std::cell::RefCell::new(None),
            hit_marks: HitMarks::default(),
            hit_marks_version: 0,
            page_matches: std::cell::RefCell::new((Vec::new(), std::rc::Rc::default())),
            reflow_anchor: std::cell::RefCell::new(None),
            reflow_scroll: std::cell::Cell::new(None),
            detail_hscroll: 0,
            detail_max_hscroll: std::cell::Cell::new(0),
            saved_detail_offsets: [DetailOffsets::default(); 2],
            horizontal_scroll_enabled: false,
            parsed_wrap: true,
            show_help: false,
            settings: crate::settings::Settings::new(palette, glyphs),
            notice: None,
            help_scroll: 0,
            help_extent: std::cell::Cell::new((0, 0)),
            status_message: None,
            status_expires_at: None,
            search_index,
            search_index_stale: false,
            search_index_rebuilt_at: None,
            color_enabled: palette.color,
            palette,
            glyphs,
            selected_flag: None,
            selected_flag_row: None,
            flag_target_source: None,
            print_selection: false,
            review: None,
            requested_path: None,
            sorts: crate::sort::Sorts::default(),
            sort_hold: crate::sort::SortHold::default(),
            keep_jump: false,
        };
        app.ensure_rows_fresh();
        app
    }

    /// The currently active filter text, if any: the search keyword while
    /// typing and after `Enter`. Also used by the tree pane to compute
    /// which characters within a row's name matched, for underlining
    /// (spec §9.2).
    pub fn active_filter(&self) -> Option<&str> {
        (!self.search_query.is_empty()).then_some(self.search_query.as_str())
    }

    /// True when the active filter finished matching and nothing matched.
    /// The tree pane then shows a notice, the title counts 0, and the
    /// cursor and detail page stay where they were.
    pub fn no_match(&self) -> bool {
        self.no_match
    }

    /// Recompute `rows` if a structural change (expand/collapse, search)
    /// has happened since the last build. Cheap no-op
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
        let hits = match self.search_hits() {
            SearchHits::NoMatch => {
                self.set_closest();
                self.set_hit_marks(HitMarks::default());
                self.show_no_match();
                return;
            }
            SearchHits::Off => None,
            SearchHits::Found(list) => Some(list),
        };
        let marks = match (&hits, self.active_filter()) {
            (Some(list), Some(query)) => HitMarks::from_hits(list, query),
            _ => HitMarks::default(),
        };
        self.set_completion(hits.as_deref().and_then(<[NodeRef]>::first));
        self.set_hit_marks(marks);
        self.no_match = false;
        let matching_paths = hits
            .as_ref()
            .map(|list| list.iter().map(|hit| hit_path(hit).clone()).collect());
        self.rows = flatten(
            &self.root,
            &self.expanded,
            matching_paths.as_ref(),
            &self.probes,
            self.sorts.tree,
        );
        if self.selected >= self.rows.len() {
            self.selected = self.rows.len().saturating_sub(1);
        }
        self.jump_to_first_match(hits.as_deref());
        self.dirty = false;
        // Filtering/expanding can move the selection under the viewport
        // just as arrow keys can, so the same rule applies here.
        self.follow_selection();
    }

    /// Spec §10: the cursor goes to the first row of the result list the
    /// tree shows. The list's first hit stands in for "the selected
    /// result": a command hit selects its row, an entity hit selects its
    /// parent command's row and remembers which entity the detail pane
    /// scrolls to. Runs when the query or the first hit changes, and again
    /// whenever `Enter` clears `flag_target_source`.
    fn jump_to_first_match(&mut self, hits: Option<&[NodeRef]>) {
        let top = self.active_filter().and_then(|query| {
            let first = hits?.first()?.clone();
            Some((query.to_string(), first))
        });
        if top == self.flag_target_source {
            return;
        }
        if self.keep_jump {
            self.flag_target_source = top;
            return;
        }
        self.selected_flag = None;
        self.selected_flag_row = None;
        self.flag_target_source = top.clone();
        let Some((_, hit)) = top else {
            return;
        };
        if let Some(idx) = self.rows.iter().position(|r| r.path == *hit_path(&hit)) {
            self.selected = idx;
        }
        match hit {
            NodeRef::Flag { path, key } => {
                self.detail_hscroll = 0;
                self.selected_flag_row = resolve(&self.root, &path)
                    .and_then(|node| crate::render::first_row_with_key(node, &key));
                self.selected_flag = Some(key);
            }
            NodeRef::Command(_) => self.reset_detail_scroll(),
        }
    }

    /// A finished match with no result. While typing, the cursor and the
    /// detail page go back to the row from before `/` (the rows are rebuilt
    /// unfiltered behind the notice so that row exists); a later keystroke
    /// that matches jumps again.
    fn show_no_match(&mut self) {
        self.no_match = true;
        self.dirty = false;
        let Some(path) = self.pre_search_path.clone() else {
            return;
        };
        let was = self.selected_path();
        let build =
            |app: &App| flatten(&app.root, &app.expanded, None, &app.probes, app.sorts.tree);
        self.rows = build(self);
        if !self.rows.iter().any(|r| r.path == path) && resolve(&self.root, &path).is_some() {
            for depth in 1..path.len() {
                self.expanded.insert(path[..depth].to_vec());
            }
            self.rows = build(self);
        }
        if let Some(idx) = self.rows.iter().position(|r| r.path == path) {
            self.selected = idx;
        }
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
        self.flag_target_source = None;
        self.settle_selection(was);
        self.selected_flag = None;
    }

    /// The active filter's result list, ranked, or why there is none.
    ///
    /// Name mode keeps the hits whose names contain every word of the
    /// keyword and ranks them by tier ([`rank_by_name`]); wide mode keeps
    /// the index's own order.
    fn search_hits(&self) -> SearchHits {
        let Some(filter) = self.active_filter() else {
            return SearchHits::Off;
        };
        if filter.trim().is_empty() {
            return SearchHits::Off;
        }
        let list = match self.search_mode {
            SearchMode::Name => {
                rank_by_name(&self.root, self.search_index.results(), filter, self.sorts)
            }
            SearchMode::Wide => {
                let (hits, groups): (Vec<_>, Vec<_>) =
                    self.search_index.results_grouped().into_iter().unzip();
                page_order_within_groups(&self.root, hits, &groups, self.sorts.detail)
            }
        };
        if list.is_empty() {
            SearchHits::NoMatch
        } else {
            SearchHits::Found(list)
        }
    }

    /// Rebuild the search index if the tree changed (spec §10), once per
    /// event-loop iteration. Marks the row list dirty when it does, so the
    /// next [`Self::ensure_rows_fresh`] picks up fresh matches.
    pub fn tick_search(&mut self) {
        self.sync_search_index(Instant::now());
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

    /// The selected node's own parent command, if it has one — `None` for
    /// the root row itself. A same-as-ancestor node's accepted-modifier
    /// block (spec §16, docs/design.md §9.3) states each letter's
    /// description from the *parent's* own modifier table, since the
    /// child's `accepted_modifiers` field carries only the bare letters.
    pub fn selected_node_parent(&self) -> Option<&CommandNode> {
        let row = self.selected_row()?;
        if row.path.len() < 2 {
            return None;
        }
        resolve(&self.root, &row.path[..row.path.len() - 1])
    }

    /// The selected row's path from the root, e.g. `["git", "rebase"]`.
    pub fn selected_path(&self) -> Option<Vec<String>> {
        Some(self.selected_row()?.path.clone())
    }

    /// The command line this selection composes to: the selected node's
    /// full command path, plus the selected flag's typed spelling when
    /// search landed on one (spec §2's `--print-selection`).
    ///
    /// `git commit` for a command row; `git commit --amend` when
    /// [`Self::selected_flag`] holds a flag that command actually
    /// documents. A flag target that no longer resolves — the tree was
    /// re-extracted, the row moved — composes the path alone rather than
    /// guessing: a command that is merely incomplete can be finished on the
    /// prompt, and one carrying a flag its tool does not have cannot.
    ///
    /// Nothing is appended for a flag's value: see
    /// `Entity::shell_spelling`.
    pub fn selection_command(&self) -> Option<String> {
        let path = &self.selected_row()?.path;
        let mut line = path.join(" ");
        if let Some(spelling) = self.selected_flag.as_ref().and_then(|key| {
            let node = resolve(&self.root, path)?;
            let at = self
                .selected_flag_row
                .and_then(|id| crate::render::entity_at(node, id));
            let entity = at
                .filter(|e| e.matches_key(key))
                .or_else(|| mandible_core::resolve_flag(&self.root, path, key))?;
            entity.shell_spelling()
        }) {
            line.push(' ');
            line.push_str(&spelling);
        }
        Some(line)
    }

    /// Set the palette, keeping [`Self::color_enabled`] in step.
    ///
    /// The two are one decision read at two kinds of call site (see
    /// `color_enabled`'s note), so they are set together here rather than
    /// assigned separately by whoever knows the terminal — the composition
    /// root, which is the only place that knows which stream is being drawn
    /// on.
    pub fn set_palette(&mut self, palette: crate::style::Palette) {
        self.settings.palette_base = palette;
        self.apply_theme();
    }

    fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Move the tree selection down one row.
    pub fn move_down(&mut self) {
        self.move_selection_by(1);
    }

    /// Move the tree selection up one row.
    pub fn move_up(&mut self) {
        self.move_selection_by(-1);
    }

    /// `PgDn`: move the tree selection down [`TREE_PAGE_ROWS`] rows.
    pub fn tree_page_down(&mut self) {
        self.move_selection_by(TREE_PAGE_ROWS);
    }

    /// `PgUp`: move the tree selection up [`TREE_PAGE_ROWS`] rows.
    pub fn tree_page_up(&mut self) {
        self.move_selection_by(-TREE_PAGE_ROWS);
    }

    /// `Home`: select the first visible row.
    pub fn tree_home(&mut self) {
        self.move_selection_by(isize::MIN);
    }

    /// `End`: select the last visible row.
    pub fn tree_end(&mut self) {
        self.move_selection_by(isize::MAX);
    }

    /// Move the selection by `delta` rows of the (filtered) row list,
    /// clamped at both ends.
    ///
    /// `ensure_rows_fresh` runs first because [`Self::settle_selection`]
    /// compares the row this key left against the row it arrived at, and a
    /// stale row list makes both sides address the wrong node.
    fn move_selection_by(&mut self, delta: isize) {
        self.ensure_rows_fresh();
        if self.no_match {
            return;
        }
        let was = self.selected_path();
        let last = self.rows.len().saturating_sub(1);
        self.selected = self.selected.saturating_add_signed(delta).min(last);
        self.settle_selection(was);
    }

    /// Finish a navigation keypress: drop the detail pane's scroll and any
    /// flag target **only if the selection actually moved to a different
    /// node**, then bring the new selection on screen.
    ///
    /// `was` is [`Self::selected_path`] read before the move. The path,
    /// not the index, is what decides: collapsing the selected row
    /// re-flattens the tree underneath it while the pane goes on
    /// describing the same node, and that is not a move.
    ///
    /// Guarding this is the whole point. [`Self::reset_detail_scroll`]
    /// exists for "the pane now describes a different node", but every
    /// navigation path used to call it unconditionally, including on the
    /// presses that go nowhere: `j` on the last row, `k` on the first,
    /// either one on a single-node tree, a click on the row already
    /// selected. Reading a long `FLAGS` list and pressing `j` once too
    /// often threw the reader back to the top of a document they had
    /// scrolled minutes into, with nothing on screen having changed to
    /// explain it — a keypress that does nothing must do *nothing*.
    fn settle_selection(&mut self, was: Option<Vec<String>>) {
        if self.selected_path() != was {
            self.selected_flag = None;
            self.reset_detail_scroll();
        }
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
        if self.no_match {
            return None;
        }
        let row = self.rows.get(self.selected)?;
        let needs_fill = !row.children_filled && !self.probes.in_flight(&row.path);
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
        self.probes.queued(path);
        self.mark_dirty();
    }

    /// Stop showing `path` as loading because its queued probe was dropped
    /// before it started.
    pub fn clear_pending(&mut self, path: &[String]) {
        self.probes.unqueued(path);
        self.mark_dirty();
    }

    /// A worker started on `path`.
    pub fn mark_running(&mut self, path: &[String]) {
        self.probes.running(path);
    }

    #[cfg(test)]
    pub(crate) fn detail_max_scroll_for_tests(&self) -> usize {
        self.detail_max_scroll.get()
    }

    /// True once a fill for `path` has completed and none is in flight.
    pub fn fill_finished(&self, path: &[String]) -> bool {
        self.probes.finished(path)
    }

    /// How the last probe of `path` came out, also while a re-run is in
    /// flight.
    pub fn probe_outcome(&self, path: &[String]) -> Option<crate::probes::Outcome> {
        self.probes.outcome(path)
    }

    /// True if `path` has been read, or came back already holding its
    /// children: nothing more is owed for it.
    pub fn is_loaded(&self, path: &[String]) -> bool {
        self.probes.outcome(path).is_some()
            || mandible_core::resolve(&self.root, path).is_some_and(|n| n.children_filled)
    }

    /// True if `path` is being read now and has no earlier result.
    pub fn is_first_read(&self, path: &[String]) -> bool {
        self.probes.first_read(path)
    }

    /// True if `path` is currently being lazily filled.
    pub fn is_pending(&self, path: &[String]) -> bool {
        self.probes.in_flight(path)
    }

    /// Splice a freshly-filled node into the tree at `path`, replacing
    /// whatever was there, clears its pending marker, and — if
    /// the node now has children and wasn't already user-expanded —
    /// expands it, so the result the user asked for is immediately
    /// visible rather than requiring a second expand press.
    pub fn splice_filled_node(&mut self, path: &[String], node: CommandNode) {
        self.finish_fill(path, node, FillEnd::Read, KeptProbes::default());
    }

    /// [`Self::splice_filled_node`] for a fill that reported how it ended
    /// and what the tool printed. Returns the paths whose kept output was
    /// dropped to stay within the budget.
    pub fn finish_fill(
        &mut self,
        path: &[String],
        node: CommandNode,
        end: FillEnd,
        kept: KeptProbes,
    ) -> Vec<Vec<String>> {
        let outcome = match end {
            FillEnd::TimedOut => Outcome::TimedOut,
            FillEnd::Failed => Outcome::Failed,
            FillEnd::Read if crate::render::is_empty_page(&node) => Outcome::Empty,
            FillEnd::Read => Outcome::Parsed,
        };
        let evicted = self.probes.finish(path.to_vec(), outcome, kept);
        if let Some(slot) = mandible_core::resolve_mut(self.root.edit(), path) {
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
        evicted
    }

    /// Try to land on [`Self::requested_path`] — the subcommand path the
    /// command line named (`mandible cargo clippy`, spec §5.4).
    ///
    /// Called after every batch of background fills, because the tree the
    /// path addresses does not exist yet when the app is built: the node
    /// arrives some frames later, and until it does there is nothing to
    /// select and nothing to complain about either.
    ///
    /// Landing means exactly what browsing there by hand means — the
    /// ancestors are opened and the node is selected, nothing is expanded
    /// beyond that, because expansion is user intent (spec §5.2).
    ///
    /// The give-up condition is deliberately narrow: the parent that would
    /// hold the missing segment must be **known-complete and not still being
    /// filled**, so a path that simply has not arrived yet is never reported
    /// as absent. Reporting it in the status line rather than refusing at
    /// the command line is what keeps `mandible <tool> <typo>` a tool that
    /// opened with a message instead of a shell error — the tree beside it
    /// is exactly where the real name is.
    pub fn settle_requested_path(&mut self) {
        let Some(target) = self.requested_path.clone() else {
            return;
        };
        if resolve(&self.root, &target).is_some() {
            for depth in 1..target.len() {
                self.expanded.insert(target[..depth].to_vec());
            }
            self.mark_dirty();
            self.ensure_rows_fresh();
            if let Some(idx) = self.rows.iter().position(|row| row.path == target) {
                self.selected = idx;
                self.selected_flag = None;
                self.reset_detail_scroll();
                self.follow_selection();
            }
            self.requested_path = None;
            return;
        }

        // The first segment that isn't in the tree; everything before it is.
        let Some(depth) = (1..target.len()).find(|&d| resolve(&self.root, &target[..=d]).is_none())
        else {
            return;
        };
        let parent_path = target[..depth].to_vec();
        let parent_complete =
            resolve(&self.root, &parent_path).is_some_and(|node| node.children_filled);
        if parent_complete && !self.probes.in_flight(&parent_path) {
            let message = format!(
                "{} has no subcommand {:?}",
                parent_path.join(" "),
                target[depth]
            );
            self.set_status(message);
            self.requested_path = None;
        }
    }

    /// `←`/`h`: collapse the selected row if it's expanded, else jump
    /// selection to its parent.
    pub fn collapse_or_jump_to_parent(&mut self) {
        self.ensure_rows_fresh();
        if self.no_match {
            return;
        }
        let was = self.selected_path();
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
        // Collapsing the selected row is not a move — the pane still
        // describes the node it described a moment ago, so its scroll
        // position still addresses something. Jumping to the parent is.
        self.settle_selection(was);
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
            if !filled && !self.probes.in_flight(path) {
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
        let was = self.selected_path();
        if idx < self.rows.len() {
            self.selected = idx;
        }
        // A click lands on the row that is already selected often enough
        // to matter — the same rule as the arrow keys applies to it, and
        // for the same reason (see [`Self::settle_selection`]).
        self.settle_selection(was);
    }

    /// `/`: focus the search box. Pressing it again while the box is
    /// already focused toggles between matching names only and matching
    /// descriptions too — the narrow mode first, so the first thing a new
    /// user sees is the mode whose results explain themselves.
    pub fn cycle_search_mode(&mut self) {
        self.search_mode = self.search_mode.toggled();
        self.search_index
            .set_wide(self.search_mode == SearchMode::Wide);
        self.mark_dirty();
    }

    /// `/`: open the search box. Typing and the filtered list are the two
    /// search states: the box opens on the keyword already in effect, and
    /// the row the cursor is on is remembered for `Esc`.
    pub fn focus_search(&mut self) {
        self.pre_search_path = self.selected_path();
        self.focus = Focus::Search;
    }

    /// A character typed into the search box.
    pub fn search_input_char(&mut self, c: char) {
        self.search_query.push(c);
        self.search_index.set_query(&self.search_query);
        self.mark_dirty();
    }

    /// Backspace in the search box. Emptying the box leaves no filter, so
    /// the cursor goes back to the row it was on before `/`.
    pub fn search_backspace(&mut self) {
        self.search_query.pop();
        self.search_index.set_query(&self.search_query);
        self.mark_dirty();
        if self.search_query.is_empty() {
            self.drop_filter();
            self.restore_cursor(self.pre_search_path.clone());
        }
    }

    /// `Enter` in the search box: jump to the first match and hand focus to
    /// the commands list, keeping the keyword and the filter.
    pub fn accept_search(&mut self) {
        self.focus = Focus::Tree;
        self.pre_search_path = None;
        if self.active_filter().is_some() {
            self.flag_target_source = None;
            self.mark_dirty();
            self.ensure_rows_fresh();
        }
    }

    /// `Esc` in the search box: cancel the search. The keyword and the
    /// filter are cleared and the cursor returns to the row it was on
    /// before `/`.
    pub fn cancel_search(&mut self) {
        self.search_query.clear();
        self.search_index.set_query("");
        self.drop_filter();
        self.focus = Focus::Tree;
        let back = self.pre_search_path.take();
        self.restore_cursor(back);
    }

    /// `Esc` in the filtered list: clear the keyword and the filter. The
    /// cursor stays on the row it is on, and the detail pane stays where
    /// it is.
    pub fn clear_filter(&mut self) {
        if self.search_query.is_empty() {
            return;
        }
        let here = self.selected_path();
        self.search_query.clear();
        self.search_index.set_query("");
        self.leave_flag_target();
        self.drop_filter();
        self.restore_cursor(here);
    }

    /// The filter is gone: forget the jump it made and the no-match state.
    fn drop_filter(&mut self) {
        self.flag_target_source = None;
        self.no_match = false;
        self.mark_dirty();
    }

    /// Rebuild the unfiltered rows and put the cursor on `path`, opening
    /// its ancestors when the filter was the only thing showing it. The
    /// detail pane keeps its place when the node is the one it described.
    fn restore_cursor(&mut self, path: Option<Vec<String>>) {
        let was = self.selected_path();
        self.ensure_rows_fresh();
        let Some(path) = path else {
            return;
        };
        let mut idx = self.rows.iter().position(|r| r.path == path);
        if idx.is_none() && resolve(&self.root, &path).is_some() {
            for depth in 1..path.len() {
                self.expanded.insert(path[..depth].to_vec());
            }
            self.mark_dirty();
            self.ensure_rows_fresh();
            idx = self.rows.iter().position(|r| r.path == path);
        }
        if let Some(idx) = idx {
            self.selected = idx;
        }
        self.settle_selection(was);
        self.selected_flag = None;
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

        self.root.replace(root);
        // Fills queued against the old tree are abandoned by the caller,
        // so nothing is still pending for this one.
        self.probes.reset();
        // The point of a re-extract is that the tool may have changed,
        // which makes shown raw output as stale as the tree it came from.
        self.raw_help.clear();

        self.search_index = SearchIndex::new();
        self.search_index
            .set_wide(self.search_mode == SearchMode::Wide);
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
    /// Each view keeps its own place. Leaving one stores where it was —
    /// line and column both — and arriving at the other restores what it
    /// stored, so `t` puts the reader back exactly where that view last
    /// stood and movement in one view never moves the other.
    ///
    /// Nothing is derived across the boundary. Line 40 of a flag table is
    /// not line 40 of the raw text, and no scaling makes it so: the
    /// coordinates of one flag in the two renderings are unrelated, which
    /// is precisely why the comparison needs the pane put back rather than
    /// approximated. A view with nothing stored yet opens at the top-left,
    /// and a stored position too deep for content that has since shrunk is
    /// clamped when it is shown and again when it is moved
    /// ([`Self::clamped_detail_scroll`], [`Self::detail_scroll_down`]) —
    /// this method cannot clamp it itself, because the extent it would
    /// clamp against is still the extent of the view being left.
    pub fn toggle_raw_mode(&mut self) -> Option<Effect> {
        self.adopt_reflow_scroll();
        self.saved_detail_offsets[view_slot(self.raw_mode)] = DetailOffsets {
            vertical: self.detail_scroll,
            horizontal: self.detail_hscroll,
        };
        self.raw_mode = !self.raw_mode;
        let restored = self.saved_detail_offsets[view_slot(self.raw_mode)];
        self.detail_scroll = restored.vertical;
        self.detail_hscroll = restored.horizontal;
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

    /// Whether the verbatim view holds anything for this node, text or a
    /// failure.
    pub fn has_raw_help(&self, path: &[String]) -> bool {
        self.raw_help.contains_key(path)
    }

    /// Whether this node's verbatim text is still being waited for.
    pub fn raw_is_pending(&self, path: &[String]) -> bool {
        matches!(self.raw_help.get(path), Some(RawHelp::Pending))
    }

    /// Stop waiting on this node's verbatim text, so the next look at it
    /// asks again. Leaves text already shown alone.
    pub fn clear_raw_pending(&mut self, path: &[String]) {
        if self.raw_is_pending(path) {
            self.raw_help.remove(path);
        }
    }

    /// Drop this node's shown verbatim text, so the next look at it asks
    /// again. Called when the bytes it came from are no longer kept.
    pub fn forget_raw_help(&mut self, path: &[String]) {
        self.raw_help.remove(path);
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
        self.help_scroll = 0;
        self.settings.tab = crate::settings::HelpTab::Keys;
    }

    /// The overlay's scroll offset, for the renderer.
    pub fn help_scroll(&self) -> usize {
        self.help_scroll
    }

    /// Record how far the overlay can scroll and how many lines it shows,
    /// from the renderer.
    pub fn set_help_extent(&self, max_scroll: usize, visible: usize) {
        self.help_extent.set((max_scroll, visible));
    }

    /// Whether the overlay's content is taller than the screen.
    pub fn help_scrollable(&self) -> bool {
        self.help_extent.get().0 > 0
    }

    /// Scroll the overlay by `pages` screenfuls (or `lines` when `page` is
    /// false), clamped to its extent.
    pub fn help_scroll_by(&mut self, delta: isize, page: bool) {
        let (max, visible) = self.help_extent.get();
        let step = if page {
            delta * visible.max(1) as isize
        } else {
            delta
        };
        self.help_scroll = self
            .help_scroll
            .min(max)
            .saturating_add_signed(step)
            .min(max);
    }

    /// Detail pane scroll down.
    pub fn detail_scroll_down(&mut self) {
        self.detail_scroll_by(1);
    }

    /// Detail pane scroll up.
    pub fn detail_scroll_up(&mut self) {
        self.detail_scroll_by(-1);
    }

    /// `PgDn`: scroll the detail pane down one screenful.
    pub fn detail_page_down(&mut self) {
        self.detail_scroll_by(self.detail_page());
    }

    /// `PgUp`: scroll the detail pane up one screenful.
    pub fn detail_page_up(&mut self) {
        self.detail_scroll_by(-self.detail_page());
    }

    /// `Home`: scroll the detail pane to the top.
    pub fn detail_scroll_home(&mut self) {
        self.detail_scroll_by(isize::MIN);
    }

    /// `End`: scroll the detail pane to the bottom.
    pub fn detail_scroll_end(&mut self) {
        self.detail_scroll_by(isize::MAX);
    }

    /// Lines in one `PgUp`/`PgDn` step: the visible height, at least one.
    fn detail_page(&self) -> isize {
        isize::try_from(self.detail_viewport.get().max(1)).unwrap_or(isize::MAX)
    }

    /// Scroll the detail pane by `delta` lines, clamped to the last frame's
    /// extent.
    fn detail_scroll_by(&mut self, delta: isize) {
        self.adopt_reflow_scroll();
        // A manual scroll always wins over an outstanding flag-scroll
        // target from a search selection — otherwise the next render
        // would just snap straight back to it.
        self.leave_flag_target();
        let max = self.detail_max_scroll.get();
        // Start from where the pane is actually showing, not from an offset
        // deeper than the content it now holds (a restored position into
        // shrunken content, or a resize that reflowed it shorter).
        self.detail_scroll = self
            .detail_scroll
            .min(max)
            .saturating_add_signed(delta)
            .min(max);
    }

    /// Record the offset a search-selected flag placed the pane at, from the
    /// renderer; `None` when the frame was positioned by the stored scroll.
    pub fn set_flag_scroll_shown(&self, offset: Option<usize>) {
        self.flag_scroll_shown.set(offset);
    }

    /// A manual scroll ends the flag target and adopts the offset it was
    /// showing as the stored one.
    fn leave_flag_target(&mut self) {
        self.adopt_reflow_scroll();
        if self.selected_flag.take().is_some() {
            if let Some(shown) = self.flag_scroll_shown.take() {
                self.detail_scroll = shown;
            }
        }
    }

    /// Record the rows inside the detail pane's border, written by the renderer.
    pub(crate) fn set_pane_height(&self, rows: usize) {
        self.pane_height.set(rows);
    }

    /// The rows inside the detail pane's border at the last draw.
    pub(crate) fn pane_height(&self) -> usize {
        self.pane_height.get()
    }

    /// Record how far the detail pane can scroll, from the renderer.
    /// Clamps any offset already past the new limit, so a resize that makes
    /// the pane taller (or a move to a shorter node) doesn't leave the view
    /// stranded below its own content.
    pub fn set_detail_extent(&self, content_lines: usize, viewport_lines: usize) {
        self.detail_max_scroll
            .set(content_lines.saturating_sub(viewport_lines));
        self.detail_viewport.set(viewport_lines);
    }

    /// The current scroll offset, clamped to what the last frame could
    /// actually show.
    pub fn clamped_detail_scroll(&self) -> usize {
        // This clamp is where a position restored by `t` meets the target
        // view's real extent: the renderer sets that extent immediately
        // before reading this, so content that shrank since the position
        // was saved lands at its own last line rather than past it.
        self.pending_detail_scroll()
            .min(self.detail_max_scroll.get())
    }

    /// The stored offset, or the one the renderer placed over it after a
    /// re-flow.
    fn pending_detail_scroll(&self) -> usize {
        match self.reflow_scroll.get() {
            Some((over, placed)) if over == self.detail_scroll => placed,
            _ => self.detail_scroll,
        }
    }

    /// From the renderer: show the pane at `offset` after a re-flow.
    pub fn place_detail_scroll(&self, offset: usize) {
        self.reflow_scroll.set(Some((self.detail_scroll, offset)));
    }

    fn adopt_reflow_scroll(&mut self) {
        self.detail_scroll = self.pending_detail_scroll();
        self.reflow_scroll.set(None);
    }

    /// Put the detail pane back at the top-left and drop what both views
    /// remember, so each one opens there next time it is shown.
    ///
    /// Called from every path that changes which node the pane is
    /// describing. An offset into one node's document addresses nothing in
    /// another's — neither the showing view's own place nor the other
    /// view's stored one — so all four numbers go at once.
    pub fn reset_detail_scroll(&mut self) {
        self.reflow_scroll.set(None);
        self.detail_scroll = 0;
        self.detail_hscroll = 0;
        self.flag_scroll_shown.set(None);
        self.saved_detail_offsets = [DetailOffsets::default(); 2];
    }

    /// `h`/`←`: scroll the detail pane left, when it has focus (spec §9:
    /// preformatted detail-pane content scrolls horizontally rather than
    /// wrapping). A no-op when the config toggle is off, or already at the
    /// left edge — `saturating_sub` alone would still be correct here, but
    /// the early return also means a disabled toggle never touches
    /// [`Self::detail_hscroll`] at all, which is one less thing for the
    /// "with the toggle off nothing is ever drawn at an offset" property
    /// to depend on holding elsewhere.
    pub fn detail_hscroll_left(&mut self) {
        if !self.sideways() {
            return;
        }
        self.detail_hscroll = self
            .detail_hscroll
            .min(self.detail_max_hscroll.get())
            .saturating_sub(crate::fit::HSCROLL_STEP);
    }

    /// `l`/`→`: scroll the detail pane right, clamped to
    /// [`Self::detail_max_hscroll`] — the widest preformatted line on
    /// screen minus the viewport width, written by the renderer each frame
    /// exactly like [`Self::detail_max_scroll`] is for vertical scroll.
    pub fn detail_hscroll_right(&mut self) {
        if !self.sideways() {
            return;
        }
        let max = self.detail_max_hscroll.get();
        self.detail_hscroll = self
            .detail_hscroll
            .min(max)
            .saturating_add(crate::fit::HSCROLL_STEP)
            .min(max);
    }

    /// Whether rows are hidden above the detail pane's window.
    pub fn detail_can_scroll_up(&self) -> bool {
        self.flag_scroll_shown
            .get()
            .unwrap_or_else(|| self.clamped_detail_scroll())
            > 0
    }

    /// Whether rows are hidden below the detail pane's window.
    pub fn detail_can_scroll_down(&self) -> bool {
        self.flag_scroll_shown
            .get()
            .unwrap_or_else(|| self.clamped_detail_scroll())
            < self.detail_max_scroll.get()
    }

    /// Whether the view the detail pane is showing is the raw `--help` text
    /// or the `unparsed` fallback, as opposed to the parsed page.
    pub fn showing_verbatim(&self) -> bool {
        self.raw_mode || self.selected_node().is_some_and(|n| !n.unparsed.is_empty())
    }

    /// Whether the parsed page wraps.
    pub fn parsed_wraps(&self) -> bool {
        self.parsed_wrap
    }

    /// Whether the view on screen scrolls sideways instead of wrapping.
    pub fn sideways(&self) -> bool {
        if self.showing_verbatim() {
            self.horizontal_scroll_enabled
        } else {
            !self.parsed_wrap
        }
    }

    /// `w`: flip wrapping in whichever view is showing, and return to
    /// column 0. Each view keeps its own state.
    pub fn toggle_wrap(&mut self) {
        self.adopt_reflow_scroll();
        if self.showing_verbatim() {
            self.horizontal_scroll_enabled = !self.horizontal_scroll_enabled;
        } else {
            self.parsed_wrap = !self.parsed_wrap;
        }
        self.detail_hscroll = 0;
    }

    /// `Esc` while the detail pane is scrolled sideways: back to column 0.
    /// Returns whether it did anything.
    pub fn return_to_column_zero(&mut self) -> bool {
        if self.clamped_detail_hscroll() == 0 {
            return false;
        }
        self.detail_hscroll = 0;
        true
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
        self.detail_hscroll.min(self.detail_max_hscroll.get())
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

mod ghost;
mod hit_marks;
mod settings_keys;
mod sort_keys;
pub(crate) use hit_marks::{HitMarks, MatchRow};

#[cfg(test)]
mod colour_tests;
#[cfg(test)]
mod contains_tests;
#[cfg(test)]
mod ghost_tests;
#[cfg(test)]
mod marking_tests;
#[cfg(test)]
mod repeat_tests;
#[cfg(test)]
mod search_tests;
#[cfg(test)]
mod settings_tests;
#[cfg(test)]
mod sort_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use mandible_core::{Provenance, Source};
    use std::time::{Duration, Instant};

    /// Each view owns its vertical place. `t` restores the target view's
    /// last line exactly — not a proportion of it — and scrolling one view
    /// leaves the other's saved line untouched. Supersedes the
    /// proportional carry, which resolved 100/200 of the parsed view into
    /// line 25 of a 50-line raw text: a line neither view was showing.
    #[test]
    fn each_view_keeps_its_own_vertical_place() {
        let mut app = App::new("git".to_string(), sample_tree());
        app.set_detail_extent(220, 20); // parsed: max 200
        app.detail_scroll = 100;

        // First visit to the raw view starts at the top, whatever the
        // parsed view is showing.
        app.toggle_raw_mode();
        app.set_detail_extent(70, 20); // raw: max 50
        assert_eq!(app.clamped_detail_scroll(), 0);
        app.detail_scroll_down();
        assert_eq!(app.clamped_detail_scroll(), 1);

        // Back to parsed: exactly where it was, not 1/50 of 200.
        app.toggle_raw_mode();
        app.set_detail_extent(220, 20);
        assert_eq!(app.clamped_detail_scroll(), 100);

        // And the raw view kept its own line while parsed was showing.
        app.toggle_raw_mode();
        app.set_detail_extent(70, 20);
        assert_eq!(app.clamped_detail_scroll(), 1);
    }

    /// The horizontal offset is remembered per view on the same terms, and
    /// a column scrolled in one view never appears in the other.
    #[test]
    fn each_view_keeps_its_own_horizontal_place() {
        let mut app = App::new("git".to_string(), sample_tree());
        app.horizontal_scroll_enabled = true;
        app.set_detail_hextent(140, 40); // parsed: max 100
        app.detail_hscroll = 50;

        app.toggle_raw_mode();
        app.set_detail_hextent(90, 40); // raw: max 50
        assert_eq!(app.clamped_detail_hscroll(), 0, "first visit starts left");
        app.detail_hscroll_right();
        assert_eq!(app.clamped_detail_hscroll(), crate::fit::HSCROLL_STEP);

        app.toggle_raw_mode();
        app.set_detail_hextent(140, 40);
        assert_eq!(app.clamped_detail_hscroll(), 50);

        app.toggle_raw_mode();
        app.set_detail_hextent(90, 40);
        assert_eq!(app.clamped_detail_hscroll(), crate::fit::HSCROLL_STEP);
    }

    /// The rapid `t`-`t`-`t` comparison the key exists for: with no scroll
    /// key pressed in between, each view keeps returning to its own place,
    /// on both axes, for as many flips as it takes.
    #[test]
    fn repeated_toggles_return_each_view_to_its_own_place() {
        let mut app = App::new("git".to_string(), sample_tree());
        app.set_detail_extent(220, 20); // parsed: max 200
        app.set_detail_hextent(140, 40); // parsed: max 100
        app.detail_scroll = 150;
        app.detail_hscroll = 64;

        // Give the raw view a place of its own to return to.
        app.toggle_raw_mode();
        app.set_detail_extent(70, 20); // raw: max 50
        app.set_detail_hextent(90, 40); // raw: max 50
        app.detail_scroll = 38;
        app.detail_hscroll = 24;

        for _ in 0..3 {
            app.toggle_raw_mode();
            app.set_detail_extent(220, 20);
            app.set_detail_hextent(140, 40);
            assert_eq!(app.clamped_detail_scroll(), 150);
            assert_eq!(app.clamped_detail_hscroll(), 64);

            app.toggle_raw_mode();
            app.set_detail_extent(70, 20);
            app.set_detail_hextent(90, 40);
            assert_eq!(app.clamped_detail_scroll(), 38);
            assert_eq!(app.clamped_detail_hscroll(), 24);
        }
    }

    /// Moving the tree selection puts the pane back at the top-left in
    /// both views, not just the one on screen. Keyboard navigation used to
    /// clear the horizontal offset and leave the vertical one pointing
    /// into the document of the node the reader had just left, while a
    /// mouse click on the very same row cleared both.
    #[test]
    fn moving_the_selection_resets_both_views() {
        let mut app = App::new("git".to_string(), sample_tree());
        app.set_detail_extent(220, 20);
        app.set_detail_hextent(140, 40);
        app.detail_scroll = 100;
        app.detail_hscroll = 32;

        app.toggle_raw_mode();
        app.set_detail_extent(70, 20);
        app.set_detail_hextent(90, 40);
        app.detail_scroll = 30;
        app.detail_hscroll = 16;

        app.move_down();
        assert_eq!(app.clamped_detail_scroll(), 0, "raw view starts at the top");
        assert_eq!(app.clamped_detail_hscroll(), 0);

        // The parsed view has nothing stored for the new node either.
        app.toggle_raw_mode();
        app.set_detail_extent(220, 20);
        app.set_detail_hextent(140, 40);
        assert_eq!(app.clamped_detail_scroll(), 0);
        assert_eq!(app.clamped_detail_hscroll(), 0);
    }

    /// A keypress that cannot move the selection must not move anything
    /// else either: `k` on the first row and `j` on the last leave both
    /// views' offsets and the flag target exactly where the reader left
    /// them.
    ///
    /// Maintainer-reported. The scroll reset belongs to "the pane now
    /// describes a different node", and every navigation path used to run
    /// it unconditionally — so reading down a long `FLAGS` list and
    /// pressing `j` once past the end threw the document back to the top
    /// with nothing on screen having changed to explain why.
    #[test]
    fn a_boundary_press_leaves_both_views_where_they_were() {
        let mut app = App::new("git".to_string(), sample_tree());
        app.set_detail_extent(220, 20);
        app.set_detail_hextent(140, 40);

        for (label, at_end) in [("first row", false), ("last row", true)] {
            app.selected = if at_end { app.rows().len() - 1 } else { 0 };
            app.detail_scroll = 100;
            app.detail_hscroll = 32;
            // Give the other view a stored place too, so a reset that
            // wipes `saved_detail_offsets` cannot hide behind the
            // showing view's own numbers.
            app.toggle_raw_mode();
            app.set_detail_extent(70, 20);
            app.set_detail_hextent(90, 40);
            app.detail_scroll = 30;
            app.detail_hscroll = 16;
            app.selected_flag = Some(mandible_core::FlagKey::Long("verbose".to_string()));

            let before = app.selected;
            if at_end {
                app.move_down();
            } else {
                app.move_up();
            }

            assert_eq!(app.selected, before, "{label}: the selection cannot move");
            assert_eq!(app.clamped_detail_scroll(), 30, "{label}: showing view");
            assert_eq!(app.clamped_detail_hscroll(), 16, "{label}: showing view");
            assert!(app.selected_flag.is_some(), "{label}: flag target");

            // The view that was not showing kept its place as well.
            app.toggle_raw_mode();
            app.set_detail_extent(220, 20);
            app.set_detail_hextent(140, 40);
            assert_eq!(app.clamped_detail_scroll(), 100, "{label}: stored view");
            assert_eq!(app.clamped_detail_hscroll(), 32, "{label}: stored view");
        }
    }

    /// A root with one plain child per name, for paging tests.
    fn wide_tree(names: &[String]) -> CommandNode {
        let mut root = CommandNode::new("tool", Provenance::single(Source::HelpText));
        for name in names {
            root.subcommands
                .push(CommandNode::new(name, Provenance::single(Source::HelpText)));
        }
        root
    }

    fn numbered(prefix: &str, n: usize) -> Vec<String> {
        (0..n).map(|i| format!("{prefix}{i:02}")).collect()
    }

    #[test]
    fn tree_paging_steps_ten_rows_and_clamps_at_both_ends() {
        let mut app = App::new("tool".to_string(), wide_tree(&numbered("c", 25)));
        app.ensure_rows_fresh();
        app.tree_page_down();
        assert_eq!(app.selected, 10);
        app.tree_page_down();
        assert_eq!(app.selected, 20);
        app.tree_page_down();
        assert_eq!(app.selected, 25, "26 rows: clamped to the last");
        app.tree_page_up();
        assert_eq!(app.selected, 15);
        app.tree_home();
        assert_eq!(app.selected, 0);
        app.tree_page_up();
        assert_eq!(app.selected, 0, "clamped at the top");
        app.tree_end();
        assert_eq!(app.selected, 25);
    }

    #[test]
    fn tree_paging_keeps_the_cursor_in_view() {
        let mut app = App::new("tool".to_string(), wide_tree(&numbered("c", 25)));
        app.ensure_rows_fresh();
        app.tree_viewport = 8;
        app.tree_page_down();
        assert!(app.selected >= app.tree_scroll && app.selected < app.tree_scroll + 8);
        app.tree_end();
        assert_eq!(app.tree_scroll, 25 + 1 - 8);
        app.tree_home();
        assert_eq!(app.tree_scroll, 0);
    }

    #[test]
    fn tree_paging_moves_only_through_filtered_rows() {
        let mut names = numbered("aa", 20);
        names.extend(numbered("zq", 12));
        let mut app = App::new("tool".to_string(), wide_tree(&names));
        app.focus_search();
        for c in "zq".chars() {
            app.search_input_char(c);
        }
        settle_search(&mut app);
        app.ensure_rows_fresh();
        let shown: Vec<String> = app.rows().iter().map(|r| r.name.clone()).collect();
        assert!(shown.len() < 20, "filter narrows the list: {shown:?}");

        app.tree_end();
        assert_eq!(app.selected, shown.len() - 1);
        assert!(app.rows()[app.selected].name.starts_with("zq"));
        app.tree_home();
        app.tree_page_down();
        assert_eq!(app.selected, 10.min(shown.len() - 1));
        assert!(shown.contains(&app.rows()[app.selected].name));
    }

    #[test]
    fn detail_paging_steps_one_viewport_and_clamps() {
        let mut app = App::new("git".to_string(), sample_tree());
        app.set_detail_extent(100, 30); // max 70, page 30
        app.detail_page_down();
        assert_eq!(app.clamped_detail_scroll(), 30);
        app.detail_page_down();
        app.detail_page_down();
        assert_eq!(app.clamped_detail_scroll(), 70);
        app.detail_page_up();
        assert_eq!(app.clamped_detail_scroll(), 40);
        app.detail_scroll_home();
        assert_eq!(app.clamped_detail_scroll(), 0);
        app.detail_scroll_end();
        assert_eq!(app.clamped_detail_scroll(), 70);
    }

    #[test]
    fn detail_page_starts_from_the_search_selected_flag_position() {
        let mut app = App::new("git".to_string(), sample_tree());
        app.set_detail_extent(200, 20); // max 180, page 20
        app.selected_flag = Some(mandible_core::FlagKey::Long("verbose".to_string()));
        app.set_flag_scroll_shown(Some(90));

        app.detail_page_down();

        assert_eq!(app.clamped_detail_scroll(), 110);
        assert!(app.selected_flag.is_none());
    }

    /// The other half of the same rule: a press that *does* move still
    /// resets, so the guard cannot be satisfied by never resetting at all.
    #[test]
    fn a_real_move_still_resets_both_views() {
        let mut app = App::new("git".to_string(), sample_tree());
        app.selected = 1;
        app.set_detail_extent(220, 20);
        app.set_detail_hextent(140, 40);
        app.detail_scroll = 100;
        app.detail_hscroll = 32;
        app.selected_flag = Some(mandible_core::FlagKey::Long("verbose".to_string()));

        app.move_up();

        assert_eq!(app.selected, 0, "this press moves");
        assert_eq!(app.clamped_detail_scroll(), 0);
        assert_eq!(app.clamped_detail_hscroll(), 0);
        assert!(app.selected_flag.is_none());
    }

    /// The same rule on the other navigation paths. A click on the row
    /// already selected is a no-op; collapsing the selected row leaves the
    /// pane describing the very same node, so neither disturbs the
    /// reader's place, while jumping to the parent does.
    #[test]
    fn clicks_and_collapses_that_go_nowhere_leave_the_pane_alone() {
        let mut app = App::new("git".to_string(), sample_tree());
        app.selected = 2; // rebase, which has a child to collapse
        app.expand_selected();
        app.ensure_rows_fresh();
        app.set_detail_extent(220, 20);
        app.set_detail_hextent(140, 40);
        app.detail_scroll = 100;
        app.detail_hscroll = 32;

        app.select_index(2);
        assert_eq!(app.clamped_detail_scroll(), 100, "click on the same row");
        assert_eq!(app.clamped_detail_hscroll(), 32, "click on the same row");

        app.collapse_or_jump_to_parent();
        assert_eq!(app.selected_node().unwrap().name, "rebase");
        assert_eq!(app.clamped_detail_scroll(), 100, "collapse is not a move");
        assert_eq!(app.clamped_detail_hscroll(), 32, "collapse is not a move");

        // Now the same key genuinely changes node, and resets.
        app.collapse_or_jump_to_parent();
        assert_eq!(app.selected_node().unwrap().name, "git");
        assert_eq!(app.clamped_detail_scroll(), 0);
        assert_eq!(app.clamped_detail_hscroll(), 0);
    }

    /// Bring the search index up to date with the tree, as the event
    /// loop's `tick_search` does before every render.
    fn settle_search(app: &mut App) {
        app.flush_search_index();
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

    fn path(segments: &[&str]) -> Vec<String> {
        segments.iter().map(|s| s.to_string()).collect()
    }

    /// `mandible git rebase` lands exactly where browsing there lands: the
    /// ancestors opened, the node selected (spec §5.4).
    #[test]
    fn a_requested_path_selects_its_node_once_the_tree_has_it() {
        let mut app = App::new("git".to_string(), sample_tree_known_complete());
        app.requested_path = Some(path(&["git", "rebase", "--onto-helper"]));

        app.settle_requested_path();

        assert_eq!(app.selected_node().unwrap().name, "--onto-helper");
        assert!(
            app.requested_path.is_none(),
            "a landed path must not be re-applied on every later fill"
        );
    }

    /// The tree is empty until the background root fill arrives (spec §5.2
    /// step 1), and a path that has not arrived yet is not a path that is
    /// missing — nothing may be reported until the parent that would hold it
    /// is known-complete.
    #[test]
    fn a_requested_path_waits_instead_of_reporting_a_node_that_has_not_arrived() {
        let stub = CommandNode::new("git", Provenance::default());
        let mut app = App::new("git".to_string(), stub);
        app.requested_path = Some(path(&["git", "rebase"]));

        app.settle_requested_path();

        assert!(app.status_message.is_none(), "{:?}", app.status_message);
        assert!(
            app.requested_path.is_some(),
            "the intent must survive until the tree can answer"
        );
    }

    /// And once it can answer, it says so rather than sitting on a
    /// selection the user never asked for.
    #[test]
    fn a_requested_path_that_the_finished_tree_lacks_is_reported() {
        let mut app = App::new("git".to_string(), sample_tree_known_complete());
        app.requested_path = Some(path(&["git", "nosuchthing"]));

        app.settle_requested_path();

        let message = app.status_message.clone().expect("should have reported");
        assert!(message.contains("nosuchthing"), "{message}");
        assert!(app.requested_path.is_none());
    }

    /// A fill still in flight for the parent is not an answer either.
    #[test]
    fn a_requested_path_waits_while_its_parent_is_still_filling() {
        let mut app = App::new("git".to_string(), sample_tree_known_complete());
        app.requested_path = Some(path(&["git", "nosuchthing"]));
        app.mark_pending(path(&["git"]));

        app.settle_requested_path();

        assert!(app.status_message.is_none(), "{:?}", app.status_message);
        assert!(app.requested_path.is_some());
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
        root.subcommands[1].entities.push(autosquash); // rebase

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

    /// Spec §10 extended to dashless kinds: searching a *modifier*'s
    /// description in wide mode must surface its parent command and
    /// remember the modifier as the scroll target, exactly as a flag
    /// match does — the same mechanism, generalized by `FlagKey::Name`
    /// rather than a per-kind branch.
    #[test]
    fn searching_a_modifier_description_selects_its_parent_command() {
        let mut root = sample_tree();
        let mut d = mandible_core::Entity::modifier('d', Provenance::single(Source::HelpText));
        d.description = Some(mandible_core::Text::sanitize(
            "delete a member from the archive",
        ));
        root.subcommands[1].entities.push(d); // rebase, standing in for the node

        let mut app = App::new("git".to_string(), root);
        app.focus_search();
        app.cycle_search_mode(); // Name -> Wide
        for c in "delete a member".chars() {
            app.search_input_char(c);
        }
        settle_search(&mut app);
        app.ensure_rows_fresh();

        assert_eq!(app.rows()[app.selected].name, "rebase");
        assert_eq!(
            app.selected_flag,
            Some(mandible_core::FlagKey::Name("d".to_string()))
        );
    }

    /// Spec §10, name mode: a name that contains the query anywhere
    /// matches, so `run` surfaces the command that owns `--no-trunc` as
    /// well as the command named `prune`, and the command comes first.
    #[test]
    fn name_mode_admits_the_middle_of_a_flag_name() {
        let mut root = sample_tree();
        let mut no_trunc =
            mandible_core::Entity::flag_long("no-trunc", Provenance::single(Source::HelpText));
        no_trunc.description = Some(mandible_core::Text::sanitize("don't truncate output"));
        root.subcommands[1].entities.push(no_trunc); // rebase, standing in for the node
        root.subcommands.push(CommandNode::new(
            "prune",
            Provenance::single(Source::HelpText),
        ));

        let mut app = App::new("git".to_string(), root);
        app.focus_search();
        for c in "run".chars() {
            app.search_input_char(c);
        }
        settle_search(&mut app);
        app.ensure_rows_fresh();

        let names: Vec<&str> = app.rows().iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"prune"), "{names:?}");
        assert!(names.contains(&"rebase"), "{names:?}");
        assert_eq!(app.selected_row().unwrap().name, "prune");
    }

    /// The word-prefix half of the same case: `trunc` (a real word of
    /// `--no-trunc`) does surface the parent command in name mode, and the
    /// detail pane remembers the flag as the scroll target.
    #[test]
    fn name_mode_admits_a_word_prefix_flag_match() {
        let mut root = sample_tree();
        let mut no_trunc =
            mandible_core::Entity::flag_long("no-trunc", Provenance::single(Source::HelpText));
        no_trunc.description = Some(mandible_core::Text::sanitize("don't truncate output"));
        root.subcommands[1].entities.push(no_trunc); // rebase

        let mut app = App::new("git".to_string(), root);
        app.focus_search();
        for c in "trunc".chars() {
            app.search_input_char(c);
        }
        settle_search(&mut app);
        app.ensure_rows_fresh();

        let names: Vec<&str> = app.rows().iter().map(|r| r.name.as_str()).collect();
        assert!(
            names.contains(&"rebase"),
            "'trunc' should admit '--no-trunc' in name mode: {names:?}"
        );
        assert_eq!(
            app.selected_flag,
            Some(mandible_core::FlagKey::Long("no-trunc".to_string()))
        );
    }

    /// Name mode also admits a dashless entity by its bare-letter query —
    /// closing spec §10's "any spelling's bare name" requirement for the
    /// modifier kind specifically, with a real (non-fallback) tree filter:
    /// the tree narrows to just `ar`, not every row.
    #[test]
    fn name_mode_admits_a_bare_modifier_letter_match() {
        let mut root = CommandNode::new("ar", Provenance::single(Source::HelpText));
        let mut d = mandible_core::Entity::modifier('d', Provenance::single(Source::HelpText));
        d.description = Some(mandible_core::Text::sanitize(
            "delete a member from the archive",
        ));
        root.entities.push(d);
        root.subcommands
            .push(CommandNode::new("m", Provenance::single(Source::HelpText)));

        let mut app = App::new("ar".to_string(), root);
        app.focus_search();
        for c in "d".chars() {
            app.search_input_char(c);
        }
        settle_search(&mut app);
        app.ensure_rows_fresh();

        // A real filter, not the empty-set fallback to the unfiltered
        // tree: only "ar" itself is a visible row (its "m" subcommand,
        // which the query doesn't match, is filtered out).
        let names: Vec<&str> = app.rows().iter().map(|r| r.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["ar"],
            "expected the tree filtered to just 'ar': {names:?}"
        );
        assert_eq!(
            app.selected_flag,
            Some(mandible_core::FlagKey::Name("d".to_string()))
        );
    }

    /// Manually moving the tree selection away from a flag search match
    /// must drop the flag scroll target — otherwise the detail pane would
    /// keep snapping back to a flag on a command the user has since
    /// navigated away from.
    ///
    /// The press has to be one that genuinely moves. The filtered tree
    /// here is two rows deep and the match puts the selection on the last
    /// of them, so `j` cannot move at all — and a press that changes
    /// nothing on screen leaves the target alone by design
    /// ([`App::settle_selection`]). `k` is the move; the assertion below
    /// that the selection changed keeps it one.
    #[test]
    fn manual_navigation_clears_the_selected_flag_target() {
        let mut root = sample_tree();
        let mut autosquash =
            mandible_core::Entity::flag_long("autosquash", Provenance::single(Source::HelpText));
        autosquash.description = Some(mandible_core::Text::sanitize(
            "Automatically squash commits",
        ));
        root.subcommands[1].entities.push(autosquash); // rebase

        let mut app = App::new("git".to_string(), root);
        app.focus_search();
        app.cycle_search_mode(); // Name -> Wide
        for c in "autosquash".chars() {
            app.search_input_char(c);
        }
        settle_search(&mut app);
        app.ensure_rows_fresh();
        assert!(app.selected_flag.is_some(), "precondition");

        let before = app.selected_path();
        app.move_up();
        assert_ne!(app.selected_path(), before, "the press must actually move");
        assert_eq!(app.selected_flag, None);
    }

    /// A rows refresh with the same query and the same top result (the
    /// background fill does this) leaves a manual scroll alone; a changed
    /// query targets again.
    #[test]
    fn refresh_keeps_a_manual_scroll_until_the_query_changes() {
        let mut root = sample_tree();
        let autosquash =
            mandible_core::Entity::flag_long("autosquash", Provenance::single(Source::HelpText));
        root.subcommands[1].entities.push(autosquash); // rebase

        let mut app = App::new("git".to_string(), root);
        app.focus_search();
        app.cycle_search_mode(); // Name -> Wide
        for c in "autosquash".chars() {
            app.search_input_char(c);
        }
        settle_search(&mut app);
        app.ensure_rows_fresh();
        assert!(app.selected_flag.is_some(), "precondition");

        app.detail_scroll_down();
        app.mark_dirty();
        app.ensure_rows_fresh();
        assert_eq!(app.selected_flag, None, "same top result: no re-target");

        app.search_backspace();
        settle_search(&mut app);
        app.ensure_rows_fresh();
        assert!(app.selected_flag.is_some(), "new query: target again");
    }

    /// `--print-selection`'s whole output, composed the way the user got
    /// there: search a flag spelling, and the line is that flag's command
    /// plus the flag — `git rebase --autosquash`, ready to edit.
    #[test]
    fn selection_command_composes_a_searched_flag_onto_its_command() {
        let mut root = sample_tree();
        root.subcommands[1]
            .entities
            .push(mandible_core::Entity::flag_spelled(
                Some('i'),
                Some("autosquash".to_string()),
                false,
                false,
                Provenance::single(Source::HelpText),
            ));

        let mut app = App::new("git".to_string(), root);
        app.focus_search();
        app.cycle_search_mode(); // Name -> Wide
        for c in "autosquash".chars() {
            app.search_input_char(c);
        }
        settle_search(&mut app);
        app.ensure_rows_fresh();

        assert_eq!(app.selected_row().unwrap().name, "rebase", "precondition");
        // The long spelling, not the `-i` the tool printed first.
        assert_eq!(
            app.selection_command().as_deref(),
            Some("git rebase --autosquash")
        );
    }

    /// No flag target: a command row composes to its own full path, which
    /// is the other half of what `Enter` accepts.
    #[test]
    fn selection_command_is_the_command_path_with_no_flag_target() {
        let mut app = App::new("git".to_string(), sample_tree());
        assert_eq!(app.selection_command().as_deref(), Some("git"));
        app.selected = 2; // rebase
        assert_eq!(app.selection_command().as_deref(), Some("git rebase"));
    }

    /// A flag target the selected command does not document composes the
    /// path alone. An incomplete line can be finished on the prompt; one
    /// carrying a flag the tool does not have cannot, and printing it
    /// would be the mode's one chance to hand over something wrong.
    #[test]
    fn selection_command_drops_a_flag_target_that_does_not_resolve() {
        let mut app = App::new("git".to_string(), sample_tree());
        app.selected = 2; // rebase, which documents no flags at all
        app.selected_flag = Some(mandible_core::FlagKey::Long("autosquash".to_string()));
        assert_eq!(app.selection_command().as_deref(), Some("git rebase"));
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
            app.root.edit().subcommands.push(child.clone());
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

        app.tick_search();
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
        app.parsed_wrap = false;
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
    fn detail_hscroll_is_a_noop_while_the_view_wraps() {
        let mut app = App::new("git".to_string(), sample_tree());
        app.parsed_wrap = true;
        app.set_detail_hextent(50, 20);
        app.detail_hscroll_right();
        app.detail_hscroll_right();
        assert_eq!(
            app.clamped_detail_hscroll(),
            0,
            "a wrapped view must not scroll at all"
        );
    }

    #[test]
    fn w_flips_wrapping_in_the_showing_view_only_and_returns_to_column_zero() {
        let mut app = App::new("git".to_string(), sample_tree());
        assert!(app.parsed_wraps() && !app.sideways());
        app.toggle_wrap();
        assert!(!app.parsed_wraps() && app.sideways());
        assert!(
            !app.horizontal_scroll_enabled,
            "the raw view keeps its own state"
        );
        app.set_detail_hextent(50, 20);
        app.detail_hscroll_right();
        assert!(app.clamped_detail_hscroll() > 0);
        app.toggle_wrap();
        assert_eq!(
            app.clamped_detail_hscroll(),
            0,
            "toggling returns to column 0"
        );
        assert!(app.parsed_wraps());
    }

    #[test]
    fn escape_returns_to_column_zero_before_anything_else() {
        let mut app = App::new("git".to_string(), sample_tree());
        app.parsed_wrap = false;
        app.set_detail_hextent(50, 20);
        app.detail_hscroll_right();
        assert!(app.return_to_column_zero());
        assert!(!app.return_to_column_zero(), "already at column 0");
    }

    #[test]
    fn detail_hscroll_resets_on_selection_change() {
        let mut app = App::new("git".to_string(), sample_tree());
        app.parsed_wrap = false;
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
        app.parsed_wrap = false;
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

    #[test]
    fn detail_hscroll_overflow_flags_track_the_current_offset() {
        let mut app = App::new("git".to_string(), sample_tree());
        app.parsed_wrap = false;
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
