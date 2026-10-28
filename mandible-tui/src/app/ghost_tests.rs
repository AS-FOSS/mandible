//! The dim suggestion in the search box and `Tab` to take it (spec §10).

use super::search_tests::{cmd, prov, selected_name, settle, type_query, with_flag};
use super::*;
use crate::search_ghost::GhostKind;
use mandible_core::{Entity, FlagKey};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};
use ratatui::Terminal;

fn git() -> CommandNode {
    let mut root = cmd("git");
    let mut ps = cmd("ps");
    ps.entities.push(Entity::flag_long("no-trunc", prov()));
    root.subcommands = vec![
        cmd("add"),
        with_flag(cmd("commit"), "squash"),
        with_flag(cmd("merge"), "squash"),
        ps,
    ];
    root
}

fn typing(root: CommandNode, query: &str) -> App {
    let mut app = App::new("git".to_string(), root);
    app.focus_search();
    type_query(&mut app, query);
    app
}

fn shown(app: &App) -> Option<String> {
    app.ghost().map(|g| g.shown("→"))
}

#[test]
fn the_rest_of_the_spelling_the_cursor_lands_on_is_suggested() {
    let app = typing(git(), "--sq");
    assert_eq!(shown(&app).as_deref(), Some("uash"));
    assert_eq!(app.ghost().unwrap().kind, GhostKind::Completes);
    assert_eq!(app.ghost().unwrap().accept, "--squash");
    assert_eq!(app.search_query, "--sq", "the input is not changed");
}

#[test]
fn the_suggestion_is_found_whatever_case_is_typed() {
    let app = typing(git(), "--SQ");
    assert_eq!(shown(&app).as_deref(), Some("uash"));
    assert_eq!(app.ghost().unwrap().accept, "--squash");
}

#[test]
fn a_command_name_is_suggested_too() {
    let app = typing(git(), "comm");
    assert_eq!(shown(&app).as_deref(), Some("it"));
}

#[test]
fn the_no_form_of_a_negatable_flag_is_suggested() {
    let mut root = cmd("git");
    let mut commit = cmd("commit");
    commit.entities.push(Entity::flag_spelled(
        None,
        Some("squash".into()),
        false,
        true,
        prov(),
    ));
    root.subcommands = vec![commit];
    let app = typing(root, "--no-sq");
    assert_eq!(app.ghost().unwrap().accept, "--no-squash");
}

#[test]
fn a_match_in_the_middle_suggests_the_target_after_an_arrow() {
    let app = typing(git(), "trunc");
    assert!(!app.no_match());
    let ghost = app.ghost().unwrap();
    assert_eq!(ghost.kind, GhostKind::Target);
    assert_eq!(ghost.accept, "--no-trunc");
    assert_eq!(ghost.shown("→"), "   → --no-trunc");
    assert_eq!(app.search_query, "trunc", "the input is not changed");
}

#[test]
fn a_multi_word_query_suggests_the_target_after_an_arrow() {
    let mut root = cmd("git");
    root.subcommands = vec![cmd("cherry-pick"), cmd("add")];
    let app = typing(root, "cherry pick");
    assert_eq!(shown(&app).as_deref(), Some("   → cherry-pick"));
    assert_eq!(
        typing(git(), "no trunc").ghost().unwrap().accept,
        "--no-trunc"
    );
}

#[test]
fn no_suggestion_when_the_input_is_the_whole_spelling() {
    assert_eq!(typing(git(), "--squash").ghost(), None);
    assert_eq!(typing(git(), "--SQUASH").ghost(), None);
}

#[test]
fn nothing_matched_suggests_the_closest_name() {
    let app = typing(git(), "--sqash");
    assert!(app.no_match());
    let ghost = app.ghost().unwrap();
    assert_eq!(ghost.kind, GhostKind::Closest);
    assert_eq!(ghost.accept, "--squash");
    assert_eq!(ghost.shown("→"), "   → --squash");
    assert_eq!(ghost.shown("->"), "   -> --squash");
    assert_eq!(app.search_query, "--sqash");
}

