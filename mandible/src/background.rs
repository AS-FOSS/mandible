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
//! Queued fills wait in [`Lanes`], which the pool's workers pull from: a
//! first-in first-out lane for the walk, plus one slot for the node the
//! user is looking at. That node is read ahead of everything queued, and
//! only the latest one holds the slot, so a cursor sweeping down the list
//! queues nothing for the rows it passes.
//!
//! Results are delivered back to the main loop over a channel it polls
//! each iteration (`Warmer::drain`), never by touching `App` from a
//! background thread.

use mandible_extract::{FillResult, KeptProbes, NodeHints, ResolvedTool, Runner};
use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{mpsc, Arc, Mutex, MutexGuard};

/// Upper bound on how many nodes the background walk will queue in one
/// session. The cascade walks the whole reachable tree, and a few CLIs are
/// enormous (kubectl's subcommand tree runs to the hundreds, and each node
/// costs a subprocess spawn). This is a backstop against a pathological
/// tree turning a helpful prefetch into a spawn storm, not a tuning knob.
/// It stops only the walk: a node the walk never queued is still probed
/// when the cursor lands on it or the user expands it ([`Warmer::prioritize`]).
const MAX_WARMED_NODES: usize = 4096;

/// One completed background fill, ready to be spliced into the tree by
/// the main loop.
pub struct WarmedNode {
    /// The path this fill was for.
    pub path: Vec<String>,
    /// The fill outcome.
    pub result: FillResult,
}

/// The probes behind one node's verbatim view, run because nothing was
/// kept for it.
pub struct RawFetched {
    /// The node the probes were for.
    pub path: Vec<String>,
    /// What they printed.
    pub kept: KeptProbes,
}

/// Owns a bounded thread pool and a cancellation flag; submits fills and
/// delivers completed ones back without ever touching `App` off the main
/// thread.
pub struct Warmer {
    pool: rayon::ThreadPool,
    lanes: Arc<Mutex<Lanes<Job>>>,
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
    raw_tx: Sender<RawFetched>,
    raw_rx: Receiver<RawFetched>,
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
        let (raw_tx, raw_rx) = mpsc::channel();
        Warmer {
            pool,
            lanes: Arc::new(Mutex::new(Lanes::default())),
            cancelled: Arc::new(AtomicBool::new(false)),
            submitted: AtomicUsize::new(0),
            generation: Arc::new(AtomicUsize::new(0)),
            tx,
            rx,
            raw_tx,
            raw_rx,
        }
    }

    /// Queue a single node for the background walk. Returns `false` when
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
        let job = self.job(runner, tool, probe_path, existing);
        lock(&self.lanes).push(path, job);
        self.spawn_worker();
        true
    }

    /// Ask for `path` to be read ahead of the background walk: the node the
    /// cursor is on, or one the user expanded. [`MAX_WARMED_NODES`] does not
    /// apply, so a node the walk never reached still loads.
    ///
    /// A node already queued moves to the front, one already running is left
    /// alone, and one never queued takes the front slot. Only one node holds
    /// the front at a time: asking for another gives the slot up, and a node
    /// that only held it is not probed. [`Prioritized::dropped`] names it, so
    /// the caller can stop showing it as loading.
    pub fn prioritize(
        &self,
        runner: Arc<Runner>,
        tool: ResolvedTool,
        probe_path: Vec<String>,
        path: Vec<String>,
        existing: mandible_core::CommandNode,
    ) -> Prioritized {
        if self.cancelled.load(Ordering::Relaxed) {
            return Prioritized::default();
        }
        let job = self.job(runner, tool, probe_path, existing);
        let (outcome, added) = lock(&self.lanes).prioritize(path, job);
        if added {
            self.spawn_worker();
        }
        outcome
    }

    /// [`Self::prioritize`] for the verbatim view: probe `path` for its
    /// raw output instead of filling it. Takes the same front slot, so
    /// only the row under the cursor is fetched. The result comes back
    /// through [`Self::drain_raw`].
    pub fn prioritize_raw(
        &self,
        runner: Arc<Runner>,
        tool: ResolvedTool,
        probe_path: Vec<String>,
        path: Vec<String>,
        existing: mandible_core::CommandNode,
    ) -> Prioritized {
        if self.cancelled.load(Ordering::Relaxed) {
            return Prioritized::default();
        }
        let mut job = self.job(runner, tool, probe_path, existing);
        job.raw = true;
        let (outcome, added) = lock(&self.lanes).prioritize(path, job);
        if added {
            self.spawn_worker();
        }
        outcome
    }

    fn job(
        &self,
        runner: Arc<Runner>,
        tool: ResolvedTool,
        probe_path: Vec<String>,
        existing: mandible_core::CommandNode,
    ) -> Job {
        Job {
            runner,
            tool,
            probe_path,
            existing,
            raw: false,
            submitted_under: self.generation.load(Ordering::Relaxed),
        }
    }

    /// Hand the pool one pull. Every queued job is matched by one of these,
    /// and a pull that finds the queue empty (its job was dropped or
    /// cleared) simply returns.
    fn spawn_worker(&self) {
        let lanes = Arc::clone(&self.lanes);
        let cancelled = Arc::clone(&self.cancelled);
        let generation = Arc::clone(&self.generation);
        let tx = self.tx.clone();
        let raw_tx = self.raw_tx.clone();
        self.pool.spawn(move || {
            if cancelled.load(Ordering::Relaxed) {
                return;
            }
            let Some((path, job)) = lock(&lanes).take() else {
                return;
            };
            if job.raw {
                let hints = NodeHints {
                    heading_attested: job.existing.heading_attested,
                    abbrev_probe_attested: job.existing.abbrev_probe_attested,
                    help_word_attested: job.existing.help_word_attested,
                };
                let probe =
                    || mandible_extract::help_text::probe_raw(&job.tool, &job.probe_path, hints);
                deliver_raw(
                    &cancelled,
                    &generation,
                    job.submitted_under,
                    &raw_tx,
                    path,
                    probe,
                );
                return;
            }
            let fill = || {
                job.runner
                    .fill_node(&job.tool, &job.probe_path, job.existing)
            };
            deliver(
                &cancelled,
                &generation,
                job.submitted_under,
                &tx,
                path,
                fill,
            );
        });
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

    /// Which tree the queued work belongs to; changes on [`Warmer::reset`].
    pub fn generation(&self) -> usize {
        self.generation.load(Ordering::Relaxed)
    }

    /// Drain any background fills that have completed since the last
    /// call, without blocking.
    pub fn drain(&self) -> Vec<WarmedNode> {
        let done: Vec<WarmedNode> = self.rx.try_iter().collect();
        let mut lanes = lock(&self.lanes);
        for node in &done {
            lanes.finish(&node.path);
        }
        done
    }

    /// Drain any raw fetches that have completed, without blocking.
    pub fn drain_raw(&self) -> Vec<RawFetched> {
        let done: Vec<RawFetched> = self.raw_rx.try_iter().collect();
        let mut lanes = lock(&self.lanes);
        for fetched in &done {
            lanes.finish(&fetched.path);
        }
        done
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
        lock(&self.lanes).clear();
        // Results that arrived from the old generation but haven't been
        // drained yet describe the discarded tree too.
        while self.rx.try_recv().is_ok() {}
        while self.raw_rx.try_recv().is_ok() {}
    }
}

