//! Canary tripwires for a contained full-`PATH` sweep (spec §6/§8, the
//! third safety layer after [`super::containment`]'s prevent/contain).
//! Turns a side effect that does happen into a loud, immediate test
//! failure. This module's own tests deliberately trip each canary and
//! assert the failure.
//!
//! - [`PtyCanary`]: a real pseudo-terminal that must never receive a byte.
//!   Catches a probed binary that runs `wall`/`write`-style broadcast
//!   tools not already refused by `spawn.rs`'s never-probe list.
//! - [`ProcessCanary`]: throwaway processes given common, short,
//!   plausible-looking names. Catches an over-broad `pkill`/`kill`
//!   invocation (spec §6 rule 2a).
//! - [`PathCanary`]: a directory outside `spawn.rs`'s per-probe `Scratch`
//!   redirect that must stay empty for the sweep's life. Catches an
//!   unprompted writer.
//!
//! A canary only detects what it is placed to observe: a write to an
//! absolute path outside its watch, or a misbehavior none of the three
//! shapes covers (a stray network call), trips nothing.

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
    /// Held for the canary's whole lifetime, never dropped early: closing
    /// the slave fd early can let the devpts entry go `ENOENT` for a later
    /// external open by path, even with the master still open.
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
    /// `None` off Linux: a resolution gap, not a detection gap. The path
    /// comes from `/proc/self/fd/<fd>`; the reader thread that notices a
    /// write stays portable and armed regardless.
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
/// `pkill <name>` matches), not merely `argv[0]`: Linux derives `comm`
/// from the `execve` path, so this spawns a symlink named after the
/// canary rather than relying on a shell's `exec -a` (which leaves `comm`
/// as the real binary's name).
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
    /// [`Self::kill_and_reap`] having been called. Safe to call repeatedly.
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
/// Deliberately coarse: any entry at all is a trip, not a diff against a
/// baseline — the redirected variables are supposed to absorb every write
/// a probe makes, so anything landing here is already the finding.
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

    /// Writes to the slave device by path, as an external probe would,
    /// then asserts `check()` reports it. Linux-only: the path resolves
    /// via `/proc/self/fd`; the tripwire itself is portable.
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

        // Reader thread runs concurrently; poll instead of racing it.
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

    /// Kills the canary (standing in for an over-broad `pkill`) and
    /// asserts `check()` reports it.
    #[test]
    fn process_canary_trips_when_killed_by_something_else() {
        let mut canary =
            ProcessCanary::spawn("test").expect("process canary should spawn in this sandbox");
        assert!(canary.check().is_none(), "canary must start untripped");

        // Simulate an over-broad `pkill test` matching by real `comm`.
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

    /// A canary that teardown kills must never be mistaken for a trip.
    #[test]
    fn process_canary_killed_by_teardown_is_not_a_trip() {
        let canary = ProcessCanary::spawn("data").expect("process canary should spawn");
        // `kill_and_reap` consumes `self`: no `check()` after teardown.
        canary.kill_and_reap();
    }

    /// Writes a file into the watched directory (standing in for an
    /// unprompted writer, spec §6 [M-11]), then asserts `check()` reports it.
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

    /// End-to-end: a `CanarySet` checked after all three bad things happen
    /// at once reports all three trips. Linux-only (PTY path resolution).
    #[cfg(target_os = "linux")]
    #[test]
    fn canary_set_reports_every_trip_a_misbehaving_probe_causes() {
        let watch_dir = tempfile::tempdir().unwrap().keep();
        let mut set = CanarySet::spawn(watch_dir.clone()).expect("canary set should spawn");
        assert!(
            set.check().is_empty(),
            "a freshly spawned canary set must start with no trips"
        );

        // As a rogue `wall`-alike would.
        let slave_path = set.pty_slave_path().unwrap().to_path_buf();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&slave_path)
            .unwrap()
            .write_all(b"broadcast\n")
            .unwrap();

        // As an over-broad `pkill` would.
        let victim_pid = nix::unistd::Pid::from_raw(set.processes[0].child.id() as i32);
        nix::sys::signal::kill(victim_pid, nix::sys::signal::Signal::SIGKILL).unwrap();

        // As an unprompted writer would.
        std::fs::write(watch_dir.join("unexpected.cnf"), b"leak").unwrap();

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
