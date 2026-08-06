//! Tier F: user overrides (spec §7 Tier F).
//!
//! `~/.config/mandible/overrides/<tool>.toml`, merged with
//! `Authority { structural: 255, prose: 255 }` — the highest of any tier,
//! so a user override always wins a merge conflict against every other
//! source (spec §4.4). This exists so the rare bad case (a tool this
//! project's general tiers genuinely can't parse well) has a clean exit.
//!
//! **Binding policy: overrides are user-local and must never be vendored
//! into this repository.** This single rule is what actually enforces the
//! project's no-per-tool-patches invariant (spec §1) — without it, the
//! first hard tool gets an override committed to git, and the per-tool
//! patch pile that the tiered architecture exists to prevent begins. This
//! module only ever reads from the user's own config directory
//! (`directories::ProjectDirs`, i.e. `$XDG_CONFIG_HOME/mandible/overrides`,
//! `~/.config/mandible/overrides` on a default Linux setup); it has no code
//! path that reads from, or writes to, anywhere inside this repository.
//!
//! The pipeline never depends on an override file existing: [`detect`]
//! simply returns `false` when there is none, exactly like any other tier
//! that doesn't apply to a given tool.
//!
//! No subprocess is ever spawned here — this tier reads one small local
//! file, so it needs none of spec §6's execution-safety machinery.

use crate::errors::ExtractError;
use crate::resolve::ResolvedTool;
use crate::tier::ExtractionTier;
use mandible_core::{Authority, CommandNode, Flag, Provenance, Source, Text, ValueKind};
use serde::Deserialize;
use std::path::PathBuf;

/// Tier F: applies a user's local override file, if one exists for the
/// requested tool.
#[derive(Debug, Default)]
pub struct OverridesTier;

impl ExtractionTier for OverridesTier {
    fn name(&self) -> &'static str {
        "overrides"
    }

    fn authority(&self) -> Authority {
        Source::UserOverride.authority()
    }

    fn detect(&self, tool: &ResolvedTool) -> bool {
        override_path(&tool.name).is_some_and(|p| p.is_file())
    }

    fn extract_node(
        &self,
        tool: &ResolvedTool,
        path: &[String],
    ) -> Result<CommandNode, ExtractError> {
        let file_path = override_path(&tool.name)
            .ok_or_else(|| ExtractError::Other("could not resolve a config directory".into()))?;
        let raw = std::fs::read_to_string(&file_path)
            .map_err(|e| ExtractError::Other(format!("reading {}: {e}", file_path.display())))?;
        let file: OverrideFile = toml::from_str(&raw)
            .map_err(|e| ExtractError::ParseFailed(format!("{}: {e}", file_path.display())))?;

        // `path` always includes the tool's own name first (spec §4.3's
        // `NodeRef::Command` convention); overrides address nodes by their
        // path *relative to the tool root*, so strip that first segment.
        let relative = path.get(1..).unwrap_or(&[]);

        let node_override = if relative.is_empty() {
            file.as_root_override()
        } else {
            file.node
                .iter()
                .find(|n| n.path == relative)
                .cloned()
                .ok_or(ExtractError::PathNotFound)?
        };

        let name = path.last().cloned().unwrap_or_else(|| tool.name.clone());
        Ok(node_override.into_command_node(name))
    }

    fn is_incremental(&self) -> bool {
        // Re-read per requested node rather than claim to know the whole
        // tree up front — a user override might only ever cover a handful
        // of specific nodes, never a complete subtree.
        true
    }
}

/// `$XDG_CONFIG_HOME/mandible/overrides/<tool>.toml` (or the equivalent
/// per-OS config directory `directories::ProjectDirs` resolves), if a home
/// directory could be determined at all.
fn override_path(tool_name: &str) -> Option<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "mandible")?;
    Some(
        dirs.config_dir()
            .join("overrides")
            .join(format!("{tool_name}.toml")),
    )
}

/// The on-disk override file shape. Top-level fields override the tool's
/// own root node; `[[node]]` entries address subcommands by their path
/// relative to the root (e.g. `path = ["build"]` for `<tool> build`).
#[derive(Debug, Default, Deserialize)]
struct OverrideFile {
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    hidden: Option<bool>,
    #[serde(default)]
    deprecated: Option<String>,
    #[serde(default)]
    flags: Vec<FlagOverride>,
    #[serde(default)]
    node: Vec<NodeOverride>,
}

