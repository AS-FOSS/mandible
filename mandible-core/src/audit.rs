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
    /// (`xtask::existence`'s `line_start_words` only considered each line's
    /// *first* token, so a multi-column or comma-separated
    /// applet/subcommand list reported every column after the first as
    /// "fabricated" even though it's right there in the raw text).
    ///
    /// **The gap itself is closed** — `existence::list_row_words` now reads
    /// a whole list row, and the 359 fleet-wide fabrications this tag
    /// existed to explain away are gone (spec §K2). Existing entries keep
    /// their recorded tag, since a verdict is a record of what the reviewer
    /// was shown; freshly sampled tools of the same shape simply produce no
    /// fabrication to tag. Retained so an old manifest still round-trips,
    /// and so a regression in the list-row rule shows up as this tag coming
    /// back rather than as silent noise.
    ///
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
    /// **Defect-family labels** for a `wrong`/`incomplete` verdict: which
    /// *shapes* of defect this tool exhibits, drawn from the closed set in
    /// [`DEFECT_FAMILIES`]. Empty for a `correct`/`skip` entry (there is no
    /// defect to name), and — importantly — also empty for a
    /// `wrong`/`incomplete` entry nobody could confidently classify. That
    /// second case is [`Entry::is_unclassified`], and it is deliberately
    /// representable: an honest "we do not know which family this is" is
    /// worth far more than a fabricated label, because the whole purpose of
    /// these labels is to calibrate a detector against them.
    ///
    /// Stored as plain strings rather than an enum for the same reason
    /// [`Self::verdict`] is: a hand-edited manifest with an unrecognized
    /// family fails loudly at the point of use
    /// ([`Entry::validate_families`]) rather than silently at
    /// deserialization, where the error would name a line number and not a
    /// tool.
    ///
    /// **A family is a shape, never a tool.** spec §1's no-per-tool-logic
    /// rule applies here exactly as it does to a parser: `tcpdump` is not a
    /// family, `bundled-short-flag` is, and the tool name is data.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub families: Vec<String>,
    /// Provenance of [`Self::families`], and the reason that field is safe
    /// to have in a tracked manifest at all.
    ///
    /// A verdict is a **human judgment**: a reviewer read the tool's real
    /// output. A family label derived by a machine *reading that reviewer's
    /// prose* is a strictly weaker claim, and this project's posture (spec
    /// §13.1b's fifth rule: a name a reader could mistake for a stronger
    /// claim is itself a defect) is that a weaker claim must be labelled as
    /// one rather than left to be inferred.
    ///
    /// - `Some(true)` — derived by machine from the reviewer's note plus the
    ///   fixture evidence. **Not** a reviewer's own classification.
    /// - `Some(false)` — the reviewer classified it themselves.
    /// - `None` — no provenance recorded, which
    ///   [`Entry::validate_families`] rejects whenever `families` is
    ///   non-empty. Absence must never silently read as "a human said so":
    ///   a writer that forgets this field would otherwise launder a machine
    ///   reading into a human judgment, which is the single worst outcome
    ///   this schema can produce.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub families_derived: Option<bool>,
    /// A history of corrections applied to this entry's original verdict,
    /// oldest first — **appended to, never used to overwrite [`Self::verdict`]
    /// or [`Self::note`]**. Empty for the overwhelming majority of entries,
    /// which is exactly why this is a `Vec` that serializes to nothing when
    /// empty rather than a field every existing manifest would need
    /// migrating to carry: an `audit/<seed>.toml` written before this field
    /// existed deserializes with `amendments: vec![]`, identical in every
    /// observable way to a freshly reviewed entry that has never been
    /// amended. See [`Self::effective_verdict`]/[`Self::effective_note`] for
    /// what a caller should actually read, and [`amend`] for how an entry
    /// gets one of these appended.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub amendments: Vec<Amendment>,
}

impl Entry {
    /// The verdict every aggregate computation (accuracy tallies, the
    /// wrong/incomplete listing, fixture generation) should read: the
    /// `new_verdict` of the most recent [`Amendment`] if this entry has any,
    /// else the original [`Self::verdict`] untouched. A verdict amendment
    /// changes what the project believes about a tool without destroying
    /// the record of what a reviewer originally wrote — see [`amend`]'s doc
    /// comment for the full rationale.
    pub fn effective_verdict(&self) -> Option<&str> {
        match self.amendments.last() {
            Some(a) => Some(a.new_verdict.as_str()),
            None => self.verdict.as_deref(),
        }
    }

