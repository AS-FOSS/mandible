//! Execution safety policy (spec §6). **This is the only module in the
//! entire workspace permitted to use `std::process`** — a workspace-wide
//! test (`tests/no_process_outside_exec.rs`) enforces that by grepping the
//! source tree, so this boundary is auditable rather than aspirational.
//!
//! Every tier that needs to run a subprocess goes through [`run_inert`]
//! with an [`InertArgv`], never `std::process::Command` directly.

mod policy;
mod spawn;

pub use policy::InertArgv;
pub use spawn::{run_inert, ExecError, ExecOutput, MAX_OUTPUT_BYTES};
