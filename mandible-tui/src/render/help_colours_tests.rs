//! The seven colour rows as drawn: the value, the block and the sample,
//! and the border label while one is typed into (docs/design.md §9.5).

use crate::app::App;
use crate::event::handle_key;
use crate::glyphs;
use crate::settings::{EnvLocks, HelpTab, Row};
use crate::style::Palette;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mandible_core::config::{ColorIndex, Role, Theme};
use mandible_core::{CommandNode, Entity, Provenance, Source};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};
use ratatui::Terminal;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn app() -> App {
    let mut root = CommandNode::new("tool", Provenance::single(Source::HelpText));
    root.children_filled = true;
    root.entities = vec![Entity::flag_long(
        "alpha",
        Provenance::single(Source::HelpText),
    )];
    let mut app = App::new("tool".to_string(), root);
    app.set_palette(Palette::extended());
    app.glyphs = glyphs::UNICODE;
    app.settings.glyphs_base = glyphs::UNICODE;
    app.toggle_help();
    app.settings.tab = HelpTab::Settings;
    app
}

fn custom() -> App {
    let mut a = app();
    a.settings.theme = Theme::Custom;
    a.apply_theme();
    a
}

fn draw(app: &App, w: u16, h: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
    terminal.draw(|f| crate::render::render(f, app)).unwrap();
    terminal.backend().buffer().clone()
}

