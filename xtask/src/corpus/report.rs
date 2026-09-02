//! The public `CorpusReport`/`ReplayedFixture` types and the `replay_version`/`show_fixture` entry points.
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

/// Replay every fixture whose directory name is `version` (e.g.
/// `audit-seed2`) and hand back what each one parsed to.
///
/// Zero subprocesses, exactly like [`run`]: this is the same frozen-bytes
/// replay the corpus suite performs, exposed so `crate::detector` can run a
/// detector over the audited tools without a `PATH` sweep. Fixtures that
/// carry no usable help capture are skipped rather than yielded with an
/// empty `raw`, so a caller cannot mistake "nothing was captured" for "the
/// tool's help text is empty".
pub fn replay_version(corpus_root: &Path, version: &str) -> anyhow::Result<Vec<ReplayedFixture>> {
    let mut out = Vec::new();
    for fixture in discover_fixtures(corpus_root)? {
        if !fixture.label.ends_with(&format!("/{version}")) {
            continue;
        }
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
