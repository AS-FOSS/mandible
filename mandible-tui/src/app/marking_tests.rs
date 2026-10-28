//! Marking what a search matched: the detail pane's rows, the commands
//! list's rows and counts, `n`/`N`, and the no-match notice margin
//! (spec §10).

use super::search_tests::{cmd, prov, selected_name, settle, type_query};
use super::*;
use mandible_core::{Entity, Text};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::Modifier;
use ratatui::Terminal;

fn flag(name: &str, description: &str) -> Entity {
    let mut e = Entity::flag_long(name, prov());
    e.description = Some(Text::sanitize(description));
    e
}

/// A tool whose `commit` carries flags that match `re` and flags that do
/// not, beside commands that match by name.
fn tool() -> CommandNode {
    let mut commit = cmd("commit");
    commit.summary = Some(Text::sanitize("Record changes to the repository"));
    commit.entities = vec![
        flag("amend", "amend previous commit"),
        flag("reedit-message", "reuse and edit a message"),
        flag("signoff", "add a trailer"),
        flag("reset-author", "the commit is authored by me now"),
    ];
    let mut pull = cmd("pull");
    pull.summary = Some(Text::sanitize("Fetch from and integrate"));
    pull.entities = vec![flag("rebase", "rebase instead of merge")];
    let mut group = cmd("group");
    group.subcommands = vec![cmd("reset")];
    let mut root = cmd("tool");
    root.subcommands = vec![cmd("clone"), commit, group, pull, cmd("restore")];
    root
}

fn searched(query: &str, name: &str) -> App {
    let mut app = App::new("tool".to_string(), tool());
    app.set_palette(crate::style::Palette::extended());
    app.focus_search();
    type_query(&mut app, query);
    app.accept_search();
    if let Some(at) = app.rows().iter().position(|r| r.name == name) {
        app.select_index(at);
        app.selected_flag = None;
    }
    app
}

fn draw(app: &App, cols: u16, rows: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(cols, rows)).unwrap();
    terminal.draw(|f| crate::render::render(f, app)).unwrap();
    terminal.backend().buffer().clone()
}

fn line(buf: &Buffer, y: u16) -> String {
    (0..buf.area.width)
        .map(|x| buf[(x, y)].symbol().to_string())
        .collect()
}

/// The row and column of `text` among the cells left of column `limit`.
fn find_in(buf: &Buffer, text: &str, limit: u16) -> Option<(u16, u16)> {
    let want: Vec<String> = text.chars().map(|c| c.to_string()).collect();
    (0..buf.area.height).find_map(|y| {
        (0..limit.saturating_sub(want.len() as u16)).find_map(|x| {
            want.iter()
                .enumerate()
                .all(|(i, c)| buf[(x + i as u16, y)].symbol() == c)
                .then_some((y, x))
        })
    })
}

fn find(buf: &Buffer, text: &str) -> Option<(u16, u16)> {
    find_in(buf, text, buf.area.width)
}

/// Search the commands list only.
fn find_left(buf: &Buffer, text: &str) -> Option<(u16, u16)> {
    find_in(buf, text, buf.area.width / 2)
}

fn has(buf: &Buffer, x: u16, y: u16, m: Modifier) -> bool {
    buf[(x, y)].style().add_modifier.contains(m)
}

/// The margin column of the detail pane: just inside its left border.
fn gutter(buf: &Buffer) -> u16 {
    find(buf, "tool › commit")
        .or_else(|| find(buf, "tool > commit"))
        .map(|(_, x)| x - 1)
        .unwrap()
}

