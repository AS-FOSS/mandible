//! The notice on a loaded page that holds nothing past its summary.

use super::*;
use crate::probes::{FillEnd, Outcome};
use mandible_core::notice::{NO_FURTHER_HELP_NOTICE, READING_NOTICE, TIMED_OUT_NOTICE};
use mandible_core::{Provenance, Source, Text};
use mandible_extract::KeptProbes;
use ratatui::backend::TestBackend;
use ratatui::style::Color;
use ratatui::Terminal;

fn child(summary: Option<&str>) -> CommandNode {
    let mut node = CommandNode::new("start", Provenance::single(Source::HelpText));
    node.summary = summary.map(Text::sanitize);
    node
}

/// A tool whose one child is `child`, with the child selected.
fn app_over(child: CommandNode) -> App {
    let mut root = CommandNode::new("tool", Provenance::single(Source::HelpText));
    root.children_filled = true;
    root.subcommands.push(child);
    let mut app = App::new("tool".to_string(), root);
    app.select_index(1);
    app
}

fn screen(app: &App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal
        .draw(|frame| {
            let area = frame.area();
            render(frame, area, app);
        })
        .unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect()
}

fn filled(mut c: CommandNode) -> CommandNode {
    c.children_filled = true;
    c
}

fn has_notice(app: &App) -> bool {
    screen(app).contains("No further help was read")
}

#[test]
fn a_filled_page_with_only_a_summary_gets_the_notice() {
    let app = app_over(filled(child(Some("Runs a script"))));
    let text = screen(&app);
    assert!(text.contains("Runs a script"), "{text}");
    assert!(has_notice(&app), "{text}");
    assert!(NO_FURTHER_HELP_NOTICE.contains("Press t"));
}

#[test]
fn a_page_with_no_summary_gets_it_too() {
    assert!(has_notice(&app_over(filled(child(None)))));
}

#[test]
fn any_one_section_removes_it() {
    let mut cases: Vec<CommandNode> = Vec::new();
    let mut n = filled(child(Some("s")));
    n.description = Some(Text::sanitize("long text"));
    cases.push(n);
    let mut n = filled(child(Some("s")));
    n.usage.push(Text::sanitize("tool start [args]"));
    cases.push(n);
    let mut n = filled(child(Some("s")));
    n.entities.push(Entity::new(
        EntityKind::Flag,
        Provenance::single(Source::HelpText),
    ));
    n.entities[0]
        .spellings
        .push(mandible_core::Spelling::long("x"));
    cases.push(n);
    let mut n = filled(child(Some("s")));
    n.examples
        .push(mandible_core::Example::new(Text::sanitize("tool start")));
    cases.push(n);
    for node in cases {
        assert!(!has_notice(&app_over(node)));
    }
}

#[test]
fn a_same_as_parent_page_keeps_only_its_own_notice() {
    let mut n = filled(child(Some("s")));
    n.same_as_ancestor = true;
    assert!(!has_notice(&app_over(n)));
}

#[test]
fn the_root_never_gets_it() {
    let mut root = CommandNode::new("tool", Provenance::single(Source::HelpText));
    root.children_filled = true;
    root.summary = Some(Text::sanitize("A tool"));
    assert!(!has_notice(&App::new("tool".to_string(), root)));
}

#[test]
fn a_node_still_loading_gets_none() {
    let mut app = app_over(child(Some("Runs a script")));
    app.mark_pending(vec!["tool".to_string(), "start".to_string()]);
    assert!(!has_notice(&app));
}

#[test]
fn past_the_cap_a_never_probed_node_gets_none() {
    let app = app_over(child(Some("Runs a script")));
    assert!(!has_notice(&app));
}

#[test]
fn a_failed_fill_gets_it_and_the_cached_page_is_replaced() {
    let mut app = app_over(child(Some("Runs a script")));
    let path = vec!["tool".to_string(), "start".to_string()];
    app.mark_pending(path.clone());
    assert!(!has_notice(&app));
    // The probe failed: the same bare stub comes back.
    app.splice_filled_node(&path, child(Some("Runs a script")));
    assert!(has_notice(&app));
}

#[test]
fn a_refill_in_flight_drops_the_notice_from_the_cached_page() {
    let mut app = app_over(filled(child(Some("Runs a script"))));
    assert!(has_notice(&app));
    app.mark_pending(vec!["tool".to_string(), "start".to_string()]);
    assert!(!has_notice(&app));
}

#[test]
fn the_corpus_page_counts_only_a_probed_node() {
    let stub = child(Some("Runs a script"));
    let probed = filled(child(Some("Runs a script")));
    let notice = |n: &CommandNode| {
        page_text(n, 76, false)
            .unwrap()
            .iter()
            .any(|l| l.contains("No further help"))
    };
    assert!(!notice(&stub));
    assert!(notice(&probed));
    assert!(!page_text(&probed, 76, true)
        .unwrap()
        .iter()
        .any(|l| l.contains("No further help")));
}

