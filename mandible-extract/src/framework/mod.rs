//! Tier A′: framework identification (spec §7 "Tier A′ — framework
//! identification").
//!
//! `--help` text is not written by hand, it is generated, from a small
//! closed set of generators. Per-tool knowledge is unbounded and forbidden
//! (spec §1); per-framework knowledge is bounded and is the correct unit
//! of parsing. Everything here is framework-keyed, never `if tool ==
//! "docker"`.
//!
//! Identification proceeds in the order spec §7 lays out, most reliable
//! first:
//!
//! 1. [`identify_from_artifact`] — scan the compiled binary's embedded
//!    strings (or, for a script, its shebang plus imports). Ground truth
//!    when it matches, independent of which headings that framework
//!    version's `--help` happens to render.
//! 2. [`identify_from_help_text`] — distinctive marker strings in `--help`
//!    output itself. Weaker: docker prints `Common Commands:` instead of
//!    cobra's usual `Available Commands:` [M-13], which is why step 1
//!    leads, and why this module never adds a tool-specific heading to
//!    patch that one case (spec §1).
//! 3. Unidentified — neither step matched; callers fall through to a
//!    framework-agnostic parser.

mod artifact;
mod help_text_signature;

use crate::exec::{InertArgv, LiveProbe, Probe};
use crate::resolve::ResolvedTool;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// Wall-clock cap for the one `--help` probe [`identify`] spawns itself
/// when artifact scanning didn't resolve anything (spec §6 rule 4's
/// `detect`-class budget).
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// The generator that produced a tool's `--help` output. Framework-keyed,
/// never tool-keyed: knowledge here is "how does cobra format its help
/// text", never "how does docker format its help text" (spec §1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Framework {
    /// clap v3/v4 (Rust). Payoff-ordered first among Tier B grammars [M-12].
    ClapV3V4,
    /// clap v2 (Rust), predating the `clap_builder` split.
    ClapV2,
    /// cobra (Go): kubectl, docker, gh, helm, and much of modern infra.
    Cobra,
    /// urfave/cli (Go).
    UrfaveCli,
    /// Go's standard library `flag` package.
    GoFlag,
    /// argparse (Python) [M-12].
    Argparse,
    /// click (Python).
    Click,
    /// docopt (Python and other languages with a port).
    Docopt,
    /// GNU argp / `getopt_long` (C/POSIX) — coreutils and friends, the
    /// single largest fingerprint measured [M-12].
    GnuArgp,
    /// Terse BSD-style `usage:` output with no long-form flags at all —
    /// a coarse catch-all, not a precise fingerprint (see
    /// [`help_text_signature`]'s doc comment on it).
    BsdTerse,
    /// BusyBox's multi-call `--help`.
    Busybox,
    /// commander (Node).
    Commander,
    /// yargs (Node).
    Yargs,
    /// oclif (Node).
    Oclif,
    /// picocli (JVM).
    Picocli,
    /// System.CommandLine (.NET).
    DotNetSystemCommandLine,
    /// Symfony Console (PHP).
    SymfonyConsole,
    /// Ruby's `OptionParser` or the Thor gem — grouped together per spec
    /// §7 Tier B's own table.
    OptionParserOrThor,
}

impl Framework {
    /// A short, human-readable name for `--doctor` and the TUI's
    /// provenance footer.
    pub fn name(&self) -> &'static str {
        match self {
            Framework::ClapV3V4 => "clap (v3/v4)",
            Framework::ClapV2 => "clap (v2)",
            Framework::Cobra => "cobra",
            Framework::UrfaveCli => "urfave/cli",
            Framework::GoFlag => "go flag",
            Framework::Argparse => "argparse",
            Framework::Click => "click",
            Framework::Docopt => "docopt",
            Framework::GnuArgp => "GNU argp/getopt_long",
            Framework::BsdTerse => "BSD-terse",
            Framework::Busybox => "busybox",
            Framework::Commander => "commander",
            Framework::Yargs => "yargs",
            Framework::Oclif => "oclif",
            Framework::Picocli => "picocli",
            Framework::DotNetSystemCommandLine => "System.CommandLine",
            Framework::SymfonyConsole => "Symfony Console",
            Framework::OptionParserOrThor => "OptionParser/Thor",
        }
    }
}

