//! The `audit/<seed>.toml` manifest schema: one sampled tool's entry, its
//! pre-tag suggestions, and its verdict — plus the load/save/parse helpers
//! that let every reader and writer of the file agree on what's in it.
//!
//! **Why this lives in `mandible-core` rather than `xtask`,** which used to
//! own the whole format: `xtask` is a binary crate, and `mandible` (the
//! `--review` TUI) cannot depend on another binary, so a shared library is
//! the only way for both to read and write byte-for-byte the same file
//! rather than maintaining two serde structs that silently drift apart.
//! This project has already been bitten by exactly that —
//! `mandible/src/pipeline.rs`'s old `LoadedTool` was a field-for-field copy
//! of `mandible_extract::ExtractionResult`, and the two drifted into
//! computing different metrics (see `AGENTS.md`).
//!
//! **What stays in `xtask/src/audit.rs`, deliberately not moved here:**
//! drawing the stratified sample and *computing* the K1/K2/K3 pre-tag
//! suggestions. That needs `xtask`-only detectors (`status`, `existence`,
//! `misattribution`) and a live extraction pass, and runs exactly once, at
//! `xtask audit sample` time. `mandible --review` never recomputes a
//! suggestion — it only ever reads the one already sitting in the file that
//! `xtask audit sample` wrote, displays it, and lets the reviewer confirm or
//! override it with the same `k1=`/`k2=`/`k3=` token syntax [`xtask audit
//! review`] uses.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One entry in a verdict file: a sampled tool, its drawn stratum, and —
/// once reviewed — a verdict plus an optional note. `verdict: None` is the
/// "pending" state; every command that touches the file treats absence of a
/// verdict as "not yet reviewed", never as an implicit skip.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    /// The tool name as found on `PATH` (or supplied via `--tools`).
    pub tool: String,
    /// The parse-status label this tool had when it was drawn — recorded at
    /// draw time, not recomputed later, so a tool whose parse changes
    /// between `sample` and `review` (a grammar fix landing mid-session)
    /// still reports against the stratum it was actually drawn from.
    pub stratum: String,
    /// `"correct"` / `"incomplete"` / `"wrong"` / `"skip"`, or absent while
    /// pending. Stored as a plain string (not an enum) so a hand-edited
    /// verdict file with an unrecognized word fails loudly at the point of
    /// use ([`parse_verdict_word`]) rather than silently at deserialization.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<String>,
    /// The reviewer's free-text note. Becomes an `[xfail]` `reason` for a
    /// `wrong`/`incomplete` fixture (`xtask::audit::cmd_fixtures`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// **K1 pre-tag**: the GCC-family single-dash-long-option parser defect
    /// (`short.is_some() && long.is_none() && value_name.is_some()`).
    /// Computed once, at sample time, by `xtask::audit::k1_signature`;
    /// displayed and overridden here (`k1=true`/`k1=false` anywhere in a
    /// verdict line or note, via [`extract_tag_override`]) exactly the same
    /// way regardless of whether the reviewing tool is `xtask audit review`
    /// or `mandible --review`. `Some(true)` when the tool's tree contains at
    /// least one matching flag, `None` when it contains none — never
    /// `Some(false)`, since there is no "confirmed not K1" state worth
    /// asserting for a tool that never exhibited the shape at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub k1: Option<bool>,
    /// **K2 pre-tag**: the existence detector's own tokenizer gap
    /// (`xtask::existence`'s `line_start_words` only considers each line's
    /// *first* token, so a multi-column or comma-separated
    /// applet/subcommand list reports every column after the first as
    /// "fabricated" even though it's right there in the raw text).
    /// Computed once, at sample time, by `xtask::audit::k2_signature`.
    /// `Some(true)` when every subcommand-kind existence fabrication for
    /// this tool is explained by the known tokenizer gap, `Some(false)`
    /// when at least one is not (worth a real look), `None` when the tool
    /// has no subcommand-kind fabrications to judge at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub k2: Option<bool>,
    /// **K3 pre-tag**: "subcommand help was never fetched, so this node is
    /// a bare stub." Two distinct causes produce it, both computed once at
    /// sample time by `xtask::audit::k3_signature` from the same
    /// single-pass snapshot K1/K2 use, and both should tag:
    ///
    /// - the attestation gate refused to probe a subcommand because its
    ///   name came from a native/cobra artifact rather than a recognized
    ///   `--help` heading (`git-lfs`: 36 nodes, 34 suspects, status
    ///   `suspicious`, every subcommand a cobra stub — and, unlike an
    ///   ordinary un-recursed node that just hasn't been fetched *yet*,
    ///   this shape is structurally permanent: the gate refuses it live,
    ///   in the TUI, exactly as it does here);
    /// - the tool's subcommands simply carry no flags because their own
    ///   help was never fetched (`openssl`: 151 subcommands, zero flags
    ///   anywhere in the extracted tree, root included).
    ///
    /// Without this, a reviewer re-derives the same "still empty, still not
    /// this tool's fault" verdict once per subcommand. `Some(true)` when the
    /// tool's snapshot shows at least one of the two shapes, `None`
    /// otherwise — the same "no `Some(false)`" convention as K1, since
    /// there is nothing to assert-not for a tool that shows neither shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub k3: Option<bool>,
    /// `Some(reason)` when this entry was force-included in the sample
    /// outside the normal stratified draw (see `xtask::audit::cmd_sample`'s
    /// `force_include` parameter). `None` for an entry drawn by the
    /// ordinary stratified sample.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_reason: Option<String>,
}

