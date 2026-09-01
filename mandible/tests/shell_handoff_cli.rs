//! `--print-selection` and `--shell-init` at the process boundary, which
//! is the only place the promise they make can be checked: **stdout
//! carries the composed selection and nothing else.**
//!
//! Everything here runs the real binary with stdout on a pipe — the shape
//! the shell binding uses (`sel=$(mandible --print-selection git)`) — so a
//! byte written to stdout by any path lands where the test can see it.
//! What these cannot do is drive the UI: this sandbox has no tty
//! (AGENTS.md §3.2), and `--print-selection` refuses without one. That
//! half is verified through `scripts/pty_screenshot.py`, and the
//! composition itself is unit-tested in `mandible-tui`.

use std::path::PathBuf;
use std::process::Command;

fn mandible() -> Command {
    Command::new(env!("CARGO_BIN_EXE_mandible"))
}

/// Refusing is fine; refusing *onto stdout* is not. Whatever the mode has
/// to say about a missing terminal goes to stderr, because a shell binding
/// puts stdout on the user's prompt and running it.
#[test]
fn a_refusal_leaves_stdout_empty_and_explains_itself_on_stderr() {
    let out = mandible()
        .args(["--print-selection", "git"])
        .output()
        .expect("failed to run mandible");

    assert!(!out.status.success(), "expected a non-zero exit");
    assert_eq!(
        out.stdout,
        b"",
        "stdout must stay empty: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("stderr is not a tty"),
        "the message must name the stream it needs: {stderr:?}"
    );
}

/// `mandible mandible` is an about screen printed to stdout. Under
/// `--print-selection` stdout belongs to the calling shell, so the easter
/// egg must not fire there — a binding invoked on the word `mandible`
/// would otherwise hand the user an ASCII banner to run.
#[test]
fn the_about_screen_never_reaches_a_print_selection_stdout() {
    let plain = mandible()
        .arg("mandible")
        .output()
        .expect("failed to run mandible");
    assert!(
        !plain.stdout.is_empty(),
        "precondition: `mandible mandible` prints the about screen"
    );

    let piped = mandible()
        .args(["--print-selection", "mandible"])
        .output()
        .expect("failed to run mandible");
    assert_eq!(
        piped.stdout,
        b"",
        "stdout must stay empty: {:?}",
        String::from_utf8_lossy(&piped.stdout)
    );
}

/// The diagnostic flags all print to stdout, so combining one with
/// `--print-selection` would put a diagnostic dump where a command line
/// belongs. Refused by clap rather than left to surprise someone.
#[test]
fn print_selection_refuses_to_share_stdout_with_a_diagnostic() {
    for other in [
        vec!["--doctor", "git"],
        vec!["--report", "git"],
        vec!["--completions", "bash"],
        vec!["--shell-init", "bash"],
        vec!["--review", "1"],
    ] {
        let out = mandible()
            .arg("--print-selection")
            .args(&other)
            .output()
            .expect("failed to run mandible");
        assert!(
            !out.status.success(),
            "{other:?} should conflict with --print-selection"
        );
        assert!(
            out.stdout.is_empty(),
            "{other:?}: stdout must stay empty, got {:?}",
            String::from_utf8_lossy(&out.stdout)
        );
    }
}

/// Without the flag, a non-tty stdout still gets the message it always
/// got. The mode is opt-in, including in how it fails.
#[test]
fn the_default_refusal_is_unchanged() {
    let out = mandible().arg("git").output().expect("failed to run");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("mandible requires an interactive terminal (stdout is not a tty)"),
        "{stderr:?}"
    );
}

/// The snippets are shell code, and a snippet that does not parse is a
/// broken rc file for everyone who evaluated it. `bash -n` is the check
/// the shell itself would do.
#[test]
fn the_bash_snippet_parses_as_bash() {
    let out = mandible()
        .args(["--shell-init", "bash"])
        .output()
        .expect("failed to run mandible");
    assert!(out.status.success());

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("mandible.bash");
    std::fs::write(&path, &out.stdout).expect("write snippet");

    let check = Command::new("bash")
        .arg("-n")
        .arg(&path)
        .output()
        .expect("failed to run bash -n");
    assert!(
        check.status.success(),
        "bash rejected the snippet: {}",
        String::from_utf8_lossy(&check.stderr)
    );
}

/// One generator, spec §15's rule for completions applied to these: what
/// the binary prints is the file in the repo, byte for byte, so no
/// packaging channel can ship a snippet that disagrees with it.
#[test]
fn each_snippet_is_the_packaged_file_verbatim() {
    for (shell, file) in [("bash", "mandible.bash"), ("zsh", "mandible.zsh")] {
        let out = mandible()
            .args(["--shell-init", shell])
            .output()
            .expect("failed to run mandible");
        assert!(out.status.success(), "{shell}");
        // Compared as text, not as bytes: a mismatch here is read by a
        // human, and two 1 KB byte vectors in an assertion message are not.
        let printed = String::from_utf8(out.stdout).expect("the snippet is UTF-8");
        // The snippet lives inside this crate (`mandible/shell/`), never
        // under the repo's `packaging/`: a published crate's tarball holds
        // only files under the crate root, and `cargo publish` verifies the
        // build from that tarball — the 0.6.0 binary crate failed exactly
        // there. Resolved from the manifest dir, so this test would fail
        // the same way the registry did if the file ever moved back out.
        let packaged = std::fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("shell")
                .join(file),
        )
        .unwrap_or_else(|e| panic!("reading mandible/shell/{file}: {e}"));
        assert_eq!(
            printed, packaged,
            "`--shell-init {shell}` must print mandible/shell/{file} verbatim"
        );
    }
}
