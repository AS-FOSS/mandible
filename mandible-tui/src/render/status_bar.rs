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

/// One footer, true everywhere.
///
/// It used to change with focus, which meant the controls moved under the
/// user exactly when they were least sure of them. It also used to promise
/// `q quit` while `q` typed the letter q in the search box — the one
/// genuinely dangerous line, since someone who wants out, hammers `q`, and
/// watches `qqqq` appear in the filter has been *told* that should work.
///
/// So: `Ctrl-C` rather than `q`, because it is the only key that quits from
/// every focus; `Esc` named explicitly, because it is how you get out of
/// the search box; and wide separators, since a run of hints crammed
/// together reads as one long string rather than a list of keys.
const HINTS: &[&str] = &[
    "↑↓ move",
    "←→ expand",
    "/ search",
    "Esc back",
    "y copy",
    "? help",
    "^C quit",
];

/// Gap between hints. Wide on purpose: at two spaces the row reads as one
/// long string rather than a list of separate keys.
const HINT_GAP: &str = "    ";

/// Join as many hints as fit, **always keeping `^C quit`**.
///
/// Plain truncation cut the row mid-word at narrow widths — an 88-column
/// terminal showed `… ^C qu`, losing the one hint that matters most to
/// someone who is stuck. Hints are dropped from the least important end
/// instead, so what remains is always whole and always ends in the escape
/// hatch.
fn hints_for_width(width: usize) -> String {
    let quit = HINTS[HINTS.len() - 1];
    let mut kept: Vec<&str> = Vec::new();
    for hint in &HINTS[..HINTS.len() - 1] {
        let candidate_len = kept
            .iter()
            .chain(std::iter::once(hint))
            .chain(std::iter::once(&quit))
            .map(|h| h.chars().count())
            .sum::<usize>()
            + HINT_GAP.chars().count() * (kept.len() + 1);
        if candidate_len > width {
            break;
        }
        kept.push(hint);
    }
    kept.push(quit);
    kept.join(HINT_GAP)
}

/// Render the status bar into `area` (a single row, no border, per spec
/// §2's layout).
pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let text = match &app.status_message {
        Some(msg) => defensive_single_line(msg),
        None => hints_for_width(area.width as usize),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_terminal_shows_every_hint() {
        let hints = hints_for_width(120);
        for h in HINTS {
            assert!(hints.contains(h), "{h} missing from {hints:?}");
        }
    }

    /// The escape hatch survives at any width. Plain truncation used to cut
    /// this row mid-word (`^C qu` at 88 columns), removing the one hint a
    /// stuck user needs.
    #[test]
    fn quit_hint_survives_a_narrow_terminal() {
        for width in [20, 30, 40, 60, 88] {
            let hints = hints_for_width(width);
            assert!(hints.contains("^C quit"), "width {width}: {hints:?}");
            assert!(
                hints.chars().count() <= width.max(7),
                "width {width} overflowed: {hints:?}"
            );
        }
    }
}
