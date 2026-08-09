//! Execution safety policy (spec §6). **This is the only module in the
//! entire workspace permitted to use `std::process`** — a workspace-wide
//! test (`tests/no_process_outside_exec.rs`) enforces that by grepping the
//! source tree, so this boundary is auditable rather than aspirational.
//!
//! Every tier that needs to run a subprocess goes through [`run_inert`]
//! with an [`InertArgv`], never `std::process::Command` directly.
//!
//! **`--help`/`-h` is not reliably read-only (spec §6 rule 8, [M-11]).**
//! Running the coverage harness (spec §13.1) against ~1600 real
//! executables found font-cache builders writing `fonts.dir`/
//! `fonts.scale` into the invoking directory, and
//! `mysql_secure_installation` (a shell script) writing a `.my.cnf.<pid>`
//! config file — with an empty root password — when probed with nothing
//! but `--help`. (That script's write is a plain relative path,
//! `config=".my.cnf.$$"` — no `$HOME` involved; an earlier version of
//! this comment guessed otherwise before actually reading the script.)
//! Every probe therefore runs with its working directory, `HOME`,
//! `TMPDIR`, and the writable XDG base-directory variables
//! (`XDG_RUNTIME_DIR`, `XDG_CACHE_HOME`, `XDG_CONFIG_HOME`,
//! `XDG_DATA_HOME`, `XDG_STATE_HOME`) all pointed at one scratch directory
//! — a `tempfile::TempDir` created fresh for that single invocation and
//! removed again (recursively) the moment it returns, so nothing a probe
//! writes ever outlives the probe or accumulates across invocations. This
//! is a general policy applied uniformly to every probe, never a
//! per-tool exclusion list (spec §1's invariant) — verified for both the
//! CWD and `HOME` cases in `spawn`'s test module.
//!
//! It still is not a complete guarantee: a tool that constructs a write
//! path some other way entirely — an absolute path baked into the binary
//! itself, say — sits outside what an environment/CWD redirect can reach.
//! Full containment needs OS-level sandboxing (a container, a restricted
//! mount namespace, seccomp), which is out of scope here; that residual
//! risk is documented rather than papered over with a claim of full
//! inertness.

mod policy;
mod spawn;

pub use policy::InertArgv;
pub use spawn::{is_help_only_probe, run_inert, ExecError, ExecOutput, MAX_OUTPUT_BYTES};
