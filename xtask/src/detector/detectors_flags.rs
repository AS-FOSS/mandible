//! Flag-shaped detectors: bundled short flags, brace alternation, unparsed command tables, and dropped aliases.
use super::*;

/// The bundled-short-flag collapse (`crate::bundling`): a synopsis bundle
/// of boolean short flags parsed as one flag swallowing the rest as a
/// value. Zero `crate::existence` fabrications on a collapsed `-2CDlNuVv`
/// is not a claim of a correct parse — the collapsed token attests cleanly
/// while the parse destroys seven flags.
///
/// Shares a structural fingerprint (`short && !long && value_name`) with
/// `single-dash-long` and `repeated-char-flag` (all under `k1 = true` in
/// the labelled set); `crate::bundling` discriminates on what the
/// swallowed text is, not on structure.
pub(crate) struct BundledShortFlag;

/// `bundled-short-flag`'s declared exclusions. The one entry is the shape
/// of every future one: a witness token, the constant it falls below, and
/// arithmetic that has to agree — see [`Exclusion`].
pub(crate) const BUNDLED_SHORT_FLAG_EXCLUSIONS: &[Exclusion] = &[Exclusion {
    tool: "ssh-keygen",
    ground: Ground::BelowMemberThreshold {
        cluster: crate::bundling::SSH_KEYGEN_CLUSTER,
        constant: "bundling::MIN_BUNDLED_MEMBERS",
        threshold: crate::bundling::MIN_BUNDLED_MEMBERS,
    },
    note: "a real collapse this detector knowingly does not claim, not an oversight: at one \
           swallowed member the shape is genuinely ambiguous, and the fleet scan found the \
           one-member population is about half correct parses (`xxd -ps`, `which -as`, \
           `sg_map -st`, `mandoc -ac`)",
}];

impl Detector for BundledShortFlag {
    fn name(&self) -> &'static str {
        "bundled-short-flag"
    }
    fn family(&self) -> Option<&'static str> {
        Some("bundled-short-flag")
    }
    fn describes(&self) -> &'static str {
        "a synopsis bundle of boolean short flags (`[-abcXYZ]`) parsed as one flag carrying the \
         rest as a required value, destroying every other member"
    }
    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        crate::bundling::detect(evidence.raw, evidence.root)
            .collapses
            .iter()
            .map(|c| {
                format!(
                    "{:?} at {:?} swallows {} member(s) of the cluster {:?}",
                    c.spelling, c.path, c.destroyed, c.cluster
                )
            })
            .collect()
    }
    fn scope(&self) -> Scope {
        Scope {
            claim: "synopsis-sourced short-flag clusters with 2 or more swallowed members \
                    (`bundling::MIN_BUNDLED_MEMBERS`); a single swallowed member is deliberately \
                    excluded because the fleet scan found it genuinely ambiguous — see this \
                    detector's own module doc comment for the measured counter-examples \
                    (`xxd -ps`, `which -as`, `sg_map -st`, `mandoc -ac`) that a looser threshold \
                    would false-positive on",
            known_exclusions: BUNDLED_SHORT_FLAG_EXCLUSIONS,
        }
    }
    fn self_checks(&self) -> Vec<SelfCheck> {
        crate::bundling::self_checks()
    }
}

/// `brace-alternation-flag` (`crate::alternation`), the fourth oracle and
/// second family detector. Its `hits` calls the same
/// `help_text::parse_flag_alternation` the grammar calls, rather than
/// restating the rule — imports the grammar's own rule rather than
/// risking the drift `crate::misattribution`'s hand-copied `pick_stream`
/// cost 200 of 656 fabrications.
///
/// Fleet count is reported, not ratcheted at zero: `btrfs`'s `btrfs
/// device scan [-d|--all-devices] <device>` reads the alternation
/// correctly, but the flags belong to a subcommand node
/// `unparsed-subcommand` prevents from existing, and `-d` is not a root
/// flag — reaching further would assert something false. `btrfs` is the
/// same tool [`UnparsedCommandTable`] excludes as its shape C; whichever
/// fix lands first zeroes the other's residual for free.
///
/// Declared scope names no excluded tool (honestly — both labelled
/// members, `cache_restore` and `eqn`, are inside the claim), but does
/// carry the two shapes this detector knowingly does not reach.
pub(crate) struct BraceAlternationFlag;

