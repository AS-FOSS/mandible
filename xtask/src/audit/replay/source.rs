//! Cached source checkouts and harness builds, one per ref.
//!
//! A checkout is `git archive` of the ref's commit extracted under
//! `<root>/src/<ref>`, so no branch, worktree or index of the repository is
//! touched. The harness crate is written into that copy and added to its
//! workspace; the copy is never committed anywhere. The build lands in
//! `<root>/build/<ref>`.

use anyhow::{bail, Context};
use mandible_extract::exec::host::{self, HostOutput};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

const HARNESS_MAIN: &str = include_str!("harness/main.rs");
const HARNESS_DUMP: &str = include_str!("harness/dump.rs");
const HARNESS_MANIFEST: &str = include_str!("harness/Cargo.toml.tmpl");
const HARNESS_DIR: &str = "replay-harness";
const MARKER: &str = ".replay-commit";

/// A built harness for one ref.
pub struct Prepared {
    /// The harness executable.
    pub bin: PathBuf,
    /// Full commit sha the ref resolved to.
    pub commit: String,
    /// The workspace version declared at that commit.
    pub version: String,
}

/// Directory name for a ref (slashes replaced).
pub fn ref_dir_name(git_ref: &str) -> String {
    git_ref.replace(['/', '\\'], "_")
}

