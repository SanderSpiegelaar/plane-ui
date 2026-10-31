//! Lossless binding operations. Document mutations stay inside `ConfigDoc`.

use crate::commands::{Command, ParsedScript, Script, ScriptShape, format_script};
use crate::config_doc::{ConfigDoc, EditError, KeyPath};
use crate::config_view::{Binding, ConfigView};
use crate::keys::{Chord, ChordStyle, ModeName};

/// Why a mode cannot be removed. Errors leave the document unchanged.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RemoveModeError {
    /// The required main mode cannot be removed.
    #[error("main cannot be deleted")]
    Main,
    /// Bindings still switch to the requested mode.
    #[error("mode is referenced by {referencing:?}")]
    Blocked {
        /// Every referencing binding, in file order, as (mode, raw key).
        referencing: Vec<(String, String)>,
    },
    /// The document could not be edited losslessly.
    #[error(transparent)]
    Edit(#[from] EditError),
}

/// Creates an empty binding table in the existing modes' TOML style.
///
/// # Errors
///
/// Refuses existing modes or a document that cannot be edited losslessly.
pub fn add_mode(doc: &mut ConfigDoc, name: &ModeName) -> Result<(), BindingEditError> {
    let path = KeyPath::from(["mode", name.as_str()]);
    if doc.get(&path).is_some() {
        return Err(BindingEditError::AlreadyExists(path));
    }
    let mut edited = doc.clone();
    prepare_binding_table(&mut edited, name.as_str())?;
    let binding = path.join("binding");
    if edited.get(&binding).is_none() {
        edited.insert_empty_table(&binding)?;
    }
    *doc = edited;
    Ok(())
}

/// Removes a mode and all its tables, unless it is main or any binding switches to it.
///
/// Read-only arrays are checked command by command. Compound shell strings and unknown
/// command syntax are not interpreted, so references hidden inside them cannot be detected.
///
/// # Errors
///
/// Returns all referencing bindings, the main-mode guard, or a lossless edit error.
pub fn remove_mode(doc: &mut ConfigDoc, name: &ModeName) -> Result<(), RemoveModeError> {
    if name.as_str() == "main" {
        return Err(RemoveModeError::Main);
    }
    let view = ConfigView::from_doc(doc);
    let referencing = view
        .modes
        .iter()
        .flat_map(|mode| {
            mode.bindings
                .iter()
                .filter(|binding| {
                    binding_switches_to(doc, &mode.name.value, binding, name.as_str())
                })
                .map(|binding| (mode.name.value.clone(), binding.raw_key.clone()))
        })
        .collect::<Vec<_>>();
    if !referencing.is_empty() {
        return Err(RemoveModeError::Blocked { referencing });
    }
    doc.remove(&KeyPath::from(["mode", name.as_str()]))?;
    Ok(())
}

pub(crate) fn binding_switches_to(
    doc: &ConfigDoc,
    mode: &str,
    binding: &Binding,
    target: &str,
) -> bool {
    let switches =
        |command: &Command| matches!(command, Command::Mode(name) if name.as_str() == target);
    match &binding.script {
        ParsedScript::Editable { commands, .. } => commands.iter().any(switches),
        ParsedScript::ReadOnly { .. } => {
            let Some(item) = doc.get(&binding_path(mode, &binding.raw_key)) else {
                return false;
            };
            let parses = |text: &str| Command::parse(text).is_ok_and(|command| switches(&command));
            item.as_str().is_some_and(parses)
                || item.as_array().is_some_and(|array| {
                    array
                        .iter()
                        .filter_map(toml_edit::Value::as_str)
                        .any(parses)
                })
        }
    }
}

/// Why a binding operation could not be applied. Errors leave the document unchanged.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BindingEditError {
    /// The mode or script cannot be edited through the binding form.
    #[error("{path} is read-only: {reason}")]
    ReadOnly {
        /// Mode or binding that cannot be rewritten.
        path: KeyPath,
        /// Explanation shown to the user.
        reason: String,
    },
    /// The requested binding or mode does not exist.
    #[error("{0} does not exist")]
    NotFound(KeyPath),
    /// The destination already has this literal key.
    #[error("{0} already exists")]
    AlreadyExists(KeyPath),
    /// The raw value cannot be copied into the destination's representation safely.
    #[error("{0} cannot be copied verbatim into an inline table")]
    IncompatibleStyle(KeyPath),
    /// A lossless document operation failed.
    #[error(transparent)]
    Edit(#[from] EditError),
}

