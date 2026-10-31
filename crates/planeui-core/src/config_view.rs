//! Typed read-only view built from a `ConfigDoc`, re-derived after every edit. A bad value
//! never fails the whole view; it becomes [`Field::Invalid`] so the rest of the file stays
//! editable.

use std::ops::Range;

use toml_edit::{Document, Item, Value};

use crate::config_doc::{ConfigDoc, KeyPath};

/// A value with the byte range it came from in the rendered document, for diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Located<T> {
    /// The value.
    pub value: T,
    /// Byte range in the document text, when known.
    pub span: Option<Range<usize>>,
}

/// A setting as found in the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Field<T> {
    /// The key is not in the file; AeroSpace uses its default.
    Missing,
    /// The key holds a value of the expected type.
    Valid(Located<T>),
    /// The key holds something AeroSpace will not accept, shown as written.
    Invalid {
        /// Byte range of the value, when known.
        span: Option<Range<usize>>,
        /// The value as written in the file.
        raw: String,
    },
}

impl<T> Field<T> {
    /// The value when the field is valid.
    #[must_use]
    pub fn value(&self) -> Option<&T> {
        match self {
            Self::Valid(located) => Some(&located.value),
            Self::Missing | Self::Invalid { .. } => None,
        }
    }

    /// Whether the file holds a value of the wrong type here.
    #[must_use]
    pub fn is_invalid(&self) -> bool {
        matches!(self, Self::Invalid { .. })
    }
}

/// The type AeroSpace expects for a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueType {
    /// `true` or `false`.
    Bool,
    /// An integer.
    Integer,
    /// A string.
    String,
    /// An array of strings.
    StringArray,
    /// One command string or an array of them.
    Commands,
    /// A table (`[header]`, dotted keys, or inline).
    Table,
    /// An array of tables (`[[header]]` or an inline array of inline tables).
    TableArray,
}

impl ValueType {
    /// Whether `item` has this type.
    #[must_use]
    pub fn matches(self, item: &Item) -> bool {
        let is_strings = |value: &Value| {
            value
                .as_array()
                .is_some_and(|array| array.iter().all(Value::is_str))
        };
        match self {
            Self::Bool => item.as_bool().is_some(),
            Self::Integer => item.as_integer().is_some(),
            Self::String => item.as_str().is_some(),
            Self::StringArray => item.as_value().is_some_and(is_strings),
            Self::Commands => item
                .as_value()
                .is_some_and(|value| value.is_str() || is_strings(value)),
            Self::Table => item.is_table_like(),
            Self::TableArray => {
                item.is_array_of_tables()
                    || item
                        .as_array()
                        .is_some_and(|array| array.iter().all(Value::is_inline_table))
            }
        }
    }

    /// How the type reads in an error message.
    #[must_use]
    pub fn describe(self) -> &'static str {
        match self {
            Self::Bool => "true or false",
            Self::Integer => "an integer",
            Self::String => "a string",
            Self::StringArray => "an array of strings",
            Self::Commands => "a command or an array of commands",
            Self::Table => "a table",
            Self::TableArray => "an array of tables",
        }
    }
}

/// Whether PlaneUI edits a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Edited through a form.
    Edited,
    /// Valid AeroSpace config PlaneUI shows read-only and never rewrites.
    OutOfScope,
    /// Accepted by older AeroSpace versions only.
    Deprecated,
}

/// One entry of the key catalogue (SPEC "Config surface").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeySpec {
    /// Dotted key path from the root.
    pub path: &'static [&'static str],
    /// Expected type, when AeroSpace has one.
    pub value_type: Option<ValueType>,
    /// Whether PlaneUI edits it.
    pub scope: Scope,
}

const fn spec(path: &'static [&'static str], value_type: ValueType, scope: Scope) -> KeySpec {
    KeySpec {
        path,
        value_type: Some(value_type),
        scope,
    }
}

