//! The `?` pop-up as drawn: both tabs, the bar, the locked rows, the border
//! labels, the startup footer line, and the page cache under the settings.

use crate::app::{App, Focus};
use crate::event::handle_key;
use crate::glyphs;
use crate::settings::{EnvLocks, HelpTab, Row};
use crate::style::{self, Palette};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mandible_core::{CommandNode, Entity, Provenance, Source, Text};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};
use ratatui::Terminal;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn node() -> CommandNode {
    let mut root = CommandNode::new("tool", Provenance::single(Source::HelpText));
    root.children_filled = true;
    root.entities = vec![
        Entity::flag_long("zeta", Provenance::single(Source::HelpText)),
        Entity::flag_long("alpha", Provenance::single(Source::HelpText)),
    ];
    root
}

fn app() -> App {
    let mut app = App::new("tool".to_string(), node());
    app.set_palette(Palette::extended());
    app.glyphs = glyphs::UNICODE;
    app.settings.glyphs_base = glyphs::UNICODE;
    app
}

fn settings_open(w: u16, h: u16) -> App {
    let mut app = app();
    app.toggle_help();
    app.settings.tab = HelpTab::Settings;
    let _ = draw(&app, w, h);
    app
}

fn select(app: &mut App, row: Row) {
    app.settings.row = Row::ALL.iter().position(|r| *r == row).unwrap();
}

fn draw(app: &App, w: u16, h: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal
        .draw(|frame| crate::render::render(frame, app))
        .unwrap();
    terminal.backend().buffer().clone()
}

fn line(buf: &Buffer, y: u16) -> String {
    (0..buf.area.width)
        .map(|x| buf[(x, y)].symbol())
        .collect::<String>()
}

fn lines(buf: &Buffer) -> Vec<String> {
    (0..buf.area.height).map(|y| line(buf, y)).collect()
}

/// The row of the screen that holds `needle`.
fn find(buf: &Buffer, needle: &str) -> Option<(u16, usize)> {
    (0..buf.area.height).find_map(|y| {
        let text = line(buf, y);
        text.find(needle).map(|at| (y, text[..at].chars().count()))
    })
}

fn popup(app: &App, w: u16, h: u16) -> Rect {
    crate::render::help_overlay_rect(Rect::new(0, 0, w, h), app.print_selection)
}

fn bottom(app: &App, buf: &Buffer, w: u16, h: u16) -> String {
    let p = popup(app, w, h);
    line(buf, p.bottom() - 1)
        .chars()
        .skip(usize::from(p.x))
        .take(usize::from(p.width))
        .collect()
}

#[test]
fn the_title_names_both_tabs_with_the_one_on_show_in_the_accent() {
    let mut app = app();
    app.toggle_help();
    let buf = draw(&app, 120, 30);
    let (y, x) = find(&buf, "help · keys  settings").expect("the title");
    let at = |offset: usize| &buf[(u16::try_from(x + offset).unwrap(), y)];
    assert_eq!(at(7).style().fg, Some(style::ACCENT), "keys is the accent");
    assert_eq!(
        at(13).style().fg,
        Some(Color::DarkGray),
        "settings is muted"
    );
    app.settings.tab = HelpTab::Settings;
    let buf = draw(&app, 120, 30);
    let (y, x) = find(&buf, "help · keys  settings").unwrap();
    let at = |offset: usize| &buf[(u16::try_from(x + offset).unwrap(), y)];
    assert_eq!(at(7).style().fg, Some(Color::DarkGray));
    assert_eq!(at(13).style().fg, Some(style::ACCENT));
}

#[test]
fn without_colour_the_active_tab_is_bold_and_the_other_plain() {
    let mut app = app();
    app.set_palette(Palette::plain());
    app.toggle_help();
    let buf = draw(&app, 120, 30);
    let (y, x) = find(&buf, "help · keys  settings").unwrap();
    let keys = &buf[(u16::try_from(x + 7).unwrap(), y)];
    let settings = &buf[(u16::try_from(x + 13).unwrap(), y)];
    assert!(keys.modifier.contains(Modifier::BOLD));
    assert!(!settings.modifier.contains(Modifier::BOLD));
    assert_eq!(settings.style().fg.unwrap_or(Color::Reset), Color::Reset);
}

#[test]
fn the_ascii_title_uses_the_glyph_sets_separator() {
    let mut app = app();
    app.glyphs = glyphs::ASCII;
    app.toggle_help();
    let buf = draw(&app, 120, 30);
    assert!(find(&buf, "help | keys  settings").is_some());
}