/// Run `git` with `args` and return trimmed stdout.
pub fn git(args: &[&str]) -> anyhow::Result<String> {
    let out = host::run("git", args, None, &[]).context("running git")?;
    if !out.success {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// True when `ancestor` is reachable from `commit` (or is it).
pub fn is_ancestor(ancestor: &str, commit: &str) -> bool {
    host::run(
        "git",
        &["merge-base", "--is-ancestor", ancestor, commit],
        None,
        &[],
    )
    .is_ok_and(|out| out.success)
}

/// The commit `git_ref` names.
pub fn resolve_commit(git_ref: &str) -> anyhow::Result<String> {
    git(&["rev-parse", "--verify", &format!("{git_ref}^{{commit}}")])
}

/// The `[workspace.package] version` declared by `Cargo.toml` text.
pub fn workspace_version(cargo_toml: &str) -> Option<String> {
    let doc: toml::Value = toml::from_str(cargo_toml).ok()?;
    doc.get("workspace")?
        .get("package")?
        .get("version")?
        .as_str()
        .map(str::to_string)
}

/// The workspace version at `commit`.
pub fn version_at(commit: &str) -> anyhow::Result<String> {
    let manifest = git(&["show", &format!("{commit}:Cargo.toml")])?;
    workspace_version(&manifest).with_context(|| format!("no workspace version at {commit}"))
}

fn extract(commit: &str, dir: &Path) -> anyhow::Result<()> {
    if dir.exists() {
        std::fs::remove_dir_all(dir).with_context(|| format!("clearing {}", dir.display()))?;
    }
    std::fs::create_dir_all(dir)?;
    let tarball = dir.join(".replay-source.tar");
    let archive = host::run(
        "git",
        &[
            OsStr::new("archive"),
            OsStr::new("--format=tar"),
            OsStr::new("-o"),
            tarball.as_os_str(),
            OsStr::new(commit),
        ],
        None,
        &[],
    )
    .context("running git archive")?;
    check(&archive, "git archive")?;
    let tar = host::run(
        "tar",
        &[
            OsStr::new("-x"),
            OsStr::new("-f"),
            tarball.as_os_str(),
            OsStr::new("-C"),
            dir.as_os_str(),
        ],
        None,
        &[],
    )
    .context("running tar")?;
    check(&tar, "tar")?;
    std::fs::remove_file(&tarball)?;
    std::fs::write(dir.join(MARKER), commit)?;
    Ok(())
}

fn check(out: &HostOutput, what: &str) -> anyhow::Result<()> {
    if out.success {
        return Ok(());
    }
    bail!(
        "{what} failed: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    )
}

fn write_if_changed(path: &Path, content: &str) -> anyhow::Result<()> {
    if std::fs::read_to_string(path).is_ok_and(|old| old == content) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content).with_context(|| format!("writing {}", path.display()))
}

/// Write the harness crate into `checkout` and list it as a workspace member.
pub fn install_harness(checkout: &Path) -> anyhow::Result<()> {
    let crate_dir = checkout.join(HARNESS_DIR);
    write_if_changed(&crate_dir.join("Cargo.toml"), HARNESS_MANIFEST)?;
    write_if_changed(&crate_dir.join("src/main.rs"), HARNESS_MAIN)?;
    write_if_changed(&crate_dir.join("src/dump.rs"), HARNESS_DUMP)?;
    let manifest_path = checkout.join("Cargo.toml");
    let manifest = std::fs::read_to_string(&manifest_path)?;
    if let Some(patched) = add_member(&manifest) {
        std::fs::write(&manifest_path, patched)?;
    }
    Ok(())
}

/// The workspace manifest with the harness added to `members`, or `None`
/// when it is already listed.
pub fn add_member(manifest: &str) -> Option<String> {
    if manifest.contains(&format!("\"{HARNESS_DIR}\"")) {
        return None;
    }
    Some(manifest.replacen(
        "members = [",
        &format!("members = [\n    \"{HARNESS_DIR}\","),
        1,
    ))
}

/// Check out `git_ref` under `root` and build its harness, reusing what is
/// cached. `root` is `target/replay`.
pub fn prepare(root: &Path, git_ref: &str) -> anyhow::Result<Prepared> {
    let commit = resolve_commit(git_ref)?;
    let version = version_at(&commit)?;
    let name = ref_dir_name(git_ref);
    let checkout = root.join("src").join(&name);
    let cached = std::fs::read_to_string(checkout.join(MARKER)).ok();
    if cached.as_deref() != Some(commit.as_str()) {
        extract(&commit, &checkout)?;
    }
    install_harness(&checkout)?;
    if !add_member_present(&checkout)? {
        bail!(
            "{} has no workspace members list to extend",
            checkout.display()
        );
    }
    let target = std::path::absolute(root.join("build").join(&name))?;
    let build_args = ["build", "--release", "-p", HARNESS_DIR];
    let env = [("CARGO_TARGET_DIR", target.as_os_str())];
    let out = host::run("cargo", &build_args, Some(&checkout), &env).context("running cargo")?;
    if !out.success {
        let err = String::from_utf8_lossy(&out.stderr);
        let tail: Vec<&str> = err.lines().rev().take(25).collect();
        bail!(
            "building {git_ref} failed:\n{}",
            tail.into_iter().rev().collect::<Vec<_>>().join("\n")
        );
    }
    let bin = target.join("release").join(HARNESS_DIR);
    if !bin.is_file() {
        bail!("{} was not produced", bin.display());
    }
    Ok(Prepared {
        bin,
        commit,
        version,
    })
}

fn add_member_present(checkout: &Path) -> anyhow::Result<bool> {
    let manifest = std::fs::read_to_string(checkout.join("Cargo.toml"))?;
    Ok(manifest.contains(&format!("\"{HARNESS_DIR}\"")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_version_is_read() {
        let m = "[workspace]\nmembers = []\n[workspace.package]\nversion = \"0.8.4\"\n";
        assert_eq!(workspace_version(m).as_deref(), Some("0.8.4"));
        assert_eq!(workspace_version("[package]\nname = \"x\"\n"), None);
    }

    #[test]
    fn member_is_added_once() {
        let m = "[workspace]\nmembers = [\n    \"a\",\n]\n";
        let once = add_member(m).unwrap();
        assert!(once.contains("members = [\n    \"replay-harness\",\n    \"a\","));
        assert!(add_member(&once).is_none());
    }

    #[test]
    fn ref_names_are_path_safe() {
        assert_eq!(ref_dir_name("feat/x"), "feat_x");
        assert_eq!(ref_dir_name("v0.9.0"), "v0.9.0");
    }
}
