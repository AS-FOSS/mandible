//! Running one harness against one tool under a wall-clock cap.

use mandible_extract::exec::host::{run_capped, Capped};
use serde_json::Value;
use std::ffi::OsStr;
use std::path::Path;
use std::time::Duration;

/// Per-tool cap for one ref.
pub const TOOL_CAP: Duration = Duration::from_secs(120);

/// What one harness run produced.
pub enum Outcome {
    /// The raw dump (`{"root": ...}`).
    Dump(Value),
    /// The harness failed; a short reason.
    Error(String),
    /// The cap was hit and the harness group was killed.
    Timeout,
}

/// Run `bin <tool> <out>` in `work` (created and removed here) and collect
/// its dump. The harness leads its own process group, killed on timeout.
pub fn run_harness(bin: &Path, tool: &str, work: &Path, cap: Duration) -> Outcome {
    let outcome = run_inner(bin, tool, work, cap);
    let _ = std::fs::remove_dir_all(work);
    outcome
}

fn run_inner(bin: &Path, tool: &str, work: &Path, cap: Duration) -> Outcome {
    if let Err(e) = std::fs::create_dir_all(work) {
        return Outcome::Error(format!("creating work dir: {e}"));
    }
    let out_path = work.join("dump.json");
    let err_path = work.join("stderr.txt");
    let args = [OsStr::new(tool), out_path.as_os_str()];
    match run_capped(bin, &args, work, &err_path, cap) {
        Err(e) => Outcome::Error(format!("running harness: {e}")),
        Ok(Capped::TimedOut) => Outcome::Timeout,
        Ok(Capped::Exited {
            success: false,
            code,
        }) => Outcome::Error(format!(
            "harness exited {}: {}",
            code.map_or("by signal".to_string(), |c| c.to_string()),
            last_line(&err_path)
        )),
        Ok(Capped::Exited { success: true, .. }) => read_dump(&out_path),
    }
}

fn read_dump(path: &Path) -> Outcome {
    match std::fs::read(path)
        .map_err(|e| e.to_string())
        .and_then(|b| serde_json::from_slice(&b).map_err(|e| e.to_string()))
    {
        Ok(v) => Outcome::Dump(v),
        Err(e) => Outcome::Error(format!("reading dump: {e}")),
    }
}

fn last_line(path: &Path) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let line = text.lines().rev().map(str::trim).find(|l| !l.is_empty());
    line.unwrap_or("no stderr").chars().take(160).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Instant;

    fn script(dir: &Path, body: &str) -> std::path::PathBuf {
        let path = dir.join("fake-harness");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn a_dump_is_read_back() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = script(tmp.path(), "echo '{\"root\":null}' > \"$2\"");
        let work = tmp.path().join("w1");
        match run_harness(&bin, "x", &work, Duration::from_secs(20)) {
            Outcome::Dump(v) => assert!(v["root"].is_null()),
            _ => panic!("expected a dump"),
        }
        assert!(!work.exists(), "the work dir is removed");
    }

    #[test]
    fn a_failing_harness_reports_its_last_stderr_line() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = script(tmp.path(), "echo first >&2; echo boom >&2; exit 3");
        match run_harness(&bin, "x", &tmp.path().join("w2"), Duration::from_secs(20)) {
            Outcome::Error(e) => assert!(e.contains("boom") && e.contains('3'), "{e}"),
            _ => panic!("expected an error"),
        }
    }

    #[test]
    fn a_hung_harness_is_killed_at_the_cap() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = script(tmp.path(), "sleep 60");
        let start = Instant::now();
        let out = run_harness(
            &bin,
            "x",
            &tmp.path().join("w3"),
            Duration::from_millis(300),
        );
        assert!(matches!(out, Outcome::Timeout));
        assert!(start.elapsed() < Duration::from_secs(20));
    }
}
