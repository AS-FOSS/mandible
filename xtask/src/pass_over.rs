//! The one rule both draws share: a tool that already has a verdict, has a
//! corpus fixture, or is no longer on `PATH` is passed over, never drawn.
//! `xtask audit sample` walks the frozen queue with it ([`draw`]);
//! `xtask audit contribute` filters its `PATH` population with the same
//! [`Gate::reason`]. The rule is what keeps a batch about tools mandible
//! was not tuned for (docs/accuracy.md).

use crate::queue::QueueEntry;
use mandible_core::audit::{AuditFile, PassedOver};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

/// Why a tool was passed over. Checked in this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// A manifest under the audit dir already holds a verdict for it.
    Verdict,
    /// `corpus/<tool>/` exists.
    Fixture,
    /// It is not an executable on `PATH` any more.
    NotOnPath,
}

impl Reason {
    /// The spelling written to the manifest.
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::Verdict => "verdict",
            Reason::Fixture => "fixture",
            Reason::NotOnPath => "not-on-path",
        }
    }
}

/// A `<digits>.toml` file name is a verdict manifest; `queue.toml` and the
/// rest are not.
pub(crate) fn verdict_file_seed(path: &Path) -> Option<u64> {
    let name = path.file_name()?.to_str()?;
    let stem = name.strip_suffix(".toml")?;
    if stem.is_empty() || !stem.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    stem.parse::<u64>().ok()
}

/// Everything the rule consults, read once.
#[derive(Debug, Default)]
pub struct Gate {
    verdicts: HashSet<String>,
    fixtures: HashSet<String>,
    on_path: Option<HashSet<String>>,
}

impl Gate {
    /// Reads the verdicts of every manifest in `audit_dir` and under
    /// `audit_dir/submissions/`, and the fixture directories of
    /// `corpus_root`. Without [`Gate::with_path`] no tool counts as gone
    /// from `PATH`.
    pub fn load(audit_dir: &Path, corpus_root: &Path) -> anyhow::Result<Gate> {
        let mut gate = Gate::default();
        let audit_dir = if audit_dir.as_os_str().is_empty() {
            Path::new(".")
        } else {
            audit_dir
        };
        gate.read_manifests(audit_dir, 0)?;
        gate.read_manifests(&audit_dir.join("submissions"), 1)?;
        if corpus_root.is_dir() {
            for entry in read_dir(corpus_root)? {
                if entry.file_type()?.is_dir() {
                    if let Some(name) = entry.file_name().to_str() {
                        gate.fixtures.insert(name.to_string());
                    }
                }
            }
        }
        Ok(gate)
    }

    /// Passes over any tool absent from `on_path`.
    pub fn with_path(mut self, on_path: HashSet<String>) -> Gate {
        self.on_path = Some(on_path);
        self
    }

    /// Scans `dir` for manifests; `depth` more levels of subdirectory are
    /// searched (`submissions/<login>/` is one level down).
    fn read_manifests(&mut self, dir: &Path, depth: usize) -> anyhow::Result<()> {
        if !dir.is_dir() {
            return Ok(());
        }
        for entry in read_dir(dir)? {
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                if depth > 0 {
                    self.read_manifests(&path, depth - 1)?;
                }
                continue;
            }
            if verdict_file_seed(&path).is_none() {
                continue;
            }
            // A manifest that fails to parse is skipped: this is a filter,
            // not a validator for someone else's file.
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(file) = toml::from_str::<AuditFile>(&text) else {
                continue;
            };
            for entry in &file.entries {
                if entry.verdict.is_some() {
                    self.verdicts.insert(entry.tool.clone());
                }
            }
        }
        Ok(())
    }

    /// The reason `tool` is passed over, or `None` when it may be drawn.
    pub fn reason(&self, tool: &str) -> Option<Reason> {
        if self.verdicts.contains(tool) {
            Some(Reason::Verdict)
        } else if self.fixtures.contains(tool) {
            Some(Reason::Fixture)
        } else if self.on_path.as_ref().is_some_and(|p| !p.contains(tool)) {
            Some(Reason::NotOnPath)
        } else {
            None
        }
    }
}

fn read_dir(dir: &Path) -> anyhow::Result<Vec<std::fs::DirEntry>> {
    let mut out = Vec::new();
    for entry in
        std::fs::read_dir(dir).map_err(|e| anyhow::anyhow!("reading {}: {e}", dir.display()))?
    {
        out.push(entry?);
    }
    Ok(out)
}

/// What one walk of the queue produced.
#[derive(Debug)]
pub struct Drawn {
    /// The tools to sample, in queue order.
    pub tools: Vec<QueueEntry>,
    /// The tools walked past on the way, with why.
    pub passed_over: Vec<PassedOver>,
    /// Where the next walk starts: past every drawn and passed-over entry.
    pub next_cursor: usize,
}

