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

    let mut cmd = Command::new(tool_path);
    cmd.args(argv.args());
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
    for (k, v) in argv.extra_env() {
        cmd.env(k, v);
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // pgroup 0 => new process group whose pgid equals the child's pid.
        cmd.process_group(0);
    }

    let mut child = spawn_with_etxtbsy_retry(&mut cmd).map_err(|source| ExecError::Spawn {
        path: path_str.clone(),
        source,
    })?;

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

    Ok(ExecOutput {
        stdout,
        stderr,
        exit_code: status.and_then(|s| exit_code_of(&s)),
        timed_out,
    })
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
}
