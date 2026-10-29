//! The colour rows of the settings tab: which rows are enabled, how they
//! step and are typed into, and what each change applies and saves
//! (docs/design.md §9.5).

use super::search_tests::{cmd, prov};
use super::*;
use crate::event::handle_key;
use crate::glyphs;
use crate::settings::{EnvLocks, HelpTab, Row};
use crate::style::Palette;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mandible_core::config::{ColorIndex, Config, Role, Roles, Theme};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn app() -> App {
    let mut root = cmd("tool");
    root.subcommands = vec![cmd("zap")];
    let _ = prov();
    let mut app = App::new("tool".to_string(), root);
    app.set_palette(Palette::extended());
    app.glyphs = glyphs::UNICODE;
    app.settings.glyphs_base = glyphs::UNICODE;
    app
}

fn with_file(contents: &str) -> (tempfile::TempDir, std::path::PathBuf, App) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    if !contents.is_empty() {
        std::fs::write(&path, contents).unwrap();
    }
    let mut app = app();
    app.settings.config_path = Some(path.clone());
    (dir, path, app)
}

fn colour(role: Role) -> Row {
    Row::Color(role)
}

fn go(app: &mut App, row: Row) {
    app.settings.row = Row::ALL.iter().position(|r| *r == row).unwrap();
}

fn step(app: &mut App, row: Row, dir: isize) {
    go(app, row);
    app.settings_change(dir);
}

fn to_custom(app: &mut App) {
    step(app, Row::Theme, -2);
    assert_eq!(app.settings.theme, Theme::Custom);
}

fn type_text(app: &mut App, text: &str) {
    app.settings_begin_typing();
    for c in text.chars() {
        app.settings_typing_key(KeyCode::Char(c));
    }
}

fn shown(app: &App, role: Role) -> String {
    crate::settings::value_text(app, colour(role))
}

#[test]
fn the_colour_rows_follow_the_theme_row_in_the_documented_order() {
    let at = Row::ALL.iter().position(|r| *r == Row::Theme).unwrap();
    let labels: Vec<_> = Row::ALL[at + 1..]
        .iter()
        .map(|r| r.label(glyphs::UNICODE))
        .collect();
    assert_eq!(
        labels,
        [
            "  Accent",
            "  Warning",
            "  Muted text",
            "  Section heading",
            "  Group heading",
            "  Selection",
            "  Selection text"
        ]
    );
    assert_eq!(Row::ALL.len(), at + 8);
}

#[test]
fn another_theme_shows_its_own_values_on_disabled_rows() {
    let mut a = app();
    let want = ["cyan", "yellow", "bright-black", "246", "240", "237", "252"];
    for (role, text) in Role::ALL.iter().zip(want) {
        assert_eq!(shown(&a, *role), text);
        assert!(!a.settings.enabled(colour(*role)));
    }
    step(&mut a, Row::Theme, 1);
    assert_eq!(a.settings.theme, Theme::Light);
    assert_eq!(shown(&a, Role::Accent), "25");
    assert_eq!(shown(&a, Role::SelectionText), "232");
}

#[test]
fn disabled_rows_ignore_the_arrows_and_enter() {
    let mut a = app();
    go(&mut a, colour(Role::Accent));
    assert_eq!(a.settings_change(1), None);
    a.settings_begin_typing();
    assert!(a.settings.typing.is_none());
    assert_eq!(a.settings.custom, Roles::DEFAULT);
    assert_eq!(a.palette, Palette::extended());
}

#[test]
fn up_and_down_skip_the_disabled_rows_and_reach_them_under_custom() {
    let mut a = app();
    go(&mut a, Row::Commands);
    a.settings_move(1);
    assert_eq!(a.settings.selected(), Row::Theme);
    a.settings_move(1);
    assert_eq!(a.settings.selected(), Row::Theme, "nothing enabled below");
    to_custom(&mut a);
    go(&mut a, Row::Theme);
    a.settings_move(1);
    assert_eq!(a.settings.selected(), colour(Role::Accent));
    for _ in 0..20 {
        a.settings_move(1);
    }
    assert_eq!(a.settings.selected(), colour(Role::SelectionText));
    a.settings_move(-1);
    assert_eq!(a.settings.selected(), colour(Role::Selection));
}

#[test]
fn going_back_up_from_below_skips_nothing_enabled() {
    let mut a = app();
    to_custom(&mut a);
    go(&mut a, colour(Role::Accent));
    a.settings_move(-1);
    assert_eq!(a.settings.selected(), Row::Theme);
}

