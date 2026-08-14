//! Descendant reaping: no probe is declared complete while anything it
//! started is still alive (spec §6 rule 4).
//!
//! **The gap this closes.** `spawn.rs` gives every probe its own session
//! and kills its *process group* on timeout. Both are necessary and
//! neither is sufficient, for one measured reason: **a program that
//! daemonises leaves the group and the session on its own**. `fork()`,
//! parent exits, child `setsid()`s — the textbook daemon — and from that
//! moment nothing about the child's process group, session, or parent
//! points back at the probe that started it.
//!
//! Measured on a developer box, 622 processes left behind by a single
//! sweep, the oldest five days old: `blkmapd` ×148, `rpc.idmapd` ×144,
//! `rpc.gssd` ×144, plus `sudo_logsrvd` (listening on `0.0.0.0:30343` and
//! `[::]:30343`), `guacd` (listening on `127.0.0.1:4822`), and
//! `pam-auth-update` (burning a full core for three days). The mechanism
//! is not a hang: **all 2,302 `probe-start` lines in a traced sweep had a
//! matching `probe-done`**. Every one of those probes *completed
//! normally* — these tools ignore an argument they don't recognise, start,
//! fork, detach, and are already gone by the time the direct child exits.
//! A tighter timeout cannot help with that, because there was never a
//! timeout to hit.
//!
//! **The mechanism used here, in two halves.**
//!
//! 1. **Adoption, so the descendants are findable at all.** This process
//!    marks itself a *child subreaper* (`prctl(PR_SET_CHILD_SUBREAPER)`,
//!    Linux ≥3.4). Orphaned descendants are then reparented to *us* rather
//!    than to init, no matter how many times they forked or `setsid`ed on
//!    the way — the kernel walks up to the nearest subreaper ancestor, and
//!    session and process group play no part in that walk. So the
//!    candidate set for a probe's escapees is just "our own children", a
//!    handful of pids, not all of `/proc`.
//! 2. **A per-invocation token, so only *this* probe's escapees are
//!    killed.** Adoption alone is too blunt: it cannot tell a daemon this
//!    probe leaked from a child some concurrent probe (or, in the test
//!    suite, some unrelated `Command::spawn`) legitimately owns. Every
//!    probe therefore carries a unique [`PROBE_TOKEN_VAR`] value in the
//!    environment `run_inert` already builds from scratch, and descendants
//!    inherit it across `fork` and `exec`. A process is killed only when it
//!    is *both* adopted by us *and* carrying this invocation's exact token
//!    — so a concurrently running probe's child (different token) and
//!    anything not started by a probe at all (no token) are untouchable by
//!    construction, not by timing.
//!
//! **Kill by pid, never by process group.** The escapee's own group id is
//! frequently the direct child's pid, which `run_inert` has already waited
//! on — a pid the kernel is free to have recycled. Signalling that group
//! would be signalling whatever now owns the number. Killing the pid we
//! just read out of `/proc`, and letting the round loop below pick up
//! whatever that unparents, keeps every signal aimed at a process this
//! invocation has positively identified.
//!
//! **Bounded, always.** Rounds and a wall-clock budget both cap the work,
//! so a process wedged in uninterruptible sleep delays a probe by
//! milliseconds and is then left alone, rather than turning "reap before
//! returning" into a new way for extraction to hang.
//!
//! **Linux only.** `/proc` and `PR_SET_CHILD_SUBREAPER` have no portable
//! equivalent, and the measurement above is a Linux one. Elsewhere this
//! module is a no-op and the leak stands as the documented residual risk
//! it already was (spec §6 rule 8's closing paragraphs).

use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// The environment variable each probe's unique token is passed in.
///
/// Read back out of `/proc/<pid>/environ` to attribute a surviving
/// process to the exact invocation that started it. Named as mandible's
/// own so a reader who finds it on a stray process knows where it came
/// from.
pub(super) const PROBE_TOKEN_VAR: &str = "MANDIBLE_PROBE_ID";

/// How many kill-then-rescan rounds one reap performs. Each round can
/// only reveal descendants one level deeper (killing an adopted process
/// reparents *its* children to us in turn), and real probe descendants
/// nest one or two levels; the cap is what keeps a fork bomb from turning
/// this into an unbounded loop.
const MAX_ROUNDS: usize = 8;

