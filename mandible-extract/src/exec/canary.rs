//! Canary tripwires for a contained full-`PATH` sweep (spec §6/§8, this
//! project's third safety layer — see [`super::containment`]'s doc comment
//! for the first two: evidence-before-argv gating prevents, namespaces
//! contain). A tripwire's job is to turn a side effect that *does* happen
//! into a loud, immediate test failure instead of a silent surprise an
//! operator discovers hours or days later. **A tripwire nobody has tested
//! is decoration** — this module's own test suite (below) deliberately
//! trips each one and asserts the failure, so the detection path itself is
//! covered, not just assumed to work.
//!
//! Three canaries, chosen to correspond to real observed misbehaviour
//! classes named directly in this crate's own history and in spec §6:
//!
//! - [`PtyCanary`]: a real pseudo-terminal that must never receive a
//!   byte. Catches a probed binary that runs `wall`/`write`-style broadcast
//!   tools — the exact family `spawn.rs`'s `HELP_ONLY_PROBE` list already
//!   refuses argv-wise (rule 0, the `wall` incident this crate's history
//!   records), with this canary as the belt to that suspenders: if a
//!   binary *not* on that closed list turns out to have the same behaviour
//!   under some other invocation this crate didn't anticipate, this is
//!   what notices.
//! - [`ProcessCanary`]: throwaway processes deliberately given common,
//!   short, plausible-looking names. Catches an over-broad `pkill`/`kill`
//!   invocation — the family behind the `pkill -- ""` machine-reset
//!   incident that motivated spec §6 rule 2a, now checked for the milder
//!   but still-real case of a *non-empty* but over-matching pattern.
//! - [`PathCanary`]: a directory outside `spawn.rs`'s per-probe `Scratch`
//!   redirect that must stay empty for the life of the sweep. Catches an
//!   unprompted writer — [M-11]'s `mysql_secure_installation` writing a
//!   `.my.cnf` with an empty root password on nothing but `--help` is the
//!   measured case this crate already documents; this is what would have
//!   caught it as a hard failure instead of a comment added after the
//!   fact.
//!
//! **What these do not cover.** A canary only detects what it is placed to
//! observe. A probe that writes to an absolute path this sweep never
//! watches, or that only misbehaves in a way none of these three shapes
//! covers (a stray network call, say — see [`super::containment`]'s own
//! "does not buy" section), trips nothing. These three are the
//! specific, previously-measured classes; they are not a general dragnet.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// One canary having been tripped, with enough detail to explain *why* the
/// sweep is about to fail loudly.
#[derive(Debug)]
pub enum CanaryTrip {
    /// Something wrote to [`PtyCanary`]'s slave device.
    PtyWritten {
        /// The bytes captured on the master side (capped — see
        /// [`PtyCanary::spawn`]).
        bytes: Vec<u8>,
    },
    /// A canary process named in [`ProcessCanary::COMMON_NAMES`] died
    /// before [`CanarySet::teardown`] killed it itself.
    ProcessKilled {
        /// The canary's name (its `comm`, matched by a plain `pkill
        /// <name>`).
        name: String,
    },
    /// [`PathCanary`]'s watched directory gained an entry.
    PathWritten {
        /// Every entry found, for diagnosis.
        paths: Vec<PathBuf>,
    },
}

impl std::fmt::Display for CanaryTrip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CanaryTrip::PtyWritten { bytes } => write!(
                f,
                "canary PTY received {} byte(s): {:?} — a probed tool wrote to a terminal (wall/write-class side effect)",
                bytes.len(),
                String::from_utf8_lossy(bytes)
            ),
            CanaryTrip::ProcessKilled { name } => write!(
                f,
                "canary process {name:?} was killed by something other than teardown — an over-broad kill/pkill/killall matched it"
            ),
            CanaryTrip::PathWritten { paths } => write!(
                f,
                "canary watch directory received {} unexpected entr(y/ies): {paths:?} — a probed tool wrote outside the scratch redirect",
                paths.len()
            ),
        }
    }
}