#[test]
fn the_closest_name_is_at_most_two_edits_away_for_any_length() {
    assert_eq!(
        shown(&typing(git(), "--sqxxsh")).as_deref(),
        Some("   → --squash")
    );
    assert_eq!(typing(git(), "--sqxxxh").ghost(), None, "three edits");
    let mut root = cmd("git");
    root.entities
        .push(Entity::flag_long("a-flag-name-of-some-length", prov()));
    assert!(typing(root.clone(), "--a-flag-name-of-sme-lngth")
        .ghost()
        .is_some());
    assert_eq!(typing(root, "--a-flag-name-of-sme-lngt").ghost(), None);
    assert_eq!(typing(git(), "zzqq").ghost(), None);
}

#[test]
fn equal_distances_go_to_the_name_sharing_the_longer_beginning_first() {
    let mut root = cmd("git");
    root.entities.push(Entity::flag_long("stat", prov()));
    root.entities.push(Entity::flag_long("squash", prov()));
    assert_eq!(typing(root, "--sqas").ghost().unwrap().accept, "--squash");
}

#[test]
fn equal_distances_go_to_the_earlier_command_then_the_earlier_line_of_its_page() {
    let mut root = cmd("tool");
    root.subcommands = vec![cmd("commat"), cmd("commit")];
    assert_eq!(typing(root, "commet").ghost().unwrap().accept, "commat");
    let mut root = cmd("tool");
    root.subcommands = vec![cmd("commit"), cmd("commat")];
    assert_eq!(typing(root, "commet").ghost().unwrap().accept, "commit");

    let mut page = cmd("tool");
    page.entities.push(Entity::flag_long("aab", prov()));
    page.entities.push(Entity::flag_long("aaa", prov()));
    page.entities[0].group = Some("Later".to_string());
    assert_eq!(typing(page, "--aac").ghost().unwrap().accept, "--aaa");
}

#[test]
fn tab_takes_a_completion_and_the_jump_lands_on_it() {
    let mut app = typing(git(), "--sq");
    app.accept_ghost();
    settle(&mut app);
    assert_eq!(app.search_query, "--squash");
    assert_eq!(selected_name(&app), "commit");
    assert_eq!(app.selected_flag, Some(FlagKey::Long("squash".into())));
    assert_eq!(app.ghost(), None);
    assert_eq!(app.focus, Focus::Search);
}

#[test]
fn tab_takes_the_closest_name_and_the_notice_goes() {
    let mut app = typing(git(), "--sqash");
    app.accept_ghost();
    settle(&mut app);
    assert_eq!(app.search_query, "--squash");
    assert!(!app.no_match());
    assert_eq!(app.selected_flag, Some(FlagKey::Long("squash".into())));
}

#[test]
fn tab_takes_a_target_spelling_and_the_jump_lands_on_it() {
    let mut app = typing(git(), "trunc");
    app.accept_ghost();
    settle(&mut app);
    assert_eq!(app.search_query, "--no-trunc");
    assert_eq!(selected_name(&app), "ps");
    assert_eq!(app.selected_flag, Some(FlagKey::Long("no-trunc".into())));
    assert_eq!(app.ghost(), None);
}

#[test]
fn tab_without_a_suggestion_does_nothing() {
    let mut app = typing(git(), "--squash");
    let before = (app.search_query.clone(), app.selected_path());
    app.accept_ghost();
    settle(&mut app);
    assert_eq!((app.search_query.clone(), app.selected_path()), before);
    assert_eq!(app.focus, Focus::Search);
}

#[test]
fn tab_in_the_box_does_not_switch_panes() {
    use crate::event::handle_key;
    use crossterm::event::{KeyCode, KeyEvent};
    let mut app = typing(git(), "--sq");
    assert!(handle_key(&mut app, KeyEvent::from(KeyCode::Tab)).is_none());
    settle(&mut app);
    assert_eq!(app.search_query, "--squash");
    assert_eq!(app.focus, Focus::Search);
}

