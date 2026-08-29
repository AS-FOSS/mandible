//! End-to-end proof of spec §6 rules 1-3, run through the actual sanctioned
//! path (`mandible_extract::exec::run_inert`), against a shim binary that
//! logs exactly what it was invoked with. Spec §13.3 calls this out
//! explicitly as a required test class ("Execution-policy tests: a shim
//! binary logs argv/env; any invocation outside the allowlist fails the
//! suite.") and as the fix for a specific prior bug ("Real-argv tests":
//! a mocked probe can pass while the real argv construction is broken).

use mandible_extract::exec::{run_inert, ExecError, InertArgv};
use mandible_extract::help_text::HelpTextTier;
use mandible_extract::{ExtractError, ExtractionTier, NodeHints, ResolvedTool};
use std::io::Write;
use std::path::Path;
use std::time::Duration;

// --- [M-17]: the `/dev/tty` hazard, closed structurally, not just
// --- documented ---
//
// A reported `mandible systemctl` freeze, tracked down to a pager (spec §6
// rule 6): `env_clear()` used to leave `PAGER` merely *absent* (which lets
// a tool go find one itself, e.g. `less`), and `process_group(0)` gives a
// probe its own process *group* but leaves it in the *same session* as
// mandible — so a descendant can still `open("/dev/tty")` and reach the
// real controlling terminal directly, bypassing whatever stdin/stdout/
// stderr were redirected to. That's how a pager (or anything else that
// explicitly wants a controlling terminal, e.g. a password prompt) reads
// real keystrokes and leaves real termios changes behind, which a
// process-group kill on timeout does not undo (termios state lives on the
// tty device, not the process).
//
// [M-17] measured that no argv `run_inert` actually constructs against
// `systemctl` triggers this in this environment: systemd's own pager gate
// checks `isatty` on its *own* stdout/stderr, which `run_inert` always
// makes pipes, so it never even reaches the point of trying. (See spec.md
// Appendix A [M-17] for the full method — a 74-verb sweep plus `strace`
// confirmation that `less` itself, even with a real controlling terminal
// available via the session, never attempts `/dev/tty` once its own
// stdout is non-tty.) But the underlying mechanism the bug report
// pointed at — a descendant reaching the real controlling terminal via
// `process_group(0)`'s session-sharing — is real, independent of
// `systemctl` or pagers specifically, and this test demonstrates it
// directly with a shim that just tries the `open("/dev/tty")` call: the
// fixture-not-prose version of the lesson.
mod dev_tty_hazard {
    use super::*;

    const ROLE_VAR: &str = "MANDIBLE_TTY_TEST_ROLE";
    const SHIM_VAR: &str = "MANDIBLE_TTY_TEST_SHIM";
    const RESULT_VAR: &str = "MANDIBLE_TTY_TEST_RESULT_FILE";
    const WORKER_ROLE: &str = "session-leader-worker";

