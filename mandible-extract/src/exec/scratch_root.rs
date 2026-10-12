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
    #[cfg(unix)]
    secure_root(&root)?;

    static SWEPT: OnceLock<()> = OnceLock::new();
    SWEPT.get_or_init(|| remove_stale(&root, SystemTime::now()));
    Ok(root)
}

/// Make an existing root private. `create` only applies its mode to a
/// directory it makes, so a folder from an older run keeps whatever mode it
/// had; a group member could then plant files above every probe. The root is
/// opened without following a symlink and judged by the descriptor, so the
/// check and the `chmod` act on the same object.
#[cfg(unix)]
fn secure_root(root: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
    let dir = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
        .open(root)
        .map_err(|e| {
            std::io::Error::new(
                e.kind(),
                format!(
                    "scratch root {} is not a real directory: {e}",
                    root.display()
                ),
            )
        })?;
    let meta = dir.metadata()?;
    check_owner(meta.uid(), nix::unistd::geteuid().as_raw()).map_err(|m| {
        std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("{} {m}", root.display()),
        )
    })?;
    if meta.mode() & 0o777 != 0o700 {
        dir.set_permissions(std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// The scratch root must belong to the user running the probe.
#[cfg(unix)]
fn check_owner(owner: u32, me: u32) -> Result<(), &'static str> {
    if owner == me {
        Ok(())
    } else {
        Err("is owned by another user")
    }
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

    #[cfg(unix)]
    fn mode_of(p: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    #[cfg(unix)]
    #[test]
    fn an_existing_loose_root_is_tightened_to_0700() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        std::fs::set_permissions(t.path(), std::fs::Permissions::from_mode(0o775)).unwrap();
        secure_root(t.path()).unwrap();
        assert_eq!(mode_of(t.path()), 0o700);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_root_is_refused_and_its_target_left_alone() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let target = t.path().join("target");
        std::fs::create_dir(&target).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
        let link = t.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(secure_root(&link).is_err());
        assert_eq!(mode_of(&target), 0o755, "the target must not be chmodded");
    }

    #[cfg(unix)]
    #[test]
    fn a_regular_file_root_is_refused() {
        let t = tempfile::NamedTempFile::new().unwrap();
        assert!(secure_root(t.path()).is_err());
    }

    /// Creating a directory owned by someone else needs root, so the
    /// ownership rule is a pure function tested directly.
    #[cfg(unix)]
    #[test]
    fn a_foreign_owner_is_refused() {
        assert!(check_owner(1000, 1000).is_ok());
        assert!(check_owner(0, 1000).is_err());
    }
}
