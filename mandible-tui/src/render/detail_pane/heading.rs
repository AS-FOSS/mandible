//! The two kinds of ruled line in the detail pane: a section heading and a
//! group divider.

use super::*;

/// A section heading followed by a rule to the pane's edge, with the
/// section's entity count for list sections: `FLAGS (41)`. Label and rule
/// are drawn in [`style::section_rule`], the middle of the pane's three
/// neutral steps (spec §9.3); the rule goes through the glyph set so a
/// non-UTF-8 terminal gets `-` rather than tofu.
///
/// Distinguished from a group divider ([`group_divider_line`]) by shape,
/// not styling alone — CAPS-plus-count versus mixed-case-without — since
/// spec §9.2 forbids an attribute as the sole distinction between two
/// kinds of text.
pub(super) fn heading_line_ruled(
    text: &str,
    count: Option<usize>,
    width: usize,
    palette: style::Palette,
    glyphs: Glyphs,
) -> Line<'static> {
    let heading = match count {
        Some(n) => format!("{text} ({n})"),
        None => text.to_string(),
    };
    let used = display_width(&heading) + 1;
    let rule_width = width.saturating_sub(used);
    // Label drawn in exactly the rule's style, never bold: CAPS and the
    // count mark this as the outer level, not weight (spec §9.3).
    let shade = style::section_rule(palette);
    let mut spans = vec![Span::styled(heading, shade)];
    if rule_width > 0 {
        spans.push(Span::styled(" ", shade));
        spans.push(Span::styled(glyphs.rule.repeat(rule_width), shade));
    }
    Line::from(spans)
}

/// A group divider within a section (spec §9.3): the group's label at
/// column 0 with a rule running from it to the pane's edge,
/// `Operation ──────…`. Rows beneath stay at the section's normal margin;
/// grouping is drawn, never indented. Label-first like the section header
/// ([`heading_line_ruled`]), starting at the same column-0 left edge as
/// every heading and ungrouped row. Rule and label share
/// [`style::group_rule`], the dimmest of the pane's three neutral steps,
/// a clear step below the header's [`style::section_rule`] (spec §9.3);
/// never bold. `ruled` is false for a divider that opens its section —
/// see [`group_divider_lead_line`]. The label is tool-authored text of
/// unbounded length; [`group_divider_lines`] wraps one that cannot fit.
pub(super) fn group_divider_line(
    label: &str,
    width: usize,
    palette: style::Palette,
    glyphs: Glyphs,
    ruled: bool,
) -> Line<'static> {
    let shade = style::group_rule(palette);
    if !ruled {
        return group_divider_lead_line(label, width, palette, glyphs);
    }
    let label = label.to_string();
    let rule_width = width.saturating_sub(display_width(&label) + 1);
    let mut spans = vec![Span::styled(label, shade)];
    if rule_width > 0 {
        spans.push(Span::styled(" ", shade));
        spans.push(Span::styled(glyphs.rule.repeat(rule_width), shade));
    }
    Line::from(spans)
}

/// The divider that opens a section: its label alone, at column 0, with
/// no rule (spec §9.3) — a ruled divider directly under the header's own
/// full-width rule would read as one doubled line. Distinguishable from
/// the section header with every attribute stripped (spec §9.2): CAPS
/// with a count and a rule versus mixed case with neither.
pub(super) fn group_divider_lead_line(
    label: &str,
    width: usize,
    palette: style::Palette,
    glyphs: Glyphs,
) -> Line<'static> {
    let _ = (width, glyphs);
    Line::from(Span::styled(label.to_string(), style::group_rule(palette)))
}

/// The lines of one group divider. A label that does not fit the pane with
/// a rule after it wraps over rows of its own with no rule, and is never
/// cut: it is the only place the group's name is written.
pub(super) fn group_divider_lines(
    label: &str,
    width: usize,
    wrap: bool,
    palette: style::Palette,
    glyphs: Glyphs,
    ruled: bool,
) -> Vec<Line<'static>> {
    let furniture = display_width(glyphs.rule) + 1;
    if !wrap || display_width(label) + furniture <= width {
        return vec![group_divider_line(label, width, palette, glyphs, ruled)];
    }
    wrap_words(label, width)
        .into_iter()
        .map(|chunk| Line::from(Span::styled(chunk, style::group_rule(palette))))
        .collect()
}
