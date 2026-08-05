//! The status bar: keybinding hints, or a transient status message (spec
//! §2's footer row: `↑↓ move   → expand   / search   y copy   ? help   q
//! quit`).

use crate::app::App;
use crate::sanitize::{defensive_single_line, truncate_to_width};
use crate::style;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

const HINTS: &str = "↑↓ move   → expand   / search   y copy   ? help   q quit";

/// Render the status bar into `area` (a single row, no border, per spec
/// §2's layout).
pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let text = match &app.status_message {
        Some(msg) => defensive_single_line(msg),
        None => HINTS.to_string(),
    };
    let truncated = truncate_to_width(&text, area.width as usize);
    let style = if app.status_message.is_some() {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        style::muted(app.color_enabled)
    };
    let paragraph = Paragraph::new(Line::styled(truncated, style));
    frame.render_widget(paragraph, area);
}
