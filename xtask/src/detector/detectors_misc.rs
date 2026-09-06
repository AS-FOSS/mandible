//! The two anti-fabrication oracles and the remaining option-shape detectors (single-dash-long, repeated-char-flag).
use super::*;

/// The existence oracle (`crate::existence`), registered so the harness's
/// answer for an uncalibratable detector is exercised by a real one rather
/// than only by a test double.
///
/// Its `family()` is `None` and that is a finding, not an omission: across
/// 94 human verdicts, **not one reviewer reported a fabricated subcommand
/// or flag spelling**. The defect [M-10] shipped — `tar`'s 39 invented
/// nodes — has no representative in the labelled set, so this set cannot
/// confirm or refute this oracle at all.
pub(crate) struct ExistenceOracle;

impl Detector for ExistenceOracle {
    fn name(&self) -> &'static str {
        "existence"
    }
    fn family(&self) -> Option<&'static str> {
        None
    }
    fn describes(&self) -> &'static str {
        "a help-text-sourced subcommand name or flag spelling that does not occur in the tool's \
         own raw text (spec §13.1)"
    }
    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        crate::existence::detect(evidence.raw, evidence.root)
            .fabrications
            .iter()
            .map(|f| {
                format!(
                    "{:?} at {:?} does not occur in the raw text",
                    f.name, f.path
                )
            })
            .collect()
    }
    fn self_checks(&self) -> Vec<SelfCheck> {
        existence_self_checks()
    }
}

/// `tar --help`'s real first line, byte-exact. Both halves of the operand
/// rule live in this one string: `OPTION` is the slot that names tar's own
/// flag list, `FILE` is the operand a user passes, and the two are written
/// in identical notation one slot apart.
const TAR_SYNOPSIS: &str = "Usage: tar [OPTION...] [FILE]...\n";

/// `uobjnew`'s real argparse synopsis, byte-exact — the counter-shape: a
/// synopsis that spells its own flags out, where every slot is an operand.
const UOBJNEW_SYNOPSIS: &str =
    "usage: uobjnew [-h] [-l {c,java,ruby,tcl}] [-C TOP_COUNT] [-v] pid [interval]\n";

/// `gh --help`'s real shape, byte-exact: a bare `USAGE` heading (no colon,
/// so it is never a labelled `usage:` marker) followed by a line that opens
/// with the tool's own name and reads as usage grammar with no marker
/// anywhere — the **unlabelled** synopsis this task's own fix taught the
/// oracle to enter. Its trailing `[flags]` is also the anchor case for the
/// vocabulary fallback in `existence::option_list_slot`: the real flag
/// stand-in sits *last* here, not first, which is exactly the shape that
/// would have tripped the position rule into also excluding the genuine
/// leading operand `command` if the two rules ran unconditionally instead
/// of one gating the other.
const GH_UNLABELLED_SYNOPSIS: &str = "USAGE\n  gh <command> <subcommand> [flags]\n";

/// `nfsidmap --help`'s real shape, byte-exact: the C `fprintf(stderr, "%s:
/// Usage: ...", argv[0])` idiom, which repeats the tool's own name twice on
/// one line (once as the `fprintf` prefix, once again as the invocation's
/// own program name) with no ordinary `usage:`-prefixed line anywhere else
/// in the document.
const NFSIDMAP_NAME_PREFIXED_SYNOPSIS: &str = "nfsidmap: Usage: nfsidmap [-vh] [-c || -d] path\n";

/// `vgextend --help`'s real shape, byte-exact: LVM's own bare-own-name
/// convention (the whole `vg*`/`lv*`/`pv*` family shares it) — no docopt
/// notation anywhere on the invocation line itself, only on the line right
/// after it. The entire family (29 tools) was newly flagged on this task's
/// own before/after sweep until this shape was added, every one of them
/// false.
const VGEXTEND_BARE_OWN_NAME_SYNOPSIS: &str = concat!(
    "  vgextend - Add physical volumes to a volume group\n",
    "\n",
    "  vgextend VG PV ...\n",
    "\t[ -A|--autobackup y|n ]\n",
    "\t[ COMMON_OPTIONS ]\n",
);

/// A root carrying `positionals` with the given names, all help-text-sourced.
fn positional_node(name: &str, positionals: &[&str]) -> CommandNode {
    let mut root = CommandNode::new(name, Provenance::single(Source::HelpText));
    root.set_positionals(
        positionals
            .iter()
            .map(|p| Entity::positional(*p, Provenance::single(Source::HelpText)))
            .collect(),
    );
    root
}

