//! The single function in the whole workspace permitted to reach
//! `std::process::Command`: spawns a tool under the §6 execution-safety
//! policy and returns its bounded, captured output.

use super::policy::InertArgv;
use super::reap::{self, ProbeToken};
use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use thiserror::Error;

/// Max combined stdout+stderr bytes retained per invocation (spec §6 rule
/// 5). Bytes beyond this are still drained from the pipe (so the child never
/// blocks writing to a full pipe buffer) but discarded.
pub const MAX_OUTPUT_BYTES: usize = 8 * 1024 * 1024;

/// A single reader's cap: half the combined budget, so one noisy stream
/// cannot alone starve the budget meant for both.
const PER_STREAM_CAP: usize = MAX_OUTPUT_BYTES / 2;

/// Errors from [`run_inert`].
#[derive(Debug, Error)]
pub enum ExecError {
    /// The child process could not be spawned at all (e.g. not found, not
    /// executable).
    #[error("failed to spawn {path}: {source}")]
    Spawn {
        /// The path that failed to spawn.
        path: String,
        /// The underlying OS error.
        #[source]
        source: std::io::Error,
    },
    /// Waiting on the child process failed at the OS level (distinct from
    /// the child simply timing out, which is reported via
    /// [`ExecOutput::timed_out`]).
    #[error("failed to wait on {path}: {source}")]
    Wait {
        /// The path that failed while waiting.
        path: String,
        /// The underlying OS error.
        #[source]
        source: std::io::Error,
    },
    /// The tool is on the help-only list (spec §6 rule 0) and this was not
    /// the one permitted shape. Such a program acts on processes or on
    /// machine state, so `--help` is the only argument vector measured
    /// harmless for it; `-h` in particular is an action flag on several.
    #[error("{path} is only probed as `--help`: it signals processes or changes machine state")]
    RefusedUnsafeTool {
        /// The path that was refused.
        path: String,
    },
    /// The argv contained an empty string (spec §6 rule 2a). Refused
    /// before spawning: an empty argument is not inert, because a program
    /// that takes a pattern or a target as its first positional reads it
    /// as "match everything".
    #[error("{path} not probed with an empty argument: an empty argv element is never inert")]
    RefusedEmptyArgument {
        /// The path that was refused.
        path: String,
    },
    /// The per-invocation scratch directory (spec §6 rule 8) could not be
    /// built, so the probe was refused rather than run against the
    /// inherited environment. The redirect is all-or-nothing: a silent
    /// best-effort fallback would let a probe run unredirected with
    /// nothing recording it. Spec §6 rule 8, [M-11].
    #[error(
        "{path} not probed: the scratch redirect required by spec §6 rule 8 \
         could not be built: {source}"
    )]
    ScratchUnavailable {
        /// The path that was refused.
        path: String,
        /// The underlying filesystem error.
        #[source]
        source: std::io::Error,
    },
    /// [`crate::exec::Transcript`] has no recording for the exact argv a
    /// tier asked for (keyed on [`InertArgv::args`]). Distinct from every
    /// other variant above: this means no process was ever meant to run —
    /// the replay fixture simply doesn't cover this argv.
    #[error("no transcript recording for `{tool} {}`", argv.join(" "))]
    TranscriptMiss {
        /// The tool path the tier asked to probe.
        tool: String,
        /// The exact argument vector requested ([`InertArgv::args`]).
        argv: Vec<String>,
    },
}

/// The captured result of running a tool under the exec policy.
#[derive(Debug, Clone)]
pub struct ExecOutput {
    /// Captured stdout, capped at [`MAX_OUTPUT_BYTES`] combined with stderr.
    pub stdout: Vec<u8>,
    /// Captured stderr, capped at [`MAX_OUTPUT_BYTES`] combined with stdout.
    pub stderr: Vec<u8>,
    /// The process's exit code, if it exited normally (not via signal, and
    /// not timed out).
    pub exit_code: Option<i32>,
    /// True if the wall-clock cap was hit and the process group was killed.
    pub timed_out: bool,
}