#[test]
fn matching_rows_get_a_bar_bold_and_underlined_letters() {
    let app = searched("re", "commit");
    let buf = draw(&app, 120, 30);
    let g = gutter(&buf);
    let (y, x) = find(&buf, "reedit-message").unwrap();
    assert_eq!(buf[(g, y)].symbol(), "▌", "{}", line(&buf, y));
    let re_at = x;
    assert!(has(&buf, re_at, y, Modifier::UNDERLINED));
    assert!(has(&buf, re_at, y, Modifier::BOLD));
    assert!(
        !has(&buf, re_at + 2, y, Modifier::UNDERLINED),
        "only the matched letters"
    );
    assert!(
        has(&buf, re_at + 2, y, Modifier::BOLD),
        "the whole spelling is bold"
    );
    let (dy, dx) = find(&buf, "reuse and edit").unwrap();
    assert!(
        has(&buf, dx, dy, Modifier::BOLD),
        "and so is the description"
    );
    let (ay, ax) = find(&buf, "--amend").unwrap();
    assert_eq!(buf[(g, ay)].symbol(), " ");
    assert!(!has(&buf, ax, ay, Modifier::BOLD));
}

#[test]
fn every_matching_row_is_marked_not_only_the_jump_target() {
    let app = searched("re", "commit");
    let buf = draw(&app, 120, 30);
    let g = gutter(&buf);
    let bars = (0..buf.area.height)
        .filter(|&y| buf[(g, y)].symbol() == "▌")
        .count();
    assert_eq!(bars, 2, "reedit-message and reset-author");
}

#[test]
fn no_color_keeps_the_bar_the_underline_and_the_bold() {
    let mut app = searched("re", "commit");
    app.set_palette(crate::style::Palette::plain());
    let buf = draw(&app, 120, 30);
    let g = gutter(&buf);
    let (y, x) = find(&buf, "reedit-message").unwrap();
    assert_eq!(buf[(g, y)].symbol(), "▌");
    assert!(has(&buf, x, y, Modifier::UNDERLINED));
    let (dy, dx) = find(&buf, "reuse and edit").unwrap();
    assert!(has(&buf, dx, dy, Modifier::BOLD));
    let (ay, ax) = find(&buf, "amend previous").unwrap();
    assert!(
        !has(&buf, ax, ay, Modifier::BOLD),
        "unmarked rows stay plain"
    );
}

#[test]
fn the_ascii_glyph_set_has_a_bar() {
    let mut app = searched("re", "commit");
    app.glyphs = crate::glyphs::ASCII;
    let buf = draw(&app, 120, 30);
    let g = gutter(&buf);
    let (y, _) = find(&buf, "reedit-message").unwrap();
    assert_eq!(buf[(g, y)].symbol(), "|");
}

#[test]
fn a_page_without_a_search_is_not_marked() {
    let mut app = App::new("tool".to_string(), tool());
    let at = app.rows().iter().position(|r| r.name == "commit").unwrap();
    app.select_index(at);
    let buf = draw(&app, 120, 30);
    let g = gutter(&buf);
    assert!((0..buf.area.height).all(|y| buf[(g, y)].symbol() != "▌"));
}

#[test]
fn list_rows_that_match_are_bold_and_context_rows_stay_plain() {
    let app = searched("re", "commit");
    let buf = draw(&app, 120, 30);
    let (y, x) = find_left(&buf, "restore").unwrap();
    assert!(has(&buf, x, y, Modifier::BOLD));
    assert!(has(&buf, x, y, Modifier::UNDERLINED));
    let (gy, gx) = find_left(&buf, "group").unwrap();
    assert!(
        !has(&buf, gx, gy, Modifier::BOLD),
        "an ancestor kept for shape"
    );
    let (py, px) = find_left(&buf, "pull").unwrap();
    assert!(has(&buf, px, py, Modifier::BOLD), "matches by an entity");
    assert!(
        !has(&buf, px, py, Modifier::UNDERLINED),
        "its own name did not match"
    );
}

#[test]
fn the_selected_list_row_keeps_its_reversed_style() {
    let app = searched("re", "restore");
    let buf = draw(&app, 120, 30);
    let (y, x) = find_left(&buf, "restore").unwrap();
    assert!(has(&buf, x, y, Modifier::REVERSED));
    assert!(!has(&buf, x, y, Modifier::BOLD));
}

