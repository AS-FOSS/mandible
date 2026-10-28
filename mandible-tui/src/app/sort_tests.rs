//! `s` sorts the focused pane: the detail pane in three states, the
//! commands pane in two, and what each keeps (spec §2, §9.1, §9.3, §10).

use super::search_tests::{cmd, prov, selected_name, settle, type_query};
use super::*;
use crate::sort::DetailSort;
use mandible_core::{Entity, Text};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::Terminal;

fn grouped(mut e: Entity, group: &str) -> Entity {
    e.group = Some(group.to_string());
    e
}

fn passed_down(mut e: Entity) -> Entity {
    e.inherited = true;
    e
}

fn long(name: &str) -> Entity {
    Entity::flag_long(name, prov())
}

/// A command whose page has prose, positionals and flags in three kinds of
/// place: ungrouped, in two groups, and inherited.
fn page() -> CommandNode {
    let mut node = cmd("page");
    node.summary = Some(Text::sanitize("Summary line"));
    node.description = Some(Text::sanitize("Prose that stays as written"));
    node.usage = vec![Text::sanitize("tool page [OPTIONS] <zebra> <apple>")];
    node.entities = vec![
        Entity::positional("zebra", prov()),
        Entity::positional("apple", prov()),
        long("zeta"),
        Entity::flag_short('q', prov()),
        grouped(long("yankee"), "Output options"),
        grouped(Entity::flag_short('b', prov()), "Output options"),
        grouped(
            Entity::flag_spelled(Some('z'), Some("Alpha".into()), false, false, prov()),
            "Output options",
        ),
        grouped(
            Entity::flag_spelled(None, Some("verbose".into()), false, true, prov()),
            "Input options",
        ),
        grouped(long("kilo"), "Input options"),
        passed_down(long("zulu")),
        passed_down(long("echo")),
    ];
    node
}

fn tool() -> CommandNode {
    let mut mid = cmd("mid");
    mid.subcommands = vec![cmd("y"), cmd("b")];
    let mut root = cmd("tool");
    root.subcommands = vec![
        cmd("zap"),
        cmd("Beta"),
        cmd("alpha"),
        cmd("Alpha"),
        mid,
        page(),
    ];
    root
}

fn app_on(root: CommandNode, name: &str) -> App {
    let mut app = App::new("tool".to_string(), root);
    app.set_palette(crate::style::Palette::extended());
    app.expanded.insert(vec!["tool".into(), "mid".into()]);
    app.mark_dirty();
    app.ensure_rows_fresh();
    let at = app.rows().iter().position(|r| r.name == name).unwrap();
    app.select_index(at);
    app
}

fn draw(app: &App, cols: u16, rows: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(cols, rows)).unwrap();
    terminal.draw(|f| crate::render::render(f, app)).unwrap();
    terminal.backend().buffer().clone()
}

fn text(buf: &Buffer, y: u16, from: u16) -> String {
    (from..buf.area.width)
        .map(|x| buf[(x, y)].symbol().to_string())
        .collect()
}

/// The detail pane's lines, trimmed.
fn detail(app: &App, cols: u16, rows: u16) -> Vec<String> {
    let buf = draw(app, cols, rows);
    (0..rows)
        .map(|y| {
            text(&buf, y, cols / 2 + 1)
                .trim_matches(['│', ' '])
                .to_string()
        })
        .collect()
}

/// The commands pane's row names, top to bottom.
fn tree_names(app: &App) -> Vec<String> {
    app.rows().iter().map(|r| r.name.clone()).collect()
}

/// `tokens` in the order their lines stand in `lines`.
fn order(lines: &[String], tokens: &[&str]) -> Vec<String> {
    let mut found: Vec<(usize, &str)> = tokens
        .iter()
        .map(|t| {
            let at = lines.iter().position(|l| l.contains(t));
            (
                at.unwrap_or_else(|| panic!("{t} not drawn in {lines:#?}")),
                *t,
            )
        })
        .collect();
    found.sort();
    found.into_iter().map(|(_, t)| t.to_string()).collect()
}

const FLAGS: [&str; 9] = [
    "--zeta",
    "-q",
    "--yankee",
    "-b",
    "--Alpha",
    "--[no-]verbose",
    "--kilo",
    "--zulu",
    "--echo",
];

fn flag_order(app: &App) -> Vec<String> {
    order(&detail(app, 120, 40), &FLAGS)
}

