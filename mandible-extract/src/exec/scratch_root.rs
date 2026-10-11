//! Where per-probe scratch directories live (spec §6 rule 8): always
//! `$HOME/.cache/mandible`, never under a temp directory.
//!
//! `XDG_CACHE_HOME` and `TMPDIR` are deliberately not consulted. Either may
//! name a shared, world-writable directory, and a tool that searches upward
//! from its working directory for project files would then run files planted
//! by another local user.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime};

/// Prefix of every per-probe directory. Short on purpose: a long path is
/// more likely to wrap at `COLUMNS` and escape the mask (spec §6 rule 9).
pub(super) const PROBE_PREFIX: &str = "mnd-";

/// A per-probe directory untouched for this long is a crash leftover.
const STALE_AFTER: Duration = Duration::from_secs(60 * 60);

/// Create (mode 0700) and return `$HOME/.cache/mandible`, removing stale
/// leftovers from earlier crashed runs once per process.
pub(super) fn ensure() -> std::io::Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|h| h.is_absolute())
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "HOME is not set to an absolute path",
            )
        })?;
    let root = home.join(".cache").join("mandible");
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&root)?;

    static SWEPT: OnceLock<()> = OnceLock::new();
    SWEPT.get_or_init(|| remove_stale(&root, SystemTime::now()));
    Ok(root)
}

/// Remove directories directly under `root` that carry [`PROBE_PREFIX`] and
/// were last modified over an hour before `now`. A symlink is never
/// followed or removed; failures are ignored since this is housekeeping.
fn remove_stale(root: &Path, now: SystemTime) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with(PROBE_PREFIX) {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(entry.path()) else {
            continue;
        };
        if !meta.is_dir() {
            continue;
        }
        let age = meta
            .modified()
            .ok()
            .and_then(|m| now.duration_since(m).ok());
        if age.is_some_and(|a| a >= STALE_AFTER) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_old_prefixed_real_directories_are_removed() {
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join("mnd-old");
        let other = root.path().join("keep-me");
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir(&old).unwrap();
        std::fs::create_dir(&other).unwrap();
        std::fs::write(outside.path().join("f"), "x").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), root.path().join("mnd-link")).unwrap();

        remove_stale(root.path(), SystemTime::now());
        assert!(old.exists(), "a fresh directory must stay");

        remove_stale(root.path(), SystemTime::now() + STALE_AFTER * 2);
        assert!(!old.exists(), "a stale probe directory must go");
        assert!(other.exists(), "a foreign name must stay");
        assert!(
            outside.path().join("f").exists(),
            "a symlink target must stay"
        );
    }
}
