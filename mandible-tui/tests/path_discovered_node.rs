//! The convention-discovered node, rendered through a real frame
//! (`ratatui::backend::TestBackend` — AGENTS.md §3.2: there is no tty in
//! this sandbox).
//!
//! `mandible cargo` shows `clippy` because `cargo-clippy` sits on `PATH`,
//! not because `cargo --help` ever mentions it (spec §5.4). That is a guess
//! made from a filename, and the whole point of showing it is that the
//! screen says so: the tree row carries the `unverified` badge and the
//! footer names the binary the guess came from. Exercised here through the
//! real `render` path rather than only through `tree_pane`'s and
//! `detail_pane`'s own unit tests, for the reason `confidence_footer.rs`
//! gives — those call the builders directly, never the frame the user sees.

use mandible_core::{CommandNode, Provenance, Source, Text};
use mandible_tui::app::App;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

const WIDTH: u16 = 100;
const HEIGHT: u16 = 30;

/// `cargo` with one documented child and one discovered by the convention.
fn cargo_with_a_discovered_child() -> App {
    let mut root = CommandNode::new("cargo", Provenance::single(Source::HelpText));
    root.children_filled = true;

    let mut clean = CommandNode::new("clean", Provenance::single(Source::HelpText));
    clean.summary = Some(Text::sanitize("Remove the target directory"));
    clean.heading_attested = true;
    clean.children_filled = true;
    root.subcommands.push(clean);

    let mut clippy = CommandNode::new("clippy", Provenance::single(Source::HelpText));
    clippy.summary = Some(Text::sanitize("Checks a package to catch common mistakes"));
    clippy.discovered_binary = Some("cargo-clippy".to_string());
    clippy.children_filled = true;
    root.subcommands.push(clippy);

    App::new("cargo".to_string(), root)
}

fn screen(app: &App) -> Vec<String> {
    let backend = TestBackend::new(WIDTH, HEIGHT);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| mandible_tui::render::render(frame, app))
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    (0..HEIGHT)
        .map(|y| {
            (0..WIDTH)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect::<String>()
        })
        .collect()
}

fn row_containing<'a>(screen: &'a [String], needle: &str) -> &'a str {
    screen
        .iter()
        .find(|line| line.contains(needle))
        .unwrap_or_else(|| panic!("no row contains {needle:?}: {screen:#?}"))
}

#[test]
fn the_discovered_row_is_badged_and_the_documented_one_is_not() {
    let app = cargo_with_a_discovered_child();
    let screen = screen(&app);

    let clippy = row_containing(&screen, "clippy");
    assert!(
        clippy.contains("unverified"),
        "the discovered row must say so: {clippy:?}"
    );
    // The summary is not sacrificed to the badge.
    assert!(clippy.contains("Checks a package"), "{clippy:?}");

    let clean = row_containing(&screen, "clean");
    assert!(
        !clean.contains("unverified"),
        "a documented row must not be badged: {clean:?}"
    );
}

#[test]
fn selecting_the_discovered_node_names_its_binary_in_the_footer() {
    let mut app = cargo_with_a_discovered_child();
    app.ensure_rows_fresh();
    app.move_down();
    app.move_down();
    assert_eq!(
        app.selected_path(),
        Some(vec!["cargo".to_string(), "clippy".to_string()])
    );

    let screen = screen(&app);
    let footer = &screen[(HEIGHT - 1) as usize];
    assert!(footer.contains("unverified"), "{footer:?}");
    assert!(
        footer.contains("cargo-clippy"),
        "the footer must name the evidence: {footer:?}"
    );
}

/// The badge is the row's claim about whether the command exists, so it has
/// to survive a terminal with no colour at all (spec §9.2's "what may be
/// drawn": colour may never be the sole carrier of meaning).
#[test]
fn the_badge_is_legible_without_color() {
    let mut app = cargo_with_a_discovered_child();
    app.color_enabled = false;
    let screen = screen(&app);
    assert!(row_containing(&screen, "clippy").contains("unverified"));
}
