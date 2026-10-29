//! The `?` pop-up: the keys tab (spec §2 "Interaction model" table) and the
//! frame both tabs share. The settings tab is in `help_settings`.

use super::help_settings;
use crate::app::App;
use crate::sanitize::display_width;
use crate::settings::HelpTab;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph};
use ratatui::Frame;

/// `None` in the key slot is a section heading rather than a binding.
const BINDINGS: &[(Option<&str>, &str)] = &[
    (None, "MOVE"),
    (Some("↑ ↓  k j"), "Move selection"),
    (Some("PgUp PgDn"), "Move 10 rows, or scroll a screenful"),
    (Some("Home End"), "First or last row, or top or bottom"),
    (Some(EXPAND_KEYS), "Expand"),
    (Some("←  h"), "Collapse, or jump to parent"),
    (Some("Tab"), "Switch between tree and detail"),
    (None, "SEARCH"),
    (Some("/"), "Search; again to widen names → everything"),
    (Some("Tab"), "In the box: take the dim suggestion"),
    (Some("n"), "Next match on the page"),
    (Some("p"), "Previous match"),
    (
        Some("Esc"),
        "Back to column 0 if scrolled sideways; else clear the search. In the box: Enter keeps it, Esc cancels it",
    ),
    (None, "VIEW"),
    (
        Some("t"),
        "Verbatim: the tool\u{2019}s own --help, unparsed",
    ),
    (Some("w"), "Wrap text, or scroll it sideways, in this view"),
    (
        Some("s"),
        "Sort the focused pane A–Z; the detail pane sorts within each section first",
    ),
    (
        Some("h l  ← →"),
        "Scroll sideways when wrap is off (detail pane focused)",
    ),
    (None, "ACTIONS"),
    (Some("y"), "Copy selected flag or command path"),
    (Some("r"), "Reload what you are viewing"),
    (Some("?"), "Toggle this help and settings"),
    (Some("Ctrl-C"), "Quit from anywhere"),
    (Some("q"), "Quit (from the tree)"),
];

/// The key `--print-selection` rebinds, as it reads in [`BINDINGS`].
const EXPAND_KEYS: &str = "→  Enter  l";

/// [`BINDINGS`] as `--print-selection` leaves them: `Enter` is the accept
/// key there, so the Expand row can no longer claim it and the overlay
/// gains the row that says what it does instead.
///
/// The overlay is the one place a user goes when a key did something they
/// did not expect. Leaving it describing the default bindings in a mode
/// that changed one would make it wrong exactly there.
fn bindings(print_selection: bool) -> Vec<(Option<&'static str>, &'static str)> {
    let mut rows = BINDINGS.to_vec();
    if !print_selection {
        return rows;
    }
    if let Some(at) = rows.iter().position(|(key, _)| *key == Some(EXPAND_KEYS)) {
        rows[at].0 = Some("→  l");
        rows.insert(at + 1, (Some("Enter"), "Print this selection and exit"));
    }
    rows
}

/// Width of the key column. Wide enough for the longest chord, so the
/// descriptions align into one column the way the detail pane's do.
const KEY_COLUMN: usize = 15;

/// Blank columns between the two columns of a wide overlay.
const COLUMN_GAP: usize = 3;

/// Cells the border and padding take from the popup.
const CHROME_W: usize = 6;
const CHROME_H: usize = 4;

type Rows = [(Option<&'static str>, &'static str)];

/// One display line of the overlay.
#[derive(Debug, Clone, PartialEq, Eq)]
enum HelpLine {
    Blank,
    Heading(String),
    Row { key: String, desc: String },
    Continuation(String),
}

/// What [`plan`] decides: where the popup goes and what each column holds.
#[derive(Debug)]
struct Plan {
    popup: Rect,
    columns: Vec<Vec<HelpLine>>,
    /// Lines of content the popup shows at once.
    visible: usize,
    /// Content lines beyond `visible`; zero when everything fits.
    max_scroll: usize,
}

