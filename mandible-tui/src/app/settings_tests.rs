//! The settings tab: what each setting applies, what it saves, and the
//! order of the environment, the file and the defaults (docs/design.md §9.5).

use super::search_tests::{cmd, prov};
use super::*;
use crate::event::handle_key;
use crate::glyphs;
use crate::settings::{HelpTab, Row};
use crate::sort::DetailSort;
use crate::style::Palette;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mandible_core::config::{CommandSort, Config, FlagSort, Theme};
use mandible_core::Entity;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn tree() -> CommandNode {
    let mut page = cmd("page");
    page.entities = vec![
        Entity::flag_long("zeta", prov()),
        Entity::flag_long("alpha", prov()),
    ];
    let mut root = cmd("tool");
    root.subcommands = vec![cmd("zap"), cmd("alpha"), page];
    root
}

fn app() -> App {
    let mut app = App::new("tool".to_string(), tree());
    app.set_palette(Palette::extended());
    app.glyphs = glyphs::UNICODE;
    app.settings.glyphs_base = glyphs::UNICODE;
    app
}

fn names(app: &App) -> Vec<String> {
    app.rows().iter().map(|r| r.name.clone()).collect()
}

fn on_row(app: &mut App, row: Row) {
    app.settings.row = Row::ALL.iter().position(|r| *r == row).unwrap();
}

fn change(app: &mut App, row: Row, dir: isize) -> Option<Effect> {
    on_row(app, row);
    app.settings_change(dir)
}

fn with_file(contents: Option<&str>) -> (tempfile::TempDir, std::path::PathBuf, App) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mandible").join("config.toml");
    if let Some(text) = contents {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
    }
    let mut app = app();
    app.settings.config_path = Some(path.clone());
    (dir, path, app)
}

#[test]
fn a_fresh_app_has_every_default() {
    let app = app();
    assert!(!app.raw_mode && app.parsed_wrap && !app.horizontal_scroll_enabled);
    assert!(app.settings.description_preview && !app.settings.ascii);
    assert_eq!(app.settings.theme, Theme::Default);
    assert_eq!(app.sorts.detail, DetailSort::Printed);
    assert!(!app.sorts.tree);
    assert_eq!(app.settings.tab, HelpTab::Keys);
}

#[test]
fn the_default_config_changes_nothing() {
    let mut a = app();
    a.apply_config(&Config::default());
    assert!(!a.raw_mode && a.parsed_wrap && !a.horizontal_scroll_enabled);
    assert!(a.settings.description_preview && !a.settings.ascii);
    assert_eq!(a.palette, Palette::extended());
    assert_eq!(a.glyphs, glyphs::UNICODE);
    assert_eq!(names(&a), ["tool", "zap", "alpha", "page"]);
}

#[test]
fn every_key_of_the_file_applies_at_startup() {
    let mut cfg = Config::default();
    cfg.ui.raw_view = true;
    cfg.ui.horizontal_scroll = true;
    cfg.ui.description_preview = false;
    cfg.ui.ascii = true;
    cfg.sort.flags = FlagSort::AzSections;
    cfg.sort.commands = CommandSort::Az;
    cfg.colors.theme = Theme::None;
    let mut a = app();
    a.apply_config(&cfg);
    assert!(a.raw_mode);
    assert!(a.horizontal_scroll_enabled && !a.parsed_wrap, "both views");
    assert!(!a.settings.description_preview);
    assert_eq!(a.glyphs, glyphs::ASCII);
    assert_eq!(a.sorts.detail, DetailSort::Sections);
    assert_eq!(names(&a), ["tool", "alpha", "page", "zap"]);
    assert!(!a.color_enabled && !a.palette.color);
}

#[test]
fn no_color_in_the_environment_beats_the_file() {
    // The environment decides the base palette; the file's theme can only
    // lower it further.
    let mut a = app();
    a.set_palette(Palette::plain());
    let mut cfg = Config::default();
    cfg.colors.theme = Theme::Default;
    a.apply_config(&cfg);
    assert!(!a.palette.color, "theme = default cannot bring colour back");
    assert!(!a.color_enabled);
}

#[test]
fn mandible_ascii_in_the_environment_beats_the_file() {
    let mut a = app();
    a.settings.glyphs_base = glyphs::ASCII;
    a.glyphs = glyphs::ASCII;
    let mut cfg = Config::default();
    cfg.ui.ascii = false;
    a.apply_config(&cfg);
    assert_eq!(
        a.glyphs,
        glyphs::ASCII,
        "ascii = false cannot undo the variable"
    );
}

