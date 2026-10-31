//! Typed scalar settings and their AeroSpace defaults.

use toml_edit::{Item, Value};

use crate::config_doc::{ConfigDoc, EditError, KeyPath};
use crate::keys::Preset;

/// The default root container layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// Tiled windows.
    Tiles,
    /// Overlapping windows.
    Accordion,
}

impl Layout {
    /// The AeroSpace spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tiles => "tiles",
            Self::Accordion => "accordion",
        }
    }

    /// Parses an AeroSpace spelling.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "tiles" => Some(Self::Tiles),
            "accordion" => Some(Self::Accordion),
            _ => None,
        }
    }
}

/// The default root container orientation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    /// Columns.
    Horizontal,
    /// Rows.
    Vertical,
    /// Chosen from the monitor's shape.
    Auto,
}

impl Orientation {
    /// The AeroSpace spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Horizontal => "horizontal",
            Self::Vertical => "vertical",
            Self::Auto => "auto",
        }
    }

    /// Parses an AeroSpace spelling.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "horizontal" => Some(Self::Horizontal),
            "vertical" => Some(Self::Vertical),
            "auto" => Some(Self::Auto),
            _ => None,
        }
    }
}

/// One editable scalar setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    /// Launch AeroSpace at login.
    StartAtLogin,
    /// Let AeroSpace watch its config file.
    AutoReloadConfig,
    /// Accordion window padding.
    AccordionPadding,
    /// Root container layout.
    RootLayout,
    /// Root container orientation.
    RootOrientation,
    /// Flatten redundant containers.
    FlattenContainers,
    /// Alternate nested container orientation.
    OppositeOrientation,
    /// Unhide hidden applications.
    UnhideApps,
    /// Focus windows under the pointer.
    FocusFollowsMouse,
    /// Keyboard mapping preset.
    KeyMappingPreset,
}

/// A setting's accepted value type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingKind {
    /// Boolean switch.
    Bool,
    /// Nonnegative integer.
    Count,
    /// Layout choice.
    Layout,
    /// Orientation choice.
    Orientation,
    /// Keyboard preset choice.
    Preset,
}

impl SettingKind {
    /// Allowed choices for a select control; empty for numbers and switches.
    #[must_use]
    pub const fn choices(self) -> &'static [SettingValue] {
        match self {
            Self::Layout => &[
                SettingValue::Layout(Layout::Tiles),
                SettingValue::Layout(Layout::Accordion),
            ],
            Self::Orientation => &[
                SettingValue::Orientation(Orientation::Horizontal),
                SettingValue::Orientation(Orientation::Vertical),
                SettingValue::Orientation(Orientation::Auto),
            ],
            Self::Preset => &[
                SettingValue::Preset(Preset::Qwerty),
                SettingValue::Preset(Preset::Dvorak),
                SettingValue::Preset(Preset::Colemak),
            ],
            Self::Bool | Self::Count => &[],
        }
    }
}

/// A validated scalar setting value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingValue {
    /// Boolean switch.
    Bool(bool),
    /// Nonnegative integer.
    Count(u32),
    /// Layout choice.
    Layout(Layout),
    /// Orientation choice.
    Orientation(Orientation),
    /// Keyboard preset choice.
    Preset(Preset),
}

impl SettingValue {
    /// The value's kind.
    #[must_use]
    pub const fn kind(self) -> SettingKind {
        match self {
            Self::Bool(_) => SettingKind::Bool,
            Self::Count(_) => SettingKind::Count,
            Self::Layout(_) => SettingKind::Layout,
            Self::Orientation(_) => SettingKind::Orientation,
            Self::Preset(_) => SettingKind::Preset,
        }
    }

    fn into_toml(self) -> Value {
        match self {
            Self::Bool(value) => value.into(),
            Self::Count(value) => i64::from(value).into(),
            Self::Layout(value) => value.as_str().into(),
            Self::Orientation(value) => value.as_str().into(),
            Self::Preset(value) => value.as_str().into(),
        }
    }
}

