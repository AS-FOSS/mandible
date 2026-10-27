//! The probe queue: the one place that decides what runs, in what order, and
//! whether a request is new.
//!
//! Every trigger asks [`Queue::request`]. A command already waiting is
//! promoted and never added twice, one running is left alone, a forced
//! request for it re-runs it once after it finishes, and one already done
//! runs again only when forced.
//!
//! Waiting work is taken forced user requests first, then the one unforced
//! user request (only the latest holds its place, so a cursor sweeping down
//! the list queues nothing for the rows it passes), then the walk. One thread
//! of the pool is kept for user requests.

use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Duration;

/// Who is asking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Urgency {
    /// The background walk of the tree.
    Walk,
    /// Something the user is looking at or asked for.
    User,
}

/// How long a probe may run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Limit {
    /// The standard cap.
    Standard,
    /// The longer cap `r` gives a command whose last read timed out.
    Long,
}

/// The cap `r` gives a command whose last read timed out.
pub const LONG_LIMIT: Duration = Duration::from_secs(120);

impl Limit {
    /// The cap on one probe.
    pub fn duration(self) -> Option<Duration> {
        match self {
            Limit::Standard => Some(mandible_extract::exec::EXTRACT_LIMIT),
            Limit::Long => Some(LONG_LIMIT),
        }
    }
}

/// A request to have one command probed.
pub struct Request<'a> {
    /// The command.
    pub path: &'a [String],
    /// Who is asking.
    pub urgency: Urgency,
    /// Run it again even if it is done.
    pub force: bool,
    /// How long the probe may run.
    pub limit: Limit,
}

/// What the queue did with a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Added to the queue.
    Queued,
    /// Already waiting; moved up or merged into the waiting entry.
    Promoted,
    /// Running; left alone.
    Running,
    /// Running; it runs once more after it finishes.
    Rerun,
    /// Already done; nothing owed.
    Done,
    /// The caller had nothing to probe.
    Declined,
    /// Not accepted: quitting, or the walk budget is spent.
    Refused,
}

/// The queue's answer: what it decided, and the waiting request it displaced.
#[derive(Debug, PartialEq, Eq)]
pub struct Answer {
    /// What happened to the request.
    pub decision: Decision,
    /// A waiting request that lost its place to this one and will not run.
    pub dropped: Option<Vec<String>>,
}

impl Answer {
    fn of(decision: Decision) -> Self {
        Self {
            decision,
            dropped: None,
        }
    }
}

/// Which share of the pool a taken entry runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Share {
    /// The walk's threads.
    Walk,
    /// Any free thread, the reserved one included.
    User,
}

/// A waiting request with its payload.
#[derive(Debug)]
pub struct Entry<J> {
    /// The command.
    pub path: Vec<String>,
    /// What the worker needs to run it.
    pub job: J,
    /// Run it even if done.
    pub force: bool,
    /// How long the probe may run.
    pub limit: Limit,
}

/// Waiting work in the order workers take it.
pub struct Queue<J> {
    walk: VecDeque<Entry<J>>,
    forced: VecDeque<Entry<J>>,
    look: Option<Entry<J>>,
    wanted: Option<Vec<String>>,
    running: HashMap<Vec<String>, Option<(Urgency, Entry<J>)>>,
    done: HashSet<Vec<String>>,
    threads: usize,
    active: usize,
    walk_active: usize,
    walk_budget: usize,
    walk_spent: usize,
}

impl<J> Queue<J> {
    /// A queue for a pool of `threads`, whose walk may queue `walk_budget`
    /// commands per tree.
    pub fn new(threads: usize, walk_budget: usize) -> Self {
        Self {
            walk: VecDeque::new(),
            forced: VecDeque::new(),
            look: None,
            wanted: None,
            running: HashMap::new(),
            done: HashSet::new(),
            threads,
            active: 0,
            walk_active: 0,
            walk_budget,
            walk_spent: 0,
        }
    }