    /// Proves the hazard is closed, using a *real* controlling terminal.
    ///
    /// This sandbox has none at all (`AGENTS.md` §3.2 — `open("/dev/tty")`
    /// already fails with `ENXIO` for every process here, fix or no fix),
    /// so a naive version of this test would vacuously pass regardless of
    /// whether `run_inert` does the right thing. To actually exercise the
    /// mechanism, this test spawns a **fresh, single-purpose copy of this
    /// same test binary** (never `fork()`s the already-running,
    /// multi-threaded `cargo test` process itself — racing an arbitrary
    /// other test thread's locks across a raw `fork()` is its own hazard)
    /// that calls the POSIX `login_tty()` primitive to become the leader
    /// of a brand-new session with a real pty as its controlling terminal
    /// — standing in for the interactive `mandible` TUI process a real
    /// user runs this against — and only then, from that single-threaded
    /// worker process, makes the one real call this test is actually
    /// about: `run_inert` against a shim that tries `open("/dev/tty")`
    /// and reports whether it succeeded.
    ///
    /// Verified to fail without the session fix and pass with it (see
    /// this crate's exec-policy work: reverting `spawn.rs`'s `pre_exec`
    /// `setsid` call back to bare `process_group(0)` flips this test from
    /// pass to fail, with the shim reporting `TTY_OPEN:ok`).
    #[test]
    fn probe_cannot_reopen_the_controlling_terminal() {
        // Re-invocation (see `main` below) lands back in this same `#[test]`
        // fn under `--exact`; the role var routes it into worker mode
        // instead of re-running the orchestrator logic recursively.
        if std::env::var(ROLE_VAR).as_deref() == Ok(WORKER_ROLE) {
            run_as_session_leader_worker();
        }

        let dir = tempfile::tempdir().unwrap();
        // Deliberately *not* `if exec 3<>/dev/tty; then ... else ... fi`:
        // measured directly while building this test that `dash` (this
        // sandbox's `/bin/sh`) treats a failed redirection on `exec` (and
        // on an ordinary simple command, e.g. the `:` builtin) as fatal
        // and exits the whole script immediately with its own diagnostic
        // on stderr, even inside an `if` condition — so an `else` branch
        // never runs and never gets the chance to report anything. A
        // shim can't out-cleverness that: it just needs to be a redirect
        // whose *reachability of the next line* is the signal. If
        // `/dev/tty` opens, the script continues and prints the marker;
        // if it doesn't, the shell exits right there and the marker is
        // simply absent — which the worker below treats as the (fixed,
        // expected) outcome, corroborated by checking that the shell's
        // own failure diagnostic mentions the device.
        let shim = write_named_shim(
            dir.path(),
            "tty_prober",
            "#!/bin/sh\n: 3<>/dev/tty\necho TTY_OPEN:ok\n",
        );
        let result_file = dir.path().join("result.txt");

        let exe = std::env::current_exe().expect("path to this test binary");
        let output = std::process::Command::new(&exe)
            .arg("--exact")
            .arg("dev_tty_hazard::probe_cannot_reopen_the_controlling_terminal")
            .arg("--nocapture")
            .arg("--test-threads=1")
            .env(ROLE_VAR, WORKER_ROLE)
            .env(SHIM_VAR, &shim)
            .env(RESULT_VAR, &result_file)
            .output()
            .expect("spawn a fresh worker copy of this test binary");

        let detail = std::fs::read_to_string(&result_file)
            .unwrap_or_else(|_| "(worker wrote no result file)".to_string());
        let harness_output = format!(
            "worker exit status: {:?}\nworker stdout: {}\nworker stderr: {}\nworker detail: {detail}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );

        match output.status.code() {
            Some(0) => {} // TTY_OPEN:failed — the fixed, expected outcome.
            Some(1) => panic!(
                "a probe spawned through run_inert opened /dev/tty — the \
                 controlling terminal was reachable from a descendant, the \
                 exact mechanism behind the reported TUI freeze:\n{harness_output}"
            ),
            other => panic!(
                "worker setup was inconclusive (exit {other:?}), not a \
                 pass — this must not be silently treated as green:\n{harness_output}"
            ),
        }
    }

    /// Becomes a session leader with a real pty as its controlling
    /// terminal, then makes the one production call under test.
    ///
    /// Always exits the process rather than returning — it must never
    /// fall back into the orchestrator's own test-body logic.
    fn run_as_session_leader_worker() -> ! {
        let shim_path = std::env::var(SHIM_VAR).expect("orchestrator sets the shim path");
        let result_file = std::env::var(RESULT_VAR).expect("orchestrator sets the result path");

        let finish = |code: i32, detail: String| -> ! {
            let _ = std::fs::write(&result_file, &detail);
            std::process::exit(code);
        };

        let pty = match nix::pty::openpty(None, None) {
            Ok(p) => p,
            Err(e) => finish(2, format!("openpty failed: {e}")),
        };
        // Deliberately *not* dropped: once nothing holds the master side
        // open, the slave hangs up and `TIOCSCTTY`/`login_tty` on it fails
        // with `EIO` regardless of session state — measured directly while
        // building this test. Leaked into a raw fd for the rest of this
        // short-lived, single-purpose worker process's life; it closes on
        // exit either way.
        let _master_fd: std::os::fd::RawFd = std::os::fd::IntoRawFd::into_raw_fd(pty.master);

        let slave_fd: std::os::fd::RawFd = std::os::fd::IntoRawFd::into_raw_fd(pty.slave);
        // SAFETY: `login_tty(3)` is the standard glibc primitive for
        // exactly this — make the caller a session leader with `fd` as
        // its controlling terminal, then dup it onto 0/1/2 — and this
        // process is a freshly `exec`'d, single-purpose, single-threaded
        // worker (see the orchestrator above), so there is no concurrent
        // Rust state for a post-fork-style call to race. This file is a
        // separate compilation unit from `mandible-extract`'s library
        // crate — its `#![deny(unsafe_code)]` (one audited exception, in
        // `exec/spawn.rs`) does not extend here — and this is test-only
        // scaffolding to manufacture a controlling terminal the sandbox
        // doesn't otherwise have, never part of the exec-safety path
        // itself.
        let rc = unsafe { libc::login_tty(slave_fd) };
        if rc != 0 {
            finish(
                2,
                format!("login_tty failed: {}", std::io::Error::last_os_error()),
            );
        }

        // The one call this whole test exists to check: exactly what
        // every real probe goes through in production.
        let result = run_inert(
            Path::new(&shim_path),
            &InertArgv::HelpLong,
            Duration::from_secs(5),
        );

        match result {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
                let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
                let detail = format!(
                    "stdout={stdout:?} stderr={stderr:?} timed_out={}",
                    out.timed_out
                );
                if stdout.contains("TTY_OPEN:ok") {
                    // The shim's script continued past the redirect, so
                    // /dev/tty genuinely opened: the hazard is present.
                    finish(1, detail);
                } else if stderr.to_lowercase().contains("tty")
                    || stderr.to_lowercase().contains("device")
                {
                    // The marker line was never reached because the
                    // shell aborted on the failed redirect first (dash's
                    // behavior for `exec`/simple-command redirect
                    // failures — see the orchestrator's comment on the
                    // shim), and its own diagnostic corroborates *why*:
                    // this is the fixed, expected outcome.
                    finish(0, detail);
                } else {
                    finish(2, format!("inconclusive — neither the success marker nor a device/tty failure diagnostic: {detail}"));
                }
            }
            Err(e) => finish(2, format!("run_inert errored: {e}")),
        }
    }
}

