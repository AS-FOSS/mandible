//! Bounded background warming of the whole tree (spec §5.2 step 4).
//!
//! Every node discovered in the tree is queued for a background fill on a
//! bounded pool — one thread per core, clamped to `[2, 8]` — starting from the root
//! as soon as the TUI opens, and cascading: each completed fill queues the
//! children it just discovered. The user never waits for the tree, and
//! never has to expand a node by hand to make it real.
//!
//! This replaces an earlier "one level ahead of what the user expanded"
//! policy. That policy kept the spawn count minimal, but it made an
//! unexpanded node *invisible to search* — the index can only contain what
//! has been extracted — and an empty node with nothing on screen
//! explaining that it needs a keypress reads as a bug, not as laziness.
//! Filling everything in the background costs the same total work spread
//! over idle time, and it is what makes a search over the whole tree
//! honest.
//!
//! What this is *not* is a return to eager extraction (spec §5.1): nothing
//! here blocks startup or any keystroke. [`MAX_WARMED_NODES`] bounds the
//! walk, and [`Warmer::cancel`] stops it on quit.
//!
//! Results are delivered back to the main loop over a channel it polls
//! each iteration (`Warmer::drain`), never by touching `App` from a
//! background thread.

use mandible_extract::{FillResult, ResolvedTool, Runner};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{mpsc, Arc};

/// Upper bound on how many nodes one session will warm in the background.
/// The cascade walks the whole reachable tree, and a few CLIs are enormous
/// (kubectl's subcommand tree runs to the hundreds, and each node costs a
/// subprocess spawn). This is a backstop against a pathological tree
/// turning a helpful prefetch into a spawn storm, not a tuning knob — a
/// tool that hits it still works, it just stops prefetching and falls back
/// to filling on expand.
const MAX_WARMED_NODES: usize = 4096;

/// One completed background fill, ready to be spliced into the tree by
/// the main loop.
pub struct WarmedNode {
    /// The path this fill was for.
    pub path: Vec<String>,
    /// The fill outcome.
    pub result: FillResult,
}

/// Owns a bounded thread pool and a cancellation flag; submits fills and
/// delivers completed ones back without ever touching `App` off the main
/// thread.
pub struct Warmer {
    pool: rayon::ThreadPool,
    cancelled: Arc<AtomicBool>,
    /// How many fills have been submitted against [`MAX_WARMED_NODES`]
    /// *for the current generation*. Reset by [`Warmer::reset`].
    submitted: AtomicUsize,
    /// Bumped by [`Warmer::reset`] to abandon everything in flight.
    ///
    /// A re-extract throws away the tree these jobs were filling, so their
    /// results describe a tree that no longer exists. They cannot be
    /// cancelled outright (a running probe is blocked in `wait()` and a
    /// rayon worker cannot be safely force-killed), so each job instead
    /// compares the generation it was submitted under against the current
    /// one and drops its result on the floor if they differ.
    generation: Arc<AtomicUsize>,
    tx: Sender<WarmedNode>,
    rx: Receiver<WarmedNode>,
}

impl Warmer {
    /// Build a warmer with one worker thread per core, clamped to
    /// `[2, 8]`.
    ///
    /// This used to be `available_parallelism * 4`, clamped to `[4, 32]`,
    /// on the theory that a warming job is not CPU work — it spawns
    /// `<tool> <path> --help` and blocks on the child. That theory is
    /// true for the typical small C tool and measured false for the
    /// tools where warming is heaviest: a `docker` invocation burns
    /// 70–100ms of real CPU per spawn (Go runtime startup plus a daemon
    /// round trip), so 16 concurrent probes on a 4-core machine was the
    /// warm pegging every core for minutes — the exact user report the
    /// resizing answers. One probe per core keeps the machine responsive
    /// for whatever else the user is doing; the price is a slower warm on
    /// cheap-probe trees, paid in background time nobody is waiting on.
    /// The user-expanded path still jumps the queue, so what is on
    /// screen fills at the same speed as before.
    pub fn new() -> Warmer {
        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            .clamp(2, 8);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(|i| format!("mandible-warm-{i}"))
            .build()
            .expect("failed to build the background warming thread pool");
        let (tx, rx) = mpsc::channel();
        Warmer {
            pool,
            cancelled: Arc::new(AtomicBool::new(false)),
            submitted: AtomicUsize::new(0),
            generation: Arc::new(AtomicUsize::new(0)),
            tx,
            rx,
        }
    }