/// Total wall-clock budget for one reap, across every round. A process in
/// uninterruptible sleep cannot be killed at all, and waiting on it
/// forever would make this a worse hang than the leak it fixes.
const REAP_BUDGET: Duration = Duration::from_millis(500);

/// How long to wait for a single SIGKILLed process to actually go away
/// before moving on to the next one.
const PER_PROCESS_WAIT: Duration = Duration::from_millis(100);

/// Polling interval while waiting for a killed process to be reaped.
const POLL_INTERVAL: Duration = Duration::from_millis(1);

/// One probe invocation's identity, as seen by its own descendants.
///
/// Constructed per `run_inert` call and never reused: the value combines
/// this process's pid, a monotonically increasing counter, and a
/// wall-clock stamp, so it cannot collide with a concurrent probe in this
/// process, nor with a probe from an earlier mandible run whose pid the
/// kernel has since recycled.
#[derive(Debug, Clone)]
pub(super) struct ProbeToken(String);

impl ProbeToken {
    /// Mint a token for one invocation.
    pub(super) fn new() -> ProbeToken {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        ProbeToken(format!("{}-{stamp}-{seq}", std::process::id()))
    }

    /// Put the token into the child's environment, where every descendant
    /// inherits it.
    pub(super) fn apply(&self, cmd: &mut Command) {
        cmd.env(PROBE_TOKEN_VAR, &self.0);
    }

    /// The token text, as it appears in a descendant's `environ`.
    pub(super) fn value(&self) -> &str {
        &self.0
    }
}

/// Make this process the reaper for orphaned descendants, once.
///
/// Idempotent and cheap after the first call. Failure is deliberately
/// silent and non-fatal: on a kernel without `PR_SET_CHILD_SUBREAPER` the
/// escapees are reparented to init as before, [`reap_probe_descendants`]
/// then finds nothing, and the probe behaves exactly as it did before this
/// module existed. Refusing to probe at all would be a far worse trade
/// than the documented residual risk.
#[cfg(target_os = "linux")]
pub(super) fn arm_subreaper() {
    use std::sync::OnceLock;
    static ARMED: OnceLock<bool> = OnceLock::new();
    ARMED.get_or_init(|| nix::sys::prctl::set_child_subreaper(true).is_ok());
}

#[cfg(not(target_os = "linux"))]
pub(super) fn arm_subreaper() {}

/// Kill and reap every process this invocation started that is still
/// alive, returning how many were dealt with.
///
/// Call *after* the direct child has been waited on and *before* joining
/// the output readers: a descendant holding the inherited stdout/stderr
/// pipe open keeps those readers blocked at EOF, so reaping first is what
/// makes the read finish promptly as well as what stops the leak.
#[cfg(target_os = "linux")]
pub(super) fn reap_probe_descendants(token: &str) -> usize {
    use nix::sys::signal::{kill, Signal};
    use nix::unistd::Pid;
    use std::time::Instant;

    let me = std::process::id() as i32;
    let deadline = Instant::now() + REAP_BUDGET;
    let mut reaped = 0usize;

    for _ in 0..MAX_ROUNDS {
        let escapees = adopted_children_with_token(me, token);
        if escapees.is_empty() {
            break;
        }
        for pid in escapees {
            // By pid, never by process group — see this module's doc
            // comment. Anything this unparents is picked up next round.
            let _ = kill(Pid::from_raw(pid), Signal::SIGKILL);
            wait_briefly_for(pid, deadline);
            reaped += 1;
        }
        if Instant::now() >= deadline {
            break;
        }
    }
    reaped
}

#[cfg(not(target_os = "linux"))]
pub(super) fn reap_probe_descendants(_token: &str) -> usize {
    0
}

/// Every process that is both a child of `me` (by adoption, since a
/// probe's direct child has already been waited on by the time this runs)
/// and carrying `token` in its environment.
#[cfg(target_os = "linux")]
fn adopted_children_with_token(me: i32, token: &str) -> Vec<i32> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return out;
    };
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<i32>().ok())
        else {
            continue;
        };
        if pid == me {
            continue;
        }
        if parent_of(pid) != Some(me) {
            continue;
        }
        if has_token(pid, token) {
            out.push(pid);
        }
    }
    out
}

