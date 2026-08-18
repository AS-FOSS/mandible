//! Namespace containment for full-`PATH` sweeps (spec §6/§8's residual
//! risk, closed one layer further). A coverage or audit sweep invokes
//! `--help`/`-h` on every executable on `PATH` — dozens to low thousands of
//! arbitrary third-party binaries in one process — and no static check can
//! enumerate ahead of time what those binaries actually do. Evidence-
//! before-argv gating (`help_text::sections`'s `heading_attested`, this
//! module's sibling `spawn.rs`'s rule 0/2a checks) *prevents* what it can
//! reason about from argv shape alone. This module adds the layer
//! underneath prevention: **containment**. It does not replace gating, and
//! must never become a reason to loosen it — a namespace around a probe is
//! not a license to send it a riskier argv (see this module's own "what
//! this does not buy" section below, and spec §6/§8).
//!
//! **The mechanism.** The sweep process re-executes itself under
//! `unshare --user --map-root-user --pid --mount --fork`, so the actual
//! probing runs as the leader of a brand-new user, PID and mount namespace
//! rather than directly on the operator's machine. `unshare(1)` (util-
//! linux) is used rather than a raw `unshare(2)`/`clone(2)` syscall from
//! inside this process, for a specific reason: this crate keeps `unsafe`
//! to a short, audited list (`spawn.rs`'s `setsid` `pre_exec`, and this
//! module's own fd-reconstruction in `secured_scoreboard_file` — see
//! their doc comments), and calling `unshare(CLONE_NEWPID)` on a live,
//! potentially multi-threaded process is unsound in exactly the way
//! `pre_exec` documents — the PID namespace only takes effect for
//! *children* created after the call, so entering it correctly requires a
//! fork, and forking a multi-threaded Rust process outside of `pre_exec`'s
//! narrow, audited, async-signal-safe window is the hazard this crate's
//! `#![deny(unsafe_code)]` exists to keep out. Re-executing through the
//! external `unshare` binary gets the same containment with zero new
//! `unsafe` in this crate: it is just another [`std::process::Command`],
//! sanctioned exactly like every other spawn in this module.
//!
//! **What this buys, precisely.**
//! - **PID namespace**: a rogue `kill`/`pkill`/`killall` run by a probed
//!   tool can only see and signal processes inside the sweep's own PID
//!   namespace — the operator's shell, browser, and everything else on the
//!   real machine is a different, invisible PID space. This is containment
//!   for exactly the class of hazard spec §6 rule 0 was written for
//!   (`pkill -- ""` terminating every process on the machine, [M-x]).
//! - **Mount namespace**: the sweep gets a private copy of the mount
//!   table, so a probe that mounts or unmounts something cannot change
//!   what the operator's shell sees once the sweep exits.
//! - **User namespace**: what makes creating the other two possible
//!   unprivileged (verified on this host below — every namespace type this
//!   module requires was confirmed working via a real `unshare` invocation
//!   before this module was written, not assumed), and it means anything
//!   that appears to run as "root" inside the sweep maps back to an
//!   ordinary, unprivileged, sweep-only UID outside it.
//!
//! **What this does NOT buy, and must not be read as buying:**
//! - **No network namespace is requested.** A probe that phones home,
//!   resolves DNS, or opens a socket does so exactly as if unsandboxed.
//!   This containment layer is silent on network side effects entirely —
//!   said plainly here because "namespaced" is an easy word to
//!   over-read as "sandboxed against everything."
//! - **The mount namespace is a private *table*, not a private
//!   filesystem.** Without an explicit `pivot_root`/tmpfs overlay (which
//!   this module does not do), the namespace's mounts still point at the
//!   same underlying inodes as the host. A probe writing to an absolute
//!   path outside the §6 rule 8 scratch redirect (`spawn.rs`'s `Scratch`)
//!   can still reach a real file. This is the same residual risk
//!   `exec/mod.rs` already documents for the scratch redirect alone; the
//!   namespace does not shrink it, and [`canary::PathCanary`] exists
//!   precisely because prevention here is incomplete and detection is the
//!   next layer, not a substitute for it.
//! - **Not a substitute for argv gating.** Every rule in `spawn.rs` — the
//!   never-probe list, the empty-argument refusal, the timeout, the output
//!   cap, the scratch redirect — applies identically whether or not the
//!   sweep is namespaced. Containment is what happens *after* a probe is
//!   already judged safe enough to run; it is not a reason to judge more
//!   things safe.
//!
//! **Refuse loudly rather than degrade silently.** [`enter_or_refuse`]
//! requires all three namespace types. If any is unavailable on the host —
//! an old kernel, `CONFIG_USER_NS` disabled, a container already denying
//! nested namespaces — it returns [`ContainmentError::Unavailable`] rather
//! than falling back to an uncontained sweep. A documented gap that stops
//! the sweep is a result; a silently-missing containment layer is a
//! hazard, which is exactly what a full-`PATH` sweep run directly on a
//! developer's own machine already was before this module existed.