/// Which step of spec §7 Tier A′ resolved a [`Framework`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectionMethod {
    /// Step 1: the artifact itself (embedded strings, or a script's
    /// shebang + imports). Ground truth.
    Artifact,
    /// Step 2: a distinctive marker in `--help` output. Weaker — see this
    /// module's doc comment on why step 1 must lead.
    HelpTextSignature,
}

/// The outcome of running the full three-step identification.
#[derive(Debug, Clone, Copy)]
pub struct FrameworkDetection {
    /// `None` when neither step matched (spec §7 Tier A′ step 3,
    /// "Unidentified").
    pub framework: Option<Framework>,
    /// Always `Some` exactly when `framework` is.
    pub method: Option<DetectionMethod>,
}

impl FrameworkDetection {
    fn unidentified() -> FrameworkDetection {
        FrameworkDetection {
            framework: None,
            method: None,
        }
    }

    /// A one-line description for `--doctor`, e.g. `"cobra (from
    /// artifact)"` or `"unidentified"`.
    pub fn describe(&self) -> String {
        match (self.framework, self.method) {
            (Some(f), Some(DetectionMethod::Artifact)) => format!("{} (from artifact)", f.name()),
            (Some(f), Some(DetectionMethod::HelpTextSignature)) => {
                format!("{} (help-text signature)", f.name())
            }
            _ => "unidentified".to_string(),
        }
    }
}

/// Process-wide memoization of [`artifact::scan`] results, keyed by
/// resolved binary path. `identify_from_artifact` is called once per node
/// extraction by Tier B — a large tree like `docker`'s means dozens of
/// calls against the same binary, and a scan can read tens of MB.
/// In-memory, per-process memoization of a pure fact about a file that
/// cannot change mid-run — unrelated to spec §11's deleted on-disk
/// extraction cache.
fn artifact_cache() -> &'static Mutex<HashMap<PathBuf, Option<Framework>>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Option<Framework>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Step 1 alone: scan the artifact at `tool.path`, bounded, without
/// spawning a process. `None` if the tool didn't resolve to a path, the
/// file couldn't be read, or no marker matched within the bound.
///
/// Exposed separately from [`identify`] so a caller with help text already
/// in hand can fall back to [`identify_from_help_text`] without probing
/// twice. Memoized per binary path (see [`artifact_cache`]).
pub fn identify_from_artifact(tool: &ResolvedTool) -> Option<Framework> {
    let path = tool.path.as_ref()?;
    let cache = artifact_cache();
    if let Some(hit) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(path) {
        return *hit;
    }
    let result = artifact::scan(path);
    cache
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(path.clone(), result);
    result
}

/// Step 2 alone: scan already-fetched `--help` text for a signature.
/// Exposed separately from [`identify`] for the same reason as
/// [`identify_from_artifact`].
pub fn identify_from_help_text(help_text: &str) -> Option<Framework> {
    help_text_signature::scan(help_text)
}

/// The full three-step identification spec §7 Tier A′ describes: artifact,
/// then (if needed) exactly one bounded, inert `--help` probe
/// ([`InertArgv::HelpLong`], spec §6 rule 2) for the help-text signature,
/// then unidentified. What `--doctor` uses for a standalone answer.
///
/// A thin [`LiveProbe`] wrapper over [`identify_with_probe`].
pub fn identify(tool: &ResolvedTool) -> FrameworkDetection {
    identify_with_probe(&LiveProbe, tool)
}

