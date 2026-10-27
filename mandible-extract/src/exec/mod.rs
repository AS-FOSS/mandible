//! Execution safety policy (spec §6). The only module in the workspace
//! permitted to use `std::process` — enforced by `tests/no_process_outside_exec.rs`.
//! Every tier that runs a subprocess goes through [`run_inert`] with an
//! [`InertArgv`], never `std::process::Command` directly.
//!
//! `--help`/`-h` is not reliably read-only (spec §6 rule 8). Every probe
//! runs with its working directory, `HOME`, `TMPDIR`, and the writable XDG
//! base-directory variables all pointed into one scratch `TempDir` under `$HOME/.cache/mandible`, created
//! fresh per invocation and removed recursively when it returns. Applied
//! uniformly to every probe, never a per-tool exclusion list (spec §1).
//! Not a complete guarantee: a tool writing to an absolute path baked into
//! the binary sits outside what an env/CWD redirect can reach.
//!
//! Three layers for a full-`PATH` sweep: [`run_inert`]'s rules **prevent**
//! what argv shape alone can reason about; [`containment`] **contains**
//! what runs anyway inside a fresh user/PID/mount namespace; [`canary`]
//! **detects** a side effect that happens despite both. Containment must
//! never become a reason to loosen the first layer.

pub mod canary;
pub mod containment;
mod kept;
mod limit;
mod policy;
mod probe;
mod reap;
mod scratch_root;
mod spawn;

pub use kept::{KeptProbes, Recorder, Replay};
pub use limit::{extract_limit, with_extract_limit, EXTRACT_LIMIT};
pub use policy::InertArgv;
pub use probe::{LiveProbe, Probe, Transcript};
pub use spawn::{is_help_only_probe, run_inert, ExecError, ExecOutput, MAX_OUTPUT_BYTES};

/// The probe scratch root (spec §6 rule 8), created if missing. No probe runs
/// without it, so the binary checks it once before opening a tool.
pub fn scratch_root() -> std::io::Result<std::path::PathBuf> {
    scratch_root::ensure()
}
