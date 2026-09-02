//! Namespace containment for full-`PATH` sweeps (spec §6/§8's residual
//! risk, closed one layer further). Evidence-before-argv gating
//! (`spawn.rs`'s rule 0/2a checks) *prevents* what can be reasoned about
//! from argv shape alone; this module adds **containment** underneath it.
//! Must never become a reason to loosen gating — a namespace is not a
//! license to send a riskier argv.
//!
//! **The mechanism.** The sweep re-executes itself under `unshare --user
//! --map-root-user --pid --mount --fork`, so probing runs as the leader
//! of a fresh user/PID/mount namespace. `unshare(1)` is used rather than
//! a raw `unshare(2)`/`clone(2)` syscall from inside this process because
//! unsharing a PID namespace only takes effect for children created after
//! the call, requiring a fork afterward — forking a multi-threaded Rust
//! process outside `pre_exec`'s narrow, audited window is exactly what
//! this crate's `#![deny(unsafe_code)]` keeps out. The external `unshare`
//! binary gets the same containment with zero new `unsafe`.
//!
//! **What this buys:** PID namespace isolates a rogue `kill`/`pkill`
//! (spec §6 rule 0) to the sweep's own PID space; mount namespace gives a
//! private copy of the mount table; user namespace makes the other two
//! possible unprivileged, mapping in-sweep "root" to an ordinary UID.
//!
//! **What this does NOT buy:** no network namespace — a probe that phones
//! home behaves as if unsandboxed. The mount namespace is a private
//! *table*, not a private filesystem — without `pivot_root`/tmpfs overlay,
//! writes to an absolute path outside the rule 8 scratch redirect can
//! still reach a real file ([`canary::PathCanary`] exists for exactly
//! this gap). Not a substitute for argv gating — every `spawn.rs` rule
//! applies identically whether or not the sweep is namespaced.
//!
//! **Refuse loudly rather than degrade silently.** [`enter_or_refuse`]
//! requires all three namespace types; if any is unavailable it returns
//! [`ContainmentError::Unavailable`] rather than falling back to an
//! uncontained sweep.

use std::fs::{File, OpenOptions};
use std::io;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
#[cfg(target_os = "linux")]
use std::process::Command;
use thiserror::Error;

/// Set in the environment of the re-executed, already-namespaced process.
/// Its mere presence — not its value — is what [`is_contained`] checks.
pub const CONTAINED_ENV_VAR: &str = "MANDIBLE_SWEEP_CONTAINED";

/// Set alongside [`CONTAINED_ENV_VAR`] on a full-`PATH` sweep whose `--out`
/// file was pre-secured (see [`secure_out_file`]) before entering
/// containment. Value is the raw fd number of that already-open file,
/// carried across `unshare` + re-exec by clearing `FD_CLOEXEC`
/// ([`enter_or_refuse_with_scoreboard`]); [`write_scoreboard`] writes
/// through it instead of reopening the path, which can `EACCES` from
/// inside the namespace when the checkout dir has an unmapped owner.
pub const SCOREBOARD_FD_ENV_VAR: &str = "MANDIBLE_SWEEP_SCOREBOARD_FD";

/// True if this process is already running inside the namespace
/// [`enter_or_refuse`] constructs — i.e. this is the re-exec'd copy, not
/// the original invocation.
pub fn is_contained() -> bool {
    std::env::var_os(CONTAINED_ENV_VAR).is_some()
}

/// Open (creating parent directories first) `path` for reading and
/// writing, and clear `FD_CLOEXEC` so the fd survives an `exec` call.
///
/// **Must be called by the pre-exec process, before
/// [`enter_or_refuse_with_scoreboard`].** A sweep's final scoreboard write
/// happens from inside the user namespace, where the checkout directory
/// may be owned by a UID the contained "root" does not map — opening the
/// path there for creation fails `EACCES`. Opening it here, before
/// entering the namespace, and carrying the fd across sidesteps the
/// permission check instead of trying to satisfy it from the wrong side.
pub fn secure_out_file(path: &Path) -> io::Result<File> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    // truncate(false): [`write_scoreboard`] does its own `set_len(0)`
    // right before writing, so a process that opens but exits before
    // writing never destroys a previously-good scoreboard.
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    clear_cloexec(&file)?;
    Ok(file)
}

