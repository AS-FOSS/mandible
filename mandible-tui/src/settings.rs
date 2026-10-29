//! The settings tab of the `?` pop-up: its rows, the values each row
//! steps through, and the state the tab keeps (docs/design.md §9.5).

use crate::app::App;
use crate::glyphs::Glyphs;
use crate::sort::DetailSort;
use crate::style::Palette;
use mandible_core::config::{CommandSort, FlagSort, Key, Theme, Value};
use std::ffi::OsStr;
use std::path::PathBuf;

/// The two tabs of the `?` pop-up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HelpTab {
    /// The key bindings.
    #[default]
    Keys,
    /// The settings.
    Settings,
}

/// The headings the rows sit under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    /// How pages are shown.
    View,
    /// How panes are ordered.
    Sort,
    /// Colours.
    Colours,
}

impl Group {
    /// The heading text.
    pub fn heading(self) -> &'static str {
        match self {
            Group::View => "VIEW",
            Group::Sort => "SORT",
            Group::Colours => "COLOURS",
        }
    }
}

/// One setting in the tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    /// Start in the tool's own `--help`.
    RawView,
    /// Lines scroll sideways instead of folding.
    Scroll,
    /// Preview a long description.
    Preview,
    /// The ASCII glyph set.
    Ascii,
    /// The detail pane's sort.
    Flags,
    /// The commands pane's sort.
    Commands,
    /// The colour theme.
    Theme,
}

impl Row {
    /// Every row, top to bottom.
    pub const ALL: [Row; 7] = [
        Row::RawView,
        Row::Scroll,
        Row::Preview,
        Row::Ascii,
        Row::Flags,
        Row::Commands,
        Row::Theme,
    ];

    /// The heading the row sits under.
    pub fn group(self) -> Group {
        match self {
            Row::RawView | Row::Scroll | Row::Preview | Row::Ascii => Group::View,
            Row::Flags | Row::Commands => Group::Sort,
            Row::Theme => Group::Colours,
        }
    }

    /// The label column's text.
    pub fn label(self, glyphs: Glyphs) -> String {
        match self {
            Row::RawView => format!("Open in the tool{}s own --help", glyphs.apostrophe),
            Row::Scroll => "Scroll long lines sideways".to_string(),
            Row::Preview => "Description preview".to_string(),
            Row::Ascii => "ASCII glyphs".to_string(),
            Row::Flags => "Flags".to_string(),
            Row::Commands => "Commands".to_string(),
            Row::Theme => "Theme".to_string(),
        }
    }

    /// The `config.toml` key the row writes.
    pub fn key(self) -> Key {
        match self {
            Row::RawView => Key::RawView,
            Row::Scroll => Key::HorizontalScroll,
            Row::Preview => Key::DescriptionPreview,
            Row::Ascii => Key::Ascii,
            Row::Flags => Key::SortFlags,
            Row::Commands => Key::SortCommands,
            Row::Theme => Key::Theme,
        }
    }
}

/// The environment variables that outrank the file, read once at startup.
/// Each locks the row it decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EnvLocks {
    /// `MANDIBLE_ASCII` is set.
    pub ascii: bool,
    /// What turns colour off: `NO_COLOR`, or `TERM` being `dumb`.
    pub color: Option<&'static str>,
}

impl EnvLocks {
    /// The locks this process's environment sets.
    pub fn from_env() -> EnvLocks {
        EnvLocks::of(
            std::env::var_os("NO_COLOR").as_deref(),
            std::env::var("TERM").ok().as_deref(),
            std::env::var_os("MANDIBLE_ASCII").as_deref(),
        )
    }

    /// [`EnvLocks::from_env`] as a function of the three variables, so the
    /// rule is testable without touching the process environment.
    pub fn of(no_color: Option<&OsStr>, term: Option<&str>, ascii: Option<&OsStr>) -> EnvLocks {
        let color = if no_color.is_some_and(|v| !v.is_empty()) {
            Some("NO_COLOR")
        } else {
            match term {
                Some("dumb") => Some("TERM=dumb"),
                None | Some("") => Some("TERM"),
                Some(_) => None,
            }
        };
        EnvLocks {
            ascii: ascii.is_some_and(|v| !v.is_empty()),
            color,
        }
    }

    /// The variable that decides `row`, if one does.
    pub fn variable(&self, row: Row) -> Option<&'static str> {
        match row {
            Row::Ascii if self.ascii => Some("MANDIBLE_ASCII"),
            Row::Theme => self.color,
            _ => None,
        }
    }
}