/// Everything a worker needs to run one fill.
struct Job {
    runner: Arc<Runner>,
    tool: ResolvedTool,
    probe_path: Vec<String>,
    existing: mandible_core::CommandNode,
    /// Probe for the verbatim view only; the node is not filled.
    raw: bool,
    /// The generation current when the job was queued, which is what the
    /// result is checked against before it is delivered.
    submitted_under: usize,
}

/// What [`Warmer::prioritize`] did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Prioritized {
    /// The node is queued, running or already waiting at the front, so it
    /// will be filled. `false` only when the session is quitting.
    pub accepted: bool,
    /// A node that held the front slot until now and will not be probed.
    pub dropped: Option<Vec<String>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Send the result of `fill` unless the session quit or the tree was
/// re-extracted, checked before the probe and again after it. A re-extract
/// that lands mid-probe is the case the second check exists for: splicing
/// the result would put a node from the discarded tree into the new one.
fn deliver(
    cancelled: &AtomicBool,
    generation: &AtomicUsize,
    submitted_under: usize,
    tx: &Sender<WarmedNode>,
    path: Vec<String>,
    fill: impl FnOnce() -> FillResult,
) {
    if is_stale(cancelled, generation, submitted_under) {
        return;
    }
    let result = fill();
    if is_stale(cancelled, generation, submitted_under) {
        return;
    }
    let _ = tx.send(WarmedNode { path, result });
}

/// [`deliver`] for a raw fetch.
fn deliver_raw(
    cancelled: &AtomicBool,
    generation: &AtomicUsize,
    submitted_under: usize,
    tx: &Sender<RawFetched>,
    path: Vec<String>,
    probe: impl FnOnce() -> KeptProbes,
) {
    if is_stale(cancelled, generation, submitted_under) {
        return;
    }
    let kept = probe();
    if is_stale(cancelled, generation, submitted_under) {
        return;
    }
    let _ = tx.send(RawFetched { path, kept });
}

fn is_stale(cancelled: &AtomicBool, generation: &AtomicUsize, submitted_under: usize) -> bool {
    cancelled.load(Ordering::Relaxed) || generation.load(Ordering::Relaxed) != submitted_under
}

