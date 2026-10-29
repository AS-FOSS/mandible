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
use crate::probing::{self, Ctx};
use crate::queue::{Limit, Urgency};
use crate::raw_view;
use crate::reload::Reloader;
use anyhow::Context;
use crossterm::event::{self, Event};
use mandible_core::audit::{self, Entry};
use mandible_extract::{default_tiers, resolve_tool, Runner};
use mandible_tui::app::App;
use mandible_tui::app_review::{
    handle_review_key, ReviewKeyOutcome, ReviewOverlay, ReviewSubmission,
};
use mandible_tui::{clipboard, event as tui_event, layout, render, terminal, Effect};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// State the loop keeps beside the tree: the re-extraction in flight.
struct Session {
    reload: Reloader,
}

/// Why [`run_loop`] returned.
enum LoopExit {
    /// The user quit (`q`, `Ctrl-C`, or the search-box `q` special case) —
    /// terminates the whole session, review or not.
    Quit,
    /// A review draft was confirmed with `Enter`. Only ever produced when
    /// `app.review.is_some()`; [`run`]'s plain single-tool path never sees
    /// this variant since it never attaches a review overlay.
    ReviewSubmit(ReviewSubmission),
    /// `Enter` accepted a selection under `--print-selection`. Carries the
    /// composed command line, which [`run`] prints on stdout *after* the
    /// terminal is restored. Only ever produced when
    /// `app.print_selection` is set, which nothing but `main` sets.
    PrintSelection(String),
}

/// What [`apply_effect`] decided the event loop should do next.
///
/// It replaced a bare `bool` when a second reason to leave the loop
/// appeared: `false` could only ever mean "quit", so a new exit had to
/// carry its payload some other way — and the other way is a field
/// somewhere that the three call sites would each have to remember to
/// read.
enum Control {
    /// Keep polling.
    Continue,
    /// Leave the loop for this reason.
    Exit(LoopExit),
}

/// Build an `App` for `tool`, with the user's `config.toml` applied on top
/// of `App::new`'s defaults and the environment applied on top of that
/// (docs/design.md §9.5).
///
/// `App::new` does no filesystem I/O, so every embedder and test gets the
/// same defaults whatever file exists on the machine. Reading the file is
/// the composition root's job, here, so both a plain session and each tool
/// of `--review` get it.
///
/// `sink` is the stream the UI is drawn on, which decides whether there is
/// a terminal to colour for (see [`mandible_tui::style::Palette::for_sink`]).
/// Problems found in the file become the footer line until the first key.
pub fn new_app(tool: String, stub: mandible_core::CommandNode, sink: terminal::Sink) -> App {
    let loaded = mandible_core::config::load();
    let mut app = App::new(tool, stub);
    app.set_palette(mandible_tui::style::Palette::for_sink(sink));
    app.settings.locks = mandible_tui::settings::EnvLocks::from_env();
    app.settings.config_path = mandible_core::config::config_path();
    app.apply_config(&loaded.config);
    app.notice = loaded.notice();
    app
}

