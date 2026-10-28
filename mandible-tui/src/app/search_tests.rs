//! One result list, dashed spellings, the no-match notice, and the two
//! search states (spec §10).

use super::*;
use mandible_core::{Entity, Provenance, Source, Text};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

pub(super) fn prov() -> Provenance {
    Provenance::single(Source::HelpText)
}

pub(super) fn cmd(name: &str) -> CommandNode {
    let mut node = CommandNode::new(name, prov());
    node.children_filled = true;
    node
}

pub(super) fn with_flag(mut node: CommandNode, long: &str) -> CommandNode {
    node.entities.push(Entity::flag_long(long, prov()));
    node
}

/// A git-shaped tree in which every command a query could jump to sits
/// after a command that also matches it.
fn git() -> CommandNode {
    let mut root = cmd("git");
    let mut bisect = cmd("bisect");
    bisect.entities.push(Entity::positional("rev", prov()));
    bisect.entities.push(Entity::positional("good", prov()));
    let mut squash_only = cmd("squash");
    squash_only.summary = Some(Text::sanitize("A command named like the flag"));
    root.subcommands = vec![
        cmd("add"),
        bisect,
        with_flag(cmd("commit"), "squash"),
        with_flag(cmd("merge"), "squash"),
        cmd("merge-file"),
        with_flag(cmd("pull"), "squash"),
        with_flag(cmd("submodule"), "rebase"),
        cmd("rebase"),
        cmd("reset"),
        cmd("restore"),
        squash_only,
        with_flag(cmd("status"), "short"),
    ];
    let mut s = Entity::flag_short('s', prov());
    s.description = Some(Text::sanitize("short status"));
    root.subcommands[11].entities.push(s);
    root
}

fn names(app: &App) -> Vec<String> {
    app.rows().iter().map(|r| r.name.clone()).collect()
}

pub(super) fn settle(app: &mut App) {
    app.flush_search_index();
    app.mark_dirty();
    app.ensure_rows_fresh();
}

pub(super) fn type_query(app: &mut App, query: &str) {
    for c in query.chars() {
        app.search_input_char(c);
    }
    settle(app);
}

fn searched(query: &str) -> App {
    let mut app = App::new("git".to_string(), git());
    app.focus_search();
    type_query(&mut app, query);
    app
}

pub(super) fn selected_name(app: &App) -> String {
    app.selected_row().unwrap().name.clone()
}

