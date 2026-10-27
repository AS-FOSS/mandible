//! Raw probe output kept after a fill, so the verbatim view can show the
//! bytes a page was parsed from without running the tool again.
//!
//! [`Recorder`] wraps the probe a tier reads through and logs every call.
//! [`Replay`] serves a log back as a [`Probe`], keyed on tool path and
//! rendered argv, so the raw-view code runs unchanged over kept bytes.

use super::policy::InertArgv;
use super::probe::Probe;
use super::spawn::{ExecError, ExecOutput};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Fixed cost charged per kept probe on top of its output bytes.
const PROBE_OVERHEAD_BYTES: usize = 256;

#[derive(Debug, Clone)]
struct KeptProbe {
    tool_path: PathBuf,
    args: Vec<String>,
    /// A failed probe keeps the text of its error, which is all the raw
    /// view shows of it.
    result: Result<ExecOutput, String>,
}

/// Every probe one fill ran, in order, with the output or error of each.
/// Cloning shares the log.
#[derive(Debug, Clone, Default)]
pub struct KeptProbes(Arc<Vec<KeptProbe>>);

impl KeptProbes {
    /// Nothing was probed.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Bytes held: each probe's two streams plus a fixed charge per probe.
    pub fn byte_size(&self) -> usize {
        self.0
            .iter()
            .map(|p| {
                let out = p
                    .result
                    .as_ref()
                    .map_or(0, |o| o.stdout.len() + o.stderr.len());
                out + PROBE_OVERHEAD_BYTES
            })
            .sum()
    }

    /// The probes of `parts`, one after another.
    pub fn concat<'a>(parts: impl IntoIterator<Item = &'a KeptProbes>) -> KeptProbes {
        let mut all = Vec::new();
        for part in parts {
            all.extend(part.0.iter().cloned());
        }
        KeptProbes(Arc::new(all))
    }
}

/// A [`Probe`] that forwards to `inner` and logs each call.
pub struct Recorder<'a> {
    inner: &'a dyn Probe,
    log: Mutex<Vec<KeptProbe>>,
}

impl<'a> Recorder<'a> {
    /// Log the calls made through this probe to `inner`.
    pub fn new(inner: &'a dyn Probe) -> Self {
        Self {
            inner,
            log: Mutex::new(Vec::new()),
        }
    }

    /// The calls made so far.
    pub fn finish(self) -> KeptProbes {
        let log = self.log.into_inner().unwrap_or_else(|e| e.into_inner());
        KeptProbes(Arc::new(log))
    }
}

impl Probe for Recorder<'_> {
    fn run(
        &self,
        tool_path: &Path,
        argv: &InertArgv,
        timeout: Duration,
    ) -> Result<ExecOutput, ExecError> {
        let result = self.inner.run(tool_path, argv, timeout);
        let kept = KeptProbe {
            tool_path: tool_path.to_path_buf(),
            args: argv.args(),
            result: match &result {
                Ok(out) => Ok(out.clone()),
                Err(e) => Err(e.to_string()),
            },
        };
        self.log
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(kept);
        result
    }
}

/// A [`Probe`] that answers from kept logs and never runs a process.
/// A call no log covers is refused and remembered in [`Self::missed`].
pub struct Replay<'a> {
    sources: &'a [&'a KeptProbes],
    missed: AtomicBool,
}

impl<'a> Replay<'a> {
    /// Answer from `sources`; the first log holding a call answers it.
    pub fn new(sources: &'a [&'a KeptProbes]) -> Self {
        Self {
            sources,
            missed: AtomicBool::new(false),
        }
    }

    /// Whether any call found no recording.
    pub fn missed(&self) -> bool {
        self.missed.load(Ordering::Relaxed)
    }
}

impl Probe for Replay<'_> {
    fn run(
        &self,
        tool_path: &Path,
        argv: &InertArgv,
        _timeout: Duration,
    ) -> Result<ExecOutput, ExecError> {
        let args = argv.args();
        let hit = self
            .sources
            .iter()
            .flat_map(|s| s.0.iter())
            .find(|p| p.tool_path == tool_path && p.args == args);
        match hit {
            Some(p) => match &p.result {
                Ok(out) => Ok(out.clone()),
                Err(text) => Err(ExecError::Recorded(text.clone())),
            },
            None => {
                self.missed.store(true, Ordering::Relaxed);
                Err(ExecError::TranscriptMiss {
                    tool: tool_path.display().to_string(),
                    argv: args,
                })
            }
        }
    }
}
