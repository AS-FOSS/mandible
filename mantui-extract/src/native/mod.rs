//! Tier E: native, self-describing binary probes. **Batch 3** (spec roadmap
//! phase 4).
//!
//! Spec §7 Tier E: cobra `__complete` (two probes per node — subcommands
//! with `""`, flags with `"-"`, per measurement [M-2]) with a depth cap,
//! visited-set cycle guard, and `Alias for "..."` detection; clap
//! `CompleteEnv` (`COMPLETE=<shell> <tool> --`, probed but not roadmapped
//! on, since it's nearly absent in the wild — [M-4]); argcomplete's
//! `_ARGCOMPLETE` convention. All argv shapes here must already exist on
//! [`crate::exec::InertArgv`] before this tier can be implemented, per
//! spec §6.
//!
//! Left unimplemented in this batch; the `native` feature flag and this
//! module already exist so batch 3 slots in without a restructure.