/// Like [`write_shim`], but with a caller-chosen name and script — needed
/// for the [M-16] D1.3.2 tests below, which must name a shim `pkill` to
/// exercise `HELP_ONLY_PROBE` matching (spec §6 rule 0's file-name check)
/// and need custom argv-dependent behaviour rather than the fixed
/// argv-dumping script `write_shim` always installs.
fn write_named_shim(dir: &Path, name: &str, script: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(script.as_bytes()).unwrap();
    drop(f);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

fn write_shim(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("shim.sh");
    let script = r#"#!/bin/sh
echo "ARGC:$#"
i=0
for a in "$@"; do
    echo "ARGV[$i]:$a"
    i=$((i + 1))
done
echo "ENV_COMPLETE:${COMPLETE:-<unset>}"
echo "ENV_TERM:${TERM:-<unset>}"
echo "ENV_NO_COLOR:${NO_COLOR:-<unset>}"
echo "ENV_LESS:${LESS:-<unset>}"
if IFS= read -r line; then
    echo "STDIN:GOT:$line"
else
    echo "STDIN:EOF"
fi
"#;
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(script.as_bytes()).unwrap();
    drop(f);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

/// Rule 1 (never bare) + rule 2 (only inert shapes): drive every
/// `InertArgv` variant through the real `run_inert` path and confirm the
/// shim actually received a non-empty, well-formed argv matching the
/// variant — not a mocked stand-in.
#[test]
fn every_inert_argv_shape_reaches_the_child_correctly() {
    let dir = tempfile::tempdir().unwrap();
    let shim = write_shim(dir.path());

    let cases: Vec<(InertArgv, Vec<&str>)> = vec![
        (
            InertArgv::CobraComplete {
                words: vec!["pr".to_string()],
            },
            vec!["__complete", "pr"],
        ),
        (
            InertArgv::CompletionScript {
                shell: "zsh".to_string(),
            },
            vec!["completion", "zsh"],
        ),
        (InertArgv::HelpLong, vec!["--help"]),
        (InertArgv::HelpShort, vec!["-h"]),
        (
            InertArgv::HelpSubcommand {
                words: vec!["rebase".to_string()],
            },
            vec!["help", "rebase"],
        ),
    ];

    for (argv, expected) in cases {
        let out = run_inert(&shim, &argv, Duration::from_secs(2)).unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        let argc_line = text.lines().next().unwrap();
        assert_eq!(
            argc_line,
            format!("ARGC:{}", expected.len()),
            "argv={argv:?}"
        );
        for (i, exp) in expected.iter().enumerate() {
            let want = format!("ARGV[{i}]:{exp}");
            assert!(
                text.contains(&want),
                "expected {want:?} in output for {argv:?}:\n{text}"
            );
        }
        // Rule 3: stdin is always /dev/null.
        assert!(
            text.contains("STDIN:EOF"),
            "stdin should be immediately EOF for {argv:?}:\n{text}"
        );
    }
}

/// Rule 6 (sanitized environment) end to end: the `COMPLETE=` variable is
/// set only for the clap probe shapes, and the always-present baseline
/// vars land correctly, through the real spawn path.
#[test]
fn clap_complete_env_shape_carries_its_env_var_to_the_real_child() {
    let dir = tempfile::tempdir().unwrap();
    let shim = write_shim(dir.path());

    let argv = InertArgv::ClapCompleteEnvProbe {
        shell: "zsh".to_string(),
    };
    let out = run_inert(&shim, &argv, Duration::from_secs(2)).unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("ARGV[0]:--"),
        "clap probe must send literal '--', never bare:\n{text}"
    );
    assert!(text.contains("ENV_COMPLETE:zsh"), "{text}");
    assert!(text.contains("ENV_TERM:dumb"), "{text}");
    assert!(text.contains("ENV_NO_COLOR:1"), "{text}");
    assert!(
        text.contains("ENV_LESS:<unset>"),
        "LESS must not leak through:\n{text}"
    );
}

/// Rule 2a: an empty argument the tool could read as its first positional
/// is refused before anything is spawned.
///
/// This is the shape behind the machine reset that motivated rule 0.
/// `ClapCompleteEnvComplete { partial: "" }` renders as `-- ""`; because
/// `--` is the option terminator essentially every getopt program
/// discards, the empty string arrives as the first positional, and a
/// program whose first positional is a pattern reads it as "match
/// everything". Measured: `pkill -- ""` terminated every process in a
/// private PID namespace, pkill included. The never-probe list hid this
/// for thirteen tools while the same argv was still emitted at the rest of
/// PATH, so the fix belongs at the chokepoint, not in a name list.
#[test]
fn empty_first_positional_is_refused_before_spawning() {
    let dir = tempfile::tempdir().unwrap();
    let shim = write_shim(dir.path());

    let refused = InertArgv::ClapCompleteEnvComplete {
        shell: "zsh".to_string(),
        partial: String::new(),
    };
    assert_eq!(refused.args(), vec!["--".to_string(), String::new()]);

    let err = run_inert(&shim, &refused, Duration::from_secs(2))
        .expect_err("`-- \"\"` must be refused, not spawned");
    assert!(
        err.to_string().contains("empty argument"),
        "unexpected error: {err}"
    );

    // The safe expression of the same request must still reach the child:
    // `--` alone, which `ClapCompleteEnvProbe` exists to produce.
    let allowed = InertArgv::ClapCompleteEnvProbe {
        shell: "zsh".to_string(),
    };
    let out = run_inert(&shim, &allowed, Duration::from_secs(2)).unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("ARGC:1"), "{text}");
    assert!(text.contains("ARGV[0]:--"), "{text}");
}

/// The one empty argument that is allowed, and why: cobra's completion
/// word is protocol-required — `docker __complete` without it fails with
/// "requires at least 1 arg(s), only received 0" and native detection
/// collapses for every cobra tool. It is safe for a reason the chokepoint
/// can check: it is never the first positional, always shielded behind the
/// `__complete` sentinel, which a non-cobra tool rejects rather than acts
/// on.
#[test]
fn cobra_completion_word_may_be_empty_because_a_sentinel_guards_it() {
    let dir = tempfile::tempdir().unwrap();
    let shim = write_shim(dir.path());

    let argv = InertArgv::CobraComplete {
        words: vec![String::new()],
    };
    let out = run_inert(&shim, &argv, Duration::from_secs(2))
        .expect("cobra's empty completion word must still be permitted");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("ARGC:2"), "{text}");
    assert!(text.contains("ARGV[0]:__complete"), "{text}");

    // But the sentinel must genuinely be the guard: an empty *first*
    // argument is refused even for this variant.
    let unguarded = InertArgv::HelpSubcommand {
        words: vec![String::new()],
    };
    assert!(
        run_inert(&shim, &unguarded, Duration::from_secs(2)).is_err(),
        "`help \"\"` has no sentinel shielding the empty word"
    );
}

// --- [M-16] sub-case (a): the `-h` fallback for a man-shaped subcommand
// --- probe, D1.3.2's "both halves matter" shim suite ---
//
// These drive `HelpTextTier::extract_node` — not `run_inert` directly —
// against real shim binaries, through the tier's actual `Probe`
// construction (`HelpTextTier::default()` uses the live `LiveProbe`, so
// every probe below is a real subprocess spawn through the real
// `run_inert` chokepoint, exactly like the tests above). A rendered man
// page is built from the same banner shape `git bisect --help` actually
// produces (identical `NAME(section)` token at both margins around a
// centred title) — see `help_text::sections::is_man_page_banner`'s own
// tests for the real fixture this is modeled on.

/// A minimal man-page banner in the exact shape `looks_like_man_page`
/// recognizes: identical `NAME(1)` token at both margins, a centred title
/// between them.
fn man_page_banner(name: &str) -> String {
    let tag = format!("{}(1)", name.to_uppercase());
    format!(
        "{tag}                Some Manual                {tag}\n\nNAME\n     {name} - a thing\n"
    )
}