impl Entry {
    /// True when this entry's note is obligatory but missing or blank — a
    /// `wrong`/`incomplete` verdict with nothing recorded about *what* was
    /// wrong. See [`verdict_requires_note`].
    pub fn missing_required_note(&self) -> bool {
        self.verdict.as_deref().is_some_and(verdict_requires_note) && self.note.trim().is_empty()
    }

    /// True when a review session should still stop at this entry: no
    /// verdict yet, or a verdict whose obligatory note never got written.
    pub fn needs_attention(&self) -> bool {
        self.verdict.is_none() || self.missing_required_note()
    }
}

/// The persisted state of one audit run: everything needed to resume, and
/// nothing that would make two runs of `sample` with the same `--seed`
/// disagree with each other.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditFile {
    /// The seed and sample size that produced this file.
    pub meta: AuditMeta,
    /// Every sampled tool, reviewed or not, in file order.
    #[serde(default, rename = "entry")]
    pub entries: Vec<Entry>,
}

/// [`AuditFile`]'s own metadata: the seed and requested sample size that
/// produced it, re-asserted whenever the sample is (re)drawn so a stale
/// `--sample`/`--seed` combination against an existing file is a loud
/// error, never a silent merge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditMeta {
    /// The seed the stratified draw used.
    pub seed: u64,
    /// The total sample size requested at draw time.
    pub sample_size: usize,
}

impl AuditFile {
    /// Indices of entries with no verdict yet, in file order — the ordered
    /// walk both `xtask audit review` and `mandible --review` follow, so an
    /// interrupted session resumes at the same entry regardless of which of
    /// the two last touched the file.
    pub fn pending(&self) -> impl Iterator<Item = usize> + '_ {
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.verdict.is_none())
            .map(|(i, _)| i)
    }

    /// Indices of entries a review session should still stop at, in file
    /// order: everything [`Self::pending`] yields, plus anything already
    /// judged `wrong`/`incomplete` whose note is missing or blank
    /// ([`verdict_requires_note`]).
    ///
    /// The second half exists because such an entry is a *record* that is
    /// incomplete even though a verdict was given. For accuracy arithmetic
    /// it counts as judged and always did — the tool really was judged
    /// wrong — but for the triage the audit exists to feed it is useless: it
    /// names a tool and says nothing about what was wrong with it. Rather
    /// than a separate repair command, the ordinary walk simply stops there
    /// again, so a session that recorded bare verdicts before this rule
    /// existed heals itself on the next run.
    pub fn needing_attention(&self) -> impl Iterator<Item = usize> + '_ {
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.needs_attention())
            .map(|(i, _)| i)
    }
}

