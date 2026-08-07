//! The status bar: keybinding hints, or a transient status message (spec
//! §2's footer row: `↑↓ move   → expand   / search   y copy   ? help   q
//! quit`).

use crate::app::{App, Focus};
use crate::sanitize::{defensive_single_line, truncate_to_width};
use crate::style;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

const HINTS: &str = "↑↓ move   → expand   / search   y copy   ? help   q quit";

/// Shown while the search box has focus, where the normal hints are not
/// merely unhelpful but actively wrong: `q` types the letter q there, and
/// a footer still advertising "q quit" invites exactly the trap of
/// hammering it and concluding the app is frozen. Every key named here
/// does what it says in that mode, and `Ctrl-C` is included because it is
/// the one escape that works from anywhere.
const SEARCH_HINTS: &str =
    "type to filter   ↑↓ move   Enter/Esc leave search   / names↔text   ^C quit";

/// Render the status bar into `area` (a single row, no border, per spec
/// §2's layout).
pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let text = match (&app.status_message, app.focus) {
        (Some(msg), _) => defensive_single_line(msg),
        (None, Focus::Search) => SEARCH_HINTS.to_string(),
        (None, _) => HINTS.to_string(),
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