/// Adds a binding, or edits and optionally renames an existing binding in place.
///
/// # Errors
///
/// Refuses read-only scripts or modes, missing originals, and literal key collisions.
pub fn upsert_binding(
    doc: &mut ConfigDoc,
    mode: &str,
    original_raw_key: Option<&str>,
    chord: &Chord,
    commands: &[Command],
) -> Result<(), BindingEditError> {
    ensure_mode_editable(doc, mode)?;
    let raw_key = chord.format(original_raw_key.map_or(ChordStyle::Canonical, ChordStyle::LikeRaw));
    let destination = binding_path(mode, &raw_key);
    let shape = if let Some(original) = original_raw_key {
        let path = binding_path(mode, original);
        let item = doc
            .get(&path)
            .ok_or_else(|| BindingEditError::NotFound(path.clone()))?;
        match Script::parse(item) {
            ParsedScript::Editable { shape, .. } => shape,
            ParsedScript::ReadOnly { reason } => {
                return Err(BindingEditError::ReadOnly { path, reason });
            }
        }
    } else {
        ScriptShape::String
    };
    if original_raw_key != Some(raw_key.as_str()) && doc.get(&destination).is_some() {
        return Err(BindingEditError::AlreadyExists(destination));
    }
    let mut edited = doc.clone();
    if let Some(original) = original_raw_key {
        edited.rename_key(&binding_path(mode, original), &raw_key)?;
    } else {
        prepare_binding_table(&mut edited, mode)?;
    }
    edited.set_value(&destination, format_script(commands, shape))?;
    *doc = edited;
    Ok(())
}

/// Removes a binding and only the comments owned by that binding, even for read-only scripts.
///
/// # Errors
///
/// Returns an error when the binding is missing or the document cannot be edited losslessly.
pub fn remove_binding(
    doc: &mut ConfigDoc,
    mode: &str,
    raw_key: &str,
) -> Result<(), BindingEditError> {
    doc.remove(&binding_path(mode, raw_key))?;
    Ok(())
}

/// Copies a raw binding value into another mode without parsing or rewriting its script.
///
/// # Errors
///
/// Refuses missing sources, literal collisions, unsafe destination modes, or incompatible decor.
pub fn duplicate_binding(
    doc: &mut ConfigDoc,
    from_mode: &str,
    raw_key: &str,
    to_mode: &str,
) -> Result<(), BindingEditError> {
    let source = binding_path(from_mode, raw_key);
    let value = doc
        .get(&source)
        .and_then(toml_edit::Item::as_value)
        .ok_or_else(|| BindingEditError::NotFound(source.clone()))?
        .clone();
    ensure_mode_editable(doc, to_mode)?;
    let destination = binding_path(to_mode, raw_key);
    if doc.get(&destination).is_some() {
        return Err(BindingEditError::AlreadyExists(destination));
    }
    let mut edited = doc.clone();
    prepare_binding_table(&mut edited, to_mode)?;
    edited.insert_value_verbatim(&destination, value)?;
    // A trailing line comment from a section value may swallow an inline table's closing brace.
    if ConfigDoc::parse(&edited.to_string()).is_err() {
        return Err(BindingEditError::IncompatibleStyle(destination));
    }
    *doc = edited;
    Ok(())
}

fn binding_path(mode: &str, raw_key: &str) -> KeyPath {
    KeyPath::from(["mode", mode, "binding", raw_key])
}

fn prepare_binding_table(doc: &mut ConfigDoc, mode: &str) -> Result<(), EditError> {
    let path = KeyPath::from(["mode", mode, "binding"]);
    if doc.get(&path).is_none()
        && doc
            .get(&KeyPath::from(["mode"]))
            .and_then(toml_edit::Item::as_table_like)
            .is_some_and(|modes| {
                modes.iter().any(|(_, item)| {
                    item.as_table_like()
                        .and_then(|mode| mode.get("binding"))
                        .is_some_and(toml_edit::Item::is_inline_table)
                })
            })
    {
        doc.set_value(
            &path,
            toml_edit::Value::InlineTable(toml_edit::InlineTable::new()),
        )?;
    }
    Ok(())
}

fn ensure_mode_editable(doc: &ConfigDoc, name: &str) -> Result<(), BindingEditError> {
    let path = KeyPath::from(["mode", name]);
    let view = ConfigView::from_doc(doc);
    let mode = view
        .modes
        .iter()
        .find(|mode| mode.name.value == name)
        .ok_or_else(|| BindingEditError::NotFound(path.clone()))?;
    if !mode.editable {
        return Err(BindingEditError::ReadOnly {
            path,
            reason: "the mode's TOML structure cannot be edited safely".to_owned(),
        });
    }
    Ok(())
}
