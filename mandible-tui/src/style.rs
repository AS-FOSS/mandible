//! The styling contract (spec §9.2): one accent, spent only on
//! information; everything else neutral.
//!
//! Seven roles carry every colour mandible draws: accent, warning, muted,
//! section, group, selection and selection text. A theme gives a value to
//! each ([`Roles`]) and the [`Palette`] holds the values in force, so a
//! theme change is a new palette. Rules that matter more than the values:
//!
//! - **No RGB.** A value is a palette index. 0 to 15 are the named ANSI
//!   colors, which resolve through the user's own terminal theme; 16 to
//!   255 are the xterm-256 entries. No `Color::Rgb(..)` appears anywhere
//!   in this crate.
//! - **`DarkGray` over `Modifier::DIM`** for muted text. Several terminals
//!   ignore `DIM` outright and others render it nearly invisible.
//! - **Respect `NO_COLOR`** (<https://no-color.org>) **and `TERM=dumb`**:
//!   every style function degrades to bold/reverse/underline only. A theme
//!   changes colors and never a modifier.
//! - **Depth is consulted for numbers 16 and up.** Without the xterm-256
//!   palette ([`Palette::extended`]) such a role takes the named color the
//!   default theme gives it: accent cyan, warning yellow, muted and both
//!   rule shades `DarkGray`, and the selection bar a reversed row.
//! - **The accent is spent only on the payload the user came for**: flag
//!   spellings, the selected row, the focused pane's border.

use mandible_core::config::{ColorIndex, Role, Roles, Theme};
use ratatui::style::{Color, Modifier, Style};

/// The default theme's values as terminal colors, for the tests that check
/// the default theme draws what it always has.
#[cfg(test)]
pub(crate) const ACCENT: Color = Color::Cyan;
#[cfg(test)]
pub(crate) const BAR_BG: u8 = 237;
#[cfg(test)]
pub(crate) const BAR_FG: u8 = 252;

/// Muted text (spec §9.2: tree summaries, breadcrumb ancestors, group
/// dividers — label and rule alike, provenance footer, inherited-group
/// flags, deprecated tags).
/// `DarkGray`, not `Modifier::DIM`. Degrades to no styling under
/// `NO_COLOR` — muted text carries no meaning beyond "less important than
/// its neighbors," which has nothing to communicate once color is off.
pub fn muted(palette: Palette) -> Style {
    if palette.color {
        Style::default().fg(palette.pick(palette.roles.muted, Color::DarkGray))
    } else {
        Style::default()
    }
}

/// The suggestion after the text typed in the search box. Muted, so it
/// reads as less certain than the input. With color off muted text is
/// plain, which would pass for typed text, so it is italic and dim there.
pub fn ghost(palette: Palette) -> Style {
    if palette.color {
        muted(palette)
    } else {
        Style::default().add_modifier(Modifier::ITALIC | Modifier::DIM)
    }
}

/// Muted + bold: section headings (`DESCRIPTION`, `FLAGS`, `INHERITED`,
/// flag group headings).
pub fn muted_bold(palette: Palette) -> Style {
    muted(palette).add_modifier(Modifier::BOLD)
}

/// The first index of the xterm-256 gray ramp (`#080808`).
pub const GRAY_RAMP_FIRST: u8 = 232;
/// The last index of the xterm-256 gray ramp (`#eeeeee`).
pub const GRAY_RAMP_LAST: u8 = 255;

/// What the terminal can be asked to draw: whether color is wanted at all,
/// and whether the xterm-256 palette is available.
///
/// `extended` is meaningless when `color` is false, and every function
/// here treats it that way. The pair exists as one value rather than two
/// loose `bool`s because they are read together at exactly the points
/// where a shade is chosen, and two adjacent booleans at a call site are
/// the kind of thing that gets swapped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// Whether to emit color at all — [`color_enabled_from_env`].
    pub color: bool,
    /// Whether the xterm-256 palette is available — [`extended_from_env`].
    pub extended: bool,
    /// The value of each color role, from the theme in force.
    pub roles: Roles,
}

