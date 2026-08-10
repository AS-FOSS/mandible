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
use mandible_tui::app::{App, RawHelp};
use mandible_tui::{clipboard, event as tui_event, layout, render, terminal, Effect};
use std::sync::Arc;
use std::time::Duration;

/// Upper bound on events dropped after a blocking re-extract.
const MAX_DISCARDED_EVENTS: usize = 1024;

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

    // Queue the root itself before the first frame. `main` hands us a stub
    // root carrying only the tool's name, so this is the fill that
    // discovers everything; its result then cascades into the children it
    // finds. Nothing is extracted on the main thread, which is what makes
    // launching instant regardless of how slow the tool is to probe.
    submit_root_fill(app, &runner, &resolved, &warmer);

    loop {
        // Splice in any background fills that completed since the last
        // iteration before rendering, so the tree reflects them promptly.
        for warmed in warmer.drain() {
            let node = warmed.result.node.clone();
            // Cascade unconditionally: every fill queues the children it
            // just discovered, so the background walk reaches the whole
            // tree instead of stopping one level past whatever the user
            // expanded. Marking them pending is what makes them render as
            // loading rows rather than as silently empty ones.
            let queued = warmer.warm_children(&runner, &resolved, &node, &warmed.path);
            app.splice_filled_node(&warmed.path, node);
            for path in queued {
                app.mark_pending(path);
            }
        }

        // Let a transient status message ("copied: …") time out. This loop
        // already wakes every 100ms to poll for input, so expiry needs no
        // timer of its own.
        app.expire_status(std::time::Instant::now());

        // Drive the search index's background matcher forward from this
        // same poll timeout (spec §10 "Threading") — never as a blocking
        // spin inside the keystroke handler itself.
        app.tick_search(10);

        app.ensure_rows_fresh();

        // Publish the tree pane's visible row count so keyboard navigation
        // can scroll the viewport to follow the selection (App::
        // follow_selection). Derived from the same layout the renderer
        // uses, and through `Block::inner` rather than a hardcoded border
        // thickness, so the two can't drift apart. Computed before the
        // draw, so the very first `↓` already has a real viewport rather
        // than waiting a frame.
        {
            let size = term.size()?;
            let area = ratatui::layout::Rect::new(0, 0, size.width, size.height);
            let regions = layout::compute(area, app.focus);
            app.tree_viewport = regions
                .tree
                .map(|rect| {
                    ratatui::widgets::Block::default()
                        .borders(ratatui::widgets::Borders::ALL)
                        .inner(rect)
                        .height as usize
                })
                .unwrap_or(0);
            app.follow_selection();
        }

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

        // The verbatim view (`t`) is a mode, so moving the selection while
        // it is on has to fetch the newly-selected node's raw text. Hooked
        // here, after every event, rather than onto each movement key:
        // there are eight ways to move the selection (arrows, hjkl,
        // expand, collapse, a search hit jumping to a flag's parent) and
        // wiring the fetch into each one would leave the mode silently
        // broken on whichever was missed. `raw_fetch_needed` is a cheap
        // map lookup that returns `None` unless the mode is on and this
        // node is genuinely unfetched.
        if let Some(effect) = app.raw_fetch_needed() {
            if !apply_effect(app, effect, &runner, &resolved, &warmer) {
                warmer.cancel();
                return Ok(());
            }
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
            // Abandon the previous whole-tree walk *first*. Its results
            // describe the tree about to be replaced, and without this
            // every `r` left another cascade running against a discarded
            // tree while eating from a warming budget that never refilled.
            warmer.reset();

            let loaded = crate::pipeline::load(&app.tool);
            match loaded.root {
                Some(root) => {
                    app.reload(root);
                    // Re-queue the root fill. This is what restarts the
                    // cascade that walks the tree; without it the pane sat
                    // empty after a re-extract until the user pressed
                    // Enter, because expand was the only remaining path
                    // that still triggered extraction.
                    submit_root_fill(app, runner, resolved, warmer);
                    app.set_status("re-extracted");
                }
                None => {
                    app.set_status("re-extraction failed: no tier produced a result");
                }
            }
            discard_input_typed_during_the_block();
        }
        Effect::Fill(path) => {
            if let Some(existing) = mandible_core::resolve(&app.root, &path).cloned() {
                app.mark_pending(path.clone());
                warmer.submit(Arc::clone(runner), resolved.clone(), path, existing);
            }
        }
        // Run on the UI thread rather than through the warm pool. This is
        // one probe of a node the warmer has almost certainly already
        // faulted in, so it costs tens of milliseconds; routing it through
        // the background pool would need a second result channel to carry
        // a different payload type, for a wait nobody would notice. The
        // pathological case is a tool that hangs until EXTRACT_TIMEOUT,
        // which the timeout itself bounds.
        Effect::FetchRaw(path) => {
            app.mark_raw_pending(path.clone());
            // Read attestation from the node this view is *about*, so the
            // raw fetch reproduces the same probe the parse used. The `t`
            // key's whole job is letting a reader check our reading against
            // the author's own bytes; fetching a different document than
            // the tree came from answers a question nobody asked. A node
            // the tree doesn't have cannot be attested, so `false` is the
            // right default rather than a fallback worth worrying about.
            let hints = mandible_extract::NodeHints {
                heading_attested: mandible_core::resolve(&app.root, &path)
                    .is_some_and(|n| n.heading_attested),
            };
            let result = match mandible_extract::help_text::raw_help(resolved, &path, hints) {
                Ok(lines) => RawHelp::Ready(lines),
                // Shown in the pane, not swallowed: "refused: kill is
                // never probed" is a useful answer to `t`, and a blank
                // pane is not.
                Err(e) => RawHelp::Failed(format!("could not fetch raw --help: {e}")),
            };
            app.set_raw_help(path, result);
        }
    }
    true
}

/// Queue the root for a background fill, which is what starts the cascade
/// that walks the whole tree.
///
/// Shared by startup and by re-extract, deliberately: these are the two
/// moments a tree exists with nothing filling it, and having only the
/// startup path do it is precisely how `r` came to leave an empty pane.
fn submit_root_fill(
    app: &mut App,
    runner: &Arc<Runner>,
    resolved: &mandible_extract::ResolvedTool,
    warmer: &Warmer,
) {
    let root_path = vec![app.root.name.clone()];
    app.mark_pending(root_path.clone());
    warmer.submit(
        Arc::clone(runner),
        resolved.clone(),
        root_path,
        app.root.clone(),
    );
}

/// Throw away input that arrived while a blocking re-extract held the loop.
///
/// `pipeline::load` runs a full extraction on this thread, so on a slow
/// tool the UI is unresponsive for seconds. Key auto-repeat keeps filling
/// crossterm's buffer throughout, and every one of those events was typed
/// blind at a frozen screen. Replaying them meant holding `r` queued one
/// more complete re-extraction per repeat, which is what made the key look
/// like it spawned unbounded work.
///
/// Bounded rather than looping until empty, so a key that is genuinely
/// stuck down cannot keep us here indefinitely.
fn discard_input_typed_during_the_block() {
    for _ in 0..MAX_DISCARDED_EVENTS {
        match event::poll(Duration::from_millis(0)) {
            Ok(true) => {
                if event::read().is_err() {
                    return;
                }
            }
            _ => return,
        }
    }
}
