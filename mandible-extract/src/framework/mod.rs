//! Tier A′: framework identification (spec §7 "Tier A′ — framework
//! identification").
//!
//! The load-bearing insight behind spec revision 3: `--help` text is not
//! written by hand, it is *generated*, and only a small closed set of
//! generators exists. Per-tool knowledge is unbounded and forbidden (spec
//! §1); per-*framework* knowledge is bounded at roughly fifteen entries and
//! is the correct unit of parsing. Everything in this module and its
//! callers (Tier B's per-framework grammars, batch 6 part 4) is
//! framework-keyed — never `if tool == "docker"`.
//!
//! Identification proceeds in the order spec §7 lays out, most reliable
//! first:
//!
//! 1. [`identify_from_artifact`] — scan the compiled binary's embedded
//!    strings (or, for a script, its shebang plus import lines). Ground
//!    truth when it matches: a Go binary linking `spf13/cobra` says so
//!    directly in its own bytes, independent of which section headings
//!    that particular cobra version's `--help` happens to render this
//!    week.
//! 2. [`identify_from_help_text`] — distinctive marker strings in
//!    `--help` output itself. Weaker: docker prints `Common Commands:`
//!    instead of cobra's usual `Available Commands:`, so a signature
//!    keyed on the latter alone misses it entirely [M-13]. This is
//!    exactly why step 1 leads and is authoritative when it matches, and
//!    why this module never adds a docker-flavored heading to the
//!    signature table just to patch that one case — that would be §1's
//!    forbidden per-tool special case wearing a framework's name.
//! 3. Unidentified — neither step matched; callers fall through to a
//!    framework-agnostic parser (Tier B step 2, batch 6 part 4).

mod artifact;
mod help_text_signature;

use crate::exec::{run_inert, InertArgv};
use crate::resolve::ResolvedTool;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// Wall-clock cap for the one `--help` probe [`identify`] spawns itself
/// when artifact scanning didn't resolve anything (spec §6 rule 4's
/// `detect`-class budget — this is a detection step, not a full
/// extraction).
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// The generator that produced a tool's `--help` output. Framework-keyed,
/// never tool-keyed: knowledge here is "how does cobra format its help
/// text", never "how does docker format its help text" (spec §1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Framework {
    /// clap v3/v4 (Rust). Payoff-ordered first among Tier B grammars —
    /// 24.6% of a real machine's tools [M-12].
    ClapV3V4,
    /// clap v2 (Rust), predating the `clap_builder` split.
    ClapV2,
    /// cobra (Go): kubectl, docker, gh, helm, and much of modern infra.
    Cobra,
    /// urfave/cli (Go).
    UrfaveCli,
    /// Go's standard library `flag` package.
    GoFlag,
    /// argparse (Python). 4.1% of a real machine's tools [M-12].
    Argparse,
    /// click (Python).
    Click,
    /// docopt (Python and other languages with a port).
    Docopt,
    /// GNU argp / `getopt_long` (C/POSIX) — coreutils and friends. 15.5%
    /// of a real machine's tools [M-12], the single largest single
    /// fingerprint measured.
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
/// extraction by Tier B (batch 6 part 4) — a large tree like `docker`'s
/// means dozens of calls against the *same* binary in one run, and a scan
/// can read tens of MB (see `artifact`'s module doc comment on why the
/// bound is that generous). Re-scanning the same file that many times
/// would reintroduce exactly the kind of per-node cost spec §5.1 exists to
/// avoid, just relocated into this module instead of a subprocess spawn.
/// This is in-memory, per-process memoization of a pure fact about a file
/// that cannot change mid-run — unrelated to, and not a reintroduction of,
/// spec §11's deleted on-disk *extraction* cache.
fn artifact_cache() -> &'static Mutex<HashMap<PathBuf, Option<Framework>>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Option<Framework>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Step 1 alone: scan the artifact at `tool.path`, bounded, without
/// spawning a process. `None` if the tool didn't resolve to a path, the
/// file couldn't be read, or no marker matched within the bound.
///
/// Exposed separately from [`identify`] so Tier B's `extract_node` (called
/// once per node) can try this cheap, spawn-free step itself and, only on
/// a miss, fall back to whatever help text it *already fetched* for its
/// own parsing via [`identify_from_help_text`] — without ever probing
/// twice for the same information. Memoized per binary path (see
/// [`artifact_cache`]) so a tree with many nodes over the same tool only
/// ever pays the scan cost once.
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
/// [`identify_from_artifact`] — callers that already have the text on
/// hand should never spawn a second probe just to re-derive it.
pub fn identify_from_help_text(help_text: &str) -> Option<Framework> {
    help_text_signature::scan(help_text)
}

/// The full three-step identification spec §7 Tier A′ describes, as a
/// single self-contained call: artifact, then (if needed) exactly one
/// bounded, inert `--help` probe of its own — [`InertArgv::HelpLong`] is
/// on the spec §6 rule 2 allowlist — to try the help-text signature, then
/// unidentified.
///
/// This is what `--doctor` uses: it wants a standalone answer without
/// depending on Tier B having already run (Tier B may be disabled, or may
/// not have reached this tool's node yet).
pub fn identify(tool: &ResolvedTool) -> FrameworkDetection {
    if let Some(framework) = identify_from_artifact(tool) {
        return FrameworkDetection {
            framework: Some(framework),
            method: Some(DetectionMethod::Artifact),
        };
    }

    let Some(path) = tool.path.as_ref() else {
        return FrameworkDetection::unidentified();
    };
    let Ok(out) = run_inert(path, &InertArgv::HelpLong, PROBE_TIMEOUT) else {
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
        // A shim shell script (no framework markers in its own bytes)
        // whose --help output carries argparse's distinctive marker —
        // exercises the *real* argv construction (spec AGENTS.md §3.1: a
        // prior cobra tier was silently dead because its unit tests
        // mocked the probe instead of going through exec::run_inert).
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
}
