//! The search bar: a 3-row box at the top of the screen (spec §2).

use crate::app::{App, Focus};
use crate::sanitize::defensive_single_line;
use crate::style;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
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
        .title(" search ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let query = defensive_single_line(&app.search_query);
    let pinned = app.search_pinned.as_deref().map(defensive_single_line);

    let text = if focused {
        format!("› {query}")
    } else if let Some(pinned) = &pinned {
        format!("(pinned) {pinned}")
    } else {
        "› ".to_string()
    };

    let truncated = crate::sanitize::truncate_to_width(&text, inner.width as usize);
    let paragraph = Paragraph::new(Line::from(truncated));
    frame.render_widget(paragraph, inner);
}
