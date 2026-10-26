//! A command's own DESCRIPTION section: drawn in place, or, when it would
//! fill most of the pane, as a short preview at the top with the full text
//! as the last section of the page (docs/design.md §9.3).

use super::*;

/// Fewest drawn rows a preview shows.
const MIN_PREVIEW_ROWS: usize = 6;

/// Fewest drawn rows a preview must hide.
const MIN_HIDDEN_ROWS: usize = 6;

/// How many drawn rows the preview shows in a pane `pane_height` rows tall:
/// a third of the pane, and never fewer than six.
pub(super) fn preview_rows(pane_height: usize) -> usize {
    MIN_PREVIEW_ROWS.max(pane_height / 3)
}

/// Whether a description that draws as `body_rows` rows previews in a pane
/// `pane_height` rows tall: it fills more than half the pane, and the
/// preview would hide at least [`MIN_HIDDEN_ROWS`] rows. A pane of height
/// zero has not been measured, so it never previews.
pub(super) fn wants_preview(body_rows: usize, pane_height: usize) -> bool {
    pane_height > 0
        && body_rows > pane_height / 2
        && body_rows >= preview_rows(pane_height) + MIN_HIDDEN_ROWS
}

/// The page under construction: its lines, and the item and rule starts
/// recorded for them.
pub(super) struct Page<'a> {
    pub(super) lines: &'a mut Vec<Line<'static>>,
    pub(super) items: &'a mut Vec<usize>,
    pub(super) rules: &'a mut Vec<usize>,
}

/// The description's drawn rows, and the row each logical line starts at.
pub(super) struct DescriptionBody {
    pub(super) lines: Vec<Line<'static>>,
    pub(super) starts: Vec<usize>,
}

/// Draw `text` at `text_width`: one blank row between paragraphs, none
/// after the last, each source line wrapped on its own at its own indent.
pub(super) fn description_body(text: &str, text_width: usize) -> DescriptionBody {
    let mut lines = Vec::new();
    let mut starts = Vec::new();
    let mut first = true;
    for paragraph in text.split("\n\n") {
        if paragraph.trim().is_empty() {
            continue;
        }
        if !first {
            lines.push(Line::default());
        }
        first = false;
        for logical in paragraph.split('\n') {
            starts.push(lines.len());
            let indent = logical.len() - logical.trim_start().len();
            let indent_str = " ".repeat(indent);
            let avail = text_width.saturating_sub(indent).max(1);
            for chunk in wrap_words(logical, avail) {
                lines.push(Line::from(format!("{indent_str}{chunk}")));
            }
        }
    }
    DescriptionBody { lines, starts }
}

/// Push the DESCRIPTION heading named `label` and `body` under it.
pub(super) fn push_description(
    page: &mut Page<'_>,
    label: &str,
    body: DescriptionBody,
    (width, palette, glyphs): (usize, style::Palette, Glyphs),
    gap: usize,
) {
    open_block(page.lines, gap);
    mark(page.items, page.lines.len());
    page.lines
        .push(heading_line_ruled(label, None, width, palette, glyphs));
    note_rule(page.rules, page.lines, glyphs);
    let base = page.lines.len();
    page.items.extend(body.starts.iter().map(|s| base + s));
    page.lines.extend(body.lines);
}

/// Push the preview: its heading, the first [`preview_rows`] rows of `body`
/// as drawn in the full text, an ellipsis row, and the pointer to the full
/// text.
pub(super) fn push_preview(
    page: &mut Page<'_>,
    body: &DescriptionBody,
    (width, palette, glyphs): (usize, style::Palette, Glyphs),
    pane_height: usize,
) {
    open_block(page.lines, SECTION_BLANKS);
    mark(page.items, page.lines.len());
    let label = format!("DESCRIPTION {} preview", glyphs.dot);
    page.lines
        .push(heading_line_ruled(&label, None, width, palette, glyphs));
    note_rule(page.rules, page.lines, glyphs);

    let count = preview_rows(pane_height).min(body.lines.len());
    let mut shown: Vec<Line<'static>> = body.lines[..count].to_vec();
    while shown.last().is_some_and(line_is_blank) {
        shown.pop();
    }
    mark(page.items, page.lines.len());
    page.lines.extend(shown);
    page.lines.push(Line::from(glyphs.ellipsis));

    let hint = format!("{} full description at the end", glyphs.arrow_down);
    let pad = " ".repeat(width.saturating_sub(display_width(&hint)));
    mark(page.items, page.lines.len());
    page.lines.push(Line::from(Span::styled(
        format!("{pad}{hint}"),
        style::section_rule(palette),
    )));
}