fn start() -> Vec<String> {
    vec!["tool".to_string(), "start".to_string()]
}

fn finish(app: &mut App, node: CommandNode, end: FillEnd) {
    app.finish_fill(&start(), node, end, KeptProbes::default());
}

fn has_timed_out_notice(app: &App) -> bool {
    screen(app).contains("took longer than 30 seconds")
}

#[test]
fn the_timed_out_notice_names_the_cap_the_probes_run_under() {
    let secs = mandible_extract::exec::EXTRACT_LIMIT.as_secs();
    assert!(TIMED_OUT_NOTICE.contains(&format!("longer than {secs} seconds")));
    assert!(TIMED_OUT_NOTICE.contains("Press r to try again with more time."));
}

#[test]
fn a_probe_stopped_at_the_limit_shows_its_notice_and_not_the_empty_one() {
    let mut app = app_over(child(Some("Runs a script")));
    finish(&mut app, child(Some("Runs a script")), FillEnd::TimedOut);
    let text = screen(&app);
    assert!(text.contains("Runs a script"), "{text}");
    assert!(has_timed_out_notice(&app), "{text}");
    assert!(!has_notice(&app), "two notices on one page: {text}");
}

#[test]
fn an_empty_read_shows_the_empty_notice_and_not_the_timed_out_one() {
    let mut app = app_over(child(Some("Runs a script")));
    finish(&mut app, child(Some("Runs a script")), FillEnd::Read);
    assert!(has_notice(&app));
    assert!(!has_timed_out_notice(&app));
    assert_eq!(app.probe_outcome(&start()), Some(Outcome::Empty));
}

#[test]
fn a_same_as_parent_page_keeps_its_own_notice_over_a_timed_out_one() {
    let mut node = child(Some("Runs a script"));
    node.same_as_ancestor = true;
    let mut app = app_over(node.clone());
    finish(&mut app, node, FillEnd::TimedOut);
    let text = screen(&app);
    assert!(
        text.contains("prints the same help as its parent"),
        "{text}"
    );
    assert!(!has_timed_out_notice(&app), "{text}");
    assert!(!has_notice(&app));
}

#[test]
fn a_timed_out_notice_is_replaced_when_a_later_read_succeeds() {
    let mut app = app_over(child(Some("Runs a script")));
    finish(&mut app, child(Some("Runs a script")), FillEnd::TimedOut);
    assert!(has_timed_out_notice(&app));
    let mut read = child(Some("Runs a script"));
    read.description = Some(Text::sanitize("Does the work."));
    finish(&mut app, read, FillEnd::Read);
    assert!(!has_timed_out_notice(&app));
    assert!(!has_notice(&app));
    assert_eq!(app.probe_outcome(&start()), Some(Outcome::Parsed));
}

#[test]
fn a_re_read_in_flight_drops_the_timed_out_notice_from_the_cached_page() {
    let mut app = app_over(child(Some("Runs a script")));
    finish(&mut app, child(Some("Runs a script")), FillEnd::TimedOut);
    assert!(has_timed_out_notice(&app));
    app.mark_pending(start());
    assert!(!has_timed_out_notice(&app));
}

#[test]
fn the_timed_out_notice_wraps_at_the_pane_width() {
    let mut app = app_over(child(Some("Runs a script")));
    finish(&mut app, child(Some("Runs a script")), FillEnd::TimedOut);
    let mut terminal = Terminal::new(TestBackend::new(40, 24)).unwrap();
    terminal
        .draw(|frame| {
            let area = frame.area();
            render(frame, area, &app);
        })
        .unwrap();
    let rows: Vec<String> = terminal
        .backend()
        .buffer()
        .content()
        .chunks(40)
        .map(|r| r.iter().map(|c| c.symbol()).collect::<String>())
        .collect();
    let joined = rows.join("\n");
    assert!(joined.contains("Press r to try again"), "{joined}");
    assert!(rows.iter().all(|r| r.chars().count() <= 40));
}

fn has_reading_line(app: &App) -> bool {
    screen(app).contains(READING_NOTICE)
}

#[test]
fn a_first_read_in_flight_shows_the_loading_line_under_the_summary() {
    let mut app = app_over(child(Some("Runs a script")));
    assert!(!has_reading_line(&app));
    app.mark_pending(start());
    let text = screen(&app);
    assert!(text.contains("Runs a script"), "{text}");
    assert!(text.contains(READING_NOTICE), "{text}");
    app.mark_running(&start());
    assert!(has_reading_line(&app));
}

#[test]
fn the_startup_root_shows_the_loading_line() {
    let root = CommandNode::new("tool", Provenance::single(Source::HelpText));
    let mut app = App::new("tool".to_string(), root);
    app.mark_pending(vec!["tool".to_string()]);
    assert!(has_reading_line(&app));
}