#[test]
fn custom_starts_from_the_default_values_and_keeps_its_own_across_themes() {
    let mut a = app();
    to_custom(&mut a);
    assert_eq!(a.settings.shown_roles(), Roles::DEFAULT);
    step(&mut a, colour(Role::Accent), 1);
    step(&mut a, colour(Role::Warning), -1);
    let mine = a.settings.custom;
    assert_eq!(mine.accent, ColorIndex(7));
    assert_eq!(mine.warning, ColorIndex(2));
    step(&mut a, Row::Theme, 1);
    assert_eq!(a.settings.theme, Theme::None);
    step(&mut a, Row::Theme, 1);
    step(&mut a, Row::Theme, 1);
    assert_eq!(a.settings.theme, Theme::Light);
    assert_eq!(a.settings.custom, mine, "scrolling never touches custom");
    assert_eq!(a.palette.roles, Roles::LIGHT);
    for _ in 0..3 {
        step(&mut a, Row::Theme, -1);
    }
    assert_eq!(a.settings.theme, Theme::Custom);
    assert_eq!(a.palette.roles, mine);
}

#[test]
fn the_values_step_by_name_then_number_and_wrap_at_both_ends() {
    let mut a = app();
    to_custom(&mut a);
    let row = colour(Role::Muted);
    go(&mut a, row);
    let mut seen = vec![shown(&a, Role::Muted)];
    for _ in 0..9 {
        a.settings_change(1);
        seen.push(shown(&a, Role::Muted));
    }
    assert_eq!(
        seen,
        [
            "bright-black",
            "bright-red",
            "bright-green",
            "bright-yellow",
            "bright-blue",
            "bright-magenta",
            "bright-cyan",
            "bright-white",
            "16",
            "17"
        ]
    );
    a.settings_change(-1);
    a.settings_change(-1);
    assert_eq!(shown(&a, Role::Muted), "bright-white");
    for _ in 0..15 {
        a.settings_change(-1);
    }
    assert_eq!(shown(&a, Role::Muted), "black");
    a.settings_change(-1);
    assert_eq!(shown(&a, Role::Muted), "255");
    a.settings_change(1);
    assert_eq!(shown(&a, Role::Muted), "black", "255 wraps to 0");
}

#[test]
fn a_change_recolours_at_once() {
    let mut a = app();
    to_custom(&mut a);
    step(&mut a, colour(Role::Accent), 1);
    assert_eq!(
        crate::style::accent(a.palette).fg,
        Some(ratatui::style::Color::Gray)
    );
    step(&mut a, colour(Role::Selection), 1);
    assert_eq!(a.palette.roles.selection, ColorIndex(238));
}

#[test]
fn typing_a_number_or_a_name_sets_the_value_and_enter_ends_typing() {
    let mut a = app();
    to_custom(&mut a);
    go(&mut a, colour(Role::Warning));
    type_text(&mut a, "220");
    assert_eq!(a.settings.typed_color(), Some(ColorIndex(220)));
    assert_eq!(
        a.settings.custom.warning,
        ColorIndex(3),
        "set only on Enter"
    );
    a.settings_typing_key(KeyCode::Enter);
    assert!(a.settings.typing.is_none());
    assert_eq!(a.settings.custom.warning, ColorIndex(220));
    assert_eq!(
        crate::style::warning(a.palette).fg,
        Some(ratatui::style::Color::Indexed(220))
    );
    type_text(&mut a, "bright-magenta");
    a.settings_typing_key(KeyCode::Enter);
    assert_eq!(a.settings.custom.warning, ColorIndex(13));
    type_text(&mut a, "7");
    a.settings_typing_key(KeyCode::Backspace);
    type_text(&mut a, "");
    a.settings_typing_key(KeyCode::Char('9'));
    a.settings_typing_key(KeyCode::Enter);
    assert_eq!(a.settings.custom.warning, ColorIndex(9));
}

#[test]
fn invalid_text_and_enter_change_nothing_and_stay_in_typing() {
    let (_dir, path, mut a) = with_file("");
    to_custom(&mut a);
    let before = std::fs::read_to_string(&path).unwrap();
    go(&mut a, colour(Role::Accent));
    for bad in ["999", "#ff0000", "cyna", ""] {
        a.settings.typing = Some(bad.to_string());
        a.settings_typing_key(KeyCode::Enter);
        assert_eq!(a.settings.typing.as_deref(), Some(bad), "{bad:?}");
        assert_eq!(a.settings.custom, Roles::DEFAULT);
    }
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
}

#[test]
fn escape_gives_up_the_text_and_keeps_the_value() {
    let mut a = app();
    to_custom(&mut a);
    go(&mut a, colour(Role::Accent));
    type_text(&mut a, "220");
    a.settings_typing_key(KeyCode::Esc);
    assert!(a.settings.typing.is_none());
    assert_eq!(a.settings.custom, Roles::DEFAULT);
    a.show_help = true;
    a.settings.tab = HelpTab::Settings;
    type_text(&mut a, "5");
    handle_key(&mut a, key(KeyCode::Esc));
    assert!(
        a.show_help,
        "Esc cancels the typing and does not close the pop-up"
    );
    assert!(a.settings.typing.is_none());
}

