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
/// Built per-frame because the arrow glyphs depend on what the terminal
/// can draw (see [`crate::glyphs`]).
fn hints(glyphs: crate::glyphs::Glyphs) -> Vec<String> {
    vec![
        format!("{} move", glyphs.arrows_vertical),
        format!("{} expand", glyphs.arrows_horizontal),
        "/ search".to_string(),
        "Esc back".to_string(),
        "y copy".to_string(),
        "? help".to_string(),
        "^C quit".to_string(),
    ]
}

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
fn hints_for_width(width: usize, glyphs: crate::glyphs::Glyphs) -> String {
    let all = hints(glyphs);
    let (quit, rest) = all.split_last().expect("hints() is never empty");
    let mut kept: Vec<&str> = Vec::new();
    for hint in rest {
        let candidate_len = kept
            .iter()
            .copied()
            .chain(std::iter::once(hint.as_str()))
            .chain(std::iter::once(quit.as_str()))
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
        None => hints_for_width(area.width as usize, app.glyphs),
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
        let rendered = hints_for_width(120, crate::glyphs::UNICODE);
        for h in hints(crate::glyphs::UNICODE) {
            assert!(rendered.contains(&h), "{h} missing from {rendered:?}");
        }
    }

    /// Every hint stays readable without Unicode — the footer is the last
    /// thing that should turn into boxes for someone who cannot get out.
    #[test]
    fn ascii_fallback_hints_are_pure_ascii() {
        let rendered = hints_for_width(120, crate::glyphs::ASCII);
        assert!(rendered.is_ascii(), "{rendered:?}");
        assert!(rendered.contains("^C quit"));
    }

    /// The escape hatch survives at any width. Plain truncation used to cut
    /// this row mid-word (`^C qu` at 88 columns), removing the one hint a
    /// stuck user needs.
    #[test]
    fn quit_hint_survives_a_narrow_terminal() {
        for width in [20, 30, 40, 60, 88] {
            let hints = hints_for_width(width, crate::glyphs::UNICODE);
            assert!(hints.contains("^C quit"), "width {width}: {hints:?}");
            assert!(
                hints.chars().count() <= width.max(7),
                "width {width} overflowed: {hints:?}"
            );
        }
    }
}
