//! The user's general settings file, `~/.config/mandible/config.toml`.
//!
//! mandible reads it at startup and never creates it. The settings tab of
//! the `?` pop-up writes single keys back with [`save`]. The directory is
//! [`CONFIG_DIR_ENV`] when set, else the per-OS default. Precedence is
//! environment variable, then file, then default; the terminal UI applies
//! the environment half.
//!
//! A key that is unknown or has a bad value costs only that key. A file
//! that does not parse gives the defaults. Each problem is returned in
//! [`Loaded::problems`] and logged with `tracing::warn!`. Nothing here
//! panics, because it runs before the user has typed anything.

mod colors;
mod save;

pub use colors::{ColorIndex, Role, Roles, NAMES as COLOR_NAMES};
pub use save::{save, SaveError};

use std::path::PathBuf;

/// Environment variable that overrides the config directory outright.
///
/// The override tier in `mandible-extract` reads the same variable, so the
/// two readers cannot resolve different directories.
pub const CONFIG_DIR_ENV: &str = "MANDIBLE_CONFIG_DIR";

/// A setting whose value is one of a fixed set of words in the file.
macro_rules! word_setting {
    ($(#[$meta:meta])* $name:ident { $($(#[$vmeta:meta])* $variant:ident = $word:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
        pub enum $name {
            $($(#[$vmeta])* $variant),+
        }

        impl $name {
            /// Every value, in the order the settings tab steps through them.
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            /// The word this value is written as in `config.toml`.
            pub fn word(self) -> &'static str {
                match self {
                    $($name::$variant => $word),+
                }
            }

            fn from_word(word: &str) -> Option<$name> {
                match word {
                    $($word => Some($name::$variant),)+
                    _ => None,
                }
            }
        }
    };
}

word_setting! {
    /// How the detail pane starts out ordering a page's flag sections.
    FlagSort {
        /// The order the tool printed them in.
        #[default]
        Tool = "tool",
        /// A to Z inside each group; the group headings stay.
        AzSections = "az-sections",
        /// A to Z over the whole section.
        Az = "az",
    }
}

word_setting! {
    /// How the commands pane starts out ordering its rows.
    CommandSort {
        /// The order the tool printed them in.
        #[default]
        Tool = "tool",
        /// A to Z.
        Az = "az",
    }
}

word_setting! {
    /// The colour theme. A theme is a value that yields the palette.
    Theme {
        /// Cyan accent on the terminal's own colours.
        #[default]
        Default = "default",
        /// For a light terminal.
        Light = "light",
        /// Brighter text and a stronger bar.
        HighContrast = "high-contrast",
        /// Warm greens and yellows.
        GruvboxStyle = "gruvbox-style",
        /// Cool blues.
        NordStyle = "nord-style",
        /// Purple accent.
        DraculaStyle = "dracula-style",
        /// Pastel accent.
        CatppuccinStyle = "catppuccin-style",
        /// The seven values `[colors]` gives.
        Custom = "custom",
        /// No colour at all, exactly what `NO_COLOR` gives.
        None = "none",
    }
}

impl Theme {
    /// The seven values of a built-in theme. `custom` has the user's own
    /// and `none` has no colours, so both give `None`.
    pub fn roles(self) -> Option<Roles> {
        match self {
            Theme::Default => Some(Roles::DEFAULT),
            Theme::Light => Some(Roles::LIGHT),
            Theme::HighContrast => Some(Roles::HIGH_CONTRAST),
            Theme::GruvboxStyle => Some(Roles::GRUVBOX),
            Theme::NordStyle => Some(Roles::NORD),
            Theme::DraculaStyle => Some(Roles::DRACULA),
            Theme::CatppuccinStyle => Some(Roles::CATPPUCCIN),
            Theme::Custom | Theme::None => None,
        }
    }
}

/// `[ui]`: what the interface starts out doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiConfig {
    /// Start in the tool's own `--help` view, as if `t` had been pressed.
    pub raw_view: bool,
    /// Lines stay one row and scroll sideways (`true`), or fold to the pane
    /// (`false`). One setting for the parsed view and the raw view.
    pub horizontal_scroll: bool,
    /// Preview a long DESCRIPTION. Off draws it in full, in place.
    pub description_preview: bool,
    /// Force the ASCII glyph set.
    pub ascii: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        UiConfig {
            raw_view: false,
            horizontal_scroll: false,
            description_preview: true,
            ascii: false,
        }
    }
}

