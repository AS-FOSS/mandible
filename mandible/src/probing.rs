//! What the event loop asks of the probe queue, and what it shows of the
//! answer.
//!
//! Every trigger builds the same [`Request`] and sends it through
//! [`request`]: the root fill at startup, the cascade of a finished fill,
//! the cursor coming to rest, `→`/`Enter`, the verbatim view's fetch and `r`.
//! Whether a command is new, waiting, running or done is the queue's to
//! decide (`crate::queue`); this module only records the answer in the
//! command's probe record.

use crate::background::{Spec, Warmer};
use crate::queue::{Decision, Limit, Request, Urgency};
use mandible_extract::{FillResult, PathSibling, ResolvedTool, Runner};
use mandible_tui::app::App;
use mandible_tui::probes::{FillEnd, Outcome};
use std::sync::Arc;

/// What a request needs beside the app.
pub struct Ctx<'a> {
    /// Runs the fills.
    pub runner: &'a Arc<Runner>,
    /// The session's tool.
    pub resolved: &'a ResolvedTool,
    /// The queue and its pool.
    pub warmer: &'a Warmer,
}

/// Ask for `path` to be probed, and show the answer. A node whose children
/// came with the tree has nothing to probe unless `force` asks for it.
pub fn request(
    app: &mut App,
    ctx: &Ctx,
    path: Vec<String>,
    urgency: Urgency,
    force: bool,
    limit: Limit,
) -> Decision {
    let req = Request {
        path: &path,
        urgency,
        force,
        limit,
    };
    let root = &app.root;
    let answer = ctx.warmer.request(&req, || {
        let node = mandible_core::resolve(root, &path)?;
        if node.children_filled && !force {
            return None;
        }
        let existing = node.clone();
        // Same redirect the background cascade applies (spec §5.4): a node
        // discovered by convention probes its own binary, never the parent
        // with a guessed word.
        let (tool, probe_path) = crate::discovery::probe_target(root, ctx.resolved, &path);
        Some(Spec {
            runner: Arc::clone(ctx.runner),
            tool,
            probe_path,
            existing,
        })
    });
    if matches!(answer.decision, Decision::Queued | Decision::Promoted) {
        app.mark_pending(path);
    }
    if let Some(dropped) = answer.dropped {
        app.clear_pending(&dropped);
        app.clear_raw_pending(&dropped);
    }
    answer.decision
}

/// Queue every direct child of `path` for the background walk. After a
/// forced fill (`rewalk`), a child that was read before is read again.
pub fn cascade(app: &mut App, ctx: &Ctx, path: &[String], rewalk: bool) {
    let Some(node) = mandible_core::resolve(&app.root, path) else {
        return;
    };
    let names: Vec<String> = node.subcommands.iter().map(|c| c.name.clone()).collect();
    for name in names {
        let mut child = path.to_vec();
        child.push(name);
        let force = rewalk && app.probes.get(&child).is_some();
        let decision = request(app, ctx, child, Urgency::Walk, force, Limit::Standard);
        if decision == Decision::Refused {
            break;
        }
    }
}

/// The row under the cursor, once the cursor has rested: read it, and read
/// it again for the verbatim view when that view is waiting on output nothing
/// kept. Called when no input arrived during a poll, so a held arrow key asks
/// for nothing on the rows it passes.
pub fn at_rest(app: &mut App, ctx: &Ctx) {
    let Some(path) = app.selected_path() else {
        return;
    };
    request(
        app,
        ctx,
        path.clone(),
        Urgency::User,
        false,
        Limit::Standard,
    );
    if app.raw_mode && app.raw_is_pending(&path) && app.is_loaded(&path) && !app.is_pending(&path) {
        request(app, ctx, path, Urgency::User, true, Limit::Standard);
    }
}

/// The limit `r` gives `path`: longer when its last read timed out.
pub fn retry_limit(app: &App, path: &[String]) -> Limit {
    if app.probe_outcome(path) == Some(Outcome::TimedOut) {
        Limit::Long
    } else {
        Limit::Standard
    }
}

