//! `xtask audit history`: the audited accuracy of each release, from the
//! human verdicts of every judged batch and the replay of older releases
//! over the same tools.
//!
//! A release R is scored on a batch only when R is not newer than the
//! batch's judged version. The judged version takes the human verdicts
//! directly. For an older R, per tool, R's tree hash is compared with the
//! judged build's:
//!
//! - equal: the human verdict carries over;
//! - different and judged `correct`: wrong;
//! - different and judged `incomplete`/`wrong`: R is compared with the
//!   tool's truth file (equal: correct, else wrong); with no truth file the
//!   tool is `needs-eye`, left out of the counts and listed;
//! - R failed to run the tool (`error`/`timeout`): wrong.
//!
//! Files read, under the audit dir: `<seed>.toml`, `<seed>/replay/*.json`
//! and `<seed>/truth/*.json`. Output: `history.json` plus a plain table.

use super::batch::sanitize_filename;
use super::report::wilson_interval;
use super::sha256::sha256_hex;
use mandible_core::audit::{load, verdict_path, AuditFile};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

/// A `major.minor.patch` version, ordered numerically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Version(pub(super) u64, pub(super) u64, pub(super) u64);

impl Version {
    pub(super) fn parse(text: &str) -> anyhow::Result<Self> {
        let bare = text.strip_prefix('v').unwrap_or(text);
        let parts: Vec<&str> = bare.split('.').collect();
        let num = |s: &str| s.parse::<u64>().ok();
        match parts.as_slice() {
            [a, b, c] => match (num(a), num(b), num(c)) {
                (Some(a), Some(b), Some(c)) => Ok(Self(a, b, c)),
                _ => anyhow::bail!("{text:?} is not a major.minor.patch version"),
            },
            _ => anyhow::bail!("{text:?} is not a major.minor.patch version"),
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// The human verdicts that count toward accuracy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Verdict {
    Correct,
    Incomplete,
    Wrong,
}

/// One tool's line in a replay file.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct ReplayTool {
    pub(super) status: String,
    #[serde(default)]
    pub(super) help_sha256: Option<String>,
    #[serde(default)]
    pub(super) tree_sha256: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct ReplayFile {
    pub(super) seed: u64,
    #[serde(rename = "ref")]
    pub(super) git_ref: String,
    pub(super) version: String,
    pub(super) tools: BTreeMap<String, ReplayTool>,
}

/// Everything the scoring reads for one batch.
#[derive(Debug, Clone)]
pub(super) struct SeedInput {
    pub(super) seed: u64,
    pub(super) judged: Version,
    pub(super) verdicts: BTreeMap<String, Verdict>,
    pub(super) replays: Vec<ReplayFile>,
    /// Tool name to the sha256 of its truth file.
    pub(super) truth: BTreeMap<String, String>,
    pub(super) tags: Tags,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Tags {
    pub(super) known: usize,
    pub(super) new: usize,
    pub(super) untagged: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Outcome {
    Correct,
    Wrong,
    NeedsEye,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(super) struct NeedsEye {
    pub(super) seed: u64,
    pub(super) tool: String,
    pub(super) command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(super) struct Release {
    pub(super) version: String,
    pub(super) correct: usize,
    pub(super) judged: usize,
    pub(super) needs_eye: Vec<NeedsEye>,
    pub(super) wilson_low: f64,
    pub(super) wilson_high: f64,
    pub(super) seeds: Vec<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(super) struct SeedRow {
    pub(super) seed: u64,
    pub(super) version: String,
    pub(super) known: usize,
    pub(super) new: usize,
    pub(super) untagged: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(super) struct History {
    pub(super) releases: Vec<Release>,
    pub(super) seeds: Vec<SeedRow>,
}

/// Score one tool on an older release against the judged build.
pub(super) fn score_older(
    verdict: Verdict,
    older: &ReplayTool,
    judged: &ReplayTool,
    truth: Option<&str>,
) -> anyhow::Result<Outcome> {
    if older.status != "ok" {
        return Ok(Outcome::Wrong);
    }
    let (Some(old_tree), Some(judged_tree)) = (&older.tree_sha256, &judged.tree_sha256) else {
        anyhow::bail!("a replay entry with status ok has no tree_sha256");
    };
    if old_tree == judged_tree {
        return Ok(carry(verdict));
    }
    if verdict == Verdict::Correct {
        return Ok(Outcome::Wrong);
    }
    Ok(match truth {
        Some(sha) if sha == old_tree => Outcome::Correct,
        Some(_) => Outcome::Wrong,
        None => Outcome::NeedsEye,
    })
}

fn carry(verdict: Verdict) -> Outcome {
    if verdict == Verdict::Correct {
        Outcome::Correct
    } else {
        Outcome::Wrong
    }
}

/// The replay that stands for `version` in `seed`: for the judged version,
/// the one non-tag ref when several share that version.
fn replay_for(seed: &SeedInput, version: Version) -> anyhow::Result<Option<&ReplayFile>> {
    let same: Vec<&ReplayFile> = seed
        .replays
        .iter()
        .filter(|r| Version::parse(&r.version).ok() == Some(version))
        .collect();
    match same.as_slice() {
        [] => Ok(None),
        [one] => Ok(Some(one)),
        many if version == seed.judged => {
            let untagged: Vec<&&ReplayFile> = many
                .iter()
                .filter(|r| !r.git_ref.starts_with('v'))
                .collect();
            match untagged.as_slice() {
                [one] => Ok(Some(**one)),
                _ => anyhow::bail!(
                    "seed {}: {} replays share judged version {version}; expected exactly one \
                     ref that is not a version tag",
                    seed.seed,
                    many.len()
                ),
            }
        }
        _ => anyhow::bail!(
            "seed {}: more than one replay for version {version} ({})",
            seed.seed,
            same.iter()
                .map(|r| r.git_ref.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Fail when two replays of one seed saw different `--help` for a tool.
pub(super) fn check_help_hashes(seed: &SeedInput) -> anyhow::Result<()> {
    let mut seen: BTreeMap<&str, (&str, &str)> = BTreeMap::new();
    for replay in &seed.replays {
        for (tool, entry) in &replay.tools {
            let Some(hash) = entry.help_sha256.as_deref() else {
                continue;
            };
            match seen.get(tool.as_str()) {
                Some((other_ref, other_hash)) if *other_hash != hash => anyhow::bail!(
                    "seed {}: the --help of {tool} differs between {other_ref} and {}; the tools \
                     changed between replays, so the runs are not comparable",
                    seed.seed,
                    replay.git_ref
                ),
                Some(_) => {}
                None => {
                    seen.insert(tool, (&replay.git_ref, hash));
                }
            }
        }
    }
    Ok(())
}

/// Every replay must be for a version not newer than the judged one, and
/// the judged replay must exist.
fn check_refs(seed: &SeedInput) -> anyhow::Result<()> {
    for replay in &seed.replays {
        if replay.seed != seed.seed {
            anyhow::bail!(
                "replay {} is for seed {}, found under seed {}",
                replay.git_ref,
                replay.seed,
                seed.seed
            );
        }
        let version = Version::parse(&replay.version)?;
        if version > seed.judged {
            anyhow::bail!(
                "seed {}: ref {} is version {version}, newer than the judged version {}",
                seed.seed,
                replay.git_ref,
                seed.judged
            );
        }
    }
    if replay_for(seed, seed.judged)?.is_none() {
        anyhow::bail!(
            "seed {}: no replay of the judged version {}; run `xtask audit replay` for it",
            seed.seed,
            seed.judged
        );
    }
    Ok(())
}

fn review_command(git_ref: &str, tool: &str) -> String {
    format!(
        "git worktree add target/review/{git_ref} {git_ref} && cargo run --release \
         --manifest-path target/review/{git_ref}/Cargo.toml -p mandible -- {tool}"
    )
}

/// One release's tally over the batches it applies to.
pub(super) fn score_release(seeds: &[SeedInput], version: Version) -> anyhow::Result<Release> {
    let (mut correct, mut judged) = (0usize, 0usize);
    let mut needs_eye = Vec::new();
    let mut used = Vec::new();
    for seed in seeds.iter().filter(|s| version <= s.judged) {
        let judged_replay = replay_for(seed, seed.judged)?.ok_or_else(|| {
            anyhow::anyhow!("seed {}: no replay of the judged version", seed.seed)
        })?;
        let release_replay = replay_for(seed, version)?.ok_or_else(|| {
            anyhow::anyhow!(
                "seed {}: no replay of {version}, which the batch scores; run `xtask audit \
                 replay` for it",
                seed.seed
            )
        })?;
        used.push(seed.seed);
        for (tool, verdict) in &seed.verdicts {
            let judged_entry = lookup(judged_replay, tool)?;
            if judged_entry.status != "ok" {
                anyhow::bail!(
                    "seed {}: the judged build did not read {tool} ({}); it was judged, so the \
                     replay must succeed",
                    seed.seed,
                    judged_entry.status
                );
            }
            let outcome = if version == seed.judged {
                carry(*verdict)
            } else {
                let truth = seed.truth.get(tool).map(String::as_str);
                score_older(*verdict, lookup(release_replay, tool)?, judged_entry, truth)?
            };
            match outcome {
                Outcome::Correct => {
                    correct += 1;
                    judged += 1;
                }
                Outcome::Wrong => judged += 1,
                Outcome::NeedsEye => needs_eye.push(NeedsEye {
                    seed: seed.seed,
                    tool: tool.clone(),
                    command: review_command(&release_replay.git_ref, tool),
                }),
            }
        }
    }
    let (low, high) = wilson_interval(correct, judged);
    Ok(Release {
        version: version.to_string(),
        correct,
        judged,
        needs_eye,
        wilson_low: low,
        wilson_high: high,
        seeds: used,
    })
}

fn lookup<'a>(replay: &'a ReplayFile, tool: &str) -> anyhow::Result<&'a ReplayTool> {
    replay
        .tools
        .get(tool)
        .ok_or_else(|| anyhow::anyhow!("replay {} has no entry for {tool}", replay.git_ref))
}

/// The whole history over `seeds`, releases sorted by version.
pub(super) fn build_history(seeds: &[SeedInput]) -> anyhow::Result<History> {
    for seed in seeds {
        check_refs(seed)?;
        check_help_hashes(seed)?;
    }
    let versions: BTreeSet<Version> = seeds
        .iter()
        .flat_map(|s| s.replays.iter())
        .map(|r| Version::parse(&r.version))
        .collect::<anyhow::Result<_>>()?;
    let releases = versions
        .into_iter()
        .map(|v| score_release(seeds, v))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let mut rows: Vec<SeedRow> = seeds
        .iter()
        .map(|s| SeedRow {
            seed: s.seed,
            version: s.judged.to_string(),
            known: s.tags.known,
            new: s.tags.new,
            untagged: s.tags.untagged,
        })
        .collect();
    rows.sort_by_key(|r| (Version::parse(&r.version).ok(), r.seed));
    Ok(History {
        releases,
        seeds: rows,
    })
}

fn verdict_of(word: &str) -> Option<Verdict> {
    match word {
        "correct" => Some(Verdict::Correct),
        "incomplete" => Some(Verdict::Incomplete),
        "wrong" => Some(Verdict::Wrong),
        _ => None,
    }
}

/// Verdicts that count (display-only findings, skips and pending excluded)
/// and the failure tag tally, from a manifest.
pub(super) fn verdicts_and_tags(
    file: &AuditFile,
) -> anyhow::Result<(BTreeMap<String, Verdict>, Tags)> {
    let mut verdicts = BTreeMap::new();
    let mut tags = Tags::default();
    for entry in &file.entries {
        entry.validate_failure_tag()?;
        if entry.is_display_only() {
            continue;
        }
        let Some(verdict) = entry.effective_verdict().and_then(verdict_of) else {
            continue;
        };
        verdicts.insert(entry.tool.clone(), verdict);
        if verdict != Verdict::Correct {
            match entry.failure_kind.as_deref() {
                Some("known") => tags.known += 1,
                Some("new") => tags.new += 1,
                _ => tags.untagged += 1,
            }
        }
    }
    Ok((verdicts, tags))
}

fn json_files(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let Ok(read) = std::fs::read_dir(dir) else {
        return Ok(out);
    };
    for item in read {
        let path = item?.path();
        if path.extension().is_some_and(|e| e == "json") {
            out.push(path);
        }
    }
    out.sort();
    Ok(out)
}

fn load_seed(
    dir: &Path,
    seed: u64,
    file: &AuditFile,
    judged_override: Option<&str>,
) -> anyhow::Result<SeedInput> {
    let text = judged_override.unwrap_or(&file.meta.mandible_version);
    let judged = Version::parse(text)?;
    let (verdicts, tags) = verdicts_and_tags(file)?;
    let mut replays = Vec::new();
    for path in json_files(&dir.join(seed.to_string()).join("replay"))? {
        let raw = std::fs::read_to_string(&path)?;
        replays.push(
            serde_json::from_str::<ReplayFile>(&raw)
                .map_err(|e| anyhow::anyhow!("parsing {}: {e}", path.display()))?,
        );
    }
    let mut truth = BTreeMap::new();
    let truth_dir = dir.join(seed.to_string()).join("truth");
    for path in json_files(&truth_dir)? {
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned());
        if let Some(stem) = stem {
            truth.insert(stem, sha256_hex(&std::fs::read(&path)?));
        }
    }
    let truth = verdicts
        .keys()
        .filter_map(|t| {
            truth
                .get(&sanitize_filename(t))
                .map(|h| (t.clone(), h.clone()))
        })
        .collect();
    Ok(SeedInput {
        seed,
        judged,
        verdicts,
        replays,
        truth,
        tags,
    })
}

/// Seeds with a numeric manifest in `dir`, sorted.
fn manifest_seeds(dir: &Path) -> anyhow::Result<Vec<u64>> {
    let mut seeds = Vec::new();
    for item in std::fs::read_dir(dir).map_err(|e| anyhow::anyhow!("{}: {e}", dir.display()))? {
        let path = item?.path();
        if path.extension().is_some_and(|e| e == "toml") {
            if let Some(n) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| s.parse::<u64>().ok())
            {
                seeds.push(n);
            }
        }
    }
    seeds.sort_unstable();
    Ok(seeds)
}

pub(super) fn load_seeds(
    dir: &Path,
    only: Option<u64>,
    judged_override: Option<&str>,
) -> anyhow::Result<Vec<SeedInput>> {
    let seeds = match only {
        Some(s) => vec![s],
        None => manifest_seeds(dir)?,
    };
    let mut out = Vec::new();
    for seed in seeds {
        let file = load(&verdict_path(dir, seed))?;
        if file.meta.mandible_version.is_empty() && judged_override.is_none() {
            continue;
        }
        out.push(load_seed(dir, seed, &file, judged_override)?);
    }
    Ok(out)
}

pub(super) fn render_table(history: &History) -> String {
    let mut out =
        String::from("release   correct/judged   accuracy   95% interval     needs-eye\n");
    for r in &history.releases {
        let acc = if r.judged == 0 {
            "   n/a".to_string()
        } else {
            format!("{:>5.1}%", r.correct as f64 / r.judged as f64 * 100.0)
        };
        out.push_str(&format!(
            "{:<9} {:>7}/{:<7}  {acc}   [{:>5.1}%, {:>5.1}%]   {}\n",
            r.version,
            r.correct,
            r.judged,
            r.wilson_low * 100.0,
            r.wilson_high * 100.0,
            r.needs_eye.len(),
        ));
    }
    out.push_str("\nseed   judged    known   new   untagged\n");
    for s in &history.seeds {
        out.push_str(&format!(
            "{:<6} {:<9} {:>5}   {:>3}   {:>8}\n",
            s.seed, s.version, s.known, s.new, s.untagged
        ));
    }
    for r in &history.releases {
        for n in &r.needs_eye {
            out.push_str(&format!(
                "\nneeds eye: {} seed {} {}\n  {}\n",
                r.version, n.seed, n.tool, n.command
            ));
        }
    }
    out
}

/// `xtask audit history`.
pub fn cmd_history(args: &super::HistoryArgs) -> anyhow::Result<()> {
    write_history(
        &args.dir,
        &args.out,
        args.seed,
        args.judged_version.as_deref(),
    )
}

fn write_history(
    dir: &Path,
    out: &Path,
    seed: Option<u64>,
    judged_version: Option<&str>,
) -> anyhow::Result<()> {
    let seeds = load_seeds(dir, seed, judged_version)?;
    if seeds.is_empty() {
        anyhow::bail!(
            "no manifest in {} records `mandible_version`; pass --seed and --judged-version for \
             a batch that predates it",
            dir.display()
        );
    }
    let history = build_history(&seeds)?;
    if let Some(parent) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let mut json = serde_json::to_string_pretty(&history)?;
    json.push('\n');
    std::fs::write(out, json).map_err(|e| anyhow::anyhow!("writing {}: {e}", out.display()))?;
    print!("{}", render_table(&history));
    println!("\nwrote {}", out.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandible_core::audit::{save, AuditMeta, Entry};

    fn tool(status: &str, help: &str, tree: &str) -> ReplayTool {
        ReplayTool {
            status: status.to_string(),
            help_sha256: Some(help.to_string()),
            tree_sha256: (status == "ok").then(|| tree.to_string()),
        }
    }

    fn replay(seed: u64, git_ref: &str, version: &str, tools: &[(&str, ReplayTool)]) -> ReplayFile {
        ReplayFile {
            seed,
            git_ref: git_ref.to_string(),
            version: version.to_string(),
            tools: tools
                .iter()
                .map(|(n, t)| (n.to_string(), t.clone()))
                .collect(),
        }
    }

    fn seed_input(
        judged: &str,
        verdicts: &[(&str, Verdict)],
        replays: Vec<ReplayFile>,
        truth: &[(&str, &str)],
    ) -> SeedInput {
        SeedInput {
            seed: 7,
            judged: Version::parse(judged).unwrap(),
            verdicts: verdicts.iter().map(|(t, v)| (t.to_string(), *v)).collect(),
            replays,
            truth: truth
                .iter()
                .map(|(t, h)| (t.to_string(), h.to_string()))
                .collect(),
            tags: Tags::default(),
        }
    }

    #[test]
    fn versions_order_numerically_not_textually() {
        let mut v: Vec<Version> = ["0.10.0", "0.9.0", "v0.8.4", "0.2.2"]
            .iter()
            .map(|s| Version::parse(s).unwrap())
            .collect();
        v.sort();
        let shown: Vec<String> = v.iter().map(Version::to_string).collect();
        assert_eq!(shown, ["0.2.2", "0.8.4", "0.9.0", "0.10.0"]);
        assert!(Version::parse("0.9").is_err());
        assert!(Version::parse("a.b.c").is_err());
    }

    #[test]
    fn score_older_covers_every_branch() {
        let ok = |t: &str| tool("ok", "h", t);
        let judged = ok("J");
        // Equal trees: the verdict carries over.
        assert_eq!(
            score_older(Verdict::Correct, &ok("J"), &judged, None).unwrap(),
            Outcome::Correct
        );
        assert_eq!(
            score_older(Verdict::Wrong, &ok("J"), &judged, None).unwrap(),
            Outcome::Wrong
        );
        assert_eq!(
            score_older(Verdict::Incomplete, &ok("J"), &judged, None).unwrap(),
            Outcome::Wrong
        );
        // Different, judged correct: wrong, even with a truth file.
        assert_eq!(
            score_older(Verdict::Correct, &ok("O"), &judged, Some("O")).unwrap(),
            Outcome::Wrong
        );
        // Different, judged a failure: the truth file decides.
        for v in [Verdict::Incomplete, Verdict::Wrong] {
            assert_eq!(
                score_older(v, &ok("O"), &judged, Some("O")).unwrap(),
                Outcome::Correct
            );
            assert_eq!(
                score_older(v, &ok("O"), &judged, Some("T")).unwrap(),
                Outcome::Wrong
            );
            assert_eq!(
                score_older(v, &ok("O"), &judged, None).unwrap(),
                Outcome::NeedsEye
            );
        }
        // The old build failed to run the tool.
        for status in ["error", "timeout"] {
            let failed = tool(status, "h", "");
            assert_eq!(
                score_older(Verdict::Correct, &failed, &judged, None).unwrap(),
                Outcome::Wrong
            );
            assert_eq!(
                score_older(Verdict::Wrong, &failed, &judged, Some("O")).unwrap(),
                Outcome::Wrong
            );
        }
    }

    #[test]
    fn a_release_is_tallied_with_wilson_and_needs_eye_excluded() {
        let j = |t: &str| tool("ok", "h", t);
        let tools = [("a", j("A")), ("b", j("B")), ("c", j("C")), ("d", j("D"))];
        let older = [
            ("a", j("A")),
            ("b", j("X")),
            ("c", tool("error", "h", "")),
            ("d", j("X")),
        ];
        let seed = seed_input(
            "0.9.0",
            &[
                ("a", Verdict::Correct),
                ("b", Verdict::Correct),
                ("c", Verdict::Wrong),
                ("d", Verdict::Incomplete),
            ],
            vec![
                replay(7, "v0.9.0", "0.9.0", &tools),
                replay(7, "v0.8.4", "0.8.4", &older),
            ],
            &[],
        );
        let h = build_history(&[seed]).unwrap();
        let old = &h.releases[0];
        assert_eq!(
            (old.version.as_str(), old.correct, old.judged),
            ("0.8.4", 1, 3)
        );
        assert_eq!(old.needs_eye.len(), 1);
        assert_eq!(old.needs_eye[0].tool, "d");
        assert!(old.needs_eye[0].command.contains("v0.8.4"));
        let (lo, hi) = wilson_interval(1, 3);
        assert_eq!((old.wilson_low, old.wilson_high), (lo, hi));
        let new = &h.releases[1];
        assert_eq!((new.correct, new.judged), (2, 4));
    }

    #[test]
    fn a_release_newer_than_a_batch_does_not_score_it() {
        let t = [("a", tool("ok", "h", "A"))];
        let old = seed_input(
            "0.7.0",
            &[("a", Verdict::Correct)],
            vec![replay(7, "r", "0.7.0", &t)],
            &[],
        );
        let mut new = seed_input(
            "0.9.0",
            &[("a", Verdict::Wrong)],
            vec![
                replay(8, "v0.9.0", "0.9.0", &t),
                replay(8, "v0.7.0", "0.7.0", &t),
            ],
            &[],
        );
        new.seed = 8;
        let h = build_history(&[old, new]).unwrap();
        assert_eq!(h.releases[0].seeds, vec![7, 8]);
        assert_eq!(h.releases[1].seeds, vec![8]);
        assert_eq!((h.releases[1].correct, h.releases[1].judged), (0, 1));
    }

    #[test]
    fn hard_errors_name_what_is_wrong() {
        let t = |h: &str| [("a", tool("ok", h, "A"))];
        // Help text changed between runs.
        let seed = seed_input(
            "0.9.0",
            &[("a", Verdict::Correct)],
            vec![
                replay(7, "v0.9.0", "0.9.0", &t("1")),
                replay(7, "v0.8.4", "0.8.4", &t("2")),
            ],
            &[],
        );
        let err = build_history(&[seed]).unwrap_err().to_string();
        assert!(
            err.contains(" a ") && err.contains("v0.8.4") && err.contains("v0.9.0"),
            "{err}"
        );
        // Judged replay missing.
        let seed = seed_input(
            "0.9.0",
            &[("a", Verdict::Correct)],
            vec![replay(7, "v0.8.4", "0.8.4", &t("1"))],
            &[],
        );
        assert!(build_history(&[seed])
            .unwrap_err()
            .to_string()
            .contains("judged version"));
        // A ref newer than the judged version.
        let seed = seed_input(
            "0.8.4",
            &[("a", Verdict::Correct)],
            vec![
                replay(7, "v0.8.4", "0.8.4", &t("1")),
                replay(7, "v0.9.0", "0.9.0", &t("1")),
            ],
            &[],
        );
        assert!(build_history(&[seed])
            .unwrap_err()
            .to_string()
            .contains("newer"));
        // The judged build must have read the tool.
        let failed = [("a", tool("timeout", "1", ""))];
        let seed = seed_input(
            "0.9.0",
            &[("a", Verdict::Correct)],
            vec![replay(7, "v0.9.0", "0.9.0", &failed)],
            &[],
        );
        assert!(build_history(&[seed])
            .unwrap_err()
            .to_string()
            .contains("judged build"));
        // A batch with no replay of a release it scores.
        let other = seed_input(
            "0.7.0",
            &[("a", Verdict::Correct)],
            vec![replay(7, "v0.7.0", "0.7.0", &t("1"))],
            &[],
        );
        let mut full = seed_input(
            "0.9.0",
            &[("a", Verdict::Correct)],
            vec![replay(8, "v0.9.0", "0.9.0", &t("1"))],
            &[],
        );
        full.seed = 8;
        assert!(build_history(&[other, full])
            .unwrap_err()
            .to_string()
            .contains("no replay of 0.7.0"));
    }

    #[test]
    fn the_judged_version_prefers_the_ref_that_is_not_a_tag() {
        let t = [("a", tool("ok", "h", "A"))];
        let seed = seed_input(
            "0.7.0",
            &[("a", Verdict::Correct)],
            vec![
                replay(7, "v0.7.0", "0.7.0", &[("a", tool("ok", "h", "Z"))]),
                replay(7, "0b30c156", "0.7.0", &t),
            ],
            &[],
        );
        let picked = replay_for(&seed, seed.judged).unwrap().unwrap();
        assert_eq!(picked.git_ref, "0b30c156");
    }

    fn manifest(dir: &Path, seed: u64, version: &str, verdicts: &[(&str, &str, Option<&str>)]) {
        let entries = verdicts
            .iter()
            .map(|(tool, verdict, kind)| Entry {
                tool: tool.to_string(),
                stratum: "ok".to_string(),
                verdict: Some(verdict.to_string()),
                note: "n".to_string(),
                k1: None,
                k2: None,
                k3: None,
                include_reason: None,
                spot_audit_event: None,
                families: Vec::new(),
                families_derived: None,
                amendments: Vec::new(),
                failure_kind: kind.map(str::to_string),
                shape: None,
            })
            .collect();
        let meta = AuditMeta {
            seed,
            sample_size: verdicts.len(),
            mandible_version: version.to_string(),
            ..Default::default()
        };
        save(&verdict_path(dir, seed), &AuditFile { meta, entries }).unwrap();
    }

    fn write(path: PathBuf, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn history_is_built_from_files_and_the_truth_file_hash_counts() {
        let dir = tempfile::tempdir().unwrap();
        manifest(
            dir.path(),
            7,
            "0.9.0",
            &[
                ("a", "correct", None),
                ("b", "wrong", Some("known")),
                ("c", "skip", None),
            ],
        );
        let tree = |t: &str| format!(r#"{{"status":"ok","help_sha256":"h","tree_sha256":"{t}"}}"#);
        let file = |r: &str, v: &str, a: &str, b: &str| {
            format!(
                r#"{{"seed":7,"ref":"{r}","version":"{v}","tools":{{"a":{},"b":{}}}}}"#,
                tree(a),
                tree(b)
            )
        };
        let truth_bytes = b"{\"path\":[\"b\"]}";
        let truth_hash = sha256_hex(truth_bytes);
        write(
            dir.path().join("7/replay/v0.9.0.json"),
            &file("v0.9.0", "0.9.0", "A", "B"),
        );
        write(
            dir.path().join("7/replay/v0.8.4.json"),
            &file("v0.8.4", "0.8.4", "A", &truth_hash),
        );
        write(
            dir.path().join("7/truth/b.json"),
            std::str::from_utf8(truth_bytes).unwrap(),
        );
        let out = dir.path().join("history.json");
        write_history(dir.path(), &out, None, None).unwrap();
        let history: History =
            serde_json::from_str(&std::fs::read_to_string(out).unwrap()).unwrap();
        assert_eq!(history.releases.len(), 2);
        assert_eq!(
            (history.releases[0].correct, history.releases[0].judged),
            (2, 2)
        );
        assert_eq!(
            (history.releases[1].correct, history.releases[1].judged),
            (1, 2)
        );
        assert_eq!(
            history.seeds,
            vec![SeedRow {
                seed: 7,
                version: "0.9.0".into(),
                known: 1,
                new: 0,
                untagged: 0
            }]
        );
    }

    #[test]
    fn manifests_without_a_version_are_skipped_unless_overridden() {
        let dir = tempfile::tempdir().unwrap();
        manifest(dir.path(), 7, "", &[("a", "correct", None)]);
        assert!(load_seeds(dir.path(), None, None).unwrap().is_empty());
        let seeds = load_seeds(dir.path(), Some(7), Some("0.7.0")).unwrap();
        assert_eq!(seeds[0].judged, Version(0, 7, 0));
    }

    #[test]
    fn failure_tags_are_counted_and_misplaced_ones_rejected() {
        let dir = tempfile::tempdir().unwrap();
        manifest(
            dir.path(),
            7,
            "0.9.0",
            &[
                ("a", "wrong", Some("new")),
                ("b", "incomplete", Some("known")),
                ("c", "wrong", None),
                ("d", "correct", None),
            ],
        );
        let file = load(&verdict_path(dir.path(), 7)).unwrap();
        let (_, tags) = verdicts_and_tags(&file).unwrap();
        assert_eq!(
            tags,
            Tags {
                known: 1,
                new: 1,
                untagged: 1
            }
        );
        manifest(dir.path(), 8, "0.9.0", &[("a", "correct", Some("new"))]);
        let bad = load(&verdict_path(dir.path(), 8)).unwrap();
        assert!(verdicts_and_tags(&bad).is_err());
    }

    #[test]
    fn report_shows_the_failure_tag_counts() {
        let dir = tempfile::tempdir().unwrap();
        manifest(
            dir.path(),
            7,
            "0.9.0",
            &[("a", "wrong", Some("new")), ("b", "wrong", None)],
        );
        let text = super::super::render_report(dir.path(), 7).unwrap();
        assert!(
            text.contains("failures: 0 known, 1 new, 1 untagged"),
            "{text}"
        );
        assert!(text.contains("mandible_version=0.9.0"));
    }
}