    /// Submit a single node for a background fill. Returns `false` when
    /// the submission was refused — the app is quitting, or the
    /// [`MAX_WARMED_NODES`] budget is spent — so a cascading caller knows
    /// to stop walking.
    /// `tool` and `probe_path` say *what gets probed* — ordinarily the
    /// session's tool at this node's own path, but a convention-discovered
    /// node is probed against its own binary at a rebased path (spec §5.4,
    /// `crate::discovery::probe_target`). `path` stays the node's place in
    /// the tree, which is where the result is spliced.
    pub fn submit(
        &self,
        runner: Arc<Runner>,
        tool: ResolvedTool,
        probe_path: Vec<String>,
        path: Vec<String>,
        existing: mandible_core::CommandNode,
    ) -> bool {
        if self.cancelled.load(Ordering::Relaxed) {
            return false;
        }
        if self.submitted.fetch_add(1, Ordering::Relaxed) >= MAX_WARMED_NODES {
            return false;
        }
        let cancelled = Arc::clone(&self.cancelled);
        let generation = Arc::clone(&self.generation);
        let submitted_under = generation.load(Ordering::Relaxed);
        let tx = self.tx.clone();
        self.pool.spawn(move || {
            let stale = |g: &AtomicUsize| g.load(Ordering::Relaxed) != submitted_under;
            if cancelled.load(Ordering::Relaxed) || stale(&generation) {
                return;
            }
            let result = runner.fill_node(&tool, &probe_path, existing);
            // Checked again after the probe, not only before it: a
            // re-extract that lands mid-probe is exactly the case this
            // exists for, and splicing this result would put a node from
            // the discarded tree into the new one.
            if cancelled.load(Ordering::Relaxed) || stale(&generation) {
                return;
            }
            let _ = tx.send(WarmedNode { path, result });
        });
        true
    }

    /// Queue every direct child of `node` that isn't already
    /// known-complete for a background fill, and return the paths queued
    /// so the caller can mark them pending (which is what renders them as
    /// loading rows rather than as empty ones).
    ///
    /// Results from these fills are themselves cascaded by the event
    /// loop, so the whole reachable tree is walked progressively in the
    /// background rather than one level at a time on demand. That is a
    /// deliberate revision of the original "one level ahead" policy: a
    /// node the user had not personally expanded was not merely slow, it
    /// was *invisible to search*, and a lazily-empty node with nothing on
    /// screen to say it needs a keypress reads as a bug. Startup stays
    /// non-blocking either way — the difference is only how much gets
    /// filled while the user reads the first screen.
    ///
    /// [`MAX_WARMED_NODES`] keeps the walk from becoming unbounded on a
    /// very large tree.
    /// `root` is the whole tree, not just the node being walked: each
    /// child's probe target is resolved against it
    /// (`crate::discovery::probe_target`), because whether a node is probed
    /// through the session's tool or through a `<parent>-<sub>` binary of
    /// its own is a fact about its ancestors (spec §5.4). The node at `path`
    /// must already be spliced into `root` when this is called.
    pub fn warm_children(
        &self,
        runner: &Arc<Runner>,
        tool: &ResolvedTool,
        root: &mandible_core::CommandNode,
        path: &[String],
    ) -> Vec<Vec<String>> {
        let mut queued = Vec::new();
        let Some(node) = mandible_core::resolve(root, path) else {
            return queued;
        };
        for child in &node.subcommands {
            if child.children_filled {
                continue;
            }
            let mut child_path = path.to_vec();
            child_path.push(child.name.clone());
            let (child_tool, probe_path) = crate::discovery::probe_target(root, tool, &child_path);
            if !self.submit(
                Arc::clone(runner),
                child_tool,
                probe_path,
                child_path.clone(),
                child.clone(),
            ) {
                break;
            }
            queued.push(child_path);
        }
        queued
    }

    /// Drain any background fills that have completed since the last
    /// call, without blocking.
    pub fn drain(&self) -> Vec<WarmedNode> {
        self.rx.try_iter().collect()
    }