type Section = (&'static str, Vec<(&'static str, &'static str)>);

/// Bindings grouped under their headings, in order.
fn sections(rows: &Rows) -> Vec<Section> {
    let mut out: Vec<Section> = Vec::new();
    for (key, desc) in rows {
        match key {
            None => out.push((desc, Vec::new())),
            Some(key) => {
                if let Some(last) = out.last_mut() {
                    last.1.push((key, desc));
                }
            }
        }
    }
    out
}

/// Split `text` into lines of at most `width` cells, breaking at spaces.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split(' ') {
        match lines.last_mut() {
            Some(line) if display_width(line) + 1 + display_width(word) <= width => {
                line.push(' ');
                line.push_str(word);
            }
            _ => lines.push(word.to_string()),
        }
    }
    lines
}

/// One section as lines for a column `width` cells wide.
fn section_lines(section: &Section, width: usize) -> Vec<HelpLine> {
    let room = width.saturating_sub(KEY_COLUMN).max(1);
    let mut lines = vec![HelpLine::Heading(section.0.to_string())];
    for (key, desc) in &section.1 {
        for (n, part) in wrap(desc, room).into_iter().enumerate() {
            lines.push(if n == 0 {
                HelpLine::Row {
                    key: (*key).to_string(),
                    desc: part,
                }
            } else {
                HelpLine::Continuation(part)
            });
        }
    }
    lines
}

/// Join sections into one column, a blank line between them.
fn join_sections(parts: &[Vec<HelpLine>]) -> Vec<HelpLine> {
    let mut out = Vec::new();
    for (n, part) in parts.iter().enumerate() {
        if n > 0 {
            out.push(HelpLine::Blank);
        }
        out.extend(part.iter().cloned());
    }
    out
}

/// Order-preserving split into two columns with the fewest lines in the
/// taller one; a section is never divided.
fn split_balanced(parts: &[Vec<HelpLine>]) -> Vec<Vec<HelpLine>> {
    let height = |p: &[Vec<HelpLine>]| join_sections(p).len();
    let at = (1..parts.len())
        .min_by_key(|&k| height(&parts[..k]).max(height(&parts[k..])))
        .unwrap_or(1);
    vec![join_sections(&parts[..at]), join_sections(&parts[at..])]
}

/// Decide the overlay's shape from its content and the screen.
///
/// The popup is as large as its content, one cell short of the screen edge
/// on every side. Two columns are used once each can hold a typical
/// description on one line. Content taller than the popup scrolls.
fn plan(rows: &Rows, area: Rect) -> Plan {
    let groups = sections(rows);
    let mut widths: Vec<usize> = groups
        .iter()
        .flat_map(|g| g.1.iter().map(|(_, d)| display_width(d)))
        .collect();
    widths.sort_unstable();
    let widest = widths.last().copied().unwrap_or(0);
    let typical = widths.get(widths.len() / 2).copied().unwrap_or(0);

    let room_w = usize::from(area.width).saturating_sub(2 + CHROME_W).max(1);
    let two_columns = groups.len() > 1 && room_w >= 2 * (KEY_COLUMN + typical) + COLUMN_GAP;
    let col_w = if two_columns {
        (KEY_COLUMN + widest).min((room_w - COLUMN_GAP) / 2)
    } else {
        (KEY_COLUMN + widest).min(room_w)
    };
    let parts: Vec<Vec<HelpLine>> = groups.iter().map(|g| section_lines(g, col_w)).collect();
    let columns = if two_columns {
        split_balanced(&parts)
    } else {
        vec![join_sections(&parts)]
    };

    let content = columns.iter().map(Vec::len).max().unwrap_or(0);
    let wide = columns.len() * col_w + (columns.len() - 1) * COLUMN_GAP + CHROME_W;
    let max_h = usize::from(area.height).saturating_sub(2).max(CHROME_H + 1);
    let tall = (content + CHROME_H).min(max_h);
    let popup_w = u16::try_from(wide).unwrap_or(u16::MAX).min(area.width);
    let popup_h = u16::try_from(tall).unwrap_or(u16::MAX).min(area.height);
    let popup = Rect::new(
        area.x + (area.width - popup_w) / 2,
        area.y + (area.height - popup_h) / 2,
        popup_w,
        popup_h,
    );
    let visible = tall - CHROME_H;
    Plan {
        popup,
        columns,
        visible,
        max_scroll: content.saturating_sub(visible),
    }
}