#[test]
fn a_suggestion_for_an_earlier_input_is_not_shown() {
    let mut app = typing(git(), "--sq");
    assert!(app.ghost().is_some());
    app.search_input_char('x');
    assert_eq!(app.ghost(), None, "before the new input is searched");
    app.accept_ghost();
    assert_eq!(app.search_query, "--sqx");
}

#[test]
fn the_suggestion_is_only_for_the_box_being_typed_in() {
    let mut app = typing(git(), "--sq");
    app.accept_search();
    assert_eq!(app.ghost(), None);
}

fn draw(app: &App, cols: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(cols, 20)).unwrap();
    terminal.draw(|f| crate::render::render(f, app)).unwrap();
    terminal.backend().buffer().clone()
}

fn row(buf: &Buffer, y: u16) -> String {
    (0..buf.area.width)
        .map(|x| buf[(x, y)].symbol().to_string())
        .collect()
}

#[test]
fn the_box_draws_the_suggestion_muted_after_the_typed_text() {
    let mut app = typing(git(), "--sq");
    app.set_palette(crate::style::Palette::basic());
    let buf = draw(&app, 80);
    assert!(row(&buf, 1).contains("› --squash"), "{}", row(&buf, 1));
    let x = row(&buf, 1).chars().position(|c| c == 'u').unwrap() as u16;
    assert_eq!(buf[(x, 1)].style().fg, Some(Color::DarkGray));
    assert_ne!(buf[(x - 1, 1)].style().fg, Some(Color::DarkGray));
}

#[test]
fn without_color_the_suggestion_is_italic_and_dim() {
    let mut app = typing(git(), "--sq");
    app.set_palette(crate::style::Palette::plain());
    let buf = draw(&app, 80);
    let x = row(&buf, 1).chars().position(|c| c == 'u').unwrap() as u16;
    let m = buf[(x, 1)].style().add_modifier;
    assert!(m.contains(Modifier::ITALIC) && m.contains(Modifier::DIM));
    assert!(!buf[(x - 1, 1)]
        .style()
        .add_modifier
        .contains(Modifier::ITALIC));
}

#[test]
fn a_closest_name_is_drawn_after_an_arrow_and_the_notice_stays() {
    let app = typing(git(), "--sqash");
    let buf = draw(&app, 80);
    assert!(
        row(&buf, 1).contains("› --sqash   → --squash"),
        "{}",
        row(&buf, 1)
    );
    let screen: String = (0..20).map(|y| row(&buf, y)).collect();
    assert!(screen.contains("No matches for \"--sqash\""));
}

#[test]
fn the_ascii_set_draws_an_ascii_arrow() {
    let mut app = typing(git(), "--sqash");
    app.glyphs = crate::glyphs::ASCII;
    let buf = draw(&app, 80);
    assert!(
        row(&buf, 1).contains("--sqash   -> --squash"),
        "{}",
        row(&buf, 1)
    );
}

#[test]
fn a_narrow_box_cuts_the_suggestion_and_never_overflows() {
    let app = typing(git(), "--sqash");
    for cols in 30..60 {
        let buf = draw(&app, cols);
        assert_eq!(buf[(cols - 1, 0)].symbol(), "╮", "{cols}: {}", row(&buf, 0));
        assert_eq!(buf[(cols - 1, 1)].symbol(), "│", "{cols}: {}", row(&buf, 1));
    }
}

#[test]
fn the_footer_names_tab_as_accept_while_the_box_has_focus() {
    for query in ["--sq", "trunc", "zzqq", "--squash"] {
        let app = typing(git(), query);
        let buf = draw(&app, 140);
        let screen: String = (0..20).map(|y| row(&buf, y)).collect();
        assert!(
            screen.contains("Tab accept") && !screen.contains("Tab pane"),
            "{query}"
        );
    }
    let mut app = typing(git(), "--sq");
    app.accept_search();
    let buf = draw(&app, 140);
    let screen: String = (0..20).map(|y| row(&buf, y)).collect();
    assert!(screen.contains("Tab pane") && !screen.contains("Tab accept"));
}
