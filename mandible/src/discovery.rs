//! Children discovered by the `<parent>-<sub>` PATH convention (spec §5.4).
//!
//! `cargo --help` never mentions `clippy`; `cargo-clippy` sits on `PATH`,
//! and `cargo clippy` works because cargo dispatches to it. git does the
//! same for `git-lfs`. The convention is the tool's, not this program's, so
//! the rule here is keyed on the *name shape* alone and knows nothing about
//! cargo or git (AGENTS.md §1) — `mandible-extract`'s
//! [`discover_path_siblings`] is the filesystem half, and this module is
//! what attaches the result to a tree and decides what a probe of such a
//! node is aimed at.
//!
//! **Why here and not in an extraction tier.** Discovery reads the running
//! machine's `PATH`, so a tier that did it would make every corpus fixture
//! (spec §13.2 — frozen bytes, zero subprocesses, a synthetic
//! `/corpus-replay/<tool>` path) depend on which binaries happen to be
//! installed beside the machine running the suite. It is tree *assembly*,
//! which is already this crate's job rather than the extractor's — the same
//! division the background warmer's cascade lives on (spec §5.2).
//!
//! **Nothing here decides what may be executed.** A discovered node's own
//! probe is the ordinary root `--help` of the sibling binary — exactly what
//! `mandible cargo-clippy` already runs — so it passes through
//! `exec::run_inert` and spec §6 untouched, and the guessed word never
//! becomes an argument to the parent. See [`probe_target`].

use mandible_core::{CommandNode, Provenance};
use mandible_extract::{resolve_tool, PathSibling, ResolvedTool};

/// Add a child node for every sibling the parent's own help text did not
/// already document, marked with the binary it was discovered as.
///
/// Appended after the documented children, never merged into them: a name
/// the tool itself listed is attested, and a convention guess must not be
/// able to overwrite what the tool said about it — a sibling whose name is
/// already a child (or an alias of one) is simply dropped, since the
/// documented node already reaches the same command.
///
/// **A parent that documents no subcommand at all gets none of these.** The
/// convention is a tool *dispatching* on its first argument, and a tool that
/// dispatches says so by documenting at least one command of its own; where
/// there is no such list, a `<parent>-<sub>` file is far more likely a
/// sibling tool sharing a name prefix. Measured on this rule's own worst
/// case: `dpkg --help` lists no commands (its operations are flags), and the
/// 27 `dpkg-*` programs beside it — `dpkg-deb`, `dpkg-architecture`,
/// `dpkg-buildpackage` — are separate tools that `dpkg deb` does not reach.
/// Without this, `mandible dpkg` opened on 27 rows of guesses and nothing
/// else. Keyed on what the parent's own text said, never on its name (§1),
/// and it cannot suppress issue #70's case: `cargo --help` and `git --help`
/// both document plenty.
pub fn attach_path_siblings(root: &mut CommandNode, siblings: &[PathSibling]) {
    if root.subcommands.is_empty() {
        return;
    }
    for sibling in siblings {
        let documented = root
            .subcommands
            .iter()
            .any(|c| c.name == sibling.name || c.aliases.contains(&sibling.name));
        if documented {
            continue;
        }
        let mut node = CommandNode::new(sibling.name.clone(), Provenance::default());
        node.discovered_binary = Some(sibling.binary.clone());
        root.subcommands.push(node);
    }
}

