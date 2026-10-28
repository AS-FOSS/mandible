//! One record per command: where its probe stands and what came of it.
//!
//! The tree rows, the page notices and the verbatim view all read this
//! record, and every reset of the tree goes through [`Probes::reset`]. The
//! output a probe kept for the verbatim view lives in the record and is
//! held to a byte budget.

use mandible_extract::KeptProbes;
use std::collections::HashMap;

/// Total kept output for one session. Past it, the records least recently
/// filled or viewed drop their output.
pub const KEPT_BUDGET_BYTES: usize = 64 * 1024 * 1024;

/// How a finished probe came out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The help was read into a page.
    Parsed,
    /// The command printed no help that could be read.
    Empty,
    /// Every tier that tried it failed.
    Failed,
    /// The help probe was stopped at the time limit.
    TimedOut,
}

/// What a fill reported, before the page it produced is looked at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FillEnd {
    /// A tier read the help.
    Read,
    /// Every tier that tried failed.
    Failed,
    /// The help probe hit the time limit.
    TimedOut,
}

/// A finished probe: its outcome and the output it printed.
#[derive(Debug)]
pub struct Done {
    outcome: Outcome,
    kept: Option<KeptProbes>,
    bytes: usize,
    used: u64,
}

/// Whether a waiting probe has started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// In the queue.
    Queued,
    /// On a worker.
    Running,
}

/// A command's probe. A command never probed has no record.
#[derive(Debug)]
pub enum Probe {
    /// Queued or running; `before` is the result being replaced, which the
    /// page keeps using until the new one lands.
    Waiting {
        /// Queued or running.
        phase: Phase,
        /// The result a re-run replaces.
        before: Option<Done>,
    },
    /// Finished.
    Done(Done),
}

impl Probe {
    fn done(&self) -> Option<&Done> {
        match self {
            Probe::Done(d) => Some(d),
            Probe::Waiting { before, .. } => before.as_ref(),
        }
    }

    fn done_mut(&mut self) -> Option<&mut Done> {
        match self {
            Probe::Done(d) => Some(d),
            Probe::Waiting { before, .. } => before.as_mut(),
        }
    }
}

/// Every command's probe record, with the kept output under a byte budget.
#[derive(Debug)]
pub struct Probes {
    map: HashMap<Vec<String>, Probe>,
    total: usize,
    clock: u64,
    budget: usize,
}

impl Probes {
    /// An empty record holding at most `budget` bytes of kept output.
    pub fn new(budget: usize) -> Self {
        Self {
            map: HashMap::new(),
            total: 0,
            clock: 0,
            budget,
        }
    }

    /// Forget every record, as a reload of the tree does.
    pub fn reset(&mut self) {
        self.map.clear();
        self.total = 0;
    }

    /// `path` is waiting in the queue. A result it already has is kept
    /// until the new one lands.
    pub fn queued(&mut self, path: Vec<String>) {
        match self.map.remove(&path) {
            Some(Probe::Done(d)) => self.map.insert(
                path,
                Probe::Waiting {
                    phase: Phase::Queued,
                    before: Some(d),
                },
            ),
            Some(waiting @ Probe::Waiting { .. }) => self.map.insert(path, waiting),
            None => self.map.insert(
                path,
                Probe::Waiting {
                    phase: Phase::Queued,
                    before: None,
                },
            ),
        };
    }

    /// `path` was dropped from the queue before it ran.
    pub fn unqueued(&mut self, path: &[String]) {
        if !matches!(
            self.map.get(path),
            Some(Probe::Waiting {
                phase: Phase::Queued,
                ..
            })
        ) {
            return;
        }
        if let Some(Probe::Waiting {
            before: Some(d), ..
        }) = self.map.remove(path)
        {
            self.map.insert(path.to_vec(), Probe::Done(d));
        }
    }

    /// A worker started on `path`.
    pub fn running(&mut self, path: &[String]) {
        if let Some(Probe::Waiting { phase, .. }) = self.map.get_mut(path) {
            *phase = Phase::Running;
        }
    }

    /// `path` finished with `outcome` and printed `kept`. Returns the paths
    /// whose kept output was dropped to stay within the budget.
    pub fn finish(
        &mut self,
        path: Vec<String>,
        outcome: Outcome,
        kept: KeptProbes,
    ) -> Vec<Vec<String>> {
        self.clock += 1;
        let bytes = kept.byte_size();
        let done = Done {
            outcome,
            kept: Some(kept),
            bytes,
            used: self.clock,
        };
        if let Some(old) = self.map.insert(path.clone(), Probe::Done(done)) {
            self.total -= old.done().map_or(0, |d| d.bytes);
        }
        self.total += bytes;
        let mut dropped = Vec::new();
        while self.total > self.budget {
            let oldest = self
                .map
                .iter()
                .filter(|(p, probe)| **p != path && probe.done().is_some_and(|d| d.kept.is_some()))
                .min_by_key(|(_, probe)| probe.done().map_or(0, |d| d.used))
                .map(|(p, _)| p.clone());
            let Some(oldest) = oldest else { break };
            if let Some(d) = self.map.get_mut(&oldest).and_then(Probe::done_mut) {
                self.total -= d.bytes;
                d.bytes = 0;
                d.kept = None;
            }
            dropped.push(oldest);
        }
        dropped
    }