/// Every key AeroSpace 0.21 reads at the levels PlaneUI checks: the root, and the children of
/// `exec`, `key-mapping` and `focus-follows-mouse`. Deeper tables (`gaps`, `mode`, rules) get
/// their own checks in later milestones.
pub const KEY_CATALOGUE: &[KeySpec] = &[
    spec(&["config-version"], ValueType::Integer, Scope::Edited),
    spec(&["start-at-login"], ValueType::Bool, Scope::Edited),
    spec(&["auto-reload-config"], ValueType::Bool, Scope::Edited),
    spec(
        &["enable-normalization-flatten-containers"],
        ValueType::Bool,
        Scope::Edited,
    ),
    spec(
        &["enable-normalization-opposite-orientation-for-nested-containers"],
        ValueType::Bool,
        Scope::Edited,
    ),
    spec(&["accordion-padding"], ValueType::Integer, Scope::Edited),
    spec(
        &["default-root-container-layout"],
        ValueType::String,
        Scope::Edited,
    ),
    spec(
        &["default-root-container-orientation"],
        ValueType::String,
        Scope::Edited,
    ),
    spec(
        &["automatically-unhide-macos-hidden-apps"],
        ValueType::Bool,
        Scope::Edited,
    ),
    spec(
        &["persistent-workspaces"],
        ValueType::StringArray,
        Scope::Edited,
    ),
    spec(&["gaps"], ValueType::Table, Scope::Edited),
    spec(&["mode"], ValueType::Table, Scope::Edited),
    spec(
        &["workspace-to-monitor-force-assignment"],
        ValueType::Table,
        Scope::Edited,
    ),
    spec(
        &["on-window-detected"],
        ValueType::TableArray,
        Scope::Edited,
    ),
    spec(&["key-mapping"], ValueType::Table, Scope::Edited),
    spec(&["key-mapping", "preset"], ValueType::String, Scope::Edited),
    spec(
        &["key-mapping", "key-notation-to-key-code"],
        ValueType::Table,
        Scope::OutOfScope,
    ),
    spec(&["focus-follows-mouse"], ValueType::Table, Scope::Edited),
    spec(
        &["focus-follows-mouse", "enabled"],
        ValueType::Bool,
        Scope::Edited,
    ),
    spec(
        &["after-startup-command"],
        ValueType::Commands,
        Scope::OutOfScope,
    ),
    spec(
        &["on-focus-changed"],
        ValueType::Commands,
        Scope::OutOfScope,
    ),
    spec(
        &["on-focused-monitor-changed"],
        ValueType::Commands,
        Scope::OutOfScope,
    ),
    spec(&["on-mode-changed"], ValueType::Commands, Scope::OutOfScope),
    spec(
        &["exec-on-workspace-change"],
        ValueType::StringArray,
        Scope::OutOfScope,
    ),
    spec(&["exec"], ValueType::Table, Scope::OutOfScope),
    spec(
        &["exec", "inherit-env-vars"],
        ValueType::Bool,
        Scope::OutOfScope,
    ),
    spec(&["exec", "env-vars"], ValueType::Table, Scope::OutOfScope),
    KeySpec {
        path: &["after-login-command"],
        value_type: None,
        scope: Scope::Deprecated,
    },
    KeySpec {
        path: &["non-empty-workspaces-root-containers-layout-on-startup"],
        value_type: None,
        scope: Scope::Deprecated,
    },
    KeySpec {
        path: &["indent-for-nested-containers-with-the-same-orientation"],
        value_type: None,
        scope: Scope::Deprecated,
    },
];

/// Tables whose children the catalogue lists; children of other known tables are not checked.
const CATALOGUED_TABLES: &[&str] = &["exec", "key-mapping", "focus-follows-mouse"];

/// The catalogue entry for `path`, if AeroSpace knows the key.
#[must_use]
pub fn key_spec(path: &KeyPath) -> Option<&'static KeySpec> {
    KEY_CATALOGUE.iter().find(|spec| {
        spec.path
            .iter()
            .copied()
            .eq(path.segments().iter().map(String::as_str))
    })
}

/// One key found in the file with its catalogue entry (`None` when unknown).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CataloguedKey {
    /// Where the key is.
    pub key: Located<KeyPath>,
    /// What AeroSpace expects there, or `None` for an unknown key.
    pub spec: Option<&'static KeySpec>,
}

/// General settings (filled in by PUI-042).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct General {
    /// `config-version`.
    pub config_version: Field<i64>,
    /// `accordion-padding`.
    pub accordion_padding: Field<i64>,
    /// `auto-reload-config`.
    pub auto_reload_config: Field<bool>,
}

/// Gaps (filled in by PUI-040).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Gaps {}

/// A binding mode (filled in by PUI-032).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mode {
    /// The mode's name, e.g. `main`.
    pub name: Located<String>,
}

/// A window rule (filled in by PUI-051).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WindowRule {}

/// A workspace-to-monitor assignment (filled in by PUI-054).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspaceAssignment {}

/// Typed, read-only view of a config document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigView {
    /// General settings.
    pub general: General,
    /// Gaps.
    pub gaps: Gaps,
    /// Binding modes in file order.
    pub modes: Vec<Mode>,
    /// Window rules in file order.
    pub window_rules: Vec<WindowRule>,
    /// Workspace-to-monitor assignments in file order.
    pub workspace_to_monitor: Vec<WorkspaceAssignment>,
    /// Every catalogued-level key in the file, in file order.
    pub keys: Vec<CataloguedKey>,
    /// Keys PlaneUI shows but does not edit: callbacks, `exec`, key-code aliases, deprecated
    /// and unknown keys.
    pub out_of_scope: Vec<Located<KeyPath>>,
}