impl Detector for BraceAlternationFlag {
    fn name(&self) -> &'static str {
        "brace-alternation-flag"
    }
    fn family(&self) -> Option<&'static str> {
        Some("brace-alternation-flag")
    }
    fn describes(&self) -> &'static str {
        "a delimited alternation of bare flag spellings (`{-i|--input}`, `[[-c|-C] cmd]`) whose \
         members reach no flag in the tree, or whose surviving member kept the group's own \
         punctuation as its value"
    }
    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        crate::alternation::detect(evidence.raw, evidence.root)
            .findings
            .iter()
            .map(|f| format!("{} at {:?}: {}", f.group, f.path, f.detail))
            .collect()
    }
    fn scope(&self) -> Scope {
        Scope {
            claim: "delimited groups offering at least 2 bare flag spellings \
                    (`alternation::MIN_ALTERNATIVES`); a one-member group is an ordinary \
                    bracketed optional flag the synopsis path already reads correctly, and an \
                    alternation whose members carry their own values (`sg_sanitize`'s \
                    `--count=OC|-c OC`) is the value-name-mangled family — genuinely ambiguous \
                    about whether one value or two are meant, so neither this detector nor the \
                    grammar claims it",
            known_exclusions: &[],
        }
    }
    fn self_checks(&self) -> Vec<SelfCheck> {
        crate::alternation::self_checks()
    }
}

pub(crate) struct UnparsedCommandTable;

/// `unparsed-command-table`'s declared exclusions: the three
/// `unparsed-subcommand` tools that turned out to write their subcommand
/// lists in entirely different grammars (see `crate::commandtable`'s module
/// doc comment for the four-shape breakdown). Each cites a real line from
/// the tool's own capture, and [`Ground::UnreadableEntryShape`] runs this
/// detector's row parser over it, so none of the three can be excluded by
/// assertion.
const UNPARSED_COMMAND_TABLE_EXCLUSIONS: &[Exclusion] = &[
    Exclusion {
        tool: "apt-ftparchive",
        ground: Ground::UnreadableEntryShape {
            entry: crate::commandtable::APT_FTPARCHIVE_ENTRY,
            grammar: crate::commandtable::ENTRY_GRAMMAR,
        },
        note: "shape B: the `Commands:` label carries the first entry on its own line and the \
               rest hang under it by alignment, with no dash column anywhere — a different \
               recovery problem that shares only the audit label",
    },
    Exclusion {
        tool: "btrfs",
        ground: Ground::UnreadableEntryShape {
            entry: crate::commandtable::BTRFS_ENTRY,
            grammar: crate::commandtable::ENTRY_GRAMMAR,
        },
        note: "shape C: no command heading exists at all; the names are recoverable only by \
               stripping the repeated program name off a catalogue of full usage lines, two \
               levels deep (`btrfs balance start`)",
    },
    Exclusion {
        tool: "ip",
        ground: Ground::UnreadableEntryShape {
            entry: crate::commandtable::IP_ENTRY,
            grammar: crate::commandtable::ENTRY_GRAMMAR,
        },
        note: "shape D: the objects are a brace-delimited, pipe-separated alternation set bound \
               to a metavariable used in the usage line — and this tool's own corpus contract \
               tests flags (`-V`, `-s`, `-d`), not subcommands, which is its own evidence that \
               the `unparsed-subcommand` label is not what is chiefly wrong with it",
    },
];

impl Detector for UnparsedCommandTable {
    fn name(&self) -> &'static str {
        "unparsed-command-table"
    }
    fn family(&self) -> Option<&'static str> {
        Some("unparsed-subcommand")
    }
    fn describes(&self) -> &'static str {
        "a dash-separated `<name>  - <description>` command table under a `commands:` heading \
         whose every name is absent from the tree, because the usage-block scanner joined the \
         heading and all its rows into one synopsis string"
    }
    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        crate::commandtable::detect(evidence.raw, evidence.root)
            .missing
            .iter()
            .map(|t| {
                format!(
                    "{:?} offers {} command(s) {:?}, none of which reached the tree",
                    t.heading,
                    t.names.len(),
                    t.names
                )
            })
            .collect()
    }
    fn scope(&self) -> Scope {
        Scope {
            claim: "shape A only — a `commands:` heading followed by two or more indented \
                    `<name>  - <description>` rows (`commandtable::MIN_TABLE_ENTRIES`), with \
                    none of the names in the tree. The `unparsed-subcommand` label covers four \
                    unrelated grammars and this detector deliberately reads one of them; the \
                    other three are named below with a witness line each, because one detector \
                    loose enough to span all four would be worthless in each",
            known_exclusions: UNPARSED_COMMAND_TABLE_EXCLUSIONS,
        }
    }
    fn self_checks(&self) -> Vec<SelfCheck> {
        crate::commandtable::self_checks()
    }
}

