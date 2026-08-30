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
///
/// A backstop against a directory full of `<parent>-*` helpers (git's own
/// `libexec` layout is ~150 of them, and a machine that puts such a
/// directory on `PATH` would otherwise hand the background warmer that many
/// extra probes for names the parent never documented), not a tuning knob.
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
    // Neither of these has a `PATH` neighbourhood to look in. The empty
    // check is the one that changes an answer: the prefix would collapse to
    // a bare `-`, and every `-foo` on `PATH` would read as a subcommand of
    // nothing. A tool the user spelled as a path (`mandible
    // ./scripts/tool.py`) can never match — a directory entry's file name
    // holds no separator — so that half only skips a scan whose result is
    // already known.
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
            // The same name-shape rule every tier applies before believing a
            // bare word is a command (spec §7 Tier B rule 3), so a build
            // artifact (`cargo-clippy.exe.bak`, `git-2.43`) or a
            // capitalized helper never becomes a tree row.
            if !mandible_core::is_command_name_shaped(sub) {
                continue;
            }
            // First `PATH` entry wins, exactly as `find_on_path` resolves a
            // tool name, so a shadowed sibling is reported once and by the
            // binary that would actually run.
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
    // Alphabetical, not `readdir` order: there is no document order to
    // preserve here (spec §4.4's ordering argument is about what a tool's
    // own text listed), and directory order differs between filesystems, so
    // sorting is what makes the tree the same on two machines with the same
    // binaries installed.
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found.truncate(MAX_PATH_SIBLINGS);
    found
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

    /// A directory of fixtures for the sibling-discovery tests: every entry
    /// is created executable unless `plain` names it.
    ///
    /// `new_in(".")` rather than `TempDir::new()`, for the same reason the
    /// two tests above build their fixtures that way: this crate's tests
    /// share one process under `cargo test`, some of them redirect `TMPDIR`
    /// at a scratch directory they then delete (spec §6 rule 8), and a
    /// concurrent `TempDir::new()` fails with `NotFound` on a `$TMPDIR` that
    /// no longer exists. `cargo nextest` gives each test its own process and
    /// never sees it, which is exactly the kind of runner-dependent failure
    /// not worth leaving in place.
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

    /// The specimen from issue #70: `cargo --help` never lists `clippy`, but
    /// `cargo-clippy` is right there on `PATH` (spec §5.4).
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

    /// The same name-shape rule every tier applies to a bare word (spec §7
    /// Tier B rule 3): a versioned or capitalized helper beside the tool is
    /// not a subcommand name.
    #[cfg(unix)]
    #[test]
    fn a_name_that_is_not_command_shaped_is_not_a_sibling() {
        let dir = sibling_dir(&["git-lfs", "git-2.43", "git-README", "git-"], &[]);
        let found = discover_path_siblings_in(&[dir.path().to_path_buf()], "git");
        let names: Vec<&str> = found.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["lfs"]);
    }

    /// First `PATH` entry wins, exactly as [`find_on_path`] resolves a tool
    /// name — a shadowed sibling must be reported once, under the binary
    /// that would actually run.
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

    /// A parent with no `PATH` neighbourhood finds nothing in it.
    ///
    /// The empty spelling is the half that can go wrong: its prefix
    /// collapses to a bare `-`, and every `-foo` on `PATH` would read as a
    /// subcommand of nothing. A path-spelled tool is asserted alongside it
    /// because that is the case the early return is *written* for, even
    /// though a directory entry's file name can never contain a separator.
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
