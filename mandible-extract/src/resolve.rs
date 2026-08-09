//! Resolving a tool name to a binary on `PATH`. This is filesystem-only —
//! no process is spawned to resolve a tool, so this module lives outside
//! `exec/` (spec §6's restriction is specifically about `std::process`).

use std::path::PathBuf;

/// A tool name, resolved (or not) to a binary location on `PATH`. Every
/// [`crate::ExtractionTier`] method takes a `&ResolvedTool` rather than a
/// bare tool name so tiers don't each re-implement `PATH` search.
#[derive(Debug, Clone)]
pub struct ResolvedTool {
    /// The tool name as the user typed it, e.g. `"git"`.
    pub name: String,
    /// The resolved binary path, if found on `PATH`.
    pub path: Option<PathBuf>,
    /// The tool's own reported version string, if previously captured via
    /// an inert `--version`-style probe. `None` until a tier populates it;
    /// used by the cache key (spec §11).
    pub version: Option<String>,
}

/// Resolve `name` against `PATH`, without spawning anything.
pub fn resolve_tool(name: &str) -> ResolvedTool {
    ResolvedTool {
        name: name.to_string(),
        path: find_on_path(name),
        version: None,
    }
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    // A path separator in `name` means the caller already gave us a path;
    // don't search PATH for it, just check it directly.
    //
    // Canonicalized, because a *relative* path is resolved against a
    // different directory here than where it will eventually run: this
    // check uses the process's own CWD, while every probe is spawned with
    // its working directory redirected into a scratch dir (spec §6 rule
    // 8). `mandible ./scripts/tool.py` therefore resolved fine and then
    // failed to spawn with ENOENT, which reads as "the file isn't there"
    // for a file plainly sitting right there.
    if name.contains(std::path::MAIN_SEPARATOR) {
        let candidate = PathBuf::from(name);
        return is_executable(&candidate).then(|| absolute(candidate));
    }
    // `PATH` entries may themselves be relative (`PATH=.:/usr/bin`), so
    // the same treatment applies to what the search finds.
    let path_var = std::env::var_os("PATH")?;
    std::env::split_paths(&path_var)
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
        .map(absolute)
}

/// Make `path` absolute **without resolving symlinks**, falling back to it
/// unchanged if the filesystem won't say. A still-relative path is no worse
/// than what the caller gave us.
///
/// `std::path::absolute` rather than `std::fs::canonicalize`, and the
/// difference is not cosmetic — canonicalize follows symlinks, which broke
/// two things at once when it was used here, both caught by the PATH-wide
/// coverage sweep and neither by any test:
///
/// - **It defeated spec §6 rule 0.** `is_help_only_probe` matches on the file
///   *name*, and `reboot`, `poweroff`, `shutdown` and `telinit` are all
///   symlinks to `systemctl`. Canonicalizing renamed them before that
///   check ran, so the never-probe list stopped refusing them and the
///   scoreboard showed them going from `no-tier` (correctly refused) to
///   `ok` (probed). That is the rule that exists because `mandible pkill`
///   reset a user's machine.
/// - **It broke multi-call binaries.** `iptables` and its nine siblings
///   are symlinks to `xtables-nft-multi`, which dispatches on `argv[0]`;
///   invoked under its real name it no longer knows which tool it is, and
///   all ten degraded from `ok` to `verbatim`.
///
/// Absoluteness alone is what the caller needs: the path is resolved
/// against this process's CWD but spawned from a scratch directory.
/// Resolving symlinks was never part of that requirement.
fn absolute(path: PathBuf) -> PathBuf {
    std::path::absolute(&path).unwrap_or(path)
}

#[cfg(unix)]
fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &std::path::Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_a_real_binary() {
        // `sh` is present on every POSIX system this test would run on.
        let resolved = resolve_tool("sh");
        assert!(resolved.path.is_some());
    }

    #[test]
    fn unknown_tool_resolves_to_none() {
        let resolved = resolve_tool("definitely-not-a-real-tool-xyz123");
        assert!(resolved.path.is_none());
    }

    /// A resolved path must be absolute, because it is resolved here
    /// against this process's CWD but spawned from a scratch directory
    /// (spec §6 rule 8). `mandible ./scripts/tool.py` used to resolve and
    /// then fail with "No such file or directory" for a file plainly
    /// present.
    #[test]
    fn a_relative_path_resolves_to_an_absolute_one() {
        // Created *inside* the current directory so the path below is
        // genuinely relative, without calling `set_current_dir` — that is
        // process-global, and the test harness runs these in parallel
        // threads, so mutating it would race every other test.
        let dir = tempfile::TempDir::new_in(".").unwrap();
        let script = dir.path().join("toolish");
        std::fs::write(&script, "#!/bin/sh\necho hi\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        // `TempDir::new_in(".")` hands back an absolute path, so rebuild
        // the relative spelling from its final component.
        let dir_name = dir
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let relative = format!(
            ".{sep}{dir_name}{sep}toolish",
            sep = std::path::MAIN_SEPARATOR
        );
        assert!(
            !std::path::Path::new(&relative).is_absolute(),
            "fixture must be relative to be worth testing: {relative}"
        );

        let path = resolve_tool(&relative)
            .path
            .expect("relative path should resolve");
        assert!(path.is_absolute(), "still relative: {}", path.display());
        assert!(path.ends_with("toolish"), "{}", path.display());
    }

    /// Making the path absolute must not follow symlinks, because the
    /// never-probe refusal (spec §6 rule 0) matches on the file *name*.
    ///
    /// `reboot`, `poweroff`, `shutdown` and `telinit` are symlinks to
    /// `systemctl`; resolving them renames the tool before that check
    /// runs, and the whole never-probe list stops refusing anything that
    /// reaches it by a link. A PATH-wide coverage sweep caught this —
    /// those four went from correctly refused to probed — and no unit
    /// test did, so here is the unit test.
    ///
    /// The same resolution also breaks multi-call binaries that dispatch
    /// on `argv[0]` (`iptables` and nine siblings are links to
    /// `xtables-nft-multi`).
    #[cfg(unix)]
    #[test]
    fn resolving_does_not_follow_symlinks() {
        let dir = tempfile::TempDir::new_in(".").unwrap();
        let real = dir.path().join("systemctl-ish");
        std::fs::write(&real, "#!/bin/sh\nexit 0\n").unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let link = dir.path().join("reboot-ish");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let resolved = resolve_tool(&link.to_string_lossy()).path.unwrap();
        assert_eq!(
            resolved.file_name().unwrap(),
            "reboot-ish",
            "symlink was resolved, so a never-probe name would be lost: {}",
            resolved.display()
        );
    }
}
