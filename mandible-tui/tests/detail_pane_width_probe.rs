//! Narrow-terminal width probe (docs/design.md §9.3 rule 10,
//! `MIN_DESC_WIDTH` in `render/detail_pane/layout.rs`): does the FLAGS
//! description column stay readable at 60, 70 and 80 columns for a
//! long-flag tool?
//!
//! `systemctl`'s real captured `--help` (`tests/fixtures/systemctl_help.txt`)
//! replays with zero subprocesses through `Transcript` (the pattern
//! `detail_sections.rs`'s bpftrace test uses) into the real `HelpTextTier`.

use mandible_tui::app::App;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn systemctl_node() -> mandible_core::CommandNode {
    let raw = std::fs::read_to_string(format!(
        "{}/tests/fixtures/systemctl_help.txt",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("reading the captured systemctl --help bytes");

    let transcript = mandible_extract::exec::Transcript::new([(
        vec!["--help".to_string()],
        mandible_extract::exec::ExecOutput {
            stdout: raw.into_bytes(),
            stderr: Vec::new(),
            exit_code: Some(0),
            timed_out: false,
        },
    )]);
    let tier = mandible_extract::help_text::HelpTextTier::new(std::sync::Arc::new(transcript));
    let tool = mandible_extract::ResolvedTool {
        name: "systemctl".to_string(),
        path: Some(std::path::PathBuf::from("/replayed/systemctl")),
        version: None,
    };
    const ATTESTED: mandible_extract::NodeHints = mandible_extract::NodeHints {
        heading_attested: true,
    };
    mandible_extract::ExtractionTier::extract_node(
        &tier,
        &tool,
        &["systemctl".to_string()],
        ATTESTED,
    )
    .expect("the transcript covers the exact argv extract_node sends")
}

fn app_for(node: mandible_core::CommandNode) -> App {
    let mut app = App::new("systemctl".to_string(), node);
    app.focus = mandible_tui::Focus::Detail;
    app
}

/// Every row the whole frame renders for the detail pane, border and
/// padding stripped (matches `detail_sections.rs`'s `detail_rows`).
fn detail_rows(app: &App, width: u16, height: u16) -> Vec<String> {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| mandible_tui::render::render(frame, app))
        .unwrap();
    let buffer = terminal.backend().buffer().clone();
    let regions =
        mandible_tui::layout::compute(ratatui::layout::Rect::new(0, 0, width, height), app.focus);
    let rect = regions.detail.expect("wide enough for a detail pane");

    let mut rows = Vec::new();
    for y in (rect.y + 1)..(rect.y + rect.height - 1) {
        let mut line = String::new();
        for x in (rect.x + 2)..(rect.x + rect.width - 2) {
            line.push_str(buffer[(x, y)].symbol());
        }
        rows.push(line.trim_end().to_string())
    }
    rows
}

/// Where the row for `-a, --all` starts, and how many lines its own
/// wrapped description takes: from that row up to, but not including, the
/// next flag's own row (`-l, --full`'s). The description itself is real
/// and long ("Show all properties/all units currently in memory,
/// including dead/empty ones. To list all units installed on the system,
/// use 'list-unit-files' instead."), so its wrapped height is a direct
/// reading of how wide the description column actually is.
fn all_flag_row(rows: &[String]) -> (usize, usize) {
    let start = rows
        .iter()
        .position(|r| r.contains("--all Show all"))
        .expect("the --all row must be present");
    let height = rows[start..]
        .iter()
        .position(|r| r.contains("--full Don't ellipsize"))
        .expect("the next flag's row (--full) must be present");
    (start, height)
}

/// Measures whether `MIN_DESC_WIDTH` (28, spec §9.3 rule 10) holds at 60,
/// 70 and 80 columns for a real long-flag document. Numbers and verdict:
/// commit message.
#[test]
fn width_probe_reports_row_heights_at_60_70_80() {
    let mut heights = Vec::new();
    for width in [60u16, 70, 80] {
        let app = app_for(systemctl_node());
        let rows = detail_rows(&app, width, 300);
        let (start, height) = all_flag_row(&rows);
        let indent = rows[start + 1].len() - rows[start + 1].trim_start().len();
        // Printed so the measurement is legible in `cargo nextest run
        // --workspace --no-capture` output and in the commit's report,
        // not just asserted on.
        println!("width {width}: --all row height = {height} lines, description column indent = {indent}");
        heights.push((width, height));
    }

    // The row must never be empty (a parser regression, not a layout
    // one), and a wider terminal must never make the same real
    // description wrap *taller* than a narrower one did — the layout
    // invariant this probe exists to guard, whatever the measured heights
    // turn out to be.
    for (width, height) in &heights {
        assert!(*height >= 1, "width {width}: --all row rendered as 0 lines");
    }
    for pair in heights.windows(2) {
        let (narrower_w, narrower_h) = pair[0];
        let (wider_w, wider_h) = pair[1];
        assert!(
            wider_h <= narrower_h,
            "width {wider_w} ({wider_h} lines) must not wrap taller than width {narrower_w} ({narrower_h} lines)"
        );
    }
}