fn sort_detail(app: &mut App) {
    app.focus = Focus::Detail;
    app.sort_focused();
}

#[test]
fn the_detail_pane_cycles_through_three_states() {
    let mut app = app_on(tool(), "page");
    assert_eq!(
        flag_order(&app),
        [
            "--zeta",
            "-q",
            "--yankee",
            "-b",
            "--Alpha",
            "--[no-]verbose",
            "--kilo",
            "--zulu",
            "--echo"
        ]
    );
    sort_detail(&mut app);
    assert_eq!(app.sorts.detail, DetailSort::Sections);
    assert_eq!(
        flag_order(&app),
        [
            "-q",
            "--zeta",
            "--Alpha",
            "-b",
            "--yankee",
            "--kilo",
            "--[no-]verbose",
            "--echo",
            "--zulu"
        ]
    );
    sort_detail(&mut app);
    assert_eq!(app.sorts.detail, DetailSort::Overall);
    assert_eq!(
        flag_order(&app),
        [
            "--Alpha",
            "-b",
            "--kilo",
            "-q",
            "--[no-]verbose",
            "--yankee",
            "--zeta",
            "--echo",
            "--zulu"
        ]
    );
    sort_detail(&mut app);
    assert_eq!(app.sorts.detail, DetailSort::Printed);
    assert_eq!(flag_order(&app)[0], "--zeta");
}

#[test]
fn group_headings_stay_in_sections_and_go_overall() {
    let mut app = app_on(tool(), "page");
    let headings = ["Output options", "Input options", "Inherited"];
    let present = |app: &App| -> Vec<bool> {
        let lines = detail(app, 120, 40);
        headings
            .iter()
            .map(|h| lines.iter().any(|l| l.starts_with(h)))
            .collect()
    };
    assert_eq!(present(&app), [true, true, true]);
    sort_detail(&mut app);
    assert_eq!(present(&app), [true, true, true]);
    sort_detail(&mut app);
    assert_eq!(
        present(&app),
        [false, false, true],
        "the tool's own groups go, the inherited group is a different kind of group"
    );
    let lines = detail(&app, 120, 40);
    let count = lines.iter().find(|l| l.starts_with("FLAGS")).unwrap();
    assert!(count.starts_with("FLAGS (9)"), "{count}");
    let list: Vec<&String> = lines
        .iter()
        .skip_while(|l| !l.starts_with("FLAGS"))
        .skip(1)
        .take_while(|l| !l.starts_with("Inherited"))
        .collect();
    assert!(
        list[..list.len() - 1]
            .iter()
            .all(|l| l.starts_with('-') && !l.is_empty()),
        "one sorted list with nothing between the rows: {list:?}"
    );
    assert_eq!(
        list.len(),
        8,
        "seven rows and the blank row before Inherited"
    );
}

#[test]
fn the_sort_key_is_the_bare_long_name_else_the_short_letter() {
    let mut app = app_on(tool(), "page");
    sort_detail(&mut app);
    sort_detail(&mut app);
    let seen = flag_order(&app);
    let at = |t: &str| seen.iter().position(|s| s == t).unwrap();
    assert!(
        at("--Alpha") < at("-b"),
        "-z, --Alpha sorts as `alpha`, not `z`"
    );
    assert!(at("-b") < at("--kilo"));
    assert!(at("-q") < at("--[no-]verbose") && at("--[no-]verbose") < at("--yankee"));
}

#[test]
fn rows_with_the_same_key_keep_the_tools_order() {
    let mut node = cmd("page");
    node.entities = vec![long("same"), long("Same"), long("alpha"), long("SAME")];
    let mut root = cmd("tool");
    root.subcommands = vec![node];
    let mut app = app_on(root, "page");
    sort_detail(&mut app);
    let lines = detail(&app, 120, 40);
    let names: Vec<&str> = lines
        .iter()
        .filter_map(|l| l.split_whitespace().next())
        .filter(|w| w.starts_with("--"))
        .collect();
    assert_eq!(names, ["--alpha", "--same", "--Same", "--SAME"]);
}