    /// Ask for a command. `make` builds the payload and runs only when the
    /// request is new; returning `None` says there is nothing to probe.
    pub fn request(&mut self, req: &Request, make: impl FnOnce() -> Option<J>) -> Answer {
        let path = req.path.to_vec();
        if let Some(rerun) = self.running.get_mut(&path) {
            if !req.force {
                return Answer::of(Decision::Running);
            }
            let Some(job) = make() else {
                return Answer::of(Decision::Declined);
            };
            let entry = Entry {
                path,
                job,
                force: true,
                limit: req.limit,
            };
            *rerun = Some((req.urgency, entry));
            return Answer::of(Decision::Rerun);
        }
        if let Some(answer) = self.promote(req) {
            return answer;
        }
        if self.done.contains(&path) && !req.force {
            return Answer::of(Decision::Done);
        }
        if req.urgency == Urgency::Walk && self.walk_spent >= self.walk_budget {
            return Answer::of(Decision::Refused);
        }
        let Some(job) = make() else {
            return Answer::of(Decision::Declined);
        };
        if req.urgency == Urgency::Walk {
            self.walk_spent += 1;
        }
        let entry = Entry {
            path,
            job,
            force: req.force,
            limit: req.limit,
        };
        let mut answer = Answer::of(Decision::Queued);
        match (req.urgency, req.force) {
            (Urgency::Walk, _) => self.walk.push_back(entry),
            (Urgency::User, true) => self.forced.push_back(entry),
            (Urgency::User, false) => {
                answer.dropped = self.set_look(entry);
            }
        }
        answer
    }

    fn set_look(&mut self, entry: Entry<J>) -> Option<Vec<String>> {
        self.wanted = None;
        let old = self.look.replace(entry)?;
        Some(old.path)
    }

    fn promote(&mut self, req: &Request) -> Option<Answer> {
        let path = req.path;
        let mut answer = Answer::of(Decision::Promoted);
        if let Some(e) = self.forced.iter_mut().find(|e| e.path == path) {
            e.limit = e.limit.max(req.limit);
            return Some(answer);
        }
        if self.look.as_ref().is_some_and(|e| e.path == path) {
            if req.force {
                let mut e = self.look.take()?;
                e.force = true;
                e.limit = e.limit.max(req.limit);
                self.forced.push_front(e);
            } else if let Some(e) = self.look.as_mut() {
                e.limit = e.limit.max(req.limit);
            }
            return Some(answer);
        }
        let at = self.walk.iter().position(|e| e.path == path)?;
        let e = &mut self.walk[at];
        e.force |= req.force;
        e.limit = e.limit.max(req.limit);
        if req.urgency == Urgency::User {
            if req.force {
                let e = self.walk.remove(at)?;
                self.forced.push_front(e);
            } else {
                answer.dropped = self.drop_look();
                self.wanted = Some(path.to_vec());
            }
        }
        Some(answer)
    }

    fn drop_look(&mut self) -> Option<Vec<String>> {
        self.look.take().map(|e| e.path)
    }

    /// Take the next entry a free thread may run, and count it as running.
    pub fn take(&mut self) -> Option<(Entry<J>, Share)> {
        if self.active >= self.threads {
            return None;
        }
        let (entry, share) = self.next()?;
        self.active += 1;
        if share == Share::Walk {
            self.walk_active += 1;
        }
        self.running.insert(entry.path.clone(), None);
        Some((entry, share))
    }

    fn next(&mut self) -> Option<(Entry<J>, Share)> {
        if let Some(e) = self.forced.pop_front() {
            return Some((e, Share::User));
        }
        if let Some(e) = self.look.take() {
            return Some((e, Share::User));
        }
        if let Some(wanted) = self.wanted.take() {
            if let Some(at) = self.walk.iter().position(|e| e.path == wanted) {
                return Some((self.walk.remove(at)?, Share::User));
            }
        }
        if self.walk_active + 1 < self.threads {
            return Some((self.walk.pop_front()?, Share::Walk));
        }
        None
    }

    /// A worker is done with the entry it took.
    pub fn release(&mut self, share: Share) {
        self.active = self.active.saturating_sub(1);
        if share == Share::Walk {
            self.walk_active = self.walk_active.saturating_sub(1);
        }
    }

