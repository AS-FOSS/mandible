//! `mandible`: the binary. Wires the extraction pipeline, the cache, and the
//! TUI together; also hosts the non-interactive `--doctor` diagnostic.

#![forbid(unsafe_code)]

mod app_runner;
mod background;
mod cli;
mod doctor;
mod pipeline;

use clap::{CommandFactory, Parser};
use cli::Cli;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_env("MANDIBLE_LOG"))
        .with_writer(std::io::stderr)
        .try_init()
        .ok();

    let cli = Cli::parse();

    if let Some(shell) = cli.completions {
        let mut cmd = Cli::command();
        let bin_name = cmd.get_name().to_string();
        clap_complete::generate(shell, &mut cmd, bin_name, &mut std::io::stdout());
        return Ok(());
    }

    let Some(tool) = cli.target_tool() else {
        anyhow::bail!("usage: mandible <tool>  (or: mandible --doctor <tool>)");
    };
    let tool = tool.to_string();

    if let Some(doctor_tool) = &cli.doctor {
        let loaded = pipeline::load(doctor_tool, cli.refresh);
        let ok = loaded.root.is_some();
        doctor::print_report(&loaded);
        // Note: deliberately `anyhow::bail!` rather than
        // `std::process::exit`, so `std::process` stays confined to
        // `mandible-extract/src/exec/` workspace-wide (spec §6, §8) — even
        // though `exit` doesn't spawn anything, keeping the grep-based
        // invariant test literal and unambiguous is worth the minor
        // indirection.
        if !ok {
            anyhow::bail!("doctor: no extraction tier produced a result for {doctor_tool:?}");
        }
        return Ok(());
    }

    if !mandible_tui::terminal::stdout_is_tty() {
        anyhow::bail!(
            "mandible requires an interactive terminal (stdout is not a tty). \
             Try running it directly in a terminal, or use `mandible --doctor {tool}` \
             for a non-interactive report."
        );
    }

    let loaded = pipeline::load(&tool, cli.refresh);
    let Some(root) = loaded.root else {
        anyhow::bail!(
            "no extraction tier could produce a tree for {tool:?}. Run `mandible --doctor {tool}` \
             for details on what was tried."
        );
    };

    let mut app = mandible_tui::App::new(tool, root);
    app.from_cache = loaded.from_cache;

    app_runner::run(app)
}