/// The existence oracle's own hand-built cases (spec §13.1e).
///
/// The oracle is not calibratable against the labelled set — no reviewer in
/// the seed-2 audit reported a fabricated name — so these cases are the
/// *only* runtime evidence that it still works, and the operand half needs
/// them more than the other two: the grammar fix that removed 15 fabricated
/// operands from the fleet also removed every live example of them, so from
/// the fleet count alone "zero because the fabrications are gone" and "zero
/// because nobody ever wired the positional check up" are the same number.
///
/// The `Silent` cases are what make the `Fires` ones mean anything. Each is
/// a real tool whose operand sits in the same notation, one slot over, from
/// a placeholder — a rule that fired on shape alone would report all of them.
fn existence_self_checks() -> Vec<SelfCheck> {
    vec![
        SelfCheck {
            name: "tar's OPTION operand",
            why: "the defect itself, from the tool that shipped it: `OPTION` is lifted out of \
                  `[OPTION...]`, which names tar's own flag list and is not an argument anyone \
                  passes",
            expect: Expect::Fires(1),
            raw: TAR_SYNOPSIS.to_string(),
            root: positional_node("tar", &["OPTION", "FILE"]),
        },
        SelfCheck {
            name: "tar's FILE operand alone",
            why: "the other half of the fleet-count-of-zero question: after the grammar fix the \
                  same bytes must go silent because the fabricated operand is gone, not because \
                  the rule stopped looking",
            expect: Expect::Silent,
            raw: TAR_SYNOPSIS.to_string(),
            root: positional_node("tar", &["FILE"]),
        },
        SelfCheck {
            name: "uobjnew's two real operands",
            why: "the nearest real false positive: a synopsis that writes its own flags needs no \
                  stand-in for them, so `pid` is an operand in exactly the slot `OPTION` occupies \
                  in tar's. These two were *recovered* by the same commit that removed the 15; \
                  reporting them would undo it",
            expect: Expect::Silent,
            raw: UOBJNEW_SYNOPSIS.to_string(),
            root: positional_node("uobjnew", &["pid", "interval"]),
        },
        SelfCheck {
            name: "an operand named nowhere",
            why: "the base claim underneath the position rule — an operand the document does not \
                  contain at all is still reported, whatever the synopsis's shape",
            expect: Expect::Fires(1),
            raw: TAR_SYNOPSIS.to_string(),
            root: positional_node("tar", &["TELEPORT"]),
        },
        SelfCheck {
            name: "gh's two real operands from its unlabelled synopsis",
            why: "this task's own worked example: `command` and `subcommand` are real, occur \
                  literally in gh's own output, and sit one slot from `flags` — gh's own flag-list \
                  stand-in, written *last* rather than first. Before this fix the line was entirely \
                  invisible to the oracle (no `usage:` marker anywhere on it), so both were reported \
                  as invented; after it, both must go silent and `flags` must not itself become an \
                  attested operand hiding a genuine fabrication spelled the same way",
            expect: Expect::Silent,
            raw: GH_UNLABELLED_SYNOPSIS.to_string(),
            root: positional_node("gh", &["command", "subcommand"]),
        },
        SelfCheck {
            name: "an operand named nowhere beside gh's real unlabelled synopsis",
            why: "the same base claim as tar's TELEPORT case, replayed against the new unlabelled \
                  entry point: recognizing gh's synopsis line must not turn into a blanket amnesty \
                  for anything claimed beside it",
            expect: Expect::Fires(1),
            raw: GH_UNLABELLED_SYNOPSIS.to_string(),
            root: positional_node("gh", &["command", "teleport"]),
        },
        SelfCheck {
            name: "nfsidmap's real operand from the name-prefixed usage idiom",
            why: "the C `\"%s: Usage: ...\"` convention (`nfsidmap: Usage: nfsidmap ...`), the \
                  other synopsis-entry shape this task's fix adds: the tool's own name occurs \
                  twice on one line, and `path` — the real operand — must still attest correctly \
                  once both copies are accounted for",
            expect: Expect::Silent,
            raw: NFSIDMAP_NAME_PREFIXED_SYNOPSIS.to_string(),
            root: positional_node("nfsidmap", &["path"]),
        },
        SelfCheck {
            name: "vgextend's two real operands from its bare own-name line",
            why: "LVM's own convention: the invocation line carries no docopt notation at all, so \
                  the unlabelled shape above can never find it — only the next physical line's \
                  bracketed flag row proves it is usage grammar. Newly flagged the whole `vg*`/ \
                  `lv*`/`pv*` family (29 tools) on this task's own before/after sweep until this \
                  shape was added, every one of them false",
            expect: Expect::Silent,
            raw: VGEXTEND_BARE_OWN_NAME_SYNOPSIS.to_string(),
            root: positional_node("vgextend", &["VG", "PV"]),
        },
        SelfCheck {
            name: "an operand named nowhere beside vgextend's real bare own-name line",
            why: "the same base claim replayed a third time: recognizing this entry shape must not \
                  turn into a blanket amnesty for anything claimed beside it",
            expect: Expect::Fires(1),
            raw: VGEXTEND_BARE_OWN_NAME_SYNOPSIS.to_string(),
            root: positional_node("vgextend", &["VG", "TELEPORT"]),
        },
    ]
}

