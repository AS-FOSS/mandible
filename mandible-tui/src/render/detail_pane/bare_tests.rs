//! The notice on a loaded page that holds nothing past its summary.

use super::*;
use mandible_core::notice::NO_FURTHER_HELP_NOTICE;
use mandible_core::{Provenance, Source, Text};
use ratatui::backend::TestBackend;
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