#[test]
fn the_keys_tab_is_plain_text_with_nothing_selected() {
    let mut app = app();
    app.toggle_help();
    let buf = draw(&app, 120, 30);
    let p = popup(&app, 120, 30);
    for y in p.y..p.bottom() {
        for x in p.x..p.right() {
            assert_ne!(buf[(x, y)].style().bg, Some(Color::Indexed(style::BAR_BG)));
        }
    }
    assert!(find(&buf, "Toggle this help and settings").is_some());
    assert!(bottom(&app, &buf, 120, 30).contains("↑↓ scroll · Tab settings · Esc close"));
}

#[test]
fn the_settings_tab_lists_the_rows_under_three_headings_without_key_names() {
    let app = settings_open(120, 30);
    let buf = draw(&app, 120, 30);
    let text = lines(&buf).join("\n");
    for expected in [
        "VIEW",
        "Open in the tool’s own --help",
        "Scroll long lines sideways",
        "Description preview",
        "ASCII glyphs",
        "SORT",
        "Flags",
        "Commands",
        "COLOURS",
        "Theme",
        "tool order",
        "default",
    ] {
        assert!(text.contains(expected), "{expected} missing:\n{text}");
    }
    for key_name in [
        "raw_view",
        "horizontal_scroll",
        "description_preview",
        "az-sections",
    ] {
        assert!(!text.contains(key_name), "{key_name} is a file detail");
    }
    let (y, x) = find(&buf, "VIEW").unwrap();
    let head = &buf[(u16::try_from(x).unwrap(), y)];
    assert_eq!(head.style().fg, Some(Color::DarkGray));
    assert!(head.modifier.contains(Modifier::BOLD));
    let (vy, _) = find(&buf, "Open in the tool").unwrap();
    let (fy, _) = find(&buf, "Flags").unwrap();
    assert!(vy > y && fy > vy);
}

#[test]
fn the_label_and_value_sit_in_two_columns() {
    let app = settings_open(120, 30);
    let buf = draw(&app, 120, 30);
    let mut columns = Vec::new();
    for label in [
        "Scroll long lines sideways",
        "Description preview",
        "Flags",
        "Commands",
        "Theme",
    ] {
        let (y, x) = find(&buf, label).unwrap();
        let text: Vec<char> = line(&buf, y).chars().collect();
        let value = text[x + label.chars().count()..]
            .iter()
            .position(|c| *c != ' ')
            .map(|n| x + label.chars().count() + n)
            .unwrap();
        columns.push(value);
    }
    assert!(columns.iter().all(|c| *c == columns[0]), "{columns:?}");
    assert!(columns[0] > 30, "{columns:?}");
}

#[test]
fn the_highlighted_row_is_the_grey_bar_with_its_value_in_arrows() {
    let mut app = settings_open(120, 30);
    select(&mut app, Row::Flags);
    let buf = draw(&app, 120, 30);
    let (y, _) = find(&buf, "Flags").unwrap();
    assert!(line(&buf, y).contains("‹ tool order ›"));
    let p = popup(&app, 120, 30);
    for x in p.x + 1..p.right() - 1 {
        let cell = &buf[(x, y)];
        assert_eq!(
            cell.style().bg,
            Some(Color::Indexed(style::BAR_BG)),
            "x={x}"
        );
    }
    assert_ne!(
        buf[(p.x, y)].style().bg,
        Some(Color::Indexed(style::BAR_BG))
    );
    let (other, _) = find(&buf, "Commands").unwrap();
    assert!(!line(&buf, other).contains('‹'));
    assert_ne!(
        buf[(p.x + 5, other)].style().bg,
        Some(Color::Indexed(style::BAR_BG))
    );
}

#[test]
fn the_bar_is_reversed_and_bold_without_colour() {
    let mut app = settings_open(120, 30);
    app.set_palette(Palette::plain());
    select(&mut app, Row::Commands);
    let buf = draw(&app, 120, 30);
    let (y, _) = find(&buf, "Commands").unwrap();
    let p = popup(&app, 120, 30);
    for x in p.x + 1..p.right() - 1 {
        let m = buf[(x, y)].modifier;
        assert!(m.contains(Modifier::REVERSED | Modifier::BOLD), "x={x}");
    }
}

#[test]
fn the_bar_is_reversed_where_only_sixteen_colours_exist() {
    let mut app = settings_open(120, 30);
    app.set_palette(Palette::basic());
    let buf = draw(&app, 120, 30);
    let (y, _) = find(&buf, "Open in the tool").unwrap();
    assert!(buf[(popup(&app, 120, 30).x + 3, y)]
        .modifier
        .contains(Modifier::REVERSED));
}

