//! Command-line arguments of `audit tag`, `audit history` and `audit chart`.

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