impl std::fmt::Display for SettingValue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bool(value) => value.fmt(formatter),
            Self::Count(value) => value.fmt(formatter),
            Self::Layout(value) => formatter.write_str(value.as_str()),
            Self::Orientation(value) => formatter.write_str(value.as_str()),
            Self::Preset(value) => formatter.write_str(value.as_str()),
        }
    }
}

/// The single source of a setting's path, type and effective default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettingSpec {
    /// TOML path segments.
    pub path: &'static [&'static str],
    /// Accepted value type.
    pub kind: SettingKind,
    /// AeroSpace's value when omitted.
    pub default: SettingValue,
}

impl Setting {
    /// Settings in form order.
    pub const ALL: [Self; 10] = [
        Self::StartAtLogin,
        Self::AutoReloadConfig,
        Self::AccordionPadding,
        Self::RootLayout,
        Self::RootOrientation,
        Self::FlattenContainers,
        Self::OppositeOrientation,
        Self::UnhideApps,
        Self::FocusFollowsMouse,
        Self::KeyMappingPreset,
    ];

    /// The setting's path, kind and default.
    #[must_use]
    pub const fn spec(self) -> SettingSpec {
        let (path, default): (&'static [&'static str], SettingValue) = match self {
            Self::StartAtLogin => (&["start-at-login"], SettingValue::Bool(false)),
            Self::AutoReloadConfig => (&["auto-reload-config"], SettingValue::Bool(false)),
            Self::AccordionPadding => (&["accordion-padding"], SettingValue::Count(30)),
            Self::RootLayout => (
                &["default-root-container-layout"],
                SettingValue::Layout(Layout::Tiles),
            ),
            Self::RootOrientation => (
                &["default-root-container-orientation"],
                SettingValue::Orientation(Orientation::Auto),
            ),
            Self::FlattenContainers => (
                &["enable-normalization-flatten-containers"],
                SettingValue::Bool(true),
            ),
            Self::OppositeOrientation => (
                &["enable-normalization-opposite-orientation-for-nested-containers"],
                SettingValue::Bool(true),
            ),
            Self::UnhideApps => (
                &["automatically-unhide-macos-hidden-apps"],
                SettingValue::Bool(false),
            ),
            Self::FocusFollowsMouse => (
                &["focus-follows-mouse", "enabled"],
                SettingValue::Bool(false),
            ),
            Self::KeyMappingPreset => (
                &["key-mapping", "preset"],
                SettingValue::Preset(Preset::Qwerty),
            ),
        };
        SettingSpec {
            path,
            kind: default.kind(),
            default,
        }
    }

    /// Reads and validates a present scalar value.
    #[must_use]
    pub fn read(self, item: &Item) -> Option<SettingValue> {
        match self.spec().kind {
            SettingKind::Bool => item.as_bool().map(SettingValue::Bool),
            SettingKind::Count => item
                .as_integer()
                .and_then(|value| u32::try_from(value).ok())
                .map(SettingValue::Count),
            SettingKind::Layout => item
                .as_str()
                .and_then(Layout::parse)
                .map(SettingValue::Layout),
            SettingKind::Orientation => item
                .as_str()
                .and_then(Orientation::parse)
                .map(SettingValue::Orientation),
            SettingKind::Preset => item
                .as_str()
                .and_then(Preset::parse)
                .map(SettingValue::Preset),
        }
    }
}

/// A refused setting edit.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SettingEditError {
    /// The value belongs to another control kind.
    #[error("{setting:?} requires {expected:?}, not {actual:?}")]
    WrongKind {
        /// The target setting.
        setting: Setting,
        /// Required type.
        expected: SettingKind,
        /// Supplied type.
        actual: SettingKind,
    },
    /// A lossless document edit failed.
    #[error(transparent)]
    Edit(#[from] EditError),
}