#[test]
fn a_row_set_by_the_environment_is_muted_and_says_which_variable() {
    let mut app = settings_open(120, 30);
    app.settings.locks = EnvLocks {
        ascii: true,
        color: Some("NO_COLOR"),
    };
    select(&mut app, Row::Flags);
    let buf = draw(&app, 120, 30);
    let (y, x) = find(&buf, "ASCII glyphs").unwrap();
    let text = line(&buf, y);
    assert!(text.contains("on (MANDIBLE_ASCII)"), "{text}");
    assert_eq!(
        buf[(u16::try_from(x).unwrap(), y)].style().fg,
        Some(Color::DarkGray)
    );
    let (ty, _) = find(&buf, "Theme").unwrap();
    assert!(line(&buf, ty).contains("none (NO_COLOR)"));

    select(&mut app, Row::Ascii);
    let buf = draw(&app, 120, 30);
    let (y, _) = find(&buf, "ASCII glyphs").unwrap();
    assert!(!line(&buf, y).contains('‹'), "no arrows on a locked row");
    assert_eq!(
        buf[(popup(&app, 120, 30).x + 3, y)].style().bg,
        Some(Color::Indexed(style::BAR_BG)),
        "still highlighted"
    );
    assert!(bottom(&app, &buf, 120, 30)
        .contains("set by MANDIBLE_ASCII: unset it to change this here · Tab keys · Esc close"));
}

#[test]
fn the_settings_border_says_where_it_saves_with_the_home_directory_as_a_tilde() {
    let mut app = settings_open(160, 30);
    app.settings.config_path = Some("/somewhere/else/mandible/config.toml".into());
    let buf = draw(&app, 160, 30);
    assert!(bottom(&app, &buf, 160, 30).contains(
        "↑↓ choose · ←→ change · Tab keys · Esc close · saved to /somewhere/else/mandible/config.toml"
    ));
    let home = std::path::Path::new("/srv/someone");
    let shown = super::help_settings::tilde(&home.join(".config/mandible/config.toml"), Some(home));
    assert_eq!(shown, "~/.config/mandible/config.toml");
    let elsewhere = super::help_settings::tilde(std::path::Path::new("/etc/x.toml"), Some(home));
    assert_eq!(elsewhere, "/etc/x.toml");
}

#[test]
fn a_short_border_drops_the_path_then_the_rest_and_keeps_its_corners() {
    let mut seen_full = false;
    let mut seen_no_path = false;
    let mut seen_close_only = false;
    for w in 30..=140u16 {
        let mut app = settings_open(w, 24);
        app.settings.config_path = Some("/srv/u/.config/mandible/config.toml".into());
        let buf = draw(&app, w, 24);
        let label = bottom(&app, &buf, w, 24);
        assert!(
            label.starts_with('╰') && label.ends_with('╯'),
            "w={w}: {label}"
        );
        let inner = &label[label.find('─').unwrap()..];
        assert!(
            inner.starts_with('─'),
            "a rule cell after the left corner, w={w}"
        );
        assert!(
            label.ends_with("─╯"),
            "a rule cell before the right corner, w={w}"
        );
        seen_full |= label.contains("saved to");
        seen_no_path |= label.contains("Esc close") && !label.contains("saved to");
        seen_close_only |= label.contains("Esc close") && !label.contains("choose");
        if !label.contains("saved to") && label.contains("choose") {
            assert!(
                label.contains("Esc close"),
                "path goes first, w={w}: {label}"
            );
        }
    }
    assert!(seen_full && seen_no_path && seen_close_only);
}

#[test]
fn every_screen_size_draws_whole_borders_on_both_tabs() {
    for (w, h) in [(120u16, 30u16), (80, 24), (60, 20), (40, 12), (30, 10)] {
        for tab in [HelpTab::Keys, HelpTab::Settings] {
            let mut app = app();
            app.toggle_help();
            app.settings.tab = tab;
            app.settings.config_path = Some("/srv/u/.config/mandible/config.toml".into());
            let buf = draw(&app, w, h);
            let p = popup(&app, w, h);
            assert_eq!(buf[(p.x, p.y)].symbol(), "╭", "{w}x{h} {tab:?}");
            assert_eq!(buf[(p.right() - 1, p.y)].symbol(), "╮", "{w}x{h} {tab:?}");
            assert_eq!(buf[(p.x, p.bottom() - 1)].symbol(), "╰", "{w}x{h} {tab:?}");
            assert_eq!(
                buf[(p.right() - 1, p.bottom() - 1)].symbol(),
                "╯",
                "{w}x{h} {tab:?}"
            );
            for y in p.y + 1..p.bottom() - 1 {
                assert_eq!(buf[(p.x, y)].symbol(), "│", "{w}x{h} {tab:?} y={y}");
                assert_eq!(
                    buf[(p.right() - 1, y)].symbol(),
                    "│",
                    "{w}x{h} {tab:?} y={y}"
                );
            }
        }
    }
}