impl ConfigView {
    /// Builds the view; never fails as a whole.
    #[must_use]
    pub fn from_doc(document: &ConfigDoc) -> Self {
        let spanned = Document::parse(document.to_string()).ok();
        let root = spanned.as_ref().map(Document::as_item);
        let spans = |path: &[&str]| root.and_then(|root| value_span(root, path));

        let field = |name: &str| (document.get(&KeyPath::from([name])), spans(&[name]));
        let general = General {
            config_version: typed(field("config-version"), Item::as_integer),
            accordion_padding: typed(field("accordion-padding"), Item::as_integer),
            auto_reload_config: typed(field("auto-reload-config"), Item::as_bool),
        };

        let modes = document
            .get(&KeyPath::from(["mode"]))
            .and_then(Item::as_table_like)
            .map(|modes| {
                modes
                    .iter()
                    .map(|(name, _)| Mode {
                        name: Located {
                            value: name.to_owned(),
                            span: root.and_then(|root| key_span(root, &["mode", name])),
                        },
                    })
                    .collect()
            })
            .unwrap_or_default();

        let keys = catalogued_keys(document, root);
        let out_of_scope = keys
            .iter()
            .filter(|key| key.spec.is_none_or(|spec| spec.scope != Scope::Edited))
            .map(|key| key.key.clone())
            .collect();

        Self {
            general,
            gaps: Gaps::default(),
            modes,
            window_rules: Vec::new(),
            workspace_to_monitor: Vec::new(),
            keys,
            out_of_scope,
        }
    }
}

fn typed<T: Clone>(
    (item, span): (Option<&Item>, Option<Range<usize>>),
    read: impl Fn(&Item) -> Option<T>,
) -> Field<T> {
    match item {
        None => Field::Missing,
        Some(item) => match read(item) {
            Some(value) => Field::Valid(Located { value, span }),
            None => Field::Invalid {
                span,
                raw: item.to_string().trim().to_owned(),
            },
        },
    }
}

fn catalogued_keys(document: &ConfigDoc, spanned: Option<&Item>) -> Vec<CataloguedKey> {
    let mut keys = Vec::new();
    let mut visit = |path: KeyPath| {
        let segments: Vec<&str> = path.segments().iter().map(String::as_str).collect();
        let span = spanned.and_then(|root| key_span(root, &segments));
        keys.push(CataloguedKey {
            spec: key_spec(&path),
            key: Located { value: path, span },
        });
    };
    for (name, item) in document.document().as_table() {
        let path = KeyPath::from([name]);
        visit(path.clone());
        if CATALOGUED_TABLES.contains(&name)
            && let Some(table) = item.as_table_like()
        {
            for (child, _) in table.iter() {
                visit(path.join(child));
            }
        }
    }
    keys
}

fn lookup<'a>(root: &'a Item, path: &[&str]) -> Option<(&'a toml_edit::Key, &'a Item)> {
    let (leaf, parents) = path.split_last()?;
    let parent = parents
        .iter()
        .try_fold(root, |item, segment| item.as_table_like()?.get(segment))?;
    parent.as_table_like()?.get_key_value(leaf)
}

fn key_span(root: &Item, path: &[&str]) -> Option<Range<usize>> {
    lookup(root, path).and_then(|(key, _)| key.span())
}

fn value_span(root: &Item, path: &[&str]) -> Option<Range<usize>> {
    lookup(root, path).and_then(|(key, item)| item.span().or_else(|| key.span()))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::*;

    fn view(source: &str) -> ConfigView {
        ConfigView::from_doc(&ConfigDoc::parse(source).unwrap())
    }

    fn fixture(name: &str) -> String {
        fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures")
                .join(name),
        )
        .unwrap()
    }

    #[test]
    fn callbacks_and_exec_are_out_of_scope() {
        let view = view(&fixture("callbacks-and-exec.toml"));
        let listed: Vec<String> = view
            .out_of_scope
            .iter()
            .map(|key| key.value.to_string())
            .collect();
        assert_eq!(
            listed,
            [
                "after-login-command",
                "after-startup-command",
                "exec-on-workspace-change",
                "on-focus-changed",
                "on-focused-monitor-changed",
                "exec",
                "exec.inherit-env-vars",
                "exec.env-vars",
                "unknown-future-section",
            ]
        );
        assert!(view.out_of_scope.iter().all(|key| key.span.is_some()));
    }

    #[test]
    fn wrong_typed_value_is_marked_invalid() {
        let source = "accordion-padding = 'wide'\nauto-reload-config = true\n";
        let view = view(source);
        assert_eq!(
            view.general.accordion_padding,
            Field::Invalid {
                span: Some(20..26),
                raw: "'wide'".to_owned()
            }
        );
        assert_eq!(view.general.auto_reload_config.value(), Some(&true));
        assert_eq!(view.general.config_version, Field::Missing);
    }

    #[test]
    fn key_code_aliases_are_out_of_scope() {
        let view = view("[key-mapping]\npreset = 'qwerty'\nkey-notation-to-key-code.q = 'a'\n");
        let listed: Vec<String> = view
            .out_of_scope
            .iter()
            .map(|key| key.value.to_string())
            .collect();
        assert_eq!(listed, ["key-mapping.key-notation-to-key-code"]);
    }

    #[test]
    fn modes_are_listed_in_file_order() {
        let view = view(&fixture("table-style.toml"));
        let names: Vec<&str> = view
            .modes
            .iter()
            .map(|mode| mode.name.value.as_str())
            .collect();
        assert_eq!(names, ["main", "service"]);
    }
}
