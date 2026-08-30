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
    (Some(EXPAND_KEYS), "Expand"),
    (Some("←  h"), "Collapse, or jump to parent"),
    (Some("Tab"), "Switch between tree and detail"),
    (None, "SEARCH"),
    (Some("/"), "Focus search; again to widen names → everything"),
    (
        Some("Esc"),
        "Leave search, keeping the filter; again clears it",
    ),
    (None, "VIEW"),
    (
        Some("t"),
        "Verbatim: the tool\u{2019}s own --help, unparsed",
    ),
    (
        Some("h l  ← →"),
        "Scroll preformatted text sideways (detail pane focused)",
    ),
    (Some("."), "Show hidden and deprecated items"),
    (None, "ACTIONS"),
    (Some("y"), "Copy selected flag or command path"),
    (Some("r"), "Re-extract this tool"),
    (Some("?"), "Toggle this help"),
    (Some("Ctrl-C"), "Quit from anywhere"),
    (Some("q"), "Quit (from the tree)"),
];

/// The key `--print-selection` rebinds, as it reads in [`BINDINGS`].
const EXPAND_KEYS: &str = "→  Enter  l";

/// [`BINDINGS`] as `--print-selection` leaves them: `Enter` is the accept
/// key there, so the Expand row can no longer claim it and the overlay
/// gains the row that says what it does instead.
///
/// The overlay is the one place a user goes when a key did something they
/// did not expect. Leaving it describing the default bindings in a mode
/// that changed one would make it wrong exactly there.
fn bindings(print_selection: bool) -> Vec<(Option<&'static str>, &'static str)> {
    let mut rows = BINDINGS.to_vec();
    if !print_selection {
        return rows;
    }
    if let Some(at) = rows.iter().position(|(key, _)| *key == Some(EXPAND_KEYS)) {
        rows[at].0 = Some("→  l");
        rows.insert(at + 1, (Some("Enter"), "Print this selection and exit"));
    }
    rows
}

/// Width of the key column. Wide enough for the longest chord, so the
/// descriptions align into one column the way the detail pane's do.
const KEY_COLUMN: usize = 15;

/// Render the overlay centered over `full_area`.
pub fn render(
    frame: &mut Frame,
    full_area: Rect,
    glyphs: crate::glyphs::Glyphs,
    color_enabled: bool,
    print_selection: bool,
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
    let rows = bindings(print_selection);
    let mut lines: Vec<Line> = Vec::with_capacity(rows.len());
    for (key, desc) in &rows {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The default overlay is the table spec §2 prints, untouched.
    #[test]
    fn the_default_overlay_is_unchanged() {
        assert_eq!(bindings(false), BINDINGS.to_vec());
    }

    /// In `--print-selection`, `Enter` is documented once, as the accept
    /// key — the Expand row must not also claim it, or the overlay
    /// contradicts itself about the only key the mode moved.
    #[test]
    fn print_selection_moves_enter_off_the_expand_row() {
        let rows = bindings(true);
        assert!(
            !rows.iter().any(|(key, _)| *key == Some(EXPAND_KEYS)),
            "Expand must give Enter up: {rows:?}"
        );
        let enter: Vec<_> = rows
            .iter()
            .filter(|(key, _)| key.is_some_and(|k| k.contains("Enter")))
            .collect();
        assert_eq!(enter.len(), 1, "exactly one Enter row: {enter:?}");
        assert_eq!(enter[0].1, "Print this selection and exit");
        // Expansion keeps its other two keys, so nothing is unreachable.
        assert!(rows
            .iter()
            .any(|(key, desc)| *key == Some("→  l") && *desc == "Expand"));
    }
}