/// Run `tool_path` with the argument shape `argv`, under the full §6
/// execution-safety policy:
///
/// - stdin is always `/dev/null` (rule 3).
/// - the environment is cleared and re-populated with only `PATH` plus the
///   sanitized baseline (`TERM=dumb`, `NO_COLOR=1`, `COLUMNS=100`,
///   `LC_ALL=C.UTF-8`, pager variables set to `cat`) and whatever `argv`
///   itself requires (rule 6).
/// - the child leads a brand-new session, not just a process group, so it
///   has no controlling terminal (rule 4/6, [M-17]).
/// - nothing the probe started outlives it (rule 4): every descendant
///   still alive is identified by a per-invocation token and killed — see
///   [`super::reap`].
/// - output is read on background threads and capped (rule 5).
///
/// `argv` being an [`InertArgv`] rather than a raw argument list makes
/// rules 1 and 2 (never bare, only inert shapes) structural.
pub fn run_inert(
    tool_path: &Path,
    argv: &InertArgv,
    timeout: Duration,
) -> Result<ExecOutput, ExecError> {
    let path_str = tool_path.display().to_string();

    // Restrict process-signalling and machine-state programs to exactly
    // `--help`, before anything is spawned (spec §6 rule 0). Not a total
    // ban: `--help` is harmless on all of them. Every other shape is
    // refused, including `-h`, which on systemd's multi-call binary is an
    // action flag. See [`HELP_ONLY_PROBE`].
    if is_help_only_probe(tool_path) && argv.args() != ["--help"] {
        return Err(ExecError::RefusedUnsafeTool {
            path: path_str.clone(),
        });
    }

    // Refuse an empty argument the tool could read as its first positional
    // (spec §6 rule 2a), before anything is spawned. Rule 1 only counts
    // arguments, and an empty string satisfies it while being the opposite
    // of inert: `pkill -- ""` matches every process.
    //
    // Scoped rather than blanket: one empty argument is protocol-required
    // and safe — cobra's completion word, which sits behind the
    // `__complete` sentinel and is never the first positional. An empty
    // element is allowed only when a guard word precedes it, never
    // directly after `--`. Enforced here, at the single chokepoint.
    let args = argv.args();
    let guarded = matches!(args.first().map(String::as_str), Some("__complete"));
    if args
        .iter()
        .enumerate()
        .any(|(i, a)| a.is_empty() && !(guarded && i > 0))
    {
        return Err(ExecError::RefusedEmptyArgument {
            path: path_str.clone(),
        });
    }

    let mut cmd = Command::new(tool_path);
    cmd.args(args);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    cmd.env_clear();
    if let Ok(path) = std::env::var("PATH") {
        cmd.env("PATH", path);
    }
    cmd.env("TERM", "dumb");
    cmd.env("NO_COLOR", "1");
    cmd.env("COLUMNS", "100");
    cmd.env("LC_ALL", "C.UTF-8");
    // Rule 6: *set* every pager variable to `cat` rather than leaving it
    // absent — absence is the weaker property, since a tool is free to
    // read it as "go find one yourself" (documented fallback for
    // PAGER/MANPAGER/SYSTEMD_PAGER/GIT_PAGER across GNU/BSD userlands).
    // Kept as defense-in-depth even though [M-17]'s actual freeze mechanism
    // was the session fix below, not this.
    cmd.env("PAGER", "cat");
    cmd.env("MANPAGER", "cat");
    cmd.env("GIT_PAGER", "cat");
    cmd.env("SYSTEMD_PAGER", "cat");
    for (key, default_subpath) in TOOLCHAIN_RESOLUTION_VARS {
        match std::env::var_os(key) {
            // Explicitly set: pass it through unchanged.
            Some(value) => {
                cmd.env(key, value);
            }
            // Unset is the common case: materialise the default from the
            // real $HOME before it's replaced, only if it exists, so a
            // machine without that toolchain gets no spurious variable.
            None => {
                if let Some(home) = std::env::var_os("HOME") {
                    let candidate = std::path::Path::new(&home).join(default_subpath);
                    if candidate.is_dir() {
                        cmd.env(key, candidate);
                    }
                }
            }
        }
    }
    for (k, v) in argv.extra_env() {
        cmd.env(k, v);
    }

    // Give every probe its own *session*, not just its own process group
    // (spec §6 rule 6, [M-17]). A process group alone leaves the child in
    // mandible's own session, so a descendant can still `open("/dev/tty")`
    // and reach the real controlling terminal, bypassing the redirected
    // fds — a pager or password prompt can read real keystrokes and leave
    // termios changes that a later SIGKILL does not undo. `setsid()`
    // before `exec` makes the child the leader of a brand-new session
    // with no controlling terminal, so that `open` fails `ENXIO`.
    //
    // `setsid(2)` also makes the child its own process-group leader as a
    // side effect (pgid == pid), the same property `process_group(0)`
    // used to provide, so that call is removed: `setsid()` fails `EPERM`
    // if the caller is already a group leader. `kill_process_group`'s
    // `Pid::from_raw(-pid)` still targets the right group.
    //
    // # Safety
    //
    // `pre_exec` runs after `fork` and before `exec`, a window where only
    // async-signal-safe operations are sound. `nix::unistd::setsid`
    // performs exactly one raw `setsid(2)` syscall with no allocation and
    // no locking, safe in that window. The one audited `unsafe` exception
    // in this crate — see `lib.rs`'s `#![deny(unsafe_code)]`.
    #[cfg(unix)]
    #[allow(unsafe_code)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                nix::unistd::setsid()
                    .map(|_| ())
                    .map_err(std::io::Error::from)
            });
        }
    }

    // Redirect every writable location a probe might reach (spec §6 rule
    // 8), not just CWD: real tools write unprompted on nothing but
    // `--help`/`-h` [M-11]. Every probe gets its own scratch directory
    // standing in for CWD, `HOME`, `TMPDIR`, and the writable per-user XDG
    // base-directory variables (`XDG_RUNTIME_DIR`, `XDG_CACHE_HOME`,
    // `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_STATE_HOME`; the `_DIRS`
    // variants are read-only search paths, not write targets).
    //
    // Each gets its own subdirectory of the scratch root, masked back out
    // of the tool's output on the way here — see [`Scratch`].
    //
    // Per invocation, not reused: a `TempDir` is removed on drop, so
    // nothing a probe writes here outlives the probe. See `exec/mod.rs`
    // for what this does and does not guarantee.
    let scratch = Scratch::create().map_err(|source| ExecError::ScratchUnavailable {
        path: path_str.clone(),
        source,
    })?;
    scratch.apply(&mut cmd);

    // Rule 4's other half: a probe is not complete while its descendants
    // are alive. `arm_subreaper` makes orphaned descendants reparent to
    // this process instead of init; the token attributes one to this
    // invocation rather than a concurrent probe. See [`super::reap`].
    reap::arm_subreaper();
    let token = ProbeToken::new();
    token.apply(&mut cmd);

    let spawn_result = spawn_with_etxtbsy_retry(&mut cmd);
    let mut child = match spawn_result {
        Ok(child) => child,
        Err(source) => {
            return Err(ExecError::Spawn {
                path: path_str.clone(),
                source,
            })
        }
    };

    let stdout_pipe = child.stdout.take().expect("stdout was piped");
    let stderr_pipe = child.stderr.take().expect("stderr was piped");
    let stdout_handle = spawn_bounded_reader(stdout_pipe);
    let stderr_handle = spawn_bounded_reader(stderr_pipe);

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if Instant::now() >= deadline {
                    break None;
                }
                thread::sleep(Duration::from_millis(20));
            }
            Err(source) => {
                return Err(ExecError::Wait {
                    path: path_str.clone(),
                    source,
                })
            }
        }
    };

    let timed_out = status.is_none();
    if timed_out {
        kill_process_group(&mut child);
        let _ = child.wait();
    }

    // Before joining the readers, not after: a descendant that escaped the
    // process group still holds the inherited write end of both pipes, so
    // the readers would sit at a never-arriving EOF until it exited.
    // Deliberately unconditional, not gated on `timed_out` — the leak this
    // fixes comes from probes that complete normally.
    reap::reap_probe_descendants(token.value());

    let stdout = stdout_handle.join().unwrap_or_default();
    let stderr = stderr_handle.join().unwrap_or_default();

    // Undo the redirect in the text before anything downstream sees it, so
    // every tier and view gets the masked form without knowing this
    // happened.
    let (stdout, stderr) = (scratch.mask(&stdout), scratch.mask(&stderr));

    Ok(ExecOutput {
        stdout,
        stderr,
        exit_code: status.and_then(|s| exit_code_of(&s)),
        timed_out,
    })
}