/// Run the interactive TUI for `app` until the user quits. Always restores
/// the terminal on the way out, even if the loop returns an error.
pub fn run(mut app: App, sink: terminal::Sink) -> anyhow::Result<()> {
    let mut term = terminal::init(sink).context("failed to initialize the terminal")?;
    let result = run_loop(&mut term, &mut app, sink);
    let restore_result = terminal::restore(sink).context("failed to restore the terminal");
    // The loop's own error wins over the restore's, as it did when this
    // was one `result.and(restore_result)` — but the restore has already
    // run either way, which is the part that matters.
    let exit = result?;
    restore_result?;
    // Printed after the terminal is restored, never before: under
    // `--print-selection` this is the one line the calling shell reads,
    // and writing it while the alternate screen is still up interleaves it
    // with the teardown sequences on their way to the *other* stream.
    if let LoopExit::PrintSelection(line) = exit {
        println!("{line}");
    }
    Ok(())
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

    let mut term =
        terminal::init(terminal::Sink::Stdout).context("failed to initialize the terminal")?;
    let result = run_review_loop(&mut term, dir, seed);
    let restore_result =
        terminal::restore(terminal::Sink::Stdout).context("failed to restore the terminal");
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
        let mut app = new_app(entry.tool.clone(), stub, terminal::Sink::Stdout);
        app.review = Some(review_overlay_for(&entry, remaining, total));

        match run_loop(term, &mut app, terminal::Sink::Stdout)? {
            LoopExit::Quit => break,
            // `--review` never turns `print_selection` on, so `Enter` is
            // the verdict key there and never the accept key. Ending the
            // session is the honest response to a variant this path
            // cannot produce, rather than an `unreachable!` on a state
            // one field change away from being reachable.
            LoopExit::PrintSelection(_) => break,
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

fn run_loop(
    term: &mut terminal::Term,
    app: &mut App,
    sink: terminal::Sink,
) -> anyhow::Result<LoopExit> {
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
    let ctx = Ctx {
        runner: &runner,
        resolved: &resolved,
        warmer: &warmer,
    };
    submit_root_fill(app, &ctx, Limit::Standard);
    let mut session = Session {
        reload: Reloader::new(),
    };

    loop {
        // Splice in any background fills that completed since the last
        // iteration before rendering, so the tree reflects them promptly.
        if let Some(loaded) = session.reload.poll(&app.tool) {
            apply_reload(app, loaded.root, &ctx);
        }
        probing::collect(app, &ctx, &siblings);

        // Let a transient status message ("copied: …") time out. This loop
        // already wakes every 100ms to poll for input, so expiry needs no
        // timer of its own.
        app.expire_status(std::time::Instant::now());

        // Rebuild the search index once for a batch of spliced nodes
        // (spec §10), never once per node.
        app.tick_search();

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
            probing::at_rest(app, &ctx);
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
                        if let Control::Exit(exit) =
                            apply_effect(app, effect, &ctx, &mut session, sink)
                        {
                            warmer.cancel();
                            return Ok(exit);
                        }
                    }
                }
            }
            Event::Mouse(mouse) => {
                let size = term.size()?;
                let area = ratatui::layout::Rect::new(0, 0, size.width, size.height);
                let regions = layout::compute(area, app.focus);
                if let Some(effect) = tui_event::handle_mouse(app, mouse, &regions) {
                    if let Control::Exit(exit) = apply_effect(app, effect, &ctx, &mut session, sink)
                    {
                        warmer.cancel();
                        return Ok(exit);
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
            if let Control::Exit(exit) = apply_effect(app, effect, &ctx, &mut session, sink) {
                warmer.cancel();
                return Ok(exit);
            }
        }
    }
}

/// Apply an [`Effect`] produced by the event layer, saying whether the
/// loop should keep going.
fn apply_effect(
    app: &mut App,
    effect: Effect,
    ctx: &Ctx,
    session: &mut Session,
    sink: terminal::Sink,
) -> Control {
    match effect {
        Effect::Quit => return Control::Exit(LoopExit::Quit),
        // The composed line is carried out of the loop rather than printed
        // here: the terminal is still in raw mode and on the alternate
        // screen at this point, and `run` prints it once both are undone.
        Effect::PrintSelection(line) => return Control::Exit(LoopExit::PrintSelection(line)),
        Effect::Copy(text) => {
            // To `sink`, not to stdout: the OSC-52 fallback writes an
            // escape sequence, and under `--print-selection` stdout is the
            // line the calling shell will run.
            let status = match clipboard::copy(&text, sink) {
                Ok(()) => format!("copied: {text}"),
                Err(_) => format!("copy failed (clipboard unavailable): {text}"),
            };
            app.set_status(status);
        }
        Effect::Refresh => refresh(app, ctx, session),
        Effect::Fill(path) => {
            probing::request(app, ctx, path, Urgency::User, false, Limit::Standard);
        }
        // Served from the output the fill kept; nothing is probed here.
        // With nothing kept the pane says it is loading, and the fetch
        // goes to the background pool once the cursor rests.
        Effect::FetchRaw(path) => {
            if !raw_view::serve(app, ctx.resolved, &path) {
                app.mark_raw_pending(path);
            }
        }
    }
    Control::Continue
}

/// `r`: reload what you are looking at. On a command's page that command is
/// read again, then its subcommands; on the tool's root page the whole tool
/// is extracted again, off this thread.
fn refresh(app: &mut App, ctx: &Ctx, session: &mut Session) {
    let Some(path) = app.selected_path() else {
        return;
    };
    if path.len() > 1 {
        probing::reread(app, ctx, path);
    } else if session
        .reload
        .start(&app.tool, probing::retry_limit(app, &path).duration())
    {
        app.set_status("re-extracting");
    }
}

/// Queue the root for a background fill, which is what starts the cascade
/// that walks the whole tree.
///
/// Shared by startup and by re-extract, deliberately: these are the two
/// moments a tree exists with nothing filling it, and having only the
/// startup path do it is precisely how `r` came to leave an empty pane.
fn submit_root_fill(app: &mut App, ctx: &Ctx, limit: Limit) {
    let root_path = vec![app.root.name.clone()];
    probing::request(app, ctx, root_path, Urgency::Walk, true, limit);
}

/// Swap a finished re-extraction in. The previous whole-tree walk is
/// abandoned first, since its results describe the tree being replaced, and
/// the root fill restarts the walk (without it the pane sat empty until the
/// user pressed Enter). A root whose last read timed out is read with the
/// longer limit again.
fn apply_reload(app: &mut App, root: Option<mandible_core::CommandNode>, ctx: &Ctx) {
    let Some(root) = root else {
        app.set_status("re-extraction failed: no tier produced a result");
        return;
    };
    let limit = probing::retry_limit(app, std::slice::from_ref(&app.root.name));
    ctx.warmer.reset();
    app.reload(root);
    submit_root_fill(app, ctx, limit);
    app.set_status("re-extracted");
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

    /// `new_app`, not `App::new`, is where a real `config.toml` takes
    /// effect: the file on disk reaches every setting, and `App::new`
    /// stays at its defaults whatever is on disk.
    #[test]
    fn new_app_applies_config_toml_while_app_new_stays_pure() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "[ui]\nhorizontal_scroll = true\nraw_view = true\ndescription_preview = false\n\
             [sort]\nflags = \"az\"\ncommands = \"az\"\n[colors]\ntheme = \"none\"\n",
        )
        .unwrap();
        std::env::set_var(CONFIG_DIR_ENV, dir.path());

        let pure = App::new("git".to_string(), stub());
        assert!(!pure.horizontal_scroll_enabled && pure.parsed_wrap && !pure.raw_mode);
        assert!(pure.settings.description_preview);

        let wired = new_app("git".to_string(), stub(), terminal::Sink::Stdout);
        assert!(wired.horizontal_scroll_enabled && !wired.parsed_wrap);
        assert!(wired.raw_mode);
        assert!(!wired.settings.description_preview);
        assert_eq!(wired.sorts.detail, mandible_tui::sort::DetailSort::Overall);
        assert!(wired.sorts.tree);
        assert!(!wired.palette.color && !wired.color_enabled);
        assert_eq!(
            wired.settings.config_path.as_deref(),
            Some(dir.path().join("config.toml").as_path())
        );
        assert_eq!(wired.notice, None);

        std::env::remove_var(CONFIG_DIR_ENV);
    }

    /// With no file every setting is its default, and nothing is created.
    #[test]
    fn new_app_with_no_config_file_uses_the_defaults_and_writes_nothing() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var(CONFIG_DIR_ENV, dir.path());

        let app = new_app("git".to_string(), stub(), terminal::Sink::Stdout);
        assert!(!app.horizontal_scroll_enabled && app.parsed_wrap && !app.raw_mode);
        assert!(app.settings.description_preview);
        assert_eq!(app.notice, None);
        assert!(!dir.path().join("config.toml").exists());

        std::env::remove_var(CONFIG_DIR_ENV);
    }

    /// A bad key and a bad value cost only themselves, and the footer line
    /// names the first of the two.
    #[test]
    fn new_app_reports_problems_in_one_footer_line() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "[sort]\nsrot = 1\nflags = \"sideways\"\ncommands = \"az\"\n",
        )
        .unwrap();
        std::env::set_var(CONFIG_DIR_ENV, dir.path());

        let app = new_app("git".to_string(), stub(), terminal::Sink::Stdout);
        assert!(app.sorts.tree, "the good key still applies");
        assert_eq!(
            app.notice.as_deref(),
            Some("2 problems in config.toml: line 2: unknown key \"srot\", ignored")
        );

        std::env::remove_var(CONFIG_DIR_ENV);
    }

    /// A custom theme reaches the first frame's palette, and a bad colour
    /// costs only its own key and shows in the footer line.
    #[test]
    fn new_app_applies_the_custom_colours_of_the_file() {
        use mandible_core::config::{ColorIndex, Roles};
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "[colors]\ntheme = \"custom\"\naccent = \"bright-red\"\nwarning = \"#ff0000\"\nmuted = 244\n",
        )
        .unwrap();
        std::env::set_var(CONFIG_DIR_ENV, dir.path());

        let app = new_app("git".to_string(), stub(), terminal::Sink::Stdout);
        let want = Roles {
            accent: ColorIndex(9),
            muted: ColorIndex(244),
            ..Roles::DEFAULT
        };
        assert_eq!(app.settings.custom, want);
        assert_eq!(
            app.notice.as_deref(),
            Some("config.toml line 4: bad value for warning, ignored")
        );
        assert_eq!(app.settings.theme, mandible_core::config::Theme::Custom);

        std::env::remove_var(CONFIG_DIR_ENV);
    }

    fn rig() -> (std::sync::Arc<crate::doubles::Spy>, App) {
        let spy = crate::doubles::Spy::new();
        spy.children("t", &["a", "b"]);
        let root = mandible_core::CommandNode::new("t", mandible_core::Provenance::default());
        (spy, App::new("no-such-tool-xyz-123".to_string(), root))
    }

    fn walked(app: &mut App, ctx: &Ctx) {
        submit_root_fill(app, ctx, Limit::Standard);
        let end = std::time::Instant::now() + Duration::from_secs(10);
        while !app.fill_finished(&["t".to_string(), "b".to_string()])
            && std::time::Instant::now() < end
        {
            probing::collect(app, ctx, &[]);
            std::thread::sleep(Duration::from_millis(5));
        }
        app.ensure_rows_fresh();
    }

    /// `r` reloads what you are looking at: on a command's page, that
    /// command alone; on the tool's root page, the whole tool.
    #[test]
    fn r_on_a_command_reads_that_command_and_on_the_root_reloads_the_tool() {
        let (spy, mut app) = rig();
        let runner = spy.runner();
        let resolved = crate::doubles::Spy::tool();
        let warmer = Warmer::with_threads(3);
        let ctx = Ctx {
            runner: &runner,
            resolved: &resolved,
            warmer: &warmer,
        };
        let mut session = Session {
            reload: Reloader::new(),
        };
        walked(&mut app, &ctx);
        spy.forget();

        app.select_index(2);
        assert_eq!(app.selected_path().unwrap()[1], "b");
        refresh(&mut app, &ctx, &mut session);
        assert!(spy.wait_for(&["t".to_string(), "b".to_string()], 1));
        assert!(!session.reload.is_running(), "a command is not the tool");

        app.select_index(0);
        refresh(&mut app, &ctx, &mut session);
        assert!(session.reload.is_running(), "the root reloads the tool");
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(spy.calls().len(), 1, "the root's r read no command");
    }
}