    /// The note that belongs to [`Self::effective_verdict`]: the most
    /// recent amendment's `new_note` if this entry has been amended, else
    /// the original [`Self::note`]. Never a concatenation of both — an
    /// amendment's `new_note` is a complete, self-contained note for the
    /// corrected verdict (enforced by [`amend`]), not a delta on top of the
    /// original.
    pub fn effective_note(&self) -> &str {
        match self.amendments.last() {
            Some(a) => a.new_note.as_str(),
            None => self.note.as_str(),
        }
    }

    /// True when this entry's note is obligatory but missing or blank — a
    /// `wrong`/`incomplete` verdict with nothing recorded about *what* was
    /// wrong. Reads the *effective* verdict/note, so an amendment that
    /// corrects a bare-note defect heals this the same way a plain
    /// re-review would. See [`verdict_requires_note`].
    pub fn missing_required_note(&self) -> bool {
        self.effective_verdict().is_some_and(verdict_requires_note)
            && self.effective_note().trim().is_empty()
    }

    /// True when a review session should still stop at this entry: no
    /// verdict yet, or a verdict whose obligatory note never got written.
    pub fn needs_attention(&self) -> bool {
        self.verdict.is_none() || self.missing_required_note()
    }

    /// True when this entry is a judged defect — `wrong` or `incomplete`
    /// under [`Self::effective_verdict`]. The population a family label is
    /// *about*, and the population a detector is expected to fire on once
    /// the label says it belongs to that detector's family.
    pub fn is_judged_defect(&self) -> bool {
        self.effective_verdict()
            .is_some_and(|v| matches!(v, "wrong" | "incomplete"))
    }

    /// True when this entry is a judged non-defect — `correct` under
    /// [`Self::effective_verdict`]. The population a detector must stay
    /// **silent** on: a fire here is a false alarm against a human who read
    /// the tool's real output and said the parse was right.
    ///
    /// `skip` is neither this nor [`Self::is_judged_defect`]. A skipped
    /// entry carries no judgment about the parse at all (spec §13.1c
    /// excludes it from the accuracy ratio for the same reason), so it can
    /// neither confirm nor refute a detector and is excluded from
    /// calibration entirely rather than silently counted as "good".
    pub fn is_judged_correct(&self) -> bool {
        self.effective_verdict() == Some("correct")
    }

    /// True when this entry is a judged defect that carries no family label
    /// — the honest "nobody could tell which family this is" state. Counted
    /// and printed rather than hidden, because an unclassified entry is a
    /// known hole in a detector's calibration set, and a hole you can see is
    /// not the same kind of problem as a hole papered over with a guess.
    pub fn is_unclassified(&self) -> bool {
        self.is_judged_defect() && self.families.is_empty()
    }

    /// True when this entry carries `family` among its labels.
    pub fn has_family(&self, family: &str) -> bool {
        self.families.iter().any(|f| f == family)
    }

    /// Check this entry's [`Self::families`]/[`Self::families_derived`] pair
    /// for every way it could be a claim nobody can evaluate later:
    ///
    /// - a family word outside the closed [`DEFECT_FAMILIES`] set (a typo,
    ///   or an ad-hoc family invented in a hand edit and therefore invisible
    ///   to every reader that matches on the set);
    /// - the same family listed twice (harmless to a matcher, but it makes a
    ///   per-family count wrong, and counts are what calibration reports);
    /// - labels with no recorded provenance — see
    ///   [`Self::families_derived`] for why silence there is unacceptable;
    /// - labels on a verdict that names no defect (`correct`/`skip`), which
    ///   would put a tool into a detector's expected-fires set on the
    ///   strength of a verdict that says nothing is wrong with it.
    pub fn validate_families(&self) -> anyhow::Result<()> {
        for (i, family) in self.families.iter().enumerate() {
            if family_meaning(family).is_none() {
                anyhow::bail!(
                    "{:?}: unrecognized defect family {family:?} — expected one of: {}",
                    self.tool,
                    family_names().join(", ")
                );
            }
            if self.families[..i].contains(family) {
                anyhow::bail!("{:?}: defect family {family:?} listed twice", self.tool);
            }
        }
        if !self.families.is_empty() {
            if self.families_derived.is_none() {
                anyhow::bail!(
                    "{:?} carries family labels with no `families_derived` provenance — a machine \
                     reading of a reviewer's note must never be mistakable for the reviewer's own \
                     classification",
                    self.tool
                );
            }
            if !self.is_judged_defect() {
                anyhow::bail!(
                    "{:?} is {:?}, which names no defect, yet carries family labels {:?} — a \
                     family describes what is wrong, so labelling a non-defect would put this \
                     tool in a detector's expected-fires set on a verdict that says nothing is \
                     wrong with it",
                    self.tool,
                    self.effective_verdict().unwrap_or("pending"),
                    self.families,
                );
            }
        }
        Ok(())
    }
}