#[test]
fn a_re_read_keeps_the_old_page_without_the_loading_line() {
    let mut app = app_over(child(Some("Runs a script")));
    finish(&mut app, child(Some("Runs a script")), FillEnd::TimedOut);
    app.mark_pending(start());
    assert!(!has_reading_line(&app));
    let mut read = child(Some("Runs a script"));
    read.description = Some(Text::sanitize("Does the work."));
    finish(&mut app, read, FillEnd::Read);
    app.mark_pending(start());
    assert!(!has_reading_line(&app));
}

#[test]
fn a_loaded_page_has_no_loading_line() {
    let mut app = app_over(child(Some("Runs a script")));
    app.mark_pending(start());
    finish(&mut app, child(Some("Runs a script")), FillEnd::Read);
    assert!(!has_reading_line(&app));
    let mut read = child(Some("Runs a script"));
    read.description = Some(Text::sanitize("Does the work."));
    let mut app = app_over(read);
    app.mark_pending(start());
    assert!(!has_reading_line(&app));
}

#[test]
fn the_loading_line_gives_way_to_every_other_notice() {
    let mut node = child(Some("Runs a script"));
    node.same_as_ancestor = true;
    let mut app = app_over(node);
    app.mark_pending(start());
    let text = screen(&app);
    assert!(
        text.contains("prints the same help as its parent"),
        "{text}"
    );
    assert!(!text.contains(READING_NOTICE), "{text}");

    let mut app = app_over(child(Some("Runs a script")));
    app.mark_pending(start());
    let text = screen(&app);
    assert!(!has_notice(&app) && !has_timed_out_notice(&app), "{text}");
}

#[test]
fn the_loading_line_wraps_like_the_other_notices() {
    let mut app = app_over(child(Some("Runs a script")));
    app.mark_pending(start());
    let mut terminal = Terminal::new(TestBackend::new(24, 24)).unwrap();
    terminal
        .draw(|frame| {
            let area = frame.area();
            render(frame, area, &app);
        })
        .unwrap();
    let rows: Vec<String> = terminal
        .backend()
        .buffer()
        .content()
        .chunks(24)
        .map(|r| r.iter().map(|c| c.symbol()).collect::<String>())
        .collect();
    let joined = rows.join("\n");
    assert!(joined.contains("help"), "{joined}");
    assert!(!joined.contains(READING_NOTICE), "{joined}");
}

/// The style of the first cell of the row of `app`'s page that holds `needle`.
fn style_of(app: &App, needle: &str) -> Style {
    let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
    terminal
        .draw(|frame| {
            let area = frame.area();
            render(frame, area, app);
        })
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    for y in 0..buffer.area.height {
        let row: String = (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect();
        if let Some(at) = row.find(needle) {
            let x = row[..at].chars().count() as u16;
            return buffer[(x, y)].style();
        }
    }
    panic!("no row holds {needle:?}");
}

#[test]
fn the_problem_notices_are_drawn_in_the_warning_style() {
    for palette in [style::Palette::basic(), style::Palette::plain()] {
        let warning = style::warning(palette);
        let mut timed_out = app_over(child(Some("Runs a script")));
        timed_out.palette = palette;
        finish(
            &mut timed_out,
            child(Some("Runs a script")),
            FillEnd::TimedOut,
        );
        let got = style_of(&timed_out, "Reading this command's help took");
        assert_eq!(
            (got.fg, got.add_modifier),
            (warning.fg.or(Some(Color::Reset)), warning.add_modifier)
        );

        let mut bare = app_over(filled(child(Some("Runs a script"))));
        bare.palette = palette;
        let got = style_of(&bare, "No further help was read");
        assert_eq!(
            (got.fg, got.add_modifier),
            (warning.fg.or(Some(Color::Reset)), warning.add_modifier)
        );
    }
    let bold = style::Palette::plain();
    let mut app = app_over(filled(child(Some("Runs a script"))));
    app.palette = bold;
    let got = style_of(&app, "No further help was read");
    assert!(got.add_modifier.contains(ratatui::style::Modifier::BOLD));
}

#[test]
fn the_status_notices_keep_the_plain_style() {
    let mut reading = app_over(child(Some("Runs a script")));
    reading.mark_pending(start());
    let got = style_of(&reading, "Reading this command");
    assert_eq!(got.fg, Some(Color::Reset));
    assert!(got.add_modifier.is_empty());

    let mut node = child(Some("Runs a script"));
    node.same_as_ancestor = true;
    let same = app_over(node);
    let got = style_of(&same, "prints the same help as its parent");
    assert_eq!(got.fg, Some(Color::Reset));
    assert!(got.add_modifier.is_empty());
}
