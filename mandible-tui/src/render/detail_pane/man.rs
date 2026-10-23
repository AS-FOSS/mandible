//! The man layout of one row: the head alone on its line, the description
//! on the lines below (docs/design.md, "Small terminals").

use super::*;

/// One row in man layout. The head carries every spelling joined, then its
/// placeholder, flush at the section's left edge; a head longer than the pane
/// wraps with [`fit::MAN_HEAD_HANG`]. The description follows, indented
/// [`fit::MAN_DESC_INDENT`] past the section's indent and wrapped to the
/// pane, then the `values:` and `env:` lines.
pub(super) fn man_entity_lines(
    flag: &Entity,
    dim: bool,
    width: usize,
    color_enabled: bool,
    layout: SectionLayout,
) -> Vec<Line<'static>> {
    let head_column = layout.indent;
    let leading = " ".repeat(head_column);
    let spelling_style = if dim {
        style::muted(color_enabled)
    } else {
        style::accent(color_enabled)
    };
    let value_style = style::muted(color_enabled);
    let desc_style = if dim {
        style::muted(color_enabled)
    } else {
        Style::default()
    };

    let name_spec = entity_name_spec(flag);
    let suffix = flag_value_suffix(flag);
    let head_text = format!("{name_spec}{suffix}");
    let avail = width.saturating_sub(head_column).max(1);
    let mut cursor = 0;
    let mut lines: Vec<Line<'static>> = Vec::new();
    for (i, chunk) in wrap_hanging(&head_text, avail, fit::MAN_HEAD_HANG)
        .iter()
        .enumerate()
    {
        while head_text[cursor..].starts_with(' ') {
            cursor += 1;
        }
        let start = cursor;
        cursor = (start + chunk.len()).min(head_text.len());
        let lead = if i == 0 {
            leading.clone()
        } else {
            format!("{leading}{}", " ".repeat(fit::MAN_HEAD_HANG))
        };
        let split = name_spec.len().clamp(start, cursor);
        let mut spans = vec![Span::styled(
            format!("{lead}{}", &head_text[start..split]),
            spelling_style,
        )];
        if split < cursor {
            spans.push(Span::styled(
                head_text[split..cursor].to_string(),
                value_style,
            ));
        }
        lines.push(Line::from(spans));
    }

    let column = layout.man_column();
    if let Some(description) = row_description(flag) {
        let indent = " ".repeat(column);
        let room = width.saturating_sub(column).max(1);
        for chunk in wrap_description(&description, room) {
            lines.push(Line::from(Span::styled(
                format!("{indent}{chunk}"),
                desc_style,
            )));
        }
    }
    lines.extend(subordinate_lines(flag, column, width, color_enabled, true));
    lines
}
