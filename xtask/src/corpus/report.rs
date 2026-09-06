//! The public `CorpusReport`/`ReplayedFixture` types and the `replay_version_for_tools`/`show_fixture` entry points.
use super::*;

/// The outcome of a full corpus run.
pub struct CorpusReport {
    /// Human-readable per-fixture results plus a summary line.
    pub text: String,
    /// Every reason the run should fail (contract/snapshot/strict-xfail
    /// violations, plus any parse-time-ceiling violations), empty when
    /// everything is clean. Always empty in `--bless` mode.
    pub failures: Vec<String>,
    pub(crate) bless: bool,
}

impl CorpusReport {
    /// True when this run should exit non-zero.
    pub fn failed(&self) -> bool {
        !self.bless && !self.failures.is_empty()
    }
}

/// Print one fixture's captured help text beside the tree the parser makes
/// of it, then return. Renders the same side-by-side comparison `xtask
/// audit emit` produces for a live tool, sourced from the frozen capture,
/// so an `[xfail]` fixture's asserted defect can be seen directly.
///
/// Read-only and separate from the checking run: neither blesses nor fails.
/// One fixture replayed through the real pipeline: the raw help text the
/// tiers built from, and the tree they produced.
pub struct ReplayedFixture {
    /// The fixture's tool name (`meta.tool.name`), which is what an audit
    /// entry is keyed on.
    pub tool: String,
    /// The raw help text, chosen by the same expansion/`--help`/`-h` rule
    /// the live oracles use ([`crate::misattribution::root_help_text_from`])
    /// so a fixture-sourced detector run and a sweep-sourced one are reading
    /// the same bytes for the same tool.
    pub raw: String,
    /// The extracted root, or `None` when no tier produced one.
    pub root: Option<CommandNode>,
}

/// Match a fixture-version pattern against one directory name. `*` means
/// "any sequence, including empty"; every other byte matches literally, so
/// a pattern with no `*` is an exact match (`audit-seed2` unchanged). `*`
/// also names "a tool's own version directory" for a seed-7 fixture, whose
/// directory is the tool's own version, not a shared `audit-seedN` name;
/// see [`replay_version_for_tools`] for how ambiguity is refused.
fn version_pattern_matches(pattern: &str, name: &str) -> bool {
    fn go(p: &[u8], t: &[u8]) -> bool {
        match p.first() {
            None => t.is_empty(),
            Some(b'*') => go(&p[1..], t) || (!t.is_empty() && go(p, &t[1..])),
            Some(c) => t.first() == Some(c) && go(&p[1..], &t[1..]),
        }
    }
    go(pattern.as_bytes(), name.as_bytes())
}

