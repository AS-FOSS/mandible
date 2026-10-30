//! `xtask audit replay` and `audit truth`: what each release of mandible
//! read of the tools in an audit batch, as one comparable tree hash. Scoring
//! is in docs/accuracy.md.
//!
//! Replay builds each ref with a small harness (`harness/`) that runs that
//! version's pipeline (`Runner`, `default_tiers`, `resolve_tool`, `fill_node`,
//! stable since 0.1.0) and dumps the tree; [`tree`] reduces it to the
//! canonical form that is hashed. Runs are namespace-contained like a
//! full-`PATH` sweep. Output: `<seed>/replay/<ref>.json` and
//! `target/replay/<seed>/<ref>/<tool>.json`. Truth writes the current
//! build's canonical tree to `<seed>/truth/<tool>.json`.

mod run;
pub(crate) mod sha256;
mod source;
mod tree;

#[path = "harness/dump.rs"]
mod dump;

use anyhow::{bail, Context};
use mandible_extract::exec::{InertArgv, LiveProbe, Probe};
use rayon::prelude::*;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use super::ReplayArgs;
use run::{run_harness, Outcome};
use sha256::sha256_hex;

/// How many tools run through one harness at a time.
const PARALLEL_TOOLS: usize = 4;
/// The error text for a tool that resolves to no binary.
const NOT_FOUND: &str = "not found on PATH";
/// Cap on the one root `--help` capture per tool.
const HELP_CAP: Duration = Duration::from_secs(10);

/// One tool's line in a replay file.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct ToolRecord {
    /// `ok`, `error` or `timeout`.
    pub status: &'static str,
    /// Short reason, only when not `ok`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// sha256 of the root `--help` bytes captured in this run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help_sha256: Option<String>,
    /// sha256 of the canonical normalized tree.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tree_sha256: Option<String>,
}

/// `<seed>/replay/<ref>.json`.
#[derive(Debug, Serialize)]
pub struct ReplayFile {
    /// The batch seed.
    pub seed: u64,
    /// The ref as given.
    #[serde(rename = "ref")]
    pub git_ref: String,
    /// The workspace version at that commit.
    pub version: String,
    /// Full commit sha.
    pub commit: String,
    /// Date of the run, `YYYY-MM-DD`.
    pub generated: String,
    /// One record per tool of the batch, by name.
    pub tools: BTreeMap<String, ToolRecord>,
}

impl ReplayFile {
    /// Pretty JSON with a trailing newline, the committed form.
    pub fn to_json(&self) -> anyhow::Result<String> {
        let mut text = serde_json::to_string_pretty(self)?;
        text.push('\n');
        Ok(text)
    }
}

/// `(major, minor, patch)` of a `vX.Y.Z` tag.
fn parse_tag(tag: &str) -> Option<(u64, u64, u64)> {
    let mut parts = tag.strip_prefix('v')?.split('.');
    let triple = (
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    );
    parts.next().is_none().then_some(triple)
}

/// The last patch tag of every minor earlier than `judged`, plus the last
/// tag of the judged minor itself that `precedes` the judged build (an
/// untagged commit has earlier tags in its own minor). Oldest first.
fn per_minor_tags(
    tags: &[String],
    judged: (u64, u64),
    precedes: &dyn Fn(&str) -> bool,
) -> Vec<String> {
    let mut last: BTreeMap<(u64, u64), (u64, String)> = BTreeMap::new();
    for tag in tags {
        let Some((major, minor, patch)) = parse_tag(tag) else {
            continue;
        };
        let minor_key = (major, minor);
        if minor_key > judged || (minor_key == judged && !precedes(tag)) {
            continue;
        }
        let slot = last
            .entry(minor_key)
            .or_insert_with(|| (patch, tag.clone()));
        if patch >= slot.0 {
            *slot = (patch, tag.clone());
        }
    }
    last.into_values().map(|(_, tag)| tag).collect()
}