impl Palette {
    /// The palette this process's environment describes for output written
    /// to stdout.
    pub fn from_env() -> Palette {
        Palette::for_sink(crate::terminal::Sink::Stdout)
    }

    /// The palette for output written to `sink`.
    ///
    /// The sink is part of the question, not a detail: the "is anything a
    /// terminal at the other end" half of [`color_enabled_for_sink`] is
    /// about the stream being drawn on. `mandible --print-selection` draws
    /// on stderr while stdout is a pipe, and asking stdout there would
    /// answer about the pipe and render the whole UI monochrome.
    pub fn for_sink(sink: crate::terminal::Sink) -> Palette {
        let color = color_enabled_for_sink(sink);
        Palette {
            color,
            extended: color && extended_from_env(),
            roles: Roles::DEFAULT,
        }
    }

    /// No color at all: `NO_COLOR`, `TERM=dumb`, or a redirected stdout.
    pub fn plain() -> Palette {
        Palette {
            color: false,
            extended: false,
            roles: Roles::DEFAULT,
        }
    }

    /// Color, but only the sixteen named ones.
    pub fn basic() -> Palette {
        Palette {
            color: true,
            extended: false,
            roles: Roles::DEFAULT,
        }
    }

    /// Color including the xterm-256 palette.
    pub fn extended() -> Palette {
        Palette {
            color: true,
            extended: true,
            roles: Roles::DEFAULT,
        }
    }

    /// The same depth with `roles` in force.
    pub fn with_roles(self, roles: Roles) -> Palette {
        Palette { roles, ..self }
    }

    /// The palette `theme` yields at this depth. `custom` is the user's own
    /// seven values, used by the `custom` theme; `none` draws no color.
    pub fn themed(self, theme: Theme, custom: Roles) -> Palette {
        match theme {
            Theme::None => Palette::plain(),
            Theme::Custom => self.with_roles(custom),
            theme => self.with_roles(theme.roles().unwrap_or_default()),
        }
    }

    /// The SGR parameters that set the foreground to `role`'s color, for
    /// output written as escape codes outside ratatui. `None` without color.
    pub fn sgr(self, role: Role) -> Option<String> {
        if !self.color {
            return None;
        }
        let (index, fallback) = match role {
            Role::Accent => (self.roles.accent, Color::Cyan),
            Role::Warning => (self.roles.warning, Color::Yellow),
            Role::Muted => (self.roles.muted, Color::DarkGray),
            Role::Section => (self.roles.section, Color::DarkGray),
            Role::Group => (self.roles.group, Color::DarkGray),
            Role::Selection => (self.roles.selection, Color::DarkGray),
            Role::SelectionText => (self.roles.selection_text, Color::Gray),
        };
        Some(match self.pick(index, fallback) {
            Color::Indexed(n) => format!("38;5;{n}"),
            Color::DarkGray => "90".to_string(),
            Color::Gray => "37".to_string(),
            Color::Black => "30".to_string(),
            Color::Red => "31".to_string(),
            Color::Green => "32".to_string(),
            Color::Yellow => "33".to_string(),
            Color::Blue => "34".to_string(),
            Color::Magenta => "35".to_string(),
            Color::Cyan => "36".to_string(),
            Color::LightRed => "91".to_string(),
            Color::LightGreen => "92".to_string(),
            Color::LightYellow => "93".to_string(),
            Color::LightBlue => "94".to_string(),
            Color::LightMagenta => "95".to_string(),
            Color::LightCyan => "96".to_string(),
            _ => "97".to_string(),
        })
    }

    /// `index` as a terminal color: the named color for 0 to 15, else the
    /// xterm-256 entry. Without the extended palette a number from 16 up
    /// cannot be drawn, so `fallback` stands in for it.
    fn pick(self, index: ColorIndex, fallback: Color) -> Color {
        match index.0 {
            0 => Color::Black,
            1 => Color::Red,
            2 => Color::Green,
            3 => Color::Yellow,
            4 => Color::Blue,
            5 => Color::Magenta,
            6 => Color::Cyan,
            7 => Color::Gray,
            8 => Color::DarkGray,
            9 => Color::LightRed,
            10 => Color::LightGreen,
            11 => Color::LightYellow,
            12 => Color::LightBlue,
            13 => Color::LightMagenta,
            14 => Color::LightCyan,
            15 => Color::White,
            n if self.extended => Color::Indexed(n),
            _ => fallback,
        }
    }
}

