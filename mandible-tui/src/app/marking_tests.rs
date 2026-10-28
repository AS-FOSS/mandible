//! Marking what a search matched: the detail pane's rows, the commands
//! list's rows and counts, `n`/`p`, and the no-match notice margin
//! (spec §10).

use super::search_tests::{cmd, prov, selected_name, settle, type_query};
use super::*;
use mandible_core::{Entity, Text};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};
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

/// The bar's background and foreground, as the style module names them.
const BG: Color = Color::Indexed(crate::style::BAR_BG);
const FG: Color = Color::Indexed(crate::style::BAR_FG);

fn bg(buf: &Buffer, x: u16, y: u16) -> Color {
    buf[(x, y)].style().bg.unwrap_or(Color::Reset)
}

fn fg(buf: &Buffer, x: u16, y: u16) -> Color {
    buf[(x, y)].style().fg.unwrap_or(Color::Reset)
}

/// The column of the right border of the pane that holds column `x`.
fn right_border(buf: &Buffer, x: u16, y: u16) -> u16 {
    (x..buf.area.width)
        .find(|&c| buf[(c, y)].symbol() == "│")
        .unwrap()
}

/// Rows of the detail pane (right half) with the bar's background in them.
fn bar_rows(buf: &Buffer) -> Vec<u16> {
    let half = buf.area.width / 2;
    (0..buf.area.height)
        .filter(|&y| (half..buf.area.width).any(|x| bg(buf, x, y) == BG))
        .collect()
}

#[test]
fn matching_rows_get_a_bar_and_underlined_letters_and_no_bold() {
    let app = searched("re", "commit");
    let buf = draw(&app, 120, 30);
    let g = gutter(&buf);
    let (y, x) = find(&buf, "reedit-message").unwrap();
    assert_eq!(buf[(g, y)].symbol(), "▌", "{}", line(&buf, y));
    assert!(has(&buf, x, y, Modifier::UNDERLINED));
    assert!(
        !has(&buf, x + 2, y, Modifier::UNDERLINED),
        "only the matched letters"
    );
    assert!(
        !has(&buf, x, y, Modifier::BOLD),
        "matching rows are not bold"
    );
    let (dy, dx) = find(&buf, "reuse and edit").unwrap();
    assert!(!has(&buf, dx, dy, Modifier::BOLD));
    assert!(bar_rows(&buf).is_empty(), "no current match yet");
}

#[test]
fn the_current_match_sits_on_the_bar_across_the_row() {
    let mut app = searched("re", "commit");
    draw(&app, 120, 30);
    app.step_match(true);
    let buf = draw(&app, 120, 30);
    let (y, x) = find(&buf, "reedit-message").unwrap();
    assert_eq!(bar_rows(&buf), vec![y], "exactly one row");
    let g = gutter(&buf);
    assert_eq!(buf[(g, y)].symbol(), "▌");
    assert_ne!(bg(&buf, g, y), BG, "the margin bar stays as it was");
    assert_eq!(
        fg(&buf, x, y),
        crate::style::ACCENT,
        "spelling stays accent"
    );
    assert!(has(&buf, x, y, Modifier::UNDERLINED));
    let (dy, dx) = find(&buf, "reuse and edit").unwrap();
    assert_eq!(
        (dy, fg(&buf, dx, dy)),
        (y, FG),
        "description in the bar foreground"
    );
    let edge = right_border(&buf, dx, y);
    assert_eq!(bg(&buf, edge - 2, y), BG, "the bar runs to the padding");
    assert_ne!(bg(&buf, edge - 1, y), BG, "and stops inside it");
    assert_eq!(bg(&buf, g + 1, y), BG);
    let (ry, rx) = find(&buf, "reset-author").unwrap();
    assert_ne!(bg(&buf, rx, ry), BG, "the other match is not current");
}

#[test]
fn only_one_row_is_current_after_n_and_p() {
    let mut app = searched("re", "commit");
    draw(&app, 120, 30);
    for forward in [true, true, false] {
        app.step_match(forward);
        let buf = draw(&app, 120, 30);
        assert_eq!(bar_rows(&buf).len(), 1);
    }
    let buf = draw(&app, 120, 30);
    assert_eq!(
        bar_rows(&buf),
        vec![find(&buf, "reedit-message").unwrap().0]
    );
}

#[test]
fn the_current_match_wraps_its_lines_on_the_bar() {
    let mut app = searched("re", "commit");
    draw(&app, 120, 30);
    app.step_match(true);
    let buf = draw(&app, 120, 30);
    let (y, _) = find(&buf, "reedit-message").unwrap();
    let wrapped = bar_rows(&buf);
    assert_eq!(wrapped, vec![y]);
    let narrow = draw(&app, 100, 30);
    let rows = bar_rows(&narrow);
    let (ny, _) = find(&narrow, "reedit-message").unwrap();
    assert!(rows.contains(&ny));
}

