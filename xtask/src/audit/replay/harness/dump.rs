//! Extract one tool's full tree with whichever mandible this file is
//! compiled against, and return it as serde JSON. The same source is
//! compiled into the replay harness of every old checkout and into
//! `xtask`, so every build reads a tool through identical walking code and
//! only the parser underneath differs.

use mandible_core::CommandNode;
use mandible_extract::{default_tiers, resolve_tool, ResolvedTool, Runner};

/// Cap on nodes filled below the root, the corpus runner's own bound.
const MAX_NODES: usize = 4096;

/// Root extraction followed by a recursive `fill_node` into every
/// discovered subcommand. `{"root": null}` when no tier produced a root.
pub fn dump(tool: &str) -> serde_json::Value {
    let runner = Runner::new(default_tiers());
    let resolved = resolve_tool(tool);
    let Some(root) = runner.extract_full_for(&resolved).root else {
        return serde_json::json!({ "root": null });
    };
    let mut budget = MAX_NODES.saturating_sub(1);
    let path = vec![resolved.name.clone()];
    let tree = warm(&runner, &resolved, root, &path, &mut budget);
    serde_json::json!({ "root": serde_json::to_value(&tree).unwrap_or(serde_json::Value::Null) })
}

fn warm(
    runner: &Runner,
    resolved: &ResolvedTool,
    mut node: CommandNode,
    path: &[String],
    budget: &mut usize,
) -> CommandNode {
    let children = std::mem::take(&mut node.subcommands);
    let mut filled = Vec::with_capacity(children.len());
    for child in children {
        if *budget == 0 {
            filled.push(child);
            continue;
        }
        *budget -= 1;
        let mut child_path = path.to_vec();
        child_path.push(child.name.clone());
        let fill = runner.fill_node(resolved, &child_path, child);
        filled.push(warm(runner, resolved, fill.node, &child_path, budget));
    }
    node.subcommands = filled;
    node
}