#[test]
fn the_highlight_stays_in_view_on_a_short_screen() {
    let (w, h) = (60u16, 14u16);
    let mut app = settings_open(w, h);
    for step in 0..Row::ALL.len() {
        select(&mut app, Row::ALL[step]);
        let buf = draw(&app, w, h);
        let p = popup(&app, w, h);
        let barred = (p.y..p.bottom())
            .any(|y| buf[(p.x + 3, y)].style().bg == Some(Color::Indexed(style::BAR_BG)));
        assert!(
            barred,
            "row {step} is not on screen:\n{}",
            lines(&buf).join("\n")
        );
    }
    for step in (0..Row::ALL.len()).rev() {
        select(&mut app, Row::ALL[step]);
        let buf = draw(&app, w, h);
        let p = popup(&app, w, h);
        assert!(
            (p.y..p.bottom())
                .any(|y| buf[(p.x + 3, y)].style().bg == Some(Color::Indexed(style::BAR_BG))),
            "row {step} going up"
        );
    }
}

#[test]
fn the_first_row_of_a_group_brings_its_heading_along() {
    let (w, h) = (60u16, 14u16);
    let mut app = settings_open(w, h);
    select(&mut app, Row::Theme);
    let _ = draw(&app, w, h);
    select(&mut app, Row::Commands);
    let _ = draw(&app, w, h);
    select(&mut app, Row::Flags);
    let buf = draw(&app, w, h);
    assert!(find(&buf, "SORT").is_some(), "{}", lines(&buf).join("\n"));
}

#[test]
fn the_settings_tab_is_ascii_with_the_ascii_glyphs_and_so_are_its_borders() {
    let mut app = app();
    app.glyphs = glyphs::ASCII;
    app.toggle_help();
    app.settings.tab = HelpTab::Settings;
    app.settings.config_path = Some("/srv/u/.config/mandible/config.toml".into());
    select(&mut app, Row::Flags);
    let buf = draw(&app, 120, 30);
    let p = popup(&app, 120, 30);
    for y in p.y..p.bottom() {
        for x in p.x..p.right() {
            assert!(
                buf[(x, y)].symbol().is_ascii(),
                "{:?} at ({x},{y})",
                buf[(x, y)].symbol()
            );
        }
    }
    let text = lines(&buf).join("\n");
    assert!(text.contains("< tool order >"), "{text}");
    assert!(text.contains("Open in the tool's own --help"));
    assert!(text.contains("up/dn choose | left/right change | Tab keys | Esc close"));
}

#[test]
fn without_colour_nothing_in_the_popup_is_coloured() {
    let mut app = settings_open(120, 30);
    app.set_palette(Palette::plain());
    let buf = draw(&app, 120, 30);
    let p = popup(&app, 120, 30);
    for y in p.y..p.bottom() {
        for x in p.x..p.right() {
            let cell = &buf[(x, y)];
            assert_eq!(
                cell.style().fg.unwrap_or(Color::Reset),
                Color::Reset,
                "({x},{y})"
            );
            assert_eq!(
                cell.style().bg.unwrap_or(Color::Reset),
                Color::Reset,
                "({x},{y})"
            );
        }
    }
}

fn status_row(app: &App, w: u16, h: u16) -> (String, Buffer) {
    let buf = draw(app, w, h);
    (line(&buf, h - 1), buf)
}

#[test]
fn a_startup_notice_replaces_the_hints_in_the_warning_style_with_margins() {
    let mut app = app();
    app.notice = Some("config.toml line 7: unknown key \"srot\", ignored".to_string());
    let (text, buf) = status_row(&app, 100, 24);
    assert_eq!(
        text.trim_end(),
        "  config.toml line 7: unknown key \"srot\", ignored"
    );
    assert!(!text.contains("move"), "the hints are gone: {text}");
    assert_eq!(buf[(2, 23)].style().fg, Some(Color::Yellow));
    assert_eq!(buf[(0, 23)].symbol(), " ");
    assert_eq!(buf[(1, 23)].symbol(), " ");
}

#[test]
fn several_problems_name_the_count_and_the_first() {
    let mut app = app();
    app.notice = Some("2 problems in config.toml: line 7: unknown key \"srot\", ignored".into());
    let (text, _) = status_row(&app, 100, 24);
    assert!(text.contains("2 problems in config.toml: line 7"), "{text}");
}