#[test]
fn no_color_puts_the_current_match_in_reverse() {
    let mut app = searched("re", "commit");
    app.set_palette(crate::style::Palette::plain());
    draw(&app, 120, 30);
    app.step_match(true);
    let buf = draw(&app, 120, 30);
    let (y, x) = find(&buf, "reedit-message").unwrap();
    let (dy, dx) = find(&buf, "reuse and edit").unwrap();
    assert!(has(&buf, x, y, Modifier::REVERSED));
    assert!(has(&buf, dx, dy, Modifier::REVERSED));
    let edge = right_border(&buf, dx, dy);
    assert!(has(&buf, edge - 2, dy, Modifier::REVERSED));
    let (ry, rx) = find(&buf, "reset-author").unwrap();
    assert!(!has(&buf, rx, ry, Modifier::REVERSED));
    assert_eq!(bg(&buf, x, y), Color::Reset);
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
fn no_color_keeps_the_bar_and_the_underline() {
    let mut app = searched("re", "commit");
    app.set_palette(crate::style::Palette::plain());
    let buf = draw(&app, 120, 30);
    let g = gutter(&buf);
    let (y, x) = find(&buf, "reedit-message").unwrap();
    assert_eq!(buf[(g, y)].symbol(), "▌");
    assert!(has(&buf, x, y, Modifier::UNDERLINED));
    let (ay, ax) = find(&buf, "amend previous").unwrap();
    assert!(!has(&buf, ax, ay, Modifier::BOLD));
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
fn the_selected_list_row_is_one_bar_with_a_light_foreground() {
    let app = searched("re", "commit");
    let buf = draw(&app, 120, 30);
    let (y, x) = find_left(&buf, "commit").unwrap();
    assert_eq!((fg(&buf, x, y), bg(&buf, x, y)), (crate::style::ACCENT, BG));
    assert!(has(&buf, x, y, Modifier::BOLD));
    let (sy, sx) = find_left(&buf, "Record changes").unwrap();
    assert_eq!(sy, y);
    assert_eq!((fg(&buf, sx, y), bg(&buf, sx, y)), (FG, BG), "summary");
    assert!(!has(&buf, sx, y, Modifier::DIM) && !has(&buf, sx, y, Modifier::BOLD));
    let (cy, cx) = find_left(&buf, "  2 found").unwrap();
    assert_eq!((cy, fg(&buf, cx + 4, y), bg(&buf, cx + 4, y)), (y, FG, BG));
    let edge = right_border(&buf, cx, y);
    assert_eq!(bg(&buf, edge - 1, y), BG, "the margin is on the bar too");
    assert!((x..edge).all(|c| bg(&buf, c, y) == BG), "no gap in the bar");
    let (oy, ox) = find_left(&buf, "restore").unwrap();
    assert_ne!(bg(&buf, ox, oy), BG, "other rows are not on it");
}

#[test]
fn without_256_colors_or_without_color_the_whole_row_is_reversed() {
    for palette in [
        crate::style::Palette::basic(),
        crate::style::Palette::plain(),
    ] {
        let mut app = searched("re", "commit");
        app.set_palette(palette);
        let buf = draw(&app, 120, 30);
        let (y, x) = find_left(&buf, "commit").unwrap();
        let (_, sx) = find_left(&buf, "Record changes").unwrap();
        let (_, cx) = find_left(&buf, "  2 found").unwrap();
        for at in [x, sx, cx + 4, right_border(&buf, cx, y) - 1] {
            assert!(has(&buf, at, y, Modifier::REVERSED), "{palette:?} at {at}");
            assert!(!has(&buf, at, y, Modifier::DIM));
        }
        assert_eq!(bg(&buf, x, y), Color::Reset);
    }
}

#[test]
fn the_selected_row_is_a_bar_without_any_search() {
    let mut app = App::new("tool".to_string(), tool());
    app.set_palette(crate::style::Palette::extended());
    app.select_index(1);
    let buf = draw(&app, 120, 30);
    let (y, x) = find_left(&buf, "clone").unwrap();
    assert_eq!(bg(&buf, x, y), BG);
    let edge = right_border(&buf, x, y);
    assert_eq!(bg(&buf, edge - 1, y), BG);
}

#[test]
fn counts_sit_in_a_fixed_width_right_aligned_column() {
    let app = searched("re", "commit");
    let buf = draw(&app, 120, 30);
    let (py, px) = find_left(&buf, "  1 found").unwrap();
    let (cy, cx) = find_left(&buf, "  2 found").unwrap();
    assert_eq!(cx, px, "both end at the same column");
    assert_eq!(buf[(cx + 9, cy)].symbol(), " ", "one column of margin");
    assert_eq!(buf[(cx + 10, cy)].symbol(), "│", "then the pane edge");
    assert_eq!(buf[(px + 10, py)].symbol(), "│");
    assert_ne!(cy, py);
    assert!(
        !line(&buf, find_left(&buf, "restore").unwrap().0).contains("found"),
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
    assert!(find_left(&buf, "99+ found").is_some());
}

#[test]
fn a_narrow_pane_drops_the_count_column_first() {
    let app = searched("re", "clone");
    let wide = draw(&app, 120, 30);
    assert!(find(&wide, "found").is_some());
    let narrow = draw(&app, 60, 20);
    assert!(
        find(&narrow, "found").is_none(),
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
fn n_and_p_walk_the_matching_entities_and_wrap() {
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
fn capital_n_and_p_act_like_the_small_keys() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let press = |app: &mut App, c: char| {
        crate::event::handle_key(app, KeyEvent::new(KeyCode::Char(c), KeyModifiers::SHIFT));
    };
    let key = |name: &str| Some(mandible_core::FlagKey::Long(name.into()));
    let mut app = searched("re", "commit");
    draw(&app, 120, 30);
    press(&mut app, 'N');
    assert_eq!(app.selected_flag, key("reedit-message"));
    draw(&app, 120, 30);
    press(&mut app, 'n');
    assert_eq!(app.selected_flag, key("reset-author"));
    draw(&app, 120, 30);
    press(&mut app, 'P');
    assert_eq!(app.selected_flag, key("reedit-message"));
    draw(&app, 120, 30);
    press(&mut app, 'p');
    assert_eq!(app.selected_flag, key("reset-author"), "wraps back");
}

#[test]
fn p_steps_back_from_the_detail_pane_too() {
    let mut app = searched("re", "commit");
    app.toggle_focus();
    draw(&app, 120, 30);
    let press = |app: &mut App, c: char| {
        crate::event::handle_key(
            app,
            crossterm::event::KeyEvent::from(crossterm::event::KeyCode::Char(c)),
        );
    };
    press(&mut app, 'p');
    assert_eq!(
        app.selected_flag,
        Some(mandible_core::FlagKey::Long("reset-author".into())),
        "from nothing, p goes to the last match"
    );
}

#[test]
fn the_count_column_goes_when_a_summary_would_get_under_20_columns() {
    let app = searched("re", "commit");
    let shows = |cols: u16| {
        let buf = draw(&app, cols, 30);
        find_left(&buf, "found").is_some()
    };
    let narrowest = (60..=120u16).find(|&c| shows(c)).unwrap();
    assert!(!shows(narrowest - 1));
    // The tree pane's inner width at that size: its top border row ends in
    // `╮`. The summary column is 13 wide; it keeps 20 columns beside the
    // count's 9, its 2-column gap and its 1-column margin.
    let buf = draw(&app, narrowest, 30);
    let top = (0..narrowest)
        .find(|&x| buf[(x, 3)].symbol() == "╮")
        .unwrap();
    assert_eq!(usize::from(top) - 1, 13 + 20 + 9 + 2 + 1);
}

#[test]
fn page_positions_follow_the_order_the_page_draws_groups_in() {
    let grouped = |name: &str, group: Option<&str>| {
        let mut e = flag(name, "x");
        e.group = group.map(str::to_string);
        e
    };
    let mut c = cmd("c");
    c.entities = vec![
        grouped("re-b1", Some("Beta")),
        grouped("re-plain", None),
        grouped("re-a1", Some("Alpha")),
        grouped("re-b2", Some("Beta")),
    ];
    let mut root = cmd("tool");
    root.subcommands = vec![c];
    let mut app = App::new("tool".to_string(), root);
    app.focus_search();
    type_query(&mut app, "re-");
    app.accept_search();
    let buf = draw(&app, 120, 40);
    let mut by_screen: Vec<(u16, String)> = ["re-b1", "re-plain", "re-a1", "re-b2"]
        .iter()
        .map(|n| (find(&buf, &format!("--{n}")).unwrap().0, n.to_string()))
        .collect();
    by_screen.sort();
    let node = crate::search_rank::tests_node(&app);
    let positions = crate::render::page_positions(&node, crate::sort::DetailSort::Printed);
    let mut by_rank: Vec<(usize, String)> = by_screen
        .iter()
        .map(|(_, n)| {
            (
                positions[&mandible_core::FlagKey::Long(n.clone())],
                n.clone(),
            )
        })
        .collect();
    by_rank.sort();
    let screen: Vec<&str> = by_screen.iter().map(|(_, n)| n.as_str()).collect();
    let ranked: Vec<&str> = by_rank.iter().map(|(_, n)| n.as_str()).collect();
    assert_eq!(screen, ranked);
    assert_eq!(screen, ["re-plain", "re-b1", "re-b2", "re-a1"]);
}

#[test]
fn n_does_nothing_without_a_search() {
    let mut app = App::new("tool".to_string(), tool());
    draw(&app, 120, 30);
    app.step_match(true);
    assert_eq!(app.selected_flag, None);
    let mut cleared = searched("re", "commit");
    draw(&cleared, 120, 30);
    cleared.clear_filter();
    cleared.step_match(true);
    assert_eq!(cleared.selected_flag, None, "the search is gone");
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
    assert!(find(&buf, "n/p match").is_none());
    app.focus_search();
    type_query(&mut app, "re");
    app.accept_search();
    let buf = draw(&app, 140, 30);
    assert!(find(&buf, "n/p match").is_some());
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
