//! Renders the real footer row through `ratatui::backend::TestBackend`
//! (spec §9, AGENTS.md §3.2 — there is no tty in this sandbox) for the
//! three shapes the ssh-keygen mislabeling bug touches:
//!
//! - a node whose confidence collapsed to a spurious `0.0` from an
//!   option-table sample of exactly one row (the ssh-keygen shape fixed by
//!   `mandible-extract`'s `MIN_MEANINGFUL_SAMPLE`) must show **no** caveat,
//!   matching `--doctor`'s "100% flags with text" for the same document;
//! - a genuinely low-confidence node (`find`-/`ip`-shaped: a real,
//!   larger option-table sample, mostly unclean) must still show the
//!   badge — this bug's fix must not wash out a real signal;
//! - a verbatim node (`git <subcommand>`-shaped) must stay silent, as
//!   before.
//!
//! `cargo test`'s synthetic-fixture blind spot (AGENTS.md §3.2) is why this
//! exists alongside the plain unit tests in `detail_pane.rs`: those call
//! `provenance_caveat` directly, never through the real frame/layout path
//! the user actually sees.

use mandible_core::{CommandNode, Entity, Provenance, Source, Spelling, Text};
use mandible_tui::app::App;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

const WIDTH: u16 = 100;
const HEIGHT: u16 = 40;

fn node_with_a_flag(confidence: f32) -> CommandNode {
    let mut n = CommandNode::new(
        "tool",
        Provenance::with_confidence(Source::HelpText, confidence),
    );
    let mut f = Entity::flag_long("verbose", Provenance::single(Source::HelpTextSynopsis));
    f.spellings.insert(0, Spelling::short('v'));
    n.flags = vec![f];
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

/// The ssh-keygen shape: this project's fix makes
/// `compute_confidence` land such a node at `0.5` (the same
/// "unidentified but parsed as cleanly as we can tell" cap `git`/`curl`/
/// `apt-get` already sit at), not a spurious `0.0` — so the footer must
/// carry no low-confidence badge, matching `--doctor ssh-keygen`'s "100%
/// flags with text".
#[test]
fn ssh_keygen_shaped_node_shows_no_low_confidence_badge() {
    let node = node_with_a_flag(0.5);
    let app = App::new("ssh-keygen".to_string(), node);
    let row = footer_row_text(&app);
    assert!(
        !row.contains("low confidence"),
        "ssh-keygen-shaped node (confidence 0.5) must not show the badge: {row:?}"
    );
}

/// A genuinely low-confidence node (real sample, mostly unclean — `find`
/// measured 0.11, `ip` measured 0.09 on the machine that wrote this
/// comment; environment-dependent per AGENTS.md, but always well under the
/// 0.5 cap) must still carry the badge. This is the signal the ssh-keygen
/// fix must not wash out.
#[test]
fn genuinely_low_confidence_node_keeps_its_badge() {
    let node = node_with_a_flag(0.11);
    let app = App::new("find".to_string(), node);
    let row = footer_row_text(&app);
    assert!(
        row.contains("low confidence") && row.contains("11%"),
        "a real low-confidence node must still show its score: {row:?}"
    );
}

/// A verbatim node (spec §7 Tier B step 3 — every `git` subcommand renders
/// this way, since `git clone --help` prints a man page) is a designed
/// fallback, not a bad parse, and must stay silent regardless of its
/// (by-construction 0.0) confidence.
#[test]
fn verbatim_node_stays_silent() {
    let mut node = node_with_a_flag(0.0);
    node.unparsed = vec![Text::sanitize("GIT-CLONE(1) Git Manual GIT-CLONE(1)")];
    let app = App::new("git".to_string(), node);
    let row = footer_row_text(&app);
    assert!(
        !row.contains("low confidence") && !row.contains("parsed"),
        "a verbatim node must never show the parse-confidence badge: {row:?}"
    );
}