#[test]
fn a_narrow_notice_is_cut_inside_the_margins_on_both_sides() {
    let mut app = app();
    app.notice = Some("config.toml line 7: unknown key \"srot\", ignored".to_string());
    for w in [30u16, 40, 50, 60] {
        let (text, buf) = status_row(&app, w, 12);
        assert!(text.chars().count() <= usize::from(w));
        for x in [0, 1, w - 2, w - 1] {
            assert_eq!(buf[(x, 11)].symbol(), " ", "w={w} x={x}: {text:?}");
        }
        assert!(text.contains("config.toml"), "w={w}: {text:?}");
    }
    let (text, _) = status_row(&app, 30, 12);
    assert!(text.contains('…'), "cut with the ellipsis: {text:?}");
    app.glyphs = glyphs::ASCII;
    let (text, _) = status_row(&app, 30, 12);
    assert!(text.is_ascii(), "{text:?}");
}

#[test]
fn the_hints_come_back_after_the_first_key() {
    let mut app = app();
    app.notice = Some("config.toml line 7: unknown key \"srot\", ignored".to_string());
    handle_key(&mut app, key(KeyCode::Down));
    let (text, _) = status_row(&app, 100, 24);
    assert!(text.contains("move") && !text.contains("srot"), "{text}");
}

#[test]
fn a_notice_is_plain_bold_without_colour() {
    let mut app = app();
    app.set_palette(Palette::plain());
    app.notice = Some("could not save config.toml: Permission denied".to_string());
    let (_, buf) = status_row(&app, 100, 24);
    assert!(buf[(2, 23)].modifier.contains(Modifier::BOLD));
    assert_eq!(
        buf[(2, 23)].style().fg.unwrap_or(Color::Reset),
        Color::Reset
    );
}

/// A node whose description is long enough to preview in a short pane.
fn long_description() -> CommandNode {
    let mut node = CommandNode::new("tool", Provenance::single(Source::HelpText));
    node.children_filled = true;
    let rows: Vec<String> = (1..=30)
        .map(|i| format!("row {i:02} of the text"))
        .collect();
    node.description = Some(Text::sanitize_preserving_layout_lines(
        rows.iter().map(String::as_str),
    ));
    node
}

fn fresh_text(app: &App) -> String {
    let mut fresh = App::new("tool".to_string(), long_description());
    fresh.focus = Focus::Detail;
    fresh.glyphs = app.glyphs;
    fresh.palette = app.palette;
    fresh.color_enabled = app.color_enabled;
    fresh.settings.description_preview = app.settings.description_preview;
    lines(&draw(&fresh, 100, 24)).join("\n")
}

#[test]
fn the_description_preview_setting_redraws_the_page() {
    let mut app = App::new("tool".to_string(), long_description());
    app.focus = Focus::Detail;
    app.glyphs = glyphs::UNICODE;
    let before = lines(&draw(&app, 100, 24)).join("\n");
    assert!(before.contains("DESCRIPTION · preview"), "{before}");
    app.settings.row = Row::ALL.iter().position(|r| *r == Row::Preview).unwrap();
    app.settings_change(1);
    let after = lines(&draw(&app, 100, 24)).join("\n");
    assert!(!after.contains("preview"), "{after}");
    assert!(after.contains("DESCRIPTION"), "{after}");
    assert_eq!(after, fresh_text(&app), "a cached page was served");
    app.settings_change(1);
    assert_eq!(lines(&draw(&app, 100, 24)).join("\n"), before);
}

#[test]
fn the_theme_and_the_glyph_set_redraw_the_page() {
    let mut app = App::new("tool".to_string(), long_description());
    app.focus = Focus::Detail;
    app.set_palette(Palette::extended());
    app.glyphs = glyphs::UNICODE;
    app.settings.glyphs_base = glyphs::UNICODE;
    let coloured = draw(&app, 100, 24);
    app.settings.row = Row::ALL.iter().position(|r| *r == Row::Theme).unwrap();
    app.settings_change(-1);
    let plain = draw(&app, 100, 24);
    assert_ne!(coloured, plain, "the colours changed at once");
    for y in 0..24 {
        for x in 0..100 {
            assert_eq!(
                plain[(x, y)].style().fg.unwrap_or(Color::Reset),
                Color::Reset
            );
        }
    }
    app.settings.row = Row::ALL.iter().position(|r| *r == Row::Ascii).unwrap();
    app.settings_change(1);
    let ascii = draw(&app, 100, 24);
    assert!(
        lines(&ascii).join("\n").contains("DESCRIPTION | preview"),
        "{}",
        lines(&ascii).join("\n")
    );
}
