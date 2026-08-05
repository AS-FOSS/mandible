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
    /// `--help`/`-h`/`help` grammar parsing (Tier B).
    HelpText,
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
            Source::HelpText => Authority {
                structural: 80,
                prose: 120,
            },
        }
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