#[test]
fn prose_and_positionals_are_never_sorted() {
    let mut app = app_on(tool(), "page");
    let before = detail(&app, 120, 40);
    let prose = |lines: &[String]| -> Vec<String> {
        lines
            .iter()
            .skip_while(|l| !l.starts_with("Summary"))
            .take_while(|l| !l.starts_with("FLAGS"))
            .cloned()
            .collect()
    };
    for _ in 0..3 {
        sort_detail(&mut app);
        let now = detail(&app, 120, 40);
        assert_eq!(prose(&now), prose(&before));
    }
    let lines = detail(&app, 120, 40);
    let section: Vec<String> = lines
        .iter()
        .skip_while(|l| !l.starts_with("POSITIONALS"))
        .cloned()
        .collect();
    assert_eq!(order(&section, &["zebra", "apple"]), ["zebra", "apple"]);
    assert!(lines
        .iter()
        .any(|l| l.contains("tool page [OPTIONS] <zebra> <apple>")));
}

#[test]
fn the_commands_pane_cycles_through_two_states_under_each_parent() {
    let mut app = app_on(tool(), "tool");
    app.focus = Focus::Tree;
    assert_eq!(
        tree_names(&app),
        ["tool", "zap", "Beta", "alpha", "Alpha", "mid", "y", "b", "page"]
    );
    app.sort_focused();
    assert!(app.sorts.tree);
    assert_eq!(
        tree_names(&app),
        ["tool", "alpha", "Alpha", "Beta", "mid", "b", "y", "page", "zap"],
        "siblings sort under each parent and equal names keep the tool's order"
    );
    app.sort_focused();
    assert!(!app.sorts.tree);
    assert_eq!(tree_names(&app)[1], "zap");
}

#[test]
fn each_pane_keeps_its_own_state() {
    let mut app = app_on(tool(), "page");
    app.focus = Focus::Tree;
    app.sort_focused();
    assert!(app.sorts.tree);
    assert_eq!(app.sorts.detail, DetailSort::Printed);
    sort_detail(&mut app);
    assert!(app.sorts.tree);
    assert_eq!(app.sorts.detail, DetailSort::Sections);
    app.focus = Focus::Tree;
    app.sort_focused();
    assert!(!app.sorts.tree);
    assert_eq!(app.sorts.detail, DetailSort::Sections);
}

#[test]
fn the_state_stays_when_moving_to_another_command() {
    let mut app = app_on(tool(), "page");
    sort_detail(&mut app);
    sort_detail(&mut app);
    app.select_index(0);
    app.select_index(app.rows().iter().position(|r| r.name == "page").unwrap());
    assert_eq!(app.sorts.detail, DetailSort::Overall);
    assert_eq!(flag_order(&app)[0], "--Alpha");
}

#[test]
fn the_selected_command_stays_selected_and_in_view() {
    let mut root = cmd("tool");
    root.subcommands = (0..60).rev().map(|i| cmd(&format!("c{i:02}"))).collect();
    let mut app = app_on(root, "c10");
    draw(&app, 120, 20);
    app.focus = Focus::Tree;
    app.sort_focused();
    assert_eq!(selected_name(&app), "c10");
    let buf = draw(&app, 120, 20);
    let shown = (0..20).any(|y| text(&buf, y, 0).contains("c10"));
    assert!(shown, "the selected row is on screen after the sort");
    app.sort_focused();
    assert_eq!(selected_name(&app), "c10");
}

fn flags_reversed() -> CommandNode {
    let mut node = cmd("page");
    node.entities = (0..80).rev().map(|i| long(&format!("f{i:02}"))).collect();
    let mut root = cmd("tool");
    root.subcommands = vec![node];
    root
}

fn first_flag(lines: &[String]) -> String {
    lines
        .iter()
        .find_map(|l| l.split_whitespace().next().filter(|w| w.starts_with("--f")))
        .unwrap()
        .to_string()
}

#[test]
fn the_row_at_the_top_of_the_pane_stays_at_the_top() {
    let mut app = app_on(flags_reversed(), "page");
    app.focus = Focus::Detail;
    draw(&app, 120, 30);
    for _ in 0..3 {
        app.detail_scroll_down();
    }
    for _ in 0..4 {
        app.detail_page_down();
        draw(&app, 120, 30);
    }
    let top = first_flag(&detail(&app, 120, 30));
    assert_ne!(top, "--f79", "the pane is scrolled");
    for _ in 0..3 {
        app.sort_focused();
        let now = first_flag(&detail(&app, 120, 30));
        assert_eq!(now, top, "state {:?}", app.sorts.detail);
        draw(&app, 120, 30);
    }
}