/// The misattribution oracle (`crate::misattribution`), registered on the
/// same terms as [`ExistenceOracle`]. Its shape — a flag's description
/// belonging to a different flag — is adjacent to `section-header-bleed`
/// and to `missing-flag-description`, but adjacency is not identity, and
/// mapping it onto either would manufacture a matrix from a correspondence
/// nobody verified.
pub(crate) struct MisattributionOracle;

impl Detector for MisattributionOracle {
    fn name(&self) -> &'static str {
        "misattribution"
    }
    fn family(&self) -> Option<&'static str> {
        None
    }
    fn describes(&self) -> &'static str {
        "a flag description containing another flag's spelling, attested at a column-aligned \
         position elsewhere in the same raw text (spec §13.1)"
    }
    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        crate::misattribution::detect(evidence.raw, evidence.root)
            .suspects
            .iter()
            .map(|s| format!("{:?} at {:?}", s.flag, s.path))
            .collect()
    }
}

/// The single-dash long-option split (`crate::single_dash_long`): `-help`
/// read as `-h` carrying a required value `"elp"`.
///
/// The second of the three families sharing `short && !long && value_name`,
/// and the one spec §13.1's K1 pre-tag is named after. Registered beside
/// [`BundledShortFlag`] and [`RepeatedCharFlag`] specifically so the
/// harness's "fired on a tool judged defective of another family" cell can
/// answer the question the three of them exist to keep honest: whether a
/// detector for one family is quietly counting another's findings.
pub(crate) struct SingleDashLong;

/// `single-dash-long`'s declared exclusions. Both are labelled members of the
/// family that this detector's own conditions refuse, each carrying the
/// witness token its [`Ground`] recomputes the refusal from.
const SINGLE_DASH_LONG_EXCLUSIONS: &[Exclusion] = &[
    Exclusion {
        tool: "ip",
        ground: Ground::OptionalBracketedTail {
            token: crate::single_dash_long::IP_BRACKETED_TOKEN,
        },
        note: "ip writes its long options as abbreviation-plus-bracketed-tail \
               (`-h[uman-readable]`, `-b[atch]`, `-rc[vbuf]`), which the grammar records as an \
               Optional value — a value spec a human deliberately typed, which this detector's \
               Required-only fingerprint excludes for the same reason bundling's does",
    },
    Exclusion {
        tool: "sg_emc_trespass",
        ground: Ground::TailIsNotAnOptionName {
            token: crate::single_dash_long::SG_EMC_TRESPASS_TOKEN,
        },
        note: "its help text glues the layout's own colon onto the flag (`-hr: Set Honor \
               Reservation bit`), so the tree stores `-h` + \"r:\" and the tail is not a name. \
               A real miss, and one no tail-shape rule can claim without also admitting every \
               value spec that leaks punctuation",
    },
];

impl Detector for SingleDashLong {
    fn name(&self) -> &'static str {
        "single-dash-long"
    }
    fn family(&self) -> Option<&'static str> {
        Some("single-dash-long")
    }
    fn describes(&self) -> &'static str {
        "an option-table row naming a single-dash long option (`-help`, `-fdump-scos`) parsed as \
         a one-character short flag carrying the rest of the name as a required value"
    }
    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        crate::single_dash_long::detect(evidence.raw, evidence.root)
            .splits
            .iter()
            .map(|s| {
                format!(
                    "{:?} at {:?} was split into {:?} plus a required value",
                    s.token, s.path, s.spelling
                )
            })
            .collect()
    }
    fn scope(&self) -> Scope {
        Scope {
            claim: "option-table-sourced (never synopsis) single-dash tokens whose tail is \
                    option-name-shaped, at least `single_dash_long::MIN_SWALLOWED_CHARS` long, \
                    not a repeat of the flag's own letter, and uniformly lowercase. The case \
                    condition is the load-bearing one and it is not free: it is what keeps the \
                    entire GCC/Clang glued-value convention out (`cargo -Zscript`, `rpcgen \
                    -Dname`, `makewhatis -Tutf8`, `perl -Idirectory`, `cc -oOUTFILE`), all of \
                    them correct parses, and it is equally why an UPPERCASE-led long option is \
                    knowingly out of reach — no measured signal separates the two",
            known_exclusions: SINGLE_DASH_LONG_EXCLUSIONS,
        }
    }
    fn self_checks(&self) -> Vec<SelfCheck> {
        crate::single_dash_long::self_checks()
    }
}

