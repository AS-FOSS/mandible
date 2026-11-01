//! The canonical normalized tree: what a version of mandible read of a tool,
//! reduced to what an audit judges (docs/accuracy.md), independent of the
//! version's types.
//!
//! Kept per node: `path`, `flags`, `positionals`, `subcommands`; per flag:
//! sorted `spellings`, `value` (no brackets or `=`), `optional`, `choices`.
//! All sorted. Two representations are folded: an alternation
//! value (`y|n`, `{a,b}`) becomes `choices`, since releases keep it as the
//! value, the choices, or both; a range row (`-0 ... -9`) is expanded to one
//! flag per member, as older releases carry it. Both the `flags` schema and
//! the later `entities` schema are read, by the keys present. Fields are
//! declared alphabetically, so the compact JSON's sha256 is the tree hash.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::sha256::sha256_hex;

/// One flag as the audit judges it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Flag {
    /// Sorted choice names, when the version models them.
    pub choices: Vec<String>,
    /// The value may be left out (`[=FILE]`).
    pub optional: bool,
    /// Sorted typed spellings, dashes included.
    pub spellings: Vec<String>,
    /// The stripped value placeholder; empty when the flag takes a value
    /// that carries no name, absent when it takes none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
}

/// One node of the normalized tree.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Node {
    /// Sorted flags.
    pub flags: Vec<Flag>,
    /// Tool name, then subcommand names down to this node.
    pub path: Vec<String>,
    /// Sorted, stripped positional names.
    pub positionals: Vec<String>,
    /// Subcommands sorted by name.
    pub subcommands: Vec<Node>,
}

impl Node {
    /// The tree of a tool no tier produced a root for.
    pub fn empty(tool: &str) -> Node {
        Node {
            flags: Vec::new(),
            path: vec![tool.to_string()],
            positionals: Vec::new(),
            subcommands: Vec::new(),
        }
    }

    /// Compact, key-sorted JSON bytes; the file format and the hash input.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }

    /// Lowercase hex sha256 of [`Self::canonical_bytes`].
    pub fn sha256(&self) -> String {
        sha256_hex(&self.canonical_bytes())
    }
}

/// Remove the decoration a version may or may not keep around a name.
pub fn strip_name(raw: &str) -> String {
    raw.chars()
        .filter(|c| !matches!(c, '[' | ']' | '<' | '>' | '='))
        .collect::<String>()
        .trim()
        .to_string()
}

/// Normalize a harness dump (`{"root": <node> | null}`) for `tool`.
pub fn normalize_dump(tool: &str, dump: &Value) -> Node {
    match dump.get("root") {
        Some(root) if root.is_object() => normalize_node(&[tool.to_string()], root),
        _ => Node::empty(tool),
    }
}

fn normalize_node(path: &[String], raw: &Value) -> Node {
    let (mut flags, mut positionals) = if raw.get("entities").is_some() {
        entity_items(raw)
    } else {
        legacy_items(raw)
    };
    flags.sort();
    positionals.sort();
    let mut subcommands: Vec<Node> = array(raw, "subcommands")
        .iter()
        .map(|child| {
            let mut child_path = path.to_vec();
            child_path.push(text(child, "name").unwrap_or_default());
            normalize_node(&child_path, child)
        })
        .collect();
    subcommands.sort();
    Node {
        flags,
        path: path.to_vec(),
        positionals,
        subcommands,
    }
}

fn array<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key)
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_string)
}

fn choice_names(raw: &Value) -> Vec<String> {
    let mut names: Vec<String> = array(raw, "choices")
        .iter()
        .filter_map(|c| match c {
            Value::String(s) => Some(s.clone()),
            other => text(other, "name"),
        })
        .collect();
    names.sort();
    names
}

