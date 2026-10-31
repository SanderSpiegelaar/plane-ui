//! Typed read-only view built from a `ConfigDoc`, re-derived after every edit. A bad value
//! never fails the whole view; it becomes [`Field::Invalid`] so the rest of the file stays
//! editable.

use std::ops::Range;

use toml_edit::{Document, Item, Value};

use crate::commands::{ParsedScript, Script};
use crate::config_doc::{ConfigDoc, KeyPath};
use crate::gaps::{GapField, GapValue};
use crate::keys::{Chord, ChordError, KeyMapping, ModeName};
use crate::settings::{Setting, SettingValue};

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

/// Typed general settings. Invalid fields do not hide valid neighbours.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct General {
    /// `config-version`.
    pub config_version: Field<i64>,
    settings: Vec<(Setting, Field<SettingValue>)>,
}

impl General {
    /// The field for a setting, including its source span.
    #[must_use]
    pub fn setting(&self, setting: Setting) -> &Field<SettingValue> {
        self.settings
            .iter()
            .find_map(|(key, field)| (*key == setting).then_some(field))
            .unwrap_or(&Field::Missing)
    }
}

/// The six gap leaves, including read-only per-monitor values.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Gaps {
    /// Space between columns.
    pub inner_horizontal: GapValue,
    /// Space between rows.
    pub inner_vertical: GapValue,
    /// Left edge.
    pub outer_left: GapValue,
    /// Right edge.
    pub outer_right: GapValue,
    /// Top edge.
    pub outer_top: GapValue,
    /// Bottom edge.
    pub outer_bottom: GapValue,
}

impl Gaps {
    /// Looks up a leaf in form order.
    #[must_use]
    pub fn get(&self, field: GapField) -> &GapValue {
        match field {
            GapField::InnerHorizontal => &self.inner_horizontal,
            GapField::InnerVertical => &self.inner_vertical,
            GapField::OuterLeft => &self.outer_left,
            GapField::OuterRight => &self.outer_right,
            GapField::OuterTop => &self.outer_top,
            GapField::OuterBottom => &self.outer_bottom,
        }
    }

    fn from_doc(doc: &ConfigDoc) -> Self {
        Self {
            inner_horizontal: GapValue::read(doc, GapField::InnerHorizontal),
            inner_vertical: GapValue::read(doc, GapField::InnerVertical),
            outer_left: GapValue::read(doc, GapField::OuterLeft),
            outer_right: GapValue::read(doc, GapField::OuterRight),
            outer_top: GapValue::read(doc, GapField::OuterTop),
            outer_bottom: GapValue::read(doc, GapField::OuterBottom),
        }
    }
}

/// A binding mode, including malformed modes that must remain visible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mode {
    /// The original name, including empty TOML keys that `ModeName` cannot represent.
    pub name: Located<String>,
    /// Bindings in file order, even when a nested binding makes the mode read-only.
    pub bindings: Vec<Binding>,
    /// False for an invalid name, a non-table mode or `binding`, or nested binding tables.
    /// Missing `binding` is editable and empty. Invalid scalar scripts affect only their row.
    pub editable: bool,
}