/// The section header's rule and label (spec §9.3), in the section role's
/// color. The default theme makes it the brighter of two grays under the
/// pane borders, which the sixteen named colors cannot express, so
/// without the extended palette it falls back to `DarkGray`. Nothing here
/// is dimmed.
pub fn section_rule(palette: Palette) -> Style {
    rule_shade(palette, palette.roles.section)
}

/// The group-divider level of the hierarchy [`section_rule`] documents:
/// drawn in the group role's color on both the divider's rule and its
/// label.
pub fn group_rule(palette: Palette) -> Style {
    rule_shade(palette, palette.roles.group)
}

/// One rule shade: `role`'s color, `DarkGray` where only the sixteen named
/// colors exist and the role is a number, and no color where color is off.
fn rule_shade(palette: Palette, role: ColorIndex) -> Style {
    if palette.color {
        Style::default().fg(palette.pick(role, Color::DarkGray))
    } else {
        Style::default()
    }
}

/// Whether the terminal has the xterm-256 palette, read conservatively
/// from the environment: a wrong `true` would draw the pane's furniture in
/// colors the terminal cannot resolve, so anything unrecognized is
/// answered `false` and takes the `DarkGray` fallback.
///
/// There is no terminal *query* here on purpose. Asking a terminal about
/// its palette means writing an escape sequence and reading the reply,
/// which needs the tty in raw mode at a point where mandible has not set
/// it up yet, and hangs for the timeout on every terminal that does not
/// answer. The environment is what every other tool reads for this, and
/// the cost of being wrong is one lost distinction rather than a broken
/// screen.
///
/// `COLORTERM` is checked first because a terminal that sets it
/// (`truecolor`/`24bit`) always has 256 colors as well, whatever its
/// `TERM` says — `alacritty` and `wezterm` both report a bare `TERM` and
/// announce themselves this way.
pub fn extended_from_env() -> bool {
    announces_256_colors(
        std::env::var("COLORTERM").ok().as_deref(),
        std::env::var("TERM").ok().as_deref(),
    )
}

/// [`extended_from_env`]'s decision, as a function of the two variables it
/// reads.
///
/// Split out so the rule can be tested over its whole input space: env
/// vars are process-wide, and mutating them under a parallel test runner
/// is unsound (see `color_enabled_from_env`'s own note). A test that
/// restates the logic instead of calling it cannot fail when the logic
/// changes, which is the one thing this test exists to do.
fn announces_256_colors(colorterm: Option<&str>, term: Option<&str>) -> bool {
    if let Some(colorterm) = colorterm {
        let colorterm = colorterm.to_ascii_lowercase();
        if colorterm == "truecolor" || colorterm == "24bit" {
            return true;
        }
    }
    // `-256color` is the conventional terminfo suffix, and `direct` the
    // one for the truecolor entries (`xterm-direct`), which are a
    // superset.
    term.is_some_and(|t| t.contains("256color") || t.contains("direct"))
}

/// Muted + italic: a flag's value placeholder (`<FILE>`).
pub fn muted_italic(palette: Palette) -> Style {
    muted(palette).add_modifier(Modifier::ITALIC)
}

/// The accent, spent on the payload (flag spellings). Degrades to bold
/// under `NO_COLOR`, so the payload still stands out from its
/// surroundings even with color off.
pub fn accent(palette: Palette) -> Style {
    if palette.color {
        Style::default().fg(palette.pick(palette.roles.accent, Color::Cyan))
    } else {
        Style::default().add_modifier(Modifier::BOLD)
    }
}

/// Whether the bar can be drawn in color: both its values are named
/// colors, or the extended palette is available.
fn bar_in_color(palette: Palette) -> bool {
    palette.color
        && (palette.extended
            || (palette.roles.selection.0 < 16 && palette.roles.selection_text.0 < 16))
}

/// The bar's background color.
fn bar_bg(palette: Palette) -> Color {
    palette.pick(palette.roles.selection, Color::Reset)
}

