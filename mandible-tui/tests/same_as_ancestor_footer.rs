//! Renders the real footer row through `ratatui::backend::TestBackend`
//! for a same-as-ancestor node. There is no tty in this sandbox
//! (AGENTS.md §3.6). The ruling is in docs/design.md §16, under a node
//! whose help repeats an ancestor's.
//!
//! `mandible-extract`'s `same_as_ancestor_node` pins `confidence` to `0.0`
//! because the probe never parsed this node at all, not because a parse
//! scored badly. Routing that through the ordinary low-confidence caveat
//! read `low confidence: 0% parsed`, wrong twice: the node was not parsed
//! at 0% and its confidence was never measured. The footer must instead
//! show the node's own status.

use mandible_core::{CommandNode, Provenance, Source};
use mandible_tui::app::App;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

const WIDTH: u16 = 100;
const HEIGHT: u16 = 40;

fn same_as_ancestor_node() -> CommandNode {
    let mut n = CommandNode::new("r", Provenance::with_confidence(Source::HelpText, 0.0));
    n.same_as_ancestor = true;
    n.children_filled = true;
    n
}

fn footer_row_text(app: &App) -> String {
    let backend = TestBackend::new(WIDTH, HEIGHT);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| mandible_tui::render::render(frame, app))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let y = HEIGHT - 1;
    (0..WIDTH)
        .map(|x| buffer[(x, y)].symbol().to_string())
        .collect::<String>()
}

/// The footer shows the node's own status, not a fabricated confidence
/// score.
#[test]
fn same_as_ancestor_node_shows_its_own_status() {
    let node = same_as_ancestor_node();
    let app = App::new("ar".to_string(), node);
    let row = footer_row_text(&app);
    assert!(
        row.contains("same as parent"),
        "a same-as-ancestor node must show its own status: {row:?}"
    );
    assert!(
        !row.contains("low confidence") && !row.contains("0% parsed"),
        "must not fabricate a parse percentage for a node never parsed: {row:?}"
    );
}
