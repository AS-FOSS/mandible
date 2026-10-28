//! Bounded background warming of the whole tree (spec §5.2 step 4).
//!
//! Every node discovered in the tree is queued for a background fill on a
//! bounded pool — one thread per core, clamped to `[2, 8]` — starting from the root
//! as soon as the TUI opens, and cascading: each completed fill queues the
//! children it just discovered. The user never waits for the tree, and
//! never has to expand a node by hand to make it real.
//!
//! Filling everything in the background costs the same total work spread
//! over idle time, and it is what makes a search over the whole tree
//! honest: the index can only contain what has been extracted.
//!
//! What this is *not* is a return to eager extraction (spec §5.1): nothing
//! here blocks startup or any keystroke. [`MAX_WARMED_NODES`] bounds the
//! walk, and [`Warmer::cancel`] stops it on quit.
//!
//! Every fill is asked for through [`Warmer::request`], and the
//! [`crate::queue::Queue`] behind it decides what runs and in what order.
//! One thread of the pool is kept for the user's requests; the walk uses
//! the rest.
//!
//! Results are delivered back to the main loop over a channel it polls
//! each iteration (`Warmer::drain`), never by touching `App` from a
//! background thread.

use crate::queue::{Answer, Decision, Entry, Queue, Request, Share};
use mandible_extract::{FillResult, ResolvedTool, Runner};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{mpsc, Arc, Mutex, MutexGuard};

/// Upper bound on how many nodes the background walk will queue in one
/// session. The cascade walks the whole reachable tree, and a few CLIs are
/// enormous (kubectl's subcommand tree runs to the hundreds, and each node
/// costs a subprocess spawn). This is a backstop against a pathological
/// tree turning a helpful prefetch into a spawn storm, not a tuning knob.
/// It stops only the walk: a node the walk never queued is still probed
/// when the cursor lands on it or the user expands it.
pub const MAX_WARMED_NODES: usize = 4096;

/// One completed background fill, ready to be spliced into the tree by
/// the main loop.
pub struct WarmedNode {
    /// The path this fill was for.
    pub path: Vec<String>,
    /// The fill outcome.
    pub result: FillResult,
    /// The request was forced, so the children it finds are read again too.
    pub forced: bool,
}

/// What one fill probes: `tool` and `probe_path` say *what gets probed* —
/// ordinarily the session's tool at this node's own path, but a
/// convention-discovered node is probed against its own binary at a rebased
/// path (spec §5.4, `crate::discovery::probe_target`). `existing` is the
/// node already in the tree.
pub struct Spec {
    /// Runs the fill.
    pub runner: Arc<Runner>,
    /// The binary to probe.
    pub tool: ResolvedTool,
    /// The path to probe it at.
    pub probe_path: Vec<String>,
    /// The node as the tree has it now.
    pub existing: mandible_core::CommandNode,
}

/// A queued fill and the generation it was queued under, which is what its
/// result is checked against before it is delivered.
struct Job {
    spec: Spec,
    submitted_under: usize,
}

/// What the pool's workers and the main loop share.
struct Shared {
    queue: Mutex<Queue<Job>>,
    cancelled: AtomicBool,
    /// Bumped by [`Warmer::reset`] to abandon everything in flight.
    ///
    /// A re-extract throws away the tree these jobs were filling, so their
    /// results describe a tree that no longer exists. They cannot be
    /// cancelled outright (a running probe is blocked in `wait()` and a
    /// rayon worker cannot be safely force-killed), so each job instead
    /// compares the generation it was submitted under against the current
    /// one and drops its result on the floor if they differ.
    generation: AtomicUsize,
    tx: Sender<WarmedNode>,
    started: Sender<Vec<String>>,
}

/// Owns a bounded thread pool and a cancellation flag; runs the queue's
/// fills and delivers completed ones back without ever touching `App` off
/// the main thread.
pub struct Warmer {
    pool: rayon::ThreadPool,
    shared: Arc<Shared>,
    rx: Receiver<WarmedNode>,
    started_rx: Receiver<Vec<String>>,
}

impl Warmer {
    /// Build a warmer with one worker thread per core, clamped to
    /// `[2, 8]`.
    ///
    /// One probe per core keeps the machine responsive for whatever else
    /// the user is doing: a `docker` invocation burns 70–100ms of real CPU
    /// per spawn, so a pool several times the core count pegged every core
    /// for minutes. The price is a slower warm on cheap-probe trees, paid
    /// in background time nobody is waiting on.
    pub fn new() -> Warmer {
        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        Self::with_threads(threads)
    }

