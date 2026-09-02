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

/// One child command discovered by the `<parent>-<sub>` convention: a file
/// on `PATH` named after the parent tool plus a dash (spec §5.4). `cargo`'s
/// `cargo-clippy` and `git`'s `git-lfs` are the two specimens; the rule is
/// keyed on the naming convention alone and knows nothing about either tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathSibling {
    /// The subcommand name — the part after the dash, e.g. `"clippy"`.
    pub name: String,
    /// The binary's own name, e.g. `"cargo-clippy"` — what a probe of this
    /// node is actually sent to (spec §6), and what the UI names as the
    /// evidence behind an unverified node (spec §9.2).
    pub binary: String,
}

/// Upper bound on how many convention-discovered children one parent gets.
/// A backstop against a `<parent>-*`-heavy directory (git's own `libexec`
/// layout has ~150) handing the background warmer that many extra probes,
/// not a tuning knob.
const MAX_PATH_SIBLINGS: usize = 64;

/// Every `<parent>-<sub>` executable on `PATH`, as [`PathSibling`]s, in
/// alphabetical order by subcommand name.
///
/// Filesystem-only, like everything else in this module: no process is
/// spawned to discover a sibling, and nothing here decides whether one may
/// be *probed* — that stays with `exec::run_inert` and spec §6, reached the
/// same way any root `--help` probe is.
pub fn discover_path_siblings(parent: &str) -> Vec<PathSibling> {
    let Some(path_var) = std::env::var_os("PATH") else {
        return Vec::new();
    };
    let dirs: Vec<PathBuf> = std::env::split_paths(&path_var).collect();
    discover_path_siblings_in(&dirs, parent)
}

/// [`discover_path_siblings`] against an explicit directory list — the seam
/// tests use, so they never have to mutate the process-global `PATH` (which
/// the test harness runs in parallel threads and cannot serialize).
pub fn discover_path_siblings_in(dirs: &[PathBuf], parent: &str) -> Vec<PathSibling> {
    // Neither has a `PATH` neighbourhood to look in. The empty check is
    // the one that changes an answer: the prefix would otherwise collapse
    // to a bare `-`, matching every `-foo` on `PATH`.
    if parent.is_empty() || parent.contains(std::path::MAIN_SEPARATOR) {
        return Vec::new();
    }
    let prefix = format!("{parent}-");
    let mut found: Vec<PathSibling> = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let file_name = entry.file_name();
            let Some(binary) = file_name.to_str() else {
                continue;
            };
            let Some(sub) = binary.strip_prefix(&prefix) else {
                continue;
            };
            // Same name-shape rule every tier applies (spec §7 Tier B rule
            // 3), so a build artifact or capitalized helper never becomes
            // a tree row.
            if !mandible_core::is_command_name_shaped(sub) {
                continue;
            }
            // First `PATH` entry wins, exactly as `find_on_path` resolves.
            if found.iter().any(|s| s.name == sub) {
                continue;
            }
            if !is_executable(&entry.path()) {
                continue;
            }
            found.push(PathSibling {
                name: sub.to_string(),
                binary: binary.to_string(),
            });
        }
    }
    // Alphabetical, not `readdir` order: directory order differs between
    // filesystems, so sorting makes the tree the same across machines.
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found.truncate(MAX_PATH_SIBLINGS);
    found
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    // A path separator in `name` means the caller already gave us a path;
    // check it directly rather than searching PATH. Made absolute below:
    // a relative path resolves against this process's CWD here, but every
    // probe spawns with its CWD redirected into a scratch dir (spec §6
    // rule 8), so an unresolved relative path would fail to spawn ENOENT.
    if name.contains(std::path::MAIN_SEPARATOR) {
        let candidate = PathBuf::from(name);
        return is_executable(&candidate).then(|| absolute(candidate));
    }
    // `PATH` entries may themselves be relative (`PATH=.:/usr/bin`).
    let path_var = std::env::var_os("PATH")?;
    std::env::split_paths(&path_var)
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
        .map(absolute)
}