/// Half one: a permitted tool's subcommand whose `--help` renders a man
/// The verbatim view (`t`) must fetch **the document the parse read**.
///
/// Its whole purpose is letting a reader check our reading against the
/// author's own bytes — which only works if both are the same bytes. When
/// [M-16] sub-case (a) fires the parse came from `-h`, not from the man
/// page `--help` returned, so a raw fetch that re-probed without the same
/// attestation would show a different document than the tree came from and
/// silently answer a question nobody asked. That shipped briefly:
/// `raw_help` hardcoded `heading_attested: false`.
#[test]
fn raw_help_fetches_the_same_document_the_parse_read() {
    let dir = tempfile::tempdir().unwrap();
    let script = format!(
        r#"#!/bin/sh
if [ "$1" = "sub" ] && [ "$2" = "--help" ]; then
    printf '%s' '{banner}'
    exit 0
fi
if [ "$1" = "sub" ] && [ "$2" = "-h" ]; then
    echo "Usage: manthing sub [options]"
    echo ""
    echo "Options:"
    echo "  --amend      Amend the previous thing"
    exit 0
fi
echo "unexpected argv: $@" >&2
exit 1
"#,
        banner = man_page_banner("manthing-sub").replace('\'', "'\\''")
    );
    let shim = write_named_shim(dir.path(), "manthing", &script);
    let tool = ResolvedTool {
        name: "manthing".to_string(),
        path: Some(shim.clone()),
        version: None,
    };
    let path = ["manthing".to_string(), "sub".to_string()];
    let attested = NodeHints {
        heading_attested: true,
    };

    let (raw, flag) = mandible_extract::help_text::raw_help(&tool, &path, attested)
        .expect("the shim answers both probes");
    // The pane labels itself from this, so a wrong value is a false claim
    // about where the bytes came from, not a cosmetic slip.
    assert_eq!(flag, "-h", "raw help must report the argv it actually ran");
    let joined: String = raw
        .iter()
        .map(|t| t.as_str().to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        joined.contains("--amend"),
        "raw help must show the -h document the parse actually read: {joined}"
    );
    assert!(
        !joined.contains("MANTHING-SUB(1)"),
        "raw help showed the man page the parse discarded — the verbatim \
         view is answering the wrong question: {joined}"
    );
}

/// page must trigger the `-h` fallback, and the fallback's output — an
/// ordinary option table — must actually be what the node parses to,
/// rather than the man page staying as verbatim degradation.
#[test]
fn man_shaped_subcommand_help_triggers_the_dash_h_fallback_when_permitted() {
    let dir = tempfile::tempdir().unwrap();
    let script = format!(
        r#"#!/bin/sh
if [ "$1" = "sub" ] && [ "$2" = "--help" ]; then
    printf '%s' '{banner}'
    exit 0
fi
if [ "$1" = "sub" ] && [ "$2" = "-h" ]; then
    touch "$0.dash_h_ran"
    echo "Usage: manthing sub [options]"
    echo ""
    echo "Options:"
    echo "  --amend      Amend the previous thing"
    echo "  --dry-run    Do not actually do anything"
    exit 0
fi
echo "unexpected argv: $@" >&2
exit 1
"#,
        banner = man_page_banner("manthing-sub").replace('\'', "'\\''")
    );
    let shim = write_named_shim(dir.path(), "manthing", &script);

    let tier = HelpTextTier::default();
    let tool = ResolvedTool {
        name: "manthing".to_string(),
        path: Some(shim.clone()),
        version: None,
    };
    let node = tier
        .extract_node(
            &tool,
            &["manthing".to_string(), "sub".to_string()],
            NodeHints {
                heading_attested: true,
            },
        )
        .expect("the shim always answers one of the two probes it's asked for");

    assert!(
        node.unparsed.is_empty(),
        "node degraded to verbatim instead of using the -h fallback's real flags: {node:?}"
    );
    let long_flags: Vec<&str> = node.flags.iter().filter_map(|f| f.long()).collect();
    assert!(long_flags.contains(&"amend"), "{long_flags:?}");
    assert!(long_flags.contains(&"dry-run"), "{long_flags:?}");
    assert!(
        dir.path().join("manthing.dash_h_ran").exists(),
        "the -h fallback's marker was never written — -h was never actually invoked"
    );
}

/// Half two: a shim named like a never-probe tool (spec §6 rule 0,
/// `HELP_ONLY_PROBE`) must never receive the `-h` fallback, even in a
/// scenario shaped to trigger it. `pkill`'s subcommand-path `--help` probe
/// (`InertArgv::HelpLongForPath` with non-empty words renders to
/// `[..words, "--help"]`, never exactly `["--help"]`) is itself already
/// refused by `run_inert`'s chokepoint before this tier's new fallback
/// logic ever runs — which is the strongest form of "cannot route around
/// it": the fallback code path is never even reached, because the probe
/// that would have supplied it man-shaped text to react to never completes.
/// The shim unconditionally leaves a marker on every invocation, so this
/// also proves the refusal happens before any spawn, not merely before the
/// tier acts on a result.
#[test]
fn never_probe_named_shim_never_receives_the_dash_h_fallback_even_when_man_shaped() {
    let dir = tempfile::tempdir().unwrap();
    let shim = write_named_shim(
        dir.path(),
        "pkill",
        "#!/bin/sh\ntouch \"$0.ran\"\necho ran\n",
    );

    let tier = HelpTextTier::default();
    let tool = ResolvedTool {
        name: "pkill".to_string(),
        path: Some(shim.clone()),
        version: None,
    };
    let result = tier.extract_node(
        &tool,
        &["pkill".to_string(), "sub".to_string()],
        NodeHints {
            heading_attested: true,
        },
    );

    assert!(
        matches!(
            result,
            Err(ExtractError::Exec(ExecError::RefusedUnsafeTool { .. }))
        ),
        "expected the never-probe list to refuse the subcommand `--help` probe outright, got {result:?}"
    );
    assert!(
        !dir.path().join("pkill.ran").exists(),
        "the never-probe shim was executed at all — refusal did not happen before spawn"
    );
}

