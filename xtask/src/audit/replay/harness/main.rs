//! Replay harness entry point: `replay-harness <tool> <out.json>` writes the
//! tool's raw serialized tree to `<out.json>`.

mod dump;

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let (Some(tool), Some(out)) = (args.next(), args.next()) else {
        return Err("usage: replay-harness <tool> <out.json>".to_string());
    };
    let bytes = serde_json::to_vec(&dump::dump(&tool)).map_err(|e| e.to_string())?;
    std::fs::write(&out, bytes).map_err(|e| format!("writing {out}: {e}"))
}
