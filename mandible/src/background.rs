//! Bounded background warming of the next tree depth (spec §5.2 step 4).
//!
//! After a node the user just expanded finishes its own lazy fill (spec
//! §5.2 step 3), its direct children are spec­ulatively pre-warmed one
//! level ahead on a bounded pool — `min(8, available_parallelism)` — so
//! that descending further often finds an answer already in the tree.
//! Warming is deliberately *not* recursive past that one extra level:
//! doing so would eventually warm an entire large tree (`kubectl` has
//! hundreds of subcommands) in the background, which is exactly the
//! eager-extraction cost problem spec §5.1 exists to avoid, just moved off
//! the main thread instead of solved.
//!
//! Results are delivered back to the main loop over a channel it polls
//! each iteration (`Warmer::drain`), never by touching `App` from a
//! background thread.

use mandible_extract::{FillResult, ResolvedTool, Runner};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{mpsc, Arc};

/// One completed background fill, ready to be spliced into the tree by
/// the main loop.
pub struct WarmedNode {
    /// The path this fill was for.
    pub path: Vec<String>,
    /// The fill outcome.
    pub result: FillResult,
    /// True if this was the fill the user directly triggered by
    /// expanding a node (as opposed to a speculative one-level-ahead
    /// warm) — the main loop uses this to decide whether to, in turn,
    /// warm *this* node's own children.
    pub warm_children: bool,
}

/// Owns a bounded thread pool and a cancellation flag; submits fills and
/// delivers completed ones back without ever touching `App` off the main
/// thread.
pub struct Warmer {
    pool: rayon::ThreadPool,
    cancelled: Arc<AtomicBool>,
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
            tx,
            rx,
        }
    }

    /// Submit a single node for a background fill.
    pub fn submit(
        &self,
        runner: Arc<Runner>,
        tool: ResolvedTool,
        path: Vec<String>,
        existing: mandible_core::CommandNode,
        warm_children: bool,
    ) {
        if self.cancelled.load(Ordering::Relaxed) {
            return;
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
            let _ = tx.send(WarmedNode {
                path,
                result,
                warm_children,
            });
        });
    }

    /// Speculatively pre-fill every direct child of `node` that isn't
    /// already known-complete, one background job each, one level deep
    /// (results from these do not themselves trigger further warming).
    pub fn warm_children(
        &self,
        runner: &Arc<Runner>,
        tool: &ResolvedTool,
        node: &mandible_core::CommandNode,
        path: &[String],
    ) {
        for child in &node.subcommands {
            if child.children_filled {
                continue;
            }
            let mut child_path = path.to_vec();
            child_path.push(child.name.clone());
            self.submit(
                Arc::clone(runner),
                tool.clone(),
                child_path,
                child.clone(),
                false,
            );
        }
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