/// Which binary a node's probe is aimed at, and the path to probe it under.
///
/// Ordinarily the tool the session opened, at the node's own tree path. Under
/// a convention-discovered node it is that node's binary instead, with the
/// path rebased onto it: `["cargo", "clippy"]` becomes `cargo-clippy` at
/// `["clippy"]` (a root probe — `cargo-clippy --help`), and a child of it,
/// `["cargo", "clippy", "fix"]`, becomes `cargo-clippy` at `["clippy",
/// "fix"]` (`cargo-clippy fix --help`).
///
/// This is what keeps spec §6 whole. `clippy` is a word read off a filename,
/// not one the parent's help attested, so it is exactly the kind of word the
/// attestation gate exists to keep out of argv — and it never enters one:
/// the probe is a *root* `--help` of a real binary, which needs no
/// attestation because it is not a subcommand word at all, and every deeper
/// word under it is attested by that binary's own help in the ordinary way.
pub fn probe_target(
    root: &CommandNode,
    tool: &ResolvedTool,
    path: &[String],
) -> (ResolvedTool, Vec<String>) {
    // Deepest first: a discovered node nested under another one is probed
    // against the nearer of the two, which is the binary that actually
    // documents the remaining words.
    for depth in (1..path.len()).rev() {
        let Some(node) = mandible_core::resolve(root, &path[..=depth]) else {
            continue;
        };
        let Some(binary) = node.discovered_binary.as_deref() else {
            continue;
        };
        let mut rebased = vec![node.name.clone()];
        rebased.extend_from_slice(&path[depth + 1..]);
        return (resolve_tool(binary), rebased);
    }
    (tool.clone(), path.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandible_core::Source;

    fn node(name: &str) -> CommandNode {
        CommandNode::new(name, Provenance::single(Source::HelpText))
    }

    fn sibling(name: &str, binary: &str) -> PathSibling {
        PathSibling {
            name: name.to_string(),
            binary: binary.to_string(),
        }
    }

    fn path(segments: &[&str]) -> Vec<String> {
        segments.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn an_undocumented_sibling_becomes_a_child_carrying_its_binary() {
        let mut root = node("cargo");
        root.subcommands.push(node("build"));
        attach_path_siblings(&mut root, &[sibling("clippy", "cargo-clippy")]);

        let clippy = root
            .subcommands
            .iter()
            .find(|c| c.name == "clippy")
            .expect("the sibling should have been attached");
        assert_eq!(clippy.discovered_binary.as_deref(), Some("cargo-clippy"));
        // Never probe-eligible as a word of the parent: the whole point is
        // that its own binary is probed instead (spec §6).
        assert!(!clippy.heading_attested);
    }

    /// A name the tool's own help already documents is attested; the
    /// convention must not add a second row for it, nor overwrite it with an
    /// empty stub.
    #[test]
    fn a_documented_name_is_not_duplicated_by_the_convention() {
        let mut root = node("git");
        let mut push = node("push");
        push.summary = Some(mandible_core::Text::sanitize("Update remote refs"));
        root.subcommands.push(push);
        attach_path_siblings(&mut root, &[sibling("push", "git-push")]);

        assert_eq!(root.subcommands.len(), 1);
        assert!(root.subcommands[0].discovered_binary.is_none());
        assert!(root.subcommands[0].summary.is_some());
    }

    /// `dpkg --help` lists no commands at all, and the 27 `dpkg-*` programs
    /// beside it are separate tools — `dpkg deb` reaches nothing. A tool
    /// that dispatches documents at least one command of its own, so a
    /// parent with no command list gets no convention children.
    #[test]
    fn a_parent_that_documents_no_subcommands_gets_no_convention_children() {
        let mut root = node("dpkg");
        attach_path_siblings(&mut root, &[sibling("deb", "dpkg-deb")]);
        assert!(root.subcommands.is_empty());
    }

    #[test]
    fn an_alias_of_a_documented_command_is_not_duplicated_either() {
        let mut root = node("cargo");
        let mut build = node("build");
        build.aliases.push("b".to_string());
        root.subcommands.push(build);
        attach_path_siblings(&mut root, &[sibling("b", "cargo-b")]);

        assert_eq!(root.subcommands.len(), 1);
    }

    #[test]
    fn an_ordinary_node_is_probed_against_the_tool_at_its_own_path() {
        let root = node("git");
        let tool = resolve_tool("git");
        let (target, probe_path) = probe_target(&root, &tool, &path(&["git", "rebase"]));
        assert_eq!(target.name, "git");
        assert_eq!(probe_path, path(&["git", "rebase"]));
    }

    /// The regression this whole feature turns on: a discovered node is a
    /// *root* probe of its own binary, so the guessed word never reaches the
    /// parent's argv (spec §6).
    #[test]
    fn a_discovered_node_is_probed_as_its_own_binarys_root() {
        let mut root = node("cargo");
        root.subcommands.push(node("build"));
        attach_path_siblings(&mut root, &[sibling("clippy", "cargo-clippy")]);
        let tool = resolve_tool("cargo");

        let (target, probe_path) = probe_target(&root, &tool, &path(&["cargo", "clippy"]));
        assert_eq!(target.name, "cargo-clippy");
        assert_eq!(
            probe_path.len(),
            1,
            "words must be empty, i.e. a root probe: {probe_path:?}"
        );
        assert_eq!(probe_path[0], "clippy", "the node keeps its own name");
    }

    /// Serializes the one test that has to touch `PATH`; `set_var` is
    /// process-global and `cargo test` runs this binary's tests on threads.
    #[cfg(unix)]
    static PATH_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[cfg(unix)]
    fn shim(dir: &std::path::Path, name: &str, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        let file = dir.join(name);
        std::fs::write(&file, format!("#!/bin/sh\ncat <<'EOF'\n{body}\nEOF\n")).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// The end-to-end half, against **real argv** through the real runner
    /// and a real spawn (AGENTS.md §3.1: a tier test that never builds argv
    /// proves nothing about the pipeline).
    ///
    /// Both halves matter. The discovered node fills from its own binary —
    /// which is the whole feature — *and* the same node filled the obvious
    /// way, as a word of the parent, recovers nothing at all, because the
    /// word came off a filename and the attestation gate refuses to send it
    /// (spec §6 rule 0's closing paragraph). The redirect is what makes the
    /// feature work; without it there is no probe to make.
    #[cfg(unix)]
    #[test]
    fn a_discovered_node_fills_from_its_own_binary_and_never_through_the_parent() {
        let dir = tempfile::TempDir::new_in(".").unwrap();
        // The parent documents one command of its own, which is what says
        // it dispatches at all (`attach_path_siblings`).
        shim(
            dir.path(),
            "mytool",
            "Usage: mytool [OPTIONS] <COMMAND>\n\nCommands:\n  local  A command the parent documents\n\nOptions:\n  --parent-only  Only the parent documents this",
        );
        shim(
            dir.path(),
            "mytool-extra",
            "Usage: mytool extra [OPTIONS]\n\nOptions:\n  --only-in-extra  Only this binary documents this",
        );

        let _guard = PATH_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let original = std::env::var_os("PATH");
        let mut dirs = vec![dir.path().to_path_buf()];
        if let Some(existing) = &original {
            dirs.extend(std::env::split_paths(existing));
        }
        // Prepended, never replaced: other tests in this binary resolve real
        // tools off `PATH` while this one runs.
        std::env::set_var("PATH", std::env::join_paths(dirs).unwrap());

        let tool = resolve_tool("mytool");
        let runner = mandible_extract::Runner::new(mandible_extract::default_tiers());
        // The real root, from the real binary, so the documented/discovered
        // split is the one the running product would see.
        let mut root = runner
            .extract_full_for(&tool)
            .root
            .expect("the shim's own --help should extract");
        assert!(
            root.subcommands.iter().any(|c| c.name == "local"),
            "the parent's own command list must be recovered first: {:?}",
            root.subcommands.iter().map(|c| &c.name).collect::<Vec<_>>()
        );
        attach_path_siblings(
            &mut root,
            &mandible_extract::discover_path_siblings("mytool"),
        );
        let extra = root
            .subcommands
            .iter()
            .find(|c| c.name == "extra")
            .expect("mytool-extra should have been discovered")
            .clone();
        let tree_path = path(&["mytool", "extra"]);

        let (target, probe_path) = probe_target(&root, &tool, &tree_path);
        let filled = runner.fill_node(&target, &probe_path, extra.clone());
        let through_parent = runner.fill_node(&tool, &tree_path, extra);

        if let Some(original) = original {
            std::env::set_var("PATH", original);
        }

        assert!(
            filled
                .node
                .flags()
                .any(|f| f.long() == Some("only-in-extra")),
            "the discovered node must be filled from its own binary: {:?}",
            filled.node.flags().map(|f| f.long()).collect::<Vec<_>>()
        );
        assert_eq!(
            through_parent.node.flags().count(),
            0,
            "a word read off a filename must never be probed as the parent's argv"
        );
    }

    #[test]
    fn a_child_of_a_discovered_node_is_probed_against_the_same_binary() {
        let mut root = node("git");
        root.subcommands.push(node("commit"));
        attach_path_siblings(&mut root, &[sibling("lfs", "git-lfs")]);
        let lfs = root
            .subcommands
            .iter_mut()
            .find(|c| c.name == "lfs")
            .expect("attached");
        lfs.subcommands.push(node("push"));
        let tool = resolve_tool("git");

        let (target, probe_path) = probe_target(&root, &tool, &path(&["git", "lfs", "push"]));
        assert_eq!(target.name, "git-lfs");
        assert_eq!(probe_path, path(&["lfs", "push"]));
    }
}