/// One shape of defect, as a machine-readable label plus what it means.
///
/// **Derived from the seed-2 audit's own notes, not asserted in advance.**
/// Every family here is backed by at least one reviewer note that describes
/// that shape; nothing was added on the strength of "a parser could
/// plausibly do this", because a family with no labelled member calibrates
/// nothing and would only make the set look more complete than the evidence
/// supports.
pub struct DefectFamily {
    /// The label as it appears in [`Entry::families`]. Kebab-case, names a
    /// shape.
    pub name: &'static str,
    /// One line describing the shape — what a detector for this family
    /// would have to recognize.
    pub meaning: &'static str,
}

/// The closed set of defect families (see [`DefectFamily`]).
///
/// Ordered roughly by how directly each one is a *parser* defect: the first
/// group is the grammar getting a flag's structure wrong, then recall gaps,
/// then help text whose shape the grammar has no model for at all, and last
/// the two families that are honest about **not** being extraction defects
/// (`display-only`, `no-usable-help`). Keeping those last two in the same
/// closed set is deliberate — a reviewer's `wrong` verdict on a tool with no
/// help text is a real recorded judgment, and dropping it from the labelled
/// set would quietly inflate every detector's apparent recall by removing
/// tools it was never going to fire on for a reason that has nothing to do
/// with the detector.
pub const DEFECT_FAMILIES: &[DefectFamily] = &[
    DefectFamily {
        name: "bundled-short-flag",
        meaning: "a bundle of boolean short flags (`[-abcXYZ]`) collapses into one flag `-a` \
                  carrying the rest as a value, instead of N separate flags",
    },
    DefectFamily {
        name: "single-dash-long",
        meaning: "a single-dash long option (`-help`, `-fdump-scos`) splits into a one-character \
                  short flag plus the remainder as a value name (the K1 pre-tag's shape)",
    },
    DefectFamily {
        name: "repeated-char-flag",
        meaning: "a repeated-character flag (`-vv`, `-dd`, `-kk`) documented in the help text is \
                  not extracted at all",
    },
    DefectFamily {
        name: "dropped-alias",
        meaning: "one half of a documented short/long alias pair is missing from the extracted \
                  flag (`-p` kept, `--pid` dropped, or the reverse)",
    },
    DefectFamily {
        name: "value-name-mangled",
        meaning: "a flag's value spec is mis-captured: an alternative form, an alias spelling, or \
                  a second accepted type is swallowed into or dropped from `value_name`",
    },
    DefectFamily {
        name: "missing-flag-description",
        meaning: "flags are extracted but carry no description text, though the help text \
                  attaches one",
    },
    DefectFamily {
        name: "section-header-bleed",
        meaning: "text belonging to a section heading is absorbed into a flag, a description, or \
                  a node name",
    },
    DefectFamily {
        name: "unparsed-subcommand",
        meaning: "subcommand names are plainly present in the help text but no child node is \
                  produced for them",
    },
    DefectFamily {
        name: "unparsed-positional",
        meaning: "a positional operand in the usage line (`<destination>`, `pid`) is never \
                  extracted — the IR has nowhere to put it",
    },
    DefectFamily {
        name: "unmodeled-help-shape",
        meaning: "the help text is structured in a way the grammar has no model for at all \
                  (topic-partitioned `--help=<topic>` pages, a combinatorial synopsis that \
                  reprints the tool name per variant, `KEY=VALUE` operands, a settings/variables \
                  table that is not a flag list, multi-column layouts)",
    },
    DefectFamily {
        name: "verbatim-fallback",
        meaning: "help text was captured but no structure came out of it, so the tool falls back \
                  to verbatim display",
    },
    DefectFamily {
        name: "display-only",
        meaning: "the extraction is right and the defect is in how the TUI renders it (width, \
                  wrapping, a truncated bracket) — recorded as not-an-extraction-defect rather \
                  than dropped, so it cannot be mistaken for one",
    },
    DefectFamily {
        name: "no-usable-help",
        meaning: "the tool yields no help text to parse under the allowlisted probe argv (prints \
                  nothing, errors, opens a REPL, or emits something that is not help) — a \
                  property of the tool, not of the parser",
    },
];

