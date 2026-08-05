//! Deserializing the carapace-spec JSON shape and converting it into the
//! shared IR. See the vendoring notes in `scripts/vendor_carapace_specs.py`
//! for the schema this mirrors.

use mantui_core::{CommandNode, Flag, Positional, Provenance, Source, Text, ValueKind};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(super) struct RawCommand {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub documentation: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub usage: Vec<String>,
    #[serde(default)]
    pub flags: Vec<RawFlag>,
    #[serde(default)]
    pub persistentflags: Vec<RawFlag>,
    #[serde(default)]
    pub commands: Vec<RawCommand>,
}

#[derive(Debug, Deserialize)]
pub(super) struct RawFlag {
    #[serde(default, deserialize_with = "de_opt_char")]
    pub short: Option<char>,
    #[serde(default)]
    pub long: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub takes_value: bool,
    #[serde(default)]
    pub optional_value: bool,
    #[serde(default)]
    pub repeatable: bool,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub hidden: bool,
}

fn de_opt_char<'de, D>(deserializer: D) -> Result<Option<char>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt: Option<String> = Option::deserialize(deserializer)?;
    Ok(opt.and_then(|s| s.chars().next()))
}

fn carapace_provenance() -> Provenance {
    Provenance::single(Source::KnownSpec {
        provider: "carapace".to_string(),
    })
}

/// Convert one raw carapace command (recursively) into the shared IR.
///
/// `inherited` carries flags already marked `inherited: true` from
/// ancestors; this node's own `persistentflags` are added to its own flag
/// list (not marked inherited *here* — they belong to this node) and then
/// propagated to `next_inherited` for descendants, marked `inherited: true`
/// (spec §4, mapping notes: "propagated to descendants with
/// `inherited: true`").
pub(super) fn convert(raw: RawCommand, inherited: &[Flag]) -> CommandNode {
    let own_flags: Vec<Flag> = raw
        .flags
        .into_iter()
        .map(|f| convert_flag(f, false, carapace_provenance()))
        .collect();
    let persistent: Vec<Flag> = raw
        .persistentflags
        .into_iter()
        .map(|f| convert_flag(f, false, carapace_provenance()))
        .collect();

    let mut flags = Vec::with_capacity(inherited.len() + own_flags.len() + persistent.len());
    flags.extend(inherited.iter().cloned());
    flags.extend(own_flags);
    flags.extend(persistent.iter().cloned());
    let flags = mantui_core::pair_aliases(flags);

    let mut next_inherited = inherited.to_vec();
    for f in &persistent {
        let mut pf = f.clone();
        pf.inherited = true;
        next_inherited.push(pf);
    }

    let subcommands = raw
        .commands
        .into_iter()
        .map(|c| convert(c, &next_inherited))
        .collect();

    CommandNode {
        name: raw.name,
        aliases: raw.aliases,
        // `description`/`documentation` are carapace's markdown-flavored
        // prose fields (`[label](uri)` links with custom schemes like
        // `man://`/`cmd://`, inline code, bold/emphasis) — see spec §4.1
        // and Text::sanitize_markdown's docs. `usage` is literal command
        // syntax, not prose, so it stays on the plain sanitizer.
        summary: raw.description.as_deref().map(Text::sanitize_markdown),
        description: raw.documentation.as_deref().map(Text::sanitize_markdown),
        usage: raw.usage.iter().map(|s| Text::sanitize(s)).collect(),
        flags,
        positionals: Vec::<Positional>::new(),
        subcommands,
        examples: Vec::new(),
        hidden: raw.hidden,
        deprecated: None,
        children_filled: true,
        group: raw.group,
        provenance: carapace_provenance(),
    }
}

fn convert_flag(raw: RawFlag, inherited: bool, provenance: Provenance) -> Flag {
    let value_kind = if raw.optional_value {
        ValueKind::Optional
    } else if raw.takes_value {
        ValueKind::Required
    } else {
        ValueKind::None
    };
    Flag {
        short: raw.short,
        long: raw.long,
        value_name: None,
        value_kind,
        choices: Vec::new(),
        repeatable: raw.repeatable,
        required: raw.required,
        hidden: raw.hidden,
        deprecated: None,
        inherited,
        group: None,
        description: raw.description.as_deref().map(Text::sanitize_markdown),
        default: None,
        env_var: None,
        provenance,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_minimal_command() {
        let json = r#"{"name":"git","description":"the stupid content tracker","flags":[{"long":"version","short":null,"takes_value":false,"optional_value":false,"repeatable":false,"required":false,"hidden":false,"description":"show version"}],"persistentflags":[],"commands":[]}"#;
        let raw: RawCommand = serde_json::from_str(json).unwrap();
        let node = convert(raw, &[]);
        assert_eq!(node.name, "git");
        assert_eq!(node.summary.unwrap().as_str(), "the stupid content tracker");
        assert_eq!(node.flags.len(), 1);
        assert_eq!(node.flags[0].long.as_deref(), Some("version"));
    }

    #[test]
    fn persistent_flags_propagate_as_inherited() {
        let json = r#"{
            "name":"docker",
            "flags":[],
            "persistentflags":[{"long":"help","short":"h","takes_value":false,"optional_value":false,"repeatable":false,"required":false,"hidden":true,"description":"Print usage"}],
            "commands":[
                {"name":"run","flags":[],"persistentflags":[],"commands":[]}
            ]
        }"#;
        let raw: RawCommand = serde_json::from_str(json).unwrap();
        let node = convert(raw, &[]);
        // The root itself carries its own persistent flag, not marked
        // inherited (it originates here).
        assert_eq!(node.flags.len(), 1);
        assert!(!node.flags[0].inherited);

        // The child inherits it, marked inherited: true.
        let run = &node.subcommands[0];
        assert_eq!(run.flags.len(), 1);
        assert!(run.flags[0].inherited);
        assert_eq!(run.flags[0].long.as_deref(), Some("help"));
    }

    #[test]
    fn value_kind_mapping() {
        let f = RawFlag {
            short: None,
            long: Some("output".to_string()),
            description: None,
            takes_value: true,
            optional_value: false,
            repeatable: false,
            required: false,
            hidden: false,
        };
        let flag = convert_flag(f, false, carapace_provenance());
        assert_eq!(flag.value_kind, ValueKind::Required);
    }
}
