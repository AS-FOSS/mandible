//! Per-item provenance: which extraction source(s) contributed a
//! [`crate::CommandNode`], [`crate::Flag`], or [`crate::Positional`], and the
//! two-axis [`Authority`] each source carries for merge decisions.
//!
//! See spec §4.2 and §4.4. Provenance lives on each item individually —
//! never as one badge for a whole tree — because after a multi-tier merge a
//! node's own fields and its children's fields may legitimately come from
//! different sources, and a single node-level badge covering both would lie
//! about the children.

use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

/// Which extraction source(s) contributed to an item, in contribution order,
/// plus a confidence score set only by heuristic tiers.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Provenance {
    /// Contributing sources, ordered by contribution (earliest first).
    pub sources: SmallVec<[Source; 2]>,
    /// Set only when a heuristic tier (e.g. Tier B help-text) produced this
    /// item; `None` for structured/authoritative sources.
    pub confidence: Option<f32>,
}

/// Serde representation of [`Provenance`], used instead of deriving
/// `Serialize`/`Deserialize` directly on the struct: `smallvec`'s own
/// `serde` feature currently miscompiles against the split `serde_core`
/// crate introduced in `serde` 1.0.229 (a lifetime error at the derive
/// site). Serializing through a plain `Vec` sidesteps that entirely and
/// keeps the on-the-wire shape identical (`{"sources": [...], "confidence":
/// ...}`).
#[derive(Serialize, Deserialize)]
struct ProvenanceRepr {
    sources: Vec<Source>,
    confidence: Option<f32>,
}

impl Serialize for Provenance {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        ProvenanceRepr {
            sources: self.sources.to_vec(),
            confidence: self.confidence,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Provenance {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let repr = ProvenanceRepr::deserialize(deserializer)?;
        Ok(Provenance {
            sources: SmallVec::from_vec(repr.sources),
            confidence: repr.confidence,
        })
    }
}

impl Provenance {
    /// A `Provenance` crediting a single source, with no confidence score.
    pub fn single(source: Source) -> Provenance {
        let mut sources = SmallVec::new();
        sources.push(source);
        Provenance {
            sources,
            confidence: None,
        }
    }

    /// A `Provenance` crediting a single heuristic source with a confidence
    /// score.
    pub fn with_confidence(source: Source, confidence: f32) -> Provenance {
        let mut sources = SmallVec::new();
        sources.push(source);
        Provenance {
            sources,
            confidence: Some(confidence),
        }
    }

    /// The highest authority on `axis` among this item's contributing
    /// sources. `0` if there are no contributing sources.
    pub fn effective_authority(&self, axis: Axis) -> u8 {
        self.sources
            .iter()
            .map(|s| s.authority().on(axis))
            .max()
            .unwrap_or(0)
    }

    /// Whether an item carrying this provenance could, in principle, have
    /// been described — spec §13's metric design rules. `true` when *any*
    /// contributing source [`Source::can_describe`] (a flag merged from
    /// several tiers is describable if even one of them could have
    /// supplied prose, e.g. a synopsis spelling later reconciled against a
    /// structured entry — see `help_text::sections::flag_spelling_already_present`).
    /// `true` also when there are no contributing sources at all: an empty
    /// `Provenance` is not this codebase's way of saying "usage-synopsis
    /// only," so it must not silently disappear from a describability
    /// count the way a real `HelpTextSynopsis`-only item correctly does.
    pub fn describable(&self) -> bool {
        self.sources.is_empty() || self.sources.iter().any(Source::can_describe)
    }

    /// Merge another `Provenance` into this one: union the source lists
    /// (deduplicated, order preserved) and combine confidence
    /// conservatively (the lower of the two, since overall trust is bounded
    /// by the least-confident contributor).
    pub fn absorb(&mut self, other: &Provenance) {
        for s in &other.sources {
            if !self.sources.contains(s) {
                self.sources.push(s.clone());
            }
        }
        self.confidence = match (self.confidence, other.confidence) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
    }
}

/// One axis of [`Authority`]: structural facts (names, nesting, arity, which
/// flags exist) vs. prose (descriptions, summaries, examples).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// Trust for names, nesting, arity, which flags exist.
    Structural,
    /// Trust for descriptions, summaries, examples.
    Prose,
}