#[test]
fn the_current_search_match_stays_on_screen() {
    let mut app = App::new("tool".to_string(), flags_reversed());
    app.set_palette(crate::style::Palette::extended());
    app.focus_search();
    type_query(&mut app, "f05");
    app.accept_search();
    draw(&app, 120, 30);
    assert!(detail(&app, 120, 30).iter().any(|l| l.contains("--f05")));
    for _ in 0..3 {
        app.sort_focused();
        let lines = detail(&app, 120, 30);
        assert!(lines.iter().any(|l| l.contains("--f05")), "{lines:#?}");
        draw(&app, 120, 30);
    }
}

fn marked() -> CommandNode {
    let mut node = cmd("page");
    node.entities = vec![long("x-b"), long("x-a"), long("x-c"), long("other")];
    let mut root = cmd("tool");
    root.subcommands = vec![node];
    root
}

fn search_page(sort: DetailSort) -> App {
    let mut app = App::new("tool".to_string(), marked());
    app.sorts.detail = sort;
    app.set_palette(crate::style::Palette::extended());
    app.focus_search();
    type_query(&mut app, "x");
    app.accept_search();
    app
}

fn key_name(key: &mandible_core::FlagKey) -> String {
    match key {
        mandible_core::FlagKey::Long(name) => name.clone(),
        other => format!("{other:?}"),
    }
}

fn walk(app: &mut App, steps: usize) -> Vec<String> {
    let mut seen = Vec::new();
    for _ in 0..steps {
        draw(app, 120, 30);
        seen.push(key_name(app.selected_flag.as_ref().unwrap()));
        app.step_match(true);
    }
    seen
}

#[test]
fn n_and_p_walk_the_page_in_the_order_it_is_drawn() {
    let mut printed = search_page(DetailSort::Printed);
    assert_eq!(walk(&mut printed, 3), ["x-b", "x-a", "x-c"]);
    let mut sorted = search_page(DetailSort::Overall);
    assert_eq!(walk(&mut sorted, 3), ["x-a", "x-b", "x-c"]);
}

#[test]
fn the_first_jump_breaks_a_tie_by_the_order_on_the_page() {
    let printed = search_page(DetailSort::Printed);
    assert_eq!(key_name(printed.selected_flag.as_ref().unwrap()), "x-b");
    let sorted = search_page(DetailSort::Sections);
    assert_eq!(key_name(sorted.selected_flag.as_ref().unwrap()), "x-a");
}

#[test]
fn the_first_jump_breaks_a_tie_by_the_order_in_the_commands_pane() {
    let mut root = cmd("tool");
    root.subcommands = vec![cmd("zed"), cmd("abe")];
    let mut app = App::new("tool".to_string(), root.clone());
    app.focus_search();
    type_query(&mut app, "e");
    assert_eq!(selected_name(&app), "zed");
    let mut app = App::new("tool".to_string(), root);
    app.sorts.tree = true;
    settle(&mut app);
    app.focus_search();
    type_query(&mut app, "e");
    assert_eq!(selected_name(&app), "abe");
}

#[test]
fn sorting_during_a_search_leaves_the_match_where_it_was() {
    let mut app = search_page(DetailSort::Printed);
    let before = app.selected_flag.clone();
    let path = app.selected_path();
    for _ in 0..3 {
        sort_detail(&mut app);
        assert_eq!(app.selected_flag, before);
        assert_eq!(app.selected_path(), path);
    }
}

#[test]
fn the_title_names_the_state_and_the_ascii_set_spells_it_with_a_hyphen() {
    let mut app = app_on(tool(), "page");
    let title = |app: &App| -> String {
        let buf = draw(app, 120, 30);
        text(&buf, 3, 60)
    };
    assert!(!title(&app).contains("A–Z"));
    sort_detail(&mut app);
    assert!(
        title(&app).contains("page · A–Z in sections"),
        "{}",
        title(&app)
    );
    sort_detail(&mut app);
    let overall = title(&app);
    assert!(overall.contains("page · A–Z ") && !overall.contains("sections"));
    app.glyphs = crate::glyphs::ASCII;
    let ascii = title(&app);
    assert!(ascii.contains("A-Z") && !ascii.contains('–'), "{ascii}");
    assert!(ascii.is_ascii());
}

#[test]
fn the_commands_title_names_the_state() {
    let mut app = app_on(tool(), "tool");
    app.focus = Focus::Tree;
    let title = |app: &App| text(&draw(app, 120, 30), 3, 0);
    assert!(!title(&app).contains("A–Z"));
    app.sort_focused();
    assert!(
        title(&app).contains("commands (9) · A–Z"),
        "{}",
        title(&app)
    );
}