pub(crate) struct RepeatedCharFlag;

impl Detector for RepeatedCharFlag {
    fn name(&self) -> &'static str {
        "repeated-char-flag"
    }
    fn family(&self) -> Option<&'static str> {
        Some("repeated-char-flag")
    }
    fn describes(&self) -> &'static str {
        "a flag whose required value is its own short character repeated (`-vv` -> `-v` + \"v\"), \
         in a document that also declares the bare short flag a boolean"
    }
    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        crate::repeated_char::detect(evidence.raw, evidence.root)
            .misreads
            .iter()
            .map(|m| {
                format!(
                    "{:?} at {:?} was read as {:?} carrying its own letter as a value",
                    m.token, m.path, m.spelling
                )
            })
            .collect()
    }
    fn scope(&self) -> Scope {
        Scope {
            claim: "repeated-character tokens in a document that ALSO writes the bare short flag \
                    as a boolean row. That last condition is the whole safety argument and it \
                    costs real recall: `strace`'s `[-DDD]` and every other synopsis that repeats \
                    a switch without also writing it alone is out of reach, because the only \
                    evidence that would admit them is the token's shape — and `lessecho`'s real \
                    `[-nn]` (a genuine `-n` taking a number) has exactly that shape and is a \
                    correct parse. No labelled member of this family is excluded",
            known_exclusions: &[],
        }
    }
    fn self_checks(&self) -> Vec<SelfCheck> {
        crate::repeated_char::self_checks()
    }
}

/// `wrapped-prose-row-boundary` (`crate::wrapped_prose`, atlas S-027): a
/// description written as running prose wraps onto a physical line that
/// begins with a dash-led word, and the grammar reads that continuation as
/// the start of a new flag row.
///
/// Not calibratable against the seed-2/seed-4 labelled sets today — its two
/// ground-truth fixtures (`zgrep`, `resolvconf`) sit in the audit queue,
/// unreviewed — so `family()` still returns `Some`, and `detector
/// calibrate` against either seed reports 0/0 while still checking no fire
/// on a `correct`-judged tool. The fleet-wide claim rests on the hand-built
/// self-checks (`crate::wrapped_prose::self_checks`) alone.
pub(crate) struct WrappedProseRowBoundary;

impl Detector for WrappedProseRowBoundary {
    fn name(&self) -> &'static str {
        "wrapped-prose-row-boundary"
    }
    fn family(&self) -> Option<&'static str> {
        Some("wrapped-prose-row-boundary")
    }
    fn describes(&self) -> &'static str {
        "a physical line beginning with a dash-led word, at the same indent as an unfinished \
         sentence above it and with no aligned description column of its own, whose own leading \
         spelling reached the tree as a flag"
    }
    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        crate::wrapped_prose::detect(evidence.raw, evidence.root)
            .findings
            .iter()
            .map(|f| format!("{:?} fabricated from the line {:?}", f.flag, f.line))
            .collect()
    }
    fn scope(&self) -> Scope {
        Scope {
            claim: "a continuation line's OWN leading spelling only — a second dash-led spelling \
                    the same row-merge pulls from mid-line (resolvconf's real `--enable-updates`, \
                    read off the middle of the `-I` line) is out of reach; see this detector's \
                    own module doc comment",
            known_exclusions: &[],
        }
    }
    fn self_checks(&self) -> Vec<SelfCheck> {
        crate::wrapped_prose::self_checks()
    }
}

/// A second `unparsed-positional` detector, narrower than
/// [`crate::detector::UnparsedArgparsePositional`] and reaching a disjoint
/// shape: a usage line's own trailing operand token (bracketed or bare,
/// atlas S-041), rather than argparse's `positional arguments:` heading.
///
/// Calibratable against `audit-seed4` (`--seed 4 --fixture-version
/// audit-seed4`), where all three of its ground-truth tools
/// (`bashbug`, `lessecho`, `vim.basic`) are labelled — `vim.basic` also
/// carries the label under seed 2 (`audit-seed2`, `k1 = true`), so a
/// default-seed run exercises it too, alongside every seed-2 `correct`
/// verdict as a false-alarm check.
pub(crate) struct UnparsedTailOperand;

