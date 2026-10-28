//! The USAGE section: its heading, then each usage form wrapped to the pane.

use super::*;

/// Push the USAGE section onto `page`, `blanks` rows below what is there.
pub(super) fn push_usage(
    page: Page,
    node: &CommandNode,
    (width, text_width): (usize, usize),
    (palette, glyphs): (style::Palette, Glyphs),
    blanks: usize,
) {
    open_block(page.lines, blanks);
    mark(page.items, page.lines.len());
    page.lines
        .push(heading_line_ruled("USAGE", None, width, palette, glyphs));
    note_rule(page.rules, page.lines, glyphs);
    // Indented as a block, the way API documentation sets a signature
    // apart from its prose.
    let indent = "  ";
    let forms = usage_forms(&node.name, &node.usage);
    // A rendered USAGE line is mandible's own reconstruction, not the
    // tool author's layout: the parser already joined a usage that
    // wrapped over several physical lines into one logical line, so it
    // soft-wraps at the pane width in both modes instead of forcing
    // horizontal scroll (spec §9 rule 9). Continuation rows hang
    // `USAGE_HANG` columns right of the form's first row, so a form
    // that has to wrap still reads as one form, and the form's
    // command path is the accented part of its first row.
    let accent = style::accent(palette.color);
    for (pad, text, path_len) in &forms {
        mark(page.items, page.lines.len());
        let lead = format!("{indent}{}", " ".repeat(*pad));
        let avail = text_width.saturating_sub(display_width(&lead)).max(1);
        let mut path_left = text
            .get(..*path_len)
            .map_or(0, |p| p.chars().filter(|c| !c.is_whitespace()).count());
        for (i, chunk) in wrap_hanging(text, avail, fit::USAGE_HANG)
            .iter()
            .enumerate()
        {
            let row_lead = if i == 0 {
                lead.clone()
            } else {
                format!("{lead}{}", " ".repeat(fit::USAGE_HANG))
            };
            page.lines
                .push(usage_line(&row_lead, chunk, &mut path_left, accent));
        }
    }
}
