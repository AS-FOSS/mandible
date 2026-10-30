//! A page with two rows of one spelling: `n`/`p` visit each row once, and
//! one row at a time carries the bar (spec §10).

use super::search_tests::{cmd, prov, type_query};
use super::*;
use crate::sort::{DetailSort, RowId};
use mandible_core::Entity;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// Tool order: `a-b`, `a-a`, `a-b`, `a-c`. Sorted, the two `a-b` rows are
/// neighbours; in the tool's order, `a-a` stands between them.
fn repeated() -> CommandNode {
    let mut node = cmd("page");
    node.entities = ["a-b", "a-a", "a-b", "a-c"]
        .map(|name| Entity::flag_long(name, prov()))
        .to_vec();
    let mut root = cmd("tool");
    root.subcommands = vec![node];
    root
}

fn searched(sort: DetailSort) -> App {
    let mut app = App::new("tool".to_string(), repeated());
    app.sorts.detail = sort;
    app.set_palette(crate::style::Palette::extended());
    app.focus_search();
    type_query(&mut app, "a-");
    app.accept_search();
    draw(&app);
    app
}

fn draw(app: &App) {
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    terminal.draw(|f| crate::render::render(f, app)).unwrap();
}

/// The rows on the bar, and the page order of all matches.
fn lit(app: &App) -> (Vec<RowId>, Vec<RowId>) {
    let rows = app.page_matches.borrow().1.clone();
    let on = rows.iter().filter(|m| m.current).map(|m| m.id).collect();
    (on, rows.iter().map(|m| m.id).collect())
}

/// The row lit after each of `steps` presses of `n` or `p`, starting from
/// the first jump.
fn walk(app: &mut App, forward: bool, steps: usize) -> Vec<RowId> {
    let mut seen = Vec::new();
    for _ in 0..=steps {
        let (on, _) = lit(app);
        assert_eq!(on.len(), 1, "one row on the bar: {on:?}");
        seen.push(on[0]);
        app.step_match(forward);
        draw(app);
    }
    seen
}

fn visits_every_row_once(sort: DetailSort) {
    let mut app = searched(sort);
    let (_, page) = lit(&app);
    assert_eq!(page.len(), 4);
    let start = lit(&app).0[0];
    let at = page.iter().position(|&r| r == start).unwrap();
    let ahead: Vec<RowId> = (0..=4).map(|i| page[(at + i) % 4]).collect();
    assert_eq!(walk(&mut app, true, 4), ahead, "n, wrapping");
    let back: Vec<RowId> = (0..=4).map(|i| page[(at + 4 - i % 4) % 4]).collect();
    let mut app = searched(sort);
    assert_eq!(walk(&mut app, false, 4), back, "p, wrapping");
}

#[test]
fn n_and_p_visit_every_copy_of_a_repeated_name_sorted() {
    visits_every_row_once(DetailSort::Overall);
}

#[test]
fn n_and_p_visit_every_copy_of_a_repeated_name_in_tool_order() {
    visits_every_row_once(DetailSort::Printed);
}

#[test]
fn the_copy_stays_current_when_the_page_is_sorted() {
    let mut app = searched(DetailSort::Printed);
    app.step_match(true);
    app.step_match(true);
    draw(&app);
    let before = lit(&app).0;
    for _ in 0..3 {
        app.focus = Focus::Detail;
        app.sort_focused();
        draw(&app);
        assert_eq!(lit(&app).0, before);
    }
}
