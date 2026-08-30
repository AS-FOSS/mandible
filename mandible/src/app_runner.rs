//! The interactive event loop: polls terminal events, translates them via
//! `mandible_tui::event`, and renders each frame. Also owns lazy per-node
//! extraction (spec §5.2 step 3) and background depth-warming (step 4) —
//! both live here rather than in `mandible-tui`, since `App` is pure state
//! with no extraction I/O of its own.
//!
//! [`run_review`] is the `mandible --review <seed>` entry point: it drives
//! the *same* [`run_loop`] a plain `mandible <tool>` session uses, one tool
//! at a time, over a persistent terminal session — a reviewer gets the real
//! product (lazy subcommand fill, the raw pane, search, everything) rather
//! than a second, parallel UI. What's added on top is a key interception
//! (`app.review.is_some()`, checked before the ordinary
//! `mandible_tui::event::handle_key`) that lets `c`/`i`/`w`/`s` start a
//! verdict draft and `Enter` confirm it, and the surrounding manifest I/O
//! (`mandible_core::audit::{load, save}`), which is the only file access
//! `mandible --review` performs — never a second probe of the tool beyond
//! what the ordinary tree view already does.
//!
//! **Not exercised by the automated test suite.** This sandbox has no tty
//! (`enable_raw_mode` fails with "No such device or address" here), so
//! this module's correctness rests on `mandible-tui`'s own state-machine and
//! render tests (which cover everything below the terminal I/O boundary),
//! `mandible-extract`'s `Runner::fill_node` tests (which cover the
//! extraction/merge logic this module calls), `mandible_tui::app_review`'s
//! own key-handling tests, and manual review via `scripts/pty_screenshot.py`
//! (AGENTS.md §3.2). See the batch report for this called out explicitly.

use crate::background::Warmer;
use anyhow::Context;
use crossterm::event::{self, Event};
use mandible_core::audit::{self, Entry};
use mandible_extract::{default_tiers, resolve_tool, Runner};
use mandible_tui::app::{App, RawHelp};
use mandible_tui::app_review::{
    handle_review_key, ReviewKeyOutcome, ReviewOverlay, ReviewSubmission,
};
use mandible_tui::{clipboard, event as tui_event, layout, render, terminal, Effect};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// Upper bound on events dropped after a blocking re-extract.
const MAX_DISCARDED_EVENTS: usize = 1024;

/// Why [`run_loop`] returned.
enum LoopExit {
    /// The user quit (`q`, `Ctrl-C`, or the search-box `q` special case) —
    /// terminates the whole session, review or not.
    Quit,
    /// A review draft was confirmed with `Enter`. Only ever produced when
    /// `app.review.is_some()`; [`run`]'s plain single-tool path never sees
    /// this variant since it never attaches a review overlay.
    ReviewSubmit(ReviewSubmission),
}

/// Build an `App` for `tool`, with settings resolved from the user's
/// `config.toml` applied on top of `App::new`'s pure defaults.
///
/// `App::new` itself does no filesystem I/O — it is a plain constructor, so
/// every embedder (including this crate's own tests, and `mandible-tui`'s)
/// gets deterministic defaults regardless of what happens to exist on the
/// machine running them. Reading `~/.config/mandible/config.toml` is a
/// startup concern, so it belongs here at the composition root beside
/// everything else `main.rs`/`app_runner.rs` already resolves before the
/// first frame — never inside `App::new`, which broke exactly this: a
/// `horizontal_scroll = false` left over from someone's own use of
/// mandible on the machine running the test suite silently changed what
/// "off reproduces today's behavior" was testing against.
///
/// The one real call site for a fresh tool session, whether that's `mandible
/// <tool>` ([`main`][crate::main] via the caller of this function) or one
/// tool inside `mandible --review` ([`run_review_loop`]) — both go through
/// this instead of `App::new` directly, so the config is never forgotten on
/// either path.
pub fn new_app(tool: String, stub: mandible_core::CommandNode) -> App {
    let mut app = App::new(tool, stub);
    app.horizontal_scroll_enabled = mandible_core::config::load().ui.horizontal_scroll;
    app
}