/// Half three, now closed all the way: a subcommand word that did *not*
/// come from a structural source (`hints.heading_attested: false`) must
/// receive **no probe at all** — not `--help`, not the `-h` fallback of any
/// kind. This is the provenance gate spec §6 rule 0's closing paragraph
/// calls for, closed in full: `HelpTextTier::extract_node` must decline
/// with an error rather than send `<words...> --help` for a word that may
/// be a fabrication, and the tree must not silently gain an
/// empty-but-successful node in its place (spec §5.3 — a declined probe is
/// a recorded per-node failure, not a quiet "found nothing").
///
/// This test previously asserted only that the *`-h`* fallback was
/// withheld while still expecting the `--help` probe itself to fire and be
/// read for man-page structure — that was the pre-existing, then-still-open
/// half of the gap this change closes. The shim now marks *both* branches
/// it could take, so this proves neither one was ever reached: refusal
/// happens before any spawn, matching the never-probe list's own contract
/// (`never_probe_named_shim_never_receives_the_dash_h_fallback_even_when_man_shaped`,
/// above, whose "before any spawn" assertion this mirrors for the new
/// general-purpose gate rather than the closed thirteen-tool list).
///
/// Verified to fail without the fix (see
/// `attestation_gate_is_load_bearing_probe_would_have_fired_without_it`
/// below, which exercises the identical shim and hint through the
/// pre-gate code path directly).
#[test]
fn non_attested_subcommand_word_is_never_probed_at_all() {
    let dir = tempfile::tempdir().unwrap();
    let shim = write_named_shim(dir.path(), "unattested", &unattested_probe_script());

    let tier = HelpTextTier::default();
    let tool = ResolvedTool {
        name: "unattested".to_string(),
        path: Some(shim.clone()),
        version: None,
    };
    let result = tier.extract_node(
        &tool,
        &["unattested".to_string(), "sub".to_string()],
        NodeHints {
            heading_attested: false,
        },
    );

    assert!(
        result.is_err(),
        "a non-attested subcommand word must be declined, not silently extracted: {result:?}"
    );
    assert!(
        !dir.path().join("unattested.help_ran").exists(),
        "the --help probe ran despite the word not being heading_attested"
    );
    assert!(
        !dir.path().join("unattested.dash_h_ran").exists(),
        "the -h fallback ran despite the word not being heading_attested"
    );
}

/// Half four, the positive case this suite was missing entirely: an
/// **attested** subcommand word must still be probed with `--help` and
/// have its real flags recovered — proving the new gate in
/// `non_attested_subcommand_word_is_never_probed_at_all` above restricts
/// exactly the unattested case, and does not accidentally withhold the
/// probe from an ordinary, legitimately-discovered subcommand.
#[test]
fn attested_subcommand_word_is_still_probed_with_dash_dash_help() {
    let dir = tempfile::tempdir().unwrap();
    let script = r#"#!/bin/sh
if [ "$1" = "sub" ] && [ "$2" = "--help" ]; then
    touch "$0.help_ran"
    echo "Usage: attested sub [options]"
    echo ""
    echo "Options:"
    echo "  --amend  Amend the previous thing"
    exit 0
fi
echo "unexpected argv: $@" >&2
exit 1
"#;
    let shim = write_named_shim(dir.path(), "attested", script);

    let tier = HelpTextTier::default();
    let tool = ResolvedTool {
        name: "attested".to_string(),
        path: Some(shim.clone()),
        version: None,
    };
    let node = tier
        .extract_node(
            &tool,
            &["attested".to_string(), "sub".to_string()],
            NodeHints {
                heading_attested: true,
            },
        )
        .expect("an attested word's --help probe must still run and succeed");

    assert!(
        dir.path().join("attested.help_ran").exists(),
        "the --help probe never ran for an attested word"
    );
    let long_flags: Vec<&str> = node.flags.iter().filter_map(|f| f.long()).collect();
    assert!(long_flags.contains(&"amend"), "{long_flags:?}");
}

/// Mirrors `non_attested_subcommand_word_is_never_probed_at_all`, but for
/// spec §7 Tier B's headingless-invocation-table recognizer specifically —
/// `mandible-extract/src/help_text/sections.rs::scan_headingless_invocation_table`.
/// A node it recovers is existence-attested (its name is checked, not
/// guessed) but deliberately **not** probe-eligible: `heading_attested`
/// must stay `false` and its name must never reach the shim as argv, even
/// though `invocation_attested` is `true`. Driven through the real root
/// extraction first (not a hand-built `CommandNode`), so this proves the
/// bit the recognizer actually sets, not merely the bit a hand-written test
/// hoped it would set.
#[test]
fn headingless_invocation_table_child_is_never_heading_attested_and_never_probed() {
    let dir = tempfile::tempdir().unwrap();
    let script = r#"#!/bin/sh
if [ "$1" = "--help" ]; then
    cat <<'HELPEOF'
Usage: btrfslike [options]

Options:
  --version   print version string

    btrfslike device add <path>
        Add a device
    btrfslike device remove <path>
        Remove a device
HELPEOF
    exit 0
fi
echo "unexpected argv: $@" >&2
exit 1
"#;
    let shim = write_named_shim(dir.path(), "btrfslike", script);

    let tier = HelpTextTier::default();
    let tool = ResolvedTool {
        name: "btrfslike".to_string(),
        path: Some(shim.clone()),
        version: None,
    };
    let root = tier
        .extract_node(
            &tool,
            &["btrfslike".to_string()],
            NodeHints {
                heading_attested: true,
            },
        )
        .expect("root probe must succeed");

    let device = root
        .subcommands
        .iter()
        .find(|n| n.name == "device")
        .unwrap_or_else(|| panic!("no `device` child recovered: {:?}", root.subcommands));
    assert!(
        !device.heading_attested,
        "a headingless-invocation-table node must never be heading_attested"
    );
    assert!(
        device.invocation_attested,
        "a headingless-invocation-table node must carry the second attestation bit"
    );

    // Feed the runner's own hint-derivation rule (NodeHints::heading_attested
    // mirrors the node's own bit — see `runner.rs`) into a deeper probe for
    // this exact node, and confirm it is refused before any argv reaches
    // the shim.
    let result = tier.extract_node(
        &tool,
        &["btrfslike".to_string(), "device".to_string()],
        NodeHints {
            heading_attested: device.heading_attested,
        },
    );
    assert!(
        result.is_err(),
        "a headingless-invocation-table name must be declined, not probed: {result:?}"
    );
}

