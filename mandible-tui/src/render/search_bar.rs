//! The search bar: a 3-row box at the top of the screen (spec §2).

use crate::app::{App, Focus};
use crate::sanitize::{defensive_single_line, display_width};
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
    let title = format!(
        " search {} {}  (/ switches) ",
        app.glyphs.dot,
        app.search_mode.label()
    );
    let title = crate::sanitize::truncate_to_width_marker(
        &title,
        usize::from(area.width.saturating_sub(2)),
        crate::fit::marker(&app.glyphs, crate::fit::Marker::Cut),
    );
    let block = Block::default()
        .title(title)
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
    let (lead, typed) = if focused {
        (format!("{} ", app.glyphs.prompt), query)
    } else if let Some(active) = app.active_filter() {
        let label = if app.search_pinned.is_some() && app.search_query.is_empty() {
            "pinned"
        } else {
            "filter"
        };
        (format!("({label}) "), defensive_single_line(active))
    } else {
        (format!("{} ", app.glyphs.prompt), String::new())
    };

    // A text field scrolls to keep the end of what is typed in view: the
    // lead stays, and the typed text shows its tail.
    let room = inner.width as usize;
    let typed_room = room.saturating_sub(display_width(&lead));
    let text = if typed_room == 0 {
        crate::sanitize::truncate_to_width(&typed_tail(&typed, room), room)
    } else {
        format!("{lead}{}", typed_tail(&typed, typed_room))
    };
    let paragraph = Paragraph::new(Line::from(text));
    frame.render_widget(paragraph, inner);
}

/// The last characters of `text` that fit in `room` columns.
fn typed_tail(text: &str, room: usize) -> String {
    let mut used = 0usize;
    let mut tail: Vec<char> = Vec::new();
    for ch in text.chars().rev() {
        used += unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if used > room {
            break;
        }
        tail.push(ch);
    }
    tail.into_iter().rev().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_end_of_a_long_query_stays_in_view() {
        assert_eq!(typed_tail("abcdef", 4), "cdef");
        assert_eq!(typed_tail("abc", 4), "abc");
        assert_eq!(typed_tail("日本語", 4), "本語");
    }
}
