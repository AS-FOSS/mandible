//! The wall-clock limit for the probes of one read (spec §6 rule 4).
//!
//! A scope ([`with_extract_limit`]) sets the limit for the reads run inside
//! it. The limit is the total for one read of one command, all help forms
//! together: [`within_read`] starts the clock, [`extract_limit`] returns
//! what is left, and [`run_probe`] starts no probe once nothing is left.

use super::policy::InertArgv;
use super::probe::Probe;
use super::spawn::{ExecError, ExecOutput};
use std::cell::Cell;
use std::path::Path;
use std::time::{Duration, Instant};

/// The cap on an `extract_node` probe when no scope says otherwise.
pub const EXTRACT_LIMIT: Duration = Duration::from_secs(30);

/// No limit: too long to reach, short enough not to overflow an `Instant`.
const UNLIMITED: Duration = Duration::from_secs(60 * 60 * 24 * 365);

/// One read in progress on this thread.
#[derive(Clone, Copy)]
struct Read {
    /// When the scope's limit runs out, `None` for no limit.
    deadline: Option<Instant>,
    /// A probe was not started, or was cut short, for lack of time.
    cut: bool,
}

thread_local! {
    static SCOPED: Cell<Option<Option<Duration>>> = const { Cell::new(None) };
    static READ: Cell<Option<Read>> = const { Cell::new(None) };
}

/// The time a probe on this thread may take now: what is left of the
/// read's limit inside [`within_read`], else the scope's limit, else
/// [`EXTRACT_LIMIT`].
pub fn extract_limit() -> Duration {
    if let Some(deadline) = READ.with(Cell::get).and_then(|r| r.deadline) {
        return deadline.saturating_duration_since(Instant::now());
    }
    read_limit()
}

/// The limit this thread's scope gives each read, whole.
pub fn read_limit() -> Duration {
    match SCOPED.with(Cell::get) {
        Some(Some(limit)) => limit,
        Some(None) => UNLIMITED,
        None => EXTRACT_LIMIT,
    }
}

/// Run `f` with `limit` as this thread's limit for each read inside it,
/// `None` for no limit. The previous limit is restored afterwards, also
/// when `f` panics.
pub fn with_extract_limit<T>(limit: Option<Duration>, f: impl FnOnce() -> T) -> T {
    struct Restore(Option<Option<Duration>>);
    impl Drop for Restore {
        fn drop(&mut self) {
            SCOPED.with(|s| s.set(self.0));
        }
    }
    let _restore = Restore(SCOPED.with(|s| s.replace(Some(limit))));
    f()
}

/// Run one read of one command. Inside a scope with a limit, every probe
/// `f` runs shares that limit; outside a scope each probe has its own
/// [`EXTRACT_LIMIT`]. Returns what `f` returned and whether a probe was
/// not started, or was cut short, for lack of time. A read inside a read
/// is part of the outer one.
pub fn within_read<T>(f: impl FnOnce() -> T) -> (T, bool) {
    struct Restore(Option<Read>);
    impl Drop for Restore {
        fn drop(&mut self) {
            READ.with(|r| r.set(self.0));
        }
    }
    let outer = READ.with(Cell::get);
    if outer.is_some() {
        return (f(), false);
    }
    let deadline = match SCOPED.with(Cell::get) {
        Some(Some(limit)) => Some(Instant::now() + limit),
        _ => None,
    };
    let _restore = Restore(outer);
    READ.with(|r| {
        r.set(Some(Read {
            deadline,
            cut: false,
        }))
    });
    let value = f();
    let cut = READ.with(Cell::get).is_some_and(|r| r.cut);
    (value, cut)
}

/// Whether the read in progress has used up its limit.
pub fn read_spent() -> bool {
    READ.with(Cell::get)
        .and_then(|r| r.deadline)
        .is_some_and(|d| Instant::now() >= d)
}

fn note_cut() {
    READ.with(|r| {
        if let Some(read) = r.get() {
            r.set(Some(Read { cut: true, ..read }));
        }
    });
}