/// The dropped-alias defect (`crate::dropped_alias`): a flag documented
/// with both a short and a long spelling reaching the tree with only one
/// of them, because a value spec interrupted its alias list. Like
/// [`BundledShortFlag`], the two anti-fabrication oracles are structurally
/// blind to it — nothing here is invented, what's wrong is what's absent.
///
/// Its risk runs the opposite way to every other detector's: over-firing
/// argues for a fix that would merge two genuinely different flags, and a
/// fabricated alias is strictly worse than a dropped one. Two of the
/// seven labelled tools are declared out of scope with structural
/// grounds rather than reached for.
pub(crate) struct DroppedAliasDetector;

/// `dropped-alias`'s declared exclusions: the two labelled tools whose
/// dropped spelling is not separated from its partner by a value spec at
/// all, each with the witness its [`Ground`] is computed from.
pub(crate) const DROPPED_ALIAS_EXCLUSIONS: &[Exclusion] = &[
    Exclusion {
        tool: "eqn",
        ground: Ground::InsideAlternationGroup {
            token: crate::dropped_alias::EQN_VERSION_GROUP,
            family: "brace-alternation-flag",
        },
        note: "A MISLABEL IN THE MANIFEST, reported rather than amended here: `eqn` carries \
               `dropped-alias` and its shape is brace alternation, which the manifest already \
               has a family for. The two are not the same defect — a brace group loses the \
               spelling because the tokenizer never opens the group, not because a value spec \
               interrupted an alias list — and the same alternation rule closes `cache_restore`'s \
               `{-i|--input} <file>` and `xfs_io`'s `[[-c|-C] cmd]...` too, neither of which has \
               a value spec in the way. Claiming it here would let this detector's fleet count \
               stand in for a fix it did not make, and makes this family look one tool bigger \
               than it is",
    },
    Exclusion {
        tool: "jdeprscan",
        ground: Ground::AcrossDescriptionColumn {
            row: crate::dropped_alias::JDEPRSCAN_LIST_ROW,
            constant: "help_text::MIN_COLUMN_GAP_SPACES",
            column_gap: mandible_extract::help_text::MIN_COLUMN_GAP_SPACES,
        },
        note: "two shapes in one tool, neither an interrupted alias list: `-l    --list` puts its \
               long form past the description column — since recovered, by the aligned-spelling- \
               column split (`help_text::sections::spelling_run`), so this half of the ground is \
               now historical and the row is kept here because the ground is still *measured* \
               from it — and `-? -h --help` names a second short that this module's `short()` \
               accessor has no way to surface (one `Option<char>`), exactly as `-A, --catenate, \
               --concatenate` names a second long its `long()` accessor cannot surface either; \
               that second half is what still excludes the tool",
    },
];

impl Detector for DroppedAliasDetector {
    fn name(&self) -> &'static str {
        "dropped-alias"
    }
    fn family(&self) -> Option<&'static str> {
        Some("dropped-alias")
    }
    fn describes(&self) -> &'static str {
        "a flag whose value spec interrupted its own alias list: the tool documents `-p PID, \
         --pid PID` (or `--count=OC|-c OC`) and the tree carries only one of the two spellings"
    }
    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        crate::dropped_alias::detect(evidence.raw, evidence.root)
            .drops
            .iter()
            .map(|d| {
                format!(
                    "{:?} at {:?} keeps {:?} and drops {:?}, documented together as {:?}",
                    d.kept, d.path, d.kept, d.dropped, d.witness
                )
            })
            .collect()
    }
    fn scope(&self) -> Scope {
        Scope {
            claim: "alias pairs a VALUE SPEC came between — `-p PID, --pid PID`, \
                    `--count=OC|-c OC` — where the separator sits at the stored placeholder's own \
                    boundary and a whole flag spelling follows it. Pairs separated by anything \
                    else are deliberately not claimed: a wide space run is the description column \
                    (`jdeprscan`), a brace group is its own labelled family (`eqn`), and a second \
                    short or a second long has no accessor to reach at all (`Entity::short`/ \
                    `Entity::long` each surface one). \
                    Narrow on purpose — the loose rule this replaces would merge two genuinely \
                    different flags, and a fabricated alias is worse than a dropped one",
            known_exclusions: DROPPED_ALIAS_EXCLUSIONS,
        }
    }
    fn self_checks(&self) -> Vec<SelfCheck> {
        crate::dropped_alias::self_checks()
    }
}