/// The parent pid recorded in `/proc/<pid>/stat`.
///
/// Parsed from after the **last** `)`, never by splitting the whole line
/// on whitespace: field 2 is the executable's `comm`, wrapped in
/// parentheses and free to contain both spaces and parentheses of its own
/// (`(sh -c foo)`, `(some (thing))`), which is exactly what makes naive
/// field indexing wrong on precisely the processes worth catching.
#[cfg(target_os = "linux")]
fn parent_of(pid: i32) -> Option<i32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_comm = &stat[stat.rfind(')')? + 1..];
    // Fields after `comm`: state, ppid, ...
    after_comm.split_whitespace().nth(1)?.parse().ok()
}

/// True if `pid`'s environment contains exactly `MANDIBLE_PROBE_ID=<token>`.
///
/// `environ` is NUL-separated, so this compares whole entries rather than
/// searching for a substring — a token is a prefix of no other token, but
/// entry-wise comparison makes that a property of the code rather than of
/// the token format.
#[cfg(target_os = "linux")]
fn has_token(pid: i32, token: &str) -> bool {
    let Ok(environ) = std::fs::read(format!("/proc/{pid}/environ")) else {
        // Unreadable is the normal case for a process that exited between
        // the directory listing and this read, and for anything not ours.
        return false;
    };
    let wanted = format!("{PROBE_TOKEN_VAR}={token}");
    environ
        .split(|b| *b == 0)
        .any(|entry| entry == wanted.as_bytes())
}

/// Wait for a just-killed adopted child to actually be gone, bounded by
/// both its own budget and the whole reap's deadline.
///
/// `waitpid` is always aimed at the one pid, **never at `-1`**: a
/// wildcard wait in this process would steal the exit status of some
/// other probe's `std::process::Child`, which owns its own `waitpid` and
/// would then fail with `ECHILD` on a child that had in fact exited
/// normally.
#[cfg(target_os = "linux")]
fn wait_briefly_for(pid: i32, deadline: std::time::Instant) {
    use nix::sys::wait::{waitpid, WaitPidFlag, WaitStatus};
    use nix::unistd::Pid;
    use std::time::Instant;

    let own_deadline = (Instant::now() + PER_PROCESS_WAIT).min(deadline);
    loop {
        match waitpid(Pid::from_raw(pid), Some(WaitPidFlag::WNOHANG)) {
            // Reaped, or it was never (or no longer) ours to reap.
            Ok(WaitStatus::StillAlive) => {}
            _ => return,
        }
        if Instant::now() >= own_deadline {
            return;
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_unique_per_invocation() {
        let a = ProbeToken::new();
        let b = ProbeToken::new();
        assert_ne!(a.value(), b.value());
        assert!(a.value().starts_with(&format!("{}-", std::process::id())));
    }

    /// `comm` can contain spaces and parentheses, which is what makes
    /// whitespace-splitting the whole `stat` line wrong. This process's
    /// own entry is a real parse against a real file.
    #[test]
    fn parent_of_reads_this_processes_real_parent() {
        let me = std::process::id() as i32;
        let parent = parent_of(me).expect("/proc/self/stat must parse");
        assert!(parent > 0, "ppid should be a real pid, got {parent}");
    }

    #[test]
    fn parent_of_is_none_for_a_pid_that_does_not_exist() {
        // A pid well past any plausible live one.
        assert_eq!(parent_of(i32::MAX), None);
    }

    /// The token check is what keeps this from being a blunt "kill
    /// anything adopted": a process not carrying *this* token is never a
    /// candidate, whoever its parent is.
    #[test]
    fn has_token_is_false_for_a_process_without_it() {
        let me = std::process::id() as i32;
        assert!(!has_token(me, "no-such-token"));
    }

    #[test]
    fn reaping_with_no_descendants_finds_nothing() {
        arm_subreaper();
        assert_eq!(reap_probe_descendants("a-token-nothing-carries"), 0);
    }
}