/// A binding as written, with independently parsed chord and script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    /// Original key notation, without TOML quoting.
    pub raw_key: String,
    /// Parsed with the view's key mapping, or the error to display inline.
    pub chord: Result<Chord, ChordError>,
    /// Structured commands, or the reason the original script must remain read-only.
    pub script: ParsedScript,
    /// Trimmed trailing value comment, including its leading `#` as shown to users.
    pub inline_comment: Option<String>,
    /// Byte range of the binding's value, when known.
    pub span: Option<Range<usize>>,
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
    /// Explicit persistent workspaces, in array order.
    pub persistent_workspaces: Field<Vec<String>>,
    pub(crate) inferred_workspaces: Vec<String>,
    /// Gaps.
    pub gaps: Gaps,
    /// Binding modes with `main` first, then the remaining modes in file order.
    pub modes: Vec<Mode>,
    /// The mapping shared by chord parsing, validation, and the UI.
    pub key_mapping: KeyMapping,
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
            settings: Setting::ALL
                .into_iter()
                .map(|setting| {
                    let path = setting.spec().path;
                    let item = document.get(&KeyPath::new(path.iter().copied()));
                    (
                        setting,
                        typed((item, spans(path)), |item| setting.read(item)),
                    )
                })
                .collect(),
        };

        let key_mapping = KeyMapping::from_doc(document);
        let mut modes: Vec<Mode> = document
            .get(&KeyPath::from(["mode"]))
            .and_then(Item::as_table_like)
            .map(|modes| {
                modes
                    .iter()
                    .map(|(name, item)| read_mode(name, item, root, &key_mapping))
                    .collect()
            })
            .unwrap_or_default();
        if let Some(index) = modes.iter().position(|mode| mode.name.value == "main") {
            let main = modes.remove(index);
            modes.insert(0, main);
        }

        let keys = catalogued_keys(document, root);
        let out_of_scope = keys
            .iter()
            .filter(|key| key.spec.is_none_or(|spec| spec.scope != Scope::Edited))
            .map(|key| key.key.clone())
            .collect();

        Self {
            general,
            persistent_workspaces: typed(field("persistent-workspaces"), |item| {
                item.as_array()?
                    .iter()
                    .map(|value| value.as_str().map(str::to_owned))
                    .collect()
            }),
            inferred_workspaces: crate::settings::infer_workspaces(document),
            gaps: Gaps::from_doc(document),
            modes,
            key_mapping,
            window_rules: Vec::new(),
            workspace_to_monitor: Vec::new(),
            keys,
            out_of_scope,
        }
    }
}

