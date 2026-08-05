//! The interactive event loop: polls terminal events, translates them via
//! `mantui_tui::event`, and renders each frame.
//!
//! **Not exercised by the automated test suite.** This sandbox has no tty
//! (`enable_raw_mode` fails with "No such device or address" here), so
//! this module's correctness rests on `mantui-tui`'s own state-machine and
//! render tests (which cover everything below the terminal I/O boundary)
//! plus manual review. See the batch report for this called out explicitly.

use anyhow::Context;
use crossterm::event::{self, Event};
use mantui_tui::app::App;
use mantui_tui::{clipboard, event as tui_event, layout, render, terminal, Effect};
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
    loop {
        app.ensure_rows_fresh();
        term.draw(|frame| render::render(frame, app))?;

        if !event::poll(Duration::from_millis(100))? {
            continue;
        }

        match event::read()? {
            Event::Key(key) => {
                if let Some(effect) = tui_event::handle_key(app, key) {
                    if !apply_effect(app, effect) {
                        return Ok(());
                    }
                }
            }
            Event::Mouse(mouse) => {
                let size = term.size()?;
                let area = ratatui::layout::Rect::new(0, 0, size.width, size.height);
                let regions = layout::compute(area, app.focus);
                tui_event::handle_mouse(app, mouse, &regions);
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
fn apply_effect(app: &mut App, effect: Effect) -> bool {
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
    }
    true
}