/// Every family name, in [`DEFECT_FAMILIES`] order.
pub fn family_names() -> Vec<&'static str> {
    DEFECT_FAMILIES.iter().map(|f| f.name).collect()
}

/// The one-line meaning of `name`, or `None` if it is not a recognized
/// family. The membership test every reader of [`Entry::families`] should
/// use, so an unrecognized word can never be silently treated as a family
/// nobody has heard of.
pub fn family_meaning(name: &str) -> Option<&'static str> {
    DEFECT_FAMILIES
        .iter()
        .find(|f| f.name == name)
        .map(|f| f.meaning)
}

/// Parse a family word to its canonical `'static` spelling, failing loudly
/// (and naming the whole valid set) on anything else — the family
/// counterpart of [`parse_verdict_word`], and shared for the same reason:
/// every entry point that accepts a family must agree on what one is.
pub fn parse_family(word: &str) -> anyhow::Result<&'static str> {
    DEFECT_FAMILIES
        .iter()
        .find(|f| f.name == word)
        .map(|f| f.name)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "unrecognized defect family {word:?} — expected one of: {}",
                family_names().join(", ")
            )
        })
}

/// One recorded correction to an [`Entry`]'s verdict — the audit's amendment
/// mechanism (see the module's own doc comment for why this exists: a
/// reviewer error, once identified, must be fixable without either silently
/// rewriting history or leaving a known-false record standing).
///
/// **Appended, never mutated once written**, and the `Entry` this lives on
/// never has [`Entry::verdict`]/[`Entry::note`] overwritten by [`amend`]
/// either — an amendment is additive by construction, so `git blame` and a
/// plain read of the TOML both show the original verdict sitting right there
/// next to the record of what it became and why, rather than requiring
/// reconstruction from a diff.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Amendment {
    /// The verdict this amendment supersedes: the entry's original
    /// [`Entry::verdict`] for a first amendment, or the previous
    /// amendment's `new_verdict` for a second — recorded explicitly (not
    /// left to be inferred by walking the list) so each amendment reads as
    /// a complete, self-contained "was X, became Y, because Z" statement on
    /// its own.
    pub previous_verdict: String,
    /// The corrected verdict, in effect from this amendment forward.
    pub new_verdict: String,
    /// The note attached to `new_verdict`, required under the same rule
    /// [`verdict_requires_note`] applies to an ordinary verdict — an
    /// amendment to `wrong`/`incomplete` with nothing recorded about what
    /// is actually wrong is exactly as useless as a bare initial verdict
    /// would be. Stored separately from [`Entry::note`] so the original
    /// note (which may itself be empty, e.g. a `correct` being amended
    /// away) survives untouched as history.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub new_note: String,
    /// Why the original verdict was wrong and is being corrected. Always
    /// required, regardless of what the new verdict is — an amendment with
    /// no stated reason is exactly the kind of unauditable rewrite this
    /// mechanism exists to prevent, the same precedent
    /// [`Entry::include_reason`] already sets for force-inclusion.
    pub reason: String,
}

