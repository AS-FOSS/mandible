//! Shared by the integration tests and, through a `#[path]` include in
//! `lib.rs`, by the in-crate tests, so every test shim is written one way.
//!
//! On macOS the first exec of a freshly written file pays a one-off scan
//! (about 225 ms, against 4 ms afterwards) and the scans queue across
//! processes, so under parallel nextest a test's 2 s probe budget could be
//! spent before its script started. `write_shim` pays that cost up front,
//! outside any timed window. Shim scripts record side effects on every run
//! and tests assert on them, so the warm-up run exits at a guard line
//! inserted after the shebang, before the script body.
#![allow(dead_code)]

use std::io::Write;
use std::path::{Path, PathBuf};

const SHEBANG: &str = "#!/bin/sh\n";
const WARM_VAR: &str = "MANDIBLE_TEST_SHIM_WARMUP";

/// Write `script` as an executable file `dir/name`, flush it to disk and
/// run it once so the platform's first-exec cost is already paid.
pub fn write_shim(dir: &Path, name: &str, script: &str) -> PathBuf {
    let path = dir.join(name);
    let guarded = match script.strip_prefix(SHEBANG) {
        Some(body) => format!("{SHEBANG}[ -n \"${WARM_VAR}\" ] && exit 0\n{body}"),
        None => script.to_string(),
    };
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(guarded.as_bytes()).unwrap();
    f.sync_all().unwrap();
    drop(f);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    if guarded.len() != script.len() {
        warm(&path);
    }
    path
}

fn warm(path: &Path) {
    // A sibling thread forking while the file was open can make the first
    // exec fail with ETXTBSY; retry briefly.
    for attempt in 0..20u64 {
        let spawned = std::process::Command::new(path)
            .env(WARM_VAR, "1")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        if spawned.is_ok() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10 * (attempt + 1)));
    }
}
