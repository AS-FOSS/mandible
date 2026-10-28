//! One matching rule in both modes: a row matches when its text contains
//! every space-separated word of the query, ignoring case; the tiers choose
//! only which match the cursor jumps to first (spec §10).

use super::search_tests::{cmd, prov, selected_name, type_query, with_flag};
use super::*;
use mandible_core::{Entity, FlagKey, Text};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::Modifier;
use ratatui::Terminal;

fn tool() -> CommandNode {
    let mut root = cmd("tool");
    let mut build = with_flag(cmd("build"), "blocksize");
    let mut quiet = Entity::flag_long("quiet", prov());
    quiet.description = Some(Text::sanitize("grumble about nothing"));
    build.entities.push(quiet);
    root.subcommands = vec![
        build,
        with_flag(cmd("gcc"), "sysroot"),
        cmd("cherry-pick"),
        cmd("cherry-pack"),
        with_flag(cmd("push"), "no-verify"),
        cmd("other"),
    ];
    root
}

fn search(root: CommandNode, wide: bool, query: &str) -> App {
    let mut app = App::new("tool".to_string(), root);
    app.focus_search();
    if wide {
        app.cycle_search_mode();
    }
    type_query(&mut app, query);
    app
}

fn shown(app: &App) -> Vec<String> {
    app.rows().iter().map(|r| r.name.clone()).collect()
}

fn long(name: &str) -> Option<FlagKey> {
    Some(FlagKey::Long(name.into()))
}

#[test]
fn a_name_that_contains_the_query_matches_in_both_modes() {
    for wide in [false, true] {
        let app = search(tool(), wide, "size");
        assert!(shown(&app).contains(&"build".to_string()), "{wide}");
        assert!(!shown(&app).contains(&"other".to_string()), "{wide}");
        assert_eq!(app.selected_flag, long("blocksize"), "{wide}");
        let app = search(tool(), wide, "root");
        assert_eq!(selected_name(&app), "gcc", "{wide}");
        assert_eq!(app.selected_flag, long("sysroot"), "{wide}");
    }
}

#[test]
fn every_word_must_appear_and_a_dash_in_the_name_is_met_by_a_space() {
    for wide in [false, true] {
        let app = search(tool(), wide, "cherry pick");
        assert!(shown(&app).contains(&"cherry-pick".to_string()), "{wide}");
        assert!(!shown(&app).contains(&"cherry-pack".to_string()), "{wide}");
        let app = search(tool(), wide, "verify no");
        assert_eq!(app.selected_flag, long("no-verify"), "{wide}");
        let app = search(tool(), wide, "no verifx");
        assert!(app.no_match(), "{wide}");
    }
}

#[test]
fn case_is_ignored() {
    for wide in [false, true] {
        let app = search(tool(), wide, "CHERRY Pick");
        assert_eq!(selected_name(&app), "cherry-pick", "{wide}");
    }
}

#[test]
fn a_description_matches_only_in_the_wide_mode() {
    let name_mode = search(tool(), false, "grumble");
    assert!(name_mode.no_match());
    let wide = search(tool(), true, "grumble");
    assert_eq!(wide.selected_flag, long("quiet"));
}

#[test]
fn both_forms_of_a_negatable_flag_are_found() {
    let mut root = cmd("tool");
    let mut commit = cmd("commit");
    commit.entities.push(Entity::flag_spelled(
        None,
        Some("squash".into()),
        false,
        true,
        prov(),
    ));
    root.subcommands = vec![commit, cmd("other")];
    for wide in [false, true] {
        for query in ["--squash", "--no-squash", "no squash", "--no"] {
            let app = search(root.clone(), wide, query);
            assert!(!app.no_match(), "{query} {wide}");
            assert_eq!(app.selected_flag, long("squash"), "{query} {wide}");
        }
    }
}