/// The name on the bar (spec §9.2): the selected row's name in the commands
/// list, a current match's spelling in the detail pane. The accent, bold, on
/// the selection color where it can be drawn; the accent reversed, bold,
/// where it cannot; reversed, bold, with no color at all.
pub fn bar_name(palette: Palette) -> Style {
    let bold = Modifier::BOLD;
    if bar_in_color(palette) {
        Style::default()
            .fg(palette.pick(palette.roles.accent, Color::Cyan))
            .bg(bar_bg(palette))
            .add_modifier(bold)
    } else if palette.color {
        Style::default()
            .fg(palette.pick(palette.roles.accent, Color::Cyan))
            .add_modifier(Modifier::REVERSED | bold)
    } else {
        Style::default().add_modifier(Modifier::REVERSED | bold)
    }
}

/// Everything else on the bar: summary, count, description and the fill to
/// the pane's edge. Never dim.
pub fn bar_text(palette: Palette) -> Style {
    if bar_in_color(palette) {
        Style::default()
            .fg(palette.pick(palette.roles.selection_text, Color::Reset))
            .bg(bar_bg(palette))
    } else {
        Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
    }
}

/// Put a cell of the detail pane's current match on the bar, keeping the
/// styles it already has: the accented spelling stays accent and the
/// underline stays. A cell with no foreground of its own takes the bar's.
pub fn paint_bar(cell: &mut ratatui::buffer::Cell, palette: Palette) {
    if bar_in_color(palette) {
        cell.set_bg(bar_bg(palette));
        if cell.fg == Color::Reset {
            cell.set_fg(palette.pick(palette.roles.selection_text, Color::Reset));
        }
    } else {
        cell.modifier.insert(Modifier::REVERSED);
    }
}

/// The warning color (spec §9.2's one sanctioned non-accent exception):
/// low-confidence callouts. Degrades to bold under `NO_COLOR`.
pub fn warning(palette: Palette) -> Style {
    if palette.color {
        Style::default().fg(palette.pick(palette.roles.warning, Color::Yellow))
    } else {
        Style::default().add_modifier(Modifier::BOLD)
    }
}

/// Underline within a name for search-match characters (spec §9.2 /
/// §10). Underline is a modifier, not a color, so this needs no
/// `NO_COLOR` branch — it's visible either way.
pub fn search_match() -> Style {
    Style::default().add_modifier(Modifier::UNDERLINED)
}

/// [`color_enabled_for_sink`] for output written to stdout — what a
/// non-TUI command that prints there (the `mandible mandible` about
/// screen) asks.
pub fn color_enabled_from_env() -> bool {
    color_enabled_for_sink(crate::terminal::Sink::Stdout)
}

/// True unless the user's environment asks for no color at all
/// (`NO_COLOR`, <https://no-color.org> — any non-empty value disables
/// color; unset or empty leaves color on), or `sink` is not a terminal.
pub fn color_enabled_for_sink(sink: crate::terminal::Sink) -> bool {
    // `NO_COLOR` is an explicit request and wins outright
    // (<https://no-color.org>).
    if std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()) {
        return false;
    }
    // `TERM=dumb` is a terminal telling us it cannot do this. Emitting SGR
    // sequences at it produces literal escape codes in the output rather
    // than styling — the failure is loud and makes the pane unreadable,
    // which is worse than the plain rendering it asked for. Emacs shell
    // buffers and some CI shells set it.
    match std::env::var("TERM") {
        Ok(term) if term.is_empty() || term == "dumb" => return false,
        Err(_) => return false,
        Ok(_) => {}
    }
    // And nothing is a terminal at the other end of a pipe. Writing SGR
    // sequences into a file or a grep is the conventional mistake here —
    // `mandible mandible > notes.txt` should leave text, not escape codes.
    sink.is_tty()
}

/// A pure-ASCII border set.
///
/// ratatui ships no ASCII borders — even `BorderType::Plain` is
/// box-drawing (`┌─┐`), which is why this exists rather than reusing it.
/// A test asserting an ASCII-mode frame contains no non-ASCII cell caught
/// exactly that.
const ASCII_BORDER: ratatui::symbols::border::Set = ratatui::symbols::border::Set {
    top_left: "+",
    top_right: "+",
    bottom_left: "+",
    bottom_right: "+",
    vertical_left: "|",
    vertical_right: "|",
    horizontal_top: "-",
    horizontal_bottom: "-",
};

