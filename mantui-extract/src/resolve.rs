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
    if name.contains(std::path::MAIN_SEPARATOR) {
        let candidate = PathBuf::from(name);
        return is_executable(&candidate).then_some(candidate);
    }
    let path_var = std::env::var_os("PATH")?;
    std::env::split_paths(&path_var)
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
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
}