/// Walks `entries` from `cursor` until `sample_size` tools are drawn or the
/// queue ends, passing over each tool [`Gate::reason`] names. Never wraps.
pub fn draw(entries: &[QueueEntry], cursor: usize, sample_size: usize, gate: &Gate) -> Drawn {
    let mut tools = Vec::new();
    let mut passed_over = Vec::new();
    let mut next_cursor = cursor.min(entries.len());
    while tools.len() < sample_size && next_cursor < entries.len() {
        let entry = &entries[next_cursor];
        next_cursor += 1;
        match gate.reason(&entry.tool) {
            Some(reason) => passed_over.push(PassedOver {
                tool: entry.tool.clone(),
                reason: reason.as_str().to_string(),
            }),
            None => tools.push(entry.clone()),
        }
    }
    Drawn {
        tools,
        passed_over,
        next_cursor,
    }
}

/// How many passed-over tools carry each reason, in name order.
pub fn count_by_reason(passed_over: &[PassedOver]) -> BTreeMap<&str, usize> {
    let mut counts = BTreeMap::new();
    for p in passed_over {
        *counts.entry(p.reason.as_str()).or_insert(0) += 1;
    }
    counts
}

/// `verdict 2, fixture 1`, or `none`.
pub fn reason_summary(passed_over: &[PassedOver]) -> String {
    let counts = count_by_reason(passed_over);
    if counts.is_empty() {
        return "none".to_string();
    }
    counts
        .iter()
        .map(|(reason, n)| format!("{reason} {n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_manifest(dir: &Path, name: &str, tools: &[(&str, Option<&str>)]) {
        std::fs::create_dir_all(dir).unwrap();
        let mut text = String::from("[meta]\nseed = 1\nsample_size = 1\n");
        for (tool, verdict) in tools {
            text.push_str(&format!(
                "\n[[entry]]\ntool = \"{tool}\"\nstratum = \"ok\"\n"
            ));
            if let Some(v) = verdict {
                text.push_str(&format!("verdict = \"{v}\"\n"));
            }
        }
        std::fs::write(dir.join(name), text).unwrap();
    }

    fn queue(tools: &[&str]) -> Vec<QueueEntry> {
        tools
            .iter()
            .map(|t| QueueEntry {
                tool: (*t).to_string(),
                stratum: "ok".to_string(),
            })
            .collect()
    }

    fn names(entries: &[QueueEntry]) -> Vec<&str> {
        entries.iter().map(|e| e.tool.as_str()).collect()
    }

    fn path_of(tools: &[&str]) -> HashSet<String> {
        tools.iter().map(|t| (*t).to_string()).collect()
    }

    #[test]
    fn a_verdict_in_the_audit_dir_or_in_submissions_passes_a_tool_over() {
        let tmp = tempfile::tempdir().unwrap();
        write_manifest(tmp.path(), "7.toml", &[("a", Some("correct")), ("p", None)]);
        write_manifest(
            &tmp.path().join("submissions/alice"),
            "5.toml",
            &[("b", Some("wrong"))],
        );
        write_manifest(tmp.path(), "queue.toml", &[("c", Some("correct"))]);
        let gate = Gate::load(tmp.path(), &tmp.path().join("corpus")).unwrap();
        assert_eq!(gate.reason("a"), Some(Reason::Verdict));
        assert_eq!(gate.reason("b"), Some(Reason::Verdict));
        assert_eq!(gate.reason("p"), None, "a pending entry is not a verdict");
        assert_eq!(gate.reason("c"), None, "queue.toml is not a manifest");
    }

    #[test]
    fn a_corpus_fixture_passes_a_tool_over() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("corpus/less")).unwrap();
        let gate = Gate::load(tmp.path(), &tmp.path().join("corpus")).unwrap();
        assert_eq!(gate.reason("less"), Some(Reason::Fixture));
        assert_eq!(gate.reason("more"), None);
    }

    #[test]
    fn a_tool_gone_from_path_is_passed_over_only_when_path_is_known() {
        let tmp = tempfile::tempdir().unwrap();
        let gate = Gate::load(tmp.path(), tmp.path()).unwrap();
        assert_eq!(gate.reason("gone"), None);
        let gate = gate.with_path(path_of(&["here"]));
        assert_eq!(gate.reason("gone"), Some(Reason::NotOnPath));
        assert_eq!(gate.reason("here"), None);
    }

    #[test]
    fn verdict_outranks_fixture_outranks_not_on_path() {
        let tmp = tempfile::tempdir().unwrap();
        write_manifest(tmp.path(), "1.toml", &[("v", Some("correct"))]);
        std::fs::create_dir_all(tmp.path().join("corpus/v")).unwrap();
        std::fs::create_dir_all(tmp.path().join("corpus/f")).unwrap();
        let gate = Gate::load(tmp.path(), &tmp.path().join("corpus"))
            .unwrap()
            .with_path(path_of(&[]));
        assert_eq!(gate.reason("v"), Some(Reason::Verdict));
        assert_eq!(gate.reason("f"), Some(Reason::Fixture));
        assert_eq!(gate.reason("n"), Some(Reason::NotOnPath));
    }

    #[test]
    fn the_walk_keeps_going_until_n_tools_are_drawn() {
        let tmp = tempfile::tempdir().unwrap();
        write_manifest(tmp.path(), "1.toml", &[("b", Some("correct"))]);
        std::fs::create_dir_all(tmp.path().join("corpus/c")).unwrap();
        let gate = Gate::load(tmp.path(), &tmp.path().join("corpus"))
            .unwrap()
            .with_path(path_of(&["a", "b", "c", "e", "f"]));
        let q = queue(&["a", "b", "c", "d", "e", "f"]);
        let drawn = draw(&q, 0, 3, &gate);
        assert_eq!(names(&drawn.tools), ["a", "e", "f"]);
        let reasons: Vec<_> = drawn
            .passed_over
            .iter()
            .map(|p| (p.tool.as_str(), p.reason.as_str()))
            .collect();
        assert_eq!(
            reasons,
            [("b", "verdict"), ("c", "fixture"), ("d", "not-on-path")]
        );
    }

    #[test]
    fn the_cursor_moves_past_drawn_and_passed_over_entries_and_stops_at_the_nth_draw() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("corpus/b")).unwrap();
        let gate = Gate::load(tmp.path(), &tmp.path().join("corpus")).unwrap();
        let q = queue(&["a", "b", "c", "d"]);
        let first = draw(&q, 0, 2, &gate);
        assert_eq!(names(&first.tools), ["a", "c"]);
        assert_eq!(
            first.next_cursor, 3,
            "past the passed-over b and the drawn c"
        );
        let second = draw(&q, first.next_cursor, 2, &gate);
        assert_eq!(names(&second.tools), ["d"]);
        assert_eq!(second.next_cursor, 4, "capped at the end, never wrapping");
        assert!(draw(&q, second.next_cursor, 2, &gate).tools.is_empty());
    }

    #[test]
    fn a_walk_that_hits_the_end_passes_over_what_it_met() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("corpus/z")).unwrap();
        let gate = Gate::load(tmp.path(), &tmp.path().join("corpus")).unwrap();
        let drawn = draw(&queue(&["a", "z"]), 0, 5, &gate);
        assert_eq!(names(&drawn.tools), ["a"]);
        assert_eq!(drawn.passed_over.len(), 1);
        assert_eq!(drawn.next_cursor, 2);
    }

    #[test]
    fn the_reason_summary_counts_each_reason() {
        let p = |t: &str, r: &str| PassedOver {
            tool: t.to_string(),
            reason: r.to_string(),
        };
        let list = [p("a", "fixture"), p("b", "verdict"), p("c", "fixture")];
        assert_eq!(reason_summary(&list), "fixture 2, verdict 1");
        assert_eq!(reason_summary(&[]), "none");
    }

    #[test]
    fn a_manifest_written_before_passed_over_existed_still_loads() {
        let old =
            "[meta]\nseed = 7\nsample_size = 10\n\n[[entry]]\ntool = \"x\"\nstratum = \"ok\"\n";
        let file: AuditFile = toml::from_str(old).unwrap();
        assert!(file.passed_over.is_empty());
        assert_eq!(file.entries.len(), 1);
    }

    #[test]
    fn passed_over_round_trips_through_save_and_load() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("3.toml");
        let mut file: AuditFile = toml::from_str("[meta]\nseed = 3\nsample_size = 1\n").unwrap();
        file.passed_over.push(PassedOver {
            tool: "less".to_string(),
            reason: "fixture".to_string(),
        });
        mandible_core::audit::save(&path, &file).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("[[passed_over]]"), "{text}");
        let back = mandible_core::audit::load(&path).unwrap();
        assert_eq!(back.passed_over, file.passed_over);
    }

    #[test]
    fn the_report_prints_the_passed_over_count_by_reason() {
        let tmp = tempfile::tempdir().unwrap();
        let mut file: AuditFile = toml::from_str("[meta]\nseed = 3\nsample_size = 1\n").unwrap();
        for (tool, reason) in [("a", "fixture"), ("b", "fixture"), ("c", "verdict")] {
            file.passed_over.push(PassedOver {
                tool: tool.to_string(),
                reason: reason.to_string(),
            });
        }
        mandible_core::audit::save(&mandible_core::audit::verdict_path(tmp.path(), 3), &file)
            .unwrap();
        let report = crate::audit::render_report(tmp.path(), 3).unwrap();
        assert!(
            report.contains("passed over: 3 (fixture 2, verdict 1)"),
            "{report}"
        );
    }
}