    /// The main loop collected the result for `path`. It counts as done, and
    /// a request that arrived while it ran starts waiting now.
    pub fn finish(&mut self, path: &[String]) {
        let Some(rerun) = self.running.remove(path) else {
            return;
        };
        self.done.insert(path.to_vec());
        if let Some((urgency, entry)) = rerun {
            match urgency {
                Urgency::Walk => self.walk.push_back(entry),
                Urgency::User => self.forced.push_back(entry),
            }
        }
    }

    #[cfg(test)]
    fn is_running(&self, path: &[String]) -> bool {
        self.running.contains_key(path)
    }

    /// Abandon everything for a new tree. Threads still busy with the old
    /// one stay counted until they [`Self::release`].
    pub fn reset(&mut self) {
        self.walk.clear();
        self.forced.clear();
        self.look = None;
        self.wanted = None;
        self.running.clear();
        self.done.clear();
        self.walk_spent = 0;
    }

    /// Drop whatever is waiting, for quitting.
    pub fn clear_waiting(&mut self) {
        self.walk.clear();
        self.forced.clear();
        self.look = None;
        self.wanted = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Vec<String> {
        s.split(' ').map(String::from).collect()
    }

    fn ask(q: &mut Queue<u32>, path: &str, urgency: Urgency, force: bool, job: u32) -> Answer {
        let path = p(path);
        let req = Request {
            path: &path,
            urgency,
            force,
            limit: Limit::Standard,
        };
        q.request(&req, || Some(job))
    }

    fn walked() -> Queue<u32> {
        let mut q = Queue::new(4, 100);
        for (name, job) in [("git a", 1), ("git b", 2), ("git c", 3)] {
            ask(&mut q, name, Urgency::Walk, false, job);
        }
        q
    }

    fn order(mut q: Queue<u32>) -> Vec<u32> {
        let mut out = Vec::new();
        while let Some((e, share)) = q.take() {
            out.push(e.job);
            q.release(share);
        }
        out
    }

    #[test]
    fn a_waiting_command_asked_for_again_is_promoted_never_added_twice() {
        for (urgency, force) in [
            (Urgency::Walk, false),
            (Urgency::Walk, true),
            (Urgency::User, false),
            (Urgency::User, true),
        ] {
            let mut q = walked();
            let answer = ask(&mut q, "git c", urgency, force, 9);
            assert_eq!(answer.decision, Decision::Promoted, "{urgency:?} {force}");
            let ran = order(q);
            assert_eq!(ran.iter().filter(|&&j| j == 3).count(), 1, "{ran:?}");
            assert!(!ran.contains(&9), "a second entry was added: {ran:?}");
            assert_eq!(ran.len(), 3);
        }
    }

    #[test]
    fn a_user_request_for_a_waiting_command_runs_it_first() {
        let mut q = walked();
        ask(&mut q, "git c", Urgency::User, false, 9);
        assert_eq!(order(q), vec![3, 1, 2]);
        let mut q = walked();
        ask(&mut q, "git c", Urgency::User, true, 9);
        assert_eq!(order(q), vec![3, 1, 2]);
    }

    #[test]
    fn a_command_the_walk_never_queued_takes_the_front() {
        let mut q = walked();
        let answer = ask(&mut q, "git z", Urgency::User, false, 9);
        assert_eq!(answer.decision, Decision::Queued);
        assert_eq!(order(q), vec![9, 1, 2, 3]);
    }

    #[test]
    fn only_the_latest_unforced_request_keeps_its_place() {
        let mut q = walked();
        ask(&mut q, "git c", Urgency::User, false, 9);
        ask(&mut q, "git b", Urgency::User, false, 9);
        assert_eq!(order(q), vec![2, 1, 3], "c went back to its own place");

        let mut q = walked();
        ask(&mut q, "git y", Urgency::User, false, 8);
        let answer = ask(&mut q, "git z", Urgency::User, false, 9);
        assert_eq!(answer.dropped, Some(p("git y")));
        assert_eq!(order(q), vec![9, 1, 2, 3], "y only held the place");
    }

    #[test]
    fn a_forced_request_is_never_displaced_by_a_cursor_resting_elsewhere() {
        let mut q = walked();
        ask(&mut q, "git y", Urgency::User, true, 8);
        let answer = ask(&mut q, "git z", Urgency::User, false, 9);
        assert_eq!(answer.dropped, None);
        assert_eq!(order(q), vec![8, 9, 1, 2, 3]);
    }

    #[test]
    fn a_running_command_is_left_alone() {
        let mut q = walked();
        let (taken, _) = q.take().unwrap();
        let answer = ask(&mut q, "git a", Urgency::User, false, 9);
        assert_eq!(answer.decision, Decision::Running);
        assert!(q.is_running(&taken.path));
        assert_eq!(order(q), vec![2, 3]);
    }

    #[test]
    fn a_forced_request_while_running_runs_it_once_more_after_it_finishes() {
        let mut q = walked();
        let (taken, share) = q.take().unwrap();
        for _ in 0..3 {
            let answer = ask(&mut q, "git a", Urgency::User, true, 9);
            assert_eq!(answer.decision, Decision::Rerun);
        }
        q.release(share);
        q.finish(&taken.path);
        let ran = order(q);
        assert_eq!(ran.iter().filter(|&&j| j == 9).count(), 1, "{ran:?}");
    }

    #[test]
    fn a_done_command_runs_again_only_when_forced() {
        let mut q = walked();
        let (taken, share) = q.take().unwrap();
        q.release(share);
        q.finish(&taken.path);
        assert_eq!(
            ask(&mut q, "git a", Urgency::User, false, 9).decision,
            Decision::Done
        );
        assert_eq!(
            ask(&mut q, "git a", Urgency::Walk, false, 9).decision,
            Decision::Done
        );
        assert_eq!(
            ask(&mut q, "git a", Urgency::Walk, true, 9).decision,
            Decision::Queued
        );
    }

    #[test]
    fn the_walk_budget_stops_the_walk_but_not_the_user() {
        let mut q: Queue<u32> = Queue::new(4, 1);
        assert_eq!(
            ask(&mut q, "git a", Urgency::Walk, false, 1).decision,
            Decision::Queued
        );
        assert_eq!(
            ask(&mut q, "git b", Urgency::Walk, false, 2).decision,
            Decision::Refused
        );
        assert_eq!(
            ask(&mut q, "git b", Urgency::User, false, 2).decision,
            Decision::Queued
        );
    }

    #[test]
    fn a_request_with_nothing_to_probe_is_declined() {
        let mut q: Queue<u32> = Queue::new(4, 10);
        let path = p("git a");
        let req = Request {
            path: &path,
            urgency: Urgency::User,
            force: false,
            limit: Limit::Standard,
        };
        assert_eq!(q.request(&req, || None).decision, Decision::Declined);
        assert!(q.take().is_none());
    }

    #[test]
    fn one_thread_is_kept_for_user_requests() {
        let mut q: Queue<u32> = Queue::new(3, 100);
        for (i, name) in ["git a", "git b", "git c", "git d"].iter().enumerate() {
            ask(&mut q, name, Urgency::Walk, false, i as u32);
        }
        let first = q.take().unwrap();
        let second = q.take().unwrap();
        assert_eq!((first.1, second.1), (Share::Walk, Share::Walk));
        assert!(q.take().is_none(), "the walk took the reserved thread");

        ask(&mut q, "git z", Urgency::User, false, 9);
        let (user, share) = q.take().unwrap();
        assert_eq!((user.job, share), (9, Share::User));
    }

    #[test]
    fn a_reset_forgets_waiting_running_and_done() {
        let mut q = walked();
        let (taken, share) = q.take().unwrap();
        q.release(share);
        q.finish(&taken.path);
        q.reset();
        assert!(q.take().is_none());
        assert_eq!(
            ask(&mut q, "git a", Urgency::Walk, false, 1).decision,
            Decision::Queued
        );
    }
}