/// Run the interactive TUI for `app` until the user quits. Always restores
/// the terminal on the way out, even if the loop returns an error.
pub fn run(mut app: App) -> anyhow::Result<()> {
    let mut term = terminal::init().context("failed to initialize the terminal")?;
    let result = run_loop(&mut term, &mut app).map(|_| ());
    let restore_result = terminal::restore().context("failed to restore the terminal");
    result.and(restore_result)
}

/// `mandible --review <seed>`: walk `<dir>/<seed>.toml`'s pending entries in
/// order, opening each tool in the normal TUI exactly as `mandible <tool>`
/// would, and saving a verdict to the manifest after every one — so an
/// interrupted session (killed process, closed terminal) resumes at the
/// next still-pending entry rather than restarting (`xtask audit`'s own
/// review loop, `xtask/src/audit.rs`'s `cmd_review`, already takes this
/// discipline seriously; this matches it).
///
/// One terminal session spans the whole sample: `terminal::init`/`restore`
/// run once here, not once per tool, so moving from one tool to the next
/// never flashes back to a bare shell prompt.
pub fn run_review(dir: &Path, seed: u64) -> anyhow::Result<()> {
    let path = audit::verdict_path(dir, seed);
    let manifest = audit::load(&path)?;
    if manifest.needing_attention().next().is_none() {
        // Nothing to do: report it plainly and skip the raw-mode dance
        // entirely, same as `xtask audit review`'s own "nothing pending"
        // message — a `--review` run after everything's already judged
        // shouldn't flash into the alternate screen for zero frames.
        println!("nothing pending in {}", path.display());
        return Ok(());
    }

    let mut term = terminal::init().context("failed to initialize the terminal")?;
    let result = run_review_loop(&mut term, dir, seed);
    let restore_result = terminal::restore().context("failed to restore the terminal");
    result.and(restore_result)
}

fn run_review_loop(term: &mut terminal::Term, dir: &Path, seed: u64) -> anyhow::Result<()> {
    let path = audit::verdict_path(dir, seed);
    let mut manifest = audit::load(&path)?;

    loop {
        let Some(idx) = manifest.needing_attention().next() else {
            break;
        };
        let entry = manifest.entries[idx].clone();
        let remaining = manifest.needing_attention().count();
        let total = manifest.entries.len();

        let resolved = resolve_tool(&entry.tool);
        if resolved.path.is_none() {
            // The tool was on PATH when `xtask audit sample` drew it but
            // isn't now (uninstalled, a stale sample, a different
            // machine). Record that honestly and move on rather than
            // blocking the whole session on one unreachable tool — a
            // `skip` is exactly the recorded-not-omitted vocabulary
            // `cmd_review` already uses for "nothing to judge".
            record_skip(
                &path,
                &mut manifest,
                idx,
                "tool not found on PATH during --review",
            )?;
            continue;
        }

        let stub = mandible_core::CommandNode::new(
            entry.tool.clone(),
            mandible_core::Provenance::default(),
        );
        let mut app = new_app(entry.tool.clone(), stub);
        app.review = Some(review_overlay_for(&entry, remaining, total));

        match run_loop(term, &mut app)? {
            LoopExit::Quit => break,
            LoopExit::ReviewSubmit(submission) => {
                apply_submission(&path, &mut manifest, idx, submission)?;
            }
        }
    }
    Ok(())
}

/// Build the display-only review context for `entry`, attached to the
/// freshly-built [`App`] before its first frame.
fn review_overlay_for(entry: &Entry, remaining: usize, total: usize) -> ReviewOverlay {
    ReviewOverlay {
        tool: entry.tool.clone(),
        stratum: entry.stratum.clone(),
        k1: entry.k1,
        k2: entry.k2,
        k3: entry.k3,
        include_reason: entry.include_reason.clone(),
        remaining,
        total,
        draft: None,
    }
}