/// Shared with the "prove the negative fails without the fix" check below:
/// a shim that marks which of the two probes it received, man-shaped on
/// `--help` exactly like `man_shaped_subcommand_help_triggers_the_dash_h_fallback_when_permitted`'s
/// shim, so the same script demonstrates both "no probe of any kind
/// reaches this binary" (with the fix) and "the `--help` probe reaches it
/// and returns man-page structure" (without the fix, reverting to the old
/// code path that only gated the `-h` fallback).
fn unattested_probe_script() -> String {
    format!(
        r#"#!/bin/sh
if [ "$1" = "sub" ] && [ "$2" = "--help" ]; then
    touch "$0.help_ran"
    printf '%s' '{banner}'
    exit 0
fi
if [ "$1" = "sub" ] && [ "$2" = "-h" ]; then
    touch "$0.dash_h_ran"
    echo "Usage: unattested sub [options]"
    echo ""
    echo "Options:"
    echo "  --amend  Amend the previous thing"
    exit 0
fi
echo "unexpected argv: $@" >&2
exit 1
"#,
        banner = man_page_banner("unattested-sub").replace('\'', "'\\''")
    )
}

/// Proves `non_attested_subcommand_word_is_never_probed_at_all` is not a
/// vacuous assertion: this test drives the exact pre-fix code path (no
/// attestation gate in `probe_help_text_reporting_flag`, matching what
/// `HelpTextTier::extract_node` did before this change) against the same
/// shim and hint, and confirms the probe *would* have fired and produced a
/// verbatim-degraded node — the failure mode the fix closes. Written
/// directly against `run_inert`/`InertArgv` (not through `HelpTextTier`,
/// which no longer exposes the ungated path) so this stays meaningful even
/// as the tier's own internals change, and so a reviewer can compare this
/// test's assertions to the one above line for line.
#[test]
fn attestation_gate_is_load_bearing_probe_would_have_fired_without_it() {
    let dir = tempfile::tempdir().unwrap();
    let shim = write_named_shim(dir.path(), "unattested", &unattested_probe_script());

    // The exact argv `HelpLongForPath` renders for this path, sent the way
    // the pre-fix `probe_help_text_reporting_flag` sent it: unconditionally,
    // with no regard for `hints.heading_attested`.
    let out = run_inert(
        &shim,
        &InertArgv::HelpLongForPath {
            words: vec!["sub".to_string()],
        },
        Duration::from_secs(2),
    )
    .expect("run_inert itself has no attestation concept — it would have spawned this");

    assert!(
        dir.path().join("unattested.help_ran").exists(),
        "the ungated probe did not even reach the shim; this test no longer demonstrates \
         what it claims to"
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("UNATTESTED-SUB(1)"),
        "expected the man-page banner the fix now prevents this argv from ever fetching: {text}"
    );
}

// --- spec §6 rule 2b: the truncation-confession follow-up
// (`InertArgv::HelpExpand`), end to end through `HelpTextTier::extract_node`
// against real shim binaries — the same discipline the [M-16] section above
// uses (real spawns through the real `run_inert` chokepoint), for the exact
// reason AGENTS.md §3.1 gives: a tier that builds the wrong argv should miss
// here, not pass because a test mocked the probe.

fn attested() -> NodeHints {
    NodeHints {
        heading_attested: true,
    }
}

/// A shim whose root `--help` confesses (curl's own wording, `"--help
/// all"`) and, when followed, prints a strictly larger flag set.
fn confessing_shim_script(short_flags: &str, long_flags: &str) -> String {
    format!(
        r#"#!/bin/sh
if [ "$#" -eq 1 ] && [ "$1" = "--help" ]; then
    touch "$0.root_ran"
    printf '%s' '{short_flags}'
    exit 0
fi
if [ "$#" -eq 2 ] && [ "$1" = "--help" ] && [ "$2" = "all" ]; then
    touch "$0.expand_ran"
    printf '%s' '{long_flags}'
    exit 0
fi
touch "$0.other_ran_$#"
echo "unexpected argv: $@" >&2
exit 1
"#,
    )
}

/// Half one, the positive case: a shim that confesses **does** get the
/// expansion probe, and the tree it produces reflects the *expanded*
/// document — more flags than the summary alone ever had.
#[test]
fn a_confessing_shim_is_followed_and_the_tree_reflects_the_expanded_document() {
    let dir = tempfile::tempdir().unwrap();
    let short = "Usage: widget [options]\n\n -v, --verbose  Be louder\n\nFor all options use the manual or \"--help all\".\n";
    let long = "Usage: widget [options]\n\n -v, --verbose  Be louder\n -q, --quiet    Be quieter\n --extra        Only in the expanded document\n";
    let shim = write_named_shim(dir.path(), "widget", &confessing_shim_script(short, long));

    let tier = HelpTextTier::default();
    let tool = ResolvedTool {
        name: "widget".to_string(),
        path: Some(shim.clone()),
        version: None,
    };
    let node = tier
        .extract_node(&tool, &["widget".to_string()], attested())
        .expect("both probes are answered");

    assert!(
        dir.path().join("widget.root_ran").exists(),
        "the root --help probe never ran"
    );
    assert!(
        dir.path().join("widget.expand_ran").exists(),
        "the confession was detected but never followed — the expansion probe never ran"
    );
    let long_flags: Vec<&str> = node.flags.iter().filter_map(|f| f.long()).collect();
    assert!(
        long_flags.contains(&"extra"),
        "the tree must reflect the expanded document, not the 2-flag summary: {long_flags:?}"
    );
    let confession = node
        .confession
        .as_ref()
        .expect("a confession was printed and must be recorded");
    assert_eq!(confession.word, "all");
    assert_eq!(confession.flag, "--help");
    assert!(
        confession.followed,
        "the expansion succeeded and must say so"
    );
}