    /// [`Self::new`] over a pool of `threads`, clamped to `[2, 8]`.
    pub fn with_threads(threads: usize) -> Warmer {
        let threads = threads.clamp(2, 8);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(|i| format!("mandible-warm-{i}"))
            .build()
            .expect("failed to build the background warming thread pool");
        let (tx, rx) = mpsc::channel();
        let (started, started_rx) = mpsc::channel();
        Warmer {
            pool,
            shared: Arc::new(Shared {
                queue: Mutex::new(Queue::new(threads, MAX_WARMED_NODES)),
                cancelled: AtomicBool::new(false),
                generation: AtomicUsize::new(0),
                tx,
                started,
            }),
            rx,
            started_rx,
        }
    }

    /// Ask for `req.path` to be probed. The queue decides what that means:
    /// see [`crate::queue`]. `spec` builds what the probe needs and runs
    /// only when the request is new.
    pub fn request(&self, req: &Request, spec: impl FnOnce() -> Option<Spec>) -> Answer {
        if self.shared.cancelled.load(Ordering::Relaxed) {
            return Answer {
                decision: Decision::Refused,
                dropped: None,
            };
        }
        let under = self.shared.generation.load(Ordering::Relaxed);
        let answer = lock(&self.shared.queue).request(req, || {
            spec().map(|spec| Job {
                spec,
                submitted_under: under,
            })
        });
        self.pump();
        answer
    }

    /// Hand the pool every entry a free thread may run now.
    fn pump(&self) {
        loop {
            let next = lock(&self.shared.queue).take();
            let Some((entry, share)) = next else { return };
            let shared = Arc::clone(&self.shared);
            self.pool.spawn(move || run_chain(&shared, entry, share));
        }
    }

    /// Drain any background fills that have completed since the last
    /// call, without blocking.
    pub fn drain(&self) -> Vec<WarmedNode> {
        let done: Vec<WarmedNode> = self.rx.try_iter().collect();
        {
            let mut queue = lock(&self.shared.queue);
            for node in &done {
                queue.finish(&node.path);
            }
        }
        self.pump();
        done
    }

    /// The paths a worker has started on since the last call.
    pub fn drain_started(&self) -> Vec<Vec<String>> {
        self.started_rx.try_iter().collect()
    }

    /// Signal in-flight and not-yet-started jobs to skip their work on
    /// quit. Already-running jobs still run to completion — a std/rayon
    /// thread can't be safely force-killed — but nothing new is submitted
    /// and nothing waiting is started.
    pub fn cancel(&self) {
        self.shared.cancelled.store(true, Ordering::Relaxed);
        lock(&self.shared.queue).clear_waiting();
    }

