//! The detail pane's page cache: the key a built page is reused under, and
//! the debug-build check that a reused page is still the page.

use super::BuiltLines;
use crate::app::App;
use crate::glyphs::Glyphs;
use crate::style;
use mandible_core::FlagKey;

/// Everything [`super::build_lines`] reads. A built page is reused only while all
/// of it is equal, so a new input to `build_lines` must be added here.
/// `tree_version` moves on every write to `app.root`; `height` decides
/// whether the description previews. Scroll and focus are applied after the build and the sideways offset is
/// re-applied every frame, so none of those are inputs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PageKey {
    path: Vec<String>,
    tree_version: u64,
    width: usize,
    height: usize,
    wrap: bool,
    palette: style::Palette,
    glyphs: Glyphs,
    target_flag: Option<FlagKey>,
}

impl PageKey {
    /// The key for the page `app` would draw at `width`.
    pub(super) fn of(app: &App, width: usize) -> Self {
        Self {
            path: app
                .selected_row()
                .map(|r| r.path.clone())
                .unwrap_or_default(),
            tree_version: app.root.version(),
            width,
            height: app.pane_height(),
            wrap: app.parsed_wraps(),
            palette: app.palette,
            glyphs: app.glyphs,
            target_flag: app.selected_flag.clone(),
        }
    }
}

/// The page cache entry: a built document, the key it was built for, and
/// whether the windowed draw may stand in for `Paragraph`'s defensive
/// wrapping.
pub(crate) struct PageCache {
    pub(super) key: PageKey,
    pub(super) built: std::rc::Rc<BuiltLines>,
    pub(super) windowable: bool,
}

/// Debug builds only: rebuild the page a cache hit is about to serve and
/// panic if it differs, so an input missing from [`PageKey`] fails a test
/// instead of showing an outdated page.
#[cfg(debug_assertions)]
pub(super) fn assert_hit_is_fresh(
    key: &PageKey,
    cached: &BuiltLines,
    node: &mandible_core::CommandNode,
    width: usize,
    app: &App,
) {
    let fresh = super::build_lines(
        node,
        width,
        app.palette,
        app.selected_flag.as_ref(),
        app.glyphs,
        app,
    );
    assert!(
        fresh.lines == cached.lines
            && fresh.target_flag_line == cached.target_flag_line
            && fresh.items == cached.items
            && fresh.rules == cached.rules,
        "page cache served an outdated page for {key:?}: the cached lines differ from a fresh build"
    );
}