/// The refs to replay: the per-minor tags (when asked), then `extra`, then
/// the judged ref, without repeats.
fn plan_refs(
    tags: &[String],
    judged_ref: &str,
    judged: (u64, u64),
    precedes: &dyn Fn(&str) -> bool,
    extra: &[String],
    per_minor: bool,
) -> Vec<String> {
    let mut refs = Vec::new();
    let mut push = |r: &str| {
        if !refs.iter().any(|x| x == r) {
            refs.push(r.to_string());
        }
    };
    if per_minor {
        per_minor_tags(tags, judged, precedes)
            .iter()
            .for_each(|t| push(t));
    }
    extra.iter().for_each(|r| push(r));
    push(judged_ref);
    refs
}

struct Batch {
    tools: Vec<String>,
    mandible_version: Option<String>,
}

fn load_batch(dir: &Path, seed: u64) -> anyhow::Result<Batch> {
    let path = dir.join(format!("{seed}.toml"));
    let file = mandible_core::audit::load(&path)?;
    let raw: toml::Value = toml::from_str(&std::fs::read_to_string(&path)?)?;
    let mandible_version = raw
        .get("meta")
        .and_then(|m| m.get("mandible_version"))
        .and_then(toml::Value::as_str)
        .map(str::to_string);
    Ok(Batch {
        tools: file.entries.into_iter().map(|e| e.tool).collect(),
        mandible_version,
    })
}

/// The root `--help` bytes (stdout then stderr), hashed.
fn help_sha256(tool: &str) -> Result<String, String> {
    let resolved = mandible_extract::resolve_tool(tool);
    let path = resolved.path.ok_or(NOT_FOUND)?;
    let out = LiveProbe
        .run(&path, &InertArgv::HelpLong, HELP_CAP)
        .map_err(|e| format!("--help probe failed: {e}"))?;
    let mut bytes = out.stdout;
    bytes.extend_from_slice(&out.stderr);
    Ok(sha256_hex(&bytes))
}

fn record_for(
    outcome: Outcome,
    tool: &str,
    help: &Result<String, String>,
    tree_path: &Path,
) -> ToolRecord {
    let help_sha256 = help.as_ref().ok().cloned();
    let failed = |status, error: String| ToolRecord {
        status,
        error: Some(error),
        help_sha256: help_sha256.clone(),
        tree_sha256: None,
    };
    match outcome {
        Outcome::Timeout => failed("timeout", format!("over {}s", run::TOOL_CAP.as_secs())),
        Outcome::Error(e) => failed("error", e),
        Outcome::Dump(dump) => {
            if help.as_ref().is_err_and(|e| e == NOT_FOUND) {
                return failed("error", NOT_FOUND.to_string());
            }
            let node = tree::normalize_dump(tool, &dump);
            let bytes = node.canonical_bytes();
            if let Err(e) = std::fs::write(tree_path, &bytes) {
                return failed("error", format!("writing tree: {e}"));
            }
            ToolRecord {
                status: "ok",
                error: None,
                help_sha256,
                tree_sha256: Some(node.sha256()),
            }
        }
    }
}

fn replay_one_ref(
    args: &ReplayArgs,
    root: &Path,
    git_ref: &str,
    tools: &[String],
    helps: &BTreeMap<String, Result<String, String>>,
) -> anyhow::Result<()> {
    println!("{git_ref}: building");
    let prepared = source::prepare(root, git_ref)?;
    let name = source::ref_dir_name(git_ref);
    let trees = root.join(args.seed.to_string()).join(&name);
    std::fs::create_dir_all(&trees)?;
    let work_root = root.join("work").join(&name);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(PARALLEL_TOOLS)
        .build()?;
    let records: Vec<(String, ToolRecord)> = pool.install(|| {
        tools
            .par_iter()
            .map(|tool| {
                let file = super::sanitize_filename(tool);
                let outcome =
                    run_harness(&prepared.bin, tool, &work_root.join(&file), run::TOOL_CAP);
                let rec = record_for(
                    outcome,
                    tool,
                    &helps[tool],
                    &trees.join(format!("{file}.json")),
                );
                (tool.clone(), rec)
            })
            .collect()
    });
    let _ = std::fs::remove_dir_all(&work_root);
    let counts = |s| records.iter().filter(|(_, r)| r.status == s).count();
    println!(
        "{git_ref}: {} ok, {} error, {} timeout",
        counts("ok"),
        counts("error"),
        counts("timeout")
    );
    let file = ReplayFile {
        seed: args.seed,
        git_ref: git_ref.to_string(),
        version: prepared.version,
        commit: prepared.commit,
        generated: crate::queue::today_iso8601(),
        tools: records.into_iter().collect(),
    };
    let out_dir = args.dir.join(args.seed.to_string()).join("replay");
    std::fs::create_dir_all(&out_dir)?;
    let path = out_dir.join(format!("{name}.json"));
    std::fs::write(&path, file.to_json()?)?;
    println!("wrote {}", path.display());
    Ok(())
}