impl Detector for UnparsedTailOperand {
    fn name(&self) -> &'static str {
        "unparsed-tail-operand"
    }
    fn family(&self) -> Option<&'static str> {
        Some("unparsed-positional")
    }
    fn describes(&self) -> &'static str {
        "a usage line's own last token group is not flag-shaped once its brackets are stripped \
         (a real operand name, not a placeholder), and the root has no positionals at all"
    }
    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        crate::tail_operand::detect(evidence.raw, evidence.root)
            .findings
            .iter()
            .map(|f| {
                format!(
                    "{:?} never became a positional, from the usage line {:?}",
                    f.operand, f.usage_line
                )
            })
            .collect()
    }
    fn scope(&self) -> Scope {
        Scope {
            claim: "lowercase-led operand names in the usage line's own trailing token group, \
                    where every earlier group is itself a flag or a flag-list placeholder \
                    (`arguments`, `options`) only; an ALL-CAPS metavariable tail (`FILE`, \
                    `PATTERN`) and a usage line naming multiple bare operands \
                    (`infont intable outfont`) are both deliberately out of reach — see this \
                    detector's own module doc comment",
            known_exclusions: &[],
        }
    }
    fn self_checks(&self) -> Vec<SelfCheck> {
        crate::tail_operand::self_checks()
    }
}

/// `ragged-command-table` (`crate::ragged_command_table`, atlas S-104): a
/// command table whose rows carry an optional short-alias prefix
/// ragged-indents its own rows, dropping the shallower rows and the run
/// of siblings after them. Generalizes `unparsed-subcommand` shape E; see
/// `mandible-core/src/audit.rs`'s own comment on that family.
pub(crate) struct RaggedCommandTable;

impl Detector for RaggedCommandTable {
    fn name(&self) -> &'static str {
        "ragged-command-table"
    }
    fn family(&self) -> Option<&'static str> {
        Some("unparsed-subcommand")
    }
    fn describes(&self) -> &'static str {
        "a run of 2+ adjacent command-table rows, one bearing a short-alias-comma prefix, whose \
         primary name never reaches the tree"
    }
    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        crate::ragged_command_table::detect(evidence.raw, evidence.root)
            .findings
            .iter()
            .map(|f| {
                let alias = f.alias.as_deref().unwrap_or("none");
                format!("{:?} (alias {alias:?}) missing, from {:?}", f.name, f.row)
            })
            .collect()
    }
    fn scope(&self) -> Scope {
        Scope {
            claim: "shape E only (a ragged alias-column table) of `unparsed-subcommand`'s five \
                    grammars — shapes B, C and D are out of reach the same way \
                    `unparsed-command-table` declares them, and are not re-declared here since \
                    this detector never claimed them",
            known_exclusions: &[],
        }
    }
    fn self_checks(&self) -> Vec<SelfCheck> {
        crate::ragged_command_table::self_checks()
    }
}

/// `wrapped-command-continuation-as-subcommand` (`crate::wrapped_command_continuation`,
/// atlas S-103): a command's description wraps onto a line with no column
/// of its own, and the grammar reads that continuation's own leading word
/// as a fresh subcommand. See that module's own doc comment for how this
/// differs from `wrapped-prose-row-boundary`.
pub(crate) struct WrappedCommandContinuation;

impl Detector for WrappedCommandContinuation {
    fn name(&self) -> &'static str {
        "wrapped-command-continuation-as-subcommand"
    }
    fn family(&self) -> Option<&'static str> {
        Some("wrapped-command-continuation-as-subcommand")
    }
    fn describes(&self) -> &'static str {
        "a bare single word, at the same indent as an unfinished sentence above it and with no \
         aligned column of its own, that reached the tree as a subcommand"
    }
    fn hits(&self, evidence: &ToolEvidence<'_>) -> Vec<String> {
        crate::wrapped_command_continuation::detect(evidence.raw, evidence.root)
            .findings
            .iter()
            .map(|f| format!("{:?} fabricated from the line {:?}", f.name, f.line))
            .collect()
    }
    fn scope(&self) -> Scope {
        Scope {
            claim: "a continuation line's own single leading word only — see this detector's \
                    own module doc comment",
            known_exclusions: &[],
        }
    }
    fn self_checks(&self) -> Vec<SelfCheck> {
        crate::wrapped_command_continuation::self_checks()
    }
}