#[test]
fn counts_sit_in_a_fixed_width_right_aligned_column() {
    let app = searched("re", "commit");
    let buf = draw(&app, 120, 30);
    let (cy, cx) = find_left(&buf, "  2 matches").unwrap();
    let (py, px) = find_left(&buf, "  1 match  ").unwrap();
    assert_eq!(cx, px, "both end at the same column");
    assert_ne!(cy, py);
    assert!(
        !line(&buf, find_left(&buf, "restore").unwrap().0).contains("match"),
        "a name-only row has no count"
    );
}

#[test]
fn a_hundred_matches_read_99_plus() {
    let mut big = cmd("big");
    big.entities = (0..120).map(|i| flag(&format!("re{i}"), "x")).collect();
    let mut root = cmd("tool");
    root.subcommands = vec![big, cmd("small")];
    let mut app = App::new("tool".to_string(), root);
    app.focus_search();
    type_query(&mut app, "re");
    let buf = draw(&app, 120, 30);
    assert!(find_left(&buf, "99+ matches").is_some());
}

#[test]
fn a_narrow_pane_drops_the_count_column_first() {
    let app = searched("re", "clone");
    let wide = draw(&app, 120, 30);
    assert!(find(&wide, "matches").is_some());
    let narrow = draw(&app, 60, 20);
    assert!(
        find(&narrow, "match").is_none(),
        "{}",
        (0..20)
            .map(|y| line(&narrow, y))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        find(&narrow, "commit").is_some(),
        "the rows themselves stay"
    );
}

#[test]
fn n_and_capital_n_walk_the_matching_entities_and_wrap() {
    let mut app = searched("re", "commit");
    draw(&app, 120, 30);
    app.step_match(true);
    assert_eq!(
        app.selected_flag,
        Some(mandible_core::FlagKey::Long("reedit-message".into()))
    );
    draw(&app, 120, 30);
    app.step_match(true);
    assert_eq!(
        app.selected_flag,
        Some(mandible_core::FlagKey::Long("reset-author".into()))
    );
    draw(&app, 120, 30);
    app.step_match(true);
    assert_eq!(
        app.selected_flag,
        Some(mandible_core::FlagKey::Long("reedit-message".into())),
        "wraps"
    );
    app.step_match(false);
    assert_eq!(
        app.selected_flag,
        Some(mandible_core::FlagKey::Long("reset-author".into())),
        "wraps back"
    );
}

#[test]
fn n_does_nothing_without_a_search() {
    let mut app = App::new("tool".to_string(), tool());
    draw(&app, 120, 30);
    app.step_match(true);
    assert_eq!(app.selected_flag, None);
    let mut keys = App::new("tool".to_string(), tool());
    crate::event::handle_key(
        &mut keys,
        crossterm::event::KeyEvent::from(crossterm::event::KeyCode::Char('n')),
    );
    assert_eq!(keys.selected_flag, None);
}

#[test]
fn the_footer_names_n_only_while_searching() {
    let mut app = App::new("tool".to_string(), tool());
    let buf = draw(&app, 140, 30);
    assert!(find(&buf, "n/N next match").is_none());
    app.focus_search();
    type_query(&mut app, "re");
    app.accept_search();
    let buf = draw(&app, 140, 30);
    assert!(find(&buf, "n/N next match").is_some());
}

#[test]
fn the_no_match_notice_has_the_panes_one_column_margin() {
    let mut app = App::new("tool".to_string(), tool());
    app.focus_search();
    type_query(&mut app, "zzqq");
    let buf = draw(&app, 120, 30);
    let (y, x) = find(&buf, "No matches").unwrap();
    assert_eq!(buf[(x - 1, y)].symbol(), " ", "{}", line(&buf, y));
    assert_eq!(buf[(x - 2, y)].symbol(), "│");
}

#[test]
fn selected_name_helper_is_reachable() {
    let app = searched("re", "commit");
    assert_eq!(selected_name(&app), "commit");
    let _ = settle;
}