/// `audit replay`.
pub fn cmd_replay(args: &ReplayArgs) -> anyhow::Result<()> {
    let batch = load_batch(&args.dir, args.seed)?;
    let judged_ref = match (&args.judged_ref, &batch.mandible_version) {
        (Some(r), _) => r.clone(),
        (None, Some(v)) => format!("v{v}"),
        (None, None) => bail!(
            "seed {} has no [meta] mandible_version; pass --judged-ref",
            args.seed
        ),
    };
    let judged_commit = source::resolve_commit(&judged_ref)?;
    let judged_version = source::version_at(&judged_commit)?;
    let judged = parse_tag(&format!("v{judged_version}"))
        .map(|(major, minor, _)| (major, minor))
        .with_context(|| format!("unreadable version {judged_version}"))?;
    let tags: Vec<String> = source::git(&["tag"])?.lines().map(str::to_string).collect();
    let precedes = |tag: &str| source::is_ancestor(tag, &judged_commit);
    let refs = plan_refs(
        &tags,
        &judged_ref,
        judged,
        &precedes,
        &args.refs,
        args.per_minor,
    );

    let canaries = crate::sweep_guard(true, args.allow_uncontained, None)?;
    let result = replay_all(args, &refs, &batch.tools);
    crate::finish_sweep_guard(canaries)?;
    result
}

fn replay_all(args: &ReplayArgs, refs: &[String], tools: &[String]) -> anyhow::Result<()> {
    println!(
        "replaying {} ref(s) over {} tool(s)",
        refs.len(),
        tools.len()
    );
    let helps: BTreeMap<String, Result<String, String>> =
        tools.iter().map(|t| (t.clone(), help_sha256(t))).collect();
    let root = std::path::absolute("target/replay")?;
    for git_ref in refs {
        replay_one_ref(args, &root, git_ref, tools, &helps)?;
    }
    Ok(())
}