/// Sets a scalar setting, rejecting values of another kind.
///
/// # Errors
/// Returns a kind mismatch or lossless edit failure without changing the document.
pub fn set_setting(
    doc: &mut ConfigDoc,
    setting: Setting,
    value: SettingValue,
) -> Result<(), SettingEditError> {
    let spec = setting.spec();
    if spec.kind != value.kind() {
        return Err(SettingEditError::WrongKind {
            setting,
            expected: spec.kind,
            actual: value.kind(),
        });
    }
    let mut edited = doc.clone();
    edited.set_value(&KeyPath::new(spec.path.iter().copied()), value.into_toml())?;
    *doc = edited;
    Ok(())
}

/// Removes a setting so AeroSpace uses its default. Missing keys are unchanged.
///
/// # Errors
/// Returns a lossless document edit failure.
pub fn reset_setting(doc: &mut ConfigDoc, setting: Setting) -> Result<(), SettingEditError> {
    let path = KeyPath::new(setting.spec().path.iter().copied());
    if doc.get(&path).is_some() {
        doc.remove(&path)?;
    }
    Ok(())
}

/// A refused persistent-workspace edit.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WorkspaceEditError {
    /// Explicit persistent workspaces require version 2.
    #[error("persistent-workspaces requires config-version = 2")]
    Version,
    /// A name occurs more than once.
    #[error("persistent workspace {0:?} already exists")]
    Duplicate(String),
    /// A lossless edit failed.
    #[error(transparent)]
    Edit(#[from] EditError),
}

fn persistent_path() -> KeyPath {
    KeyPath::from(["persistent-workspaces"])
}

fn ensure_version_two(doc: &ConfigDoc) -> Result<(), WorkspaceEditError> {
    if doc
        .get(&KeyPath::from(["config-version"]))
        .and_then(Item::as_integer)
        == Some(2)
    {
        Ok(())
    } else {
        Err(WorkspaceEditError::Version)
    }
}

/// Upgrades to version 2, optionally preserving version 1's inferred workspaces.
///
/// # Errors
/// Refuses duplicate prefill names or a lossless edit failure. The original is unchanged.
pub fn upgrade_config_version(
    doc: &mut ConfigDoc,
    prefill: Option<Vec<String>>,
) -> Result<(), WorkspaceEditError> {
    let mut edited = doc.clone();
    edited.set_value(&KeyPath::from(["config-version"]), 2.into())?;
    if let Some(names) = prefill {
        for (index, name) in names.iter().enumerate() {
            if names.iter().take(index).any(|earlier| earlier == name) {
                return Err(WorkspaceEditError::Duplicate(name.clone()));
            }
        }
        edited.set_value(
            &persistent_path(),
            toml_edit::Array::from_iter(names).into(),
        )?;
    }
    *doc = edited;
    Ok(())
}

/// Appends a workspace, creating the array when absent.
///
/// # Errors
/// Refuses version 1, duplicate names, or a lossless edit failure.
pub fn add_persistent_workspace(doc: &mut ConfigDoc, name: &str) -> Result<(), WorkspaceEditError> {
    ensure_version_two(doc)?;
    let path = persistent_path();
    let mut edited = doc.clone();
    if let Some(array) = edited.get(&path).and_then(Item::as_array) {
        if array.iter().any(|value| value.as_str() == Some(name)) {
            return Err(WorkspaceEditError::Duplicate(name.to_owned()));
        }
        let len = array.len();
        edited.array_insert(&path, len, name.into())?;
    } else if edited.get(&path).is_none() {
        edited.set_value(&path, toml_edit::Array::from_iter([name]).into())?;
    } else {
        return Err(EditError::NotAnArray(path).into());
    }
    *doc = edited;
    Ok(())
}

