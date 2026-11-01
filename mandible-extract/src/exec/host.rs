//! Spawning the helpers a developer task needs (`git`, `tar`, `cargo`, a
//! freshly built replay harness) from `xtask`. Never a tool under audit:
//! nothing here takes a name from `PATH` sweeps, and probing a tool still
//! goes through [`super::run_inert`] only. It lives in this module because
//! this is the one place `std::process` may appear. Compiled only with the
//! `dev-host` feature, which only `xtask` enables.

use std::ffi::OsStr;
use std::io;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The captured result of a finished helper.
#[derive(Debug, Clone)]
pub struct HostOutput {
    /// Exit status was success.
    pub success: bool,
    /// Exit code, when it exited normally.
    pub code: Option<i32>,
    /// Captured stdout.
    pub stdout: Vec<u8>,
    /// Captured stderr.
    pub stderr: Vec<u8>,
}

/// Run `program` with `args` to completion in `cwd` (or the current
/// directory) with `env` added, stdin closed, and capture its output.
pub fn run(
    program: impl AsRef<OsStr>,
    args: &[impl AsRef<OsStr>],
    cwd: Option<&Path>,
    env: &[(&str, &OsStr)],
) -> io::Result<HostOutput> {
    let mut cmd = Command::new(program);
    cmd.args(args).stdin(Stdio::null());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    for (key, value) in env {
        cmd.env(key, value);
    }
    let out = cmd.output()?;
    Ok(HostOutput {
        success: out.status.success(),
        code: out.status.code(),
        stdout: out.stdout,
        stderr: out.stderr,
    })
}

/// How a capped run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capped {
    /// It exited before the cap.
    Exited {
        /// Exit status was success.
        success: bool,
        /// Exit code, when it exited normally.
        code: Option<i32>,
    },
    /// The cap passed and its process group was killed.
    TimedOut,
}

/// Run `program` with `args` in `cwd` as the leader of its own process
/// group, stdout discarded and stderr written to `stderr_to`. When `cap`
/// passes, the whole group is killed.
#[cfg(unix)]
pub fn run_capped(
    program: &Path,
    args: &[&OsStr],
    cwd: &Path,
    stderr_to: &Path,
    cap: Duration,
) -> io::Result<Capped> {
    use std::os::unix::process::CommandExt;
    let stderr = std::fs::File::create(stderr_to)?;
    let mut child = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr)
        .process_group(0)
        .spawn()?;
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Capped::Exited {
                success: status.success(),
                code: status.code(),
            });
        }
        if start.elapsed() >= cap {
            if let Ok(pid) = i32::try_from(child.id()) {
                let _ = nix::sys::signal::killpg(
                    nix::unistd::Pid::from_raw(pid),
                    nix::sys::signal::Signal::SIGKILL,
                );
            }
            let _ = child.kill();
            let _ = child.wait();
            return Ok(Capped::TimedOut);
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn run_captures_output_and_status() {
        let out = run("sh", &["-c", "echo hi; echo err >&2; exit 4"], None, &[]).unwrap();
        assert!(!out.success);
        assert_eq!(out.code, Some(4));
        assert_eq!(out.stdout, b"hi\n");
        assert_eq!(out.stderr, b"err\n");
    }

    #[test]
    fn run_passes_env_and_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let out = run(
            "sh",
            &["-c", "echo $REPLAY_TEST; pwd"],
            Some(dir.path()),
            &[("REPLAY_TEST", OsStr::new("v"))],
        )
        .unwrap();
        let text = String::from_utf8(out.stdout).unwrap();
        assert!(text.starts_with("v\n"), "{text}");
    }

    #[test]
    fn capped_run_reports_exit_and_stderr() {
        let dir = tempfile::tempdir().unwrap();
        let err = dir.path().join("err.txt");
        let got = run_capped(
            Path::new("sh"),
            &[OsStr::new("-c"), OsStr::new("echo boom >&2; exit 3")],
            dir.path(),
            &err,
            Duration::from_secs(20),
        )
        .unwrap();
        assert_eq!(
            got,
            Capped::Exited {
                success: false,
                code: Some(3)
            }
        );
        assert_eq!(std::fs::read_to_string(err).unwrap(), "boom\n");
    }

    #[test]
    fn capped_run_kills_the_group_at_the_cap() {
        let dir = tempfile::tempdir().unwrap();
        let start = Instant::now();
        let got = run_capped(
            Path::new("sh"),
            &[OsStr::new("-c"), OsStr::new("sleep 60 & sleep 60")],
            dir.path(),
            &dir.path().join("e"),
            Duration::from_millis(300),
        )
        .unwrap();
        assert_eq!(got, Capped::TimedOut);
        assert!(start.elapsed() < Duration::from_secs(20));
    }
}