/// Waiting work, in the order workers take it.
///
/// `fifo` is the background walk. `front` is the one node the user asked
/// for that the walk never queued; the node to read first is `wanted`,
/// whether it sits in `fifo` or in `front`. `running` holds paths a worker
/// has taken and whose result the main loop has not yet collected.
struct Lanes<J> {
    fifo: VecDeque<(Vec<String>, J)>,
    front: Option<(Vec<String>, J)>,
    wanted: Option<Vec<String>>,
    running: HashSet<Vec<String>>,
}

impl<J> Default for Lanes<J> {
    fn default() -> Self {
        Lanes {
            fifo: VecDeque::new(),
            front: None,
            wanted: None,
            running: HashSet::new(),
        }
    }
}

impl<J> Lanes<J> {
    fn push(&mut self, path: Vec<String>, job: J) {
        self.fifo.push_back((path, job));
    }

    /// Put `path` first. Returns the outcome and whether a job was added
    /// (so the caller owes the pool one more pull).
    fn prioritize(&mut self, path: Vec<String>, job: J) -> (Prioritized, bool) {
        let mut dropped = None;
        if self.front.as_ref().is_some_and(|(p, _)| *p != path) {
            dropped = self.front.take().map(|(p, _)| p);
        }
        let queued = self.fifo.iter().any(|(p, _)| *p == path)
            || self.front.as_ref().is_some_and(|(p, _)| *p == path);
        if self.running.contains(&path) {
            self.wanted = None;
        } else if queued {
            self.wanted = Some(path);
        } else {
            self.wanted = Some(path.clone());
            self.front = Some((path, job));
            return (
                Prioritized {
                    accepted: true,
                    dropped,
                },
                true,
            );
        }
        (
            Prioritized {
                accepted: true,
                dropped,
            },
            false,
        )
    }

    fn take(&mut self) -> Option<(Vec<String>, J)> {
        let wanted = self.wanted.take();
        let mut next = None;
        if let Some(w) = wanted {
            if self.front.as_ref().is_some_and(|(p, _)| *p == w) {
                next = self.front.take();
            } else if let Some(i) = self.fifo.iter().position(|(p, _)| *p == w) {
                next = self.fifo.remove(i);
            }
        }
        let next = next.or_else(|| self.fifo.pop_front())?;
        self.running.insert(next.0.clone());
        Some(next)
    }

    fn finish(&mut self, path: &[String]) {
        self.running.remove(path);
    }