/// Whether a verdict word obliges the reviewer to write a note.
///
/// `wrong` and `incomplete` do: for those two the note *is* the finding, and
/// the whole point of the audit is to hand a later fix something actionable.
/// `correct` and `skip` do not — "it parsed correctly" is complete on its
/// own, and forcing prose out of a reviewer who has nothing to add is how a
/// review loop starts collecting "n/a".
pub fn verdict_requires_note(verdict: &str) -> bool {
    matches!(verdict, "wrong" | "incomplete")
}

/// The path a given `(dir, seed)` pair resolves to: `<dir>/<seed>.toml`.
pub fn verdict_path(dir: &Path, seed: u64) -> PathBuf {
    dir.join(format!("{seed}.toml"))
}

/// Read and parse `path` as an [`AuditFile`].
pub fn load(path: &Path) -> anyhow::Result<AuditFile> {
    let raw = std::fs::read_to_string(path).map_err(|e| {
        anyhow::anyhow!(
            "reading {}: {e} (run `xtask audit sample` first)",
            path.display()
        )
    })?;
    toml::from_str(&raw).map_err(|e| anyhow::anyhow!("parsing {}: {e}", path.display()))
}

/// Serialize and write `file` to `path`, creating its parent directory if
/// needed. Called after **every** verdict by both `xtask audit review` and
/// `mandible --review` — never batched — so a killed process leaves
/// everything answered so far recorded and everything else still pending.
pub fn save(path: &Path, file: &AuditFile) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| anyhow::anyhow!("creating {}: {e}", parent.display()))?;
        }
    }
    let text = toml::to_string_pretty(file)
        .map_err(|e| anyhow::anyhow!("serializing {}: {e}", path.display()))?;
    std::fs::write(path, text).map_err(|e| anyhow::anyhow!("writing {}: {e}", path.display()))
}

/// Parse a verdict word (`c`/`correct`, `i`/`incomplete`, `w`/`wrong`,
/// `s`/`skip`) to its canonical spelling. Shared by every entry point that
/// accepts a verdict — typed live in `xtask audit review`, read from a
/// verdicts file by `xtask audit ingest`, or chosen by a keypress in
/// `mandible --review` — so none of them can disagree about what counts as
/// a valid verdict.
pub fn parse_verdict_word(word: &str) -> anyhow::Result<&'static str> {
    match word {
        "c" | "correct" => Ok("correct"),
        "i" | "incomplete" => Ok("incomplete"),
        "w" | "wrong" => Ok("wrong"),
        "s" | "skip" => Ok("skip"),
        other => anyhow::bail!(
            "unrecognized verdict {other:?} — expected one of: c/correct, i/incomplete, w/wrong, s/skip"
        ),
    }
}

/// Pull any `k1=true`/`k1=false`/`k2=true`/`k2=false`/`k3=true`/`k3=false`
/// (case-insensitive) token for `key` out of `text`, in place, returning the
/// override it specified (if any). The token is removed from `text`
/// regardless of position — a reviewer's note is free-form prose, not a
/// fixed field order — so what remains is the plain note with no tag syntax
/// left in it. Shared by every entry point that accepts a note, for the
/// same reason [`parse_verdict_word`] is.
pub fn extract_tag_override(text: &mut String, key: &str) -> Option<bool> {
    let true_tok = format!("{key}=true");
    let false_tok = format!("{key}=false");
    let mut found = None;
    let kept: Vec<&str> = text
        .split_whitespace()
        .filter(|tok| {
            if tok.eq_ignore_ascii_case(&true_tok) {
                found = Some(true);
                false
            } else if tok.eq_ignore_ascii_case(&false_tok) {
                found = Some(false);
                false
            } else {
                true
            }
        })
        .collect();
    *text = kept.join(" ");
    found
}