/// Removes a persistent workspace by its position.
///
/// # Errors
/// Refuses version 1 or a lossless array edit failure.
pub fn remove_persistent_workspace(
    doc: &mut ConfigDoc,
    index: usize,
) -> Result<(), WorkspaceEditError> {
    ensure_version_two(doc)?;
    doc.array_remove(&persistent_path(), index)?;
    Ok(())
}

/// Reorders a workspace with its comments.
///
/// # Errors
/// Refuses version 1 or a lossless array edit failure.
pub fn move_persistent_workspace(
    doc: &mut ConfigDoc,
    from: usize,
    to: usize,
) -> Result<(), WorkspaceEditError> {
    ensure_version_two(doc)?;
    doc.array_move(&persistent_path(), from, to)?;
    Ok(())
}

/// Version 1's inferred names, deduplicated in binding order, then assignment order.
/// Includes both workspace switches and window moves, as AeroSpace does.
#[must_use]
pub fn inferred_persistent_workspaces(view: &crate::config_view::ConfigView) -> Vec<String> {
    view.inferred_workspaces.clone()
}

pub(crate) fn infer_workspaces(doc: &ConfigDoc) -> Vec<String> {
    let bindings = doc
        .get(&KeyPath::from(["mode"]))
        .and_then(Item::as_table_like)
        .into_iter()
        .flat_map(|modes| modes.iter())
        .filter_map(|(_, mode)| mode.as_table_like()?.get("binding")?.as_table_like())
        .flat_map(|bindings| bindings.iter())
        .flat_map(|(_, binding)| binding_workspaces(binding));
    let assignments = doc
        .get(&KeyPath::from(["workspace-to-monitor-force-assignment"]))
        .and_then(Item::as_table_like)
        .into_iter()
        .flat_map(|assignments| assignments.iter())
        .map(|(name, _)| name.to_owned());
    let mut names = Vec::new();
    for name in bindings.chain(assignments) {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    names
}

fn binding_workspaces(binding: &Item) -> Vec<String> {
    use crate::commands::{Command, WorkspaceTarget};
    let texts: Vec<&str> = match binding.as_str() {
        Some(text) => vec![text],
        None => binding
            .as_array()
            .map(|array| array.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default(),
    };
    let commands: Vec<Command> = texts.into_iter().flat_map(shell_commands).collect();
    let switches = commands.iter().filter_map(|command| match command {
        Command::Workspace {
            target: WorkspaceTarget::Name(name),
            ..
        } => Some(name),
        _ => None,
    });
    let moves = commands.iter().filter_map(|command| match command {
        Command::MoveNodeToWorkspace {
            target: WorkspaceTarget::Name(name),
            ..
        } => Some(name),
        _ => None,
    });
    // AeroSpace collects switches before moves within each binding.
    switches
        .chain(moves)
        .map(|name| name.as_str().to_owned())
        .collect()
}

fn shell_commands(text: &str) -> Vec<crate::commands::Command> {
    let mut quote = None;
    let mut escaped = false;
    let mut segment = String::new();
    let mut segments = Vec::new();
    for character in text.chars() {
        if escaped {
            segment.push(character);
            escaped = false;
            continue;
        }
        if character == '\\' && quote != Some('\'') {
            segment.push(character);
            escaped = true;
            continue;
        }
        if let Some(delimiter) = quote {
            segment.push(character);
            if character == delimiter {
                quote = None;
            }
        } else if matches!(character, '\'' | '"') {
            quote = Some(character);
            segment.push(character);
        } else if matches!(character, ';' | '|' | '&' | '(' | ')') {
            // exec-and-forget owns the remainder as a shell command, not AeroSpace commands.
            if segment.trim_start().starts_with("exec-and-forget ") {
                break;
            }
            segments.push(std::mem::take(&mut segment));
        } else {
            segment.push(character);
        }
    }
    segments.push(segment);
    segments
        .into_iter()
        .filter_map(|text| crate::commands::Command::parse(text.trim()).ok())
        .collect()
}