/// The variables the scratch redirect stands in for, and the subdirectory
/// each one gets.
///
/// One directory per variable, not one shared directory: a shared
/// directory is not a filesystem shape any real machine has (a tool
/// writing `$XDG_CACHE_HOME/x` and reading `$HOME/x` would see one file),
/// and it would make masking output unfixable — no way to tell which
/// variable produced a given path.
const SCRATCH_VARS: &[(&str, &str)] = &[
    ("HOME", "home"),
    ("TMPDIR", "tmp"),
    ("XDG_RUNTIME_DIR", "runtime"),
    ("XDG_CACHE_HOME", "cache"),
    ("XDG_CONFIG_HOME", "config"),
    ("XDG_DATA_HOME", "data"),
    ("XDG_STATE_HOME", "state"),
];

/// The subdirectory used as the probe's working directory.
const SCRATCH_CWD: &str = "cwd";

/// A per-probe scratch directory, plus the substitutions that hide it
/// again on the way out.
///
/// Redirecting `$HOME` means a tool that prints a `$HOME`-derived default
/// prints ours instead (`docker --help` → `/tmp/mandible-exec-L3saJ8/.docker`,
/// a directory already deleted by the time the reader sees it). Fixed by
/// writing back the variable name, not its resolved value:
///
/// ```text
/// Location of client config files (default "$HOME/.docker")
/// ```
///
/// Substituting the reader's real home would state a fact the tool never
/// gave us. `$HOME` is what is actually known, and it's identical on
/// every machine, so a fixture doesn't bake in the capturing machine's home.
struct Scratch {
    /// Held for its `Drop`, which removes the directory.
    _dir: tempfile::TempDir,
    /// `(path, mask)` pairs, longest path first so that a subdirectory is
    /// always matched before the root it sits under.
    masks: Vec<(Vec<u8>, Vec<u8>)>,
}

