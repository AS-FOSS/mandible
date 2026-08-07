//! Rendering: turns [`App`] state into a `ratatui` frame.
//!
//! Every widget in this module is permitted to assume text reaching it is
//! clean (spec §9): `CommandNode` prose fields are `Text`, sanitized at the
//! IR boundary (spec §4.1), and the few plain-`String` identity fields
//! (`name`, `group`) get an extra defensive pass via
//! [`crate::sanitize::defensive_single_line`] in [`tree_pane`] and
//! [`detail_pane`] before they ever reach a `Span`. Truncation is always by
//! display width (`unicode-width`), never byte or `char` count.

mod detail_pane;
mod help_overlay;
mod search_bar;
mod status_bar;
mod tree_pane;

use crate::app::App;
use crate::layout::{self, Regions};
use ratatui::Frame;

/// Render one full frame for `app` into `frame`.
pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let regions: Regions = layout::compute(area, app.focus);

    search_bar::render(frame, regions.search, app);
    if let Some(tree_rect) = regions.tree {
        tree_pane::render(frame, tree_rect, app, regions.hide_summaries);
    }
    if let Some(detail_rect) = regions.detail {
        detail_pane::render(frame, detail_rect, app);
    }
    status_bar::render(frame, regions.status, app);

    if app.show_help {
        help_overlay::render(frame, area, app.glyphs);
    }
}
