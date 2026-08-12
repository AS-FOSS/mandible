//! The `mandible --review` session bar: verdict keys and sample progress
//! when idle, the note being typed when a verdict is being drafted (see
//! [`crate::app_review`]).
//!
//! Drawn last, directly over the bottom row of the frame, so a reviewer
//! judging one tool never loses sight of the tree or detail pane above it —
//! the whole point of building this inside the real TUI rather than a
//! separate review screen is that navigating and inspecting (including the
//! raw pane, `t`) work exactly as they do in `mandible <tool>`.

use crate::app_review::ReviewOverlay;
use crate::glyphs::Glyphs;
use crate::sanitize::truncate_to_width;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;

/// One pre-tag's compact display: `LABEL:Y` (suggested true), `LABEL:n`
/// (suggested false), or `LABEL:-` (not flagged) — the one-line equivalent
/// of `mandible_core::audit::tag_display`'s fuller prose, which doesn't fit
/// on a single status row.
fn compact_tag(label: &str, tag: Option<bool>) -> String {
    let mark = match tag {
        Some(true) => "Y",
        Some(false) => "n",
        None => "-",
    };
    format!("{label}:{mark}")
}

/// Render the review bar into the bottom row of `full_area`.
pub fn render(frame: &mut Frame, full_area: Rect, state: &ReviewOverlay, glyphs: Glyphs) {
    if full_area.height == 0 || full_area.width == 0 {
        return;
    }
    let bar = Rect::new(
        full_area.x,
        full_area.y + full_area.height - 1,
        full_area.width,
        1,
    );
    frame.render_widget(Clear, bar);

    let text = match &state.draft {
        Some(draft) => format!(
            "verdict: {}  note (optional): {}{}   Enter save   Esc cancel",
            draft.verdict, draft.note, glyphs.prompt,
        ),
        None => {
            let reason = state
                .include_reason
                .as_deref()
                .map(|r| format!("  forced({r})"))
                .unwrap_or_default();
            format!(
                "REVIEW {} [{}]{}  {} {} {}  {} pending of {}   \
                 c correct  i incomplete  w wrong  s skip  ^C quit session",
                state.tool,
                state.stratum,
                reason,
                compact_tag("K1", state.k1),
                compact_tag("K2", state.k2),
                compact_tag("K3", state.k3),
                state.remaining,
                state.total,
            )
        }
    };

    let truncated = truncate_to_width(&text, bar.width as usize);
    let style = Style::default().add_modifier(Modifier::BOLD);
    frame.render_widget(Paragraph::new(Line::styled(truncated, style)), bar);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_review::ReviewDraft;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn idle_state() -> ReviewOverlay {
        ReviewOverlay {
            tool: "openssl".to_string(),
            stratum: "suspicious".to_string(),
            k1: None,
            k2: Some(false),
            k3: Some(true),
            include_reason: None,
            remaining: 3,
            total: 12,
            draft: None,
        }
    }

    #[test]
    fn idle_bar_shows_tool_progress_and_tags() {
        let backend = TestBackend::new(100, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        let state = idle_state();
        terminal
            .draw(|frame| render(frame, frame.area(), &state, crate::glyphs::UNICODE))
            .unwrap();
        let content = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        assert!(content.contains("openssl"));
        assert!(content.contains("K3:Y"));
        assert!(content.contains("3 pending of 12"));
    }

    #[test]
    fn drafting_bar_shows_the_verdict_and_note() {
        let backend = TestBackend::new(100, 10);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut state = idle_state();
        state.draft = Some(ReviewDraft {
            verdict: "incomplete",
            note: "flags missing".to_string(),
        });
        terminal
            .draw(|frame| render(frame, frame.area(), &state, crate::glyphs::UNICODE))
            .unwrap();
        let content = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect::<String>();
        assert!(content.contains("incomplete"));
        assert!(content.contains("flags missing"));
    }

    #[test]
    fn zero_area_does_not_panic() {
        let backend = TestBackend::new(0, 0);
        let mut terminal = Terminal::new(backend).unwrap();
        let state = idle_state();
        terminal
            .draw(|frame| render(frame, frame.area(), &state, crate::glyphs::UNICODE))
            .unwrap();
    }
}