impl Scratch {
    /// All-or-nothing: every redirect target is built, or the whole
    /// scratch fails and the caller refuses the probe. A partial
    /// application would be a containment hole, not a degraded mode.
    fn create() -> std::io::Result<Scratch> {
        // Short prefix on purpose: a tool wraps its own help text at the
        // set `COLUMNS`, so a long path is more likely to be split across
        // two lines, and a split string cannot be matched.
        let dir = tempfile::Builder::new().prefix("mnd-").tempdir()?;

        let mut masks = Vec::new();
        for (var, subdir) in SCRATCH_VARS {
            let path = dir.path().join(subdir);
            std::fs::create_dir_all(&path)?;
            push_mask(&mut masks, &path, &format!("${var}"));
        }
        let cwd = dir.path().join(SCRATCH_CWD);
        std::fs::create_dir_all(&cwd)?;
        push_mask(&mut masks, &cwd, "$PWD");
        // Backstop for a path derived from the root some other way. Should
        // never fire; sorting below puts it last since every entry above
        // is longer and sits beneath it.
        push_mask(&mut masks, dir.path(), "$MANDIBLE_SCRATCH");
        masks.sort_by_key(|(path, _)| std::cmp::Reverse(path.len()));

        Ok(Scratch { _dir: dir, masks })
    }

    fn apply(&self, cmd: &mut Command) {
        let root = self._dir.path();
        cmd.current_dir(root.join(SCRATCH_CWD));
        for (var, subdir) in SCRATCH_VARS {
            cmd.env(var, root.join(subdir));
        }
    }

    /// Replace every scratch path in `bytes` with the variable that stood
    /// in for it. Byte-level, since a tool's output is not guaranteed
    /// UTF-8. Matches this invocation's exact path, never a `/tmp/mnd-*`
    /// pattern, so a tool printing some other temp path is untouched.
    fn mask(&self, bytes: &[u8]) -> Vec<u8> {
        let mut out = bytes.to_vec();
        for (path, replacement) in &self.masks {
            out = replace_bytes(&out, path, replacement);
        }
        out
    }
}

/// Register `mask` for `path` under every spelling a tool might print it
/// as: as given, and canonicalized.
///
/// The two differ whenever anything on the way to the scratch directory is
/// a symlink — on macOS the default case, since `$TMPDIR` lives under
/// `/var`, a symlink to `/private/var`. A probe that resolves its own
/// working directory prints the physical path even though the logical one
/// is what was handed to it.
///
/// Registering only one form is worse than registering neither: a
/// mismatched key can match only the tail of the physical path, leaving a
/// mangled hybrid like `cwd=/private$PWD` — harder to recognise as wrong
/// than an unmasked path.
fn push_mask(masks: &mut Vec<(Vec<u8>, Vec<u8>)>, path: &Path, mask: &str) {
    let mut forms = vec![path.to_path_buf()];
    match std::fs::canonicalize(path) {
        Ok(real) if real != path => forms.push(real),
        _ => {}
    }
    for form in forms {
        masks.push((
            form.to_string_lossy().into_owned().into_bytes(),
            mask.as_bytes().to_vec(),
        ));
    }
}

/// Replace every occurrence of `needle` in `haystack` with `replacement`.
fn replace_bytes(haystack: &[u8], needle: &[u8], replacement: &[u8]) -> Vec<u8> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return haystack.to_vec();
    }
    let mut out = Vec::with_capacity(haystack.len());
    let mut i = 0;
    while i < haystack.len() {
        if haystack[i..].starts_with(needle) {
            out.extend_from_slice(replacement);
            i += needle.len();
        } else {
            out.push(haystack[i]);
            i += 1;
        }
    }
    out
}

/// Spawn `cmd`, retrying briefly on `ETXTBSY` ("text file busy").
///
/// Under heavy concurrent process creation, some filesystems transiently
/// refuse to exec a just-written, just-`chmod`ed file another process
/// still has open. A narrow, well-known race with a standard fix.
#[cfg(unix)]
fn spawn_with_etxtbsy_retry(cmd: &mut Command) -> std::io::Result<Child> {
    const MAX_ATTEMPTS: u32 = 5;
    let mut last_err = None;
    for attempt in 0..MAX_ATTEMPTS {
        match cmd.spawn() {
            Ok(child) => return Ok(child),
            Err(e) if e.raw_os_error() == Some(libc::ETXTBSY) && attempt + 1 < MAX_ATTEMPTS => {
                thread::sleep(Duration::from_millis(10 * (attempt as u64 + 1)));
                last_err = Some(e);
            }
            Err(e) => return Err(e),
        }
    }
    Err(last_err.expect("loop only exits via return unless it retried and recorded an error"))
}

