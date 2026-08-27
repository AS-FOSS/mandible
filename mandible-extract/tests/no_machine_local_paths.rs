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
/// Deliberately matched *without* requiring a preceding `"`. The first
/// version of this lint anchored on the quote, which only ever matched a
/// prefix sitting at the very *start* of a string literal — so a
/// machine-local path embedded further into one, `"file:///home/…"`, went
/// unseen. (Measured, because the obvious guesses are wrong: `r"/home/…"`
/// and `concat!("/home/", user)` were both already caught by the anchored
/// form, since each puts a quote immediately before the prefix. Only the
/// mid-literal case is new.) Comment lines are skipped below, which is
/// what the quote was really buying.
///
/// One hole neither form closes, stated so nobody assumes otherwise:
/// a path assembled piecewise, `PathBuf::from("/home").join(user)`, has no
/// `/home/` substring anywhere and is invisible to a line-wise text lint.
/// Catching that needs a real AST pass, which is not worth it for a
/// mistake that has never once arrived in that shape.
///
/// `/root/` is deliberately **not** on this list, and the reason is worth
/// keeping: adding it fires on `framework::artifact`'s clap fingerprint,
/// `b"/root/.cargo/registry/src/index.crates.io/clap_builder-…/src/lib.rs"`,
/// which is not a path this code ever opens — it is a byte pattern *other*
/// binaries carry, baked in by whichever machine compiled them, and the
/// test that uses it writes it into a `tempfile::tempdir()`. That is the
/// "inline literal" the assertion below recommends, so flagging it would
/// be the detector degrading working code to catch a case nobody has ever
/// hit. Same tension applies to the four prefixes that *are* listed: if a
/// future fingerprint has to embed one of them, the fingerprint is right
/// and this list is what changes.
const MACHINE_LOCAL_PREFIXES: [&str; 4] = ["/tmp/", "/home/", "/Users/", "/var/folders/"];

/// Directories with no first-party source to lint. `target` and `tmp`
/// are build and scratch output, `.git` and `.claude` are tooling state
/// (the latter holds agent worktrees, whose own checkouts are linted on
/// their own branches, not through this one), and `.venv` is the
/// gitignored local environment AGENTS.md §3.2 recommends for the pty
/// screenshot tool — third-party packages installed there carry their
/// build machines' paths in docstrings, which is their business, not a
/// leak in this repository.
const SKIPPED_DIRS: [&str; 5] = ["target", ".git", ".claude", "tmp", ".venv"];

#[test]
fn no_source_file_references_a_machine_local_absolute_path() {
    let workspace_root = workspace_root();

    // Walked from the workspace root rather than from a hand-listed set of
    // crate directories. The list version silently stopped covering
    // anything it was not updated for: `mandible-core/tests` existed and
    // was never on it, so a test added there was unlinted, and every new
    // crate would have arrived the same way — a lint that needs a human to
    // remember it is the thing it was written to replace.
    let mut violations = Vec::new();
    for file in rust_files(&workspace_root) {
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
            if trimmed.starts_with("//") || trimmed.starts_with('#') {
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
            if path
                .file_name()
                .is_some_and(|n| SKIPPED_DIRS.iter().any(|s| n == *s))
            {
                continue;
            }
            out.extend(rust_files(&path));
        } else if path
            .extension()
            .is_some_and(|e| e == "rs" || e == "py" || e == "sh")
        {
            // `.py` and `.sh` joined `.rs` after the committed specimen
            // moved: the machine-local `/tmp/ptyvenv` that kept reappearing
            // in agents' output was being *copied from*
            // `scripts/pty_screenshot.py`'s own usage text, which no lint
            // covered. Prose files stay out — AGENTS.md legitimately names
            // the forbidden shapes when stating the rule.
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