/// Clear `FD_CLOEXEC` on `file`'s underlying fd. `std::fs::File` always
/// opens with `O_CLOEXEC` set; this fd must survive the `unshare` +
/// re-exec pair, so that bit needs clearing.
#[cfg(unix)]
fn clear_cloexec(file: &File) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    let fd = file.as_raw_fd();
    let current = nix::fcntl::fcntl(fd, nix::fcntl::FcntlArg::F_GETFD)?;
    let mut flags = nix::fcntl::FdFlag::from_bits_truncate(current);
    flags.remove(nix::fcntl::FdFlag::FD_CLOEXEC);
    nix::fcntl::fcntl(fd, nix::fcntl::FcntlArg::F_SETFD(flags))?;
    Ok(())
}

/// The non-Unix twin: nothing to clear or to survive an `exec`.
#[cfg(not(unix))]
fn clear_cloexec(_file: &File) -> io::Result<()> {
    Ok(())
}

/// Write `contents` as a full-`PATH` sweep's scoreboard.
///
/// If [`SCOREBOARD_FD_ENV_VAR`] is set, writes through that inherited fd
/// (`set_len(0)` + seek-to-start + `write_all`) instead of reopening the
/// (there, possibly permission-denied) path. Otherwise falls through to
/// [`std::fs::write`] unchanged.
pub fn write_scoreboard(path: &Path, contents: &str) -> io::Result<()> {
    #[cfg(unix)]
    if let Some(mut file) = secured_scoreboard_file()? {
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(contents.as_bytes())?;
        return file.flush();
    }
    std::fs::write(path, contents)
}

/// Reconstruct the [`File`] [`enter_or_refuse_with_scoreboard`] exported,
/// if [`SCOREBOARD_FD_ENV_VAR`] is set. `Ok(None)` (not an error) when the
/// var is absent.
#[cfg(unix)]
fn secured_scoreboard_file() -> io::Result<Option<File>> {
    let Some(raw) = std::env::var_os(SCOREBOARD_FD_ENV_VAR) else {
        return Ok(None);
    };
    let fd: std::os::fd::RawFd = raw.to_str().and_then(|s| s.parse().ok()).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{SCOREBOARD_FD_ENV_VAR} is set but not a valid fd number: {raw:?}"),
        )
    })?;
    // SAFETY: this fd was opened by `secure_out_file` and exported with
    // `FD_CLOEXEC` cleared, so it is guaranteed open and valid here — the
    // contained process inherited it across `unshare` + re-exec and
    // nothing could have closed it. This is the only place that
    // reconstructs it, and the sweep writes the scoreboard exactly once
    // then exits, so there is no double-close or use-after-close.
    #[allow(unsafe_code)]
    let file = unsafe { <File as std::os::fd::FromRawFd>::from_raw_fd(fd) };
    Ok(Some(file))
}

/// Which of the three namespace types [`enter_or_refuse`] requires were
/// confirmed working on this host, checked individually so a refusal names
/// the specific layer that failed.
///
/// PID and mount checks are nested inside a user namespace
/// (`--user --map-root-user --pid …`), since creating either unprivileged
/// requires already being inside a user namespace mapping the caller to
/// root — `unshare --pid` alone fails `EPERM` even where the combined
/// invocation succeeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NamespaceSupport {
    /// `unshare --user --map-root-user -- true` succeeded.
    pub user: bool,
    /// `unshare --user --map-root-user --pid --fork -- true` succeeded.
    pub pid: bool,
    /// `unshare --user --map-root-user --mount -- true` succeeded.
    pub mount: bool,
}

impl NamespaceSupport {
    /// True only if every namespace type [`enter_or_refuse`] combines is
    /// available. Partial support is treated the same as none.
    pub fn all_supported(&self) -> bool {
        self.user && self.pid && self.mount
    }
}