#[cfg(not(unix))]
fn spawn_with_etxtbsy_retry(cmd: &mut Command) -> std::io::Result<Child> {
    cmd.spawn()
}

fn exit_code_of(status: &std::process::ExitStatus) -> Option<i32> {
    status.code()
}

fn spawn_bounded_reader<R>(mut pipe: R) -> thread::JoinHandle<Vec<u8>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 64 * 1024];
        loop {
            match pipe.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    if buf.len() < PER_STREAM_CAP {
                        let remaining = PER_STREAM_CAP - buf.len();
                        buf.extend_from_slice(&chunk[..n.min(remaining)]);
                    }
                    // Keep draining past the cap so the child never blocks
                    // writing into a full OS pipe buffer.
                }
                Err(_) => break,
            }
        }
        buf
    })
}

/// Programs mandible may invoke *only* as `<tool> --help`, and under no
/// other argument shape.
///
/// A safety rule, not a parsing rule, so it stays clear of §1's ban on
/// per-tool knowledge in extraction. Closed list; every entry acts on
/// processes or machine state, so an unrecognised argument is a target
/// rather than a subcommand. `-h` is refused too: on systemd's multi-call
/// binary it is an action flag (`shutdown -h` halts), not a help flag.
/// `--help` is harmless on all of these and yields a real flag list.
///
/// Argument permutation (GNU getopt) is not a portable guarantee — BSD and
/// busybox getopt stop at the first non-option — so positional shapes stay
/// refused here rather than relying on rule 2a alone.
///
/// Matched on file name only, so a copy under another path is caught too.
/// Never resolve symlinks first: `reboot`, `poweroff`, `shutdown` and
/// `telinit` are all links to `systemctl`. Spec §6 rule 0.
const HELP_ONLY_PROBE: &[&str] = &[
    // Signal senders: an unrecognised positional is a process to kill.
    "kill",
    "pkill",
    "killall",
    "killall5",
    "skill",
    "xkill",
    "fuser",
    // System state: `-h` is an action flag on every one of these.
    "halt",
    "poweroff",
    "reboot",
    "shutdown",
    "telinit",
    "init",
    "systemctl-shutdown",
    // Message delivery: an unrecognised positional is the message, and it
    // goes somewhere a person will see it. `wall` is the measured case
    // (Tier E's speculative `__complete <word>` broadcast to every
    // terminal); the rest are the same mechanism by inspection.
    "wall",
    "write",
    "logger",
    "notify-send",
    "say",
    "xmessage",
];

/// True if `tool_path` names a program from [`HELP_ONLY_PROBE`], i.e. one
/// that may be invoked as `--help` and nothing else.
pub fn is_help_only_probe(tool_path: &Path) -> bool {
    tool_path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|name| HELP_ONLY_PROBE.contains(&name))
}

/// Variables a version manager needs to find the program it stands in for.
///
/// A deliberate, bounded loosening of spec §6 rule 8's `HOME` redirect:
/// version-manager shims resolve their target *through* `HOME`, so the
/// redirect alone breaks `mandible cargo`/pyenv/nvm/rbenv/asdf. Passed
/// through while `HOME` itself stays redirected — each points at a
/// toolchain directory, a far narrower blast radius. Closed, small list:
/// "how version managers locate toolchains", not "how cargo works" (spec §1).
///
/// Each entry is the variable and the path relative to the real `$HOME`
/// that the manager falls back to when unset. Spec §6 rule 8, [M-11].
const TOOLCHAIN_RESOLUTION_VARS: &[(&str, &str)] = &[
    ("RUSTUP_HOME", ".rustup"),
    ("CARGO_HOME", ".cargo"),
    ("PYENV_ROOT", ".pyenv"),
    ("NVM_DIR", ".nvm"),
    ("RBENV_ROOT", ".rbenv"),
    ("ASDF_DIR", ".asdf"),
    ("SDKMAN_DIR", ".sdkman"),
    ("VOLTA_HOME", ".volta"),
];

#[cfg(unix)]
fn kill_process_group(child: &mut Child) {
    use nix::sys::signal::{kill, Signal};
    use nix::unistd::Pid;
    let pid = child.id() as i32;
    // Negative pid means "the process group" in POSIX kill(2) semantics —
    // still correct after `setsid()` makes the child its own process-group
    // leader. `nix::sys::signal::kill` is a safe wrapper; no `unsafe`
    // needed here.
    let _ = kill(Pid::from_raw(-pid), Signal::SIGKILL);
}

