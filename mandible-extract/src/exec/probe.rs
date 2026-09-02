//! The replay seam: decouples extraction tiers from *how* a probe's bytes
//! were produced, so the same tier code can run against a real subprocess
//! or against frozen fixture bytes with zero process spawns.
//!
//! [`Probe`] is the seam. [`LiveProbe`] is the production implementation —
//! the only thing outside this module that can still cause a process to
//! spawn, delegating to [`run_inert`] unchanged so every §6 rule still
//! applies. [`Transcript`] is the replay implementation.
//!
//! **Keyed on the real argv, not on the argv shape or the tool.** A tier
//! that builds the wrong argv gets a loud, named
//! [`ExecError::TranscriptMiss`] instead of a silent pass.

use super::policy::InertArgv;
use super::spawn::{run_inert, ExecError, ExecOutput};
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

/// A source of a tool probe's output: either a real subprocess
/// ([`LiveProbe`]) or frozen bytes recorded ahead of time ([`Transcript`]).
/// Passed to each extraction tier through its constructor, never through
/// [`crate::ExtractionTier`]'s own signature, so the trait stays
/// probe-agnostic.
pub trait Probe: Send + Sync {
    /// Run `tool_path` under `argv`, exactly as [`run_inert`] would.
    fn run(
        &self,
        tool_path: &Path,
        argv: &InertArgv,
        timeout: Duration,
    ) -> Result<ExecOutput, ExecError>;
}

/// The production [`Probe`]: every call forwards to [`run_inert`]
/// unmodified. Adds no logic of its own — a second execution path here
/// would be exactly what spec §6 exists to prevent.
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

/// The replay [`Probe`]: an in-memory argv → [`ExecOutput`] map, no
/// subprocess involved. Keyed on [`InertArgv::args`], never on the
/// [`InertArgv`] variant or a tool/path name.
#[derive(Debug, Default, Clone)]
pub struct Transcript {
    entries: HashMap<Vec<String>, ExecOutput>,
}

impl Transcript {
    /// Build a transcript from `(argv, output)` pairs. `argv` is the exact
    /// vector a tier's [`InertArgv::args`] must produce for a hit.
    pub fn new(pairs: impl IntoIterator<Item = (Vec<String>, ExecOutput)>) -> Self {
        Self {
            entries: pairs.into_iter().collect(),
        }
    }

    /// The argvs this transcript has a recording for.
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

    /// An argv the transcript wasn't given must miss loudly, naming the
    /// argv requested, never fall through to a confidently-wrong success.
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