fn line(buf: &Buffer, y: u16) -> String {
    (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()
}

/// The screen row of `label` and the column its text starts at.
fn row_of(buf: &Buffer, label: &str) -> (u16, u16) {
    (0..buf.area.height)
        .find_map(|y| {
            let cells: Vec<_> = (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect();
            let text: String = cells.concat();
            let at = text.find(label)?;
            Some((y, u16::try_from(text[..at].chars().count()).unwrap()))
        })
        .unwrap_or_else(|| panic!("no row {label:?}"))
}

fn select(app: &mut App, row: Row) {
    app.settings.row = Row::ALL.iter().position(|r| *r == row).unwrap();
}

/// The column of the first cell on `y` holding `symbol`.
fn col_of(buf: &Buffer, y: u16, symbol: &str) -> Option<u16> {
    (0..buf.area.width).find(|&x| buf[(x, y)].symbol() == symbol)
}

const LABELS: [(Role, &str); 7] = [
    (Role::Accent, "Accent"),
    (Role::Warning, "Warning"),
    (Role::Muted, "Muted text"),
    (Role::Section, "Section heading"),
    (Role::Group, "Group heading"),
    (Role::Selection, "Selection"),
    (Role::SelectionText, "Selection text"),
];

#[test]
fn every_colour_row_shows_value_block_and_sample_whatever_the_theme() {
    let samples = [
        "--squash",
        "config.toml line 7",
        "Record changes to the repository",
        "FLAGS (41) ───",
        "Commit message options ───",
        " commit ",
        " Record changes ",
    ];
    for custom in [false, true] {
        let a = if custom { self::custom() } else { app() };
        let buf = draw(&a, 130, 34);
        for ((role, label), sample) in LABELS.iter().zip(samples) {
            let (y, _) = row_of(&buf, &format!("  {label}"));
            let text = line(&buf, y);
            let value = a.settings.shown_roles().get(*role).shown();
            assert!(text.contains(&value), "{text}");
            let block = text.find("██").unwrap_or_else(|| panic!("{text}"));
            assert!(text[block..].contains(sample), "{label}: {text}");
            assert!(text.find(&value).unwrap() < block);
        }
    }
}

#[test]
fn the_block_and_sample_are_in_the_colour_and_the_value_text_is_not() {
    let a = custom();
    let buf = draw(&a, 130, 34);
    let cases = [
        (Role::Accent, Color::Cyan),
        (Role::Warning, Color::Yellow),
        (Role::Muted, Color::DarkGray),
        (Role::Section, Color::Indexed(246)),
        (Role::Group, Color::Indexed(240)),
    ];
    for (role, color) in cases {
        let label = LABELS.iter().find(|(r, _)| *r == role).unwrap().1;
        let (y, x) = row_of(&buf, &format!("  {label}"));
        let text = line(&buf, y);
        let block = u16::try_from(text[..text.find("██").unwrap()].chars().count()).unwrap();
        assert_eq!(buf[(block, y)].fg, color, "{label} block");
        assert_eq!(buf[(block + 3, y)].fg, color, "{label} sample");
        assert_eq!(
            buf[(x + 20, y)].fg,
            Color::Reset,
            "{label} value text is plain"
        );
    }
}

#[test]
fn the_selection_block_is_the_bar_colour_and_the_samples_sit_on_the_bar() {
    let mut a = custom();
    a.settings.custom.set(Role::Selection, ColorIndex(17));
    a.settings.custom.set(Role::SelectionText, ColorIndex(220));
    a.apply_theme();
    let buf = draw(&a, 130, 34);
    let (y, _) = row_of(&buf, "  Selection  ");
    let text = line(&buf, y);
    let block = u16::try_from(text[..text.find("██").unwrap()].chars().count()).unwrap();
    assert_eq!(buf[(block, y)].fg, Color::Indexed(17));
    let name = block + 4;
    assert_eq!(buf[(name, y)].bg, Color::Indexed(17), "{text}");
    assert_eq!(buf[(name, y)].fg, Color::Cyan);
    assert!(buf[(name, y)].modifier.contains(Modifier::BOLD));
    let (y, _) = row_of(&buf, "  Selection text");
    let text = line(&buf, y);
    let block = u16::try_from(text[..text.find("██").unwrap()].chars().count()).unwrap();
    assert_eq!(buf[(block, y)].fg, Color::Indexed(220));
    assert_eq!(buf[(block + 4, y)].fg, Color::Indexed(220));
    assert_eq!(buf[(block + 4, y)].bg, Color::Indexed(17));
}

#[test]
fn another_theme_draws_its_own_values_in_disabled_muted_rows() {
    let mut a = app();
    a.settings.theme = Theme::NordStyle;
    a.apply_theme();
    let buf = draw(&a, 130, 34);
    let (y, x) = row_of(&buf, "  Accent");
    let text = line(&buf, y);
    assert!(text.contains("110"), "{text}");
    assert_eq!(
        buf[(x + 2, y)].fg,
        Color::Indexed(102),
        "the label is in the theme's muted color"
    );
    let block = u16::try_from(text[..text.find("██").unwrap()].chars().count()).unwrap();
    assert_eq!(buf[(block, y)].fg, Color::Indexed(110));
    let (y, _) = row_of(&buf, "  Muted text");
    let text = line(&buf, y);
    let block = u16::try_from(text[..text.find("██").unwrap()].chars().count()).unwrap();
    assert_eq!(buf[(block, y)].fg, Color::Indexed(102));
}

#[test]
fn an_enabled_row_is_plain_and_a_disabled_row_is_muted() {
    let enabled = draw(&custom(), 130, 34);
    let (y, x) = row_of(&enabled, "  Warning");
    assert_eq!(enabled[(x + 2, y)].fg, Color::Reset);
    let disabled = draw(&app(), 130, 34);
    let (y, x) = row_of(&disabled, "  Warning");
    assert_eq!(disabled[(x + 2, y)].fg, Color::DarkGray);
}

#[test]
fn the_highlighted_colour_row_has_arrows_the_bar_and_keeps_its_own_colours() {
    let mut a = custom();
    select(&mut a, Row::Color(Role::Warning));
    let buf = draw(&a, 130, 34);
    let (y, x) = row_of(&buf, "Warning");
    let text = line(&buf, y);
    assert!(text.contains("‹ yellow"), "{text}");
    assert!(text.contains("config.toml line 7 ›"), "{text}");
    let block = u16::try_from(text[..text.find("██").unwrap()].chars().count()).unwrap();
    assert_eq!(buf[(block, y)].fg, Color::Yellow);
    assert_eq!(buf[(block, y)].bg, Color::Indexed(237), "block on the bar");
    assert_eq!(buf[(x + 2, y)].bg, Color::Indexed(237));
    assert_eq!(buf[(x + 2, y)].fg, Color::Indexed(252));
}

#[test]
fn the_block_column_does_not_move_when_the_highlight_does() {
    let mut a = custom();
    let first = {
        let buf = draw(&a, 130, 34);
        let (y, _) = row_of(&buf, "  Accent");
        col_of(&buf, y, "█").unwrap()
    };
    select(&mut a, Row::Color(Role::Accent));
    let buf = draw(&a, 130, 34);
    let (y, _) = row_of(&buf, "Accent");
    assert_eq!(col_of(&buf, y, "█").unwrap(), first);
}

#[test]
fn the_ascii_glyph_set_uses_hash_signs_for_the_block() {
    let mut a = custom();
    a.glyphs = glyphs::ASCII;
    let buf = draw(&a, 130, 34);
    let (y, _) = row_of(&buf, "  Accent");
    let text = line(&buf, y);
    assert!(text.contains(" ## --squash"), "{text}");
    let (y, _) = row_of(&buf, "  Section heading");
    assert!(line(&buf, y).contains("FLAGS (41) ---"));
    for y in 0..34 {
        assert!(line(&buf, y).is_ascii(), "{}", line(&buf, y));
    }
}

#[test]
fn a_narrow_pop_up_cuts_the_sample_first_then_the_block_and_never_half_of_either() {
    let a = custom();
    let mut seen_cut_sample = false;
    let mut seen_cut_block = false;
    for w in (40..=130).rev() {
        let buf = draw(&a, w, 34);
        let (y, _) = row_of(&buf, "  Accent");
        let p = crate::render::help_overlay_rect(Rect::new(0, 0, w, 34), false);
        let inside: String = line(&buf, y)
            .chars()
            .skip(usize::from(p.x) + 1)
            .take(usize::from(p.width) - 2)
            .collect();
        let blocks = inside.matches('█').count();
        assert!(blocks == 0 || blocks == 2, "half a block at {w}: {inside}");
        let after = inside.split("██").nth(1).unwrap_or_default().trim();
        assert!(
            after.is_empty() || after == "--squash",
            "half a sample at {w}: {inside}"
        );
        seen_cut_sample |= blocks == 2 && after.is_empty();
        seen_cut_block |= blocks == 0;
        assert!(
            blocks == 2 || !inside.contains("--"),
            "sample without block at {w}"
        );
    }
    assert!(seen_cut_sample && seen_cut_block);
}

#[test]
fn rows_keep_a_margin_on_both_edges_when_highlighted() {
    let mut a = custom();
    select(&mut a, Row::Color(Role::Muted));
    for w in [60, 80, 100, 130] {
        let buf = draw(&a, w, 34);
        let (y, _) = row_of(&buf, "Muted text");
        let chars: Vec<_> = line(&buf, y).chars().collect();
        let p = crate::render::help_overlay_rect(Rect::new(0, 0, w, 34), false);
        let left = usize::from(p.x);
        let right = usize::from(p.right()) - 1;
        assert_eq!(chars[left + 1], ' ', "{w}");
        assert_eq!(chars[left + 2], ' ', "{w}");
        assert_eq!(chars[right - 1], ' ', "{w}");
        assert_eq!(chars[right - 2], ' ', "{w}");
    }
}

#[test]
fn typing_shows_a_prompt_and_cursor_and_the_sample_follows_a_valid_colour() {
    let mut a = custom();
    select(&mut a, Row::Color(Role::Accent));
    handle_key(&mut a, key(KeyCode::Enter));
    for c in "17".chars() {
        handle_key(&mut a, key(KeyCode::Char(c)));
    }
    let buf = draw(&a, 130, 34);
    let (y, _) = row_of(&buf, "Accent");
    let text = line(&buf, y);
    assert!(text.contains("› 17▏"), "{text}");
    assert!(!text.contains('‹'), "{text}");
    let block = u16::try_from(text[..text.find("██").unwrap()].chars().count()).unwrap();
    assert_eq!(buf[(block, y)].fg, Color::Indexed(17));
    handle_key(&mut a, key(KeyCode::Char('x')));
    let buf = draw(&a, 130, 34);
    let (y, _) = row_of(&buf, "Accent");
    let text = line(&buf, y);
    let block = u16::try_from(text[..text.find("██").unwrap()].chars().count()).unwrap();
    assert_eq!(
        buf[(block, y)].fg,
        Color::Cyan,
        "invalid text keeps the current colour"
    );
    assert!(text.contains("› 17x▏"), "{text}");
}

#[test]
fn the_ascii_cursor_is_an_underscore() {
    let mut a = custom();
    a.glyphs = glyphs::ASCII;
    select(&mut a, Row::Color(Role::Accent));
    handle_key(&mut a, key(KeyCode::Enter));
    handle_key(&mut a, key(KeyCode::Char('5')));
    let buf = draw(&a, 130, 34);
    let (y, _) = row_of(&buf, "Accent");
    assert!(line(&buf, y).contains("> 5_"), "{}", line(&buf, y));
}

fn bottom(a: &App, buf: &Buffer, w: u16, h: u16) -> String {
    let p = crate::render::help_overlay_rect(Rect::new(0, 0, w, h), a.print_selection);
    line(buf, p.bottom() - 1)
}

#[test]
fn the_border_label_names_the_keys_for_the_row_and_for_typing() {
    let mut a = custom();
    select(&mut a, Row::Color(Role::Accent));
    let buf = draw(&a, 130, 34);
    assert!(
        bottom(&a, &buf, 130, 34)
            .contains("↑↓ choose · ←→ change · Enter type · Tab keys · Esc close"),
        "{}",
        bottom(&a, &buf, 130, 34)
    );
    handle_key(&mut a, key(KeyCode::Enter));
    let buf = draw(&a, 130, 34);
    let label = bottom(&a, &buf, 130, 34);
    assert!(label.contains("Enter set · Esc cancel"), "{label}");
    assert!(!label.contains("Tab"), "{label}");
    select(&mut a, Row::Theme);
    a.settings.typing = None;
    let buf = draw(&a, 130, 34);
    assert!(!bottom(&a, &buf, 130, 34).contains("Enter type"));
}

#[test]
fn no_color_mutes_the_rows_and_draws_no_block_or_sample() {
    let mut a = custom();
    a.settings.locks = EnvLocks {
        ascii: false,
        color: Some("NO_COLOR"),
    };
    a.set_palette(Palette::plain());
    let buf = draw(&a, 130, 34);
    let (y, _) = row_of(&buf, "  Accent");
    let text = line(&buf, y);
    assert!(text.contains("cyan"), "{text}");
    assert!(!text.contains("██") && !text.contains("--squash"), "{text}");
    let (ty, _) = row_of(&buf, "Theme");
    assert!(line(&buf, ty).contains("none (NO_COLOR)"));
    for y in 0..34 {
        for x in 0..130 {
            assert_eq!(buf[(x, y)].fg, Color::Reset, "({x},{y})");
        }
    }
}

#[test]
fn the_none_theme_draws_the_rows_plain() {
    let mut a = app();
    a.settings.theme = Theme::None;
    a.apply_theme();
    let buf = draw(&a, 130, 34);
    let (y, _) = row_of(&buf, "  Accent");
    assert!(!line(&buf, y).contains("██"));
}

#[test]
fn at_sixteen_colours_a_number_role_samples_in_the_fallback_colour() {
    let mut a = app();
    a.set_palette(Palette::basic());
    a.settings.theme = Theme::NordStyle;
    a.apply_theme();
    let buf = draw(&a, 130, 34);
    let (y, _) = row_of(&buf, "  Accent");
    let text = line(&buf, y);
    let block = u16::try_from(text[..text.find("██").unwrap()].chars().count()).unwrap();
    assert_eq!(buf[(block, y)].fg, Color::Cyan);
}

#[test]
fn the_pop_up_is_tall_enough_for_every_row_of_the_tab() {
    let a = app();
    let buf = draw(&a, 120, 30);
    let (y, _) = row_of(&buf, "  Selection text");
    assert!(y < 30);
    let rows: Vec<String> = (0..30).map(|y| line(&buf, y)).collect();
    assert!(
        !rows.iter().any(|l| l.contains('▼') && l.contains('╰')),
        "no scroll marker"
    );
}
