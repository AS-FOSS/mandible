//! `mandible-tui` may only reach a terminal stream through
//! [`terminal::Sink`], never by naming `stdout`/`stderr` itself.
//!
//! The rule exists because `mandible --print-selection` (spec §2) draws its
//! UI while stdout carries exactly one line — the composed command — for
//! the calling shell to read back. Under that mode **any** byte this crate
//! writes to stdout on its own initiative is corruption of the mode's only
//! output, and it corrupts it silently: the shell puts whatever arrived on
//! the user's prompt.
//!
//! It is a lint rather than a type because the offending call is always a
//! plausible-looking one-liner. Both existing cases were: `clipboard`'s
//! OSC-52 fallback wrote its escape sequence to `io::stdout()`, and
//! `style`'s color check asked `io::stdout().is_terminal()` — which under
//! this mode answers about the pipe rather than about the screen the user
//! is looking at, rendering the whole UI monochrome. Neither is visible in
//! a `TestBackend` render test, because neither goes through the backend.
//!
//! Modelled on `mandible-extract/tests/no_process_outside_exec.rs`, which
//! polices spec §6/§8's `Command::new` boundary the same way.

use std::path::{Path, PathBuf};

/// Every way a Rust source line can reach a standard stream directly.
/// `write!`/`writeln!` are not listed: they take an explicit writer, and
/// the only writers this crate can obtain are the sink's.
const FORBIDDEN: &[&str] = &[
    "io::stdout(",
    "io::stderr(",
    "stdout()",
    "stderr()",
    "println!",
    "print!",
    "eprintln!",
    "eprint!",
];

#[test]
fn nothing_outside_terminal_rs_writes_to_a_standard_stream() {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    assert!(src.is_dir(), "expected {} to exist", src.display());

    // The one module allowed to name the streams: it is what defines the
    // sink, and `Sink::writer`/`Sink::is_tty` are the two functions that
    // hand every other module its answer.
    let allowed = src.join("terminal.rs");

    let mut violations = Vec::new();
    walk_rs_files(&src, &mut |path, contents| {
        if path == allowed {
            return;
        }
        for (line_no, line) in contents.lines().enumerate() {
            // Prose about a stream is fine; a call to one is not. Same
            // comment heuristic the exec-boundary lint uses.
            if line.trim_start().starts_with("//") {
                continue;
            }
            for needle in FORBIDDEN {
                if line.contains(needle) {
                    violations.push(format!(
                        "{}:{}: {} — write to the terminal through \
                         `crate::terminal::Sink`, not `{needle}`",
                        path.display(),
                        line_no + 1,
                        line.trim()
                    ));
                }
            }
        }
    });

    assert!(
        violations.is_empty(),
        "mandible-tui reaches a standard stream outside terminal.rs, so \
         `mandible --print-selection` can no longer promise that stdout \
         carries only the selection:\n{}",
        violations.join("\n")
    );
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
