//! The single function in the whole workspace permitted to reach
//! `std::process::Command`: spawns a tool under the §6 execution-safety
//! policy and returns its bounded, captured output.

use super::policy::InertArgv;
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
///   `LC_ALL=C.UTF-8`) and whatever `argv` itself requires (rule 6).
/// - the child is placed in its own process group, so a timeout kills the
///   whole group, not just the direct child (rule 4).
/// - output is read on background threads and capped (rule 5), so neither
///   a pipe deadlock nor unbounded memory growth is possible.
///
/// `argv` being an [`InertArgv`] rather than a raw argument list is what
/// makes rules 1 and 2 (never bare, only inert shapes) structural rather
/// than just documented.
pub fn run_inert(
    tool_path: &Path,
    argv: &InertArgv,
    timeout: Duration,
) -> Result<ExecOutput, ExecError> {
    let path_str = tool_path.display().to_string();

    // Restrict process-signalling and machine-state programs to exactly
    // `--help`, before anything is spawned (spec §6 rule 0). Not a total
    // ban: `--help` is measured harmless on all of them and is where their
    // flag list lives. Every other shape is refused — including `-h`,
    // which on systemd's multi-call binary is an *action* flag
    // (`shutdown -h` is halt) and was measured attempting the real
    // operation. See [`HELP_ONLY_PROBE`].
    if is_help_only_probe(tool_path) && argv.args() != ["--help"] {
        return Err(ExecError::RefusedUnsafeTool {
            path: path_str.clone(),
        });
    }

    // Refuse an empty argument the tool could read as its first positional
    // (spec §6 rule 2a), before anything is spawned. Rule 1 ("never a bare
    // invocation") only counts arguments, and an empty string satisfies it
    // while being the opposite of inert: `pkill -- ""` passes the
    // universally-understood option terminator and then an empty pattern,
    // which matches every process. Measured terminating every process in a
    // PID namespace, pkill included (rc=143) — the mechanism behind the
    // machine reset that motivated rule 0. The never-probe list masked it
    // for thirteen tools while the same shape was still emitted at the
    // rest of PATH.
    //
    // Scoped rather than blanket, because one empty argument is protocol-
    // required and provably safe: cobra's completion word. It is never the
    // first positional — it sits behind the `__complete` sentinel, which a
    // non-cobra tool rejects rather than acts on. So the rule is that an
    // empty element is allowed only when a guard word precedes it, and
    // never directly after `--`, which every getopt program discards.
    // Enforced here, at the single chokepoint, so no tier can reintroduce
    // it by constructing an argv elsewhere.
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
    for (key, default_subpath) in TOOLCHAIN_RESOLUTION_VARS {
        match std::env::var_os(key) {
            // Explicitly set: pass it through unchanged.
            Some(value) => {
                cmd.env(key, value);
            }
            // Unset is the *common* case — version managers fall back to a
            // documented path under the real `$HOME`, which is precisely
            // what the sandbox redirects. Passing the variable through
            // alone therefore fixes nothing; the default has to be
            // materialised from the real home before it is replaced. Only
            // when the directory actually exists, so a machine without
            // that toolchain gets no spurious variable.
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

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // pgroup 0 => new process group whose pgid equals the child's pid.
        cmd.process_group(0);
    }

    // Redirect every writable location a probe might reach (spec §6 rule
    // 8), not just CWD. Rule 7 ("never write") is about arguments *we*
    // pass — it doesn't cover a tool's own unprompted behavior, and real
    // tools have plenty: running the coverage harness (spec §13.1)
    // against ~1600 real system binaries with nothing but `--help`/`-h`
    // surfaced font-cache builders writing `fonts.dir`/`fonts.scale` into
    // the invoking CWD, and `mysql_secure_installation` writing a
    // `.my.cnf.<pid>` containing an empty root password [M-11]. So every
    // probe gets its own scratch directory standing in for CWD, `HOME`,
    // `TMPDIR`, and every standard XDG base-directory variable (the
    // writable per-user ones — `XDG_RUNTIME_DIR`, `XDG_CACHE_HOME`,
    // `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_STATE_HOME`; the `_DIRS`
    // variants are read-only system search paths, not somewhere a probe
    // would write) — see this module's parent (`exec/mod.rs`) doc comment
    // for the full story on what's now verified vs. still a residual risk.
    //
    // Each of those gets its own *subdirectory* of the scratch root, and
    // the paths are masked back out of the tool's output on the way here.
    // See [`Scratch`] for both, and for the defect that prompted them.
    //
    // Deliberately *per invocation*, not created once and reused for the
    // process's lifetime: a `TempDir` is removed on drop, so nothing a
    // probe writes here outlives the probe, and one tool's mess can never
    // be mistaken for another's input. Best-effort — if a scratch
    // directory can't be created, the probe still runs (falling back to
    // the inherited environment) rather than failing over containment.
    //
    // This is a general policy applied to every probe uniformly, never a
    // per-tool exclusion list (spec §1) — and it is still not a complete
    // guarantee. Full containment needs OS-level sandboxing (namespaces/
    // seccomp); a tool that constructs a write path some other way (an
    // absolute path baked into itself, rather than derived from any of
    // these variables) is outside what an environment/CWD redirect can
    // reach. That residual is documented, not silently assumed away — see
    // this module's top-level doc comment.
    let scratch = Scratch::create();
    if let Some(scratch) = &scratch {
        scratch.apply(&mut cmd);
    }

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

    let stdout = stdout_handle.join().unwrap_or_default();
    let stderr = stderr_handle.join().unwrap_or_default();

    // Undo the redirect *in the text* before anything downstream sees it.
    // Done here, at the boundary that applied the redirect, so every tier,
    // every grammar, `--doctor` and the verbatim view (`t`) all get the
    // masked form without knowing this happened.
    let (stdout, stderr) = match &scratch {
        Some(scratch) => (scratch.mask(&stdout), scratch.mask(&stderr)),
        None => (stdout, stderr),
    };

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
/// **One directory per variable, not one shared directory.** They used to
/// all point at the same path, which was wrong twice over. It is not a
/// filesystem shape any real machine has — a tool writing
/// `$XDG_CACHE_HOME/x` and reading `$HOME/x` saw one file — so every probe
/// ran against an environment that cannot occur. And it made the leak
/// below unfixable: given `/tmp/…/​.docker` in a tool's output there was no
/// way to tell which variable produced it, so there was nothing correct to
/// put back.
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
/// prints *ours*: `docker --help` reported its config location as
/// `/tmp/mandible-exec-L3saJ8/.docker`, a directory that was deleted
/// moments later and never existed for the reader. That is documentation
/// stating something false, with nothing on screen to suggest it — the
/// exact failure this project refuses everywhere else, arrived at through
/// the safety mechanism rather than through a parser.
///
/// The fix is to write back the *variable name*, not its real value:
///
/// ```text
/// Location of client config files (default "$HOME/.docker")
/// ```
///
/// Substituting the reader's actual home directory would state a fact the
/// tool never gave us — filling in a blank is the same move as inventing
/// structure, just smaller. `$HOME` is what is actually known, it is how
/// man pages write it anyway, and it is identical on every machine, so a
/// fixture captured from a real tool does not bake in the capturing
/// machine's home directory.
struct Scratch {
    /// Held for its `Drop`, which removes the directory.
    _dir: tempfile::TempDir,
    /// `(path, mask)` pairs, longest path first so that a subdirectory is
    /// always matched before the root it sits under.
    masks: Vec<(Vec<u8>, Vec<u8>)>,
}

impl Scratch {
    /// Best-effort: a probe still runs without a scratch directory
    /// (falling back to the inherited environment) rather than failing
    /// over containment.
    fn create() -> Option<Scratch> {
        // Short prefix on purpose. A tool wraps its own help text at the
        // `COLUMNS` we set, so a long path is more likely to be split
        // across two lines, and a split string cannot be matched. Keeping
        // it short reduces how often that happens; it cannot rule it out.
        let dir = tempfile::Builder::new().prefix("mnd-").tempdir().ok()?;

        let mut masks = Vec::new();
        for (var, subdir) in SCRATCH_VARS {
            let path = dir.path().join(subdir);
            if std::fs::create_dir_all(&path).is_err() {
                continue;
            }
            push_mask(&mut masks, &path, &format!("${var}"));
        }
        let cwd = dir.path().join(SCRATCH_CWD);
        if std::fs::create_dir_all(&cwd).is_ok() {
            push_mask(&mut masks, &cwd, "$PWD");
        }
        // Backstop for a path derived from the root some other way (a tool
        // printing the parent of `$TMPDIR`, say). It should never fire,
        // and if it does, something visibly ours beats a real-looking path
        // to a directory that no longer exists. Sorting below puts it last,
        // since every entry above is longer and sits beneath it.
        push_mask(&mut masks, dir.path(), "$MANDIBLE_SCRATCH");
        masks.sort_by_key(|(path, _)| std::cmp::Reverse(path.len()));

        Some(Scratch { _dir: dir, masks })
    }

    fn apply(&self, cmd: &mut Command) {
        let root = self._dir.path();
        cmd.current_dir(root.join(SCRATCH_CWD));
        for (var, subdir) in SCRATCH_VARS {
            cmd.env(var, root.join(subdir));
        }
    }

    /// Replace every scratch path in `bytes` with the variable that stood
    /// in for it.
    ///
    /// Byte-level rather than `str`, because a tool's output is not
    /// guaranteed to be UTF-8 and this runs before any lossy conversion.
    /// Matching is on the exact path of *this* invocation, never a
    /// `/tmp/mnd-*` pattern: the path is unique per probe, so a tool that
    /// legitimately prints some other temp path is untouched.
    fn mask(&self, bytes: &[u8]) -> Vec<u8> {
        let mut out = bytes.to_vec();
        for (path, replacement) in &self.masks {
            out = replace_bytes(&out, path, replacement);
        }
        out
    }
}

/// Register `mask` for `path` under **every spelling a tool might print it
/// as** — as given, and canonicalized.
///
/// The two differ whenever anything on the way to the scratch directory is
/// a symlink, and on macOS that is the default case, not an edge one:
/// `$TMPDIR` lives under `/var`, which is a symlink to `/private/var`. A
/// probe that resolves its own working directory (anything calling
/// `getcwd`, `realpath`, or `pwd` with no `PWD` in the environment — and
/// we clear the environment) therefore prints the physical path, while the
/// logical one is what we handed it.
///
/// Registering only one form is worse than registering neither. CI caught
/// exactly that: the logical key matched the tail of the physical path and
/// left the head behind, so the output read `cwd=/private$PWD` — a mangled
/// hybrid that is harder to recognise as wrong than an un-masked path
/// would have been.
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
/// Under heavy concurrent process creation (many tests, or many tiers
/// probing in parallel in production), some filesystems transiently refuse
/// to exec a just-written, just-`chmod`ed file while another process still
/// has it open — the kernel's writer-vs-executor exclusion is momentarily
/// stale. This is not a correctness issue with the exec policy; it is a
/// narrow, well-known race with a standard fix (brief retry), so it's
/// handled here rather than left to intermittently fail callers.
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
/// **This is a safety rule, not a parsing rule**, and the distinction is
/// what keeps it clear of §1. §1 forbids per-tool knowledge in *extraction*
/// — "if a tool renders badly, fix the general parser" — because that kind
/// of list grows without bound and rots. This list is about what is safe to
/// *run at all*, it is closed, and every entry shares one property: the
/// program acts on processes or on machine state, so an argument it does
/// not recognise is a target rather than a subcommand.
///
/// It began as a total ban, after a user reported `mandible pkill` freezing
/// their machine badly enough to require a reset. Two later measurements
/// reshaped it into this narrower rule.
///
/// **What the ban was blamed on was false.** The claim was that
/// `<tool> <word> --help` makes `killall foo --help` kill everything named
/// `foo`. On glibc, GNU getopt permutes, so `--help` wins wherever it sits:
/// `pkill --help`, `pkill victim --help` and `killall victim --help` were
/// all measured killing nothing. The reset's real mechanism was an empty
/// argument (`pkill -- ""`), now refused for every tool by rule 2a above.
///
/// **What it was silently protecting against is real, and was never
/// written down: `-h` is not a help flag here.** Measured against systemd's
/// multi-call binary, with the machine saved only by polkit because the
/// probe ran unprivileged:
///
/// ```text
/// halt -h      → Call to Halt failed: Interactive authentication required
/// poweroff -h  → Call to PowerOff failed: Interactive authentication required
/// reboot -h    → Call to Reboot failed: Interactive authentication required
/// shutdown -h  → Failed to schedule shutdown: Interactive authentication required
/// ```
///
/// Those are not parse errors — each tool *attempted the action* (`-h` is
/// `shutdown -h now`'s halt). mandible falls back to `-h` whenever `--help`
/// fails, so as root, or wherever polkit is permissive, the fallback alone
/// would halt the machine.
///
/// Hence the shape of the rule: `--help` is measured harmless on all of
/// these and yields a real flag list (`pkill --help` parses to 27 flags,
/// all described), while every other shape — `-h`, `help <word>`,
/// `<word> --help`, `completion <shell>`, `__complete` — is refused. That
/// keeps the flag list on offer and removes the two hazards that were
/// actually measured, rather than trading the first for the second.
///
/// The remaining reason to keep positional shapes off these specific tools,
/// rather than relying on rule 2a alone: argument permutation is a glibc
/// behaviour, not a guarantee (BSD and busybox getopt stop at the first
/// non-option), and the background tree warmer would reach any subcommand a
/// future parser change starts emitting, unasked. The general form of that
/// problem — a fabricated word becoming argv for *any* tool — belongs to
/// provenance-gating positional probes, not to this list.
///
/// Matched on the file name only, so a copy under another path is caught
/// too. Never resolve symlinks before this check: `reboot`, `poweroff`,
/// `shutdown` and `telinit` are all links to `systemctl`.
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
/// A deliberate, bounded loosening of spec §6 rule 8, which redirects
/// `HOME` so a probe can never write into the user's real one ([M-11]:
/// `mysql_secure_installation --help` was measured writing a `.my.cnf`
/// containing an empty root password). That redirect also breaks every
/// version-manager shim, because they resolve their target *through*
/// `HOME`: `mandible cargo` showed "rustup could not choose a version of
/// cargo to run" rather than cargo's help, and the same applies to
/// pyenv, nvm, rbenv, asdf and mise. A whole class of developer tooling
/// was unusable.
///
/// These are passed through while `HOME` itself stays redirected. Each
/// points at a *toolchain* directory, not the user's home, so a probe that
/// misbehaves against one has a far narrower blast radius than `$HOME` —
/// and the ones that matter are read-only lookups in practice. The list is
/// closed and small, which keeps it on the right side of the project's
/// no-per-tool-knowledge rule: the knowledge here is "how version managers
/// locate toolchains", not "how cargo works".
/// Each entry is the variable and the path *relative to the real `$HOME`*
/// that the manager falls back to when it is unset — which is the usual
/// state, since almost nobody sets these by hand.
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
    // Negative pid means "the process group" in POSIX kill(2) semantics.
    // `nix::sys::signal::kill` is a safe wrapper, so this crate's
    // `#![forbid(unsafe_code)]` holds even though the underlying syscall
    // is unsafe FFI inside `nix`.
    let _ = kill(Pid::from_raw(-pid), Signal::SIGKILL);
}

#[cfg(not(unix))]
fn kill_process_group(child: &mut Child) {
    // No portable process-group kill on this platform; fall back to
    // killing the direct child only. Completion-script helper processes
    // may leak on non-Unix as a result — tracked as a known gap (spec §6
    // rule 4 is written for process groups, which are a POSIX concept).
    let _ = child.kill();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A help-only tool is refused *before* anything is spawned for every
    /// shape but `--help`, and the check lives in `run_inert`, which every
    /// tier goes through, so no tier can reach one by another route.
    ///
    /// `-h` is in the refused set deliberately, and it is the case that
    /// matters most: on systemd's multi-call binary `-h` is an action flag,
    /// and `halt -h` / `poweroff -h` / `reboot -h` / `shutdown -h` were all
    /// measured *attempting the real operation*, stopped only by polkit
    /// because the probe ran unprivileged. mandible falls back to `-h`
    /// whenever `--help` fails, so this refusal is what stands between that
    /// fallback and a machine that reboots itself.
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

        // `HelpLongForPath` with no words renders as exactly `--help`, so it
        // is the same permitted shape and must not be refused for spelling.
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
    /// state. It is a *safety* rule, not a parsing one, and must not grow
    /// into the per-tool catalogue §1 forbids.
    #[test]
    fn help_only_list_stays_small_and_matches_by_file_name() {
        assert!(
            HELP_ONLY_PROBE.len() <= 20,
            "this list must not become a catalogue"
        );
        assert!(is_help_only_probe(Path::new("/usr/bin/pkill")));
        assert!(is_help_only_probe(Path::new("/some/other/place/killall")));
        // A tool that merely *contains* a listed name is not matched.
        assert!(!is_help_only_probe(Path::new(
            "/usr/bin/killall-not-really"
        )));
        assert!(!is_help_only_probe(Path::new("/usr/bin/git")));
    }

    /// `HOME` stays redirected while toolchain-resolution variables get
    /// through. Both halves matter: the redirect is what stops a probe
    /// writing into the user's home ([M-11]), and the pass-through is what
    /// makes version-manager shims resolvable at all.
    #[test]
    fn toolchain_vars_are_a_closed_list_that_excludes_home() {
        let keys: Vec<&str> = TOOLCHAIN_RESOLUTION_VARS.iter().map(|(k, _)| *k).collect();
        for forbidden in ["HOME", "TMPDIR", "XDG_CONFIG_HOME", "XDG_DATA_HOME"] {
            assert!(
                !keys.contains(&forbidden),
                "{forbidden} must stay redirected — it is the containment boundary"
            );
        }
        // Every default is relative, so it can only ever resolve *inside*
        // the real home rather than at an absolute path somewhere else.
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
        // Not mutating process-wide env vars here (this test binary runs
        // tests in parallel, and std::env::set_var is process-global) —
        // instead this asserts the child's env is *exactly* the sanitized
        // baseline, which is a stronger property than "these particular
        // vars are absent" and doesn't depend on ambient test-runner state.
        let dir = tempfile::tempdir().unwrap();
        let shim = write_shim(dir.path(), "envdump.sh", "#!/bin/sh\nenv\n");
        let out = run_inert(&shim, &InertArgv::HelpLong, Duration::from_secs(2)).unwrap();
        let env_text = String::from_utf8_lossy(&out.stdout);
        for forbidden in ["LESS=", "PAGER=", "MANPAGER=", "GIT_PAGER="] {
            assert!(
                !env_text.contains(forbidden),
                "child env leaked {forbidden}"
            );
        }
        assert!(env_text.contains("TERM=dumb"));
        assert!(env_text.contains("NO_COLOR=1"));
        assert!(env_text.contains("COLUMNS=100"));
        assert!(env_text.contains("LC_ALL=C.UTF-8"));
        assert!(env_text.contains("PATH="));
    }

    /// Regression for a real finding from the coverage harness (spec
    /// §13.1): some real tools write files to their working directory as
    /// a side effect of being run at all — a font-cache builder created
    /// `fonts.dir`/`fonts.scale`, and something MySQL-related created
    /// `.mysql.<pid>` — even though `--help`/`-h` is the only argv shape
    /// ever passed. A child's CWD must never be the directory mandible
    /// itself was launched from.
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

    /// Regression for spec §6 rule 8 [M-11]: `--help` is not reliably
    /// read-only. The real finding was `mysql_secure_installation` writing
    /// a `.my.cnf.<pid>` with an empty root password — via a plain
    /// *relative* path (`config=".my.cnf.$$"` in the actual script, no
    /// `$HOME` involved), which the CWD redirect alone already stops. This
    /// test proves the broader claim directly and portably (no dependency
    /// on that specific binary being installed): a probe that writes to a
    /// relative path *and* one that deliberately targets `$HOME` both land
    /// in the scratch directory, never in the real `$HOME` this test
    /// process actually has.
    #[test]
    fn probe_cannot_write_into_the_real_home() {
        let real_home = std::env::var("HOME").expect("HOME must be set to run this test");
        let marker_name = format!("mandible-test-leak-{}", std::process::id());
        let real_home_marker = std::path::Path::new(&real_home).join(&marker_name);
        // Belt-and-braces: make sure a stale run never leaves this
        // assertion vacuously true.
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
    /// reader.
    ///
    /// `docker --help` reported its config location as
    /// `/tmp/mandible-exec-L3saJ8/.docker` — a directory deleted moments
    /// later that never existed for the reader. Nothing on screen marked
    /// it as anything other than docker's own documentation.
    ///
    /// Goes through `run_inert` rather than testing `Scratch::mask` alone,
    /// because the claim is about the boundary as a whole: the redirect
    /// and the mask have to agree about which directory stood in for
    /// which variable, and only running a real process checks that.
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
        // Each variable is named as itself, not as whichever one happened
        // to share its directory.
        assert!(
            text.contains("config=$XDG_CONFIG_HOME/app.toml"),
            "{text:?}"
        );
        assert!(text.contains("home=$HOME/.appconfig"), "{text:?}");
        assert!(text.contains("tmp=$TMPDIR"), "{text:?}");
        assert!(text.contains("cwd=$PWD"), "{text:?}");
    }

    /// Every redirected variable resolves somewhere different. They all
    /// pointed at one directory, so a probe writing `$XDG_CACHE_HOME/x`
    /// and reading `$HOME/x` saw the same file — a filesystem shape no
    /// real machine has, which every probe was nonetheless run against.
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

        // Read the raw paths, which means bypassing the mask — hence a
        // scratch of our own rather than `run_inert`'s.
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

    /// A tool prints whichever spelling of the path it happened to
    /// resolve, so both have to be registered.
    ///
    /// Built on an explicit symlink rather than on the real scratch
    /// directory, because on Linux `/tmp` usually is not one and the test
    /// would assert nothing. macOS gets this for free — `$TMPDIR` sits
    /// under `/var`, a symlink to `/private/var` — which is where CI found
    /// it, having passed locally.
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
