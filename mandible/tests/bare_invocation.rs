//! What `mandible` alone does, checked at the process boundary because
//! that is where the answer lives: which stream the text lands on and what
//! the exit status is are properties of the program, not of a function.
//!
//! A tool whose whole job is showing people help owes a bare invocation
//! real help. It used to answer with a single unstructured
//! `Error: usage: mandible <tool> (or: …)` line, which named three of its
//! modes and no flags at all.

use std::process::Command;

fn mandible() -> Command {
    Command::new(env!("CARGO_BIN_EXE_mandible"))
}

/// Bare `mandible`: clap's own help, on stderr, exiting non-zero.
///
/// Non-zero because nothing was asked for — a shell script that runs
/// `mandible "$1"` with an empty argument must be able to tell that apart
/// from a successful run. On stderr for the same reason: this is the error
/// path, and stdout belongs to whoever is reading it (`--print-selection`'s
/// shell binding most of all).
#[test]
fn a_bare_invocation_prints_help_and_fails() {
    let out = mandible().output().expect("failed to run mandible");

    assert!(!out.status.success(), "expected a non-zero exit");
    assert_eq!(
        out.stdout,
        b"",
        "stdout must stay empty: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Usage: mandible"),
        "the generated usage line must be there: {stderr:?}"
    );
    // Structure, not a wall of prose: the shape only clap's own renderer
    // produces, and the flags the hand-written line never mentioned.
    for expected in ["Arguments:", "Options:", "--doctor", "--print-selection"] {
        assert!(
            stderr.contains(expected),
            "help must contain {expected:?}: {stderr:?}"
        );
    }
    assert!(
        !stderr.contains("usage: mandible <tool>"),
        "the old one-line usage string must be gone: {stderr:?}"
    );
}

/// Asking for help is still a success on stdout. The fix moves the
/// *unasked-for* help to the error path; it must not drag `-h` there with
/// it, which would break anything piping `mandible --help`.
#[test]
fn asking_for_help_still_succeeds_on_stdout() {
    for flag in ["-h", "--help"] {
        let out = mandible().arg(flag).output().expect("failed to run");
        assert!(out.status.success(), "{flag} should exit zero");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("Usage: mandible"),
            "{flag} prints help on stdout: {stdout:?}"
        );
        assert!(
            out.stderr.is_empty(),
            "{flag}: stderr must stay empty, got {:?}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