use std::fs::{File, OpenOptions};
use std::io;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
#[cfg(target_os = "linux")]
use std::process::Command;
use thiserror::Error;

/// Set in the environment of the re-executed, already-namespaced process.
/// Its mere presence — not its value — is what [`is_contained`] checks, so
/// a sweep can tell "this is the first, uncontained invocation" from "this
/// is the re-exec landing inside the namespace" without any other signal.
pub const CONTAINED_ENV_VAR: &str = "MANDIBLE_SWEEP_CONTAINED";

/// Set alongside [`CONTAINED_ENV_VAR`] on a full-`PATH` sweep whose `--out`
/// file was pre-secured (see [`secure_out_file`]) before entering
/// containment. Its value is the raw fd number of that already-open file,
/// carried across `unshare` + re-exec by clearing `FD_CLOEXEC`
/// ([`enter_or_refuse_with_scoreboard`]); [`write_scoreboard`] reads it to
/// write the scoreboard through the fd instead of reopening the path, which
/// is what fails `EACCES` from inside the namespace (GitHub Actions run
/// 32063212492: all 16 shards completed their sweep, then died with
/// `failed to write scoreboard to shard-0.md: Permission denied`).
pub const SCOREBOARD_FD_ENV_VAR: &str = "MANDIBLE_SWEEP_SCOREBOARD_FD";

/// True if this process is already running inside the namespace
/// [`enter_or_refuse`] constructs — i.e. this is the re-exec'd copy, not
/// the original invocation.
pub fn is_contained() -> bool {
    std::env::var_os(CONTAINED_ENV_VAR).is_some()
}

/// Open (creating parent directories first, exactly like `xtask`'s own
/// `write_out` helper) `path` for reading and writing, and clear
/// `FD_CLOEXEC` on the result so the fd survives an `exec` call.
///
/// **Must be called by the pre-exec process, before [`enter_or_refuse_with_scoreboard`].**
/// This is the fix for GitHub Actions run 32063212492: a full-`PATH` sweep's
/// final write of its `--out` scoreboard happens from inside the user
/// namespace [`enter_or_refuse`] builds, where the checkout directory is
/// owned by a UID the contained "root" does not map — no `CAP_DAC_OVERRIDE`,
/// so opening the path there for creation fails `EACCES` even though every
/// probe in the sweep just ran successfully. The process calling this
/// function, by contrast, has not entered the namespace yet and has
/// ordinary access to `path`; opening it here and carrying the already-open
/// fd across `unshare` (via [`enter_or_refuse_with_scoreboard`] +
/// [`write_scoreboard`]) sidesteps the permission check instead of trying
/// to satisfy it from the wrong side of it.
pub fn secure_out_file(path: &Path) -> io::Result<File> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    // `truncate(false)`: an existing file at `path` (e.g. a re-run over a
    // previous sweep's scoreboard) is left alone here — [`write_scoreboard`]
    // does its own `set_len(0)` right before writing, not this open, so a
    // process that opens the file but exits before writing (an early
    // `enter_or_refuse_with_scoreboard` failure, say) never destroys a
    // previously-good scoreboard.
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?;
    clear_cloexec(&file)?;
    Ok(file)
}