    /// Abandon everything in flight and start a fresh warming budget,
    /// keeping the thread pool.
    ///
    /// Called on re-extract (`r` on the tool's root). Without it, the
    /// previous whole-tree cascade kept running against a tree that had just
    /// been thrown away, and `submitted` is monotonic, so each re-extract
    /// spent from a [`MAX_WARMED_NODES`] budget that was never replenished.
    ///
    /// The pool itself is deliberately reused rather than rebuilt.
    /// Dropping a `rayon::ThreadPool` waits for its running jobs, and a
    /// job blocked on a probe can take up to the extraction timeout, so
    /// rebuilding would freeze the UI for exactly as long as the work we
    /// are trying to abandon. Bumping a generation gets the same result
    /// immediately: in-flight probes still finish, but their results are
    /// discarded instead of spliced.
    pub fn reset(&self) {
        self.shared.generation.fetch_add(1, Ordering::Relaxed);
        lock(&self.shared.queue).reset();
        // Results that arrived from the old generation but haven't been
        // drained yet describe the discarded tree too.
        while self.rx.try_recv().is_ok() {}
        while self.started_rx.try_recv().is_ok() {}
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Run `first`, then whatever the queue hands this thread next, until it
/// has nothing for it.
fn run_chain(shared: &Shared, first: Entry<Job>, share: Share) {
    let mut current = Some((first, share));
    while let Some((entry, share)) = current.take() {
        run_one(shared, entry);
        let mut queue = lock(&shared.queue);
        queue.release(share);
        if !shared.cancelled.load(Ordering::Relaxed) {
            current = queue.take();
        }
    }
}

/// Probe one entry and deliver its result unless the session quit or the
/// tree was re-extracted, checked before the probe and again after it. A
/// re-extract that lands mid-probe is the case the second check exists for:
/// splicing the result would put a node from the discarded tree into the
/// new one.
fn run_one(shared: &Shared, entry: Entry<Job>) {
    let Entry {
        path,
        job,
        force,
        limit,
    } = entry;
    if is_stale(shared, job.submitted_under) {
        return;
    }
    let _ = shared.started.send(path.clone());
    let spec = job.spec;
    let result = mandible_extract::exec::with_extract_limit(limit.duration(), || {
        spec.runner
            .fill_node(&spec.tool, &spec.probe_path, spec.existing)
    });
    if is_stale(shared, job.submitted_under) {
        return;
    }
    let _ = shared.tx.send(WarmedNode {
        path,
        result,
        forced: force,
    });
}

fn is_stale(shared: &Shared, submitted_under: usize) -> bool {
    shared.cancelled.load(Ordering::Relaxed)
        || shared.generation.load(Ordering::Relaxed) != submitted_under
}

impl Default for Warmer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doubles::Spy;
    use crate::queue::{Limit, Urgency};
    use mandible_core::{CommandNode, Provenance, Source};
    use mandible_extract::KeptProbes;
    use std::time::{Duration, Instant};

    fn p(s: &str) -> Vec<String> {
        s.split(' ').map(String::from).collect()
    }

    fn warmed(path: &[&str]) -> WarmedNode {
        WarmedNode {
            path: path.iter().map(|s| s.to_string()).collect(),
            result: FillResult {
                node: CommandNode::new(
                    path.last().copied().unwrap_or("root"),
                    Provenance::single(Source::HelpText),
                ),
                tier_statuses: Vec::new(),
                elapsed: Duration::ZERO,
                kept: KeptProbes::default(),
                timed_out: false,
            },
            forced: false,
        }
    }

    fn ask(warmer: &Warmer, spy: &Arc<Spy>, path: &str, urgency: Urgency, force: bool) -> Answer {
        let path = p(path);
        let req = Request {
            path: &path,
            urgency,
            force,
            limit: Limit::Standard,
        };
        warmer.request(&req, || {
            Some(Spec {
                runner: spy.runner(),
                tool: Spy::tool(),
                probe_path: path.clone(),
                existing: CommandNode::new(
                    path.last().cloned().unwrap_or_default(),
                    Provenance::single(Source::HelpText),
                ),
            })
        })
    }

    fn collect(warmer: &Warmer, want: usize) -> Vec<WarmedNode> {
        let mut got = Vec::new();
        let end = Instant::now() + Duration::from_secs(10);
        while got.len() < want && Instant::now() < end {
            got.extend(warmer.drain());
            std::thread::sleep(Duration::from_millis(5));
        }
        got
    }

    /// The pool must never exceed one thread per core (capped at 8): the
    /// old `cores * 4` oversubscription was measured pegging every core
    /// of a 4-core machine for minutes against a tool whose probes burn
    /// real CPU (docker).
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

    /// Results already sitting in the channel describe the tree that the
    /// re-extract just discarded, so they must not survive into the new
    /// one.
    #[test]
    fn reset_discards_undrained_results_from_the_old_generation() {
        let warmer = Warmer::new();
        warmer.shared.tx.send(warmed(&["git", "rebase"])).unwrap();
        warmer.shared.tx.send(warmed(&["git", "add"])).unwrap();

        warmer.reset();

        assert!(
            warmer.drain().is_empty(),
            "stale fills must not be spliced into the replacement tree"
        );
    }

    #[test]
    fn a_fill_that_finishes_after_a_reset_is_dropped() {
        let spy = Spy::new();
        let warmer = Warmer::new();
        spy.hold("a");
        ask(&warmer, &spy, "t a", Urgency::User, false);
        assert!(spy.wait_for(&p("t a"), 1));

        warmer.reset();
        spy.release();
        std::thread::sleep(Duration::from_millis(200));

        assert!(warmer.drain().is_empty(), "the old tree's fill was spliced");
        let again = ask(&warmer, &spy, "t a", Urgency::User, false);
        assert_eq!(
            again.decision,
            Decision::Queued,
            "the new tree has not read it"
        );
        assert_eq!(collect(&warmer, 1).len(), 1);
    }

    #[test]
    fn nothing_is_accepted_or_queued_once_the_session_is_quitting() {
        let spy = Spy::new();
        let warmer = Warmer::new();
        warmer.cancel();
        let answer = ask(&warmer, &spy, "t a", Urgency::User, false);
        assert_eq!(answer.decision, Decision::Refused);
        std::thread::sleep(Duration::from_millis(100));
        assert!(spy.calls().is_empty());
    }

    #[test]
    fn a_request_past_the_walk_budget_is_refused_but_the_user_is_not() {
        let spy = Spy::new();
        let warmer = Warmer::new();
        *lock(&warmer.shared.queue) = Queue::new(2, 0);
        assert_eq!(
            ask(&warmer, &spy, "t a", Urgency::Walk, false).decision,
            Decision::Refused
        );
        assert_eq!(
            ask(&warmer, &spy, "t a", Urgency::User, false).decision,
            Decision::Queued
        );
        assert_eq!(collect(&warmer, 1).len(), 1);
    }

    /// With every walk thread busy on a slow command, the row under the
    /// cursor is still read at once on the thread kept for it.
    #[test]
    fn the_reserved_thread_serves_a_user_request_while_the_walk_is_busy() {
        let spy = Spy::new();
        let warmer = Warmer::with_threads(3);
        spy.hold("a");
        spy.hold("b");
        for name in ["t a", "t b", "t c"] {
            ask(&warmer, &spy, name, Urgency::Walk, false);
        }
        assert!(spy.wait_for(&p("t a"), 1) && spy.wait_for(&p("t b"), 1));

        ask(&warmer, &spy, "t z", Urgency::User, false);

        assert!(
            spy.wait_for(&p("t z"), 1),
            "the user request waited behind the walk"
        );
        assert_eq!(spy.count(&p("t c")), 0, "the walk took the reserved thread");
        spy.release();
        assert_eq!(collect(&warmer, 4).len(), 4);
    }

    #[test]
    fn the_walk_never_takes_the_reserved_thread() {
        let spy = Spy::new();
        let warmer = Warmer::with_threads(2);
        spy.hold("a");
        ask(&warmer, &spy, "t a", Urgency::Walk, false);
        ask(&warmer, &spy, "t b", Urgency::Walk, false);
        assert!(spy.wait_for(&p("t a"), 1));
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(spy.count(&p("t b")), 0);
        spy.release();
        assert_eq!(collect(&warmer, 2).len(), 2);
    }

    #[test]
    fn a_forced_request_while_a_command_runs_reads_it_once_more() {
        let spy = Spy::new();
        let warmer = Warmer::with_threads(3);
        spy.hold("a");
        ask(&warmer, &spy, "t a", Urgency::Walk, false);
        assert!(spy.wait_for(&p("t a"), 1));
        for _ in 0..3 {
            let answer = ask(&warmer, &spy, "t a", Urgency::User, true);
            assert_eq!(answer.decision, Decision::Rerun);
        }
        spy.release();

        assert_eq!(collect(&warmer, 2).len(), 2);
        std::thread::sleep(Duration::from_millis(200));
        assert!(warmer.drain().is_empty());
        assert_eq!(spy.count(&p("t a")), 2, "once, not once per request");
    }

    #[test]
    fn a_probe_runs_under_the_cap_its_request_carried() {
        let spy = Spy::new();
        let warmer = Warmer::with_threads(3);
        let path = p("t a");
        let long = Request {
            path: &path,
            urgency: Urgency::User,
            force: true,
            limit: Limit::Long,
        };
        warmer.request(&long, || {
            Some(Spec {
                runner: spy.runner(),
                tool: Spy::tool(),
                probe_path: path.clone(),
                existing: CommandNode::new("a", Provenance::single(Source::HelpText)),
            })
        });
        ask(&warmer, &spy, "t b", Urgency::Walk, false);
        assert_eq!(collect(&warmer, 2).len(), 2);
        let calls = spy.calls();
        let limit_of = |path: &str| calls.iter().find(|(c, _)| *c == p(path)).unwrap().1;
        assert_eq!(limit_of("t a"), Duration::from_secs(120));
        assert_eq!(limit_of("t b"), Duration::from_secs(30));
    }
}
