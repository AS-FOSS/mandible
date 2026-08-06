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