/// Clear `FD_CLOEXEC` on `file`'s underlying fd. Rust's `std::fs::File`
/// always opens with `O_CLOEXEC` set (so an accidental `fork`+`exec`
/// elsewhere in the process never leaks fds), which is exactly the bit
/// [`secure_out_file`]'s caller needs cleared: the whole point is for this
/// fd to survive the `unshare` + re-exec pair.
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

/// The non-Unix twin: nothing to clear, and nothing on this platform will
/// ever call [`enter_or_refuse_with_scoreboard`] with a fd that needs to
/// survive an `exec` it also never performs.
#[cfg(not(unix))]
fn clear_cloexec(_file: &File) -> io::Result<()> {
    Ok(())
}

/// Write `contents` as a full-`PATH` sweep's scoreboard.
///
/// If [`SCOREBOARD_FD_ENV_VAR`] is set — this is the contained half of a
/// sweep whose `--out` file [`secure_out_file`] already opened before
/// containment — writes through that inherited fd: `set_len(0)` +
/// seek-to-start + `write_all` produces the same bytes an ordinary
/// [`std::fs::write`] would, just through an already-open descriptor
/// instead of reopening the (there, permission-denied) path. Otherwise
/// falls straight through to [`std::fs::write`], unchanged — every
/// uncontained run, every `--tools`-pinned run (never containerized to
/// begin with), and every non-Unix platform all take this branch.
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
/// var is absent, since that is the ordinary case for almost every caller.
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
    // SAFETY: this fd was opened by `secure_out_file` in the pre-exec
    // process and exported via `SCOREBOARD_FD_ENV_VAR` specifically by
    // clearing `FD_CLOEXEC` on it (see that function's and
    // `enter_or_refuse_with_scoreboard`'s doc comments), so it is
    // guaranteed to still be open and valid here — the contained process
    // inherited it across `unshare` + re-exec and nothing in between could
    // have closed it. `from_raw_fd` takes ownership of the fd; this is the
    // only place in the process that reconstructs it from the env var, a
    // full-`PATH` sweep writes its scoreboard exactly once at the very end
    // of the run, and the process exits immediately after, so there is no
    // double-close or later use-after-close.
    #[allow(unsafe_code)]
    let file = unsafe { <File as std::os::fd::FromRawFd>::from_raw_fd(fd) };
    Ok(Some(file))
}

/// Which of the three namespace types [`enter_or_refuse`] requires were
/// confirmed working on this host, checked individually so a refusal names
/// the specific layer that failed rather than a single opaque "no."
///
/// Checked with a user namespace as the enclosing scope for the PID and
/// mount checks (`--user --map-root-user --pid …` / `--user --map-root-user
/// --mount …`), because on every Linux host measured for this module,
/// creating a PID or mount namespace unprivileged requires already being
/// inside a user namespace that maps the caller to root — `unshare --pid`
/// alone, with no enclosing `--user`, was measured failing with `EPERM`
/// even though `--user --map-root-user --pid` together succeed. Testing
/// `--pid` in isolation would therefore report a false negative for a host
/// where the real, combined invocation works fine.
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
    /// available. Partial support is treated the same as none — a sweep
    /// contained on two axes but not the third is not "mostly contained,"
    /// it is uncontained on the one axis that matters for whatever a
    /// future probe does.
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
    /// be spawned. Distinct from `Unavailable`: the namespace types probed
    /// as available, but the actual re-exec attempt still failed (`unshare`
    /// disappeared between the probe and the attempt, the current
    /// executable's path could not be resolved, etc).
    #[error("failed to re-exec under namespace containment: {source}")]
    Reexec {
        /// The underlying OS error.
        #[source]
        source: io::Error,
    },
}