/// Half two, the negative case: a shim whose text merely *mentions*
/// `--help` in passing — no quoted directive — must never receive the
/// expansion probe at all. Proves the negative actually fails without the
/// grammar being strict: a looser match (e.g. any line containing both
/// `--help` and a following word) would fire on this text too.
#[test]
fn a_shim_that_merely_mentions_help_in_passing_is_not_followed() {
    let dir = tempfile::tempdir().unwrap();
    let script = r#"#!/bin/sh
if [ "$#" -eq 1 ] && [ "$1" = "--help" ]; then
    touch "$0.root_ran"
    echo "Usage: mentioner [options]"
    echo ""
    echo " -v, --verbose  Be louder"
    echo ""
    echo "Run with --help for more information."
    exit 0
fi
touch "$0.expand_ran"
echo "unexpected argv: $@" >&2
exit 1
"#;
    let shim = write_named_shim(dir.path(), "mentioner", script);

    let tier = HelpTextTier::default();
    let tool = ResolvedTool {
        name: "mentioner".to_string(),
        path: Some(shim.clone()),
        version: None,
    };
    let node = tier
        .extract_node(&tool, &["mentioner".to_string()], attested())
        .expect("the root probe is answered");

    assert!(dir.path().join("mentioner.root_ran").exists());
    assert!(
        !dir.path().join("mentioner.expand_ran").exists(),
        "prose that merely mentions --help in passing must never trigger the expansion probe"
    );
    assert_eq!(
        node.confession, None,
        "no directive was printed, so nothing should be recorded"
    );
}

/// Half three: no chaining. A shim whose *expanded* document also
/// confesses must not be probed a third time — the follow-up's own text is
/// simply never checked for a further directive.
#[test]
fn an_expanded_document_that_also_confesses_is_not_probed_a_third_time() {
    let dir = tempfile::tempdir().unwrap();
    let script = r#"#!/bin/sh
if [ "$#" -eq 1 ] && [ "$1" = "--help" ]; then
    touch "$0.root_ran"
    echo "Usage: nested [options]"
    echo ""
    echo " -v, --verbose  Be louder"
    echo ""
    echo "For all options use the manual or \"--help all\"."
    exit 0
fi
if [ "$#" -eq 2 ] && [ "$1" = "--help" ] && [ "$2" = "all" ]; then
    touch "$0.expand_ran"
    echo "Usage: nested [options]"
    echo ""
    echo " -v, --verbose  Be louder"
    echo " -q, --quiet    Be quieter"
    echo ""
    echo "For all options use the manual or \"--help all\"."
    exit 0
fi
touch "$0.third_probe_ran"
echo "unexpected argv: $@" >&2
exit 1
"#;
    let shim = write_named_shim(dir.path(), "nested", script);

    let tier = HelpTextTier::default();
    let tool = ResolvedTool {
        name: "nested".to_string(),
        path: Some(shim.clone()),
        version: None,
    };
    let node = tier
        .extract_node(&tool, &["nested".to_string()], attested())
        .expect("both probes this test cares about are answered");

    assert!(dir.path().join("nested.root_ran").exists());
    assert!(
        dir.path().join("nested.expand_ran").exists(),
        "the first, legitimate expansion must still happen"
    );
    assert!(
        !dir.path().join("nested.third_probe_ran").exists(),
        "a confession inside the expanded document triggered a second \
         expansion — chaining must never happen"
    );
    let long_flags: Vec<&str> = node.flags.iter().filter_map(|f| f.long()).collect();
    assert!(long_flags.contains(&"quiet"), "{long_flags:?}");
    let confession = node.confession.as_ref().expect("must be recorded");
    assert!(confession.followed);
}

/// Half four: rule 0 still wins. A shim named like a never-probe tool
/// (spec §6 rule 0) that confesses must not receive the expansion argv —
/// `run_inert`'s own chokepoint refuses `HelpExpand`'s
/// `["--help", "all"]` for the exact same reason it refuses every other
/// non-`["--help"]` shape, with no special case anywhere in
/// `help_text::confession` or `HelpTextTier`.
#[test]
fn a_never_probe_named_shim_that_confesses_does_not_receive_the_expansion_argv() {
    let dir = tempfile::tempdir().unwrap();
    let shim = write_named_shim(dir.path(), "pkill", &confessing_shim_script(
        "Usage: pkill [options]\n\n -f, --full  Match against full argument list\n\nFor all options use the manual or \"--help all\".\n",
        "Usage: pkill [options]\n\n -f, --full  Match against full argument list\n --extra     Only in the expanded document\n",
    ));

    let tier = HelpTextTier::default();
    let tool = ResolvedTool {
        name: "pkill".to_string(),
        path: Some(shim.clone()),
        version: None,
    };
    // The root probe itself (`InertArgv::HelpLongForPath { words: [] }`)
    // renders to exactly `["--help"]`, the one shape rule 0 permits, so
    // the confession is genuinely detected here — this test is about what
    // happens *after* detection, not about the root probe being refused.
    let node = tier
        .extract_node(&tool, &["pkill".to_string()], attested())
        .expect("the root --help probe is the one permitted shape and must succeed");

    assert!(dir.path().join("pkill.root_ran").exists());
    assert!(
        !dir.path().join("pkill.expand_ran").exists(),
        "rule 0 must refuse the expansion argv before the shim ever sees it"
    );
    let long_flags: Vec<&str> = node.flags.iter().filter_map(|f| f.long()).collect();
    assert!(
        !long_flags.contains(&"extra"),
        "the tree must still reflect the un-expanded summary: {long_flags:?}"
    );
    let confession = node.confession.as_ref().expect("must be recorded");
    assert_eq!(confession.word, "all");
    assert!(
        !confession.followed,
        "rule 0's refusal must be recorded as an unfollowed confession, \
         which is what caps the status at `incomplete`"
    );
}

// --- Rule 4's other half: a probe is not complete while its descendants
// --- are alive ---
//
// Session-and-group reaping is not enough, and this module proves that
// rather than asserting it. `run_inert` already gives every probe its own
// session and SIGKILLs its process *group* on timeout. A program that
// daemonises walks out of both on its own — `fork`, parent exits, child
// `setsid`s — and from that instant nothing about the survivor's group,
// session, or parent points back at the probe that started it.
//
// Measured on a developer box: 622 processes left behind by sweeps, the
// oldest five days old (`blkmapd` x148, `rpc.idmapd` x144, `rpc.gssd`
// x144, plus `sudo_logsrvd` listening on `0.0.0.0:30343`, `guacd` on
// `127.0.0.1:4822`, and `pam-auth-update` burning a core for three days).
// And *not* a hang: all 2,302 `probe-start` lines in a traced sweep had a
// matching `probe-done`, so every probe involved returned normally and a
// tighter timeout would have changed nothing.
#[cfg(target_os = "linux")]
mod double_fork_escape {
    use super::*;