fn read_mode(name: &str, item: &Item, root: Option<&Item>, mapping: &KeyMapping) -> Mode {
    let table = item.as_table_like();
    let binding_item = table.and_then(|table| table.get("binding"));
    let binding_table = binding_item.and_then(Item::as_table_like);
    let editable = ModeName::new(name).is_ok()
        && table.is_some()
        && binding_item.is_none_or(Item::is_table_like)
        && binding_table.is_none_or(|table| {
            table
                .iter()
                .all(|(_, item)| !item.is_table_like() && !item.is_array_of_tables())
        });
    let bindings = binding_table
        .map(|table| {
            table
                .iter()
                .map(|(raw_key, item)| Binding {
                    raw_key: raw_key.to_owned(),
                    chord: Chord::parse(raw_key, mapping),
                    script: Script::parse(item),
                    inline_comment: item
                        .as_value()
                        .and_then(|value| value.decor().suffix())
                        .and_then(toml_edit::RawString::as_str)
                        .map(str::trim)
                        .filter(|suffix| suffix.starts_with('#'))
                        .map(str::to_owned),
                    span: root
                        .and_then(|root| value_span(root, &["mode", name, "binding", raw_key])),
                })
                .collect()
        })
        .unwrap_or_default();
    Mode {
        name: Located {
            value: name.to_owned(),
            span: root.and_then(|root| key_span(root, &["mode", name])),
        },
        bindings,
        editable,
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
    use std::fmt::Write;
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
            *view.general.setting(Setting::AccordionPadding),
            Field::Invalid {
                span: Some(20..26),
                raw: "'wide'".to_owned()
            }
        );
        assert_eq!(
            view.general.setting(Setting::AutoReloadConfig).value(),
            Some(&SettingValue::Bool(true))
        );
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

    fn bindings_summary(source: &str) -> String {
        let view = view(source);
        let mut summary = String::new();
        for mode in view.modes {
            writeln!(
                summary,
                "mode {:?} editable={}",
                mode.name.value, mode.editable
            )
            .unwrap();
            for binding in mode.bindings {
                let chord = match binding.chord {
                    Ok(chord) => chord.normalized().to_string(),
                    Err(error) => format!("error: {error}"),
                };
                let script = match binding.script {
                    ParsedScript::Editable { commands, shape } => format!(
                        "editable {shape:?}: {}",
                        commands
                            .iter()
                            .map(crate::commands::Command::format)
                            .collect::<Vec<_>>()
                            .join(" -> ")
                    ),
                    ParsedScript::ReadOnly { reason } => format!("read-only: {reason}"),
                };
                writeln!(summary, "  {} [{chord}] {script}", binding.raw_key).unwrap();
                if let Some(comment) = binding.inline_comment {
                    writeln!(summary, "    {comment}").unwrap();
                }
                assert!(binding.span.is_some());
            }
        }
        summary
    }

    #[test]
    fn bindings_views() {
        for name in [
            "aerospace-default",
            "sander",
            "dotted-keys",
            "inline-tables",
        ] {
            let source = fixture(&format!("{name}.toml"));
            insta::assert_snapshot!(format!("bindings_{name}"), bindings_summary(&source));
        }
    }

    #[test]
    fn main_is_first_and_comments_and_spans_are_preserved() {
        let source = "[mode.service.binding]\nesc = 'mode main'\n\
            [mode.other]\n\
            [mode.main]\nbinding.alt-h = 'focus left'  # Go left  \n";
        let view = view(source);
        let names: Vec<_> = view
            .modes
            .iter()
            .map(|mode| mode.name.value.as_str())
            .collect();
        assert_eq!(names, ["main", "service", "other"]);
        let main = view.modes.first().unwrap();
        assert!(main.editable);
        assert_eq!(&source[main.name.span.clone().unwrap()], "main");
        let binding = main.bindings.first().unwrap();
        assert_eq!(binding.inline_comment.as_deref(), Some("# Go left"));
        assert_eq!(&source[binding.span.clone().unwrap()], "'focus left'");
        assert_eq!(view.modes.last().unwrap().bindings, []);
    }

    #[test]
    fn unsafe_mode_shapes_are_read_only_without_hiding_rows() {
        for source in [
            "mode.main = false",
            "mode.main.binding = 42",
            "[mode.main.binding]\nalt-h = 'focus left'\nalt-l.command = 'focus right'",
            "mode.main.binding = { alt-h = 'focus left', alt-l = { command = 'focus right' } }",
            "[[mode.main.binding.alt-h]]\ncommand = 'focus left'",
            "[mode.'']\n",
        ] {
            let view = view(source);
            let mode = view.modes.first().unwrap();
            assert!(!mode.editable, "{source}");
            if source.contains("alt-l") {
                assert_eq!(mode.bindings.len(), 2);
                assert!(matches!(
                    mode.bindings.first().unwrap().script,
                    ParsedScript::Editable { .. }
                ));
                assert!(matches!(
                    mode.bindings.last().unwrap().script,
                    ParsedScript::ReadOnly { .. }
                ));
            }
        }
    }

    #[test]
    fn invalid_rows_do_not_make_other_bindings_read_only() {
        let view = view("[mode.main.binding]\nalt-unknown = 42\nalt-h = 'focus left'\n");
        let mode = view.modes.first().unwrap();
        assert!(mode.editable);
        let invalid = mode.bindings.first().unwrap();
        assert!(invalid.chord.is_err());
        assert!(matches!(invalid.script, ParsedScript::ReadOnly { .. }));
        assert!(mode.bindings.last().unwrap().chord.is_ok());
    }

    #[test]
    fn chords_use_the_mapping_kept_on_the_view() {
        let view = view(
            "key-mapping.key-notation-to-key-code.homeKey = 'h'\n\
            [mode.main.binding]\nalt-homeKey = 'focus left'\n",
        );
        let binding = view.modes.first().unwrap().bindings.first().unwrap();
        assert_eq!(
            binding.chord,
            Chord::parse("alt-homeKey", &view.key_mapping)
        );
        assert_eq!(
            binding.chord.as_ref().unwrap().normalized().to_string(),
            "alt-h"
        );
    }
}