/// Apply a confirmed [`ReviewSubmission`] to `manifest.entries[idx]` and
/// save immediately — called for every single verdict, never batched, so a
/// killed process afterward leaves this one recorded and only the rest
/// pending.
fn apply_submission(
    path: &Path,
    manifest: &mut audit::AuditFile,
    idx: usize,
    submission: ReviewSubmission,
) -> anyhow::Result<()> {
    let mut note = submission.note;
    let k1 = audit::extract_tag_override(&mut note, "k1");
    let k2 = audit::extract_tag_override(&mut note, "k2");
    let k3 = audit::extract_tag_override(&mut note, "k3");

    let entry = &mut manifest.entries[idx];
    entry.verdict = Some(submission.verdict.to_string());
    entry.note = note;
    if let Some(v) = k1 {
        entry.k1 = Some(v);
    }
    if let Some(v) = k2 {
        entry.k2 = Some(v);
    }
    if let Some(v) = k3 {
        entry.k3 = Some(v);
    }
    audit::save(path, manifest)
}

/// Record an unconditional `skip` for `manifest.entries[idx]` (the tool
/// couldn't even be opened) and save immediately, same discipline as
/// [`apply_submission`].
fn record_skip(
    path: &Path,
    manifest: &mut audit::AuditFile,
    idx: usize,
    reason: &str,
) -> anyhow::Result<()> {
    let entry = &mut manifest.entries[idx];
    entry.verdict = Some("skip".to_string());
    entry.note = reason.to_string();
    audit::save(path, manifest)
}