#[test]
fn typing_takes_every_key_so_the_pop_up_does_not_react() {
    let mut a = app();
    to_custom(&mut a);
    a.show_help = true;
    a.settings.tab = HelpTab::Settings;
    go(&mut a, colour(Role::Group));
    handle_key(&mut a, key(KeyCode::Enter));
    assert_eq!(a.settings.typing.as_deref(), Some(""));
    for code in [
        KeyCode::Char('?'),
        KeyCode::Tab,
        KeyCode::Up,
        KeyCode::Left,
        KeyCode::Char('q'),
    ] {
        handle_key(&mut a, key(code));
    }
    assert!(a.show_help);
    assert_eq!(a.settings.tab, HelpTab::Settings);
    assert_eq!(a.settings.selected(), colour(Role::Group));
    assert_eq!(a.settings.custom, Roles::DEFAULT);
    assert_eq!(a.settings.typing.as_deref(), Some("?q"));
}

#[test]
fn a_change_saves_only_its_own_key_and_keeps_the_comments() {
    let file =
        "# mine\n[colors]\ntheme = \"custom\" # picked\naccent = 3 # warm\n\n[ui]\nascii = false\n";
    let (_dir, path, mut a) = with_file(file);
    let cfg = mandible_core::config::parse(file).config;
    a.apply_config(&cfg);
    assert_eq!(a.settings.theme, Theme::Custom);
    step(&mut a, colour(Role::Accent), 1);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        file.replace("accent = 3", "accent = \"blue\"")
    );
    go(&mut a, colour(Role::Group));
    type_text(&mut a, "bright-white");
    a.settings_typing_key(KeyCode::Enter);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        file.replace("accent = 3", "accent = \"blue\"").replace(
            "accent = \"blue\" # warm\n",
            "accent = \"blue\" # warm\ngroup = \"bright-white\"\n"
        )
    );
}

#[test]
fn leaving_custom_writes_the_theme_and_no_colour_key() {
    let (_dir, path, mut a) = with_file("");
    step(&mut a, Row::Theme, 1);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "[colors]\ntheme = \"light\"\n"
    );
}

#[test]
fn the_file_gives_custom_its_values_and_a_theme_other_than_custom_ignores_them() {
    let cfg = mandible_core::config::parse(
        "[colors]\ntheme = \"custom\"\naccent = \"red\"\nselection = 20\n",
    )
    .config;
    let mut a = app();
    a.apply_config(&cfg);
    assert_eq!(a.palette.roles.accent, ColorIndex(1));
    assert_eq!(a.palette.roles.selection, ColorIndex(20));
    assert_eq!(a.palette.roles.warning, ColorIndex(3));
    let mut cfg = cfg;
    cfg.colors.theme = Theme::NordStyle;
    let mut b = app();
    b.apply_config(&cfg);
    assert_eq!(b.palette.roles, Roles::NORD);
    assert_eq!(b.settings.custom.accent, ColorIndex(1), "kept for later");
    assert_eq!(Config::default().colors.custom, Roles::DEFAULT);
}

#[test]
fn no_color_locks_the_colour_rows_even_under_custom() {
    let mut a = app();
    a.settings.theme = Theme::Custom;
    a.settings.locks = EnvLocks {
        ascii: false,
        color: Some("NO_COLOR"),
    };
    for role in Role::ALL {
        assert!(!a.settings.enabled(colour(role)));
    }
    go(&mut a, colour(Role::Accent));
    assert_eq!(a.settings_change(1), None);
    a.settings_begin_typing();
    assert!(a.settings.typing.is_none());
    assert_eq!(a.settings.custom, Roles::DEFAULT);
}

#[test]
fn a_session_without_256_colours_keeps_the_theme_and_falls_back_when_drawing() {
    let mut a = app();
    a.set_palette(Palette::basic());
    step(&mut a, Row::Theme, 4);
    assert_eq!(a.settings.theme, Theme::NordStyle);
    assert_eq!(
        crate::style::accent(a.palette).fg,
        Some(ratatui::style::Color::Cyan)
    );
    assert!(a.palette.color && !a.palette.extended);
}

#[test]
fn a_row_an_environment_variable_decides_is_still_reached_by_the_arrows() {
    let mut a = app();
    a.settings.locks = EnvLocks {
        ascii: true,
        color: Some("NO_COLOR"),
    };
    go(&mut a, Row::Preview);
    a.settings_move(1);
    assert_eq!(a.settings.selected(), Row::Ascii);
    a.settings_move(1);
    a.settings_move(1);
    a.settings_move(1);
    assert_eq!(a.settings.selected(), Row::Theme);
    a.settings_move(1);
    assert_eq!(
        a.settings.selected(),
        Row::Theme,
        "the colour rows are skipped"
    );
}
