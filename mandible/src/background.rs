//! Bounded background warming of the whole tree (spec §5.2 step 4).
//!
//! Every node discovered in the tree is queued for a background fill on a
//! bounded pool — `min(8, available_parallelism)` — starting from the root
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
    /// How many fills have been submitted this session, against
    /// [`MAX_WARMED_NODES`].
    submitted: AtomicUsize,
    tx: Sender<WarmedNode>,
    rx: Receiver<WarmedNode>,
}

impl Warmer {
    /// Build a warmer with `min(8, available_parallelism)` worker threads.
    pub fn new() -> Warmer {
        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            .min(8);
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
            tx,
            rx,
        }
    }

    /// Submit a single node for a background fill. Returns `false` when
    /// the submission was refused — the app is quitting, or the
    /// [`MAX_WARMED_NODES`] budget is spent — so a cascading caller knows
    /// to stop walking.
    pub fn submit(
        &self,
        runner: Arc<Runner>,
        tool: ResolvedTool,
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
        let tx = self.tx.clone();
        self.pool.spawn(move || {
            if cancelled.load(Ordering::Relaxed) {
                return;
            }
            let result = runner.fill_node(&tool, &path, existing);
            if cancelled.load(Ordering::Relaxed) {
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
    pub fn warm_children(
        &self,
        runner: &Arc<Runner>,
        tool: &ResolvedTool,
        node: &mandible_core::CommandNode,
        path: &[String],
    ) -> Vec<Vec<String>> {
        let mut queued = Vec::new();
        for child in &node.subcommands {
            if child.children_filled {
                continue;
            }
            let mut child_path = path.to_vec();
            child_path.push(child.name.clone());
            if !self.submit(
                Arc::clone(runner),
                tool.clone(),
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
}

impl Default for Warmer {
    fn default() -> Self {
        Self::new()
    }
}
