//! Command-line arguments of `audit tag`, `audit history`, `audit chart`, `audit replay`,
//! `audit truth`, `audit ingest`, `audit fixtures`, `audit amend` and
//! `audit spot-audit`.

use clap::Args;
use std::path::PathBuf;

#[derive(Args)]
pub struct TagArgs {
    #[arg(long)]
    pub seed: u64,
    #[arg(long)]
    pub tool: String,
    /// `known` or `new`.
    #[arg(long)]
    pub kind: String,
    /// The docs/shapes.md heading a `known` failure belongs to.
    #[arg(long)]
    pub shape: Option<String>,
    #[arg(long, default_value = "audit")]
    pub dir: PathBuf,
}

#[derive(Args)]
pub struct HistoryArgs {
    #[arg(long, default_value = "audit")]
    pub dir: PathBuf,
    #[arg(long, default_value = "audit/history.json")]
    pub out: PathBuf,
    /// Score only this batch, for a manifest that predates
    /// `mandible_version`.
    #[arg(long)]
    pub seed: Option<u64>,
    /// The version the batch is judged with; overrides the manifest.
    #[arg(long)]
    pub judged_version: Option<String>,
}

#[derive(Args)]
pub struct ChartArgs {
    #[arg(long, default_value = "audit/history.json")]
    pub history: PathBuf,
    #[arg(long, default_value = "docs/accuracy")]
    pub out_dir: PathBuf,
}

/// Run every scored release over the tools of an audit batch and record a
/// hash of the tree each one read. Namespace-contained like a full-`PATH` sweep.
#[derive(Args, Debug)]
pub struct ReplayArgs {
    /// The batch to replay (`<dir>/<seed>.toml`).
    #[arg(long)]
    pub seed: u64,
    /// Directory holding the batch; replay files go under `<dir>/<seed>/replay/`.
    #[arg(long, default_value = "audit")]
    pub dir: PathBuf,
    /// Refs to replay, comma-separated (tags or commits).
    #[arg(long, value_delimiter = ',')]
    pub refs: Vec<String>,
    /// Add the last patch tag of every minor earlier than the judged one.
    #[arg(long)]
    pub per_minor: bool,
    /// The build the batch was judged with; defaults to `v<mandible_version>`.
    #[arg(long)]
    pub judged_ref: Option<String>,
    /// Same escape hatch as `coverage --allow-uncontained`.
    #[arg(long)]
    pub allow_uncontained: bool,
}

/// Write the current build's tree for one tool of a batch whose verdict is
/// incomplete or wrong.
#[derive(Args)]
pub struct TruthArgs {
    /// The batch the tool belongs to.
    #[arg(long)]
    pub seed: u64,
    /// The tool.
    #[arg(long)]
    pub tool: String,
    /// Directory holding the batch.
    #[arg(long, default_value = "audit")]
    pub dir: PathBuf,
}

/// Read verdicts from a file and record them.
#[derive(Args)]
pub struct IngestArgs {
    #[arg(long)]
    pub seed: u64,
    #[arg(long, default_value = "audit")]
    pub dir: PathBuf,
    /// The verdicts file to read.
    #[arg(long)]
    pub verdicts: PathBuf,
    /// Replace an already-recorded verdict; without it those lines are
    /// left alone.
    #[arg(long)]
    pub overwrite: bool,
}

/// Turn every reviewed tool into a fixture.
#[derive(Args)]
pub struct FixturesArgs {
    #[arg(long)]
    pub seed: u64,
    #[arg(long, default_value = "audit")]
    pub dir: PathBuf,
    /// Where to write fixtures. Defaults to a staging area under `--dir`;
    /// `corpus` writes into the gated corpus (see `cmd_fixtures`).
    #[arg(long)]
    pub corpus_dir: Option<PathBuf>,
    /// Only emit fixtures for these tools (comma-separated) instead of
    /// every reviewed entry.
    #[arg(long, value_delimiter = ',')]
    pub only: Option<Vec<String>>,
    /// Overwrite an already-existing fixture directory.
    #[arg(long)]
    pub force: bool,
}

/// Correct one already-recorded verdict.
#[derive(Args)]
pub struct AmendArgs {
    #[arg(long)]
    pub seed: u64,
    #[arg(long, default_value = "audit")]
    pub dir: PathBuf,
    /// The tool whose verdict is being corrected.
    #[arg(long)]
    pub tool: String,
    /// The corrected verdict (`c`/`correct`, `i`/`incomplete`,
    /// `w`/`wrong`, `s`/`skip`).
    #[arg(long)]
    pub verdict: String,
    /// The note attached to the corrected verdict. Required when
    /// `--verdict` is `wrong`/`incomplete`, same obligation an ordinary
    /// verdict carries.
    #[arg(long)]
    pub note: Option<String>,
    /// Why the original verdict was wrong. Always required.
    #[arg(long)]
    pub reason: String,
}

/// Spot-audit one mass-`ok` promotion event.
#[derive(Args)]
pub struct SpotAuditArgs {
    /// Names the verdict file (`<dir>/<seed>.toml`) this draw is merged
    /// into — same meaning as `sample`'s `--seed`.
    #[arg(long)]
    pub seed: u64,
    #[arg(long, default_value = "audit")]
    pub dir: PathBuf,
    /// Names this promotion event; becomes the reported stratum label
    /// `spot-audit:<event>`.
    #[arg(long)]
    pub event: String,
    /// The tools this event actually promoted — the population this
    /// spot-check draws from, never the whole fleet.
    #[arg(long, value_delimiter = ',')]
    pub promoted: Vec<String>,
    /// How many to draw (spec §13.1b asks for 5-10). Fewer promoted tools
    /// than this are all audited and the shortfall is reported.
    #[arg(long, default_value_t = 8)]
    pub sample: usize,
    /// Seed for the reproducible random draw over `--promoted`, mixed
    /// with `--event` (via `crate::rng::stratum_seed`) so two
    /// promotion events never share a correlated draw pattern.
    #[arg(long)]
    pub draw_seed: u64,
}
