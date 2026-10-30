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
pub(crate) use detail_pane::is_empty_page;
pub(crate) use detail_pane::{entity_at, first_row_with_key};
mod help_colours;
mod help_overlay;
mod help_settings;
mod review_overlay;
mod search_bar;
mod status_bar;
mod tree_pane;

pub use detail_pane::page_text;
pub(crate) use detail_pane::{page_positions, PageCache, ReflowAnchor};

use crate::app::App;
use crate::fit;
use crate::layout::{self, Regions};
use ratatui::Frame;

/// The rectangle the `?` overlay occupies on a screen of `area`.
pub fn help_overlay_rect(
    area: ratatui::layout::Rect,
    print_selection: bool,
) -> ratatui::layout::Rect {
    help_overlay::popup_rect(area, print_selection)
}

/// Render one full frame for `app` into `frame`.
pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let regions: Regions = layout::compute(area, app.focus);

    if regions.too_small {
        draw_too_small(frame, area);
        return;
    }

    search_bar::render(frame, regions.search, app);
    if let Some(tree_rect) = regions.tree {
        tree_pane::render(frame, tree_rect, app);
    }
    if let Some(detail_rect) = regions.detail {
        detail_pane::render(frame, detail_rect, app);
    }
    status_bar::render(frame, regions.status, app);

    if app.show_help {
        let (max_scroll, visible) = help_overlay::render(frame, area, app);
        app.set_help_extent(max_scroll, visible);
    }

    if let Some(review) = &app.review {
        review_overlay::render(frame, area, review, app.glyphs);
    }
}

/// The only thing drawn under the minimum size: [`fit::too_small_message`],
/// centred, broken over rows only when the terminal is narrower than the
/// sentence itself.
fn draw_too_small(frame: &mut Frame, area: ratatui::layout::Rect) {
    let width = usize::from(area.width);
    let mut rows: Vec<String> = Vec::new();
    let mut current = String::new();
    for word in fit::too_small_message().split_whitespace() {
        let joined = if current.is_empty() {
            word.to_string()
        } else {
            format!("{current} {word}")
        };
        if !current.is_empty() && crate::sanitize::display_width(&joined) > width {
            rows.push(std::mem::take(&mut current));
            current = word.to_string();
        } else {
            current = joined;
        }
    }
    rows.push(current);
    let top = area
        .height
        .saturating_sub(u16::try_from(rows.len()).unwrap_or(0))
        / 2;
    let lines: Vec<ratatui::text::Line> = rows
        .into_iter()
        .map(|r| ratatui::text::Line::from(r).centered())
        .collect();
    let rect = ratatui::layout::Rect::new(area.x, area.y + top, area.width, area.height - top);
    frame.render_widget(ratatui::widgets::Paragraph::new(lines), rect);
}

#[cfg(test)]
mod help_colours_tests;
#[cfg(test)]
mod help_settings_tests;