/// Where the overlay is drawn on `area`.
pub fn popup_rect(area: Rect, print_selection: bool) -> Rect {
    plan(&bindings(print_selection), area).popup
}

/// The border's title: `help · keys  settings`, the tab on show in the
/// accent and the other muted.
fn title(app: &App) -> Line<'static> {
    let color = app.color_enabled;
    let on_show = |tab| {
        if app.settings.tab == tab {
            crate::style::accent(color)
        } else {
            crate::style::muted(color)
        }
    };
    let help = if color {
        Style::default().add_modifier(ratatui::style::Modifier::BOLD)
    } else {
        Style::default()
    };
    Line::from(vec![
        Span::raw(" "),
        Span::styled("help", help),
        Span::raw(format!(" {} ", app.glyphs.dot)),
        Span::styled("keys", on_show(HelpTab::Keys)),
        Span::raw("  "),
        Span::styled("settings", on_show(HelpTab::Settings)),
        Span::raw(" "),
    ])
}

/// Render the pop-up centered over `full_area`, on the tab `app` shows.
/// Returns `(max_scroll, visible)` so the caller can bound the keys tab's
/// scrolling.
pub fn render(frame: &mut Frame, full_area: Rect, app: &App) -> (usize, usize) {
    let glyphs = app.glyphs;
    let plan = plan(&bindings(app.print_selection), full_area);
    let on_keys = app.settings.tab == HelpTab::Keys;
    let settings_lines = help_settings::lines().len();
    let (max_scroll, scroll) = if on_keys {
        (plan.max_scroll, app.help_scroll().min(plan.max_scroll))
    } else {
        let scroll = help_settings::scroll_to_selection(app, plan.visible);
        (settings_lines.saturating_sub(plan.visible), scroll)
    };
    frame.render_widget(Clear, plan.popup);

    let border = Style::default();
    let block = Block::default()
        .title(title(app))
        .borders(Borders::ALL)
        .border_set(crate::style::border_set(glyphs))
        .border_style(border)
        // An overlay sits on top of content, so it needs more separation
        // from its own border than a pane does.
        .padding(Padding::new(2, 2, 1, 1));
    let inner = block.inner(plan.popup);
    frame.render_widget(block, plan.popup);
    help_settings::draw_bottom_label(frame, plan.popup, app);
    let set = crate::style::border_set(glyphs);
    let buf = frame.buffer_mut();
    if scroll > 0 {
        let up = crate::fit::Marker::Up;
        crate::fit::draw_vertical_marker(buf, plan.popup, &glyphs, up, border, set.horizontal_top);
    }
    if scroll < max_scroll {
        let down = crate::fit::Marker::Down;
        let rule = set.horizontal_bottom;
        crate::fit::draw_vertical_marker(buf, plan.popup, &glyphs, down, border, rule);
    }
    let result = (max_scroll, plan.visible);
    if inner.width == 0 || inner.height == 0 {
        return result;
    }
    if !on_keys {
        help_settings::draw(frame, plan.popup, inner, app, scroll);
        return result;
    }

    let count = plan.columns.len();
    let col_w = (inner.width as usize).saturating_sub((count - 1) * COLUMN_GAP) / count;
    for (n, column) in plan.columns.iter().enumerate() {
        let x = inner.x + u16::try_from(n * (col_w + COLUMN_GAP)).unwrap_or(0);
        let rect = Rect::new(x, inner.y, u16::try_from(col_w).unwrap_or(0), inner.height);
        let lines: Vec<Line> = column
            .iter()
            .skip(scroll)
            .take(plan.visible)
            .map(|line| styled(line, app.color_enabled))
            .collect();
        frame.render_widget(Paragraph::new(lines), rect);
    }
    result
}