/// Replay every fixture whose version directory matches `pattern` (a glob,
/// see [`version_pattern_matches`]), considering only tools named in
/// `tools` (every tool under `corpus_root` when `None`). Zero subprocesses,
/// exactly like [`run`]; a fixture with no usable help capture is skipped
/// rather than yielded with an empty `raw`.
///
/// Ambiguity is refused by name: if `pattern` matches more than one version
/// directory under one tool, this errors naming the tool and every
/// directory matched, never picking the last one silently. `tools` narrows
/// which directories are considered, so a naming collision on some other
/// tool (`curl/8.5.0` and `curl/8.5.0-all` both legitimately exist) never
/// blocks a calibration run that never named that tool.
pub fn replay_version_for_tools(
    corpus_root: &Path,
    pattern: &str,
    tools: Option<&BTreeSet<String>>,
) -> anyhow::Result<Vec<ReplayedFixture>> {
    let fixtures = discover_fixtures(corpus_root)?;
    let mut by_tool: BTreeMap<&str, Vec<&Fixture>> = BTreeMap::new();
    for fixture in &fixtures {
        if tools.is_some_and(|tools| !tools.contains(fixture.tool_name())) {
            continue;
        }
        let version = fixture
            .label
            .rsplit_once('/')
            .map_or(fixture.label.as_str(), |(_, v)| v);
        if version_pattern_matches(pattern, version) {
            by_tool
                .entry(fixture.tool_name())
                .or_default()
                .push(fixture);
        }
    }

    let ambiguous: Vec<String> = by_tool
        .iter()
        .filter(|(_, matches)| matches.len() > 1)
        .map(|(tool, matches)| {
            format!(
                "{tool:?} ({})",
                matches
                    .iter()
                    .map(|f| f.label.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
        .collect();
    if !ambiguous.is_empty() {
        anyhow::bail!(
            "fixture-version pattern {pattern:?} matches more than one directory for {} \
             tool(s): {}. Narrow the pattern.",
            ambiguous.len(),
            ambiguous.join("; ")
        );
    }

    let mut out = Vec::new();
    for matches in by_tool.into_values() {
        let fixture = matches[0];
        let transcript = fixture.build_transcript()?;
        let mut recordings = HashMap::new();
        for capture in &fixture.meta.captures {
            let key = capture.argv[1..].to_vec();
            recordings.insert(
                key,
                ExecOutput {
                    stdout: read_capture_file(&fixture.dir, &capture.stdout)?,
                    stderr: match &capture.stderr {
                        Some(name) => read_capture_file(&fixture.dir, name)?,
                        None => Vec::new(),
                    },
                    exit_code: Some(capture.exit_code.unwrap_or(0)),
                    timed_out: false,
                },
            );
        }
        let Some(raw) = crate::misattribution::root_help_text_from(&recordings) else {
            continue;
        };
        let runner = Runner::new(default_tiers_with_probe(Arc::new(transcript)));
        let root = extract_tree(&runner, &fixture.resolved_tool());
        out.push(ReplayedFixture {
            tool: fixture.meta.tool.name.clone(),
            raw,
            root,
        });
    }
    Ok(out)
}

pub fn show_fixture(corpus_root: &Path, pattern: &str) -> anyhow::Result<()> {
    let fixtures = discover_fixtures(corpus_root)?;
    let matches: Vec<&Fixture> = fixtures
        .iter()
        .filter(|f| f.label.contains(pattern))
        .collect();

    let fixture = match matches.as_slice() {
        [] => anyhow::bail!(
            "no fixture matching {pattern:?} under {}. Available: {}",
            corpus_root.display(),
            fixtures
                .iter()
                .map(|f| f.label.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        [one] => *one,
        many => anyhow::bail!(
            "{pattern:?} matches {} fixtures: {}. Narrow it.",
            many.len(),
            many.iter()
                .map(|f| f.label.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    };

    println!("fixture: {}", fixture.label);
    println!("path:    {}", fixture.dir.display());
    if let Some(xfail) = &fixture.meta.xfail {
        println!(
            "status:  [xfail] {}",
            xfail.reason.as_deref().unwrap_or("(no reason recorded)")
        );
    } else {
        println!("status:  expected to pass");
    }
    if fixture.meta.contract.verdict_scope.is_empty() {
        println!(
            "scope:   unscoped — no dimension of this tree is asserted human-verified \
             (a passing snapshot check still freezes every field, descriptions included)"
        );
    } else {
        println!(
            "scope:   {} — only these dimensions were human-verified before this fixture was \
             blessed; the rest of the tree is frozen but unreviewed",
            verdict_scope_label(&fixture.meta.contract.verdict_scope)
        );
    }
    println!(
        "provenance: {} — who blessed this fixture's expected.snap (see corpus/README.md)",
        provenance_label(fixture.meta.bless.provenance)
    );
    println!();

    for capture in &fixture.meta.captures {
        let argv = capture.argv.join(" ");
        let files: [(&str, Option<&str>); 2] = [
            ("stdout", Some(capture.stdout.as_str())),
            ("stderr", capture.stderr.as_deref()),
        ];
        for (label, file) in files {
            let Some(name) = file else { continue };
            let bytes = std::fs::read(fixture.dir.join(name))?;
            if bytes.is_empty() {
                continue;
            }
            println!("=== captured: {argv}  ({label}) ===");
            println!("{}", String::from_utf8_lossy(&bytes));
        }
    }

    let transcript = fixture.build_transcript()?;
    let runner = Runner::new(default_tiers_with_probe(Arc::new(transcript)));
    let resolved = fixture.resolved_tool();
    println!("=== parsed tree ===");
    match extract_tree(&runner, &resolved) {
        Some(node) => println!("{}", render_snapshot(&node)?),
        None => println!("(no tier produced a root node)"),
    }
    Ok(())
}

#[cfg(test)]
mod pattern_tests {
    use super::*;

    #[test]
    fn exact_pattern_matches_only_itself() {
        assert!(version_pattern_matches("audit-seed2", "audit-seed2"));
        assert!(!version_pattern_matches("audit-seed2", "audit-seed20"));
        assert!(!version_pattern_matches("audit-seed2", "2.03.16"));
    }

    #[test]
    fn star_matches_any_name() {
        assert!(version_pattern_matches("*", "2.03.16"));
        assert!(version_pattern_matches("*", "audit-seed2"));
        assert!(version_pattern_matches("*", ""));
    }

    #[test]
    fn prefix_glob_matches_a_family_of_names() {
        assert!(version_pattern_matches("audit-seed*", "audit-seed2"));
        assert!(version_pattern_matches("audit-seed*", "audit-seed7"));
        assert!(!version_pattern_matches("audit-seed*", "2.03.16"));
    }
}
