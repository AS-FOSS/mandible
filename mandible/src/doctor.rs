//! `mandible --doctor <tool>`: a non-TUI diagnostic (spec §5.3).
//!
//! Prints the detected framework (spec §7 Tier A′), tier statuses,
//! node/flag counts, and the percentage of flags with a description — the
//! primary way to verify extraction behavior without a terminal.

use crate::pipeline::LoadedTool;
use mandible_core::CommandNode;

/// Print the diagnostic report for `loaded` to stdout.
pub fn print_report(loaded: &LoadedTool) {
    println!("mandible --doctor {}", loaded.tool);
    println!();

    let resolved = mandible_extract::resolve_tool(&loaded.tool);
    let framework = mandible_extract::framework::identify(&resolved);
    println!("framework:  {}", framework.describe());
    println!();

    println!("tiers:");
    for status in &loaded.tier_statuses {
        let state = if !status.detected {
            "not detected".to_string()
        } else if let Some(err) = &status.error {
            format!("FAILED: {err}")
        } else {
            "ok".to_string()
        };
        println!("  {:<28} {}", status.tier, state);
    }
    println!();

    match &loaded.root {
        Some(root) => {
            let nodes = count_nodes(root);
            let flags = count_flags(root);
            let described = count_described(root);
            let pct = if flags == 0 {
                0.0
            } else {
                described as f64 / flags as f64 * 100.0
            };
            println!("nodes:      {nodes}");
            println!("flags:      {flags} ({pct:.1}% described)");
        }
        None => {
            println!(
                "result:     no tier produced a root node for {:?}",
                loaded.tool
            );
        }
    }
    println!();

    // Spec §11: there is no cache — every extraction is fresh.
    println!("elapsed:    {:.2}ms", loaded.elapsed.as_secs_f64() * 1000.0);
}

fn count_nodes(node: &CommandNode) -> usize {
    1 + node.subcommands.iter().map(count_nodes).sum::<usize>()
}

fn count_flags(node: &CommandNode) -> usize {
    node.flags.len() + node.subcommands.iter().map(count_flags).sum::<usize>()
}

fn count_described(node: &CommandNode) -> usize {
    node.flags
        .iter()
        .filter(|f| f.description.is_some())
        .count()
        + node.subcommands.iter().map(count_described).sum::<usize>()
}