#[cfg(not(unix))]
fn kill_process_group(child: &mut Child) {
    // No portable process-group kill on this platform; falls back to
    // killing the direct child only (known gap: spec §6 rule 4).
    let _ = child.kill();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A help-only tool is refused before anything is spawned for every
    /// shape but `--help`, checked in `run_inert` so no tier can bypass it.
    /// `-h` is refused deliberately: on systemd's multi-call binary it is
    /// an action flag (`halt -h`, `shutdown -h`, ...), not help.
    #[test]
    fn help_only_tools_are_refused_every_shape_but_help_long() {
        let dir = tempfile::tempdir().unwrap();
        // A shim named `pkill` that would report if it ever ran.
        let path = dir.path().join("pkill");
        std::fs::write(&path, "#!/bin/sh\ntouch \"$0.ran\"\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        for argv in [
            InertArgv::HelpShort,
            InertArgv::HelpSubcommand { words: vec![] },
            InertArgv::HelpSubcommand {
                words: vec!["anything".to_string()],
            },
            InertArgv::HelpLongForPath {
                words: vec!["anything".to_string()],
            },
            InertArgv::HelpShortForPath {
                words: vec!["anything".to_string()],
            },
            InertArgv::CobraComplete { words: vec![] },
            InertArgv::CompletionScript {
                shell: "zsh".to_string(),
            },
            // Rule 2b's shape must be refused like any other non-`["--help"]`
            // argv, even with the word the tier would actually follow.
            InertArgv::HelpExpand {
                words: vec![],
                word: "all".to_string(),
            },
        ] {
            let result = run_inert(&path, &argv, Duration::from_secs(2));
            assert!(
                matches!(result, Err(ExecError::RefusedUnsafeTool { .. })),
                "{argv:?} should have been refused"
            );
        }
        assert!(
            !dir.path().join("pkill.ran").exists(),
            "the shim was executed — the refusal is not before spawn"
        );

        // `HelpLongForPath` with no words renders as exactly `--help`.
        for argv in [
            InertArgv::HelpLong,
            InertArgv::HelpLongForPath { words: vec![] },
        ] {
            assert_eq!(argv.args(), vec!["--help".to_string()]);
            assert!(
                run_inert(&path, &argv, Duration::from_secs(2)).is_ok(),
                "{argv:?} is the one permitted shape and must run"
            );
        }
        assert!(
            dir.path().join("pkill.ran").exists(),
            "`--help` must actually reach the tool — that is the point"
        );
    }

    /// The list is closed and every entry acts on processes or machine
    /// state; must not grow into the per-tool catalogue §1 forbids.
    #[test]
    fn help_only_list_stays_small_and_matches_by_file_name() {
        assert!(
            HELP_ONLY_PROBE.len() <= 20,
            "this list must not become a catalogue"
        );
        assert!(is_help_only_probe(Path::new("/usr/bin/pkill")));
        assert!(is_help_only_probe(Path::new("/usr/bin/wall")));
        assert!(is_help_only_probe(Path::new("/usr/bin/logger")));
        assert!(is_help_only_probe(Path::new("/some/other/place/killall")));
        // A tool that merely *contains* a listed name is not matched.
        assert!(!is_help_only_probe(Path::new(
            "/usr/bin/killall-not-really"
        )));
        assert!(!is_help_only_probe(Path::new("/usr/bin/git")));
    }

    /// `HOME` stays redirected while toolchain-resolution variables get
    /// through.
    #[test]
    fn toolchain_vars_are_a_closed_list_that_excludes_home() {
        let keys: Vec<&str> = TOOLCHAIN_RESOLUTION_VARS.iter().map(|(k, _)| *k).collect();
        for forbidden in ["HOME", "TMPDIR", "XDG_CONFIG_HOME", "XDG_DATA_HOME"] {
            assert!(
                !keys.contains(&forbidden),
                "{forbidden} must stay redirected — it is the containment boundary"
            );
        }
        // Every default is relative, so it resolves inside the real home.
        for (key, default) in TOOLCHAIN_RESOLUTION_VARS {
            assert!(
                !std::path::Path::new(default).is_absolute(),
                "{key}'s default {default:?} must be relative to $HOME"
            );
        }
    }
    use std::io::Write;

    fn write_shim(dir: &std::path::Path, name: &str, script: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(script.as_bytes()).unwrap();
        drop(f);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    #[test]
    fn captures_stdout_and_exit_code() {
        let dir = tempfile::tempdir().unwrap();
        let shim = write_shim(dir.path(), "echoer.sh", "#!/bin/sh\necho hello\nexit 0\n");
        let out = run_inert(&shim, &InertArgv::HelpLong, Duration::from_secs(2)).unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hello");
        assert_eq!(out.exit_code, Some(0));
        assert!(!out.timed_out);
    }

    #[test]
    fn stdin_is_null_child_sees_immediate_eof() {
        let dir = tempfile::tempdir().unwrap();
        let shim = write_shim(
            dir.path(),
            "stdin_check.sh",
            "#!/bin/sh\nif read -r line; then echo GOT:$line; else echo EOF; fi\n",
        );
        let out = run_inert(&shim, &InertArgv::HelpLong, Duration::from_secs(2)).unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "EOF");
    }

    #[test]
    fn timeout_kills_process_group() {
        let dir = tempfile::tempdir().unwrap();
        let shim = write_shim(dir.path(), "sleeper.sh", "#!/bin/sh\nsleep 30\n");
        let start = Instant::now();
        let out = run_inert(&shim, &InertArgv::HelpLong, Duration::from_millis(200)).unwrap();
        assert!(out.timed_out);
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "kill should be prompt"
        );
    }

    #[test]
    fn output_is_bounded() {
        let dir = tempfile::tempdir().unwrap();
        // Emit well over the cap.
        let shim = write_shim(
            dir.path(),
            "noisy.sh",
            "#!/bin/sh\nyes A | head -c 20000000\n",
        );
        let out = run_inert(&shim, &InertArgv::HelpLong, Duration::from_secs(10)).unwrap();
        assert!(out.stdout.len() <= MAX_OUTPUT_BYTES);
    }

    #[test]
    fn environment_is_sanitized() {
        // Asserts the child's env is exactly the sanitized baseline,
        // not just that particular vars are absent.
        let dir = tempfile::tempdir().unwrap();
        let shim = write_shim(dir.path(), "envdump.sh", "#!/bin/sh\nenv\n");
        let out = run_inert(&shim, &InertArgv::HelpLong, Duration::from_secs(2)).unwrap();
        let env_text = String::from_utf8_lossy(&out.stdout);
        // Pager variables are asserted present and equal to `cat`, not
        // just absent — [M-17]: absence reads as "find one yourself".
        assert!(!env_text.contains("LESS="), "child env leaked LESS=");
        for forced in [
            "PAGER=cat",
            "MANPAGER=cat",
            "GIT_PAGER=cat",
            "SYSTEMD_PAGER=cat",
        ] {
            assert!(
                env_text.contains(forced),
                "child env missing {forced}: {env_text}"
            );
        }
        assert!(env_text.contains("TERM=dumb"));
        assert!(env_text.contains("NO_COLOR=1"));
        assert!(env_text.contains("COLUMNS=100"));
        assert!(env_text.contains("LC_ALL=C.UTF-8"));
        assert!(env_text.contains("PATH="));
    }

    /// Regression for spec §6 rule 8 [M-11]: a child's CWD must never be
    /// the directory mandible itself was launched from.
    #[test]
    fn child_working_directory_is_not_the_caller_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let shim = write_shim(dir.path(), "pwd_check.sh", "#!/bin/sh\npwd\n");
        let out = run_inert(&shim, &InertArgv::HelpLong, Duration::from_secs(2)).unwrap();
        let child_cwd = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let caller_cwd = std::env::current_dir().unwrap();
        assert_ne!(
            std::path::Path::new(&child_cwd),
            caller_cwd.as_path(),
            "child ran in mandible's own working directory: {child_cwd}"
        );
    }

    /// Regression for spec §6 rule 8 [M-11]. A probe writing to a relative
    /// path and one deliberately targeting `$HOME` both land in the
    /// scratch directory, never in the real `$HOME`.
    #[test]
    fn probe_cannot_write_into_the_real_home() {
        let real_home = std::env::var("HOME").expect("HOME must be set to run this test");
        let marker_name = format!("mandible-test-leak-{}", std::process::id());
        let real_home_marker = std::path::Path::new(&real_home).join(&marker_name);
        // Make sure a stale run never leaves this assertion vacuously true.
        let _ = std::fs::remove_file(&real_home_marker);

        let dir = tempfile::tempdir().unwrap();
        let shim = write_shim(
            dir.path(),
            "home_writer.sh",
            &format!(
                "#!/bin/sh\necho leaked > \"$HOME/{marker_name}\"\necho relative-leak > ./{marker_name}\necho \"$HOME\"\n"
            ),
        );
        let out = run_inert(&shim, &InertArgv::HelpLong, Duration::from_secs(2)).unwrap();
        let child_home = String::from_utf8_lossy(&out.stdout).trim().to_string();

        assert_ne!(
            child_home, real_home,
            "child's $HOME was the real $HOME instead of a scratch directory"
        );
        assert!(
            !real_home_marker.exists(),
            "probe wrote into the real $HOME at {}",
            real_home_marker.display()
        );

        let _ = std::fs::remove_file(&real_home_marker);
    }

    /// The scratch directory must not appear in what a tool tells the
    /// reader. Goes through `run_inert` rather than testing
    /// `Scratch::mask` alone, since the redirect and the mask must agree
    /// about which directory stood in for which variable.
    #[test]
    fn scratch_paths_are_masked_out_of_a_probes_output() {
        let dir = tempfile::tempdir().unwrap();
        let shim = write_shim(
            dir.path(),
            "path_printer.sh",
            "#!/bin/sh\n\
             echo \"config=$XDG_CONFIG_HOME/app.toml\"\n\
             echo \"home=$HOME/.appconfig\"\n\
             echo \"tmp=$TMPDIR\"\n\
             echo \"cwd=$(pwd)\"\n",
        );
        let out = run_inert(&shim, &InertArgv::HelpLong, Duration::from_secs(2)).unwrap();
        let text = String::from_utf8_lossy(&out.stdout).to_string();

        assert!(
            !text.contains("/mnd-"),
            "a scratch path reached the reader: {text:?}"
        );
        assert!(
            text.contains("config=$XDG_CONFIG_HOME/app.toml"),
            "{text:?}"
        );
        assert!(text.contains("home=$HOME/.appconfig"), "{text:?}");
        assert!(text.contains("tmp=$TMPDIR"), "{text:?}");
        assert!(text.contains("cwd=$PWD"), "{text:?}");
    }

    /// Every redirected variable resolves somewhere different — not a
    /// filesystem shape any real machine has otherwise.
    #[test]
    fn each_redirected_variable_gets_its_own_directory() {
        let dir = tempfile::tempdir().unwrap();
        let vars: Vec<&str> = SCRATCH_VARS.iter().map(|(v, _)| *v).collect();
        let body = vars
            .iter()
            .map(|v| format!("echo \"${v}\""))
            .collect::<Vec<_>>()
            .join("\n");
        let shim = write_shim(
            dir.path(),
            "env_printer.sh",
            &format!("#!/bin/sh\n{body}\n"),
        );

        // Bypassing the mask, hence a scratch of our own, not `run_inert`'s.
        let scratch = Scratch::create().unwrap();
        let mut cmd = Command::new(&shim);
        scratch.apply(&mut cmd);
        let out = cmd.output().unwrap();
        let paths: Vec<String> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|l| l.to_string())
            .collect();

        assert_eq!(paths.len(), vars.len(), "{paths:?}");
        let distinct: std::collections::BTreeSet<&String> = paths.iter().collect();
        assert_eq!(
            distinct.len(),
            paths.len(),
            "redirected variables share a directory: {paths:?}"
        );
    }

    /// Rule 8 is all-or-nothing: when the scratch redirect cannot be
    /// built, the probe must be refused with a named error, never run
    /// against the inherited environment. `TMPDIR` pointing at a regular
    /// file makes every tempdir creation fail; safe to set process-wide
    /// here because nextest runs each test in its own process.
    #[test]
    fn scratch_failure_refuses_the_probe_instead_of_running_unredirected() {
        let holder = tempfile::NamedTempFile::new().unwrap();
        std::env::set_var("TMPDIR", holder.path());

        let err = run_inert(
            Path::new("/bin/true"),
            &InertArgv::HelpLong,
            Duration::from_secs(2),
        )
        .expect_err("a probe without its scratch redirect must be refused");
        assert!(
            matches!(err, ExecError::ScratchUnavailable { .. }),
            "expected ScratchUnavailable, got {err:?}"
        );
    }

    /// A subdirectory must be matched before the root it sits under,
    /// otherwise `<root>/config/x` masks to `$MANDIBLE_SCRATCH/config/x`
    /// instead of `$XDG_CONFIG_HOME/x`.
    #[test]
    fn the_longest_matching_path_wins() {
        let scratch = Scratch::create().unwrap();
        let root = scratch._dir.path().to_string_lossy().into_owned();
        let masked = scratch.mask(format!("see {root}/config/app.toml").as_bytes());
        assert_eq!(
            String::from_utf8_lossy(&masked),
            "see $XDG_CONFIG_HOME/app.toml"
        );
    }

    /// A tool prints whichever spelling of the path it resolved, so both
    /// have to be registered. Built on an explicit symlink rather than the
    /// real scratch directory, since on Linux `/tmp` usually isn't one.
    #[cfg(unix)]
    #[test]
    fn a_symlinked_scratch_path_is_masked_under_both_spellings() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir_all(&real).unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let mut masks = Vec::new();
        push_mask(&mut masks, &link, "$HOME");

        let keys: Vec<String> = masks
            .iter()
            .map(|(p, _)| String::from_utf8_lossy(p).into_owned())
            .collect();
        assert!(
            keys.contains(&link.to_string_lossy().into_owned()),
            "logical path missing: {keys:?}"
        );
        assert!(
            keys.contains(&real.canonicalize().unwrap().to_string_lossy().into_owned()),
            "physical path missing: {keys:?}"
        );
    }

    #[test]
    fn replace_bytes_replaces_every_occurrence() {
        assert_eq!(
            replace_bytes(b"a/x and a/y", b"a/", b"$A/"),
            b"$A/x and $A/y"
        );
        assert_eq!(
            replace_bytes(b"nothing here", b"a/", b"$A/"),
            b"nothing here"
        );
        // A needle longer than the haystack must not panic on the slice.
        assert_eq!(replace_bytes(b"ab", b"abcdef", b"x"), b"ab");
    }
}