fn styled(line: &HelpLine, color_enabled: bool) -> Line<'static> {
    match line {
        HelpLine::Blank => Line::default(),
        HelpLine::Heading(h) => Line::styled(h.clone(), crate::style::muted_bold(color_enabled)),
        HelpLine::Row { key, desc } => {
            let pad = KEY_COLUMN.saturating_sub(display_width(key)).max(1);
            Line::from(vec![
                Span::styled(key.clone(), crate::style::accent(color_enabled)),
                Span::raw(" ".repeat(pad)),
                Span::raw(desc.clone()),
            ])
        }
        HelpLine::Continuation(text) => Line::from(format!("{}{text}", " ".repeat(KEY_COLUMN))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default overlay is the table spec §2 prints, untouched.
    #[test]
    fn the_default_overlay_is_unchanged() {
        assert_eq!(bindings(false), BINDINGS.to_vec());
    }

    /// In `--print-selection`, `Enter` is documented once, as the accept
    /// key — the Expand row must not also claim it, or the overlay
    /// contradicts itself about the only key the mode moved.
    #[test]
    fn print_selection_moves_enter_off_the_expand_row() {
        let rows = bindings(true);
        assert!(
            !rows.iter().any(|(key, _)| *key == Some(EXPAND_KEYS)),
            "Expand must give Enter up: {rows:?}"
        );
        let enter: Vec<_> = rows
            .iter()
            .filter(|(key, _)| key.is_some_and(|k| k.contains("Enter")))
            .collect();
        assert_eq!(enter.len(), 1, "exactly one Enter row: {enter:?}");
        assert_eq!(enter[0].1, "Print this selection and exit");
        // Expansion keeps its other two keys, so nothing is unreachable.
        assert!(rows
            .iter()
            .any(|(key, desc)| *key == Some("→  l") && *desc == "Expand"));
    }

    fn area(w: u16, h: u16) -> Rect {
        Rect::new(0, 0, w, h)
    }

    fn keys_in(lines: &[HelpLine]) -> Vec<String> {
        lines
            .iter()
            .filter_map(|l| match l {
                HelpLine::Row { key, .. } => Some(key.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn no_two_keys_differ_only_by_case_and_n_and_p_are_listed() {
        let rows = bindings(false);
        let keys: Vec<&str> = rows
            .iter()
            .filter_map(|(k, _)| *k)
            .flat_map(str::split_whitespace)
            .collect();
        for a in &keys {
            for b in &keys {
                assert!(a == b || a.to_lowercase() != b.to_lowercase(), "{a} {b}");
            }
        }
        assert!(rows.contains(&(Some("n"), "Next match on the page")));
        assert!(rows.contains(&(Some("p"), "Previous match")));
    }

    #[test]
    fn a_wide_screen_gets_two_columns_split_between_sections() {
        let p = plan(&bindings(false), area(140, 45));
        assert_eq!(p.columns.len(), 2);
        assert_eq!(p.max_scroll, 0);
        for column in &p.columns {
            assert!(matches!(column.first(), Some(HelpLine::Heading(_))));
        }
        assert!(p.popup.height < 45 - 2, "sized to content: {:?}", p.popup);
    }

    #[test]
    fn a_narrow_screen_gets_one_column_sized_to_content() {
        let p = plan(&bindings(false), area(80, 40));
        assert_eq!(p.columns.len(), 1);
        assert_eq!(p.max_scroll, 0);
        assert!(p.popup.width <= 78 && p.popup.height <= 38);
    }

    #[test]
    fn a_short_screen_scrolls_and_every_row_is_reachable() {
        let rows = bindings(true);
        let p = plan(&rows, area(60, 14));
        assert!(p.max_scroll > 0);
        assert!(p.popup.height <= 12);
        let column = &p.columns[0];
        let mut seen = Vec::new();
        for offset in 0..=p.max_scroll {
            let shown: Vec<HelpLine> = column
                .iter()
                .skip(offset)
                .take(p.visible)
                .cloned()
                .collect();
            seen.extend(keys_in(&shown));
        }
        for key in rows.iter().filter_map(|(k, _)| *k) {
            assert!(seen.iter().any(|s| s == key), "unreachable: {key}");
        }
    }

    #[test]
    fn nothing_scrolls_when_everything_fits() {
        assert_eq!(plan(&bindings(false), area(110, 32)).max_scroll, 0);
    }
}