/// The two-axis trust level a [`Source`] carries. See spec §4.4's authority
/// table — the tier with the best structure is frequently not the tier with
/// the best prose, so merge resolves each axis independently rather than by
/// a single priority order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Authority {
    /// Trust for names, nesting, arity, which flags exist.
    pub structural: u8,
    /// Trust for descriptions, summaries, examples.
    pub prose: u8,
}

impl Authority {
    /// The authority value for a given axis.
    pub fn on(&self, axis: Axis) -> u8 {
        match axis {
            Axis::Structural => self.structural,
            Axis::Prose => self.prose,
        }
    }
}

/// The origin of a piece of extracted data. See spec §4.2 and §7.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Source {
    /// A native, version-accurate dynamic probe (Tier E): cobra
    /// `__complete`, clap `CompleteEnv`, argcomplete.
    NativeDynamic {
        /// e.g. `"cobra-dunder-complete"`, `"clap-complete-env"`.
        ///
        /// `String` rather than `&'static str`: a `Source` must round-trip
        /// through the on-disk cache (spec §11), and a borrowed `'static`
        /// string cannot in general be produced by `Deserialize` without
        /// leaking memory.
        protocol: String,
    },
    /// A vendored or live structured catalog (Tier A).
    KnownSpec {
        /// e.g. `"carapace"`, `"withfig"`.
        provider: String,
    },
    /// Structural parsing of a generated shell completion script (Tier C).
    CompletionScript {
        /// e.g. `"zsh"`, `"bash"`.
        shell: String,
    },
    /// Man page extraction (Tier D).
    ManPage {
        /// Whether the page used semantic `mdoc(7)` macros or plain `man(7)`.
        format: ManFormat,
    },
    /// `--help`/`-h`/`help` grammar parsing (Tier B) of a structured block
    /// (an options table, `.TP`-shaped entry, or similar) that carries
    /// prose alongside each flag.
    HelpText,
    /// `--help`/`-h`/`help` grammar parsing (Tier B) of a **usage synopsis**
    /// line specifically (spec [M-15]): `git --help`'s
    /// `[-p | --paginate | -P | --no-pager]`, mined by
    /// `help_text::sections::extract_usage_flags`. A separate variant
    /// rather than a field on `HelpText`, because a synopsis is genuinely a
    /// different extraction site — the same reason `ManPage` carries
    /// `format` and `CompletionScript` carries `shell` instead of `HelpText`
    /// growing a field each of them would also need.
    ///
    /// A usage synopsis lists spellings and value shapes only, **never**
    /// prose, by construction — spec §7 Tier B forbids fabricating a
    /// description for one from neighbouring text. A flag whose only
    /// source is this variant is therefore structurally undescribable, not
    /// merely undescribed: [`Source::can_describe`] says so, and spec
    /// §13's `pct_described` excludes it from the denominator rather than
    /// punishing recall for having found it (the defect [M-15] and this
    /// redefinition both exist to fix — see spec §13's metric design
    /// rules).
    HelpTextSynopsis,
    /// A user-local override file (Tier F).
    UserOverride,
}

/// Which man page macro package produced a [`Source::ManPage`] item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ManFormat {
    /// Semantic macros (`.Fl`, `.Ar`, `.Nm`) — real structure, not inference.
    Mdoc,
    /// Typeset prose with weak semantic tagging.
    Man,
}

impl Source {
    /// This source's two-axis [`Authority`], per spec §4.4's table.
    pub fn authority(&self) -> Authority {
        match self {
            Source::UserOverride => Authority {
                structural: 255,
                prose: 255,
            },
            Source::NativeDynamic { .. } => Authority {
                structural: 200,
                prose: 40,
            },
            Source::CompletionScript { .. } => Authority {
                structural: 150,
                prose: 30,
            },
            Source::KnownSpec { .. } => Authority {
                structural: 120,
                prose: 200,
            },
            Source::ManPage { .. } => Authority {
                structural: 60,
                prose: 180,
            },
            // Same authority for both help-text variants: this split is
            // about *measurement* (can this source's flag carry a
            // description at all — see `can_describe`), not about merge
            // precedence, so a synopsis-derived flag competes for a merge
            // exactly as a table-derived one would.
            Source::HelpText | Source::HelpTextSynopsis => Authority {
                structural: 80,
                prose: 120,
            },
        }
    }