/// `[sort]`: the starting states of `s` in each pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SortConfig {
    /// The detail pane.
    pub flags: FlagSort,
    /// The commands pane.
    pub commands: CommandSort,
}

/// `[colors]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ColorsConfig {
    /// The colour theme.
    pub theme: Theme,
    /// The seven values `custom` uses; a key the file lacks keeps the
    /// `default` theme's value.
    pub custom: Roles,
}

/// mandible's general settings, as `config.toml` states them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Config {
    /// `[ui]`.
    pub ui: UiConfig,
    /// `[sort]`.
    pub sort: SortConfig,
    /// `[colors]`.
    pub colors: ColorsConfig,
}

/// One thing wrong with `config.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// The 1-based line, when the file says where.
    pub line: Option<usize>,
    /// What is wrong and what was done about it, e.g.
    /// `unknown key "srot", ignored`.
    pub text: String,
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.line {
            Some(line) => write!(f, "line {line}: {}", self.text),
            None => f.write_str(&self.text),
        }
    }
}

/// What reading `config.toml` produced.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Loaded {
    /// The settings, with a default wherever the file gave none.
    pub config: Config,
    /// Everything wrong with the file, in file order.
    pub problems: Vec<Problem>,
}

impl Loaded {
    /// The one footer line for the startup problems, if there are any.
    pub fn notice(&self) -> Option<String> {
        let first = self.problems.first()?;
        Some(match self.problems.len() {
            1 => format!("config.toml {first}"),
            n => format!("{n} problems in config.toml: {first}"),
        })
    }
}

/// The config directory: `$MANDIBLE_CONFIG_DIR` if set and non-empty, else
/// the per-OS directory `directories::ProjectDirs` resolves.
fn config_dir() -> Option<PathBuf> {
    match std::env::var_os(CONFIG_DIR_ENV) {
        Some(dir) if !dir.is_empty() => Some(PathBuf::from(dir)),
        _ => directories::ProjectDirs::from("", "", "mandible")
            .map(|dirs| dirs.config_dir().to_path_buf()),
    }
}

/// `config.toml`'s path, if a config directory could be resolved at all.
pub fn config_path() -> Option<PathBuf> {
    config_dir().map(|dir| dir.join("config.toml"))
}

/// Read the user's `config.toml`. A missing file, directory or table is
/// the ordinary case and gives the defaults without a problem.
pub fn load() -> Loaded {
    let Some(path) = config_path() else {
        return Loaded::default();
    };
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Loaded::default(),
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "mandible config.toml unreadable; using defaults");
            return Loaded {
                config: Config::default(),
                problems: vec![Problem {
                    line: None,
                    text: "could not be read, defaults used".to_string(),
                }],
            };
        }
    };
    let loaded = parse(&raw);
    for problem in &loaded.problems {
        tracing::warn!(path = %path.display(), %problem, "mandible config.toml");
    }
    loaded
}

/// The 1-based line holding byte `offset` of `raw`.
fn line_at(raw: &str, offset: usize) -> usize {
    let head = raw.get(..offset.min(raw.len())).unwrap_or_default();
    head.bytes().filter(|&b| b == b'\n').count() + 1
}

/// Read `raw` as the contents of `config.toml`.
pub fn parse(raw: &str) -> Loaded {
    let doc = match toml_edit::ImDocument::parse(raw) {
        Ok(doc) => doc,
        Err(error) => {
            return Loaded {
                config: Config::default(),
                problems: vec![Problem {
                    line: error.span().map(|span| line_at(raw, span.start)),
                    text: "not valid TOML, defaults used".to_string(),
                }],
            }
        }
    };
    let mut out = Loaded::default();
    let root = doc.as_table();
    for (name, item) in root.iter() {
        let line = root
            .get_key_value(name)
            .and_then(|(key, _)| key.span())
            .or_else(|| item.span())
            .map(|span| line_at(raw, span.start));
        let problem = |text: String| Problem { line, text };
        let Some(table) = item.as_table_like() else {
            let known = ["ui", "sort", "colors"].contains(&name);
            out.problems.push(problem(if known {
                format!("bad value for {name}, ignored")
            } else {
                format!("unknown key \"{name}\", ignored")
            }));
            continue;
        };
        if !["ui", "sort", "colors"].contains(&name) {
            out.problems
                .push(problem(format!("unknown section \"{name}\", ignored")));
            continue;
        }
        for (key, value) in table.iter() {
            let line = table
                .get_key_value(key)
                .and_then(|(k, _)| k.span())
                .or_else(|| value.span())
                .map(|span| line_at(raw, span.start));
            if !apply(&mut out.config, name, key, value) {
                let text = if known_key(name, key) {
                    format!("bad value for {key}, ignored")
                } else {
                    format!("unknown key \"{key}\", ignored")
                };
                out.problems.push(Problem { line, text });
            }
        }
    }
    out
}