impl OverrideFile {
    fn as_root_override(&self) -> NodeOverride {
        NodeOverride {
            path: Vec::new(),
            summary: self.summary.clone(),
            description: self.description.clone(),
            hidden: self.hidden,
            deprecated: self.deprecated.clone(),
            flags: self.flags.clone(),
        }
    }
}

/// One node's worth of overrides (the root, or one `[[node]]` entry).
#[derive(Debug, Clone, Default, Deserialize)]
struct NodeOverride {
    #[serde(default)]
    path: Vec<String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    hidden: Option<bool>,
    #[serde(default)]
    deprecated: Option<String>,
    #[serde(default)]
    flags: Vec<FlagOverride>,
}

impl NodeOverride {
    fn into_command_node(self, name: String) -> CommandNode {
        let provenance = Provenance::single(Source::UserOverride);
        CommandNode {
            name,
            summary: self.summary.map(|s| Text::sanitize(&s)),
            description: self.description.map(|s| Text::sanitize(&s)),
            hidden: self.hidden.unwrap_or(false),
            deprecated: self.deprecated.map(|s| Text::sanitize(&s)),
            flags: self
                .flags
                .into_iter()
                .map(FlagOverride::into_flag)
                .collect(),
            // An override never claims to know the subcommand list or any
            // other structural field — it only ever corrects specific
            // fields on a node another tier already found (spec §7 Tier
            // F: "the rare bad case has a clean exit", not a catalog
            // replacement). Empty `Vec`s here are treated as "no opinion"
            // by the merge (`pick_vec` skips empty candidates), so they
            // can never clobber real data from another tier.
            ..CommandNode::new(String::new(), provenance)
        }
    }
}

/// One flag's worth of overrides, identified by short and/or long
/// spelling (at least one must be given for the override to be
/// addressable at all — an entry with neither is simply dropped).
#[derive(Debug, Clone, Default, Deserialize)]
struct FlagOverride {
    #[serde(default)]
    long: Option<String>,
    #[serde(default)]
    short: Option<char>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    value_name: Option<String>,
    #[serde(default)]
    hidden: Option<bool>,
    #[serde(default)]
    deprecated: Option<String>,
    #[serde(default)]
    default: Option<String>,
    #[serde(default)]
    env_var: Option<String>,
}