    /// Whether this source could, in principle, have supplied a
    /// description — spec §13's metric design rules (rule 2:
    /// "denominators are conditioned on what the source could have
    /// provided"). `false` only for [`Source::HelpTextSynopsis`]: a usage
    /// synopsis lists spellings and value shapes, never prose, by
    /// construction. Every other source at least *could* have carried a
    /// description, whether or not it did for a given flag.
    pub fn can_describe(&self) -> bool {
        !matches!(self, Source::HelpTextSynopsis)
    }

    /// A short, human-readable label for UI footers, e.g. `"carapace"`,
    /// `"help-text"`.
    pub fn label(&self) -> String {
        match self {
            Source::NativeDynamic { protocol } => protocol.to_string(),
            Source::KnownSpec { provider } => provider.to_string(),
            Source::CompletionScript { shell } => format!("completion-{shell}"),
            Source::ManPage {
                format: ManFormat::Mdoc,
            } => "man(mdoc)".to_string(),
            Source::ManPage {
                format: ManFormat::Man,
            } => "man".to_string(),
            Source::HelpText => "help-text".to_string(),
            Source::HelpTextSynopsis => "help-text-synopsis".to_string(),
            Source::UserOverride => "override".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authority_table_matches_spec() {
        assert_eq!(
            Source::UserOverride.authority(),
            Authority {
                structural: 255,
                prose: 255
            }
        );
        assert_eq!(
            Source::NativeDynamic {
                protocol: "x".to_string()
            }
            .authority(),
            Authority {
                structural: 200,
                prose: 40
            }
        );
        assert_eq!(
            Source::CompletionScript {
                shell: "zsh".to_string()
            }
            .authority(),
            Authority {
                structural: 150,
                prose: 30
            }
        );
        assert_eq!(
            Source::KnownSpec {
                provider: "carapace".to_string()
            }
            .authority(),
            Authority {
                structural: 120,
                prose: 200
            }
        );
        assert_eq!(
            Source::ManPage {
                format: ManFormat::Mdoc
            }
            .authority(),
            Authority {
                structural: 60,
                prose: 180
            }
        );
        assert_eq!(
            Source::HelpText.authority(),
            Authority {
                structural: 80,
                prose: 120
            }
        );
        // [M-15]/§13 metric redefinition: same authority as `HelpText` —
        // the split is about measurement, not merge precedence.
        assert_eq!(
            Source::HelpTextSynopsis.authority(),
            Source::HelpText.authority()
        );
    }

    #[test]
    fn only_help_text_synopsis_cannot_describe() {
        assert!(Source::HelpText.can_describe());
        assert!(Source::UserOverride.can_describe());
        assert!(Source::KnownSpec {
            provider: "carapace".to_string()
        }
        .can_describe());
        assert!(!Source::HelpTextSynopsis.can_describe());
    }

    #[test]
    fn provenance_describable_is_true_if_any_source_can_describe() {
        let synopsis_only = Provenance::single(Source::HelpTextSynopsis);
        assert!(!synopsis_only.describable());

        let mut mixed = Provenance::single(Source::HelpTextSynopsis);
        mixed.absorb(&Provenance::single(Source::HelpText));
        assert!(mixed.describable());

        assert!(Provenance::default().describable());
    }

    #[test]
    fn effective_authority_is_max_over_sources() {
        let p = Provenance {
            sources: SmallVec::from_vec(vec![
                Source::HelpText,
                Source::KnownSpec {
                    provider: "carapace".to_string(),
                },
            ]),
            confidence: None,
        };
        assert_eq!(p.effective_authority(Axis::Prose), 200);
        assert_eq!(p.effective_authority(Axis::Structural), 120);
    }

    #[test]
    fn absorb_dedups_sources() {
        let mut a = Provenance::single(Source::KnownSpec {
            provider: "carapace".to_string(),
        });
        let b = Provenance::single(Source::KnownSpec {
            provider: "carapace".to_string(),
        });
        a.absorb(&b);
        assert_eq!(a.sources.len(), 1);
    }

    #[test]
    fn absorb_takes_min_confidence() {
        let mut a = Provenance::with_confidence(Source::HelpText, 0.9);
        let b = Provenance::with_confidence(Source::HelpText, 0.4);
        a.absorb(&b);
        assert_eq!(a.confidence, Some(0.4));
    }
}
