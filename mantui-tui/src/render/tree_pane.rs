//! The tree pane: one row per visible [`crate::tree::TreeRow`], at fixed
//! column offsets `[indent 2*depth][chevron 1][space 1][name][space]
//! [summary dim]` (spec §9), so mouse hit-testing (`crate::event`) can
//! compute the chevron column arithmetically.

use super::ACCENT;
use crate::app::{App, Focus};
use crate::sanitize::{defensive_single_line, display_width, truncate_to_width};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui::Frame;

/// Render the tree pane into `area`.
pub fn render(frame: &mut Frame, area: Rect, app: &App, hide_summaries: bool) {
    let focused = app.focus == Focus::Tree;
    let border_style = if focused {
        Style::default().fg(ACCENT)
    } else {
        Style::default()
    };
    let title = format!(" {} ", defensive_single_line(&app.tool));
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let inner_width = inner.width as usize;
    let visible_height = inner.height as usize;
    let rows = app.rows();

    let max_scroll = rows.len().saturating_sub(visible_height);
    let scroll = app.tree_scroll.min(max_scroll);

    let lines: Vec<Line> = rows
        .iter()
        .enumerate()
        .skip(scroll)
        .take(visible_height)
        .map(|(i, row)| build_row_line(row, inner_width, hide_summaries, i == app.selected))
        .collect();

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, inner);
}

fn build_row_line(
    row: &crate::tree::TreeRow,
    width: usize,
    hide_summary: bool,
    selected: bool,
) -> Line<'static> {
    let indent = "  ".repeat(row.depth);
    let chevron = if !row.has_children {
        ' '
    } else if row.expanded {
        '▾'
    } else {
        '▸'
    };
    let name = defensive_single_line(&row.name);

    let mut text = format!("{indent}{chevron} {name}");
    let mut spans = Vec::new();

    let base_style = if selected {
        Style::default().fg(ACCENT).add_modifier(Modifier::REVERSED)
    } else {
        Style::default()
    };

    if row.pending {
        // A lazy fill is in flight for this node (spec §5.2 step 3, §9
        // "designed degraded states"): show a subtle spinner instead of a
        // (possibly stale or absent) summary, so the user can see
        // something is actively happening rather than a row that looks
        // simply empty.
        text.push_str("  ");
        let name_part_width = display_width(&text);
        if name_part_width < width {
            let remaining = width - name_part_width;
            let marker = truncate_to_width("⋯ loading", remaining);
            let truncated_name_part = truncate_to_width(&text, width);
            spans.push(Span::styled(truncated_name_part, base_style));
            spans.push(Span::styled(
                marker,
                Style::default().add_modifier(Modifier::DIM),
            ));
            return Line::from(spans);
        }
        let truncated = truncate_to_width(&text, width);
        spans.push(Span::styled(truncated, base_style));
        return Line::from(spans);
    }

    if !hide_summary {
        if let Some(summary) = &row.summary {
            let clean_summary = defensive_single_line(summary);
            if !clean_summary.is_empty() {
                // Two spaces (not one) so the summary reads as a distinct
                // dim column rather than a continuation of the name.
                text.push_str("  ");
                let name_part_width = display_width(&text);
                if name_part_width < width {
                    let remaining = width - name_part_width;
                    let truncated_summary = truncate_to_width(&clean_summary, remaining);
                    let truncated_name_part = truncate_to_width(&text, width);
                    spans.push(Span::styled(truncated_name_part, base_style));
                    spans.push(Span::styled(
                        truncated_summary,
                        Style::default().add_modifier(Modifier::DIM),
                    ));
                    return Line::from(spans);
                }
            }
        }
    }

    let truncated = truncate_to_width(&text, width);
    spans.push(Span::styled(truncated, base_style));
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::TreeRow;

    fn row(depth: usize, name: &str, summary: Option<&str>, has_children: bool) -> TreeRow {
        TreeRow {
            path: vec![name.to_string()],
            depth,
            name: name.to_string(),
            summary: summary.map(|s| s.to_string()),
            has_children,
            expanded: false,
            children_filled: true,
            hidden: false,
            pending: false,
        }
    }

    #[test]
    fn chevron_position_matches_two_times_depth() {
        let r = row(2, "onto", None, false);
        let line = build_row_line(&r, 80, false, false);
        let rendered: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        // indent "    " (4 chars = 2*depth) then chevron/space/name.
        assert_eq!(&rendered[0..4], "    ");
    }

    #[test]
    fn adversarial_name_never_produces_embedded_newline() {
        let r = row(0, "evil\nname\x1b[31m", None, true);
        let line = build_row_line(&r, 80, false, false);
        let rendered: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(!rendered.contains('\n'));
    }

    #[test]
    fn long_summary_is_truncated_to_width() {
        let long_summary = "x".repeat(500);
        let r = row(0, "cmd", Some(&long_summary), false);
        let line = build_row_line(&r, 40, false, false);
        let rendered: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(display_width(&rendered) <= 40);
    }

    #[test]
    fn pending_row_shows_spinner_not_summary() {
        let mut r = row(0, "get", Some("should not show while pending"), true);
        r.pending = true;
        let line = build_row_line(&r, 80, false, false);
        let rendered: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(rendered.contains("loading"), "{rendered:?}");
        assert!(
            !rendered.contains("should not show while pending"),
            "{rendered:?}"
        );
    }

    #[test]
    fn pending_row_still_respects_width_budget() {
        let mut r = row(0, "get", None, true);
        r.pending = true;
        let line = build_row_line(&r, 12, false, false);
        let rendered: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(display_width(&rendered) <= 12);
    }
}
