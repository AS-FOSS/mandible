//! Proves the fix for the reported `wall` incident (spec §7 Tier E,
//! 2026-08-12): `NativeTier::detect()` used to send `__complete <word>` to
//! every tool on `PATH` speculatively, to find out whether it answered —
//! the only way to know, absent any other signal. `wall` treats an
//! unrecognized first positional as the message to broadcast rather than
//! rejecting it, so that speculative probe became a system-wide
//! announcement of the literal text `__complete`.
//!
//! The fix gates the probe on prior evidence: `detect()` now sends
//! `__complete` only when [`mandible_extract::framework::identify_from_artifact`]
//! has already read the tool's own compiled bytes and found the cobra
//! marker. This test proves the gate holds through the real, unmodified
//! production path — `NativeTier::default()` (the real `ArtifactEvidence`
//! check, not a test seam) driving `LiveProbe` through the real
//! `run_inert` chokepoint against a real shim binary — exactly the
//! discipline `mandible-extract/tests/exec_policy.rs` already uses (spec
//! §13.3's "real-argv tests": a mocked probe can pass while the real argv
//! construction, or in this case the real gate, is broken).
//!
//! **Deliberately not named `wall`, `write`, or any other
//! `exec::spawn::HELP_ONLY_PROBE` entry.** A per-tool containment fix
//! already landed there for the six measured message-delivery tools, and a
//! shim using one of those names would be refused by *that* list
//! regardless of whether this gate works at all — proving nothing about
//! the general fix this file exists to test. The whole point of this
//! change is that the next tool nobody has been bitten by yet — not on any
//! list — is protected too, so the shim here is named after something
//! mundane and hypothetical instead.

use mandible_extract::native::NativeTier;
use mandible_extract::{ExtractionTier, ResolvedTool};
use std::io::Write;

fn write_named_shim(dir: &std::path::Path, name: &str, script: &str) -> std::path::PathBuf {
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

/// A shim that answers `__complete` exactly the way a genuine cobra tool
/// would (a candidate line plus a `:N` directive) — if the gate were
/// absent, bypassed, or merely broken, `NativeTier::detect()` would treat
/// this as a confirmed cobra tool and go on to probe it for real. It never
/// does: a `#!/bin/sh`-shebang shim can never satisfy the real,
/// file-backed artifact check (`framework::artifact::scan` only ever
/// recognizes a shebang script as a *script* framework — argparse, click,
/// commander, and the like — and cobra is Go-only, always compiled, so no
/// shell script can ever carry its marker). Not named after any
/// `HELP_ONLY_PROBE` entry — see this file's module doc comment for why
/// that distinction matters here.
fn cobra_shaped_shim_script() -> &'static str {
    "#!/bin/sh\ncase \"$1\" in\n  __complete) touch \"$0.complete_ran\"; printf 'build\\tbuild the thing\\n:0\\n' ;;\n  *) touch \"$0.other_ran\"; echo no ;;\nesac\n"
}

/// The safety property itself.
#[test]
fn a_tool_with_no_cobra_evidence_never_receives_a_dunder_complete_argv() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_named_shim(dir.path(), "sprocket", cobra_shaped_shim_script());

    let tier = NativeTier::default();
    let tool = ResolvedTool {
        name: "sprocket".to_string(),
        path: Some(path.clone()),
        version: None,
    };

    assert!(
        !tier.detect(&tool),
        "a tool with no artifact evidence of speaking cobra must not be detected as cobra"
    );
    assert!(
        !dir.path().join("sprocket.complete_ran").exists(),
        "the __complete argv reached the shim despite no prior evidence it speaks cobra — \
         this is the exact mechanism that broadcast `__complete` to every terminal via `wall`, \
         just against a tool no containment list happens to name"
    );
}

/// The same property one level up: `extract_node` must also refuse to
/// probe when `detect` was never given a chance to succeed (the runner
/// never calls `extract_node` for an undetected tier in production, but
/// this pins the tier's own contract independently of the runner, and
/// documents that the protocol cache — the only other way `extract_node`
/// could know to proceed — starts empty).
#[test]
fn extract_node_errors_rather_than_probing_when_never_detected() {
    let dir = tempfile::tempdir().unwrap();
    let path = write_named_shim(dir.path(), "gadget", cobra_shaped_shim_script());

    let tier = NativeTier::default();
    let tool = ResolvedTool {
        name: "gadget".to_string(),
        path: Some(path.clone()),
        version: None,
    };

    let result = tier.extract_node(
        &tool,
        &["gadget".to_string()],
        mandible_extract::NodeHints {
            heading_attested: true,
        },
    );
    assert!(
        result.is_err(),
        "extract_node must decline for a tool that was never detected as cobra"
    );
    assert!(
        !dir.path().join("gadget.complete_ran").exists(),
        "the shim received __complete despite no successful detection"
    );
}