/// Errors from entering the sweep namespace.
#[derive(Debug, Error)]
pub enum ContainmentError {
    /// At least one required namespace type is not available on this
    /// host. Carries the per-type breakdown so the refusal is actionable
    /// rather than a bare "no."
    #[error(
        "namespace containment unavailable on this host (user={}, pid={}, mount={}) — refusing to run a full-PATH sweep uncontained",
        .support.user, .support.pid, .support.mount
    )]
    Unavailable {
        /// The per-namespace-type breakdown.
        support: NamespaceSupport,
    },
    /// `unshare` itself, or re-executing this binary under it, could not
    /// be spawned. Distinct from `Unavailable`: namespace types probed as
    /// available, but the re-exec attempt itself still failed.
    #[error("failed to re-exec under namespace containment: {source}")]
    Reexec {
        /// The underlying OS error.
        #[source]
        source: io::Error,
    },
}

/// Probe, by actually invoking `unshare`, which of the three namespace
/// types [`enter_or_refuse`] requires work on this host. Never assumed —
/// taken fresh each call rather than cached, since a sweep is rare and
/// expensive enough that one extra `unshare` invocation is immaterial.
#[cfg(target_os = "linux")]
pub fn probe_namespace_support() -> NamespaceSupport {
    let user = unshare_probe(&["--user", "--map-root-user", "--", "true"]);
    let pid = unshare_probe(&["--user", "--map-root-user", "--pid", "--fork", "--", "true"]);
    let mount = unshare_probe(&["--user", "--map-root-user", "--mount", "--", "true"]);
    NamespaceSupport { user, pid, mount }
}

/// The non-Linux twin of [`probe_namespace_support`], reporting every
/// namespace type unavailable because none of them exists to probe.
#[cfg(not(target_os = "linux"))]
pub fn probe_namespace_support() -> NamespaceSupport {
    // Reporting every type unavailable makes `enter_or_refuse` refuse
    // loudly rather than silently no-op on this platform.
    NamespaceSupport {
        user: false,
        pid: false,
        mount: false,
    }
}

#[cfg(target_os = "linux")]
fn unshare_probe(args: &[&str]) -> bool {
    Command::new("unshare")
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Re-execute the current process under a fresh user + PID + mount
/// namespace, or return a [`ContainmentError`] explaining why not.
///
/// **This function does not return on success.** It replaces the calling
/// process's image with `unshare` (via
/// [`CommandExt::exec`](std::os::unix::process::CommandExt::exec)), which
/// unshares the three namespaces, then (`--fork`) forks a child that
/// becomes PID 1 in the new namespace and execs the original binary —
/// landing back in `main()` with [`is_contained`] now true. `unshare`,
/// not this function, is what forks and enters the namespace, which is
/// why this crate needs no additional `unsafe` for it.
///
/// Callers should treat a return from this function as failure in every
/// case — there is no `Ok` variant.
#[cfg(target_os = "linux")]
pub fn enter_or_refuse() -> ContainmentError {
    enter_or_refuse_with_scoreboard(None)
}

/// Like [`enter_or_refuse`], but first exports `scoreboard`'s raw fd (an
/// already-open file [`secure_out_file`] prepared before containment) via
/// [`SCOREBOARD_FD_ENV_VAR`], so the contained process can reach it
/// through [`write_scoreboard`] instead of reopening the (there, possibly
/// permission-denied) path. `scoreboard` is `None` for a caller with no
/// `--out` file to protect — [`enter_or_refuse`] is exactly that call.
///
/// **Does not return on success**, same as [`enter_or_refuse`].
///
/// `scoreboard` is taken as an owned parameter, not a reference: dropping
/// a `File` closes its fd immediately regardless of `FD_CLOEXEC` (which
/// only governs survival *across* `exec`, not `drop`), so a caller-held
/// binding going out of scope early would close the fd before the child
/// ever ran.
#[cfg(target_os = "linux")]
pub fn enter_or_refuse_with_scoreboard(scoreboard: Option<File>) -> ContainmentError {
    let support = probe_namespace_support();
    if !support.all_supported() {
        return ContainmentError::Unavailable { support };
    }

    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(source) => return ContainmentError::Reexec { source },
    };
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();

    let mut cmd = Command::new("unshare");
    cmd.args([
        "--user",
        "--map-root-user",
        "--pid",
        "--mount",
        "--fork",
        "--",
    ]);
    cmd.arg(&exe);
    cmd.args(&args);
    cmd.env(CONTAINED_ENV_VAR, "1");

    if let Some(file) = &scoreboard {
        use std::os::fd::AsRawFd;
        cmd.env(SCOREBOARD_FD_ENV_VAR, file.as_raw_fd().to_string());
    }

    // `exec` replaces this process's image; returns only on failure. The
    // child that eventually runs `exe` is spawned by `unshare` after its
    // own fork, never this Rust code. `scoreboard` is still alive here
    // (not yet dropped), letting its FD_CLOEXEC-cleared fd survive into it.
    use std::os::unix::process::CommandExt;
    let source = cmd.exec();
    // Only reached on failure — `scoreboard` drops here, which is fine.
    ContainmentError::Reexec { source }
}

