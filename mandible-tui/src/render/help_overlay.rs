//! The `?` keybinding overlay (spec §2 "Interaction model" table).

use crate::layout::centered_popup;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

const BINDINGS: &[(&str, &str)] = &[
    ("↑ / ↓, k / j", "Move tree selection"),
    ("→ / Enter / l", "Expand"),
    ("← / h", "Collapse, or jump to parent"),
    ("/", "Focus search"),
    ("Esc", "Leave search (pin), Esc again clears"),
    ("Tab", "Switch focus between tree and detail"),
    ("y", "Copy selected flag/path"),
    ("?", "Toggle this help"),
    ("r", "Re-extract this tool"),
    (".", "Toggle hidden/deprecated items"),
    ("q, Ctrl-C", "Quit"),
];

/// Render the overlay centered over `full_area`.
pub fn render(frame: &mut Frame, full_area: Rect, glyphs: crate::glyphs::Glyphs) {
    let popup = centered_popup(full_area, 60, 60);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .title(" keybindings ")
        .borders(Borders::ALL)
        .border_set(crate::style::border_set(glyphs));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let lines: Vec<Line> = BINDINGS
        .iter()
        .map(|(key, desc)| {
            let truncated = crate::sanitize::truncate_to_width(
                &format!("{key:<16}{desc}"),
                inner.width as usize,
            );
            Line::styled(truncated, Style::default().add_modifier(Modifier::empty()))
        })
        .collect();

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, inner);
}
