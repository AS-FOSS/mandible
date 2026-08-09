//! End-to-end proof of spec §6 rules 1-3, run through the actual sanctioned
//! path (`mandible_extract::exec::run_inert`), against a shim binary that
//! logs exactly what it was invoked with. Spec §13.3 calls this out
//! explicitly as a required test class ("Execution-policy tests: a shim
//! binary logs argv/env; any invocation outside the allowlist fails the
//! suite.") and as the fix for a specific prior bug ("Real-argv tests":
//! a mocked probe can pass while the real argv construction is broken).

use mandible_extract::exec::{run_inert, InertArgv};
use std::io::Write;
use std::time::Duration;

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