impl FlagOverride {
    fn into_flag(self) -> Flag {
        Flag {
            short: self.short,
            long: self.long,
            value_name: self.value_name,
            value_kind: ValueKind::None,
            choices: Vec::new(),
            repeatable: false,
            required: false,
            hidden: self.hidden.unwrap_or(false),
            deprecated: self.deprecated.map(|s| Text::sanitize(&s)),
            inherited: false,
            group: None,
            description: self.description.map(|s| Text::sanitize(&s)),
            default: self.default.map(|s| Text::sanitize(&s)),
            env_var: self.env_var,
            provenance: Provenance::single(Source::UserOverride),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Write an override file under `xdg_config_home` at the exact path
    /// `override_path` will look for it — `<xdg_config_home>/mandible/
    /// overrides/<tool>.toml`, mirroring `ProjectDirs`' own project
    /// subdirectory rather than assuming `xdg_config_home` itself is
    /// mandible's config dir.
    fn write_override(xdg_config_home: &std::path::Path, tool: &str, contents: &str) {
        let overrides_dir = xdg_config_home.join("mandible").join("overrides");
        std::fs::create_dir_all(&overrides_dir).unwrap();
        let mut f = std::fs::File::create(overrides_dir.join(format!("{tool}.toml"))).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
    }

    fn parse(contents: &str) -> OverrideFile {
        toml::from_str(contents).unwrap()
    }

    #[test]
    fn empty_file_yields_no_overrides() {
        let file = parse("");
        assert!(file.summary.is_none());
        assert!(file.flags.is_empty());
        assert!(file.node.is_empty());
    }

    #[test]
    fn root_level_fields_override_the_root_node() {
        let file = parse(
            r#"
            summary = "a corrected summary"
            description = "a corrected description"

            [[flags]]
            long = "output"
            short = "o"
            description = "corrected description for -o/--output"
            "#,
        );
        let root = file.as_root_override();
        assert_eq!(root.summary.as_deref(), Some("a corrected summary"));
        assert_eq!(root.flags.len(), 1);
        assert_eq!(root.flags[0].long.as_deref(), Some("output"));

        let node = root.into_command_node("tool".to_string());
        assert_eq!(node.name, "tool");
        assert_eq!(
            node.summary.as_ref().unwrap().as_str(),
            "a corrected summary"
        );
        assert_eq!(node.flags[0].short, Some('o'));
        assert_eq!(node.provenance.sources[0], Source::UserOverride);
    }

    #[test]
    fn node_entries_address_subcommands_by_relative_path() {
        let file = parse(
            r#"
            [[node]]
            path = ["build"]
            summary = "corrected build summary"

            [[node]]
            path = ["build", "release"]
            summary = "corrected nested summary"
            "#,
        );
        assert_eq!(file.node.len(), 2);
        let build = file.node.iter().find(|n| n.path == ["build"]).unwrap();
        assert_eq!(build.summary.as_deref(), Some("corrected build summary"));
        let nested = file
            .node
            .iter()
            .find(|n| n.path == ["build", "release"])
            .unwrap();
        assert_eq!(nested.summary.as_deref(), Some("corrected nested summary"));
    }

    #[test]
    fn empty_vec_fields_do_not_clobber_other_sources() {
        // An override for just `summary` must still produce an empty
        // `flags` list (not fabricate any), so the merge's `pick_vec`
        // (which treats an empty candidate vec like "no opinion") leaves
        // another tier's real flags untouched.
        let file = parse(r#"summary = "just a summary fix""#);
        let node = file
            .as_root_override()
            .into_command_node("tool".to_string());
        assert!(node.flags.is_empty());
        assert!(node.subcommands.is_empty());
    }

    // Serializes access to the process-global `XDG_CONFIG_HOME` env var
    // across the tests below: Rust test binaries run tests in parallel by
    // default, and mutating process-wide env from multiple threads at once
    // is unsound. A dedicated mutex scopes these tests to run one at a time
    // relative to each other, which is sufficient since no other test in
    // this crate touches this variable.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn detect_is_false_with_no_override_file() {
        let dir = tempfile::tempdir().unwrap();
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", dir.path());
        let tier = OverridesTier;
        let tool = ResolvedTool {
            name: "definitely-not-overridden-xyz".to_string(),
            path: Some(PathBuf::from("/bin/sh")),
            version: None,
        };
        assert!(!tier.detect(&tool));
        std::env::remove_var("XDG_CONFIG_HOME");
    }

    #[test]
    fn detect_and_extract_round_trip_through_a_real_file() {
        let dir = tempfile::tempdir().unwrap();
        write_override(
            dir.path(),
            "mytool",
            r#"
            summary = "custom summary"

            [[flags]]
            long = "verbose"
            short = "v"
            description = "custom verbose description"
            "#,
        );
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", dir.path());

        let tier = OverridesTier;
        let tool = ResolvedTool {
            name: "mytool".to_string(),
            path: Some(PathBuf::from("/bin/sh")),
            version: None,
        };
        assert!(tier.detect(&tool));

        let node = tier
            .extract_node(&tool, &["mytool".to_string()])
            .expect("root override should resolve");
        assert_eq!(node.summary.as_ref().unwrap().as_str(), "custom summary");
        assert_eq!(node.flags[0].long.as_deref(), Some("verbose"));

        std::env::remove_var("XDG_CONFIG_HOME");
    }

    #[test]
    fn extract_node_for_an_unmentioned_subcommand_path_is_path_not_found() {
        let dir = tempfile::tempdir().unwrap();
        write_override(dir.path(), "mytool", r#"summary = "root only""#);
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("XDG_CONFIG_HOME", dir.path());

        let tier = OverridesTier;
        let tool = ResolvedTool {
            name: "mytool".to_string(),
            path: Some(PathBuf::from("/bin/sh")),
            version: None,
        };
        let result = tier.extract_node(
            &tool,
            &["mytool".to_string(), "some-subcommand".to_string()],
        );
        assert!(matches!(result, Err(ExtractError::PathNotFound)));

        std::env::remove_var("XDG_CONFIG_HOME");
    }
}