#[test]
fn the_file_can_force_ascii_and_turning_it_off_restores_the_detected_set() {
    let mut a = app();
    let mut cfg = Config::default();
    cfg.ui.ascii = true;
    a.apply_config(&cfg);
    assert_eq!(a.glyphs, glyphs::ASCII);
    change(&mut a, Row::Ascii, 1);
    assert_eq!(a.glyphs, glyphs::UNICODE);
}

#[test]
fn opening_the_popup_shows_the_keys_tab_and_up_down_stop_at_the_ends() {
    let mut a = app();
    a.settings.tab = HelpTab::Settings;
    a.toggle_help();
    assert_eq!(a.settings.tab, HelpTab::Keys);
    a.settings_move(-1);
    assert_eq!(a.settings.selected(), Row::RawView);
    for _ in 0..20 {
        a.settings_move(1);
    }
    assert_eq!(a.settings.selected(), Row::Theme);
    assert_eq!(a.settings.row, Row::ALL.len() - 1, "no hidden overshoot");
    a.settings_move(-1);
    assert_eq!(a.settings.selected(), Row::Commands);
}

#[test]
fn left_and_right_wrap_around_the_values() {
    let mut a = app();
    let seen: Vec<_> = (0..4)
        .map(|_| {
            change(&mut a, Row::Flags, 1);
            a.sorts.detail
        })
        .collect();
    assert_eq!(
        seen,
        [
            DetailSort::Sections,
            DetailSort::Overall,
            DetailSort::Printed,
            DetailSort::Sections
        ]
    );
    change(&mut a, Row::Flags, -1);
    assert_eq!(a.sorts.detail, DetailSort::Printed);
    change(&mut a, Row::Flags, -1);
    assert_eq!(
        a.sorts.detail,
        DetailSort::Overall,
        "left from the first wraps"
    );
    change(&mut a, Row::Commands, -1);
    assert!(a.sorts.tree);
    change(&mut a, Row::Commands, 1);
    assert!(!a.sorts.tree);
    change(&mut a, Row::Theme, 1);
    assert_eq!(a.settings.theme, Theme::None);
    change(&mut a, Row::Theme, 1);
    assert_eq!(a.settings.theme, Theme::Default);
}

#[test]
fn a_row_an_environment_variable_decides_does_nothing() {
    let (_dir, path, mut a) = with_file(None);
    a.settings.locks = crate::settings::EnvLocks {
        ascii: true,
        color: Some("NO_COLOR"),
    };
    change(&mut a, Row::Ascii, 1);
    change(&mut a, Row::Theme, 1);
    assert!(!a.settings.ascii);
    assert_eq!(a.settings.theme, Theme::Default);
    assert_eq!(a.glyphs, glyphs::UNICODE);
    assert!(!path.exists(), "nothing was saved");
    change(&mut a, Row::Preview, 1);
    assert!(!a.settings.description_preview, "other rows still change");
}

#[test]
fn raw_view_switches_the_view_and_asks_for_the_text() {
    let mut a = app();
    let effect = change(&mut a, Row::RawView, 1);
    assert!(a.raw_mode);
    assert!(matches!(effect, Some(Effect::FetchRaw(_))));
    change(&mut a, Row::RawView, -1);
    assert!(!a.raw_mode);
}

#[test]
fn scroll_sets_both_views_and_returns_to_column_zero() {
    let mut a = app();
    change(&mut a, Row::Scroll, 1);
    assert!(a.horizontal_scroll_enabled && !a.parsed_wrap);
    assert!(a.sideways());
    change(&mut a, Row::Scroll, 1);
    assert!(!a.horizontal_scroll_enabled && a.parsed_wrap);
}

#[test]
fn flags_sort_resorts_the_page_and_the_commands_sort_resorts_the_tree() {
    let mut a = app();
    change(&mut a, Row::Commands, 1);
    assert_eq!(names(&a), ["tool", "alpha", "page", "zap"]);
    assert_eq!(a.selected_path(), Some(vec!["tool".to_string()]));
    change(&mut a, Row::Flags, 1);
    assert_eq!(a.sorts.detail, DetailSort::Sections);
    // `s` continues from the state the setting left.
    a.focus = Focus::Detail;
    a.sort_focused();
    assert_eq!(a.sorts.detail, DetailSort::Overall);
}

