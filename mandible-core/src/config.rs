//! The user's general settings file, `~/.config/mandible/config.toml`.
//!
//! This is deliberately the *only* general config file mandible reads.
//! `mandible-extract/src/overrides/mod.rs` defines a different mechanism —
//! per-tool structure/prose replacement, keyed by tool name, one file per
//! tool under `overrides/<tool>.toml` — living beside this one in the same
//! config directory but serving a different job (spec §7 Tier F). This
//! module is for settings that apply to mandible itself, independent of
//! which tool is open.
//!
//! Resolution reuses exactly the escape hatch `overrides` already
//! establishes: [`CONFIG_DIR_ENV`] beats the per-OS default
//! (`directories::ProjectDirs`), because macOS ignores `XDG_CONFIG_HOME`
//! entirely and a second, independently-invented resolution path would be
//! one more place for that difference to go unnoticed.
//!
//! **Failure shape.** No config directory resolvable, no file present, no
//! `[ui]` table, or a key missing from it: all four are the ordinary
//! "this tier doesn't apply" case (`overrides::detect`'s shape) and fall
//! back to the default silently. Only a file that exists but fails to
//! *parse* is a user mistake worth surfacing — that is the one case where
//! the user's own intent is being discarded rather than simply absent — so
//! it gets one `tracing::warn!` (routed through `MANDIBLE_LOG` like
//! everything else in this project) and then the same default. Never a
//! panic: this runs on the startup path, before anything the user typed on
//! the command line has had a chance to be wrong.

use std::path::PathBuf;

/// Environment variable that overrides the config directory outright.
///
/// The same variable `mandible-extract`'s override tier uses — see that
/// module's doc comment for why one explicit variable beats a per-OS
/// mental model. Two independent config readers resolving the same
/// directory two different ways is exactly the kind of drift this project
/// has already been bitten by once; this module intentionally does not
/// redefine its own copy.
pub const CONFIG_DIR_ENV: &str = "MANDIBLE_CONFIG_DIR";

/// UI settings — currently just the detail pane's horizontal-scroll
/// behavior, but the natural home for any future terminal-rendering
/// preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiConfig {
    /// Whether preformatted detail-pane content (the raw `--help` view and
    /// USAGE-section synopsis lines) scrolls horizontally instead of
    /// wrapping. Default on; `false` restores exactly the pre-existing
    /// wrapping behavior, byte for byte.
    pub horizontal_scroll: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        UiConfig {
            horizontal_scroll: true,
        }
    }
}

/// mandible's general settings, as read from `config.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Config {
    /// Terminal-UI settings.
    pub ui: UiConfig,
}

#[derive(serde::Deserialize, Default)]
struct RawConfig {
    #[serde(default)]
    ui: RawUiConfig,
}

#[derive(serde::Deserialize, Default)]
struct RawUiConfig {
    horizontal_scroll: Option<bool>,
}

/// The config directory: `$MANDIBLE_CONFIG_DIR` if set and non-empty, else
/// the per-OS directory `directories::ProjectDirs` resolves
/// (`$XDG_CONFIG_HOME/mandible` on Linux), if a home directory could be
/// determined at all.
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

/// Load the user's config, falling back to [`Config::default`] whenever the
/// file doesn't (yet) express an opinion — see the module doc comment for
/// exactly which cases that covers. Never panics and never returns an
/// error: a config file is optional by nature, and the caller (startup)
/// has no user input yet to blame a failure on.
pub fn load() -> Config {
    let Some(path) = config_path() else {
        return Config::default();
    };
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Config::default();
    };
    match toml::from_str::<RawConfig>(&raw) {
        Ok(parsed) => Config {
            ui: UiConfig {
                horizontal_scroll: parsed.ui.horizontal_scroll.unwrap_or(true),
            },
        },
        Err(error) => {
            tracing::warn!(
                path = %path.display(),
                %error,
                "malformed mandible config.toml; using defaults"
            );
            Config::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Isolate one test's env mutation from the others — `std::env::set_var`
    /// is process-global and Rust's test runner is multi-threaded by
    /// default, so two tests touching `MANDIBLE_CONFIG_DIR` concurrently
    /// would race. A single mutex serializes just the tests in this module.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_config_dir<R>(dir: &std::path::Path, f: impl FnOnce() -> R) -> R {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY-equivalent: serialized by `ENV_LOCK` above, restored
        // before the guard drops.
        std::env::set_var(CONFIG_DIR_ENV, dir);
        let result = f();
        std::env::remove_var(CONFIG_DIR_ENV);
        result
    }

    #[test]
    fn missing_directory_falls_back_to_defaults() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var(CONFIG_DIR_ENV);
        // Can't assert much about the real per-OS directory (it may or may
        // not exist on the machine running this test), but `load` must
        // never panic either way.
        let _ = load();
    }

    #[test]
    fn missing_file_is_the_default() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = with_config_dir(dir.path(), load);
        assert_eq!(cfg, Config::default());
        assert!(cfg.ui.horizontal_scroll, "default is on");
    }

    #[test]
    fn empty_ui_table_is_the_default() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "[ui]\n").unwrap();
        let cfg = with_config_dir(dir.path(), load);
        assert!(cfg.ui.horizontal_scroll);
    }

    #[test]
    fn explicit_false_is_honored() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "[ui]\nhorizontal_scroll = false\n",
        )
        .unwrap();
        let cfg = with_config_dir(dir.path(), load);
        assert!(!cfg.ui.horizontal_scroll);
    }

    #[test]
    fn explicit_true_is_honored() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "[ui]\nhorizontal_scroll = true\n",
        )
        .unwrap();
        let cfg = with_config_dir(dir.path(), load);
        assert!(cfg.ui.horizontal_scroll);
    }

    /// Unknown keys, and unknown top-level tables, must not hard-fail —
    /// only a value that fails to *parse* is a user mistake.
    #[test]
    fn unknown_keys_do_not_break_parsing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.toml"),
            "[ui]\nhorizontal_scroll = true\nsome_future_key = 42\n\n[some_future_table]\nx = 1\n",
        )
        .unwrap();
        let cfg = with_config_dir(dir.path(), load);
        assert!(cfg.ui.horizontal_scroll);
    }

    /// A file that exists but fails to *parse* degrades to defaults rather
    /// than panicking or propagating an error up through startup.
    #[test]
    fn malformed_toml_falls_back_to_defaults_without_panicking() {
        let dir = tempfile::tempdir().unwrap();
        let mut f = std::fs::File::create(dir.path().join("config.toml")).unwrap();
        write!(f, "this is not valid toml [[[").unwrap();
        drop(f);
        let cfg = with_config_dir(dir.path(), load);
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn config_dir_env_is_honored_for_the_path_too() {
        let dir = tempfile::tempdir().unwrap();
        let path = with_config_dir(dir.path(), || config_path().unwrap());
        assert_eq!(path, dir.path().join("config.toml"));
    }
}