/// `r` on a command's page: read that command again, and then its
/// subcommands. The page on screen stays until the new result lands. A
/// second `r` while it is being read does nothing.
pub fn reread(app: &mut App, ctx: &Ctx, path: Vec<String>) {
    if app.probes.is_running(&path) {
        return;
    }
    let limit = retry_limit(app, &path);
    let name = path.last().cloned().unwrap_or_default();
    request(app, ctx, path, Urgency::User, true, limit);
    app.set_status(format!("re-reading {name}"));
}

/// Splice in every fill that finished since the last call, record how each
/// ended, and queue the children each one found.
pub fn collect(app: &mut App, ctx: &Ctx, siblings: &[PathSibling]) {
    for path in ctx.warmer.drain_started() {
        app.mark_running(&path);
    }
    for warmed in ctx.warmer.drain() {
        let end = fill_end(&warmed.result);
        let kept = warmed.result.kept;
        let mut node = warmed.result.node;
        // The root's children are the tool's own documented commands plus
        // whatever the `<tool>-<sub>` convention finds on PATH (spec §5.4).
        // Attached at the root only, because that is where the convention
        // lives: cargo dispatches `cargo clippy` to `cargo-clippy`, and
        // nothing dispatches `cargo clippy fix` to `cargo-clippy-fix`.
        if warmed.path.len() == 1 {
            crate::discovery::attach_path_siblings(&mut node, siblings);
        }
        // Spliced before the cascade, not after: each child's probe target
        // is resolved against the tree, so the node it is about has to be
        // in the tree first.
        let evicted = app.finish_fill(&warmed.path, node, end, kept);
        // Cascade unconditionally: every fill queues the children it just
        // discovered, so the background walk reaches the whole tree instead
        // of stopping one level past whatever the user expanded. Marking
        // them pending is what makes them render as loading rows rather
        // than as silently empty ones.
        cascade(app, ctx, &warmed.path, warmed.forced);
        // The node `mandible <tool> <sub>` asked for may have just arrived,
        // or may now be known not to exist (spec §5.4).
        app.settle_requested_path();
        crate::raw_view::after_fill(app, ctx.resolved, &warmed.path, evicted, warmed.forced);
    }
}

