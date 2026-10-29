//! The seven colour rows of the settings tab (docs/design.md §9.5): the
//! value, a block in the colour and a sample of what it paints.

use crate::app::App;
use crate::sanitize::{display_width, truncate_to_width_marker};
use crate::settings::Row;
use crate::style::{self, Palette};
use mandible_core::config::{ColorIndex, Role};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// Columns the value field takes, arrows or prompt included. `bright-magenta`
/// is the longest name at 14.
const FIELD: usize = 17;

/// Pad `text` with spaces to `width` columns.
fn pad(text: &str, width: usize) -> String {
    let fill = width.saturating_sub(display_width(text));
    format!("{text}{}", " ".repeat(fill))
}

/// What `role` paints, as mandible draws it, in the palette that has the
/// role's value in force.
fn sample(role: Role, p: Palette, rule: &str) -> (String, Style) {
    let bars = rule.repeat(3);
    match role {
        Role::Accent => ("--squash".to_string(), style::accent(p)),
        Role::Warning => ("config.toml line 7".to_string(), style::warning(p)),
        Role::Muted => (
            "Record changes to the repository".to_string(),
            style::muted(p),
        ),
        Role::Section => (format!("FLAGS (41) {bars}"), style::section_rule(p)),
        Role::Group => (
            format!("Commit message options {bars}"),
            style::group_rule(p),
        ),
        Role::Selection => (" commit ".to_string(), style::bar_name(p)),
        Role::SelectionText => (" Record changes ".to_string(), style::bar_text(p)),
    }
}

/// The block's style: the colour as a foreground, or as the background for
/// the selection, falling back to the sample's own style where the bar is
/// a reversed row.
fn swatch_style(role: Role, sample: Style) -> Style {
    let color = match role {
        Role::Selection => sample.bg,
        Role::SelectionText => sample.fg,
        _ => sample.fg,
    };
    match color {
        Some(c) => Style::default().fg(c),
        None => Style::default().add_modifier(sample.add_modifier & Modifier::REVERSED),
    }
}

/// The value column of a colour row, cut to `room` columns: the full
/// indicator, else the value and its block, else the value alone. Every row
/// keeps or drops its sample by the width of the longest, so the column
/// reads the same on all seven.
pub(super) fn value_spans(app: &App, row: Row, selected: bool, room: usize) -> Vec<Span<'static>> {
    let Some(role) = row.role() else {
        return Vec::new();
    };
    let g = app.glyphs;
    let enabled = app.settings.enabled(row);
    let typing = enabled && selected && app.settings.typing.is_some();
    let shown = app.settings.shown_roles();
    let color = app.settings.typed_color().filter(|_| typing);
    let mut roles = shown;
    roles.set(role, color.unwrap_or_else(|| shown.get(role)));
    let text = if typing {
        format!(
            "{} {}{}",
            g.prompt,
            app.settings.typing.as_deref().unwrap_or_default(),
            g.cursor
        )
    } else {
        let value = ColorIndex::shown(roles.get(role));
        if selected && enabled {
            format!("{} {value}", g.value_open)
        } else {
            value
        }
    };
    let close = if selected && enabled && !typing {
        format!(" {}", g.value_close)
    } else {
        String::new()
    };
    let value_style = if enabled {
        Style::default()
    } else {
        style::muted(app.palette)
    };
    let (sample_text, sample_style) = sample(role, app.palette.with_roles(roles), g.rule);
    let block = g.swatch;
    let drawn = app.palette.color;
    // Room for the closing arrow is kept on every row, so moving the
    // highlight never makes a block or sample appear or vanish.
    let tail = display_width(g.value_close) + 1;
    let widest = Role::ALL
        .iter()
        .map(|r| display_width(&sample(*r, Palette::plain(), g.rule).0))
        .max()
        .unwrap_or(0);
    let with_sample = FIELD + 1 + display_width(block) + 1 + widest + tail;
    let hint_gap = "   ";
    let with_hint = FIELD
        + 1
        + display_width(block)
        + 1
        + display_width(&sample_text)
        + hint_gap.len()
        + display_width(g.type_hint)
        + tail;
    let with_block = FIELD + 1 + display_width(block) + tail;
    let mut spans = Vec::new();
    if drawn && with_block <= room {
        spans.push(Span::styled(pad(&text, FIELD), value_style));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(block, swatch_style(role, sample_style)));
        if with_sample <= room {
            spans.push(Span::raw(" "));
            spans.push(Span::styled(sample_text, sample_style));
            if typing && with_hint <= room {
                spans.push(Span::raw(hint_gap));
                spans.push(Span::styled(g.type_hint, style::muted(app.palette)));
            }
        }
    } else {
        let text = truncate_to_width_marker(&text, room.saturating_sub(tail), g.ellipsis);
        spans.push(Span::styled(text, value_style));
    }
    if !close.is_empty() {
        spans.push(Span::raw(close));
    }
    spans
}

/// One colour row: the indented label, then the value column.
pub(super) fn row_line(
    app: &App,
    row: Row,
    selected: bool,
    (label_w, content): (usize, usize),
) -> Line<'static> {
    let g = app.glyphs;
    let label =
        truncate_to_width_marker(&row.label(g), label_w.saturating_sub(1).max(1), g.ellipsis);
    let label_style = if app.settings.enabled(row) {
        Style::default()
    } else {
        style::muted(app.palette)
    };
    let mut spans = vec![Span::styled(pad(&label, label_w), label_style)];
    spans.extend(value_spans(
        app,
        row,
        selected,
        content.saturating_sub(label_w),
    ));
    Line::from(spans)
}
