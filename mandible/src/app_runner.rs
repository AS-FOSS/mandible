//! The interactive event loop: polls terminal events, translates them via
//! `mandible_tui::event`, and renders each frame. Also owns lazy per-node
//! extraction (spec §5.2 step 3) and background depth-warming (step 4) —
//! both live here rather than in `mandible-tui`, since `App` is pure state
//! with no extraction I/O of its own.
//!
//! **Not exercised by the automated test suite.** This sandbox has no tty
//! (`enable_raw_mode` fails with "No such device or address" here), so
//! this module's correctness rests on `mandible-tui`'s own state-machine and
//! render tests (which cover everything below the terminal I/O boundary),
//! `mandible-extract`'s `Runner::fill_node` tests (which cover the
//! extraction/merge logic this module calls), and manual review. See the
//! batch report for this called out explicitly.

use crate::background::Warmer;
use anyhow::Context;
use crossterm::event::{self, Event};
use mandible_extract::{default_tiers, resolve_tool, Runner};
use mandible_tui::app::App;
use mandible_tui::{clipboard, event as tui_event, layout, render, terminal, Effect};
use std::sync::Arc;
use std::time::Duration;

/// Run the interactive TUI for `app` until the user quits. Always restores
/// the terminal on the way out, even if the loop returns an error.
pub fn run(mut app: App) -> anyhow::Result<()> {
    let mut term = terminal::init().context("failed to initialize the terminal")?;
    let result = run_loop(&mut term, &mut app);
    let restore_result = terminal::restore().context("failed to restore the terminal");
    result.and(restore_result)
}

fn run_loop(term: &mut terminal::Term, app: &mut App) -> anyhow::Result<()> {
    let runner = Arc::new(Runner::new(default_tiers()));
    let resolved = resolve_tool(&app.tool);
    let warmer = Warmer::new();

    loop {
        // Splice in any background fills that completed since the last
        // iteration before rendering, so the tree reflects them promptly.
        for warmed in warmer.drain() {
            let node = warmed.result.node.clone();
            if warmed.warm_children {
                warmer.warm_children(&runner, &resolved, &node, &warmed.path);
            }
            app.splice_filled_node(&warmed.path, node);
        }

        // Drive the search index's background matcher forward from this
        // same poll timeout (spec §10 "Threading") — never as a blocking
        // spin inside the keystroke handler itself.
        app.tick_search(10);

        app.ensure_rows_fresh();
        term.draw(|frame| render::render(frame, app))?;

        if !event::poll(Duration::from_millis(100))? {
            continue;
        }

        match event::read()? {
            Event::Key(key) => {
                if let Some(effect) = tui_event::handle_key(app, key) {
                    if !apply_effect(app, effect, &runner, &resolved, &warmer) {
                        warmer.cancel();
                        return Ok(());
                    }
                }
            }
            Event::Mouse(mouse) => {
                let size = term.size()?;
                let area = ratatui::layout::Rect::new(0, 0, size.width, size.height);
                let regions = layout::compute(area, app.focus);
                if let Some(effect) = tui_event::handle_mouse(app, mouse, &regions) {
                    if !apply_effect(app, effect, &runner, &resolved, &warmer) {
                        warmer.cancel();
                        return Ok(());
                    }
                }
            }
            Event::Resize(_, _) => {
                // Nothing to do: the next loop iteration re-renders at the
                // new size automatically.
            }
            _ => {}
        }
    }
}

/// Apply an [`Effect`] produced by the event layer. Returns `false` if the
/// app should quit.
fn apply_effect(
    app: &mut App,
    effect: Effect,
    runner: &Arc<Runner>,
    resolved: &mandible_extract::ResolvedTool,
    warmer: &Warmer,
) -> bool {
    match effect {
        Effect::Quit => return false,
        Effect::Copy(text) => {
            let status = match clipboard::copy(&text) {
                Ok(()) => format!("copied: {text}"),
                Err(_) => format!("copy failed (clipboard unavailable): {text}"),
            };
            app.set_status(status);
        }
        Effect::Refresh => {
            let loaded = crate::pipeline::load(&app.tool, true);
            match loaded.root {
                Some(root) => {
                    let tool = app.tool.clone();
                    *app = App::new(tool, root);
                    app.set_status("re-extracted (cache bypassed)");
                }
                None => {
                    app.set_status("re-extraction failed: no tier produced a result");
                }
            }
        }
        Effect::Fill(path) => {
            if let Some(existing) = mandible_core::resolve(&app.root, &path).cloned() {
                app.mark_pending(path.clone());
                warmer.submit(Arc::clone(runner), resolved.clone(), path, existing, true);
            }
        }
    }
    true
}