/// Make `path` absolute **without resolving symlinks**, falling back to it
/// unchanged if the filesystem won't say.
///
/// `std::path::absolute`, never `std::fs::canonicalize`: resolving
/// symlinks defeats spec §6 rule 0 (`is_help_only_probe` matches on file
/// *name*; `reboot`/`poweroff`/`shutdown`/`telinit` are symlinks to
/// `systemctl`) and breaks multi-call binaries that dispatch on `argv[0]`
/// (`iptables` and siblings are symlinks to `xtables-nft-multi`).
/// Absoluteness alone is what the caller needs.
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

    /// A resolved path must be absolute: it resolves against this
    /// process's CWD but spawns from a scratch directory (spec §6 rule 8).
    #[test]
    fn a_relative_path_resolves_to_an_absolute_one() {
        // Inside the current directory so the path is genuinely relative,
        // without `set_current_dir` (process-global, would race other tests).
        let dir = tempfile::TempDir::new_in(".").unwrap();
        let script = dir.path().join("toolish");
        std::fs::write(&script, "#!/bin/sh\necho hi\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        // Rebuild the relative spelling from the absolute path's final component.
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

    /// A directory of fixtures for the sibling-discovery tests: every entry
    /// is created executable unless `plain` names it.
    ///
    /// `new_in(".")` rather than `TempDir::new()`: under `cargo test`,
    /// tests share one process, and a concurrent test redirecting
    /// `$TMPDIR` (spec §6 rule 8) can make `TempDir::new()` fail `NotFound`.
    #[cfg(unix)]
    fn sibling_dir(names: &[&str], plain: &[&str]) -> tempfile::TempDir {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::TempDir::new_in(".").unwrap();
        for name in names {
            let file = dir.path().join(name);
            std::fs::write(&file, "#!/bin/sh\nexit 0\n").unwrap();
            let mode = if plain.contains(name) { 0o644 } else { 0o755 };
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode)).unwrap();
        }
        dir
    }

    /// `cargo --help` never lists `clippy`, but `cargo-clippy` is right
    /// there on `PATH` (spec §5.4).
    #[cfg(unix)]
    #[test]
    fn discovers_dash_prefixed_executables_as_subcommands() {
        let dir = sibling_dir(&["cargo-clippy", "cargo-nextest", "cargo", "rustc"], &[]);
        let found = discover_path_siblings_in(&[dir.path().to_path_buf()], "cargo");
        let names: Vec<&str> = found.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["clippy", "nextest"]);
        assert_eq!(found[0].binary, "cargo-clippy");
    }

    /// A file nobody can run is not a command, however it is named.
    #[cfg(unix)]
    #[test]
    fn a_non_executable_file_is_not_a_sibling() {
        let dir = sibling_dir(&["cargo-clippy", "cargo-notes"], &["cargo-notes"]);
        let found = discover_path_siblings_in(&[dir.path().to_path_buf()], "cargo");
        let names: Vec<&str> = found.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["clippy"]);
    }

    /// Same name-shape rule every tier applies to a bare word (spec §7
    /// Tier B rule 3): a versioned or capitalized helper is not a
    /// subcommand name.
    #[cfg(unix)]
    #[test]
    fn a_name_that_is_not_command_shaped_is_not_a_sibling() {
        let dir = sibling_dir(&["git-lfs", "git-2.43", "git-README", "git-"], &[]);
        let found = discover_path_siblings_in(&[dir.path().to_path_buf()], "git");
        let names: Vec<&str> = found.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["lfs"]);
    }

    /// First `PATH` entry wins, exactly as [`find_on_path`] resolves.
    #[cfg(unix)]
    #[test]
    fn a_shadowed_sibling_is_reported_once_from_the_first_path_entry() {
        let first = sibling_dir(&["cargo-clippy"], &[]);
        let second = sibling_dir(&["cargo-clippy"], &[]);
        let found = discover_path_siblings_in(
            &[first.path().to_path_buf(), second.path().to_path_buf()],
            "cargo",
        );
        assert_eq!(found.len(), 1);
    }

    /// Directory order differs between filesystems; the tree must not.
    #[cfg(unix)]
    #[test]
    fn siblings_come_back_in_alphabetical_order() {
        let dir = sibling_dir(&["cargo-zzz", "cargo-aaa", "cargo-mmm"], &[]);
        let found = discover_path_siblings_in(&[dir.path().to_path_buf()], "cargo");
        let names: Vec<&str> = found.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["aaa", "mmm", "zzz"]);
    }

    /// A parent with no `PATH` neighbourhood finds nothing in it. The empty
    /// spelling is the half that can go wrong: its prefix would otherwise
    /// collapse to a bare `-`, matching every `-foo` on `PATH`.
    #[cfg(unix)]
    #[test]
    fn a_parent_with_no_neighbourhood_matches_nothing() {
        let dir = sibling_dir(&["-foo", "tool-real"], &[]);
        assert!(discover_path_siblings_in(&[dir.path().to_path_buf()], "").is_empty());
        assert!(discover_path_siblings_in(
            &[dir.path().to_path_buf()],
            &format!(
                ".{}scripts{}tool",
                std::path::MAIN_SEPARATOR,
                std::path::MAIN_SEPARATOR
            )
        )
        .is_empty());
    }

    /// A `libexec`-shaped directory on `PATH` must not hand the background
    /// warmer hundreds of extra probes.
    #[cfg(unix)]
    #[test]
    fn discovery_is_capped() {
        let names: Vec<String> = (0..MAX_PATH_SIBLINGS + 10)
            .map(|i| format!("git-c{i:03}"))
            .collect();
        let refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
        let dir = sibling_dir(&refs, &[]);
        let found = discover_path_siblings_in(&[dir.path().to_path_buf()], "git");
        assert_eq!(found.len(), MAX_PATH_SIBLINGS);
    }

    /// Making the path absolute must not follow symlinks, because the
    /// never-probe refusal (spec §6 rule 0) matches on file *name*:
    /// `reboot`/`poweroff`/`shutdown`/`telinit` are symlinks to `systemctl`.
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