/// The members of a value written as an alternation, if it is one.
fn alternatives(value: &str) -> Option<Vec<String>> {
    let braced = value.strip_prefix('{').and_then(|v| v.strip_suffix('}'));
    let parened = value.strip_prefix('(').and_then(|v| v.strip_suffix(')'));
    let inner = braced.or(parened).unwrap_or(value);
    let members: Vec<String> = inner
        .split(|c| c == '|' || (braced.is_some() && c == ','))
        .map(|m| m.trim().to_string())
        .collect();
    (members.len() >= 2 && members.iter().all(|m| !m.is_empty())).then_some(members)
}

fn flag_from(raw: &Value, mut spellings: Vec<String>) -> Flag {
    spellings.sort();
    let kind = text(raw, "value_kind").unwrap_or_default();
    let name = text(raw, "value_name").map(|n| strip_name(&n));
    let takes_value =
        (kind != "None" && !kind.is_empty()) || name.as_deref().is_some_and(|n| !n.is_empty());
    let mut choices = choice_names(raw);
    let mut value = takes_value.then(|| name.clone().unwrap_or_default());
    if let Some(members) = name.as_deref().and_then(alternatives) {
        choices.extend(members);
        value = Some(String::new());
    }
    choices.sort();
    choices.dedup();
    Flag {
        choices,
        optional: kind == "Optional",
        spellings,
        value,
    }
}

/// The typed spellings a range row covers beyond its first, when the range
/// runs over integers or single letters.
fn range_members(item: &Value) -> Vec<String> {
    let Some(range) = item.get("range").filter(|r| r.is_object()) else {
        return Vec::new();
    };
    let side = |key: &str| range.get(key).cloned().unwrap_or(Value::Null);
    let (start, end) = (side("start"), side("end"));
    if text(&start, "dashes") != text(&end, "dashes") {
        return Vec::new();
    }
    let dashes = match text(&start, "dashes").as_deref() {
        Some("Single") => "-",
        Some("Double") => "--",
        _ => "",
    };
    let (a, b) = (
        text(&start, "name").unwrap_or_default(),
        text(&end, "name").unwrap_or_default(),
    );
    let names: Vec<String> = match (a.parse::<u32>(), b.parse::<u32>()) {
        (Ok(lo), Ok(hi)) if lo < hi && hi - lo < 1000 => {
            (lo + 1..=hi).map(|n| n.to_string()).collect()
        }
        _ => match (a.chars().next(), b.chars().next()) {
            (Some(lo), Some(hi))
                if a.len() == 1 && b.len() == 1 && lo.is_ascii_alphabetic() && lo < hi =>
            {
                (lo..=hi).skip(1).map(String::from).collect()
            }
            _ => Vec::new(),
        },
    };
    names.into_iter().map(|n| format!("{dashes}{n}")).collect()
}

/// The entity schema: one `entities` list, kind-tagged.
fn entity_items(raw: &Value) -> (Vec<Flag>, Vec<String>) {
    let mut flags = Vec::new();
    let mut positionals = Vec::new();
    for item in array(raw, "entities") {
        let spellings: Vec<String> = array(item, "spellings")
            .iter()
            .map(|s| {
                let dashes = match text(s, "dashes").as_deref() {
                    Some("Single") => "-",
                    Some("Double") => "--",
                    _ => "",
                };
                format!("{dashes}{}", text(s, "name").unwrap_or_default())
            })
            .collect();
        match text(item, "kind").as_deref() {
            Some("Flag") => {
                let flag = flag_from(item, spellings);
                for member in range_members(item) {
                    flags.push(Flag {
                        spellings: vec![member],
                        ..flag.clone()
                    });
                }
                flags.push(flag);
            }
            Some("Positional") => {
                positionals.push(strip_name(&spellings.first().cloned().unwrap_or_default()));
            }
            _ => {}
        }
    }
    (flags, positionals)
}

