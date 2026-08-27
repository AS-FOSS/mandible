//! Non-ASCII glyphs, and the ASCII set to fall back to.
//!
//! mandible's whole pitch is that it works on any tool, anywhere — and
//! "anywhere" includes the environments where it is most useful: SSH'd
//! into an unfamiliar box, a bare Linux virtual console, a container with
//! `LANG` unset. Those are exactly the places a chevron or a box-drawing
//! border renders as tofu.
//!
//! The rule this module encodes: **a glyph may only be used if there is
//! something legible to fall back to.** That is what separates the
//! techniques this project will use (box-drawing, block elements, colour,
//! bold/reverse) from the ones it won't (Nerd Font icons, Sixel):
//!
//! - These degrade to *less pretty*. The text is still readable.
//! - A private-use-area icon degrades to `□`, which carries no meaning,
//!   and cannot be detected in advance — you can ask a terminal about its
//!   colour depth, never about its font.
//!
//! Selection happens once at startup from the locale, because that is the
//! only signal available: there is no escape sequence that asks a terminal
//! which glyphs it can draw.

/// The characters the UI draws with, chosen once at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Glyphs {
    /// Marker for an expanded tree row.
    pub chevron_open: char,
    /// Marker for a collapsed tree row.
    pub chevron_closed: char,
    /// Truncation marker.
    pub ellipsis: &'static str,
    /// Prefix for the search input.
    pub prompt: &'static str,
    /// Separator between breadcrumb path segments.
    pub breadcrumb: &'static str,
    /// Separator between provenance footer fields.
    pub dot: &'static str,
    /// "This axis is trustworthy" marker in the provenance footer.
    pub check: &'static str,
    /// Placeholder shown while a node's children are being extracted.
    pub loading: &'static str,
    /// Whether box-drawing borders are safe; ASCII gets plain ones.
    pub rounded_borders: bool,
    /// Arrow pair for the vertical-movement hint.
    pub arrows_vertical: &'static str,
    /// Arrow pair for the horizontal-movement hint.
    pub arrows_horizontal: &'static str,
    /// Em dash, used where a value is absent.
    pub absent: &'static str,
    /// Horizontal rule drawn after a section heading.
    pub rule: &'static str,
    /// Overflow affordance drawn in the detail pane's top border when
    /// horizontally-scrollable content extends further left than the
    /// current view (spec §9: preformatted content scrolls rather than
    /// wraps, and needs a visible sign there is more to see).
    pub more_left: char,
    /// Same, for content extending further right.
    pub more_right: char,
}

/// The full set, for terminals in a UTF-8 locale.
pub const UNICODE: Glyphs = Glyphs {
    chevron_open: '▾',
    chevron_closed: '▸',
    ellipsis: "…",
    prompt: "›",
    breadcrumb: "›",
    dot: "·",
    check: "✓",
    loading: "⋯ loading",
    rounded_borders: true,
    arrows_vertical: "↑↓",
    arrows_horizontal: "←→",
    absent: "—",
    rule: "─",
    more_left: '←',
    more_right: '→',
};

/// The fallback. Every entry is one column wide or plainly readable, so
/// nothing shifts alignment or disappears.
pub const ASCII: Glyphs = Glyphs {
    chevron_open: 'v',
    chevron_closed: '>',
    ellipsis: "...",
    prompt: ">",
    breadcrumb: ">",
    dot: "|",
    check: "ok",
    loading: "... loading",
    rounded_borders: false,
    arrows_vertical: "up/dn",
    arrows_horizontal: "left/right",
    absent: "-",
    rule: "-",
    more_left: '<',
    more_right: '>',
};

/// Pick a glyph set from the environment.
///
/// `MANDIBLE_ASCII` forces the fallback regardless of locale — the escape
/// hatch for a terminal that claims UTF-8 and renders it badly anyway,
/// which no amount of probing can predict.
///
/// Otherwise: UTF-8 only if the locale says so. `LC_ALL` overrides
/// `LC_CTYPE` overrides `LANG`, per POSIX. An unset locale means `C`,
/// which is *not* UTF-8 — and that is the common case inside a minimal
/// container, so defaulting the other way would put tofu exactly where
/// this tool is most often reached for.
pub fn from_env() -> Glyphs {
    if std::env::var_os("MANDIBLE_ASCII").is_some_and(|v| !v.is_empty()) {
        return ASCII;
    }
    let locale = ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .find_map(|k| std::env::var_os(k).filter(|v| !v.is_empty()))
        .unwrap_or_default();
    let locale = locale.to_string_lossy().to_ascii_lowercase();
    if locale.contains("utf-8") || locale.contains("utf8") {
        UNICODE
    } else {
        ASCII
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_fallback_is_pure_ascii() {
        // The whole point: nothing here can render as tofu.
        assert!(ASCII.chevron_open.is_ascii());
        assert!(ASCII.chevron_closed.is_ascii());
        assert!(ASCII.more_left.is_ascii());
        assert!(ASCII.more_right.is_ascii());
        for s in [
            ASCII.ellipsis,
            ASCII.prompt,
            ASCII.breadcrumb,
            ASCII.dot,
            ASCII.check,
            ASCII.loading,
            ASCII.arrows_vertical,
            ASCII.arrows_horizontal,
            ASCII.absent,
            ASCII.rule,
        ] {
            assert!(s.is_ascii(), "{s:?} is not ASCII");
        }
    }

    #[test]
    fn unicode_set_is_actually_non_ascii() {
        assert!(!UNICODE.ellipsis.is_ascii());
        assert_ne!(UNICODE.rounded_borders, ASCII.rounded_borders);
    }
}
