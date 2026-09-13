//! Spec §6 rule 10: refuses before any probe when effective uid is 0.
//!
//! Proven against a real uid 0 via a user namespace (AGENTS.md's own
//! `unshare --user --map-root-user`), not a shim, per §3.5. Skips cleanly
//! where unprivileged user namespaces are unavailable, since that is an
//! environment limit, not a claim about the code.

use std::process::Command;

fn mandible() -> Command {
    Command::new(env!("CARGO_BIN_EXE_mandible"))
}

fn user_namespaces_available() -> bool {
    Command::new("unshare")
        .args(["--user", "--map-root-user", "true"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[test]
fn root_is_refused_before_any_probe() {
    if !user_namespaces_available() {
        eprintln!("unshare --user --map-root-user unavailable; skipping");
        return;
    }
    let out = Command::new("unshare")
        .args(["--user", "--map-root-user"])
        .arg(env!("CARGO_BIN_EXE_mandible"))
        .args(["--doctor", "git"])
        .output()
        .expect("failed to run mandible under unshare");

    assert!(!out.status.success(), "root must be refused");
    assert_eq!(out.stdout, b"", "no probe output belongs on stdout");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("--allow-root"),
        "the refusal must name the flag: {stderr:?}"
    );
}

#[test]
fn allow_root_lets_root_proceed() {
    if !user_namespaces_available() {
        eprintln!("unshare --user --map-root-user unavailable; skipping");
        return;
    }
    let out = Command::new("unshare")
        .args(["--user", "--map-root-user"])
        .arg(env!("CARGO_BIN_EXE_mandible"))
        .args(["--allow-root", "--doctor", "git"])
        .output()
        .expect("failed to run mandible under unshare");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("--allow-root"),
        "the refusal must not fire: {stderr:?}"
    );
}

/// A non-root uid never sees the refusal, regardless of the flag.
#[test]
fn non_root_never_refused() {
    let out = mandible()
        .args(["--doctor", "git"])
        .output()
        .expect("failed to run mandible");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("--allow-root"),
        "a non-root run must not print the root refusal: {stderr:?}"
    );
}