/// The non-Linux twin of [`enter_or_refuse`]: always refuses, carrying the
/// same all-unavailable support report.
#[cfg(not(target_os = "linux"))]
pub fn enter_or_refuse() -> ContainmentError {
    ContainmentError::Unavailable {
        support: probe_namespace_support(),
    }
}

/// The non-Linux twin of [`enter_or_refuse_with_scoreboard`]: defers to
/// [`enter_or_refuse`] and ignores `scoreboard`.
#[cfg(not(target_os = "linux"))]
pub fn enter_or_refuse_with_scoreboard(_scoreboard: Option<File>) -> ContainmentError {
    enter_or_refuse()
}

/// Where a contained sweep should point its [`super::canary::PathCanary`]:
/// a directory that is not any per-probe `Scratch` subdirectory (those are
/// deleted after every invocation and are supposed to absorb writes) but
/// still lives inside the namespace's private mount table.
pub fn default_watch_dir() -> io::Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!("mandible-sweep-watch-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// Fails loudly if this host loses one of the three namespace types
    /// (old kernel, `CONFIG_USER_NS` disabled, nested namespaces denied)
    /// rather than letting `enter_or_refuse` silently start refusing every
    /// sweep with no test explaining why.
    #[test]
    fn namespace_support_is_confirmed_on_this_host() {
        let support = probe_namespace_support();
        assert!(
            support.all_supported(),
            "expected user+PID+mount namespaces all available on this host, got {support:?} — \
             see this module's doc comment for the exact `unshare` invocations probed"
        );
    }

    /// An invocation `unshare` itself rejects must report `false`, not
    /// panic. Independent of any real namespace working.
    #[test]
    fn unshare_probe_reports_false_on_a_failing_invocation() {
        assert!(!unshare_probe(&[
            "--this-flag-does-not-exist",
            "--",
            "true"
        ]));
    }

    /// `NamespaceSupport::all_supported` is conjunctive: any single
    /// `false` must fail it, not just all three at once.
    #[test]
    fn all_supported_requires_every_axis() {
        let combos = [
            (false, true, true),
            (true, false, true),
            (true, true, false),
            (true, true, true),
        ];
        for (user, pid, mount) in combos {
            let support = NamespaceSupport { user, pid, mount };
            assert_eq!(support.all_supported(), user && pid && mount, "{support:?}");
        }
    }

    const ROLE_VAR: &str = "MANDIBLE_CONTAINMENT_TEST_ROLE";
    const WORKER_ROLE: &str = "reexec-worker";

    /// End-to-end proof that [`enter_or_refuse`] actually lands the
    /// process inside a fresh user+PID+mount namespace, not just that it
    /// constructs a plausible-looking argv. Spawns a fresh copy of this
    /// test binary, since the real behaviour only happens through a full
    /// process-image replacement (`exec`), unsafe to exercise inside the
    /// already-running, multi-threaded test process itself.
    #[test]
    fn enter_or_refuse_lands_inside_a_fresh_namespace() {
        if std::env::var(ROLE_VAR).as_deref() == Ok(WORKER_ROLE) {
            run_reexec_worker();
        }

        if !probe_namespace_support().all_supported() {
            panic!("namespace support is missing — see namespace_support_is_confirmed_on_this_host for the breakdown");
        }

        let exe = std::env::current_exe().expect("path to this test binary");
        let output = Command::new(&exe)
            .arg("--exact")
            .arg("exec::containment::tests::enter_or_refuse_lands_inside_a_fresh_namespace")
            .arg("--nocapture")
            .arg("--test-threads=1")
            .env(ROLE_VAR, WORKER_ROLE)
            .env_remove(CONTAINED_ENV_VAR)
            .output()
            .expect("spawn a fresh worker copy of this test binary");

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        assert!(
            stdout.contains("CONTAINED:uid=0"),
            "worker did not report landing inside the namespace as uid 0 (exit={:?}):\nstdout={stdout}\nstderr={stderr}",
            output.status.code()
        );
    }

    /// Becomes the re-exec target. On its first (uncontained) run it calls
    /// the production function under test; `unshare` replaces this
    /// process, so a second, contained incarnation runs afterward and
    /// reports what it observed. Always exits rather than returning.
    fn run_reexec_worker() -> ! {
        if is_contained() {
            let uid = effective_uid()
                .map(|u| u.to_string())
                .unwrap_or_else(|| "?".to_string());
            let pid = std::process::id();
            println!("CONTAINED:uid={uid},pid={pid}");
            std::process::exit(0);
        }
        let err = enter_or_refuse();
        eprintln!("enter_or_refuse did not land inside the namespace: {err}");
        std::process::exit(2);
    }

    /// The effective UID, read from `/proc/self/status` rather than a
    /// `libc`/`nix` syscall wrapper, avoiding both an `unsafe_code`
    /// exception and an unenabled `nix` feature for a test-diagnostic read.
    fn effective_uid() -> Option<u32> {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("Uid:") {
                return rest.split_whitespace().nth(1)?.parse().ok();
            }
        }
        None
    }

    const SCOREBOARD_TEST_DIR_VAR: &str = "MANDIBLE_CONTAINMENT_TEST_SCOREBOARD_DIR";
    const SCOREBOARD_WORKER_ROLE: &str = "scoreboard-fd-worker";

    /// Reproduces the property this feature exists for under the real
    /// mechanism: a directory whose owner is not the uid `--map-root-user`
    /// maps into the namespace fails `EACCES` opened *by path* from
    /// inside; writing through the fd [`secure_out_file`] opened *before*
    /// entering the namespace still succeeds.
    ///
    /// **Needs real root** via passwordless `sudo -n`: an unprivileged
    /// `--map-root-user` can only map the caller's own real uid, so there
    /// is no way to manufacture "a uid the namespace does not map"
    /// without a second real uid in the picture. Panics loudly rather
    /// than skipping silently when passwordless `sudo` is unavailable.
    #[test]
    fn write_scoreboard_survives_containment_when_the_out_dir_has_an_unmapped_owner() {
        if std::env::var(ROLE_VAR).as_deref() == Ok(SCOREBOARD_WORKER_ROLE) {
            run_scoreboard_worker();
        }

        if !Command::new("sudo")
            .args(["-n", "true"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            panic!(
                "this test requires passwordless `sudo` (CI's containment job already assumes \
                 it — see AGENTS.md's 'dev box gotchas'): it is the only way to reproduce a \
                 directory owner that an unprivileged `--map-root-user` cannot map into the \
                 namespace, which is the exact condition GitHub Actions run 32063212492 failed \
                 under."
            );
        }

        let dir = tempfile::tempdir().expect("tempdir");
        // 0o755 on the outer dir so the contained worker can traverse into
        // it; `unmapped_owner_dir` below stays 0o700 deliberately — that's
        // the condition under test.
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755))
            .expect("chmod scratch dir traversable");
        let unmapped_owner_dir = dir.path().join("unmapped-owner");
        std::fs::create_dir(&unmapped_owner_dir).expect("create test dir");
        std::fs::set_permissions(&unmapped_owner_dir, std::fs::Permissions::from_mode(0o700))
            .expect("chmod test dir");

        // Copied to a world-traversable scratch path rather than exec'd
        // from its real build location (under this developer's `$HOME`,
        // itself unmapped-owner from the namespace's perspective), which
        // would otherwise make `unshare` fail to exec the worker at all.
        let exe = std::env::current_exe().expect("path to this test binary");
        let worker_exe = dir.path().join("worker-binary");
        std::fs::copy(&exe, &worker_exe)
            .expect("copy this test binary to a world-traversable scratch path");
        std::fs::set_permissions(&worker_exe, std::fs::Permissions::from_mode(0o755))
            .expect("chmod worker binary copy executable");

        let output = Command::new("sudo")
            .args([
                "-n",
                "--preserve-env=MANDIBLE_CONTAINMENT_TEST_ROLE,\
                 MANDIBLE_CONTAINMENT_TEST_SCOREBOARD_DIR",
                "--",
            ])
            .arg(&worker_exe)
            .args([
                "--exact",
                "exec::containment::tests::write_scoreboard_survives_containment_when_the_out_dir_has_an_unmapped_owner",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(ROLE_VAR, SCOREBOARD_WORKER_ROLE)
            .env(SCOREBOARD_TEST_DIR_VAR, &unmapped_owner_dir)
            .env_remove(CONTAINED_ENV_VAR)
            .env_remove(SCOREBOARD_FD_ENV_VAR)
            .output()
            .expect("spawn a root worker copy of this test binary via sudo -n");

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        assert!(
            stdout.contains("BY-PATH: EACCES as expected")
                && stdout.contains("VIA-FD: write succeeded"),
            "worker did not reproduce the expected split outcome (exit={:?}):\n\
             stdout={stdout}\nstderr={stderr}",
            output.status.code()
        );

        let written = std::fs::read_to_string(unmapped_owner_dir.join("scoreboard.md"))
            .expect("scoreboard file the worker should have written via the inherited fd");
        assert_eq!(written, "via-fd-contents\n");
    }

    /// Becomes the re-exec target for the test above, run as real root
    /// (via `sudo -n`). Mirrors [`run_reexec_worker`]'s shape, but
    /// exercises [`secure_out_file`] + [`enter_or_refuse_with_scoreboard`]
    /// + [`write_scoreboard`] instead of plain [`enter_or_refuse`].
    fn run_scoreboard_worker() -> ! {
        let dir = std::env::var(SCOREBOARD_TEST_DIR_VAR)
            .expect("worker needs the unmapped-owner test dir path");
        let path = std::path::Path::new(&dir).join("scoreboard.md");

        if is_contained() {
            // Prove the by-path open fails here...
            match OpenOptions::new().write(true).open(&path) {
                Err(e) if e.kind() == io::ErrorKind::PermissionDenied => {
                    println!("BY-PATH: EACCES as expected: {e}");
                }
                Err(e) => {
                    eprintln!("BY-PATH: failed with an unexpected error (not EACCES): {e}");
                    std::process::exit(2);
                }
                Ok(_) => {
                    eprintln!("BY-PATH: succeeded — test rig did not reproduce an unmapped owner");
                    std::process::exit(2);
                }
            }

            // ...while the fd secured before containment still works.
            match write_scoreboard(&path, "via-fd-contents\n") {
                Ok(()) => println!("VIA-FD: write succeeded"),
                Err(e) => {
                    eprintln!("VIA-FD: write failed (this is the bug, unfixed): {e}");
                    std::process::exit(1);
                }
            }
            std::process::exit(0);
        }

        let file = secure_out_file(&path).expect("secure_out_file (running as real root)");
        let err = enter_or_refuse_with_scoreboard(Some(file));
        eprintln!("enter_or_refuse_with_scoreboard did not land inside the namespace: {err}");
        std::process::exit(2);
    }
}
