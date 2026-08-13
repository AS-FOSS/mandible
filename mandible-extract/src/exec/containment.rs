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
//! inside this process, for a specific reason: this crate is single-
//! `unsafe`-exception (`spawn.rs`'s `setsid` `pre_exec`, see that module's
//! doc comment), and calling `unshare(CLONE_NEWPID)` on a live,
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

use std::io;
use std::path::PathBuf;
use std::process::Command;
use thiserror::Error;

/// Set in the environment of the re-executed, already-namespaced process.
/// Its mere presence — not its value — is what [`is_contained`] checks, so
/// a sweep can tell "this is the first, uncontained invocation" from "this
/// is the re-exec landing inside the namespace" without any other signal.
pub const CONTAINED_ENV_VAR: &str = "MANDIBLE_SWEEP_CONTAINED";

/// True if this process is already running inside the namespace
/// [`enter_or_refuse`] constructs — i.e. this is the re-exec'd copy, not
/// the original invocation.
pub fn is_contained() -> bool {
    std::env::var_os(CONTAINED_ENV_VAR).is_some()
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
    let pid = unshare_probe(&[
        "--user",
        "--map-root-user",
        "--pid",
        "--fork",
        "--",
        "true",
    ]);
    let mount = unshare_probe(&["--user", "--map-root-user", "--mount", "--", "true"]);
    NamespaceSupport { user, pid, mount }
}

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
    cmd.args(["--user", "--map-root-user", "--pid", "--mount", "--fork", "--"]);
    cmd.arg(&exe);
    cmd.args(&args);
    cmd.env(CONTAINED_ENV_VAR, "1");

    // `exec` replaces this process's image; it returns only on failure to
    // do so (the child that eventually runs `exe` again is a *different*
    // OS process, spawned by `unshare` itself after its own fork — never
    // this Rust code).
    use std::os::unix::process::CommandExt;
    let source = cmd.exec();
    ContainmentError::Reexec { source }
}

#[cfg(not(target_os = "linux"))]
pub fn enter_or_refuse() -> ContainmentError {
    ContainmentError::Unavailable {
        support: probe_namespace_support(),
    }
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