/// Whether `key` is a key of `section`.
fn known_key(section: &str, key: &str) -> bool {
    matches!(
        (section, key),
        (
            "ui",
            "raw_view" | "horizontal_scroll" | "description_preview" | "ascii"
        ) | ("sort", "flags" | "commands")
            | ("colors", "theme")
    ) || (section == "colors" && Role::ALL.iter().any(|r| r.key() == key))
}

/// Set one key from its TOML value. False when the key is unknown or the
/// value is not one it accepts.
fn apply(config: &mut Config, section: &str, key: &str, item: &toml_edit::Item) -> bool {
    let flag = item.as_bool();
    let word = item.as_str();
    match (section, key) {
        ("ui", "raw_view") => set(&mut config.ui.raw_view, flag),
        ("ui", "horizontal_scroll") => set(&mut config.ui.horizontal_scroll, flag),
        ("ui", "description_preview") => set(&mut config.ui.description_preview, flag),
        ("ui", "ascii") => set(&mut config.ui.ascii, flag),
        ("sort", "flags") => set(&mut config.sort.flags, word.and_then(FlagSort::from_word)),
        ("sort", "commands") => set(
            &mut config.sort.commands,
            word.and_then(CommandSort::from_word),
        ),
        ("colors", "theme") => set(&mut config.colors.theme, word.and_then(Theme::from_word)),
        ("colors", role) => match Role::ALL.iter().find(|r| r.key() == role) {
            Some(r) => match color_of(item) {
                Some(color) => {
                    config.colors.custom.set(*r, color);
                    true
                }
                None => false,
            },
            None => false,
        },
        _ => false,
    }
}

/// A colour as the file writes it: a name for 0 to 15, or a number 0 to 255.
fn color_of(item: &toml_edit::Item) -> Option<ColorIndex> {
    match item.as_integer() {
        Some(n) => u8::try_from(n).ok().map(ColorIndex),
        None => item.as_str().and_then(ColorIndex::from_name),
    }
}

fn set<T>(slot: &mut T, value: Option<T>) -> bool {
    match value {
        Some(value) => {
            *slot = value;
            true
        }
        None => false,
    }
}

/// The settings the settings tab writes, one per `config.toml` key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// `[ui] raw_view`.
    RawView,
    /// `[ui] horizontal_scroll`.
    HorizontalScroll,
    /// `[ui] description_preview`.
    DescriptionPreview,
    /// `[ui] ascii`.
    Ascii,
    /// `[sort] flags`.
    SortFlags,
    /// `[sort] commands`.
    SortCommands,
    /// `[colors] theme`.
    Theme,
    /// One of the seven `[colors]` values.
    Color(Role),
}

impl Key {
    /// The table the key lives in.
    pub fn section(self) -> &'static str {
        match self {
            Key::RawView | Key::HorizontalScroll | Key::DescriptionPreview | Key::Ascii => "ui",
            Key::SortFlags | Key::SortCommands => "sort",
            Key::Theme | Key::Color(_) => "colors",
        }
    }

    /// The key's name in `config.toml`.
    pub fn name(self) -> &'static str {
        match self {
            Key::RawView => "raw_view",
            Key::HorizontalScroll => "horizontal_scroll",
            Key::DescriptionPreview => "description_preview",
            Key::Ascii => "ascii",
            Key::SortFlags => "flags",
            Key::SortCommands => "commands",
            Key::Theme => "theme",
            Key::Color(role) => role.key(),
        }
    }
}

/// A value to write to `config.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Value {
    /// A boolean.
    Bool(bool),
    /// One of the words a word setting is written as.
    Word(&'static str),
    /// A colour 16 to 255, written as a number.
    Number(u8),
}

impl Value {
    /// How `color` is written: its name for 0 to 15, else its number.
    pub fn color(color: ColorIndex) -> Value {
        match color.name() {
            Some(name) => Value::Word(name),
            None => Value::Number(color.0),
        }
    }
}

#[cfg(test)]
mod tests;