fn run_loop(term: &mut terminal::Term, app: &mut App) -> anyhow::Result<LoopExit> {
    let runner = Arc::new(Runner::new(default_tiers()));
    let resolved = resolve_tool(&app.tool);
    let warmer = Warmer::new();
    // One `PATH` scan for the session (spec §5.4). Done here rather than
    // inside the extraction pipeline because it reads the running machine's
    // filesystem: a tier that did it would make every corpus fixture depend
    // on what happens to be installed beside the tool — see
    // `crate::discovery`'s module doc.
    let siblings = mandible_extract::discover_path_siblings(&resolved.name);

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
            let mut node = warmed.result.node;
            // The root's children are the tool's own documented commands
            // plus whatever the `<tool>-<sub>` convention finds on PATH
            // (spec §5.4). Attached at the root only, because that is where
            // the convention lives: cargo dispatches `cargo clippy` to
            // `cargo-clippy`, and nothing dispatches `cargo clippy fix` to
            // `cargo-clippy-fix`.
            if warmed.path.len() == 1 {
                crate::discovery::attach_path_siblings(&mut node, &siblings);
            }
            // Spliced before the cascade, not after: `warm_children` resolves
            // each child's probe target against the tree, so the node it is
            // about has to be in the tree first.
            app.splice_filled_node(&warmed.path, node);
            // Cascade unconditionally: every fill queues the children it
            // just discovered, so the background walk reaches the whole
            // tree instead of stopping one level past whatever the user
            // expanded. Marking them pending is what makes them render as
            // loading rows rather than as silently empty ones.
            let queued = warmer.warm_children(&runner, &resolved, &app.root, &warmed.path);
            for path in queued {
                app.mark_pending(path);
            }
            // The node `mandible <tool> <sub>` asked for may have just
            // arrived — or may now be known not to exist (spec §5.4).
            app.settle_requested_path();
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
                // The review overlay gets first refusal on every keystroke
                // while it's attached: a verdict letter or a note character
                // must never also be interpreted as tree navigation or a
                // search query. `handle_review_key` itself declines
                // Ctrl-chords and (outside a draft) declines while the
                // search box has focus, so `Ctrl-C` and ordinary typing
                // still reach the ordinary handler below in those cases.
                let mut claimed = false;
                if app.review.is_some() {
                    if let Some(outcome) = handle_review_key(app, key) {
                        claimed = true;
                        if let ReviewKeyOutcome::Submit(submission) = outcome {
                            warmer.cancel();
                            return Ok(LoopExit::ReviewSubmit(submission));
                        }
                    }
                }
                if !claimed {
                    if let Some(effect) = tui_event::handle_key(app, key) {
                        if !apply_effect(app, effect, &runner, &resolved, &warmer) {
                            warmer.cancel();
                            return Ok(LoopExit::Quit);
                        }
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
                        return Ok(LoopExit::Quit);
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
                return Ok(LoopExit::Quit);
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
                // Same redirect the background cascade applies (spec §5.4):
                // an expand on a convention-discovered node probes that
                // node's own binary, never the parent with a guessed word.
                let (tool, probe_path) = crate::discovery::probe_target(&app.root, resolved, &path);
                app.mark_pending(path.clone());
                warmer.submit(Arc::clone(runner), tool, probe_path, path, existing);
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
            // Through the same redirect the parse itself went through
            // (spec §5.4), for exactly the reason above: under a
            // convention-discovered node the tree was built from
            // `cargo-clippy --help`, so that is the document `t` has to
            // show — and the argv line it prints names the binary that was
            // really run, not an invocation nobody made.
            let (raw_tool, raw_path) = crate::discovery::probe_target(&app.root, resolved, &path);
            let result = match mandible_extract::help_text::raw_help(&raw_tool, &raw_path, hints) {
                // Render the argv exactly as a human would type it, so
                // the pane can name its own source rather than assume one.
                Ok((lines, flag)) => {
                    let argv = std::iter::once(raw_tool.name.clone())
                        .chain(raw_path.iter().skip(1).cloned())
                        .collect::<Vec<_>>()
                        .join(" ");
                    RawHelp::Ready(lines, format!("{argv} {flag}"))
                }
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
        root_path.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use mandible_core::config::CONFIG_DIR_ENV;

    /// Serializes this module's tests against `MANDIBLE_CONFIG_DIR` —
    /// `std::env::set_var` is process-global and nextest runs tests from
    /// one binary on multiple threads, so two tests setting it concurrently
    /// would race.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn stub() -> mandible_core::CommandNode {
        mandible_core::CommandNode::new("git", mandible_core::Provenance::default())
    }

    /// The regression this whole fix exists for: `new_app` — not `App::new`
    /// — is where a real `config.toml` takes effect. Reproduces the report
    /// exactly: a `config.toml` with `horizontal_scroll = false` on disk
    /// must actually turn the feature off on an `App` built through the
    /// real composition root, and `App::new` itself must stay pure (always
    /// `true`, regardless of what's on disk) so nothing but this one
    /// function ever reads the file.
    #[test]
    fn new_app_honors_config_toml_while_app_new_stays_pure() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "[ui]\nhorizontal_scroll = false\n",
        )
        .unwrap();
        std::env::set_var(CONFIG_DIR_ENV, dir.path());

        let pure = App::new("git".to_string(), stub());
        assert!(
            pure.horizontal_scroll_enabled,
            "App::new must never read config.toml itself"
        );

        let wired = new_app("git".to_string(), stub());
        assert!(
            !wired.horizontal_scroll_enabled,
            "new_app must apply the real config.toml"
        );

        std::env::remove_var(CONFIG_DIR_ENV);
    }

    /// The default: no `config.toml` on disk still leaves the feature on
    /// through the real composition root, not just through `App::new`.
    #[test]
    fn new_app_defaults_to_horizontal_scroll_on_with_no_config_file() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var(CONFIG_DIR_ENV, dir.path());

        let app = new_app("git".to_string(), stub());
        assert!(app.horizontal_scroll_enabled);

        std::env::remove_var(CONFIG_DIR_ENV);
    }
}
