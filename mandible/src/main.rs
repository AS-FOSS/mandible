//! `mandible`: the binary. Wires the extraction pipeline, the cache, and the
//! TUI together; also hosts the non-interactive `--doctor` diagnostic.

#![forbid(unsafe_code)]

mod about;
mod app_runner;
mod background;
mod cli;
mod doctor;
mod pipeline;
mod report;

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
        anyhow::bail!(
            "usage: mandible <tool>  (or: mandible --doctor <tool>, mandible --report <tool>)"
        );
    };
    let tool = tool.to_string();

    // `mandible mandible` shows the about screen rather than extracting
    // the binary's own `--help`. Self-introspection is still available
    // through `mandible --doctor mandible`, which runs the real pipeline
    // against it — the form anyone actually wants for that purpose.
    if cli.doctor.is_none() && cli.report.is_none() && tool == env!("CARGO_PKG_NAME") {
        about::print();
        return Ok(());
    }

    let resolved = mandible_extract::resolve_tool(&tool);
    // Process-signalling and machine-state tools are no longer refused
    // outright: `--help` is measured harmless on all of them and is where
    // their flag list lives, so they open like any other tool and the exec
    // chokepoint restricts them to that one shape (spec §6 rule 0). The
    // visible consequence is that they never gain a subcommand tree, which
    // is correct — they do not have one.

    if let Some(report_tool) = &cli.report {
        let loaded = pipeline::load(report_tool);
        report::print_report(&loaded);
        // Unlike `--doctor` below, this never bails on an empty root: the
        // whole point of `--report` is to hand a maintainer *something*
        // paste-ready even when extraction found nothing at all — the
        // printed block already says so (`doctor::build_report`'s own
        // "no tier produced a root node" line), and a non-zero exit here
        // would just make that block harder to pipe/redirect for no
        // benefit.
        return Ok(());
    }

    if let Some(doctor_tool) = &cli.doctor {
        let loaded = pipeline::load(doctor_tool);
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

    // Startup does *no* extraction. Resolving the name on `PATH` is a
    // filesystem lookup with no subprocess spawn, so it stays here — it is
    // the one failure worth reporting on the command line rather than
    // inside a TUI the user then has to quit. Everything else, including
    // the root node's own `--help` probe, happens on the background warmer
    // once the first frame is already on screen.
    //
    // Blocking here is what made launching feel slow: extracting the root
    // synchronously cost ~1.1s for `gh` and ~0.7s for `docker` before a
    // single pixel was drawn, and the cobra-style tools where that matters
    // most are exactly the ones with the biggest trees.
    if resolved.path.is_none() {
        anyhow::bail!(
            "{tool:?} was not found on PATH. Run `mandible --doctor {tool}` for details on \
             what was tried."
        );
    }
    let stub = mandible_core::CommandNode::new(tool.clone(), mandible_core::Provenance::default());
    let app = mandible_tui::App::new(tool, stub);

    app_runner::run(app)
}
