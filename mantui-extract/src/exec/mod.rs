//! Execution safety policy (spec §6). **This is the only module in the
//! entire workspace permitted to use `std::process`** — a workspace-wide
//! test (`tests/no_process_outside_exec.rs`) enforces that by grepping the
//! source tree, so this boundary is auditable rather than aspirational.
//!
//! Every tier that needs to run a subprocess goes through [`run_inert`]
//! with an [`InertArgv`], never `std::process::Command` directly.
//!
//! **Known residual gap**, found by running the coverage harness (spec
//! §13.1) against ~1600 real executables: the argv allowlist assumes a
//! tool treats `--help`/`-h` as a pure informational query, but a poorly
//! written one might not. `mysql_secure_installation` (a shell script)
//! wrote a `.my.cnf.<pid>` config file — with an empty root password —
//! when probed with nothing but `--help`, regardless of the scratch
//! working directory `run_inert` sets (see below); it evidently
//! constructs that path some other way, not relative to CWD. The scratch
//! CWD *does* contain the common case (confirmed: font-cache and other
//! tools' stray files stopped appearing in the caller's directory once it
//! was added), but it cannot be a complete guarantee against a
//! particular script's own internal path logic. Full containment would
//! need OS-level sandboxing (a container, a restricted mount namespace,
//! seccomp) — out of scope for this batch, and deliberately not
//! special-cased per-tool here (spec §1's invariant).

mod policy;
mod spawn;

pub use policy::InertArgv;
pub use spawn::{run_inert, ExecError, ExecOutput, MAX_OUTPUT_BYTES};