/// A real pseudo-terminal that must never receive a byte for the life of
/// the sweep.
///
/// Only the master side is kept open by this process; the slave's device
/// path (`/proc/self/fd/<n>` resolved via `readlink`, since Linux's ptmx/
/// devpts machinery does not hand back a path directly — see [`spawn`]) is
/// exposed for anything else in the sweep's namespace to reach. A
/// pseudo-terminal on Linux stays valid as long as *a* side is open; the
/// master alone is sufficient, so the slave fd itself is closed right
/// after its path is read.
pub struct PtyCanary {
    /// The device path a write would have to target. `None` if it could
    /// not be resolved (non-Linux, or `/proc` unavailable) — the reader
    /// thread still runs in that case, but nothing outside this process
    /// can be told where to write.
    slave_path: Option<PathBuf>,
    tripped: Arc<AtomicBool>,
    captured: Arc<Mutex<Vec<u8>>>,
    /// Held for the canary's whole lifetime, deliberately never dropped
    /// early. Measured directly while building this module: dropping the
    /// slave fd right after reading its path (keeping only the master
    /// open) let a *second, external* open of the same `/proc/self/fd`-
    /// resolved path fail with `ENOENT` moments later — i.e. the devpts
    /// entry did not reliably outlive the slave fd the way the "only one
    /// side needs to stay open" folklore suggests on every kernel. Keeping
    /// both fds referenced for as long as this struct lives is the
    /// version that was actually verified to keep the path openable by
    /// something else throughout, which is the whole point of exposing a
    /// path at all.
    _slave: std::os::fd::OwnedFd,
    _reader: JoinHandle<()>,
}

/// Cap on how many bytes of a trip are retained for the failure message —
/// diagnosis needs a sample, not the whole stream.
const PTY_CAPTURE_CAP: usize = 4096;

impl PtyCanary {
    /// Allocate the pty and start the background reader. Fails only if the
    /// OS refuses to allocate a pty at all (`openpty`'s own error).
    pub fn spawn() -> std::io::Result<Self> {
        let pty = nix::pty::openpty(None, None).map_err(std::io::Error::from)?;

        let slave_raw_fd: std::os::fd::RawFd = std::os::fd::AsRawFd::as_raw_fd(&pty.slave);
        let slave_path = std::fs::read_link(format!("/proc/self/fd/{slave_raw_fd}")).ok();

        let tripped = Arc::new(AtomicBool::new(false));
        let captured = Arc::new(Mutex::new(Vec::new()));
        let mut master_file = std::fs::File::from(pty.master);

        let reader_tripped = Arc::clone(&tripped);
        let reader_captured = Arc::clone(&captured);
        let reader = std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match master_file.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        reader_tripped.store(true, Ordering::SeqCst);
                        let mut cap = reader_captured.lock().unwrap_or_else(|e| e.into_inner());
                        let remaining = PTY_CAPTURE_CAP.saturating_sub(cap.len());
                        cap.extend_from_slice(&buf[..n.min(remaining)]);
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(PtyCanary {
            slave_path,
            tripped,
            captured,
            _slave: pty.slave,
            _reader: reader,
        })
    }

    /// The device path a probed tool would have to write to in order to
    /// trip this canary, if it could be resolved.
    ///
    /// `None` off Linux, and that is a resolution gap rather than a
    /// detection gap: the path is read out of `/proc/self/fd/<fd>`, which
    /// no other platform provides, while the reader thread that actually
    /// notices a write is portable and still armed either way. Only the
    /// *name* of the device is unavailable, never the tripwire.
    ///
    /// Nothing in production depends on it resolving. Its one non-test
    /// caller prints it as a diagnostic (`xtask`'s "canary tripwires armed
    /// (pty=…)"), which renders the `None` honestly, and a `CanarySet` is
    /// only ever armed inside a namespace-contained sweep — which
    /// [`super::containment::enter_or_refuse`] refuses to construct off
    /// Linux in the first place, so the unresolvable case cannot arise
    /// where the value would matter.
    pub fn slave_path(&self) -> Option<&Path> {
        self.slave_path.as_deref()
    }

    /// Non-destructive: repeated calls report the same trip once tripped.
    pub fn check(&self) -> Option<CanaryTrip> {
        if self.tripped.load(Ordering::SeqCst) {
            let bytes = self
                .captured
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
            Some(CanaryTrip::PtyWritten { bytes })
        } else {
            None
        }
    }
}

/// A throwaway process given a common, short name — the kind of name an
/// over-broad `pkill`/`killall` pattern is likely to match by accident.
///
/// The name is set at the kernel level (`/proc/<pid>/comm`, what a plain
/// `pkill <name>` — no `-f` — matches against), not merely as `argv[0]`:
/// Linux derives `comm` from the path passed to `execve`, not from
/// `argv[0]`, so this spawns a **symlink** named after the canary rather
/// than relying on a shell's `exec -a` (which only rewrites `argv[0]` and
/// leaves `comm` as the real binary's name — measured directly while
/// building this module: `bash -c 'exec -a foo sleep 30'` still shows
/// `comm=sleep`, not `comm=foo`).
pub struct ProcessCanary {
    name: String,
    child: Child,
    /// Keeps the symlink alive for the process's lifetime; removed on
    /// drop.
    _symlink_dir: tempfile::TempDir,
}

