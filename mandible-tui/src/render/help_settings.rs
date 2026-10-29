//! The settings tab of the `?` pop-up and the labels on its borders
//! (docs/design.md §9.5).

use crate::app::App;
use crate::sanitize::{display_width, truncate_to_width_marker};
use crate::settings::{self, Group, HelpTab, Row};
use crate::style;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use std::path::Path;

/// Widest the label column grows.
const LABEL_COLUMN: usize = 36;

/// Columns a value with its arrows can take: `‹ A–Z in sections ›`.
const VALUE_COLUMN: usize = 21;

/// One line of the tab's content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SettingsLine {
    Blank,
    Heading(Group),
    Row(usize),
}

/// The tab's content top to bottom: each group's heading, its rows, and a
/// blank line between groups.
pub(super) fn lines() -> Vec<SettingsLine> {
    let mut out = Vec::new();
    let mut group = None;
    for (n, row) in Row::ALL.iter().enumerate() {
        if group != Some(row.group()) {
            if group.is_some() {
                out.push(SettingsLine::Blank);
            }
            out.push(SettingsLine::Heading(row.group()));
            group = Some(row.group());
        }
        out.push(SettingsLine::Row(n));
    }
    out
}

/// The first line to show so the highlighted row is in view, moving the
/// remembered offset no further than it must. A row that opens its group
/// brings the heading above it along.
pub(super) fn scroll_to_selection(app: &App, visible: usize) -> usize {
    let all = lines();
    let max = all.len().saturating_sub(visible);
    let at = all
        .iter()
        .position(|l| *l == SettingsLine::Row(app.settings.row))
        .unwrap_or(0);
    let mut scroll = app.settings.scroll.get().min(max);
    if at < scroll {
        scroll = at;
    } else if at >= scroll + visible {
        scroll = at + 1 - visible;
    }
    if at > 0 && matches!(all[at - 1], SettingsLine::Heading(_)) && at - 1 < scroll {
        scroll = at - 1;
    }
    app.settings.scroll.set(scroll);
    scroll
}

/// Draw the tab's lines into `inner`, the highlighted row on the bar
/// across the whole width of `popup`.
pub(super) fn draw(frame: &mut Frame, popup: Rect, inner: Rect, app: &App, scroll: usize) {
    let color = app.color_enabled;
    let content = usize::from(inner.width);
    let label_w = content
        .saturating_sub(VALUE_COLUMN)
        .max(content / 2)
        .min(LABEL_COLUMN);
    let mut bar_y = None;
    let mut out = Vec::new();
    for (n, line) in lines()
        .into_iter()
        .enumerate()
        .skip(scroll)
        .take(usize::from(inner.height))
    {
        out.push(match line {
            SettingsLine::Blank => Line::default(),
            SettingsLine::Heading(group) => Line::styled(group.heading(), style::muted_bold(color)),
            SettingsLine::Row(i) => {
                let selected = i == app.settings.row;
                if selected {
                    bar_y = Some(inner.y + u16::try_from(n - scroll).unwrap_or(0));
                }
                row_line(app, Row::ALL[i], selected, (label_w, content))
            }
        });
    }
    frame.render_widget(Paragraph::new(out), inner);
    if let Some(y) = bar_y {
        let bar = style::bar_text(app.palette);
        let buf = frame.buffer_mut();
        for x in popup.x + 1..popup.right().saturating_sub(1) {
            buf[(x, y)].set_style(bar);
        }
    }
}

/// One row: the label, then the value in the value column.
fn row_line(
    app: &App,
    row: Row,
    selected: bool,
    (label_w, content): (usize, usize),
) -> Line<'static> {
    let glyphs = app.glyphs;
    let locked = app.settings.locks.variable(row);
    let value = settings::value_text(app, row);
    let value = match locked {
        Some(var) => format!("{value} ({var})"),
        None if selected => format!("{} {value} {}", glyphs.value_open, glyphs.value_close),
        None => value,
    };
    let label = truncate_to_width_marker(
        &row.label(glyphs),
        label_w.saturating_sub(1).max(1),
        glyphs.ellipsis,
    );
    let pad = label_w.saturating_sub(display_width(&label));
    let value = truncate_to_width_marker(&value, content.saturating_sub(label_w), glyphs.ellipsis);
    let text = format!("{label}{}{value}", " ".repeat(pad));
    if locked.is_some() && !selected {
        Line::from(Span::styled(text, style::muted(app.color_enabled)))
    } else {
        Line::from(text)
    }
}

/// `path` with the home directory written as `~`.
pub(super) fn tilde(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|h| path.strip_prefix(h).ok()) {
        Some(rest) if !rest.as_os_str().is_empty() => format!("~/{}", rest.display()),
        _ => path.display().to_string(),
    }
}

/// The bottom border's label candidates for the tab on show, longest
/// first. The caller uses the first that fits.
pub(super) fn label_candidates(app: &App) -> Vec<String> {
    let g = app.glyphs;
    let sep = format!(" {} ", g.dot);
    let join = |parts: &[String]| parts.join(&sep);
    let mut out = Vec::new();
    match app.settings.tab {
        HelpTab::Keys => {
            let parts = vec![
                format!("{} scroll", g.arrows_vertical),
                "Tab settings".to_string(),
                "Esc close".to_string(),
            ];
            for from in 0..parts.len() {
                out.push(join(&parts[from..]));
            }
        }
        HelpTab::Settings => {
            if let Some(var) = app.settings.locks.variable(app.settings.selected()) {
                let reason = format!("set by {var}: unset it to change this here");
                let tail = ["Tab keys".to_string(), "Esc close".to_string()];
                out.push(join(&[reason.clone(), tail[0].clone(), tail[1].clone()]));
                out.push(join(&[reason.clone(), tail[0].clone()]));
                out.push(reason);
                out.push(format!("set by {var}"));
                return out;
            }
            let parts = vec![
                format!("{} choose", g.arrows_vertical),
                format!("{} change", g.arrows_horizontal),
                "Tab keys".to_string(),
                "Esc close".to_string(),
            ];
            if let Some(path) = &app.settings.config_path {
                let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
                let shown = tilde(path, home.as_deref());
                out.push(join(
                    &[parts.clone(), vec![format!("saved to {shown}")]].concat(),
                ));
            }
            for from in 0..parts.len() {
                out.push(join(&parts[from..]));
            }
        }
    }
    out
}

/// Write the first candidate that fits on the bottom border of `popup`:
/// right-aligned, a space either side, ending before the cell the `▼`
/// marker uses, and never touching the left corner.
pub(super) fn draw_bottom_label(frame: &mut Frame, popup: Rect, app: &App) {
    if popup.height == 0 {
        return;
    }
    let y = popup.bottom() - 1;
    let end = usize::from(
        popup
            .right()
            .saturating_sub(crate::fit::VERTICAL_MARKER_FROM_RIGHT),
    );
    let start_min = usize::from(popup.x) + 2;
    for text in label_candidates(app) {
        let width = display_width(&text) + 2;
        let Some(start) = end.checked_sub(width) else {
            continue;
        };
        if start < start_min {
            continue;
        }
        let x = u16::try_from(start).unwrap_or(0);
        let buf = frame.buffer_mut();
        buf.set_string(x, y, " ", ratatui::style::Style::default());
        buf.set_string(x + 1, y, &text, style::muted(app.color_enabled));
        let after = x + 1 + u16::try_from(display_width(&text)).unwrap_or(0);
        buf.set_string(after, y, " ", ratatui::style::Style::default());
        return;
    }
}