/// `audit truth`: write the current build's tree for `tool`.
pub fn cmd_truth(seed: u64, tool: &str, dir: &Path) -> anyhow::Result<()> {
    let file = mandible_core::audit::load(&dir.join(format!("{seed}.toml")))?;
    let Some(entry) = file.entries.iter().find(|e| e.tool == tool) else {
        bail!("{tool} is not in seed {seed}");
    };
    match entry.verdict.as_deref() {
        Some("incomplete" | "wrong") => {}
        Some(other) => bail!("{tool} was judged {other}; a truth file is for incomplete or wrong"),
        None => bail!("{tool} has no verdict yet"),
    }
    let dump = dump::dump(tool);
    let node = tree::normalize_dump(tool, &dump);
    let bytes = node.canonical_bytes();
    let out_dir = dir.join(seed.to_string()).join("truth");
    std::fs::create_dir_all(&out_dir)?;
    let path = out_dir.join(format!("{}.json", super::sanitize_filename(tool)));
    std::fs::write(&path, &bytes)?;
    println!(
        "wrote {} ({} bytes, {} flags, {} positionals, {} subcommands at the root, sha256 {})",
        path.display(),
        bytes.len(),
        node.flags.len(),
        node.positionals.len(),
        node.subcommands.len(),
        node.sha256()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn tags_parse_strictly() {
        assert_eq!(parse_tag("v0.8.4"), Some((0, 8, 4)));
        assert_eq!(parse_tag("v0.8"), None);
        assert_eq!(parse_tag("v0.8.4.1"), None);
        assert_eq!(parse_tag("task25-unparsed"), None);
    }

    #[test]
    fn last_patch_of_each_earlier_minor() {
        let t = tags(&[
            "v0.1.0", "v0.1.7", "v0.1.10", "v0.2.0", "v0.2.2", "v0.8.4", "v0.9.0", "junk",
        ]);
        let none = |_: &str| false;
        assert_eq!(
            per_minor_tags(&t, (0, 9), &|_| true),
            ["v0.1.10", "v0.2.2", "v0.8.4", "v0.9.0"]
        );
        assert_eq!(
            per_minor_tags(&t, (0, 9), &none),
            ["v0.1.10", "v0.2.2", "v0.8.4"]
        );
        assert_eq!(per_minor_tags(&t, (0, 2), &none), ["v0.1.10"]);
    }

    #[test]
    fn judged_minor_keeps_only_tags_before_the_judged_build() {
        let t = tags(&["v0.7.0", "v0.7.1", "v0.8.0"]);
        let before = |tag: &str| tag == "v0.7.0";
        assert_eq!(per_minor_tags(&t, (0, 7), &before), ["v0.7.0"]);
    }

    #[test]
    fn plan_dedupes_and_ends_with_the_judged_ref() {
        let t = tags(&["v0.7.0", "v0.8.4", "v0.9.0"]);
        let none = |_: &str| false;
        let extra = tags(&["v0.8.4", "v0.9.0"]);
        let refs = plan_refs(&t, "abc123", (0, 8), &none, &extra, true);
        assert_eq!(refs, ["v0.7.0", "v0.8.4", "v0.9.0", "abc123"]);
        let refs = plan_refs(&t, "v0.9.0", (0, 9), &|_| true, &[], true);
        assert_eq!(refs, ["v0.7.0", "v0.8.4", "v0.9.0"]);
        assert_eq!(
            plan_refs(&t, "v0.9.0", (0, 9), &none, &[], false),
            ["v0.9.0"]
        );
    }

    #[test]
    fn replay_file_has_the_contract_shape() {
        let mut tools = BTreeMap::new();
        tools.insert(
            "ls".to_string(),
            ToolRecord {
                status: "ok",
                error: None,
                help_sha256: Some("aa".into()),
                tree_sha256: Some("bb".into()),
            },
        );
        tools.insert(
            "slow".to_string(),
            ToolRecord {
                status: "timeout",
                error: Some("over 120s".into()),
                help_sha256: None,
                tree_sha256: None,
            },
        );
        let file = ReplayFile {
            seed: 7,
            git_ref: "v0.7.0".into(),
            version: "0.7.0".into(),
            commit: "c".repeat(40),
            generated: "2026-01-02".into(),
            tools,
        };
        let text = file.to_json().unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["ref"], "v0.7.0");
        assert_eq!(v["seed"], 7);
        assert_eq!(v["tools"]["ls"]["tree_sha256"], "bb");
        assert_eq!(v["tools"]["slow"]["status"], "timeout");
        assert!(v["tools"]["slow"].get("tree_sha256").is_none());
        assert!(text.ends_with("}\n"));
        let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        assert!(keys.contains(&"generated") && keys.contains(&"commit"));
    }

    #[test]
    fn failures_keep_the_help_hash_but_have_no_tree() {
        let help = Ok("h".to_string());
        let rec = record_for(
            Outcome::Timeout,
            "x",
            &help,
            Path::new("/nonexistent/x.json"),
        );
        assert_eq!(rec.status, "timeout");
        assert_eq!(rec.help_sha256.as_deref(), Some("h"));
        assert!(rec.tree_sha256.is_none());
        let rec = record_for(
            Outcome::Error("boom".into()),
            "x",
            &Err("e".into()),
            Path::new("/x"),
        );
        assert_eq!(rec.error.as_deref(), Some("boom"));
        assert!(rec.help_sha256.is_none());
    }
}