/// The detail pane's top border, from its corner.
fn border(app: &App, cols: u16, rows: u16) -> String {
    let buf = draw(app, cols, rows);
    let top: Vec<String> = (0..rows)
        .map(|y| text(&buf, y, 0))
        .filter(|l| l.starts_with('╭'))
        .collect();
    let line = top.last().unwrap();
    let start = line.rfind('╭').unwrap();
    line[start..].to_string()
}

#[test]
fn the_indicator_goes_before_the_title_is_cut_at_every_width() {
    let mut root = cmd("tool");
    root.subcommands = vec![page()];
    for state in [DetailSort::Sections, DetailSort::Overall] {
        for cols in 30..=120u16 {
            let mut app = app_on(root.clone(), "page");
            app.focus = Focus::Detail;
            let plain = border(&app, cols, 12);
            app.sorts.detail = state;
            let sorted = border(&app, cols, 12);
            let tag = state.indicator(app.glyphs).unwrap();
            if sorted.contains("A–Z") {
                assert!(sorted.contains(&format!("· {tag} ")), "{cols}: {sorted}");
                let head = sorted.split(" · ").next().unwrap();
                assert!(plain.starts_with(head), "{cols}: {plain} / {sorted}");
            } else {
                assert_eq!(sorted, plain, "{cols}: the title is cut as it was");
            }
        }
    }
}

#[test]
fn the_indicator_fits_or_is_gone_at_the_smallest_size() {
    let mut root = cmd("tool");
    root.subcommands = vec![page()];
    let mut app = app_on(root, "page");
    app.focus = Focus::Detail;
    app.sorts.detail = DetailSort::Sections;
    let sections = border(&app, 30, 10);
    assert!(!sections.contains('A') || sections.contains("A–Z in sections"));
    assert!(!sections.contains("A–Z"), "{sections}");
    app.sorts.detail = DetailSort::Overall;
    let overall = border(&app, 30, 10);
    assert!(overall.contains("· A–Z"), "{overall}");
    assert_eq!(overall.chars().count(), 30);
}

#[test]
fn a_view_with_nothing_to_sort_says_so_and_changes_nothing() {
    let mut app = app_on(tool(), "page");
    app.raw_mode = true;
    sort_detail(&mut app);
    assert_eq!(app.sorts.detail, DetailSort::Printed);
    assert!(app.status_message.is_some());
}

#[test]
fn s_in_either_case_sorts() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = app_on(tool(), "page");
    app.focus = Focus::Detail;
    for c in ['s', 'S'] {
        crate::event::handle_key(
            &mut app,
            KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
        );
    }
    assert_eq!(app.sorts.detail, DetailSort::Overall);
}

#[test]
fn the_help_overlay_and_a_wide_footer_name_the_key() {
    let mut app = app_on(tool(), "page");
    app.show_help = true;
    let buf = draw(&app, 140, 50);
    let key_of_the_row = (0..50).find_map(|y| {
        let row = text(&buf, y, 0);
        let at = row.find("Sort the focused pane")?;
        let before: Vec<char> = row[..at].chars().collect();
        Some(before[before.len() - 15..].iter().collect::<String>())
    });
    assert_eq!(key_of_the_row.as_deref().map(str::trim), Some("s"));
    app.show_help = false;
    let wide = draw(&app, 200, 30);
    assert!((0..30).any(|y| text(&wide, y, 0).contains("s sort")));
}

#[test]
fn a_row_scrolled_into_stays_scrolled_into_by_the_same_lines() {
    let mut node = cmd("page");
    node.entities = (0..30)
        .rev()
        .map(|i| {
            let mut e = long(&format!("f{i:02}"));
            e.description = Some(Text::sanitize(&format!(
                "a description long enough that it wraps onto several lines of the pane ends-{i:02}"
            )));
            e
        })
        .collect();
    let mut root = cmd("tool");
    root.subcommands = vec![node];
    let mut app = app_on(root, "page");
    app.focus = Focus::Detail;
    draw(&app, 100, 20);
    app.detail_page_down();
    let token = |app: &App| -> String {
        let lines = detail(app, 100, 20);
        let top = lines.iter().find(|l| l.contains("ends-")).unwrap();
        top.rsplit("ends-").next().unwrap().to_string()
    };
    let was = token(&app);
    for _ in 0..3 {
        app.sort_focused();
        assert_eq!(token(&app), was, "state {:?}", app.sorts.detail);
        draw(&app, 100, 20);
    }
}
