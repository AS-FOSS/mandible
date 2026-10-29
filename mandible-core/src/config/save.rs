//! Writing one setting back to `config.toml`.

use super::{Key, Value};
use std::path::Path;

/// Why a setting could not be written; the text goes in the footer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveError(pub String);

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Set `key` to `value` in the file at `path`, changing nothing else: the
/// comments, the order and the keys mandible does not know stay as they
/// are. The file and its directory are created when missing. A file that
/// does not parse is left alone and reported.
pub fn save(path: &Path, key: Key, value: Value) -> Result<(), SaveError> {
    let existing = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(SaveError(e.to_string())),
    };
    let mut doc: toml_edit::DocumentMut = existing
        .parse()
        .map_err(|_| SaveError("the file is not valid TOML, fix it first".to_string()))?;
    let fresh_file = existing.trim().is_empty();
    let table = match doc
        .entry(key.section())
        .or_insert_with(|| fresh_table(!fresh_file))
        .as_table_like_mut()
    {
        Some(table) => table,
        None => return Err(SaveError(format!("{} is not a table", key.section()))),
    };
    let mut new = match value {
        Value::Bool(b) => toml_edit::Value::from(b),
        Value::Word(w) => toml_edit::Value::from(w),
    };
    if let Some(old) = table.get(key.name()).and_then(toml_edit::Item::as_value) {
        *new.decor_mut() = old.decor().clone();
    }
    table.insert(key.name(), toml_edit::Item::Value(new));
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| SaveError(e.to_string()))?;
    }
    std::fs::write(path, doc.to_string()).map_err(|e| SaveError(e.to_string()))
}

/// A new `[section]`, set off by a blank line when the file has content.
fn fresh_table(blank_before: bool) -> toml_edit::Item {
    let mut table = toml_edit::Table::new();
    if blank_before {
        table.decor_mut().set_prefix("\n");
    }
    toml_edit::Item::Table(table)
}