/// Run one probe under [`extract_limit`]. With nothing left of the read's
/// limit nothing is started, and the answer is an empty stopped probe.
pub fn run_probe(
    probe: &dyn Probe,
    tool_path: &Path,
    argv: &InertArgv,
) -> Result<ExecOutput, ExecError> {
    run_probe_capped(probe, tool_path, argv, UNLIMITED)
}

/// [`run_probe`] with a cap of its own, for a probe that is shorter than
/// the read's limit by design.
pub fn run_probe_capped(
    probe: &dyn Probe,
    tool_path: &Path,
    argv: &InertArgv,
    cap: Duration,
) -> Result<ExecOutput, ExecError> {
    let left = extract_limit();
    if left.is_zero() {
        note_cut();
        return Ok(ExecOutput {
            stdout: Vec::new(),
            stderr: Vec::new(),
            exit_code: None,
            timed_out: true,
        });
    }
    let allowed = left.min(cap);
    let out = probe.run(tool_path, argv, allowed)?;
    if out.timed_out && allowed == left {
        note_cut();
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_cap_is_thirty_seconds() {
        assert_eq!(extract_limit(), Duration::from_secs(30));
    }

    #[test]
    fn a_scope_changes_the_cap_and_restores_it() {
        let short = Duration::from_secs(10);
        with_extract_limit(Some(short), || {
            assert_eq!(extract_limit(), short);
            with_extract_limit(None, || {
                assert!(extract_limit() > Duration::from_secs(86_400))
            });
            assert_eq!(extract_limit(), short);
        });
        assert_eq!(extract_limit(), EXTRACT_LIMIT);
    }

    struct Logged(std::sync::Mutex<Vec<Duration>>);

    impl Probe for Logged {
        fn run(&self, _: &Path, _: &InertArgv, timeout: Duration) -> Result<ExecOutput, ExecError> {
            self.0.lock().unwrap().push(timeout);
            Ok(ExecOutput {
                stdout: Vec::new(),
                stderr: Vec::new(),
                exit_code: Some(0),
                timed_out: false,
            })
        }
    }

    #[test]
    fn a_spent_read_starts_no_probe() {
        let probe = Logged(Default::default());
        let (out, cut) = with_extract_limit(Some(Duration::ZERO), || {
            within_read(|| run_probe(&probe, Path::new("/t"), &InertArgv::HelpLong).unwrap())
        });
        assert!(out.timed_out && cut);
        assert!(probe.0.lock().unwrap().is_empty());
    }

    #[test]
    fn a_probe_gets_what_is_left_and_a_cap_only_lowers_it() {
        let probe = Logged(Default::default());
        let argv = InertArgv::HelpLong;
        with_extract_limit(Some(Duration::from_secs(20)), || {
            within_read(|| {
                run_probe(&probe, Path::new("/t"), &argv).unwrap();
                run_probe_capped(&probe, Path::new("/t"), &argv, Duration::from_secs(5)).unwrap();
            })
        });
        let seen = probe.0.lock().unwrap().clone();
        assert!(seen[0] <= Duration::from_secs(20) && seen[0] > Duration::from_secs(19));
        assert_eq!(seen[1], Duration::from_secs(5));
    }

    #[test]
    fn outside_a_scope_each_probe_has_the_whole_default() {
        let probe = Logged(Default::default());
        within_read(|| {
            run_probe(&probe, Path::new("/t"), &InertArgv::HelpLong).unwrap();
            run_probe(&probe, Path::new("/t"), &InertArgv::HelpLong).unwrap();
        });
        assert_eq!(*probe.0.lock().unwrap(), [EXTRACT_LIMIT; 2]);
    }

    #[test]
    fn a_scope_does_not_reach_another_thread() {
        with_extract_limit(None, || {
            let other = std::thread::spawn(extract_limit).join().unwrap();
            assert_eq!(other, EXTRACT_LIMIT);
        });
    }
}