    /// The record for `path`, if it was ever asked for.
    pub fn get(&self, path: &[String]) -> Option<&Probe> {
        self.map.get(path)
    }

    /// Queued or running.
    pub fn in_flight(&self, path: &[String]) -> bool {
        matches!(self.map.get(path), Some(Probe::Waiting { .. }))
    }

    /// Queued or running with no earlier result to show.
    pub fn first_read(&self, path: &[String]) -> bool {
        matches!(
            self.map.get(path),
            Some(Probe::Waiting { before: None, .. })
        )
    }

    /// On a worker now.
    pub fn is_running(&self, path: &[String]) -> bool {
        matches!(
            self.map.get(path),
            Some(Probe::Waiting {
                phase: Phase::Running,
                ..
            })
        )
    }

    /// A probe for `path` has finished and none is in flight.
    pub fn finished(&self, path: &[String]) -> bool {
        matches!(self.map.get(path), Some(Probe::Done(_)))
    }

    /// How the last finished probe of `path` came out, also while a re-run
    /// is in flight.
    pub fn outcome(&self, path: &[String]) -> Option<Outcome> {
        self.map.get(path)?.done().map(|d| d.outcome)
    }

    /// Mark `path`'s kept output as just used. False when none is kept.
    pub fn touch(&mut self, path: &[String]) -> bool {
        self.clock += 1;
        let clock = self.clock;
        match self.map.get_mut(path).and_then(Probe::done_mut) {
            Some(d) if d.kept.is_some() => {
                d.used = clock;
                true
            }
            _ => false,
        }
    }

    /// The output kept for `path`, then for each of its ancestors,
    /// nearest first.
    pub fn kept_chain(&self, path: &[String]) -> Vec<&KeptProbes> {
        (1..=path.len())
            .rev()
            .filter_map(|len| self.map.get(&path[..len])?.done()?.kept.as_ref())
            .collect()
    }

    /// Bytes of kept output held.
    pub fn kept_bytes(&self) -> usize {
        self.total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(name: &str) -> Vec<String> {
        vec!["t".to_string(), name.to_string()]
    }

    fn none() -> KeptProbes {
        KeptProbes::default()
    }

    #[test]
    fn a_command_goes_from_not_probed_through_queued_and_running_to_done() {
        let mut probes = Probes::new(KEPT_BUDGET_BYTES);
        assert!(probes.get(&p("a")).is_none());
        probes.queued(p("a"));
        assert!(probes.in_flight(&p("a")) && !probes.is_running(&p("a")));
        probes.running(&p("a"));
        assert!(probes.is_running(&p("a")));
        probes.finish(p("a"), Outcome::TimedOut, none());
        assert!(probes.finished(&p("a")) && !probes.in_flight(&p("a")));
        assert_eq!(probes.outcome(&p("a")), Some(Outcome::TimedOut));
    }

    #[test]
    fn a_re_run_keeps_the_result_it_replaces_until_the_new_one_lands() {
        let mut probes = Probes::new(KEPT_BUDGET_BYTES);
        probes.finish(p("a"), Outcome::TimedOut, none());
        probes.queued(p("a"));
        assert!(probes.in_flight(&p("a")));
        assert_eq!(probes.outcome(&p("a")), Some(Outcome::TimedOut));
        probes.finish(p("a"), Outcome::Parsed, none());
        assert_eq!(probes.outcome(&p("a")), Some(Outcome::Parsed));
    }

    #[test]
    fn dropping_a_queued_command_restores_what_it_had() {
        let mut probes = Probes::new(KEPT_BUDGET_BYTES);
        probes.queued(p("a"));
        probes.unqueued(&p("a"));
        assert!(probes.get(&p("a")).is_none());

        probes.finish(p("b"), Outcome::Parsed, none());
        probes.queued(p("b"));
        probes.unqueued(&p("b"));
        assert!(probes.finished(&p("b")));

        probes.queued(p("c"));
        probes.running(&p("c"));
        probes.unqueued(&p("c"));
        assert!(probes.is_running(&p("c")), "a running probe is not dropped");
    }

    #[test]
    fn a_reset_forgets_every_record() {
        let mut probes = Probes::new(KEPT_BUDGET_BYTES);
        probes.queued(p("a"));
        probes.finish(p("b"), Outcome::Parsed, none());
        probes.reset();
        assert!(probes.get(&p("a")).is_none() && probes.get(&p("b")).is_none());
        assert_eq!(probes.kept_bytes(), 0);
    }
}
