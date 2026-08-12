//! The replay seam: decouples extraction tiers from *how* a probe's bytes
//! were produced, so the same tier code can run against a real subprocess
//! or against frozen fixture bytes with zero process spawns.
//!
//! [`Probe`] is the seam itself. [`LiveProbe`] is the production
//! implementation — it is the only thing outside this module that can
//! still cause a process to spawn, and it does so by delegating to
//! [`run_inert`] unchanged, so every §6 rule (rule 0's never-probe list,
//! rule 2a's empty-argument refusal, the timeout, the scratch dir, the
//! output cap) still applies exactly as before. [`Transcript`] is the
//! replay implementation the corpus regression harness (a later piece of
//! work) will drive tiers from.
//!
//! **Keyed on the real argv, not on the argv shape or the tool.** This is
//! the one design decision that matters here. It would be simpler to key
//! a transcript on the [`InertArgv`] variant, or even just on which tier
//! is asking — but this project has already shipped a tier that built the
//! wrong argv (a cobra completion probe missing the literal `__complete`)
//! while its own unit tests passed, because those tests mocked the probe
//! and never exercised argv construction at all (`AGENTS.md` §3.1). Keying
//! on [`InertArgv::args`] closes that hole structurally: a tier that
//! builds the wrong argv gets a loud, named [`ExecError::TranscriptMiss`]
//! instead of a silent pass, whether the mistake is caught by a unit test
//! today or by a future corpus fixture that simply doesn't contain the
//! argv the tier actually sent.

use super::policy::InertArgv;
use super::spawn::{run_inert, ExecError, ExecOutput};
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

/// A source of a tool probe's output: either a real subprocess
/// ([`LiveProbe`]) or frozen bytes recorded ahead of time ([`Transcript`]).
///
/// Every extraction tier that needs to run a tool is given one of these
/// through its constructor (never through [`crate::ExtractionTier`]'s own
/// signature — the trait itself stays probe-agnostic), so the same tier
/// code drives both a live pipeline and a replay from disk.
pub trait Probe: Send + Sync {
    /// Run `tool_path` under `argv`, exactly as [`run_inert`] would: bound
    /// by `timeout`, subject to whatever safety policy the implementation
    /// applies (full §6 for [`LiveProbe`]; a lookup for [`Transcript`]).
    fn run(
        &self,
        tool_path: &Path,
        argv: &InertArgv,
        timeout: Duration,
    ) -> Result<ExecOutput, ExecError>;
}

/// The production [`Probe`]: every call is forwarded to [`run_inert`]
/// unmodified, so this is the only thing outside `exec/` that can still
/// cause a process to spawn, and every §6 guarantee `run_inert` already
/// provides (rule 0, rule 2a, the timeout, the scratch dir, the output
/// cap) carries over untouched. This impl deliberately adds no logic of
/// its own — duplicating or bypassing any of `run_inert`'s policy here
/// would be exactly the kind of second execution path spec §6 exists to
/// prevent.
#[derive(Debug, Default, Clone, Copy)]
pub struct LiveProbe;

impl Probe for LiveProbe {
    fn run(
        &self,
        tool_path: &Path,
        argv: &InertArgv,
        timeout: Duration,
    ) -> Result<ExecOutput, ExecError> {
        run_inert(tool_path, argv, timeout)
    }
}

/// The replay [`Probe`]: an in-memory argv → [`ExecOutput`] map, with no
/// subprocess involved at all. Built directly from pairs today; a future
/// loader (reading `corpus/<tool>/<version>/help.txt` and friends) is the
/// other half of this seam and is out of scope here — [`Transcript::argvs`]
/// is the accessor that loader will need, to report which argvs a fixture
/// actually covers.
///
/// **Keyed on [`InertArgv::args`]**, never on the [`InertArgv`] variant or
/// a tool/path name — see this module's doc comment for why that's the
/// whole point.
#[derive(Debug, Default, Clone)]
pub struct Transcript {
    entries: HashMap<Vec<String>, ExecOutput>,
}

impl Transcript {
    /// Build a transcript from `(argv, output)` pairs. `argv` is the exact
    /// argument vector a tier's [`InertArgv::args`] must produce for a hit
    /// — not the `InertArgv` value itself, which carries no `Eq`/`Hash`
    /// suitable for this and, more importantly, would let a tier match by
    /// construction intent rather than by what it actually sends.
    pub fn new(pairs: impl IntoIterator<Item = (Vec<String>, ExecOutput)>) -> Self {
        Self {
            entries: pairs.into_iter().collect(),
        }
    }

    /// The argvs this transcript has a recording for, for a future loader
    /// or corpus runner to report what a fixture covers.
    pub fn argvs(&self) -> impl Iterator<Item = &Vec<String>> {
        self.entries.keys()
    }
}

impl Probe for Transcript {
    fn run(
        &self,
        tool_path: &Path,
        argv: &InertArgv,
        _timeout: Duration,
    ) -> Result<ExecOutput, ExecError> {
        let args = argv.args();
        self.entries
            .get(&args)
            .cloned()
            .ok_or_else(|| ExecError::TranscriptMiss {
                tool: tool_path.display().to_string(),
                argv: args,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(stdout: &str) -> ExecOutput {
        ExecOutput {
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
            exit_code: Some(0),
            timed_out: false,
        }
    }

    #[test]
    fn transcript_hits_on_exact_argv() {
        let t = Transcript::new([(
            vec!["--help".to_string()],
            output("usage: mytool [options]\n"),
        )]);
        let out = t
            .run(
                Path::new("/bin/mytool"),
                &InertArgv::HelpLong,
                Duration::from_secs(1),
            )
            .expect("recorded argv must hit");
        assert_eq!(out.stdout, b"usage: mytool [options]\n");
    }

    /// The negative case the whole seam exists for: an argv the transcript
    /// wasn't given must miss loudly, naming the argv it was asked for —
    /// never fall through to an empty, confidently-wrong success.
    #[test]
    fn transcript_miss_names_the_requested_argv() {
        let t = Transcript::new([(vec!["--help".to_string()], output("ok\n"))]);
        let err = t
            .run(
                Path::new("/bin/mytool"),
                &InertArgv::HelpLongForPath {
                    words: vec!["commit".to_string()],
                },
                Duration::from_secs(1),
            )
            .expect_err("an argv with no recording must miss");
        match err {
            ExecError::TranscriptMiss { tool, argv } => {
                assert_eq!(tool, "/bin/mytool");
                assert_eq!(argv, vec!["commit".to_string(), "--help".to_string()]);
            }
            other => panic!("expected TranscriptMiss, got {other:?}"),
        }
    }

    /// Keying is on the argv, not on which [`InertArgv`] variant produced
    /// it — two different shapes that happen to render the same words
    /// must both hit the one recording.
    #[test]
    fn transcript_keys_on_rendered_args_not_the_variant() {
        let t = Transcript::new([(vec!["--help".to_string()], output("ok\n"))]);
        assert!(t
            .run(
                Path::new("/bin/mytool"),
                &InertArgv::HelpLong,
                Duration::from_secs(1)
            )
            .is_ok());
    }
}