    /// Signal in-flight and not-yet-started jobs to skip their work on
    /// quit. Already-running jobs still run to completion — a std/rayon
    /// thread can't be safely force-killed — but nothing new is submitted
    /// and any job still queued becomes a no-op as soon as it's picked up.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    /// Abandon everything in flight and start a fresh warming budget,
    /// keeping the thread pool.
    ///
    /// Called on re-extract (`r`). Without it, that key was doubly
    /// unbounded: the previous whole-tree cascade kept running against a
    /// tree that had just been thrown away, so N re-extracts left N
    /// overlapping walks competing for the pool; and `submitted` is
    /// monotonic, so each one spent from a [`MAX_WARMED_NODES`] budget
    /// that was never replenished, until warming silently stopped working
    /// for the rest of the session.
    ///
    /// The pool itself is deliberately reused rather than rebuilt.
    /// Dropping a `rayon::ThreadPool` waits for its running jobs, and a
    /// job blocked on a probe can take up to the extraction timeout, so
    /// rebuilding would freeze the UI for exactly as long as the work we
    /// are trying to abandon. Bumping a generation gets the same result
    /// immediately: in-flight probes still finish, but their results are
    /// discarded instead of spliced.
    pub fn reset(&self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
        self.submitted.store(0, Ordering::Relaxed);
        // Results that arrived from the old generation but haven't been
        // drained yet describe the discarded tree too.
        while self.rx.try_recv().is_ok() {}
    }
}

impl Default for Warmer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandible_core::{CommandNode, Provenance, Source};

    fn warmed(path: &[&str]) -> WarmedNode {
        WarmedNode {
            path: path.iter().map(|s| s.to_string()).collect(),
            result: FillResult {
                node: CommandNode::new(
                    path.last().copied().unwrap_or("root"),
                    Provenance::single(Source::HelpText),
                ),
                tier_statuses: Vec::new(),
                elapsed: std::time::Duration::ZERO,
            },
        }
    }

    /// The pool must never exceed one thread per core (capped at 8): the
    /// old `cores * 4` oversubscription was measured pegging every core
    /// of a 4-core machine for minutes against a tool whose probes burn
    /// real CPU (docker). Asserting the bound, not an exact count, so the
    /// test holds on any machine.
    #[test]
    fn the_warming_pool_never_oversubscribes_the_cores() {
        let warmer = Warmer::new();
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        let threads = warmer.pool.current_num_threads();
        assert!(
            threads <= cores.max(2) && threads <= 8,
            "warming pool has {threads} threads on a {cores}-core machine"
        );
    }

    /// `submitted` is the [`MAX_WARMED_NODES`] budget, and it was
    /// monotonic: every re-extract spent from a pool that never refilled,
    /// so enough of them silently disabled background warming for the
    /// rest of the session.
    #[test]
    fn reset_replenishes_the_warming_budget() {
        let warmer = Warmer::new();
        warmer.submitted.store(MAX_WARMED_NODES, Ordering::Relaxed);
        warmer.reset();
        assert_eq!(warmer.submitted.load(Ordering::Relaxed), 0);
    }

    /// Results already sitting in the channel describe the tree that the
    /// re-extract just discarded, so they must not survive into the new
    /// one.
    #[test]
    fn reset_discards_undrained_results_from_the_old_generation() {
        let warmer = Warmer::new();
        warmer.tx.send(warmed(&["git", "rebase"])).unwrap();
        warmer.tx.send(warmed(&["git", "add"])).unwrap();

        warmer.reset();

        assert!(
            warmer.drain().is_empty(),
            "stale fills must not be spliced into the replacement tree"
        );
    }

    /// A job that finishes *after* a reset compares generations and drops
    /// its result. Exercised directly against the flag rather than through
    /// a real probe, since the point is the comparison, not the spawn.
    #[test]
    fn a_job_submitted_before_a_reset_is_stale_afterwards() {
        let warmer = Warmer::new();
        let submitted_under = warmer.generation.load(Ordering::Relaxed);
        warmer.reset();
        assert_ne!(
            warmer.generation.load(Ordering::Relaxed),
            submitted_under,
            "in-flight jobs detect abandonment by this counter changing"
        );
    }

    /// Draining is unaffected in the ordinary case: reset is the only
    /// thing that throws results away.
    #[test]
    fn drain_returns_results_when_nothing_was_reset() {
        let warmer = Warmer::new();
        warmer.tx.send(warmed(&["git", "rebase"])).unwrap();
        assert_eq!(warmer.drain().len(), 1);
    }
}