/// Everything the settings tab keeps beside the view state it changes.
#[derive(Debug)]
pub struct Settings {
    /// A long description previews.
    pub description_preview: bool,
    /// The ASCII glyph set is forced.
    pub ascii: bool,
    /// The colour theme.
    pub theme: Theme,
    /// What the environment locks.
    pub locks: EnvLocks,
    /// Where changes are saved; `None` saves nowhere.
    pub config_path: Option<PathBuf>,
    /// The tab the pop-up shows.
    pub tab: HelpTab,
    /// The highlighted row, an index into [`Row::ALL`].
    pub row: usize,
    /// The first content line shown, kept by the renderer.
    pub scroll: std::cell::Cell<usize>,
    /// What the `default` theme yields.
    pub palette_base: Palette,
    /// The glyph set when `ascii` is off.
    pub glyphs_base: Glyphs,
}

impl Settings {
    /// The settings of a fresh app: every default, nothing locked or saved.
    pub fn new(palette: Palette, glyphs: Glyphs) -> Settings {
        Settings {
            description_preview: true,
            ascii: false,
            theme: Theme::Default,
            locks: EnvLocks::default(),
            config_path: None,
            tab: HelpTab::Keys,
            row: 0,
            scroll: std::cell::Cell::new(0),
            palette_base: palette,
            glyphs_base: glyphs,
        }
    }

    /// The highlighted row.
    pub fn selected(&self) -> Row {
        Row::ALL[self.row.min(Row::ALL.len() - 1)]
    }
}

/// The detail pane's sort state a config word stands for.
pub fn detail_sort(flags: FlagSort) -> DetailSort {
    match flags {
        FlagSort::Tool => DetailSort::Printed,
        FlagSort::AzSections => DetailSort::Sections,
        FlagSort::Az => DetailSort::Overall,
    }
}

/// The config word for a detail pane sort state.
pub fn flag_sort(sort: DetailSort) -> FlagSort {
    match sort {
        DetailSort::Printed => FlagSort::Tool,
        DetailSort::Sections => FlagSort::AzSections,
        DetailSort::Overall => FlagSort::Az,
    }
}

/// The value `row` shows now, as the text before any variable name.
pub fn value_text(app: &App, row: Row) -> String {
    let on_off = |on: bool| if on { "on" } else { "off" }.to_string();
    if app.settings.locks.variable(row).is_some() {
        return match row {
            Row::Ascii => "on".to_string(),
            _ => "none".to_string(),
        };
    }
    match row {
        Row::RawView => on_off(app.raw_mode),
        Row::Scroll => on_off(app.sideways()),
        Row::Preview => on_off(app.settings.description_preview),
        Row::Ascii => on_off(app.settings.ascii),
        Row::Flags => match app.sorts.detail {
            DetailSort::Printed => "tool order".to_string(),
            DetailSort::Sections => format!("{} in sections", app.glyphs.a_to_z),
            DetailSort::Overall => app.glyphs.a_to_z.to_string(),
        },
        Row::Commands if app.sorts.tree => app.glyphs.a_to_z.to_string(),
        Row::Commands => "tool order".to_string(),
        Row::Theme => app.settings.theme.word().to_string(),
    }
}

/// The write that records `row`'s current state.
pub fn current_value(app: &App, row: Row) -> Value {
    match row {
        Row::RawView => Value::Bool(app.raw_mode),
        Row::Scroll => Value::Bool(app.sideways()),
        Row::Preview => Value::Bool(app.settings.description_preview),
        Row::Ascii => Value::Bool(app.settings.ascii),
        Row::Flags => Value::Word(flag_sort(app.sorts.detail).word()),
        Row::Commands => Value::Word(if app.sorts.tree {
            CommandSort::Az.word()
        } else {
            CommandSort::Tool.word()
        }),
        Row::Theme => Value::Word(app.settings.theme.word()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_color_and_a_dumb_terminal_lock_the_theme() {
        let os = |s: &'static str| Some(OsStr::new(s));
        assert_eq!(
            EnvLocks::of(os("1"), Some("xterm"), None).color,
            Some("NO_COLOR")
        );
        assert_eq!(EnvLocks::of(os(""), Some("xterm"), None).color, None);
        assert_eq!(
            EnvLocks::of(None, Some("dumb"), None).color,
            Some("TERM=dumb")
        );
        assert_eq!(EnvLocks::of(None, None, None).color, Some("TERM"));
        assert_eq!(EnvLocks::of(None, Some("xterm-256color"), None).color, None);
    }

    #[test]
    fn mandible_ascii_locks_the_glyph_row_only() {
        let locks = EnvLocks::of(None, Some("xterm"), Some(OsStr::new("1")));
        assert_eq!(locks.variable(Row::Ascii), Some("MANDIBLE_ASCII"));
        assert_eq!(locks.variable(Row::Theme), None);
        assert_eq!(locks.variable(Row::Flags), None);
        assert!(!EnvLocks::of(None, Some("xterm"), Some(OsStr::new(""))).ascii);
    }

    #[test]
    fn the_sort_words_map_both_ways() {
        for flags in FlagSort::ALL {
            assert_eq!(flag_sort(detail_sort(*flags)), *flags);
        }
    }
}
