//! The search bar: a 3-row box at the top of the screen (spec §2).

use crate::app::{App, Focus};
use crate::sanitize::defensive_single_line;
use crate::style;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

/// Render the search bar into `area`.
pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    let focused = app.focus == Focus::Search;
    let border_style = if focused {
        style::accent(app.color_enabled)
    } else {
        Style::default()
    };
    let block = Block::default()
        .title(format!(
            " search [{}]  (/ switches) ",
            app.search_mode.label()
        ))
        .borders(Borders::ALL)
        .border_set(style::border_set(app.glyphs))
        .border_style(border_style);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let query = defensive_single_line(&app.search_query);

    // Unfocused, the bar shows whatever filter is *actually* in effect —
    // `app.active_filter()`, the same value the tree pane filters on —
    // not just the pinned one. `Enter` moves focus to the tree without
    // clearing `search_query`, so keying the display off `search_pinned`
    // alone made the text vanish while the filter stayed active: the tree
    // was still filtered by a query the user could no longer see, and it
    // reappeared when they focused the box again. A filter that is in
    // effect must always be visible.
    let text = if focused {
        format!("{} {query}", app.glyphs.prompt)
    } else if let Some(active) = app.active_filter() {
        let label = if app.search_pinned.is_some() && app.search_query.is_empty() {
            "pinned"
        } else {
            "filter"
        };
        format!("({label}) {}", defensive_single_line(active))
    } else {
        format!("{} ", app.glyphs.prompt)
    };

    let truncated = crate::sanitize::truncate_to_width(&text, inner.width as usize);
    let paragraph = Paragraph::new(Line::from(truncated));
    frame.render_widget(paragraph, inner);
}