/// How a fill ended, for the command's probe record.
fn fill_end(result: &FillResult) -> FillEnd {
    let tried: Vec<_> = result.tier_statuses.iter().filter(|t| t.detected).collect();
    if result.timed_out {
        FillEnd::TimedOut
    } else if !tried.is_empty() && tried.iter().all(|t| t.error.is_some()) {
        FillEnd::Failed
    } else {
        FillEnd::Read
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doubles::Spy;
    use crate::queue::LONG_LIMIT;
    use mandible_core::{CommandNode, Provenance, Source};
    use std::time::{Duration, Instant};

    fn p(s: &str) -> Vec<String> {
        s.split(' ').map(String::from).collect()
    }

    struct Rig {
        spy: Arc<Spy>,
        runner: Arc<Runner>,
        resolved: ResolvedTool,
        warmer: Warmer,
        app: App,
    }

    impl Rig {
        /// A tool `t` with commands a (holding a1 and a2) and b.
        fn new(threads: usize) -> Rig {
            let spy = Spy::new();
            spy.children("t", &["a", "b"]);
            spy.children("a", &["a1", "a2"]);
            let mut root = CommandNode::new("t", Provenance::single(Source::HelpText));
            root.heading_attested = true;
            let app = App::new("t".to_string(), root);
            Rig {
                runner: spy.runner(),
                resolved: Spy::tool(),
                warmer: Warmer::with_threads(threads),
                spy,
                app,
            }
        }

        fn rest(&mut self) {
            let ctx = Ctx {
                runner: &self.runner,
                resolved: &self.resolved,
                warmer: &self.warmer,
            };
            at_rest(&mut self.app, &ctx);
        }

        fn r(&mut self, path: &str) {
            let ctx = Ctx {
                runner: &self.runner,
                resolved: &self.resolved,
                warmer: &self.warmer,
            };
            reread(&mut self.app, &ctx, p(path));
        }

        fn collect(&mut self) {
            let ctx = Ctx {
                runner: &self.runner,
                resolved: &self.resolved,
                warmer: &self.warmer,
            };
            collect(&mut self.app, &ctx, &[]);
        }

        fn ask(&mut self, path: &str, urgency: Urgency, force: bool) -> Decision {
            let ctx = Ctx {
                runner: &self.runner,
                resolved: &self.resolved,
                warmer: &self.warmer,
            };
            request(
                &mut self.app,
                &ctx,
                p(path),
                urgency,
                force,
                Limit::Standard,
            )
        }

        /// Collect results until nothing is in flight.
        fn settle(&mut self) {
            let end = Instant::now() + Duration::from_secs(10);
            loop {
                let ctx = Ctx {
                    runner: &self.runner,
                    resolved: &self.resolved,
                    warmer: &self.warmer,
                };
                collect(&mut self.app, &ctx, &[]);
                let busy = ["t", "t a", "t b", "t a a1", "t a a2"]
                    .iter()
                    .any(|path| self.app.is_pending(&p(path)));
                if !busy || Instant::now() > end {
                    return;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        }

        /// The whole tree read once.
        fn walked(threads: usize) -> Rig {
            let mut rig = Rig::new(threads);
            rig.ask("t", Urgency::Walk, true);
            rig.settle();
            rig.app.select_index(0);
            rig.spy.forget();
            rig
        }

        fn select(&mut self, path: &str) {
            let path = p(path);
            self.app.ensure_rows_fresh();
            for name in 2..path.len() {
                self.app.toggle_expand_path(&path[..name]);
            }
            self.app.ensure_rows_fresh();
            let at = (0..32)
                .find(|&i| {
                    self.app.select_index(i);
                    self.app.selected_path().as_ref() == Some(&path)
                })
                .expect("the row exists");
            self.app.select_index(at);
        }
    }

    #[test]
    fn a_refused_request_leaves_no_pending_mark() {
        let mut rig = Rig::walked(2);
        rig.warmer.cancel();
        let refused = rig.ask("t b", Urgency::User, true);
        assert_eq!(refused, Decision::Refused);
        assert!(!rig.app.is_pending(&p("t b")));
    }

    #[test]
    fn the_walk_reads_the_whole_tree_once() {
        let mut rig = Rig::new(3);
        rig.ask("t", Urgency::Walk, true);
        rig.settle();
        for path in ["t", "t a", "t b", "t a a1", "t a a2"] {
            assert_eq!(rig.spy.count(&p(path)), 1, "{path}");
            assert!(rig.app.fill_finished(&p(path)), "{path}");
        }
    }

    /// Every trigger asks for a command the walk has waiting, and none of
    /// them adds it a second time.
    #[test]
    fn every_trigger_promotes_a_waiting_command_without_a_duplicate() {
        type Trigger = fn(&mut Rig);
        let triggers: [(&str, Trigger); 5] = [
            ("cursor rest", |rig| {
                rig.select("t b");
                rig.rest();
            }),
            ("expand", |rig| {
                rig.ask("t b", Urgency::User, false);
            }),
            ("t fallback", |rig| {
                rig.ask("t b", Urgency::User, true);
            }),
            ("r", |rig| rig.r("t b")),
            ("walk", |rig| {
                rig.ask("t b", Urgency::Walk, false);
            }),
        ];
        for (name, trigger) in triggers {
            let mut rig = Rig::new(2);
            rig.spy.hold("a");
            rig.ask("t", Urgency::Walk, true);
            rig.settle_root_only();
            assert!(rig.spy.wait_for(&p("t a"), 1), "{name}");
            assert!(rig.app.is_pending(&p("t b")), "{name}: b is waiting");

            trigger(&mut rig);
            rig.spy.release();
            rig.settle();

            assert_eq!(rig.spy.count(&p("t b")), 1, "{name} read b twice");
        }
    }

    impl Rig {
        /// Collect the root's fill only; the walk below it stays queued.
        fn settle_root_only(&mut self) {
            assert!(self.spy.wait_for(&p("t"), 1));
            let end = Instant::now() + Duration::from_secs(10);
            while !self.app.fill_finished(&p("t")) && Instant::now() < end {
                let ctx = Ctx {
                    runner: &self.runner,
                    resolved: &self.resolved,
                    warmer: &self.warmer,
                };
                collect(&mut self.app, &ctx, &[]);
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }

    #[test]
    fn r_on_a_command_reads_that_command_and_its_subcommands_only() {
        let mut rig = Rig::walked(3);
        rig.r("t a");
        rig.settle();

        let mut read: Vec<String> = rig.spy.calls().iter().map(|(c, _)| c.join(" ")).collect();
        read.sort();
        assert_eq!(read, vec!["t a", "t a a1", "t a a2"]);
    }

    #[test]
    fn r_on_a_command_reads_it_first_and_its_subcommands_at_walk_urgency() {
        let mut rig = Rig::walked(2);
        rig.r("t a");
        rig.settle();
        let calls = rig.spy.calls();
        assert_eq!(calls[0].0, p("t a"), "the command comes first");
        assert!(calls[1..].iter().all(|(c, _)| c.len() == 3));
    }

    #[test]
    fn a_command_whose_last_read_timed_out_gets_the_long_limit_on_r() {
        let mut rig = Rig::new(3);
        rig.spy.time_out("a", true);
        rig.ask("t", Urgency::Walk, true);
        rig.settle();
        assert_eq!(
            rig.app.probe_outcome(&p("t a")),
            Some(Outcome::TimedOut),
            "precondition"
        );
        assert_eq!(rig.app.probe_outcome(&p("t b")), Some(Outcome::Empty));
        rig.spy.forget();
        rig.spy.time_out("a", false);

        rig.r("t a");
        rig.r("t b");
        rig.settle();

        let calls = rig.spy.calls();
        let limit_of = |path: &str| calls.iter().find(|(c, _)| *c == p(path)).unwrap().1;
        assert_eq!(limit_of("t a"), LONG_LIMIT);
        assert_eq!(limit_of("t b"), Duration::from_secs(30), "control");
        assert_eq!(rig.app.probe_outcome(&p("t a")), Some(Outcome::Parsed));
    }

    #[test]
    fn a_second_r_on_the_same_command_while_it_runs_does_nothing() {
        let mut rig = Rig::walked(3);
        rig.spy.hold("b");
        rig.r("t b");
        assert!(rig.spy.wait_for(&p("t b"), 1));
        rig.collect();
        assert!(rig.app.probes.is_running(&p("t b")));

        rig.r("t b");
        rig.r("t b");
        rig.spy.release();
        rig.settle();
        std::thread::sleep(Duration::from_millis(150));
        rig.settle();

        assert_eq!(rig.spy.count(&p("t b")), 1);
    }

    #[test]
    fn the_old_page_stays_until_the_new_result_lands() {
        let mut rig = Rig::walked(3);
        rig.spy.hold("a");
        rig.r("t a");
        assert!(rig.spy.wait_for(&p("t a"), 1));
        assert!(rig.app.is_pending(&p("t a")));
        let node = mandible_core::resolve(&rig.app.root, &p("t a")).unwrap();
        assert_eq!(node.subcommands.len(), 2, "the page was emptied");
        assert_eq!(rig.app.probe_outcome(&p("t a")), Some(Outcome::Parsed));
        rig.spy.release();
        rig.settle();
    }

    #[test]
    fn a_timed_out_command_is_recorded_as_such_and_not_as_empty() {
        let mut rig = Rig::new(3);
        rig.spy.time_out("b", true);
        rig.ask("t", Urgency::Walk, true);
        rig.settle();
        assert_eq!(rig.app.probe_outcome(&p("t b")), Some(Outcome::TimedOut));
        assert_eq!(rig.app.probe_outcome(&p("t a")), Some(Outcome::Parsed));
    }

    #[test]
    fn the_verbatim_views_fetch_is_a_forced_request_through_the_queue() {
        let mut rig = Rig::walked(3);
        rig.select("t b");
        rig.app.toggle_raw_mode();
        rig.app.mark_raw_pending(p("t b"));
        rig.rest();
        rig.rest();
        assert!(rig.spy.wait_for(&p("t b"), 1));
        rig.settle();
        assert_eq!(rig.spy.count(&p("t b")), 1);
    }

    #[test]
    fn the_cursor_resting_on_a_read_command_asks_for_nothing() {
        let mut rig = Rig::walked(3);
        rig.select("t b");
        for _ in 0..5 {
            rig.rest();
        }
        std::thread::sleep(Duration::from_millis(100));
        assert!(rig.spy.calls().is_empty());
    }
}
