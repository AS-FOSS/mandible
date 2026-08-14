//! Proves Tier C's gate through the real, unmodified production path:
//! `CompletionScriptTier::default()` (the live `LiveProbe`, the real
//! `run_inert` chokepoint, real argv construction) against real shim
//! binaries on disk. The sibling of `tests/native_cobra_gate.rs`, which
//! does the same job for Tier E's `__complete`, and arranged the same way
//! for the same reason (spec §13.3's "real-argv tests": a mocked probe can
//! pass while the real gate is broken).
//!
//! **What the gate is for.** `completion` and `zsh` are a framework
//! *protocol's* words, not universal ones. Sent to a program that speaks
//! that protocol they are a subcommand invocation; sent to one that does
//! not, they are two positionals — semantically the bare invocation spec
//! §6 rule 1 forbids, arriving through rule 2's closed list of "inert
//! shapes". That list's premise is that the receiving tool *parses argv*.
//! A daemon that ignores argv and starts anyway falsifies it, and 437 of
//! the 622 processes a sweep left behind on a developer box came in
//! through exactly this door (219 `completion zsh` + 218 `completion
//! bash`): `blkmapd`, `rpc.idmapd`, `rpc.gssd`, plus `guacd` holding
//! `127.0.0.1:4822` for five days and `sudo_logsrvd` holding
//! `0.0.0.0:30343`. The same probe made `docker-proxy` attempt to bind
//! `0.0.0.0:-1` and write its startup error to a terminal it did not own,
//! tripping the sweep's PTY canary.
//!
//! **Both halves are tested, deliberately.** A gate that refused
//! everything would satisfy the safety test alone while quietly deleting
//! the tier — the exact failure mode spec §6 rule 0's own test guards
//! against ("silently refusing the permitted shape would quietly undo the
//! coverage this rule now allows").

use mandible_extract::completion_script::CompletionScriptTier;
use mandible_extract::{ExtractionTier, ResolvedTool};
use std::io::Write;
use std::path::Path;

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

/// A shim that would answer `completion <shell>` perfectly well — with a
/// real `_arguments` line this tier's parser recovers a flag from — and
/// records the fact that the word reached it.
///
/// Its `--help` is the shape of the tools that actually leaked: a
/// daemon-style usage line, flags only, no command table anywhere. If the
/// gate were absent, bypassed, or merely broken, this shim would be
/// detected and `completion_ran` would exist.
fn shim(help: &str) -> String {
    format!(
        "#!/bin/sh\n\
         case \"$1\" in\n\
         \x20 --help) cat <<'EOF'\n{help}\nEOF\n ;;\n\
         \x20 completion) touch \"$0.completion_ran\"\n\
         \x20   printf '_mytool() {{\\n_arguments %s\\n}}\\n' \"'--verbose[be loud]'\" ;;\n\
         \x20 *) touch \"$0.other_ran\" ;;\n\
         esac\n"
    )
}

const HELP_WITHOUT_A_COMMAND_TABLE: &str =
    "Usage: widgetd [-h] [-d level] [-f]\n  -h  print help\n  -f  stay in the foreground";

const HELP_WITH_A_COMMAND_TABLE: &str =
    "Usage: widgetctl <COMMAND>\n\nCommands:\n  completion  Generate a completion script\n  help        Print this message";

/// The safety property itself.
#[test]
fn a_tool_with_no_completion_evidence_never_receives_a_completion_argv() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_named_shim(dir.path(), "widgetd", &shim(HELP_WITHOUT_A_COMMAND_TABLE));

    let tier = CompletionScriptTier::default();
    let tool = ResolvedTool {
        name: "widgetd".to_string(),
        path: Some(path.clone()),
        version: None,
    };

    assert!(
        !tier.detect(&tool),
        "a tool whose own help names no `completion` command must not be detected"
    );
    assert!(
        !dir.path().join("widgetd.completion_ran").exists(),
        "the completion-protocol word reached a tool with no evidence it speaks the \
         protocol — this is the argv that left 437 daemons running"
    );

    // And the refusal holds at `extract_node`, not only at `detect` — the
    // check lives on the tier, so no caller reaching it by another route
    // can send the argv either.
    let result = tier.extract_node(
        &tool,
        &["widgetd".to_string()],
        mandible_extract::NodeHints {
            heading_attested: true,
        },
    );
    assert!(result.is_err(), "expected a refusal, got {result:?}");
    assert!(
        !dir.path().join("widgetd.completion_ran").exists(),
        "`extract_node` sent the argv the gate refused at `detect`"
    );
}

/// The other half: the permitted case must still actually run, and still
/// produce the flags it always did.
#[test]
fn a_tool_that_advertises_the_command_is_still_probed_and_still_extracts() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_named_shim(dir.path(), "widgetctl", &shim(HELP_WITH_A_COMMAND_TABLE));

    let tier = CompletionScriptTier::default();
    let tool = ResolvedTool {
        name: "widgetctl".to_string(),
        path: Some(path.clone()),
        version: None,
    };

    assert!(
        tier.detect(&tool),
        "a tool whose own help names a `completion` command must still be detected — \
         a gate that refuses everything passes the safety test above while deleting \
         the tier"
    );
    assert!(
        dir.path().join("widgetctl.completion_ran").exists(),
        "the probe must actually reach the tool for the permitted case"
    );

    let node = tier
        .extract_node(
            &tool,
            &["widgetctl".to_string()],
            mandible_extract::NodeHints {
                heading_attested: true,
            },
        )
        .expect("evidence is present, so extraction must proceed exactly as before");
    assert!(
        node.flags
            .iter()
            .any(|f| f.long.as_deref() == Some("verbose")),
        "the recovered flags must be unchanged by the gate: {:?}",
        node.flags
    );
}