#[test]
fn the_first_jump_goes_exact_then_prefix_then_word_then_inside() {
    let order = ["size", "Size", "sizes", "disk-size", "blocksize"];
    let mut names: Vec<&str> = order.to_vec();
    names.reverse();
    for winner in 0..order.len() {
        let mut root = cmd("tool");
        root.subcommands = names[..names.len() - winner]
            .iter()
            .map(|n| cmd(n))
            .collect();
        let app = search(root, false, "size");
        assert_eq!(selected_name(&app), order[winner]);
    }
}

#[test]
fn the_tiers_never_hide_or_reorder_the_tree() {
    let mut root = cmd("tool");
    root.subcommands = vec![cmd("blocksize"), cmd("size"), cmd("disk-size")];
    let app = search(root, false, "size");
    assert_eq!(shown(&app), ["tool", "blocksize", "size", "disk-size"]);
    assert_eq!(selected_name(&app), "size");
}

#[test]
fn a_command_comes_before_a_flag_of_the_same_tier() {
    let mut root = cmd("tool");
    root.subcommands = vec![with_flag(cmd("a"), "size"), cmd("size")];
    let app = search(root, false, "size");
    assert_eq!(selected_name(&app), "size");
    assert_eq!(app.selected_flag, None);
}

fn draw(app: &App, cols: u16, rows: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(cols, rows)).unwrap();
    terminal.draw(|f| crate::render::render(f, app)).unwrap();
    terminal.backend().buffer().clone()
}

fn find(buf: &Buffer, text: &str) -> Option<(u16, u16)> {
    find_in(buf, text, buf.area.width)
}

fn find_left(buf: &Buffer, text: &str) -> Option<(u16, u16)> {
    find_in(buf, text, buf.area.width / 2)
}

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

/// The chars of `len` cells from `(y, x)` that are underlined, as a mask.
fn mask(buf: &Buffer, y: u16, x: u16, len: u16) -> String {
    (0..len)
        .map(|i| {
            if buf[(x + i, y)]
                .style()
                .add_modifier
                .contains(Modifier::UNDERLINED)
            {
                'u'
            } else {
                '.'
            }
        })
        .collect()
}

#[test]
fn every_occurrence_of_every_word_is_underlined_in_a_command_name() {
    let mut root = cmd("tool");
    root.subcommands = vec![cmd("cherry-pick"), cmd("foo-foo")];
    let app = search(root.clone(), false, "ch pic");
    let buf = draw(&app, 100, 30);
    let (y, x) = find_left(&buf, "cherry-pick").unwrap();
    assert_eq!(mask(&buf, y, x, 11), "uu.....uuu.");
    let app = search(root, false, "oo");
    let buf = draw(&app, 100, 30);
    let (y, x) = find_left(&buf, "foo-foo").unwrap();
    assert_eq!(mask(&buf, y, x, 7), ".uu..uu");
}

fn squash_tool() -> CommandNode {
    let mut commit = cmd("commit");
    commit.entities.push(Entity::flag_spelled(
        Some('s'),
        Some("squash".into()),
        false,
        true,
        prov(),
    ));
    commit.entities.push(Entity::flag_long("no-verify", prov()));
    let mut root = cmd("tool");
    root.subcommands = vec![commit];
    root
}

fn page_of(query: &str) -> Buffer {
    let mut app = search(squash_tool(), false, query);
    app.accept_search();
    draw(&app, 120, 30)
}

#[test]
fn the_detail_row_underlines_each_word_of_a_spelling() {
    let buf = page_of("no ver");
    let (y, x) = find(&buf, "--no-verify").unwrap();
    assert_eq!(mask(&buf, y, x, 11), "..uu.uuu...");
}

#[test]
fn the_bracketed_no_of_a_negatable_flag_is_underlined_when_the_no_form_matched() {
    let buf = page_of("no-squ");
    let (y, x) = find(&buf, "--[no-]squash").unwrap();
    assert_eq!(mask(&buf, y, x, 13), "...uuu.uuu...");
    let buf = page_of("--squ");
    let (y, x) = find(&buf, "--[no-]squash").unwrap();
    assert_eq!(mask(&buf, y, x, 13), "uu.....uuu...");
}