    fn clear(&mut self) {
        *self = Lanes::default();
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
                kept: KeptProbes::default(),
                timed_out: false,
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

    fn p(s: &str) -> Vec<String> {
        s.split(' ').map(String::from).collect()
    }

    fn inert_warmer() -> (Warmer, Arc<Runner>, ResolvedTool) {
        let tool = ResolvedTool {
            name: "git".into(),
            path: None,
            version: None,
        };
        (Warmer::new(), Arc::new(Runner::new(Vec::new())), tool)
    }

    fn wait_for(warmer: &Warmer, path: &[String]) -> bool {
        for _ in 0..200 {
            if warmer.drain().iter().any(|n| n.path == path) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        false
    }

    fn queued() -> Lanes<u32> {
        let mut l = Lanes::default();
        l.push(p("git a"), 1);
        l.push(p("git b"), 2);
        l.push(p("git c"), 3);
        l
    }

    fn order(mut l: Lanes<u32>) -> Vec<u32> {
        std::iter::from_fn(|| l.take().map(|(_, j)| j)).collect()
    }

    #[test]
    fn a_queued_node_that_is_prioritized_runs_before_the_rest() {
        let mut l = queued();
        let (out, added) = l.prioritize(p("git c"), 9);
        assert!(out.accepted && !added, "already queued: no new job");
        assert_eq!(order(l), vec![3, 1, 2]);
    }

    #[test]
    fn a_node_the_walk_never_queued_takes_the_front() {
        let mut l = queued();
        let (out, added) = l.prioritize(p("git z"), 9);
        assert!(out.accepted && added);
        assert_eq!(order(l), vec![9, 1, 2, 3]);
    }

    #[test]
    fn only_the_latest_prioritized_node_keeps_its_place_at_the_front() {
        let mut l = queued();
        l.prioritize(p("git c"), 9);
        l.prioritize(p("git b"), 9);
        assert_eq!(order(l), vec![2, 1, 3], "c went back to its own place");
    }

    #[test]
    fn a_node_that_only_held_the_front_is_dropped_and_never_probed() {
        let mut l = queued();
        l.prioritize(p("git y"), 8);
        let (out, _) = l.prioritize(p("git z"), 9);
        assert_eq!(out.dropped, Some(p("git y")));
        assert_eq!(order(l), vec![9, 1, 2, 3]);

        let mut l = queued();
        l.prioritize(p("git y"), 8);
        let (out, added) = l.prioritize(p("git b"), 9);
        assert_eq!(out.dropped, Some(p("git y")));
        assert!(!added);
        assert_eq!(order(l), vec![2, 1, 3]);
    }

    #[test]
    fn a_node_already_running_is_left_alone() {
        let mut l = queued();
        let (taken, _) = l.take().unwrap();
        let (out, added) = l.prioritize(taken, 9);
        assert!(out.accepted && !added);
        assert_eq!(order(l), vec![2, 3]);
    }

    #[test]
    fn a_node_that_finished_can_be_asked_for_again() {
        let mut l = queued();
        let (taken, _) = l.take().unwrap();
        l.finish(&taken);
        let (_, added) = l.prioritize(taken, 9);
        assert!(added);
    }

    #[test]
    fn past_the_cap_a_prioritized_node_still_loads() {
        let (warmer, runner, tool) = inert_warmer();
        warmer.submitted.store(MAX_WARMED_NODES, Ordering::Relaxed);
        let node = CommandNode::new("rebase", Provenance::single(Source::HelpText));
        let path = p("git rebase");
        assert!(!warmer.submit(
            Arc::clone(&runner),
            tool.clone(),
            path.clone(),
            path.clone(),
            node.clone()
        ));
        let out = warmer.prioritize(runner, tool, path.clone(), path.clone(), node);
        assert!(out.accepted);
        assert!(wait_for(&warmer, &path), "the fill never ran");
    }

    #[test]
    fn nothing_is_accepted_or_queued_once_the_session_is_quitting() {
        let (warmer, runner, tool) = inert_warmer();
        warmer.cancel();
        let node = CommandNode::new("rebase", Provenance::single(Source::HelpText));
        let path = p("git rebase");
        let out = warmer.prioritize(runner, tool, path.clone(), path, node);
        assert_eq!(out, Prioritized::default());
        assert!(lock(&warmer.lanes).take().is_none());
    }

    #[test]
    fn a_front_lane_result_is_dropped_after_quit_or_a_reset() {
        let (tx, rx) = mpsc::channel();
        let cancelled = AtomicBool::new(false);
        let generation = AtomicUsize::new(0);
        let fill = || warmed(&["git", "rebase"]).result;

        let mut l: Lanes<usize> = Lanes::default();
        l.prioritize(p("git rebase"), generation.load(Ordering::Relaxed));
        let (path, under) = l.take().unwrap();

        deliver(&cancelled, &generation, under, &tx, path.clone(), || {
            generation.fetch_add(1, Ordering::Relaxed);
            fill()
        });
        assert!(rx.try_recv().is_err(), "reset mid-probe must drop it");

        let under = generation.load(Ordering::Relaxed);
        cancelled.store(true, Ordering::Relaxed);
        deliver(&cancelled, &generation, under, &tx, path.clone(), fill);
        assert!(rx.try_recv().is_err(), "quit must drop it");

        cancelled.store(false, Ordering::Relaxed);
        deliver(&cancelled, &generation, under, &tx, path, fill);
        assert!(rx.try_recv().is_ok(), "control: a live job delivers");
    }

    #[test]
    fn a_raw_fetch_result_is_dropped_after_quit_or_a_reset() {
        let (tx, rx) = mpsc::channel();
        let cancelled = AtomicBool::new(false);
        let generation = AtomicUsize::new(0);
        let probe = || KeptProbes::default();

        deliver_raw(&cancelled, &generation, 0, &tx, p("git rebase"), || {
            generation.fetch_add(1, Ordering::Relaxed);
            probe()
        });
        assert!(rx.try_recv().is_err(), "reset mid-probe must drop it");

        cancelled.store(true, Ordering::Relaxed);
        deliver_raw(&cancelled, &generation, 1, &tx, p("git rebase"), probe);
        assert!(rx.try_recv().is_err(), "quit must drop it");

        cancelled.store(false, Ordering::Relaxed);
        deliver_raw(&cancelled, &generation, 1, &tx, p("git rebase"), probe);
        assert!(rx.try_recv().is_ok(), "control: a live fetch delivers");
    }

    #[test]
    fn reset_empties_the_front_lane_too() {
        let (warmer, runner, tool) = inert_warmer();
        let node = CommandNode::new("rebase", Provenance::single(Source::HelpText));
        let mut l = lock(&warmer.lanes);
        l.prioritize(
            p("git rebase"),
            warmer.job(runner, tool, p("git rebase"), node),
        );
        drop(l);
        warmer.reset();
        assert!(lock(&warmer.lanes).take().is_none());
    }
}