/// Probe, by actually invoking `unshare`, which of the three namespace
/// types [`enter_or_refuse`] requires work on this host. Never assumed —
/// container hosts, old kernels, and `sysctl kernel.unprivileged_userns_clone`
/// all vary this, so the only honest answer is a real measurement, taken
/// fresh each call rather than cached, since a sweep is a rare, expensive
/// operation for which one extra `unshare` invocation is immaterial cost.
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
    // Linux namespaces have no equivalent this module constructs on any
    // other platform. Reporting every type unavailable is what makes
    // `enter_or_refuse` refuse loudly here rather than silently no-op —
    // spec §7's non-Linux tiers (Windows in particular) still work; a
    // full-PATH sweep on those platforms is simply not containable by
    // this module and must say so.
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
/// itself unshares the three namespaces and then, because of `--fork`,
/// forks a child that becomes PID 1 in the new namespace and execs the
/// original binary with the original argv — landing back in `main()` with
/// [`is_contained`] now true. The `unshare` step, not this function's own
/// code, is what actually enters the namespace, which is why this crate
/// needs no additional `unsafe`: unsharing PID and mount namespaces
/// correctly requires forking afterward, and that fork is `unshare(1)`'s
/// job, not this process's — see this module's top doc comment for why
/// doing it any other way would need exactly the kind of raw,
/// post-`fork()` Rust code this crate's `#![deny(unsafe_code)]` keeps out.
///
/// Callers should treat a return from this function as failure in every
/// case — there is no `Ok` variant — and refuse to run the sweep rather
/// than falling through to an uncontained run.
#[cfg(target_os = "linux")]
pub fn enter_or_refuse() -> ContainmentError {
    enter_or_refuse_with_scoreboard(None)
}

/// Like [`enter_or_refuse`], but first exports `scoreboard`'s raw fd (an
/// already-open file [`secure_out_file`] prepared before containment) via
/// [`SCOREBOARD_FD_ENV_VAR`] on the `unshare` command, so the re-exec'd,
/// contained process can reach it through [`write_scoreboard`] instead of
/// reopening the (there, permission-denied) path. `scoreboard` is `None`
/// for a caller with no `--out` file to protect — `enter_or_refuse` is
/// exactly that call with `None`.
///
/// **This function does not return on success**, same as [`enter_or_refuse`].
///
/// `scoreboard`, when `Some`, must stay alive until `unshare` replaces this
/// process's image, which is why it is taken as an owned parameter of this
/// function's own stack frame rather than a caller-held reference: dropping
/// a `File` closes its fd immediately regardless of `FD_CLOEXEC` —
/// `FD_CLOEXEC` only governs survival *across* `exec`, not `drop` — so a
/// caller that opened the file, passed a reference, and let its own local
/// binding go out of scope before calling this function would close the fd
/// out from under the child before it ever ran.
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

    // `exec` replaces this process's image; it returns only on failure to
    // do so (the child that eventually runs `exe` again is a *different*
    // OS process, spawned by `unshare` itself after its own fork — never
    // this Rust code). `scoreboard` is still alive at this point (it is
    // this frame's own local, not yet dropped), which is what lets its
    // FD_CLOEXEC-cleared fd survive into the child.
    use std::os::unix::process::CommandExt;
    let source = cmd.exec();
    // Only reached on failure — `scoreboard` drops here, which is fine: the
    // sweep is about to bail out with `source` as the error either way.
    ContainmentError::Reexec { source }
}

/// The non-Linux twin of [`enter_or_refuse`]: nothing to enter, so it
/// always refuses, carrying the same all-unavailable support report.
#[cfg(not(target_os = "linux"))]
pub fn enter_or_refuse() -> ContainmentError {
    ContainmentError::Unavailable {
        support: probe_namespace_support(),
    }
}

/// The non-Linux twin of [`enter_or_refuse_with_scoreboard`]: nothing to
/// enter and no fd-passing to do, so it just defers to [`enter_or_refuse`]
/// and ignores `scoreboard`.
#[cfg(not(target_os = "linux"))]
pub fn enter_or_refuse_with_scoreboard(_scoreboard: Option<File>) -> ContainmentError {
    enter_or_refuse()
}