fn screen(app: &App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal.draw(|f| crate::render::render(f, app)).unwrap();
    let buf = terminal.backend().buffer().clone();
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_command_named_like_the_query_beats_a_flag_that_contains_it() {
    let app = searched("rebase");
    assert_eq!(selected_name(&app), "rebase");
    assert_eq!(app.selected_flag, None);
}

#[test]
fn a_prefix_query_lands_on_a_command_with_that_prefix() {
    let app = searched("re");
    let name = selected_name(&app);
    assert!(name.starts_with("re"), "{name}");
    assert_eq!(app.selected_flag, None, "bisect's `rev` must not win");
}

#[test]
fn a_command_comes_before_an_entity_of_the_same_tier() {
    let app = searched("squash");
    assert_eq!(selected_name(&app), "squash");
}

#[test]
fn wide_mode_jumps_to_the_first_row_it_shows() {
    let mut app = App::new("git".to_string(), git());
    app.focus_search();
    app.cycle_search_mode();
    type_query(&mut app, "rebase");
    let shown: Vec<String> = names(&app);
    assert!(shown.contains(&selected_name(&app)));
    assert!(shown.contains(&"rebase".to_string()));
}

#[test]
fn a_dash_is_an_ordinary_character_of_the_query() {
    let app = searched("--squash");
    let shown = names(&app);
    for name in ["commit", "merge", "pull"] {
        assert!(shown.contains(&name.to_string()), "{name} in {shown:?}");
    }
    assert!(!shown.contains(&"squash".to_string()), "{shown:?}");
    assert!(!shown.contains(&"bisect".to_string()), "{shown:?}");
    assert_eq!(selected_name(&app), "commit");
    assert!(app.selected_flag.is_some());
}

#[test]
fn a_short_flag_ranks_first_and_long_flags_with_that_initial_follow() {
    let app = searched("-s");
    let shown = names(&app);
    for name in ["status", "commit", "merge", "pull"] {
        assert!(shown.contains(&name.to_string()), "{name} in {shown:?}");
    }
    assert!(!shown.contains(&"add".to_string()), "{shown:?}");
    assert_eq!(selected_name(&app), "status", "the exact -s comes first");
    assert_eq!(app.selected_flag, Some(mandible_core::FlagKey::Short('s')));
}

#[test]
fn a_dash_led_query_finds_flags_by_containing_it() {
    for query in ["-squa", "--squa"] {
        let app = searched(query);
        let shown = names(&app);
        for name in ["commit", "merge", "pull"] {
            assert!(shown.contains(&name.to_string()), "{query}: {shown:?}");
        }
    }
}

#[test]
fn two_dashes_are_not_found_in_a_single_dash_flag() {
    let app = searched("--s");
    let shown = names(&app);
    assert!(shown.contains(&"commit".to_string()), "{shown:?}");
    assert_ne!(
        app.selected_flag,
        Some(mandible_core::FlagKey::Short('s')),
        "the short -s has one dash"
    );
}

#[test]
fn a_dash_led_query_finds_a_later_word_only_with_the_dashes_it_has() {
    let mut root = cmd("tool");
    root.subcommands = vec![with_flag(cmd("clone"), "no-squash"), cmd("run")];
    let mut app = App::new("tool".to_string(), root);
    app.focus_search();
    type_query(&mut app, "-squ");
    assert_eq!(selected_name(&app), "clone");
    app.cancel_search();
    app.focus_search();
    type_query(&mut app, "--squ");
    assert!(
        app.no_match(),
        "only one dash stands before squash in --no-squash"
    );
}

/// Flags whose index order runs the other way from their page order for the
/// query `--no`: the first one is in a group, which the page draws last.
fn no_flags() -> CommandNode {
    let mut curl = cmd("curl");
    for name in ["no-alpn", "no-buffer", "no-keepalive", "no-npn"] {
        curl.entities.push(Entity::flag_long(name, prov()));
    }
    curl.entities[0].group = Some("Extras".to_string());
    let mut root = cmd("tool");
    root.subcommands = vec![curl];
    root
}

fn first_flag(hits: &[NodeRef]) -> Option<mandible_core::FlagKey> {
    hits.iter().find_map(|h| match h {
        NodeRef::Flag { key, .. } => Some(key.clone()),
        NodeRef::Command(_) => None,
    })
}

fn long(name: &str) -> Option<mandible_core::FlagKey> {
    Some(mandible_core::FlagKey::Long(name.into()))
}

#[test]
fn the_jump_lands_on_the_top_matching_flag_of_the_page_in_name_mode() {
    let mut app = App::new("tool".to_string(), no_flags());
    app.focus_search();
    type_query(&mut app, "--no");
    let index_first = first_flag(&app.search_index.results());
    assert_eq!(
        index_first,
        long("no-alpn"),
        "precondition: index order disagrees"
    );
    assert_eq!(app.selected_flag, long("no-buffer"));
}

#[test]
fn the_jump_lands_on_the_top_matching_flag_of_the_page_in_wide_mode() {
    let mut app = App::new("tool".to_string(), no_flags());
    app.focus_search();
    app.cycle_search_mode();
    type_query(&mut app, "--no");
    let index_first = first_flag(&app.search_index.results());
    assert_eq!(
        index_first,
        long("no-alpn"),
        "precondition: index order disagrees"
    );
    assert_eq!(app.selected_flag, long("no-buffer"));
}

#[test]
fn a_better_tier_still_beats_the_top_of_the_page() {
    let mut curl = cmd("curl");
    curl.entities.push(Entity::flag_long("no-s", prov()));
    curl.entities.push(Entity::flag_short('s', prov()));
    let mut root = cmd("tool");
    root.subcommands = vec![curl];
    let mut app = App::new("tool".to_string(), root);
    app.focus_search();
    type_query(&mut app, "-s");
    assert_eq!(app.selected_flag, Some(mandible_core::FlagKey::Short('s')));
}

#[test]
fn an_exact_case_name_beats_the_same_letter_in_the_other_case() {
    let mut curl = cmd("curl");
    curl.entities.push(Entity::flag_short('S', prov()));
    curl.entities.push(Entity::flag_short('s', prov()));
    let mut root = cmd("tool");
    root.subcommands = vec![curl];
    let mut app = App::new("tool".to_string(), root);
    app.focus_search();
    type_query(&mut app, "-s");
    assert_eq!(app.selected_flag, Some(mandible_core::FlagKey::Short('s')));
}

#[test]
fn a_plain_word_finds_the_middle_of_a_flag_name() {
    let mut root = cmd("docker");
    root.subcommands = vec![with_flag(cmd("ps"), "no-trunc"), cmd("build")];
    let mut app = App::new("docker".to_string(), root);
    app.focus_search();
    type_query(&mut app, "run");
    assert!(!app.no_match());
    assert_eq!(selected_name(&app), "ps");
    assert_eq!(app.selected_flag, long("no-trunc"));
    assert!(!names(&app).contains(&"build".to_string()));
}

fn curl_like() -> CommandNode {
    let mut root = cmd("curl");
    root.entities.push(Entity::flag_long("ssl-reqd", prov()));
    root.entities.push(Entity::flag_long("ssl", prov()));
    let mut scattered = Entity::flag_long("use-ascii-lines", prov());
    scattered.description = Some(Text::sanitize("Use the stream slowly"));
    root.entities.push(scattered);
    root
}

#[test]
fn wide_mode_takes_connected_text_only() {
    let mut app = App::new("curl".to_string(), curl_like());
    app.focus_search();
    app.cycle_search_mode();
    type_query(&mut app, "-ssl");
    assert!(!app.no_match());
    assert_eq!(app.selected_flag, long("ssl-reqd"), "the top of the page");
    app.cancel_search();
    app.focus_search();
    type_query(&mut app, "s-s-l");
    assert!(app.no_match(), "dashes and letters from three places");
}

#[test]
fn wide_mode_shows_the_notice_for_letters_found_only_in_scattered_places() {
    let mut app = App::new("git".to_string(), git());
    app.focus_search();
    app.cycle_search_mode();
    type_query(&mut app, "-ssl");
    assert!(app.no_match(), "{:?}", names(&app));
    assert!(screen(&app).contains("No matches for \"-ssl\""));
}

#[test]
fn wide_mode_wants_each_word_of_the_query() {
    let mut app = App::new("git".to_string(), git());
    app.focus_search();
    app.cycle_search_mode();
    type_query(&mut app, "short status");
    assert!(!app.no_match());
    type_query(&mut app, " zzz");
    assert!(app.no_match());
}

#[test]
fn a_dashed_prefix_works_as_a_prefix_of_the_spelling() {
    let mut root = cmd("git");
    root.subcommands = vec![with_flag(cmd("clone"), "no-checkout"), cmd("run")];
    let mut app = App::new("git".to_string(), root);
    app.focus_search();
    type_query(&mut app, "--no-");
    assert_eq!(selected_name(&app), "clone");
    app.cancel_search();
    app.focus_search();
    type_query(&mut app, "--run");
    assert!(app.no_match());
}

#[test]
fn no_match_shows_a_notice_and_the_hidden_cursor_does_not_move() {
    let mut app = App::new("git".to_string(), git());
    app.move_down();
    let before = app.selected_path();
    app.focus_search();
    type_query(&mut app, "re");
    app.search_input_char('x');
    settle(&mut app);
    assert!(app.no_match());
    assert_eq!(app.selected_path(), before);
    let row = app.selected;
    let text = screen(&app);
    assert!(text.contains("No matches for \"rex\""), "{text}");
    assert!(text.contains("commands (0)"), "{text}");
    app.move_down();
    assert_eq!(app.selected, row, "the hidden cursor does not move");
    app.search_backspace();
    settle(&mut app);
    assert!(!app.no_match());
    assert!(selected_name(&app).starts_with("re"));
}

#[test]
fn a_query_that_matches_nothing_from_the_start_keeps_the_tree_behind_the_notice() {
    let mut app = App::new("git".to_string(), git());
    let rows = names(&app);
    app.focus_search();
    type_query(&mut app, "zzqq");
    assert!(app.no_match());
    assert_eq!(names(&app), rows);
    assert_eq!(app.selected, 0);
    assert!(screen(&app).contains("No matches for \"zzqq\""));
}

#[test]
fn typing_never_flashes_the_unfiltered_tree_or_the_notice() {
    let mut app = App::new("git".to_string(), git());
    app.focus_search();
    for c in "rebase".chars() {
        app.search_input_char(c);
        app.ensure_rows_fresh();
        assert!(!app.no_match());
        assert!(app.rows().len() < 13, "{:?}", names(&app));
        assert!(!screen(&app).contains("No matches"));
    }
    assert_eq!(selected_name(&app), "rebase");
}

#[test]
fn a_rebuilt_index_keeps_the_filtered_rows() {
    let mut app = searched("rebase");
    let rows = names(&app);
    let selected = app.selected;
    app.rebuild_search_index(Instant::now());
    app.ensure_rows_fresh();
    assert!(!app.no_match());
    assert_eq!(names(&app), rows, "the same rows");
    assert_eq!(app.selected, selected);
}

#[test]
fn enter_jumps_to_the_first_match_and_focuses_the_list() {
    let mut app = searched("re");
    app.move_down();
    let moved = selected_name(&app);
    app.accept_search();
    assert_eq!(app.focus, Focus::Tree);
    assert_eq!(app.active_filter(), Some("re"));
    assert_ne!(selected_name(&app), moved, "back on the first match");
    assert!(selected_name(&app).starts_with("re"));
}

#[test]
fn esc_while_typing_cancels_and_returns_the_cursor() {
    let mut app = App::new("git".to_string(), git());
    for _ in 0..5 {
        app.move_down();
    }
    let before = app.selected_path();
    app.focus_search();
    type_query(&mut app, "rebase");
    assert_ne!(app.selected_path(), before);
    app.cancel_search();
    assert_eq!(app.focus, Focus::Tree);
    assert!(app.active_filter().is_none());
    assert_eq!(app.search_query, "");
    assert_eq!(app.selected_path(), before);
    assert_eq!(app.rows().len(), 13);
}

#[test]
fn esc_in_the_filtered_list_clears_and_keeps_the_row() {
    let mut app = searched("rebase");
    app.accept_search();
    let here = app.selected_path();
    assert_eq!(here.as_ref().map(|p| p.len()), Some(2));
    app.clear_filter();
    assert!(app.active_filter().is_none());
    assert_eq!(app.selected_path(), here, "the cursor stays on its row");
    assert_eq!(app.focus, Focus::Tree);
}

#[test]
fn esc_in_the_filtered_list_opens_a_collapsed_parent_to_keep_the_row() {
    let mut root = cmd("tool");
    let mut group = cmd("group");
    group.subcommands = vec![cmd("deep")];
    root.subcommands = vec![group, cmd("other")];
    let mut app = App::new("tool".to_string(), root);
    app.focus_search();
    type_query(&mut app, "deep");
    app.accept_search();
    assert_eq!(selected_name(&app), "deep");
    app.clear_filter();
    assert_eq!(selected_name(&app), "deep");
}

#[test]
fn slash_in_the_filtered_list_reopens_with_the_keyword() {
    let mut app = searched("rebase");
    app.accept_search();
    app.focus_search();
    assert_eq!(app.focus, Focus::Search);
    assert_eq!(app.search_query, "rebase");
    assert!(screen(&app).contains("rebase"));
    app.search_backspace();
    app.search_input_char('e');
    settle(&mut app);
    app.accept_search();
    assert_eq!(selected_name(&app), "rebase");
    assert_eq!(app.focus, Focus::Tree);
}

#[test]
fn esc_in_a_reopened_box_cancels_to_the_row_it_was_on() {
    let mut app = searched("re");
    app.accept_search();
    app.move_down();
    let before = app.selected_path();
    app.focus_search();
    app.search_input_char('s');
    settle(&mut app);
    app.cancel_search();
    assert!(app.active_filter().is_none());
    assert_eq!(app.selected_path(), before);
}

#[test]
fn the_bar_shows_the_keyword_in_both_states_and_the_marker_only_while_typing() {
    let bar = |app: &App| screen(app).lines().nth(1).unwrap_or("").to_string();
    let mut app = searched("rebase");
    let typing = bar(&app);
    assert!(
        typing.contains("› rebase") || typing.contains("> rebase"),
        "{typing}"
    );
    app.accept_search();
    let filtered = bar(&app);
    assert!(filtered.contains("rebase"), "{filtered}");
    assert!(
        !filtered.contains('›') && !filtered.contains('>'),
        "{filtered}"
    );
    assert!(!filtered.contains("pinned") && !filtered.contains("(filter)"));
}

#[test]
fn a_shallower_match_beats_a_nested_one_of_the_same_tier() {
    let mut root = cmd("tool");
    let mut group = cmd("group");
    group.subcommands = vec![cmd("reset")];
    root.subcommands = vec![group, cmd("rebase")];
    let mut app = App::new("tool".to_string(), root);
    app.focus_search();
    type_query(&mut app, "re");
    assert_eq!(selected_name(&app), "rebase");
}

#[test]
fn equal_matches_jump_to_the_first_in_display_order() {
    let app = searched("--squash");
    assert_eq!(selected_name(&app), "commit");
}

#[test]
fn no_match_while_typing_returns_to_the_row_from_before_the_slash() {
    let mut app = App::new("git".to_string(), git());
    for _ in 0..3 {
        app.move_down();
    }
    let before = app.selected_path();
    app.focus_search();
    type_query(&mut app, "re");
    assert_ne!(app.selected_path(), before, "precondition: the jump moved");
    app.search_input_char('z');
    settle(&mut app);
    assert!(app.no_match());
    assert_eq!(app.selected_path(), before);
    app.search_backspace();
    settle(&mut app);
    assert!(!app.no_match());
    assert!(selected_name(&app).starts_with("re"), "the jump resumes");
}

/// Two matches of one tier: the one the tree shows first wins.
#[test]
fn the_display_order_breaks_a_tie() {
    let mut root = cmd("tool");
    root.subcommands = vec![cmd("gre"), cmd("xRe")];
    let mut app = App::new("tool".to_string(), root);
    app.focus_search();
    type_query(&mut app, "re");
    assert_eq!(selected_name(&app), "gre");
}