    /// The daemon body, run under `setsid` by the shim below. Records its
    /// own pid and session id, then `exec`s a long sleep, so what survives
    /// is a real, unrelated program image rather than the shell that
    /// started it — the shape `blkmapd` and friends actually present.
    const DAEMON: &str = r#"#!/bin/sh
echo $$ > "$1"
awk '{print $6}' /proc/$$/stat > "$2"
exec sleep 300
"#;

    /// Starts the daemon detached and then **exits successfully**, which
    /// is the whole point: this shim is not slow, does not hang, and never
    /// reaches a timeout. It waits only long enough for the daemon to have
    /// recorded itself, so the test is not racing the shell.
    fn shim_script(daemon: &Path, pid_file: &Path, sid_file: &Path) -> String {
        format!(
            "#!/bin/sh\n\
             setsid {daemon} {pid_file} {sid_file} </dev/null >/dev/null 2>&1 &\n\
             n=0\n\
             while [ ! -s {sid_file} ] && [ $n -lt 200 ]; do sleep 0.05; n=$((n+1)); done\n\
             echo daemon-started\n\
             exit 0\n",
            daemon = daemon.display(),
            pid_file = pid_file.display(),
            sid_file = sid_file.display(),
        )
    }

    /// This process's own session id, from `/proc/self/stat` — field 6,
    /// read from after the **last** `)` because field 2 (`comm`) may
    /// itself contain spaces and parentheses.
    fn session_of_self() -> i32 {
        let stat = std::fs::read_to_string("/proc/self/stat").expect("/proc/self/stat");
        let after_comm = &stat[stat.rfind(')').expect("comm is parenthesised") + 1..];
        after_comm
            .split_whitespace()
            .nth(3)
            .expect("session field")
            .parse()
            .expect("session id is numeric")
    }

    fn is_alive(pid: i32) -> bool {
        Path::new(&format!("/proc/{pid}")).exists()
    }

    fn start_a_daemon_through_a_probe(dir: &Path) -> (i32, i32) {
        let pid_file = dir.join("daemon.pid");
        let sid_file = dir.join("daemon.sid");
        let daemon = write_named_shim(dir, "daemonise.sh", DAEMON);
        let shim = write_named_shim(
            dir,
            "starts_a_daemon.sh",
            &shim_script(&daemon, &pid_file, &sid_file),
        );

        let out = run_inert(&shim, &InertArgv::HelpLong, Duration::from_secs(20)).unwrap();
        assert!(
            !out.timed_out,
            "the shim exits 0 on its own — this leak is not a timeout, and a test \
             that reached one would be measuring the wrong mechanism"
        );
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            "daemon-started",
            "the shim must have actually started the daemon"
        );

        let pid = std::fs::read_to_string(&pid_file)
            .expect("the daemon recorded its pid")
            .trim()
            .parse()
            .expect("pid is numeric");
        let sid = std::fs::read_to_string(&sid_file)
            .expect("the daemon recorded its session")
            .trim()
            .parse()
            .expect("session id is numeric");
        (pid, sid)
    }

    /// **The core regression.** A probe that starts a double-forked,
    /// `setsid`ed daemon and then exits 0 must leave nothing behind.
    ///
    /// Three assertions, and all are load-bearing:
    ///
    /// - The probe returned normally. A leak reproduced via a timeout
    ///   would be measuring the process-group kill that already existed.
    /// - The survivor really did escape: its session id is its own pid and
    ///   is not this process's session, so it is provably outside every
    ///   process group and session that kill can reach. Without this the
    ///   test could pass vacuously and prove nothing about the new
    ///   mechanism.
    /// - It is nonetheless gone by the time `run_inert` returns — not
    ///   merely signalled but *reaped*, since `/proc/<pid>` is absent
    ///   rather than holding a zombie, which is what says this process
    ///   took responsibility for the descendant it adopted instead of
    ///   handing a corpse to init.
    ///
    /// Verified to fail without the fix: with the `reap_probe_descendants`
    /// call in `exec/spawn.rs` commented out, the daemon survives and this
    /// test reports the surviving pid.
    #[test]
    fn a_daemon_that_double_forks_out_of_the_probes_session_does_not_survive_it() {
        let dir = tempfile::tempdir().unwrap();
        let (daemon_pid, daemon_sid) = start_a_daemon_through_a_probe(dir.path());

        assert_eq!(
            daemon_sid, daemon_pid,
            "the daemon should lead its own session — otherwise it never escaped \
             and this test proves nothing"
        );
        assert_ne!(
            daemon_sid,
            session_of_self(),
            "the daemon should be outside this process's session entirely"
        );

        assert!(
            !is_alive(daemon_pid),
            "a daemon started by a probe outlived it: pid {daemon_pid} is still \
             alive (session {daemon_sid}), which is the 622-process leak"
        );
    }

    /// The precision half: reaping must never reach a process this
    /// invocation did not start.
    ///
    /// Adoption alone cannot tell the difference — once this process is a
    /// child subreaper, *everything* orphaned anywhere beneath it becomes
    /// its child, including things no probe ever touched. The
    /// per-invocation token in the probe's environment is what makes the
    /// distinction, and this is the test that fails if that check is ever
    /// traded for a blunt "kill anything adopted".
    ///
    /// The bystander is a child of the *test process*, started outside
    /// `run_inert` and so carrying no token at all — the same relationship
    /// a concurrently running probe's live child has to the probe doing
    /// the reaping.
    #[test]
    fn reaping_leaves_a_process_this_probe_did_not_start_alone() {
        let dir = tempfile::tempdir().unwrap();
        let mut bystander = std::process::Command::new("sleep")
            .arg("30")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn a bystander");
        let bystander_pid = bystander.id() as i32;

        let (daemon_pid, _) = start_a_daemon_through_a_probe(dir.path());
        assert!(
            !is_alive(daemon_pid),
            "the probe's own daemon is still reaped"
        );

        let alive = is_alive(bystander_pid);
        let _ = bystander.kill();
        let _ = bystander.wait();
        assert!(
            alive,
            "the reap killed pid {bystander_pid}, which no probe started — the token \
             check is what keeps this from being an indiscriminate sweep of every \
             adopted process"
        );
    }
}