/// Append an [`Amendment`] to `entry`, correcting its effective verdict
/// without overwriting anything already on disk. Fails loudly, before
/// touching `entry`, on every way an amendment could become an unauditable
/// or incomplete record:
///
/// - `entry` has no verdict yet (nothing to amend — record an initial
///   verdict first, this is not a shortcut around the ordinary review
///   flow);
/// - `reason` is blank (the whole point of this function over hand-editing
///   the TOML directly);
/// - `new_verdict` obliges a note ([`verdict_requires_note`]) and
///   `new_note` is blank, the same obligation an ordinary verdict carries;
/// - `new_verdict` is identical to the entry's current effective verdict
///   (nothing is actually changing — that is an edit to the note, a
///   different operation this function does not perform, not a verdict
///   amendment).
///
/// `new_verdict` must already be a canonical word (run it through
/// [`parse_verdict_word`] first, the same as every other entry point that
/// accepts one) — this function does not parse `c`/`i`/`w`/`s` shorthand
/// itself, so a caller's typo surfaces as a rejected value rather than a
/// silently accepted wrong one.
pub fn amend(
    entry: &mut Entry,
    new_verdict: &str,
    new_note: String,
    reason: String,
) -> anyhow::Result<()> {
    let Some(previous_verdict) = entry.effective_verdict().map(str::to_string) else {
        anyhow::bail!(
            "{:?} has no verdict yet — nothing to amend (record an initial verdict first, via \
             `xtask audit review`/`ingest` or `mandible --review`)",
            entry.tool
        );
    };
    if reason.trim().is_empty() {
        anyhow::bail!(
            "amending {:?} needs a reason — an amendment with nothing recorded about why is \
             exactly the unauditable change this mechanism exists to prevent",
            entry.tool
        );
    }
    if verdict_requires_note(new_verdict) && new_note.trim().is_empty() {
        anyhow::bail!(
            "amending {:?} to {new_verdict:?} needs a note — the same obligation an ordinary \
             wrong/incomplete verdict carries, now aimed at the corrected value",
            entry.tool
        );
    }
    if previous_verdict == new_verdict {
        anyhow::bail!(
            "{:?} is already {new_verdict:?} (after any prior amendments) — nothing to amend",
            entry.tool
        );
    }
    entry.amendments.push(Amendment {
        previous_verdict,
        new_verdict: new_verdict.to_string(),
        new_note,
        reason,
    });
    Ok(())
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

    /// Run [`Entry::validate_families`] over every entry, reporting the
    /// first failure. Called by every command that reads family labels
    /// before it computes anything from them, so a hand edit that
    /// mistypes a family or forgets its provenance fails at the top of the
    /// run rather than quietly changing a confusion matrix.
    pub fn validate_families(&self) -> anyhow::Result<()> {
        for entry in &self.entries {
            entry.validate_families()?;
        }
        Ok(())
    }

    /// Judged defects carrying no family label, in file order — the
    /// `unclassified` population every calibration report prints. See
    /// [`Entry::is_unclassified`].
    pub fn unclassified(&self) -> impl Iterator<Item = &Entry> + '_ {
        self.entries.iter().filter(|e| e.is_unclassified())
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
            families: Vec::new(),
            families_derived: None,
            amendments: Vec::new(),
        }
    }

    fn labelled(tool: &str, verdict: &str, families: &[&str]) -> Entry {
        Entry {
            families: families.iter().map(|f| f.to_string()).collect(),
            families_derived: Some(true),
            ..entry(tool, Some(verdict), "a real finding")
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
                    families: vec!["unparsed-subcommand".to_string()],
                    families_derived: Some(true),
                    amendments: vec![Amendment {
                        previous_verdict: "incomplete".to_string(),
                        new_verdict: "wrong".to_string(),
                        new_note: "actually a genuine parser defect, not just unfetched help"
                            .to_string(),
                        reason: "re-read after a related tool surfaced the same shape".to_string(),
                    }],
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
                    families: Vec::new(),
                    families_derived: None,
                    amendments: Vec::new(),
                },
            ],
        };
        save(&path, &file).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.meta.seed, 7);
        assert_eq!(loaded.meta.sample_size, 2);
        assert_eq!(loaded.entries.len(), 2);
        assert_eq!(loaded.entries[0].tool, "openssl");
        // The original verdict/note are untouched by the amendment: the
        // file still shows what the reviewer originally wrote.
        assert_eq!(loaded.entries[0].verdict.as_deref(), Some("incomplete"));
        assert_eq!(loaded.entries[0].note, "subcommand help never fetched");
        assert_eq!(loaded.entries[0].k3, Some(true));
        assert_eq!(loaded.entries[0].families, vec!["unparsed-subcommand"]);
        assert_eq!(loaded.entries[0].families_derived, Some(true));
        assert!(loaded.entries[1].families.is_empty());
        // ...while the amendment history carries the correction.
        assert_eq!(loaded.entries[0].amendments.len(), 1);
        assert_eq!(
            loaded.entries[0].amendments[0].previous_verdict,
            "incomplete"
        );
        assert_eq!(loaded.entries[0].amendments[0].new_verdict, "wrong");
        assert_eq!(loaded.entries[0].effective_verdict(), Some("wrong"));
        assert_eq!(loaded.entries[1].amendments.len(), 0);
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

    // ------------------------------------------------------------------
    // Amendment mechanism
    // ------------------------------------------------------------------

    /// A manifest written before `amendments` existed — no `[[entry.
    /// amendments]]` block anywhere, exactly what every `audit/<seed>.toml`
    /// committed before this field was added looks like on disk — must
    /// still load, with every entry's `amendments` simply empty. This is
    /// the schema's whole backward-compatibility contract: an old file is
    /// not a migration, it's already valid.
    #[test]
    fn a_manifest_with_no_amendments_field_still_loads() {
        let tmp = tempfile::tempdir().unwrap();
        let path = verdict_path(tmp.path(), 99);
        let raw = r#"
[meta]
seed = 99
sample_size = 1

[[entry]]
tool = "tmux"
stratum = "ok"
verdict = "correct"
k1 = true
"#;
        std::fs::write(&path, raw).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.entries.len(), 1);
        assert!(loaded.entries[0].amendments.is_empty());
        assert_eq!(loaded.entries[0].effective_verdict(), Some("correct"));
        assert_eq!(loaded.entries[0].effective_note(), "");
    }

    /// The ordinary case: a `correct` verdict amended to `wrong`, with a
    /// required reason and a required note on the new value (since `wrong`
    /// obliges one). `effective_verdict`/`effective_note` must report the
    /// amendment; `verdict`/`note` must report the original, untouched.
    #[test]
    fn amend_appends_history_without_touching_the_original_fields() {
        let mut e = entry("tmux", Some("correct"), "");
        amend(
            &mut e,
            "wrong",
            "bundled-short-flag collapse, same shape judged wrong elsewhere".to_string(),
            "reviewer missed the same defect confirmed on other tools in this review".to_string(),
        )
        .unwrap();

        assert_eq!(e.verdict.as_deref(), Some("correct"), "original preserved");
        assert_eq!(e.note, "", "original note preserved");
        assert_eq!(e.effective_verdict(), Some("wrong"));
        assert_eq!(
            e.effective_note(),
            "bundled-short-flag collapse, same shape judged wrong elsewhere"
        );
        assert_eq!(e.amendments.len(), 1);
        assert_eq!(e.amendments[0].previous_verdict, "correct");
        assert_eq!(e.amendments[0].new_verdict, "wrong");
        assert!(!e.amendments[0].reason.is_empty());
    }

    /// A blank or whitespace-only reason is refused — the required-reason
    /// rule this function exists to enforce, mirroring
    /// `verdict_requires_note`'s treatment of a blank note.
    #[test]
    fn amend_refuses_a_blank_reason() {
        let mut e = entry("tmux", Some("correct"), "");
        let err = amend(
            &mut e,
            "wrong",
            "a real finding".to_string(),
            "   ".to_string(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("reason"));
        assert!(
            e.amendments.is_empty(),
            "a rejected amendment leaves no trace"
        );
    }

    /// Amending *to* `wrong`/`incomplete` still needs a note on the new
    /// value, exactly as an ordinary verdict would — the reason explains
    /// why the verdict changed, the note explains what is wrong, and they
    /// are not substitutes for each other.
    #[test]
    fn amend_refuses_a_wrong_verdict_with_no_new_note() {
        let mut e = entry("tmux", Some("correct"), "");
        let err = amend(&mut e, "wrong", "".to_string(), "a real reason".to_string()).unwrap_err();
        assert!(err.to_string().contains("note"));
        assert!(e.amendments.is_empty());
    }

    /// `correct` and `skip` carry no note obligation, so amending *to*
    /// either needs no `new_note` — same asymmetry `verdict_requires_note`
    /// already encodes for an ordinary verdict.
    #[test]
    fn amend_to_correct_needs_no_note() {
        let mut e = entry("openssl", Some("wrong"), "flags missing");
        amend(
            &mut e,
            "correct",
            String::new(),
            "re-read against a later capture; the flags were there after all".to_string(),
        )
        .unwrap();
        assert_eq!(e.effective_verdict(), Some("correct"));
        assert_eq!(e.effective_note(), "");
    }

    /// Amending an entry with no verdict yet is refused — this is not a
    /// backdoor around the ordinary review flow.
    #[test]
    fn amend_refuses_an_entry_with_no_verdict_yet() {
        let mut e = entry("tmux", None, "");
        let err = amend(&mut e, "wrong", "note".to_string(), "reason".to_string()).unwrap_err();
        assert!(err.to_string().contains("no verdict yet"));
    }

    /// Amending to the same verdict the entry already effectively has is
    /// refused — nothing is actually changing, so recording an "amendment"
    /// would just be noise.
    #[test]
    fn amend_refuses_a_no_op_amendment() {
        let mut e = entry("tmux", Some("correct"), "");
        let err = amend(&mut e, "correct", String::new(), "reason".to_string()).unwrap_err();
        assert!(err.to_string().contains("already"));
    }

    /// A second amendment chains onto the first: its `previous_verdict` is
    /// the first amendment's `new_verdict`, not the entry's original
    /// verdict, so the history reads as a true sequence of corrections.
    #[test]
    fn a_second_amendment_chains_onto_the_first() {
        let mut e = entry("tmux", Some("correct"), "");
        amend(
            &mut e,
            "wrong",
            "first finding".to_string(),
            "first reason".to_string(),
        )
        .unwrap();
        amend(
            &mut e,
            "incomplete",
            "actually just incomplete, not fully wrong".to_string(),
            "reconsidered after further review".to_string(),
        )
        .unwrap();
        assert_eq!(e.amendments.len(), 2);
        assert_eq!(e.amendments[0].previous_verdict, "correct");
        assert_eq!(e.amendments[0].new_verdict, "wrong");
        assert_eq!(e.amendments[1].previous_verdict, "wrong");
        assert_eq!(e.amendments[1].new_verdict, "incomplete");
        assert_eq!(e.effective_verdict(), Some("incomplete"));
    }

    /// An amendment round-trips through `save`/`load` byte-for-byte in
    /// meaning: every field of the [`Amendment`] survives, and
    /// `effective_verdict`/`effective_note` on the reloaded entry agree
    /// with the in-memory value before it was written.
    #[test]
    fn an_amendment_round_trips_through_save_and_load() {
        let tmp = tempfile::tempdir().unwrap();
        let path = verdict_path(tmp.path(), 2);
        let mut e = entry("tmux", Some("correct"), "");
        amend(
            &mut e,
            "wrong",
            "bundled-short-flag collapse".to_string(),
            "reviewer inconsistency caught in reconciliation".to_string(),
        )
        .unwrap();
        let file = AuditFile {
            meta: AuditMeta {
                seed: 2,
                sample_size: 1,
            },
            entries: vec![e],
        };
        save(&path, &file).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.entries[0].verdict.as_deref(), Some("correct"));
        assert_eq!(loaded.entries[0].effective_verdict(), Some("wrong"));
        assert_eq!(
            loaded.entries[0].amendments[0].reason,
            "reviewer inconsistency caught in reconciliation"
        );
    }

    // ------------------------------------------------------------------
    // Defect-family labels
    // ------------------------------------------------------------------

    /// A manifest written before `families` existed loads unchanged, with
    /// every entry simply unlabelled — the same backward-compatibility
    /// contract `amendments` carries, and the reason the seed-2 file could
    /// be backfilled incrementally rather than migrated wholesale.
    #[test]
    fn a_manifest_with_no_families_field_still_loads() {
        let tmp = tempfile::tempdir().unwrap();
        let path = verdict_path(tmp.path(), 98);
        std::fs::write(
            &path,
            "[meta]\nseed = 98\nsample_size = 1\n\n[[entry]]\ntool = \"tcpdump\"\nstratum = \
             \"ok\"\nverdict = \"wrong\"\nnote = \"single dash issue\"\n",
        )
        .unwrap();
        let loaded = load(&path).unwrap();
        assert!(loaded.entries[0].families.is_empty());
        assert_eq!(loaded.entries[0].families_derived, None);
        loaded.validate_families().unwrap();
        // ...and an unlabelled judged defect reads as unclassified, not as
        // "no defect family applies".
        assert!(loaded.entries[0].is_unclassified());
    }

    #[test]
    fn every_family_name_is_kebab_case_and_unique() {
        let mut seen = Vec::new();
        for f in DEFECT_FAMILIES {
            assert!(
                f.name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '-' || c.is_ascii_digit()),
                "{:?} is not kebab-case",
                f.name
            );
            assert!(!f.meaning.trim().is_empty(), "{:?} has no meaning", f.name);
            assert!(!seen.contains(&f.name), "{:?} listed twice", f.name);
            seen.push(f.name);
        }
    }

    #[test]
    fn parse_family_accepts_the_set_and_names_it_on_failure() {
        assert_eq!(
            parse_family("bundled-short-flag").unwrap(),
            "bundled-short-flag"
        );
        let err = parse_family("bundled_short_flag").unwrap_err().to_string();
        assert!(err.contains("unrecognized defect family"));
        assert!(
            err.contains("bundled-short-flag"),
            "the error must name the valid set, not just reject: {err}"
        );
        assert!(family_meaning("no-such-family").is_none());
    }

    /// The claim-strength rule this field exists for: labels with no
    /// recorded provenance are refused outright, so a writer that forgets
    /// the field cannot launder a machine reading of a reviewer's prose
    /// into the reviewer's own classification.
    #[test]
    fn families_without_recorded_provenance_are_refused() {
        let mut e = entry("tcpdump", Some("wrong"), "single dash issue");
        e.families = vec!["bundled-short-flag".to_string()];
        let err = e.validate_families().unwrap_err().to_string();
        assert!(err.contains("families_derived"), "{err}");

        e.families_derived = Some(true);
        e.validate_families().unwrap();
    }

    #[test]
    fn an_unrecognized_or_duplicated_family_is_refused() {
        let mut e = labelled("tcpdump", "wrong", &["not-a-real-family"]);
        assert!(e.validate_families().is_err());

        e.families = vec![
            "bundled-short-flag".to_string(),
            "bundled-short-flag".to_string(),
        ];
        let err = e.validate_families().unwrap_err().to_string();
        assert!(err.contains("twice"), "{err}");
    }

    /// A family says what is *wrong*, so a verdict that names no defect
    /// cannot carry one — otherwise a `correct` tool would sit in a
    /// detector's expected-fires set on the strength of a verdict saying
    /// nothing is wrong with it.
    #[test]
    fn a_correct_verdict_may_not_carry_family_labels() {
        let e = labelled("tmux", "correct", &["bundled-short-flag"]);
        let err = e.validate_families().unwrap_err().to_string();
        assert!(err.contains("names no defect"), "{err}");
    }

    /// Labels follow the *effective* verdict, so an amendment that turns a
    /// `correct` into a `wrong` makes that entry labellable — which is
    /// exactly how `tmux` entered the bundled-short-flag calibration set.
    #[test]
    fn an_amended_verdict_decides_whether_labels_are_allowed() {
        let mut e = labelled("tmux", "correct", &["bundled-short-flag"]);
        assert!(e.validate_families().is_err());
        e.verdict = Some("correct".to_string());
        amend(
            &mut e,
            "wrong",
            "bundled-short-flag collapse".to_string(),
            "reviewer inconsistency caught in reconciliation".to_string(),
        )
        .unwrap();
        e.validate_families().unwrap();
        assert!(e.is_judged_defect());
        assert!(!e.is_judged_correct());
        assert!(e.has_family("bundled-short-flag"));
        assert!(!e.is_unclassified());
    }

    /// `skip` is neither good nor bad for calibration purposes: it records
    /// no judgment about the parse, so counting it as either side would put
    /// a number in a confusion matrix that no human ever supported.
    #[test]
    fn skip_is_neither_a_judged_defect_nor_a_judged_correct() {
        let e = entry("xzgrep", Some("skip"), "");
        assert!(!e.is_judged_defect());
        assert!(!e.is_judged_correct());
        assert!(!e.is_unclassified());
    }

    #[test]
    fn unclassified_lists_judged_defects_with_no_label() {
        let file = AuditFile {
            meta: AuditMeta {
                seed: 2,
                sample_size: 4,
            },
            entries: vec![
                labelled("tcpdump", "wrong", &["bundled-short-flag"]),
                entry("pptpsetup", Some("incomplete"), "bad parse"),
                entry("wall", Some("correct"), ""),
                entry("xzgrep", Some("skip"), ""),
            ],
        };
        file.validate_families().unwrap();
        let names: Vec<&str> = file.unclassified().map(|e| e.tool.as_str()).collect();
        assert_eq!(names, vec!["pptpsetup"]);
    }
}