/// Where a contained sweep should point its [`super::canary::PathCanary`]:
/// a directory that is not any of `spawn.rs`'s per-probe `Scratch`
/// subdirectories (those are deleted after every single invocation and are
/// *supposed* to absorb writes) but still lives inside the namespace's
/// private mount table, so nothing under it is the operator's real
/// filesystem. Created fresh per sweep, alongside — never inside — the
/// scratch root any individual probe gets.
pub fn default_watch_dir() -> io::Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!("mandible-sweep-watch-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// **Checked, not assumed.** This module's own doc comment says every
    /// namespace type it needs was "confirmed working on this host before
    /// this module was written" — this test is what keeps that claim
    /// honest on every future run rather than letting it go stale. If a
    /// CI image or dev box loses one of the three (an old kernel,
    /// `CONFIG_USER_NS` disabled, a container denying nested namespaces),
    /// this fails loudly instead of `enter_or_refuse` silently starting to
    /// refuse every sweep with no test ever having said why.
    #[test]
    fn namespace_support_is_confirmed_on_this_host() {
        let support = probe_namespace_support();
        assert!(
            support.all_supported(),
            "expected user+PID+mount namespaces all available on this host, got {support:?} — \
             see this module's doc comment for the exact `unshare` invocations probed"
        );
    }

    /// The failure path of the underlying probe: an invocation `unshare`
    /// itself rejects must report `false`, not panic or silently succeed.
    /// Deterministic and independent of any real namespace working —
    /// unlike the positive test above, this does not depend on host
    /// capabilities at all, only on `unshare` recognizing its own flags.
    #[test]
    fn unshare_probe_reports_false_on_a_failing_invocation() {
        assert!(!unshare_probe(&[
            "--this-flag-does-not-exist",
            "--",
            "true"
        ]));
    }

    /// `NamespaceSupport::all_supported` is conjunctive: any single `false`
    /// must fail it, not just all three at once. Partial support is
    /// treated as no support, per this module's own containment claim.
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
    /// process inside a fresh user+PID+mount namespace — not just that it
    /// constructs a plausible-looking argv. Uses the same "spawn a fresh
    /// copy of this test binary" pattern as `tests/exec_policy.rs`'s
    /// `dev_tty_hazard` test, for the same reason: the real behaviour only
    /// happens through a full process-image replacement (`exec`), which
    /// cannot be exercised safely inside the already-running,
    /// multi-threaded test process itself.
    #[test]
    fn enter_or_refuse_lands_inside_a_fresh_namespace() {
        if std::env::var(ROLE_VAR).as_deref() == Ok(WORKER_ROLE) {
            run_reexec_worker();
        }

        if !probe_namespace_support().all_supported() {
            // Already asserted by `namespace_support_is_confirmed_on_this_host`
            // above; this test only adds the re-exec proof on top and
            // would be a redundant, confusing second failure here.
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

    /// Becomes the re-exec target. On its first run (uncontained) it calls
    /// the real production function under test; `unshare` replaces this
    /// very process, so a *second* incarnation of this same function runs
    /// afterward, this time contained, and reports what it observed.
    /// Always exits the process rather than returning, exactly like
    /// `dev_tty_hazard`'s worker — it must never fall back into the
    /// orchestrator's own test-body logic above.
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
    /// `libc`/`nix` syscall wrapper: this crate's `#![deny(unsafe_code)]`
    /// allows only a short, audited list of exceptions (see `lib.rs`'s doc
    /// comment), and `nix::unistd::Uid` additionally needs a crate feature
    /// this workspace does not enable — plain, safe file I/O avoids both
    /// for what is only ever a test-diagnostic read.
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

    /// **The property this whole feature exists for, proven under the real
    /// mechanism, not asserted from reading the code.** Reproduces GitHub
    /// Actions run 32063212492's exact failure shape locally: a directory
    /// whose owner is not the uid `enter_or_refuse`'s `--map-root-user`
    /// maps into the namespace. Opening a file there *by path* from inside
    /// the namespace fails `EACCES` — this is the bug: all 16 shards of a
    /// full-`PATH` sweep finished their probing and then died writing
    /// `shard-N.md` exactly this way. Writing through the fd
    /// [`secure_out_file`] opened *before* entering the namespace, carried
    /// across by [`enter_or_refuse_with_scoreboard`], still succeeds.
    ///
    /// **Needs real root**, via passwordless `sudo -n`, not just an
    /// unprivileged user namespace: an unprivileged `--map-root-user` can
    /// only ever map the caller's *own* real uid into the namespace, so a
    /// directory the caller itself owns is always reachable from inside
    /// regardless of this fix — there is no way to manufacture "a uid the
    /// namespace does not map" without a second, different real uid in the
    /// picture. Real root bypasses ownership checks entirely in the
    /// *outer* (pre-exec) step — which is what CI's privileged container
    /// actually is — and after `unshare --map-root-user` maps real uid 0 to
    /// ns uid 0 (identity: root mapping itself), a directory owned by any
    /// *other* real uid (here: whichever unprivileged uid is running
    /// `cargo nextest`) is exactly as unmapped inside the namespace as the
    /// checkout directory was in CI. Confirmed by hand against this exact
    /// mechanism before writing this test: `sudo unshare --user
    /// --map-root-user -- touch <dir-owned-by-a-non-root-uid>/f` fails
    /// `EACCES`; plain `sudo touch` on the same path (no `unshare`)
    /// succeeds. Panics loudly, rather than skipping silently, when
    /// passwordless `sudo` is unavailable — see the CI containment job's
    /// own `sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0`
    /// (AGENTS.md "dev box gotchas"), which already assumes exactly this
    /// capability for the namespace-support tests above.
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
        // `tempfile::tempdir()` defaults to `0o700`, which would also block
        // the contained worker from even *traversing* into it to reach
        // `worker-binary` below (root's ns-scoped DAC override doesn't
        // reach this uid's directories either — see `worker_exe`'s comment)
        // — loosen only this one, outer level; `unmapped_owner_dir` below
        // stays `0o700` deliberately, since blocking traversal there,
        // specifically, is the condition under test.
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755))
            .expect("chmod scratch dir traversable");
        let unmapped_owner_dir = dir.path().join("unmapped-owner");
        std::fs::create_dir(&unmapped_owner_dir).expect("create test dir");
        // 0o700: the owner (this unprivileged test process) can read,
        // write and list it; nobody else — including a namespace's mapped
        // "root," once that mapping doesn't cover this uid — has any
        // access at all. Close enough to a real checked-out repo
        // directory's bits for the property under test: real root's
        // blanket CAP_DAC_OVERRIDE vs. a namespaced root's namespace-scoped
        // one.
        std::fs::set_permissions(&unmapped_owner_dir, std::fs::Permissions::from_mode(0o700))
            .expect("chmod test dir");

        // Copied to a scratch path under the system temp dir rather than
        // exec'd from its real build location (`target/debug/deps/…`,
        // under this developer's `$HOME`, mode `0750`): once inside the
        // namespace, real root's traversal into that `$HOME` is subject to
        // the *exact same* unmapped-owner restriction this test exists to
        // prove — `$HOME` is owned by this unprivileged uid too — so
        // `unshare` itself failed to exec the worker with `Permission
        // denied` before this copy was added, a false negative from the
        // test rig rather than the code under test. A world-executable
        // temp dir (`/tmp`, mode `1777`) has no such ancestor-traversal
        // restriction for anyone.
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

    /// Becomes the re-exec target for the test above, run as real root (via
    /// `sudo -n`). Mirrors [`run_reexec_worker`]'s "call the production
    /// function, `unshare` replaces this process, the same function runs
    /// again on the other side" shape, but exercises [`secure_out_file`] +
    /// [`enter_or_refuse_with_scoreboard`] + [`write_scoreboard`] instead
    /// of plain [`enter_or_refuse`].
    fn run_scoreboard_worker() -> ! {
        let dir = std::env::var(SCOREBOARD_TEST_DIR_VAR)
            .expect("worker needs the unmapped-owner test dir path");
        let path = std::path::Path::new(&dir).join("scoreboard.md");

        if is_contained() {
            // Inside the namespace now: prove the by-path open this bug
            // report is about really does fail here...
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
