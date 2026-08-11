//! No source file may reference a path that exists only on the machine
//! that wrote it.
//!
//! This exists because it happened. A subagent debugging the existence
//! detector needed sample help text, wrote it into the agent session's
//! scratch directory — the path it had been handed for *working* files —
//! and left two tests behind that read it back by absolute path:
//!
//! ```ignore
//! std::fs::read_to_string(
//!     "/tmp/claude-1001/-home-ubuntu-projects-mantui/a018c57e-…/scratchpad/lto-dump-help.txt",
//! ).unwrap()
//! ```
//!
//! Every gate passed. `cargo fmt`, `cargo clippy -D warnings`, 575 tests,
//! the corpus runner — all green, because the file existed on that machine
//! at that moment. The tests asserted something real about `--param=`
//! parsing and would have panicked for every other reader of this
//! repository, forever. It was caught by a human reading the file, which
//! is not a control anyone should rely on.
//!
//! The distinction the rule encodes:
//!
//! - **Repo-relative committed asset — fine.** `include_str!(
//!   "../../corpus/tar/1.35/help.txt")` replays a real captured fixture
//!   from this repository. Both files travel together; the coupling is
//!   between two test assets and is the point.
//! - **Absolute machine-local path — never.** It passes for one process on
//!   one machine and is a lie everywhere else.
//!
//! Deliberately a source lint rather than a convention, per AGENTS.md §6:
//! prefer making a mistake impossible over writing it down. A convention
//! would have to be read by whoever writes the next line of code, and the
//! agent that wrote this one had been *handed* the scratch path in its
//! own instructions.

use std::path::{Path, PathBuf};

/// Path prefixes that only ever resolve on the machine that wrote them.
///
/// `/tmp` covers agent scratch directories, `mktemp` output pasted into a
/// test, and the `/tmp/ptyvenv` that `AGENTS.md` documents for the pty
/// screenshot tool. `/home` and `/Users` catch a developer's own checkout
/// path, which is the same mistake wearing different clothes.
const MACHINE_LOCAL_PREFIXES: [&str; 4] = ["\"/tmp/", "\"/home/", "\"/Users/", "\"/var/folders/"];

#[test]
fn no_source_file_references_a_machine_local_absolute_path() {
    let workspace_root = workspace_root();
    let crate_src_dirs = [
        "mandible-core/src",
        "mandible-extract/src",
        "mandible-search/src",
        "mandible-tui/src",
        "mandible/src",
        "xtask/src",
        "mandible-extract/tests",
        "mandible-tui/tests",
    ];

    let mut violations = Vec::new();
    for dir in crate_src_dirs {
        let dir = workspace_root.join(dir);
        if !dir.exists() {
            continue;
        }
        for file in rust_files(&dir) {
            // The lint's own pattern table necessarily contains the
            // literals it searches for. Exempting the implementing file is
            // the standard shape for a self-referential lint.
            if file
                .file_name()
                .is_some_and(|n| n == "no_machine_local_paths.rs")
            {
                continue;
            }
            let text = std::fs::read_to_string(&file).unwrap_or_default();
            for (lineno, line) in text.lines().enumerate() {
                // This file quotes the offending shape in its own doc
                // comment to explain the rule; exempt doc/line comments
                // so the explanation doesn't trip the check it documents.
                let trimmed = line.trim_start();
                if trimmed.starts_with("//") {
                    continue;
                }
                for prefix in MACHINE_LOCAL_PREFIXES {
                    if line.contains(prefix) {
                        violations.push(format!(
                            "{}:{}: {}",
                            file.strip_prefix(&workspace_root)
                                .unwrap_or(&file)
                                .display(),
                            lineno + 1,
                            line.trim()
                        ));
                    }
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "source references a path that exists only on the machine that wrote it.\n\
         Such code passes its own gates and fails for every other reader.\n\
         Use a repo-relative committed asset (`include_str!(\"../../corpus/…\")`),\n\
         an inline literal, or a `tempfile::TempDir` created by the test itself.\n\n{}",
        violations.join("\n")
    );
}

fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(rust_files(&path));
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
    out
}

fn workspace_root() -> PathBuf {
    // `CARGO_MANIFEST_DIR` is `<root>/mandible-extract` for this crate.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate dir always has a workspace parent")
        .to_path_buf()
}
