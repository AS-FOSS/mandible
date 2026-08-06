//! Spec §6/§8 enforcement: `std::process` (and `Command::new` specifically)
//! may appear **only** inside `mantui-extract/src/exec/`. This walks the
//! source tree of every crate in the workspace and fails if it finds the
//! forbidden patterns anywhere else, so the boundary is auditable rather
//! than aspirational (spec §8: "A `#![deny]`-style test greps the
//! workspace for `Command::new` outside that module and fails the build
//! otherwise.").

use std::path::{Path, PathBuf};

#[test]
fn command_new_and_std_process_appear_only_in_exec() {
    let workspace_root = workspace_root();
    let crate_src_dirs = [
        "mantui-core/src",
        "mantui-extract/src",
        "mantui-cache/src",
        "mantui-search/src",
        "mantui-tui/src",
        "mantui/src",
        "xtask/src",
    ];

    let allowed_dir = workspace_root.join("mantui-extract/src/exec");
    let mut violations = Vec::new();

    for crate_dir in crate_src_dirs {
        let dir = workspace_root.join(crate_dir);
        if !dir.exists() {
            panic!("expected source directory {} to exist", dir.display());
        }
        walk_rs_files(&dir, &mut |path, contents| {
            if path.starts_with(&allowed_dir) {
                return;
            }
            for (line_no, line) in contents.lines().enumerate() {
                // Skip comments/doc-comments referencing the pattern in
                // prose (this file and exec's own docs do that); only flag
                // actual code usage. A simple heuristic: ignore lines whose
                // first non-whitespace characters are `//`.
                let trimmed = line.trim_start();
                if trimmed.starts_with("//") {
                    continue;
                }
                if line.contains("Command::new") || line.contains("std::process") {
                    violations.push(format!(
                        "{}:{}: {}",
                        path.display(),
                        line_no + 1,
                        line.trim()
                    ));
                }
            }
        });
    }

    assert!(
        violations.is_empty(),
        "std::process / Command::new found outside mantui-extract/src/exec/:\n{}",
        violations.join("\n")
    );
}

fn workspace_root() -> PathBuf {
    // This crate's manifest dir is `<workspace_root>/mantui-extract`.
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .expect("mantui-extract has a parent workspace dir")
        .to_path_buf()
}

fn walk_rs_files(dir: &Path, visit: &mut impl FnMut(&Path, &str)) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_rs_files(&path, visit);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            if let Ok(contents) = std::fs::read_to_string(&path) {
                visit(&path, &contents);
            }
        }
    }
}