#[test]
fn the_theme_recolours_at_once_and_back() {
    let mut a = app();
    change(&mut a, Row::Theme, 1);
    assert_eq!(a.palette, Palette::plain());
    assert!(!a.color_enabled);
    change(&mut a, Row::Theme, 1);
    assert_eq!(a.palette, Palette::extended());
    assert!(a.color_enabled);
}

const FIXTURE: &str = "# my settings\n[ui]\nraw_view = false # start parsed\nfuture_key = 7\n\n# sorting\n[sort]\nflags = \"tool\"\n\n[other]\nkeep = \"me\"\n";

#[test]
fn a_change_saves_only_its_key_and_keeps_comments_and_unknown_keys() {
    let (_dir, path, mut a) = with_file(Some(FIXTURE));
    change(&mut a, Row::RawView, 1);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        FIXTURE.replace("raw_view = false", "raw_view = true")
    );
    change(&mut a, Row::Flags, 1);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        FIXTURE
            .replace("raw_view = false", "raw_view = true")
            .replace("flags = \"tool\"", "flags = \"az-sections\"")
    );
    assert_eq!(a.notice, None);
}

#[test]
fn every_row_saves_the_key_it_names() {
    let (_dir, path, mut a) = with_file(None);
    change(&mut a, Row::RawView, 1);
    change(&mut a, Row::Scroll, 1);
    change(&mut a, Row::Preview, 1);
    change(&mut a, Row::Ascii, 1);
    change(&mut a, Row::Flags, -1);
    change(&mut a, Row::Commands, 1);
    change(&mut a, Row::Theme, 1);
    let saved = mandible_core::config::parse(&std::fs::read_to_string(&path).unwrap());
    assert!(saved.problems.is_empty(), "{:?}", saved.problems);
    let c = saved.config;
    assert!(c.ui.raw_view && c.ui.horizontal_scroll && c.ui.ascii && !c.ui.description_preview);
    assert_eq!(c.sort.flags, FlagSort::Az);
    assert_eq!(c.sort.commands, CommandSort::Az);
    assert_eq!(c.colors.theme, Theme::None);
}

#[test]
fn the_file_and_its_directory_are_created_on_the_first_change() {
    let (_dir, path, mut a) = with_file(None);
    assert!(!path.parent().unwrap().exists());
    change(&mut a, Row::Preview, 1);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "[ui]\ndescription_preview = false\n"
    );
}

#[test]
fn a_failed_save_leaves_one_footer_line_and_the_setting_applied() {
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, "x").unwrap();
    let mut a = app();
    a.settings.config_path = Some(blocker.join("config.toml"));
    change(&mut a, Row::Theme, 1);
    assert_eq!(a.settings.theme, Theme::None, "the setting still applies");
    assert!(!a.color_enabled);
    let line = a.notice.clone().expect("a footer line");
    assert!(line.starts_with("could not save config.toml: "), "{line}");
    assert!(!line.contains('\n'));
}

#[test]
fn a_file_that_does_not_parse_is_not_overwritten() {
    let (_dir, path, mut a) = with_file(Some("not toml [[["));
    change(&mut a, Row::Preview, 1);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "not toml [[[");
    assert!(a
        .notice
        .as_deref()
        .unwrap()
        .starts_with("could not save config.toml: "));
    assert!(!a.settings.description_preview);
}

#[test]
fn with_no_path_nothing_is_written_and_nothing_is_said() {
    let mut a = app();
    change(&mut a, Row::Preview, 1);
    assert_eq!(a.notice, None);
}

#[test]
fn the_notice_goes_at_the_first_key_press() {
    let mut a = app();
    a.notice = Some("config.toml line 7: unknown key \"srot\", ignored".to_string());
    handle_key(&mut a, key(KeyCode::Down));
    assert_eq!(a.notice, None);
}

#[test]
fn a_failed_save_is_shown_until_the_next_key_press() {
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, "x").unwrap();
    let mut a = app();
    a.settings.config_path = Some(blocker.join("config.toml"));
    a.toggle_help();
    handle_key(&mut a, key(KeyCode::Tab));
    handle_key(&mut a, key(KeyCode::Down));
    handle_key(&mut a, key(KeyCode::Right));
    assert!(a.notice.is_some());
    handle_key(&mut a, key(KeyCode::Down));
    assert_eq!(a.notice, None);
}