/// The pre-entity schema: `flags` with `short`/`long`, and `positionals`.
fn legacy_items(raw: &Value) -> (Vec<Flag>, Vec<String>) {
    let flags = array(raw, "flags")
        .iter()
        .map(|f| {
            let mut spellings = Vec::new();
            if let Some(short) = text(f, "short") {
                spellings.push(format!("-{short}"));
            }
            if let Some(long) = text(f, "long") {
                let single = f.get("single_dash").and_then(Value::as_bool) == Some(true);
                spellings.push(format!("{}{long}", if single { "-" } else { "--" }));
            }
            flag_from(f, spellings)
        })
        .collect();
    let positionals = array(raw, "positionals")
        .iter()
        .map(|p| strip_name(&text(p, "name").unwrap_or_default()))
        .collect();
    (flags, positionals)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entity_dump() -> Value {
        json!({ "root": {
            "name": "tool",
            "entities": [
                { "kind": "Flag", "value_kind": "Required", "value_name": "<FILE>",
                  "choices": [{ "name": "b" }, { "name": "a" }],
                  "spellings": [
                    { "name": "output", "dashes": "Double" },
                    { "name": "o", "dashes": "Single" } ] },
                { "kind": "Flag", "value_kind": "None", "value_name": null, "choices": [],
                  "spellings": [{ "name": "help", "dashes": "Single", "negatable": true }] },
                { "kind": "Positional", "value_kind": "None", "choices": [],
                  "spellings": [{ "name": "[path]", "dashes": "None" }] },
                { "kind": "EnvVar", "spellings": [{ "name": "HOME", "dashes": "None" }] }
            ],
            "subcommands": [
                { "name": "zeta", "entities": [], "subcommands": [] },
                { "name": "alpha", "entities": [], "subcommands": [] }
            ] } })
    }

    fn legacy_dump() -> Value {
        json!({ "root": {
            "name": "tool",
            "flags": [
                { "short": "o", "long": "output", "single_dash": false,
                  "value_kind": "Required", "value_name": "FILE", "choices": ["a", "b"] },
                { "short": null, "long": "help", "single_dash": true,
                  "value_kind": "None", "value_name": null, "choices": [] }
            ],
            "positionals": [{ "name": "path", "required": false, "variadic": false }],
            "subcommands": [
                { "name": "alpha", "flags": [], "positionals": [], "subcommands": [] },
                { "name": "zeta", "flags": [], "positionals": [], "subcommands": [] }
            ] } })
    }

    #[test]
    fn both_schemas_normalize_identically() {
        let a = normalize_dump("tool", &entity_dump());
        let b = normalize_dump("tool", &legacy_dump());
        assert_eq!(a, b);
        assert_eq!(a.sha256(), b.sha256());
    }

    #[test]
    fn values_are_stripped_and_case_is_kept() {
        let n = normalize_dump("tool", &entity_dump());
        let out = n
            .flags
            .iter()
            .find(|f| f.spellings.contains(&"--output".to_string()))
            .unwrap();
        assert_eq!(out.value.as_deref(), Some("FILE"));
        assert_eq!(out.spellings, ["--output", "-o"]);
        assert_eq!(out.choices, ["a", "b"]);
        assert_eq!(n.positionals, ["path"]);
    }

    #[test]
    fn subcommands_sort_and_carry_paths() {
        let n = normalize_dump("tool", &entity_dump());
        let names: Vec<&str> = n.subcommands.iter().map(|c| c.path[1].as_str()).collect();
        assert_eq!(names, ["alpha", "zeta"]);
        assert_eq!(n.subcommands[0].path, ["tool", "alpha"]);
    }

    #[test]
    fn optional_value_and_unnamed_value() {
        let dump = json!({ "root": { "name": "t", "flags": [
            { "short": null, "long": "color", "single_dash": false,
              "value_kind": "Optional", "value_name": "[=WHEN]", "choices": [] },
            { "short": "x", "long": null, "single_dash": false,
              "value_kind": "Required", "value_name": null, "choices": [] }
        ], "positionals": [], "subcommands": [] } });
        let n = normalize_dump("t", &dump);
        let color = n.flags.iter().find(|f| f.spellings == ["--color"]).unwrap();
        assert!(color.optional);
        assert_eq!(color.value.as_deref(), Some("WHEN"));
        let x = n.flags.iter().find(|f| f.spellings == ["-x"]).unwrap();
        assert_eq!(x.value.as_deref(), Some(""));
    }

    #[test]
    fn alternations_become_choices_in_every_spelling_of_the_idea() {
        let flag = |value: Value, choices: Value| {
            json!({ "root": { "name": "t", "flags": [{ "short": null, "long": "f",
                "single_dash": false, "value_kind": "Required",
                "value_name": value, "choices": choices }],
                "positionals": [], "subcommands": [] } })
        };
        let bare = normalize_dump("t", &flag(json!("y|n"), json!([])));
        let both = normalize_dump("t", &flag(json!("y|n"), json!(["n", "y"])));
        let braced = normalize_dump("t", &flag(json!("{y,n}"), json!([])));
        let parened = normalize_dump("t", &flag(json!("(y|n)"), json!([])));
        let choices_only = normalize_dump("t", &flag(json!(null), json!(["y", "n"])));
        assert_eq!(bare.flags[0].choices, ["n", "y"]);
        assert_eq!(bare.flags[0].value.as_deref(), Some(""));
        for other in [&both, &braced, &parened, &choices_only] {
            assert_eq!(bare.sha256(), other.sha256());
        }
        let plain = normalize_dump("t", &flag(json!("module(,module)*"), json!([])));
        assert!(plain.flags[0].choices.is_empty());
        assert_eq!(plain.flags[0].value.as_deref(), Some("module(,module)*"));
    }

    #[test]
    fn a_range_row_equals_one_flag_per_member() {
        let entity = |name: &str, range: Value| {
            json!({ "kind": "Flag", "value_kind": "None", "choices": [], "range": range,
                "spellings": [{ "name": name, "dashes": "Single" }] })
        };
        let range = json!({ "start": { "name": "0", "dashes": "Single" },
                            "end": { "name": "3", "dashes": "Single" } });
        let ranged = json!({ "root": { "name": "t", "subcommands": [],
            "entities": [entity("0", range)] } });
        let expanded = json!({ "root": { "name": "t", "subcommands": [],
            "entities": [entity("0", Value::Null), entity("1", Value::Null),
                entity("2", Value::Null), entity("3", Value::Null)] } });
        let a = normalize_dump("t", &ranged);
        assert_eq!(a.flags.len(), 4);
        assert_eq!(a.sha256(), normalize_dump("t", &expanded).sha256());
    }

    #[test]
    fn missing_root_is_the_empty_tree() {
        let n = normalize_dump("tool", &json!({ "root": null }));
        assert_eq!(n, Node::empty("tool"));
    }

    #[test]
    fn canonical_bytes_have_sorted_keys_and_no_whitespace() {
        let bytes = normalize_dump("tool", &legacy_dump()).canonical_bytes();
        let s = String::from_utf8(bytes).unwrap();
        assert!(!s.contains(' ') && !s.contains('\n'));
        assert!(s.starts_with("{\"flags\":[{\"choices\":"));
        let flags = s.find("\"flags\"").unwrap();
        let path = s.find("\"path\"").unwrap();
        let positionals = s.find("\"positionals\"").unwrap();
        let subs = s.find("\"subcommands\"").unwrap();
        assert!(flags < path && path < positionals && positionals < subs);
    }

    #[test]
    fn row_order_never_changes_the_hash() {
        let mut shuffled = legacy_dump();
        shuffled["root"]["flags"].as_array_mut().unwrap().reverse();
        shuffled["root"]["subcommands"]
            .as_array_mut()
            .unwrap()
            .reverse();
        assert_eq!(
            normalize_dump("tool", &legacy_dump()).sha256(),
            normalize_dump("tool", &shuffled).sha256()
        );
    }
}
