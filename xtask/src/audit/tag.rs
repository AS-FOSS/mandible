//! `xtask audit tag`: set `failure_kind` (and optionally `shape`) on one
//! entry of a manifest, editing only that entry so comments, key order and
//! every other entry stay as they are.

use mandible_core::audit::{load, verdict_path, FAILURE_KINDS};
use std::path::Path;
use toml_edit::{value, DocumentMut, Item};

/// Apply the tag to the entry for `tool`; the entry must be a judged defect.
pub fn cmd_tag(args: &super::TagArgs) -> anyhow::Result<()> {
    tag_entry(
        &args.dir,
        args.seed,
        &args.tool,
        &args.kind,
        args.shape.as_deref(),
    )
}

fn tag_entry(
    dir: &Path,
    seed: u64,
    tool: &str,
    kind: &str,
    shape: Option<&str>,
) -> anyhow::Result<()> {
    if !FAILURE_KINDS.contains(&kind) {
        anyhow::bail!("--kind must be one of: {}", FAILURE_KINDS.join(", "));
    }
    let path = verdict_path(dir, seed);
    let file = load(&path)?;
    let Some(entry) = file.entries.iter().find(|e| e.tool == tool) else {
        anyhow::bail!("{tool:?} is not in {}", path.display());
    };
    if !entry.is_judged_defect() {
        anyhow::bail!(
            "{tool:?} is {:?}: only incomplete and wrong entries take a failure kind",
            entry.effective_verdict().unwrap_or("pending")
        );
    }
    let mut doc: DocumentMut = std::fs::read_to_string(&path)?
        .parse()
        .map_err(|e| anyhow::anyhow!("parsing {}: {e}", path.display()))?;
    let table = doc
        .get_mut("entry")
        .and_then(Item::as_array_of_tables_mut)
        .and_then(|entries| {
            entries
                .iter_mut()
                .find(|t| t.get("tool").and_then(Item::as_str) == Some(tool))
        })
        .ok_or_else(|| anyhow::anyhow!("{tool:?} has no [[entry]] table in {}", path.display()))?;
    table["failure_kind"] = value(kind);
    if let Some(shape) = shape {
        table["shape"] = value(shape);
    }
    std::fs::write(&path, doc.to_string())
        .map_err(|e| anyhow::anyhow!("writing {}: {e}", path.display()))?;
    println!(
        "tagged {tool} in {}: {kind}{}",
        path.display(),
        shape.map(|s| format!(" ({s})")).unwrap_or_default()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = "# keep this comment\n[meta]\nseed = 3\nsample_size = 3\n\n\
        [[entry]]\ntool = \"a\"\nstratum = \"ok\"\nverdict = \"wrong\"\nnote = \"bad\"\n\n\
        # between entries\n[[entry]]\ntool = \"b\"\nstratum = \"ok\"\nverdict = \"correct\"\n\n\
        [[entry]]\ntool = \"c\"\nstratum = \"ok\"\nverdict = \"incomplete\"\nnote = \"half\"\n";

    fn setup() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("3.toml"), MANIFEST).unwrap();
        dir
    }

    #[test]
    fn tag_edits_only_the_named_entry_and_keeps_comments() {
        let dir = setup();
        tag_entry(dir.path(), 3, "a", "known", Some("S-117 bundled")).unwrap();
        let after = std::fs::read_to_string(dir.path().join("3.toml")).unwrap();
        assert!(after.contains("# keep this comment") && after.contains("# between entries"));
        let expected = MANIFEST.replace(
            "note = \"bad\"\n",
            "note = \"bad\"\nfailure_kind = \"known\"\nshape = \"S-117 bundled\"\n",
        );
        assert_eq!(after, expected);
        let file = load(&dir.path().join("3.toml")).unwrap();
        assert_eq!(file.entries[2].failure_kind, None);
    }

    #[test]
    fn tag_can_be_changed_and_rejects_bad_targets() {
        let dir = setup();
        tag_entry(dir.path(), 3, "c", "known", Some("S-1")).unwrap();
        tag_entry(dir.path(), 3, "c", "new", None).unwrap();
        let file = load(&dir.path().join("3.toml")).unwrap();
        assert_eq!(file.entries[2].failure_kind.as_deref(), Some("new"));
        assert_eq!(file.entries[2].shape.as_deref(), Some("S-1"));
        assert!(
            tag_entry(dir.path(), 3, "b", "new", None).is_err(),
            "correct entry"
        );
        assert!(
            tag_entry(dir.path(), 3, "zzz", "new", None).is_err(),
            "unknown tool"
        );
        assert!(
            tag_entry(dir.path(), 3, "a", "maybe", None).is_err(),
            "bad kind"
        );
        let untouched = std::fs::read_to_string(dir.path().join("3.toml")).unwrap();
        assert!(!untouched.contains("maybe"));
    }
}
