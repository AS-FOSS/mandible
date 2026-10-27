//! A probe in a tier runs through `exec::run_probe`, so it shares the read's
//! limit. Passing `extract_limit()` to `Probe::run` straight would give a
//! later help form a fresh full limit.

use std::path::{Path, PathBuf};

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_tier_hands_the_whole_limit_to_a_probe() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    let exec = src.join("exec");
    let offenders: Vec<_> = files
        .into_iter()
        .filter(|f| !f.starts_with(&exec))
        .filter(|f| {
            std::fs::read_to_string(f)
                .unwrap()
                .contains("extract_limit()")
        })
        .collect();
    assert!(offenders.is_empty(), "{offenders:?}");
}