/// Human-readable line for a pre-tag, shown to the reviewer before they
/// record a verdict — the whole point of [`Entry::k1`]/[`Entry::k2`]/
/// [`Entry::k3`] is that this line lets a reviewer confirm-or-override in
/// one glance instead of re-deriving the same known defect per flag.
pub fn tag_display(label: &str, tag: Option<bool>, override_syntax: &str) -> String {
    match tag {
        Some(true) => format!(
            "{label}: suggested TRUE — leave as-is to confirm, or add `{override_syntax}=false` \
             to your verdict to override"
        ),
        Some(false) => format!(
            "{label}: suggested FALSE (fabrications present but not fully explained by the \
             known class — worth a real look) — add `{override_syntax}=true` to override"
        ),
        None => format!("{label}: not flagged (nothing of this class detected)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(tool: &str, verdict: Option<&str>, note: &str) -> Entry {
        Entry {
            tool: tool.to_string(),
            stratum: "ok".to_string(),
            verdict: verdict.map(str::to_string),
            note: note.to_string(),
            k1: None,
            k2: None,
            k3: None,
            include_reason: None,
        }
    }

    /// `wrong`/`incomplete` oblige a note; `correct`/`skip` do not. Forcing
    /// prose out of a reviewer with nothing to add is how a review loop
    /// starts collecting "n/a".
    #[test]
    fn only_wrong_and_incomplete_require_a_note() {
        assert!(verdict_requires_note("wrong"));
        assert!(verdict_requires_note("incomplete"));
        assert!(!verdict_requires_note("correct"));
        assert!(!verdict_requires_note("skip"));
    }

    #[test]
    fn a_blank_or_whitespace_note_does_not_satisfy_the_obligation() {
        assert!(entry("a", Some("wrong"), "").missing_required_note());
        assert!(entry("a", Some("wrong"), "   ").missing_required_note());
        assert!(!entry("a", Some("wrong"), "descriptions off by one").missing_required_note());
        assert!(!entry("a", Some("correct"), "").missing_required_note());
        assert!(!entry("a", None, "").missing_required_note());
    }

    /// The self-healing property: three `wrong` verdicts were recorded with
    /// no note before this rule existed, and the ordinary review walk must
    /// stop at them again rather than needing a separate repair command.
    #[test]
    fn the_walk_revisits_a_verdict_whose_required_note_is_missing() {
        let file = AuditFile {
            meta: AuditMeta {
                seed: 2,
                sample_size: 4,
            },
            entries: vec![
                entry("noted", Some("wrong"), "real finding"),
                entry("bare", Some("wrong"), ""),
                entry("fine", Some("correct"), ""),
                entry("fresh", None, ""),
            ],
        };
        // `pending` keeps its old meaning, so accuracy arithmetic that
        // counts a bare `wrong` as judged is unaffected.
        assert_eq!(file.pending().collect::<Vec<_>>(), vec![3]);
        // The review walk stops at the bare verdict too.
        assert_eq!(file.needing_attention().collect::<Vec<_>>(), vec![1, 3]);
    }

    #[test]
    fn verdict_path_joins_seed_as_a_toml_filename() {
        assert_eq!(
            verdict_path(Path::new("audit"), 42),
            Path::new("audit/42.toml")
        );
    }

    #[test]
    fn save_then_load_round_trips_every_field() {
        let tmp = tempfile::tempdir().unwrap();
        let path = verdict_path(tmp.path(), 7);
        let file = AuditFile {
            meta: AuditMeta {
                seed: 7,
                sample_size: 2,
            },
            entries: vec![
                Entry {
                    tool: "openssl".to_string(),
                    stratum: "suspicious".to_string(),
                    verdict: Some("incomplete".to_string()),
                    note: "subcommand help never fetched".to_string(),
                    k1: None,
                    k2: Some(false),
                    k3: Some(true),
                    include_reason: None,
                },
                Entry {
                    tool: "zoxide".to_string(),
                    stratum: "ok".to_string(),
                    verdict: None,
                    note: String::new(),
                    k1: None,
                    k2: None,
                    k3: None,
                    include_reason: Some("unaudited promotion".to_string()),
                },
            ],
        };
        save(&path, &file).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.meta.seed, 7);
        assert_eq!(loaded.meta.sample_size, 2);
        assert_eq!(loaded.entries.len(), 2);
        assert_eq!(loaded.entries[0].tool, "openssl");
        assert_eq!(loaded.entries[0].verdict.as_deref(), Some("incomplete"));
        assert_eq!(loaded.entries[0].k3, Some(true));
        assert_eq!(
            loaded.entries[1].include_reason.as_deref(),
            Some("unaudited promotion")
        );
        assert_eq!(loaded.pending().collect::<Vec<_>>(), vec![1]);
    }

    #[test]
    fn load_of_a_missing_file_names_the_sample_command() {
        let tmp = tempfile::tempdir().unwrap();
        let path = verdict_path(tmp.path(), 1);
        let err = load(&path).unwrap_err();
        assert!(err.to_string().contains("xtask audit sample"));
    }

    #[test]
    fn parse_verdict_word_accepts_short_and_long_forms() {
        assert_eq!(parse_verdict_word("c").unwrap(), "correct");
        assert_eq!(parse_verdict_word("correct").unwrap(), "correct");
        assert_eq!(parse_verdict_word("i").unwrap(), "incomplete");
        assert_eq!(parse_verdict_word("incomplete").unwrap(), "incomplete");
        assert_eq!(parse_verdict_word("w").unwrap(), "wrong");
        assert_eq!(parse_verdict_word("wrong").unwrap(), "wrong");
        assert_eq!(parse_verdict_word("s").unwrap(), "skip");
        assert_eq!(parse_verdict_word("skip").unwrap(), "skip");
        assert!(parse_verdict_word("maybe").is_err());
    }

    #[test]
    fn extract_tag_override_pulls_the_token_out_of_the_note() {
        let mut note =
            "the extra flags were genuinely wrong k1=false not the gcc defect".to_string();
        let k1 = extract_tag_override(&mut note, "k1");
        assert_eq!(k1, Some(false));
        assert_eq!(
            note, "the extra flags were genuinely wrong not the gcc defect",
            "the token is removed, the rest of the note survives untouched"
        );
    }

    #[test]
    fn extract_tag_override_is_case_insensitive_and_absent_returns_none() {
        let mut note = "K1=TRUE looks like the known defect".to_string();
        assert_eq!(extract_tag_override(&mut note, "k1"), Some(true));
        assert_eq!(extract_tag_override(&mut note, "k2"), None);
    }

    #[test]
    fn extract_tag_override_handles_three_keys_in_one_note() {
        let mut note = "k1=true k2=false k3=true mixed causes".to_string();
        assert_eq!(extract_tag_override(&mut note, "k1"), Some(true));
        assert_eq!(extract_tag_override(&mut note, "k2"), Some(false));
        assert_eq!(extract_tag_override(&mut note, "k3"), Some(true));
        assert_eq!(note, "mixed causes");
    }

    #[test]
    fn tag_display_names_every_state() {
        assert!(tag_display("K3", Some(true), "k3").contains("suggested TRUE"));
        assert!(tag_display("K3", Some(false), "k3").contains("suggested FALSE"));
        assert!(tag_display("K3", None, "k3").contains("not flagged"));
    }
}
