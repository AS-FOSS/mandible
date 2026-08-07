//! The `?` keybinding overlay (spec §2 "Interaction model" table).

use crate::layout::centered_popup;
use crate::sanitize::{display_width, truncate_to_width};
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph};
use ratatui::Frame;

/// `None` in the key slot is a section heading rather than a binding.
const BINDINGS: &[(Option<&str>, &str)] = &[
    (None, "MOVE"),
    (Some("↑ ↓  k j"), "Move selection"),
    (Some("→  Enter  l"), "Expand"),
    (Some("←  h"), "Collapse, or jump to parent"),
    (Some("Tab"), "Switch between tree and detail"),
    (None, "SEARCH"),
    (Some("/"), "Focus search; again to widen names → everything"),
    (
        Some("Esc"),
        "Leave search, keeping the filter; again clears it",
    ),
    (None, "ACTIONS"),
    (Some("y"), "Copy selected flag or command path"),
    (Some("r"), "Re-extract this tool"),
    (Some("."), "Show hidden and deprecated items"),
    (Some("?"), "Toggle this help"),
    (Some("Ctrl-C"), "Quit from anywhere"),
    (Some("q"), "Quit (from the tree)"),
];

/// Width of the key column. Wide enough for the longest chord, so the
/// descriptions align into one column the way the detail pane's do.
const KEY_COLUMN: usize = 15;

/// Render the overlay centered over `full_area`.
pub fn render(
    frame: &mut Frame,
    full_area: Rect,
    glyphs: crate::glyphs::Glyphs,
    color_enabled: bool,
) {
    let popup = centered_popup(full_area, 62, 70);
    frame.render_widget(Clear, popup);

    let block = Block::default()
        .title(" keybindings ")
        .borders(Borders::ALL)
        .border_set(crate::style::border_set(glyphs))
        // Breathing room on all four sides. An overlay sits *on top of*
        // content, so it needs more separation from its own border than a
        // pane does, or it reads as a hole punched in the screen rather
        // than a thing laid over it.
        .padding(Padding::new(2, 2, 1, 1));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let width = inner.width as usize;
    let mut lines: Vec<Line> = Vec::with_capacity(BINDINGS.len());
    for (key, desc) in BINDINGS {
        match key {
            // A section heading: a blank line above it (except first), then
            // the label. Grouping eleven bindings into three named sets is
            // the difference between a list to read and a list to scan.
            None => {
                if !lines.is_empty() {
                    lines.push(Line::default());
                }
                lines.push(Line::styled(
                    truncate_to_width(desc, width),
                    crate::style::muted_bold(color_enabled),
                ));
            }
            Some(key) => {
                let pad = KEY_COLUMN.saturating_sub(display_width(key)).max(1);
                lines.push(Line::from(vec![
                    Span::styled(key.to_string(), crate::style::accent(color_enabled)),
                    Span::raw(" ".repeat(pad)),
                    Span::raw(truncate_to_width(
                        desc,
                        width.saturating_sub(KEY_COLUMN).max(1),
                    )),
                ]));
            }
        }
    }

    frame.render_widget(Paragraph::new(lines), inner);
}