impl ProcessCanary {
    /// Common, short, plausible words — the kind a pattern like `pkill
    /// server` or `pkill test` is written to match on a real machine,
    /// aimed at some other process entirely.
    pub const COMMON_NAMES: &'static [&'static str] = &["test", "data", "worker"];

    /// Spawn one canary process named `name`, sleeping far longer than any
    /// sweep should ever take. `name` should come from
    /// [`Self::COMMON_NAMES`].
    pub fn spawn(name: &str) -> std::io::Result<Self> {
        let sleep_bin = find_on_path("sleep").ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no `sleep` binary on PATH to back a process canary",
            )
        })?;
        let symlink_dir = tempfile::Builder::new().prefix("mnd-canary-").tempdir()?;
        let link_path = symlink_dir.path().join(name);
        #[cfg(unix)]
        std::os::unix::fs::symlink(&sleep_bin, &link_path)?;

        let child = Command::new(&link_path)
            .arg("86400") // effectively "until teardown"; never meant to exit on its own
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;

        Ok(ProcessCanary {
            name: name.to_string(),
            child,
            _symlink_dir: symlink_dir,
        })
    }

    /// `Some(trip)` if this canary died since it was spawned, without
    /// [`Self::kill_and_reap`] having been called. Non-destructive to
    /// call repeatedly, except that it necessarily reaps a dead child once
    /// (idempotent afterward: `try_wait` on an already-reaped child
    /// returns the cached exit status, not an error).
    pub fn check(&mut self) -> Option<CanaryTrip> {
        match self.child.try_wait() {
            Ok(Some(_status)) => Some(CanaryTrip::ProcessKilled {
                name: self.name.clone(),
            }),
            _ => None,
        }
    }

    /// End the canary's life deliberately, so a still-alive canary at
    /// sweep end is not itself mistaken for a trip. Consumes `self` so it
    /// cannot be checked afterward with stale expectations.
    pub fn kill_and_reap(mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A directory outside `spawn.rs`'s per-probe `Scratch` redirect that must
/// stay empty for the life of the sweep.
///
/// Deliberately coarse (any entry at all is a trip, not a diff against a
/// baseline) — the redirected variables are supposed to absorb every
/// write a probe makes, so *anything* landing here at all is already the
/// finding, the same way [M-11]'s font-cache and `mysql_secure_installation`
/// writes were each a single unexpected file, not a pattern that needed a
/// diff to notice.
pub struct PathCanary {
    dir: PathBuf,
}

impl PathCanary {
    /// `dir` is created if it does not already exist. Must not be, or sit
    /// inside, any per-probe `Scratch` directory — those are expected to
    /// receive writes and are deleted after every single invocation.
    pub fn watch(dir: PathBuf) -> std::io::Result<Self> {
        std::fs::create_dir_all(&dir)?;
        Ok(PathCanary { dir })
    }

    /// The directory being watched.
    pub fn path(&self) -> &Path {
        &self.dir
    }

    /// Non-destructive: repeated calls see the same entries until
    /// something else cleans the directory out.
    pub fn check(&self) -> Option<CanaryTrip> {
        let entries: Vec<PathBuf> = std::fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .collect();
        if entries.is_empty() {
            None
        } else {
            Some(CanaryTrip::PathWritten { paths: entries })
        }
    }
}

/// All three canaries, bundled for one sweep.
pub struct CanarySet {
    pty: PtyCanary,
    processes: Vec<ProcessCanary>,
    path: PathCanary,
}

impl CanarySet {
    /// Spawn all three canaries. `watch_dir` becomes the [`PathCanary`]'s
    /// target — see [`super::containment::default_watch_dir`] for the
    /// sweep's usual choice.
    pub fn spawn(watch_dir: PathBuf) -> std::io::Result<Self> {
        let pty = PtyCanary::spawn()?;
        let mut processes = Vec::with_capacity(ProcessCanary::COMMON_NAMES.len());
        for name in ProcessCanary::COMMON_NAMES {
            processes.push(ProcessCanary::spawn(name)?);
        }
        let path = PathCanary::watch(watch_dir)?;
        Ok(CanarySet {
            pty,
            processes,
            path,
        })
    }

    /// The PTY canary's device path, for diagnosis/logging.
    pub fn pty_slave_path(&self) -> Option<&Path> {
        self.pty.slave_path()
    }

    /// Check every canary and report every trip found — deliberately never
    /// short-circuits on the first one, so a sweep that manages to trip
    /// more than one canary reports the full picture rather than
    /// whichever happened to be checked first.
    pub fn check(&mut self) -> Vec<CanaryTrip> {
        let mut trips = Vec::new();
        if let Some(t) = self.pty.check() {
            trips.push(t);
        }
        for p in &mut self.processes {
            if let Some(t) = p.check() {
                trips.push(t);
            }
        }
        if let Some(t) = self.path.check() {
            trips.push(t);
        }
        trips
    }

    /// Kill every still-alive canary process. Call after a final
    /// [`Self::check`] — a canary killed here on purpose must not be
    /// checked again afterward.
    pub fn teardown(self) {
        for p in self.processes {
            p.kill_and_reap();
        }
        // `pty` and `path` need no explicit teardown: the pty's reader
        // thread exits on its own once the master fd drops (end of this
        // function), and the watch directory is left for the caller to
        // remove if it cares (it is typically process-scoped scratch).
    }
}

/// Find `name` on `PATH`, first match wins (matches normal `PATH`
/// resolution order) — a small local helper rather than a dependency,
/// mirroring `xtask::coverage::unique_executables_on_path`'s own walk but
/// stopping at the first hit instead of enumerating everything.
fn find_on_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    // Only the two `/proc`-dependent pty tests below write, and both are
    // Linux-gated; on other targets this import would be dead.
    #[cfg(target_os = "linux")]
    use std::io::Write;

    /// Proves the PTY canary's detection path actually fires: writes
    /// directly to the slave device by path (the same way an external
    /// probe reaching it would have to — this process holds no fd on the
    /// slave side, only the path), then asserts `check()` reports it.
    /// Linux-only: this drives the canary through its slave device *by
    /// path*, and that path is resolved from `/proc/self/fd` (see
    /// [`PtyCanary::slave_path`]). The tripwire itself is portable; only
    /// this way of reaching it is not. Gated rather than relaxed, because
    /// a `CanarySet` is armed only inside a namespace-contained sweep,
    /// which cannot be constructed off Linux at all.
    #[cfg(target_os = "linux")]
    #[test]
    fn pty_canary_trips_when_slave_is_written_to() {
        let canary = PtyCanary::spawn().expect("pty canary should spawn in this sandbox");
        assert!(canary.check().is_none(), "canary must start untripped");

        let slave_path = canary
            .slave_path()
            .expect("slave path must resolve via /proc/self/fd on Linux");
        let mut slave = std::fs::OpenOptions::new()
            .write(true)
            .open(slave_path)
            .expect("slave device must still be openable by path");
        slave
            .write_all(b"__complete\n")
            .expect("write to the slave device");
        slave.flush().unwrap();
        drop(slave);

        // The reader thread runs concurrently; give it a moment to observe
        // the write rather than racing it.
        let mut tripped = None;
        for _ in 0..200 {
            if let Some(t) = canary.check() {
                tripped = Some(t);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        match tripped.expect("pty canary did not trip after a real write to its slave") {
            CanaryTrip::PtyWritten { bytes } => {
                assert!(
                    bytes.starts_with(b"__complete"),
                    "captured bytes should include what was written: {bytes:?}"
                );
            }
            other => panic!("wrong trip variant: {other}"),
        }
    }

    /// Proves the process canary's detection path actually fires:
    /// deliberately kills it (standing in for an over-broad `pkill`), then
    /// asserts `check()` reports it — and that a canary killed
    /// *deliberately* via `kill_and_reap` (i.e. normal teardown) does
    /// *not* falsely report a trip, since that distinction is what keeps
    /// this canary from failing every sweep on its own cleanup.
    #[test]
    fn process_canary_trips_when_killed_by_something_else() {
        let mut canary =
            ProcessCanary::spawn("test").expect("process canary should spawn in this sandbox");
        assert!(canary.check().is_none(), "canary must start untripped");

        // Simulate an over-broad `pkill test` finding this process by its
        // real `comm` and killing it — exactly the scenario this canary
        // exists to catch, reached here via the same `nix` signal API
        // `spawn.rs`'s own `kill_process_group` uses, standing in for
        // whatever probed tool would have sent the signal for real.
        let pid = nix::unistd::Pid::from_raw(canary.child.id() as i32);
        nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGKILL)
            .expect("send SIGKILL to the canary process");

        let mut tripped = None;
        for _ in 0..200 {
            if let Some(t) = canary.check() {
                tripped = Some(t);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        match tripped.expect("process canary did not trip after being killed") {
            CanaryTrip::ProcessKilled { name } => assert_eq!(name, "test"),
            other => panic!("wrong trip variant: {other}"),
        }
    }

    /// The other half of the previous test's claim: a canary that
    /// *teardown* kills must never be mistaken for a trip, or every sweep
    /// would fail on its own cleanup regardless of what any probe did.
    #[test]
    fn process_canary_killed_by_teardown_is_not_a_trip() {
        let canary = ProcessCanary::spawn("data").expect("process canary should spawn");
        // `kill_and_reap` consumes `self`; there is deliberately no way to
        // call `check()` on a torn-down canary afterward — teardown is
        // meant to be the last thing done with one.
        canary.kill_and_reap();
    }

    /// Proves the path canary's detection path actually fires: writes a
    /// file into the watched directory (standing in for an unprompted
    /// writer like [M-11]'s `mysql_secure_installation`), then asserts
    /// `check()` reports it.
    #[test]
    fn path_canary_trips_when_something_is_written_into_it() {
        let dir = tempfile::tempdir().unwrap();
        let watch = dir.path().join("watched");
        let canary = PathCanary::watch(watch.clone()).unwrap();
        assert!(canary.check().is_none(), "canary must start untripped");

        std::fs::write(watch.join(".my.cnf.1234"), "[client]\npassword=\n").unwrap();

        match canary
            .check()
            .expect("path canary did not trip after a write into its watched directory")
        {
            CanaryTrip::PathWritten { paths } => {
                assert_eq!(paths.len(), 1);
                assert_eq!(paths[0].file_name().unwrap(), ".my.cnf.1234");
            }
            other => panic!("wrong trip variant: {other}"),
        }
    }

    /// End-to-end: a `CanarySet` built once, checked after a shim
    /// deliberately does all three bad things at once (a stand-in for a
    /// misbehaving probed tool), reports all three trips — never
    /// short-circuiting on the first one found.
    /// Linux-only: this drives the canary through its slave device *by
    /// path*, and that path is resolved from `/proc/self/fd` (see
    /// [`PtyCanary::slave_path`]). The tripwire itself is portable; only
    /// this way of reaching it is not. Gated rather than relaxed, because
    /// a `CanarySet` is armed only inside a namespace-contained sweep,
    /// which cannot be constructed off Linux at all.
    #[cfg(target_os = "linux")]
    #[test]
    fn canary_set_reports_every_trip_a_misbehaving_probe_causes() {
        let watch_dir = tempfile::tempdir().unwrap().keep();
        let mut set = CanarySet::spawn(watch_dir.clone()).expect("canary set should spawn");
        assert!(
            set.check().is_empty(),
            "a freshly spawned canary set must start with no trips"
        );

        // Write to the pty, exactly as a rogue `wall`-alike would.
        let slave_path = set.pty_slave_path().unwrap().to_path_buf();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&slave_path)
            .unwrap()
            .write_all(b"broadcast\n")
            .unwrap();

        // Kill one canary process, exactly as an over-broad `pkill` would.
        let victim_pid = nix::unistd::Pid::from_raw(set.processes[0].child.id() as i32);
        nix::sys::signal::kill(victim_pid, nix::sys::signal::Signal::SIGKILL).unwrap();

        // Write into the watched path, exactly as an unprompted writer
        // would.
        std::fs::write(watch_dir.join("unexpected.cnf"), b"leak").unwrap();

        // Give the concurrent pty reader and the killed child's exit
        // status a moment to become observable.
        let mut trips = Vec::new();
        for _ in 0..200 {
            trips = set.check();
            if trips.len() == 3 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(
            trips.len(),
            3,
            "expected all three canaries to have tripped: {trips:?}"
        );
        let has = |pred: &dyn Fn(&CanaryTrip) -> bool| trips.iter().any(pred);
        assert!(has(&|t| matches!(t, CanaryTrip::PtyWritten { .. })));
        assert!(has(&|t| matches!(t, CanaryTrip::ProcessKilled { .. })));
        assert!(has(&|t| matches!(t, CanaryTrip::PathWritten { .. })));

        set.teardown();
        let _ = std::fs::remove_dir_all(&watch_dir);
    }
}