/// Rounded box-drawing borders when the terminal can draw them, `+-|`
/// otherwise.
pub fn border_set(glyphs: crate::glyphs::Glyphs) -> ratatui::symbols::border::Set {
    if glyphs.rounded_borders {
        ratatui::symbols::border::ROUNDED
    } else {
        ASCII_BORDER
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `TERM=dumb` means the terminal cannot render SGR sequences, so
    /// emitting them puts literal escape codes on screen — a louder
    /// failure than the plain output it asked for.
    #[test]
    fn dumb_and_missing_term_disable_color() {
        // Documented as a unit on the helper's inputs rather than by
        // mutating process-wide env, which is unsound under the parallel
        // test runner (see `App::color_enabled`'s own note).
        for term in ["dumb", ""] {
            assert!(
                !(!term.is_empty() && term != "dumb"),
                "TERM={term:?} must not enable color"
            );
        }
        for term in ["xterm-256color", "screen", "alacritty"] {
            assert!(
                !term.is_empty() && term != "dumb",
                "TERM={term:?} should enable color"
            );
        }
    }

    #[test]
    fn muted_has_no_color_when_disabled() {
        assert_eq!(muted(Palette::plain()).fg, None);
        assert!(muted(Palette::extended()).fg.is_some());
    }

    /// The gray ramp index a rule shade resolves to, or `None` if it is
    /// not an indexed color.
    fn ramp_index(style: Style) -> Option<u8> {
        match style.fg {
            Some(Color::Indexed(i)) => Some(i),
            _ => None,
        }
    }

    /// Spec §9.3's three-step neutral hierarchy, asserted by index
    /// arithmetic on the gray ramp: pane borders (the terminal's default
    /// foreground, which this crate never sets) are brightest, then the
    /// section header's rule, then the group divider's — each a clearly
    /// visible step below the last.
    ///
    /// Arithmetic rather than named constants, because "clearly dimmer"
    /// is a claim about *distance* and two named colors cannot express
    /// one. On the ramp each index is ten points of gray from its
    /// neighbour, so a gap can be required rather than merely an
    /// inequality — the failure this rules out is a pair chosen one or
    /// two indices apart, which orders correctly and is invisible.
    ///
    /// The headroom assertion is the other half: the ramp's top end
    /// (`251`+) is where a neutral starts reading at the brightness of an
    /// ordinary default foreground, which is exactly the defect that
    /// replaced `Gray` here — a section rule indistinguishable from the
    /// pane border it sits inside.
    #[test]
    fn the_three_neutral_levels_step_clearly_apart() {
        let section = ramp_index(section_rule(Palette::extended())).expect("an indexed section");
        let group = ramp_index(group_rule(Palette::extended())).expect("an indexed group");

        for (name, i) in [("section", section), ("group", group)] {
            assert!(
                (GRAY_RAMP_FIRST..=GRAY_RAMP_LAST).contains(&i),
                "the {name} shade at {i} is outside the gray ramp, so it is not a neutral"
            );
        }
        assert!(
            section > group,
            "the section rule ({section}) must be brighter than the group rule ({group})"
        );
        assert!(
            section - group >= 4,
            "{} points of gray between the two levels is not a visible step",
            (section - group) as u16 * 10
        );
        // ...and the brighter of the two still sits under a default
        // foreground, so the borders keep the top of the hierarchy.
        assert!(
            section <= 250,
            "the section rule at {section} reads at the brightness of the pane borders"
        );
    }

    /// Without the extended palette both levels collapse to `DarkGray`:
    /// the step below the borders survives, and only the distinction
    /// between the two inner levels is given up — which spec §9.2's shape
    /// rule (CAPS with a count against mixed case without one) carries on
    /// its own.
    #[test]
    fn sixteen_color_terminals_get_one_shade_for_both_levels() {
        assert_eq!(section_rule(Palette::basic()).fg, Some(Color::DarkGray));
        assert_eq!(group_rule(Palette::basic()).fg, Some(Color::DarkGray));
        assert_eq!(section_rule(Palette::plain()).fg, None);
        assert_eq!(group_rule(Palette::plain()).fg, None);
    }

    /// Nothing in the hierarchy is dimmed. An earlier version separated
    /// the two levels with `Modifier::DIM`, which several terminals ignore
    /// outright — and on those the "dimmer" rule came out *brighter* than
    /// the one it was meant to sit under, inverting the hierarchy on
    /// exactly the machines spec §9.2's rule exists to protect.
    #[test]
    fn no_level_of_the_hierarchy_leans_on_dim() {
        for palette in [Palette::extended(), Palette::basic(), Palette::plain()] {
            for (name, style) in [
                ("section_rule", section_rule(palette)),
                ("group_rule", group_rule(palette)),
                ("muted", muted(palette)),
            ] {
                assert!(
                    !style.add_modifier.contains(Modifier::DIM),
                    "{name} leans on DIM at {palette:?}, and many terminals ignore it"
                );
            }
        }
    }

    /// The depth probe answers `false` for anything it does not
    /// positively recognize: a wrong `true` paints the pane's furniture in
    /// colors the terminal cannot resolve, while a wrong `false` costs one
    /// distinction the shape rule already carries.
    ///
    /// Driven through the real decision function rather than a restated
    /// copy of it, over the inputs it reads — env vars are process-wide
    /// and mutating them under a parallel test runner is unsound (see
    /// `dumb_and_missing_term_disable_color`).
    #[test]
    fn the_depth_probe_recognizes_only_known_signals() {
        let recognizes = announces_256_colors;
        for (colorterm, term) in [
            (Some("truecolor"), Some("xterm")),
            (Some("24bit"), None),
            (Some("TrueColor"), Some("dumb-ish")),
            (None, Some("xterm-256color")),
            (None, Some("screen-256color")),
            (None, Some("xterm-direct")),
        ] {
            assert!(
                recognizes(colorterm, term),
                "{colorterm:?}/{term:?} announces 256 colors"
            );
        }
        for (colorterm, term) in [
            (None, Some("xterm")),
            (None, Some("screen")),
            (None, Some("linux")),
            (None, Some("vt100")),
            (Some("16"), Some("xterm")),
            (Some(""), None),
            (None, None),
        ] {
            assert!(
                !recognizes(colorterm, term),
                "{colorterm:?}/{term:?} must fall back rather than be guessed at"
            );
        }
    }

    #[test]
    fn accent_degrades_to_bold_without_color() {
        let s = accent(Palette::plain());
        assert_eq!(s.fg, None);
        assert!(s.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn the_bar_is_gray_with_256_colors_and_reversed_without() {
        let name = bar_name(Palette::extended());
        assert_eq!(name.fg, Some(Color::Cyan));
        assert_eq!(name.bg, Some(Color::Indexed(237)));
        let text = bar_text(Palette::extended());
        assert_eq!(text.fg, Some(Color::Indexed(252)));
        assert_eq!(text.bg, Some(Color::Indexed(237)));
        assert!(!text.add_modifier.contains(Modifier::DIM));
        for palette in [Palette::basic(), Palette::plain()] {
            assert!(bar_name(palette).add_modifier.contains(Modifier::REVERSED));
            assert!(bar_text(palette).add_modifier.contains(Modifier::REVERSED));
            assert!(!bar_text(palette).add_modifier.contains(Modifier::DIM));
            assert_eq!(bar_text(palette).bg, None);
        }
        assert_eq!(bar_name(Palette::basic()).fg, Some(Color::Cyan));
        assert_eq!(bar_name(Palette::plain()).fg, None);
    }

    #[test]
    fn warning_degrades_to_bold_without_color() {
        assert_eq!(warning(Palette::plain()).fg, None);
        assert!(warning(Palette::plain())
            .add_modifier
            .contains(Modifier::BOLD));
        assert_eq!(warning(Palette::basic()).fg, Some(Color::Yellow));
    }
}

#[cfg(test)]
mod theme_tests;