/// [`identify`], but against an explicit [`Probe`] rather than always the
/// live one.
pub fn identify_with_probe(probe: &dyn Probe, tool: &ResolvedTool) -> FrameworkDetection {
    if let Some(framework) = identify_from_artifact(tool) {
        return FrameworkDetection {
            framework: Some(framework),
            method: Some(DetectionMethod::Artifact),
        };
    }

    let Some(path) = tool.path.as_ref() else {
        return FrameworkDetection::unidentified();
    };
    let Ok(out) = probe.run(path, &InertArgv::HelpLong, PROBE_TIMEOUT) else {
        return FrameworkDetection::unidentified();
    };
    let text = if !out.stdout.is_empty() {
        String::from_utf8_lossy(&out.stdout).into_owned()
    } else {
        String::from_utf8_lossy(&out.stderr).into_owned()
    };

    match identify_from_help_text(&text) {
        Some(framework) => FrameworkDetection {
            framework: Some(framework),
            method: Some(DetectionMethod::HelpTextSignature),
        },
        None => FrameworkDetection::unidentified(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describe_unidentified() {
        assert_eq!(
            FrameworkDetection::unidentified().describe(),
            "unidentified"
        );
    }

    #[test]
    fn describe_artifact_match() {
        let d = FrameworkDetection {
            framework: Some(Framework::Cobra),
            method: Some(DetectionMethod::Artifact),
        };
        assert_eq!(d.describe(), "cobra (from artifact)");
    }

    #[test]
    fn describe_help_text_match() {
        let d = FrameworkDetection {
            framework: Some(Framework::Argparse),
            method: Some(DetectionMethod::HelpTextSignature),
        };
        assert_eq!(d.describe(), "argparse (help-text signature)");
    }

    #[test]
    fn identify_help_text_signature_falls_back_when_artifact_scan_finds_nothing() {
        // A shim shell script whose --help output carries argparse's
        // distinctive marker, exercising real argv construction.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shim.sh");
        std::fs::write(
            &path,
            "#!/bin/sh\necho 'usage: shim [-h]'\necho\necho 'show this help message and exit'\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let tool = ResolvedTool {
            name: "shim".to_string(),
            path: Some(path),
            version: None,
        };
        let detection = identify(&tool);
        assert_eq!(detection.framework, Some(Framework::Argparse));
        assert_eq!(detection.method, Some(DetectionMethod::HelpTextSignature));
    }

    #[test]
    fn identify_is_unidentified_for_unresolved_tool() {
        let tool = ResolvedTool {
            name: "definitely-not-a-real-tool-xyz".to_string(),
            path: None,
            version: None,
        };
        let detection = identify(&tool);
        assert!(detection.framework.is_none());
    }

    // --- the replay seam: real-argv tests against a `Transcript` ---

    /// Real argv, replayed: `identify`'s fallback probe is exactly
    /// `InertArgv::HelpLong` (`["--help"]`). A transcript keyed on that
    /// argv must let `identify_with_probe` recover the same detection as
    /// the real-shim test above, with zero subprocesses.
    #[test]
    fn identify_with_probe_replays_from_a_transcript_keyed_on_the_real_argv() {
        let raw = "usage: shim [-h]\n\nshow this help message and exit\n";
        let transcript = crate::exec::Transcript::new([(
            vec!["--help".to_string()],
            crate::exec::ExecOutput {
                stdout: raw.as_bytes().to_vec(),
                stderr: Vec::new(),
                exit_code: Some(0),
                timed_out: false,
            },
        )]);
        let tool = ResolvedTool {
            name: "shim".to_string(),
            path: Some(std::path::PathBuf::from("/replayed/shim")),
            version: None,
        };
        let detection = identify_with_probe(&transcript, &tool);
        assert_eq!(detection.framework, Some(Framework::Argparse));
        assert_eq!(detection.method, Some(DetectionMethod::HelpTextSignature));
    }

    /// A transcript missing the real fallback argv must not be mistaken
    /// for a successful probe — degrades to unidentified.
    #[test]
    fn identify_with_probe_is_unidentified_against_a_transcript_missing_the_argv() {
        let transcript = crate::exec::Transcript::new([(
            // Deliberately the wrong argv: `identify`'s fallback probe
            // never sends `-h`, only `--help`.
            vec!["-h".to_string()],
            crate::exec::ExecOutput {
                stdout: b"usage: shim [-h]\n".to_vec(),
                stderr: Vec::new(),
                exit_code: Some(0),
                timed_out: false,
            },
        )]);
        let tool = ResolvedTool {
            name: "shim".to_string(),
            path: Some(std::path::PathBuf::from("/replayed/shim")),
            version: None,
        };
        let detection = identify_with_probe(&transcript, &tool);
        assert!(detection.framework.is_none());
    }
}
