//! Ordered window rules and the boundary between form-editable and preserved matchers.
//!
//! Rule shapes follow AeroSpace's [0.21.3 parser](https://github.com/nikitabobko/AeroSpace/blob/v0.21.3-Beta/Sources/AppBundle/config/parseOnWindowDetected.swift).

use std::ops::Range;

use toml_edit::{InlineTable, Item, TableLike, Value};

use crate::commands::{
    Command, ParsedScript, Script, ScriptShape, ScriptToken, format_script, quote, tokenize,
};
use crate::config_doc::{ConfigDoc, EditError, KeyPath};
use crate::config_view::ConfigView;

/// The source representation of `on-window-detected`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleContainer {
    /// Repeated `[[on-window-detected]]` sections.
    ArrayOfTables,
    /// An array of inline tables, including TOML 1.1 multiline tables.
    InlineArray,
}

impl RuleContainer {
    /// Detects the representation without changing the document.
    #[must_use]
    pub fn read(doc: &ConfigDoc) -> Option<Self> {
        match doc.get(&KeyPath::from(["on-window-detected"]))? {
            Item::ArrayOfTables(_) => Some(Self::ArrayOfTables),
            Item::Value(Value::Array(_)) => Some(Self::InlineArray),
            _ => None,
        }
    }
}

/// One form condition, using AeroSpace's `test` interpolation variables.
///
/// Operators follow the [test parser](https://github.com/nikitabobko/AeroSpace/blob/v0.21.3-Beta/Sources/Common/cmdArgs/impl/TestCmdArgs.swift).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Condition {
    /// `test %{app-bundle-id} = value`.
    AppBundleIdIs(String),
    /// Case-insensitive substring regex against `%{app-name}`.
    AppNameMatches(String),
    /// Case-insensitive substring regex against `%{window-title}`.
    WindowTitleMatches(String),
    /// `test %{workspace} = value`.
    WorkspaceIs(String),
}

/// The five supported legacy `if` keys, with missing keys kept distinct from empty values.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LegacyMatcher {
    /// Exact application bundle ID.
    pub app_id: Option<String>,
    /// Case-insensitive substring regex against the application name.
    pub app_name_regex_substring: Option<String>,
    /// Case-insensitive substring regex against the window title.
    pub window_title_regex_substring: Option<String>,
    /// Exact workspace name.
    pub workspace: Option<String>,
    /// Whether the callback runs during AeroSpace startup.
    pub during_aerospace_startup: Option<bool>,
}

/// A matcher the form can edit, or the reason its source must remain untouched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Matcher {
    /// An exact conjunction of modelled tests. Empty means `true` or an absent `if`.
    Conditions(Vec<Condition>),
    /// Legacy dotted keys or an inline `if` table.
    Legacy(LegacyMatcher),
    /// Unsupported syntax is kept in the document, never reconstructed from this view.
    ReadOnly {
        /// Explanation shown by the rule form.
        reason: String,
    },
}

/// A window callback in file order, with independent matcher and command classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowRule {
    /// Conditions controlling this callback.
    pub matcher: Matcher,
    /// Structured commands or the reason to preserve the original `run` item.
    pub run: ParsedScript,
    /// Explicit callback continuation flag, or AeroSpace's default when missing.
    pub check_further_callbacks: Option<bool>,
    /// Source range of the rule table, when known.
    pub span: Option<Range<usize>>,
}

pub(crate) fn rule_tables(item: Option<&Item>) -> Vec<&dyn TableLike> {
    match item {
        Some(Item::ArrayOfTables(tables)) => {
            tables.iter().map(|table| table as &dyn TableLike).collect()
        }
        Some(Item::Value(Value::Array(array))) => array
            .iter()
            .filter_map(Value::as_inline_table)
            .map(|table| table as &dyn TableLike)
            .collect(),
        _ => Vec::new(),
    }
}

pub(crate) fn read_rules(item: Option<&Item>) -> Vec<WindowRule> {
    let spans: Vec<_> = match item {
        Some(Item::ArrayOfTables(tables)) => tables.iter().map(toml_edit::Table::span).collect(),
        Some(Item::Value(Value::Array(array))) => array
            .iter()
            .filter(|value| value.is_inline_table())
            .map(Value::span)
            .collect(),
        _ => Vec::new(),
    };
    rule_tables(item)
        .into_iter()
        .zip(spans)
        .map(|(table, span)| {
            let matcher = if table
                .iter()
                .any(|(key, _)| !["if", "run", "check-further-callbacks"].contains(&key))
            {
                read_only("rule contains unknown keys")
            } else if table
                .get("check-further-callbacks")
                .is_some_and(|item| !item.is_bool())
            {
                read_only("check-further-callbacks is not a boolean")
            } else {
                read_matcher(table.get("if"))
            };
            WindowRule {
                matcher,
                run: table.get("run").map_or_else(
                    || ParsedScript::ReadOnly {
                        reason: "run is missing".to_owned(),
                    },
                    Script::parse,
                ),
                check_further_callbacks: table
                    .get("check-further-callbacks")
                    .and_then(Item::as_bool),
                span,
            }
        })
        .collect()
}

fn read_only(reason: impl Into<String>) -> Matcher {
    Matcher::ReadOnly {
        reason: reason.into(),
    }
}

fn read_matcher(item: Option<&Item>) -> Matcher {
    match item {
        None => Matcher::Conditions(Vec::new()),
        Some(item) if item.is_str() => match parse_conditions(item.as_str().unwrap_or_default()) {
            Ok(conditions) => Matcher::Conditions(conditions),
            Err(reason) => read_only(reason),
        },
        Some(item) if item.is_table_like() => read_legacy(item),
        Some(_) => read_only("if is neither a command string nor a legacy table"),
    }
}

fn read_legacy(item: &Item) -> Matcher {
    let mut matcher = LegacyMatcher::default();
    let Some(table) = item.as_table_like() else {
        return read_only("if is not a table");
    };
    for (key, value) in table.iter() {
        if key == "during-aerospace-startup" {
            let Some(value) = value.as_bool() else {
                return read_only("legacy during-aerospace-startup is not a boolean");
            };
            matcher.during_aerospace_startup = Some(value);
            continue;
        }
        let field = match key {
            "app-id" => &mut matcher.app_id,
            "app-name-regex-substring" => &mut matcher.app_name_regex_substring,
            "window-title-regex-substring" => &mut matcher.window_title_regex_substring,
            "workspace" => &mut matcher.workspace,
            _ => return read_only(format!("unknown legacy condition `{key}`")),
        };
        let Some(value) = value.as_str() else {
            return read_only(format!("legacy {key} is not a string"));
        };
        *field = Some(value.to_owned());
    }
    Matcher::Legacy(matcher)
}

fn parse_conditions(text: &str) -> Result<Vec<Condition>, String> {
    let tokens = tokenize(text, true).map_err(|error| error.reason)?;
    if matches!(tokens.as_slice(), [ScriptToken::Word(word)] if word == "true") {
        return Ok(Vec::new());
    }
    let mut conditions = Vec::new();
    for group in tokens.split(|token| *token == ScriptToken::And) {
        let [
            ScriptToken::Word(command),
            ScriptToken::Word(variable),
            ScriptToken::Word(operator),
            ScriptToken::Word(value),
        ] = group
        else {
            return Err("if is not a conjunction of modelled tests".to_owned());
        };
        if command != "test" || value.contains("%{") {
            return Err("if is not a conjunction of modelled tests".to_owned());
        }
        let condition = match (variable.as_str(), operator.as_str()) {
            ("%{app-bundle-id}", "=") => Condition::AppBundleIdIs(value.clone()),
            ("%{app-name}", "~=") => Condition::AppNameMatches(value.clone()),
            ("%{window-title}", "~=") => Condition::WindowTitleMatches(value.clone()),
            ("%{workspace}", "=") => Condition::WorkspaceIs(value.clone()),
            _ => return Err("test uses an unmodelled variable or operator".to_owned()),
        };
        conditions.push(condition);
    }
    Ok(conditions)
}

/// A refused rule edit. Every failure leaves the source document unchanged.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RuleEditError {
    /// A matcher or script cannot be represented by the form.
    #[error("rule {index} is read-only: {reason}")]
    ReadOnly {
        /// Zero-based rule index.
        index: usize,
        /// Reason the original bytes must be preserved.
        reason: String,
    },
    /// The caller tried to convert an existing matcher to a different syntax.
    #[error("rule {0} must keep its legacy or command matcher syntax")]
    MatcherStyle(usize),
    /// A new condition cannot round-trip through AeroSpace's quoting rules.
    #[error("condition cannot be represented: {0}")]
    Unrepresentable(String),
    /// A lossless document edit failed.
    #[error(transparent)]
    Edit(#[from] EditError),
}

/// Appends a rule, retaining the existing container or creating `[[on-window-detected]]`.
///
/// # Errors
/// Refuses unsupported matchers, unrepresentable conditions, or lossless edit failures.
pub fn add_rule(
    doc: &mut ConfigDoc,
    matcher: &Matcher,
    commands: &[Command],
    check_further_callbacks: Option<bool>,
) -> Result<(), RuleEditError> {
    let mut rule = InlineTable::new();
    match matcher {
        Matcher::Conditions(conditions) => {
            rule.insert("if", format_conditions(conditions)?);
        }
        Matcher::Legacy(legacy) => {
            rule.insert("if", legacy_table(legacy));
        }
        Matcher::ReadOnly { reason } => {
            return Err(RuleEditError::ReadOnly {
                index: rule_count(doc),
                reason: reason.clone(),
            });
        }
    }
    rule.insert("run", format_script(commands, ScriptShape::String));
    if let Some(check) = check_further_callbacks {
        rule.insert("check-further-callbacks", check.into());
    }
    let mut edited = doc.clone();
    let path = rule_path();
    if edited.get(&path).is_none() {
        edited.insert_table_array(&path)?;
    }
    edited.array_insert(&path, rule_count(doc), Value::InlineTable(rule))?;
    *doc = edited;
    Ok(())
}

/// Changes a modelled rule in place, preserving legacy syntax and `run` string/array shape.
/// Unchanged fields retain their original text. An absent `if` stays absent for empty conditions.
///
/// # Errors
/// Refuses read-only matchers or scripts, syntax conversions, and lossless edit failures.
pub fn update_rule(
    doc: &mut ConfigDoc,
    index: usize,
    matcher: &Matcher,
    commands: &[Command],
    check_further_callbacks: Option<bool>,
) -> Result<(), RuleEditError> {
    let view = ConfigView::from_doc(doc);
    let rule = view
        .window_rules
        .get(index)
        .ok_or_else(|| missing_rule(index, view.window_rules.len()))?;
    for candidate in [&rule.matcher, matcher] {
        if let Matcher::ReadOnly { reason } = candidate {
            return Err(RuleEditError::ReadOnly {
                index,
                reason: reason.clone(),
            });
        }
    }
    let (old_commands, shape) = match &rule.run {
        ParsedScript::Editable { commands, shape } => (commands, shape),
        ParsedScript::ReadOnly { reason } => {
            return Err(RuleEditError::ReadOnly {
                index,
                reason: reason.clone(),
            });
        }
    };
    let changes = match (&rule.matcher, matcher) {
        (Matcher::Conditions(old), Matcher::Conditions(new)) => {
            if old == new {
                Vec::new()
            } else {
                vec![(vec!["if"], Some(format_conditions(new)?))]
            }
        }
        (Matcher::Legacy(old), Matcher::Legacy(new)) => legacy_changes(old, new),
        _ => return Err(RuleEditError::MatcherStyle(index)),
    };
    doc.array_edit_table(&rule_path(), index, |document, root| {
        for (segments, value) in changes {
            let path = segments
                .into_iter()
                .fold(root.clone(), |path, segment| path.join(segment));
            set_optional(document, &path, value)?;
        }
        if matches!(matcher, Matcher::Legacy(_))
            && document.get(&root.join("if")).is_none_or(|item| {
                item.as_table_like()
                    .is_some_and(|table| table.is_empty() && table.is_dotted())
            })
        {
            document.set_value(&root.join("if"), Value::InlineTable(InlineTable::new()))?;
        }
        if old_commands != commands {
            document.set_value(&root.join("run"), format_script(commands, *shape))?;
        }
        if rule.check_further_callbacks != check_further_callbacks {
            set_optional(
                document,
                &root.join("check-further-callbacks"),
                check_further_callbacks.map(Value::from),
            )?;
        }
        Ok(())
    })?;
    Ok(())
}

/// Removes a rule, including unsupported syntax, without rewriting any remaining rule.
///
/// # Errors
/// Returns a lossless edit error for a missing rule or unsupported document shape.
pub fn remove_rule(doc: &mut ConfigDoc, index: usize) -> Result<(), RuleEditError> {
    doc.array_remove(&rule_path(), index)?;
    Ok(())
}

/// Moves a rule to a zero-based destination, carrying its own comments and raw values.
///
/// # Errors
/// Returns a lossless edit error for missing indices or unsupported document shapes.
pub fn move_rule(doc: &mut ConfigDoc, from: usize, to: usize) -> Result<(), RuleEditError> {
    doc.array_move(&rule_path(), from, to)?;
    Ok(())
}

fn rule_path() -> KeyPath {
    KeyPath::from(["on-window-detected"])
}

fn rule_count(doc: &ConfigDoc) -> usize {
    let item = doc.get(&rule_path());
    item.and_then(Item::as_array).map_or_else(
        || {
            item.and_then(Item::as_array_of_tables)
                .map_or(0, toml_edit::ArrayOfTables::len)
        },
        toml_edit::Array::len,
    )
}

fn missing_rule(index: usize, len: usize) -> RuleEditError {
    EditError::IndexOutOfBounds {
        path: rule_path(),
        index,
        len,
    }
    .into()
}

fn format_conditions(conditions: &[Condition]) -> Result<Value, RuleEditError> {
    let text = if conditions.is_empty() {
        "true".to_owned()
    } else {
        conditions
            .iter()
            .map(|condition| {
                let (variable, operator, value) = match condition {
                    Condition::AppBundleIdIs(value) => ("app-bundle-id", "=", value),
                    Condition::AppNameMatches(value) => ("app-name", "~=", value),
                    Condition::WindowTitleMatches(value) => ("window-title", "~=", value),
                    Condition::WorkspaceIs(value) => ("workspace", "=", value),
                };
                format!("test %{{{variable}}} {operator} {}", quote(value))
            })
            .collect::<Vec<_>>()
            .join(" && ")
    };
    if parse_conditions(&text).as_deref() != Ok(conditions) {
        return Err(RuleEditError::Unrepresentable(
            "value uses interpolation or incompatible quotes".to_owned(),
        ));
    }
    if text.contains(['\'', '\n', '\r']) {
        Ok(Value::from(text))
    } else {
        format!("'{text}'")
            .parse::<Value>()
            .map_err(|error| RuleEditError::Unrepresentable(error.to_string()))
    }
}

fn legacy_fields(legacy: &LegacyMatcher) -> [(&'static str, Option<Value>); 5] {
    [
        ("app-id", legacy.app_id.clone().map(Value::from)),
        (
            "app-name-regex-substring",
            legacy.app_name_regex_substring.clone().map(Value::from),
        ),
        (
            "window-title-regex-substring",
            legacy.window_title_regex_substring.clone().map(Value::from),
        ),
        ("workspace", legacy.workspace.clone().map(Value::from)),
        (
            "during-aerospace-startup",
            legacy.during_aerospace_startup.map(Value::from),
        ),
    ]
}

fn legacy_table(legacy: &LegacyMatcher) -> Value {
    Value::InlineTable(
        legacy_fields(legacy)
            .into_iter()
            .filter_map(|(key, value)| value.map(|value| (key, value)))
            .collect(),
    )
}

fn legacy_changes(
    old: &LegacyMatcher,
    new: &LegacyMatcher,
) -> Vec<(Vec<&'static str>, Option<Value>)> {
    legacy_fields(old)
        .into_iter()
        .zip(legacy_fields(new))
        .filter_map(|((_, old), (key, new))| {
            (old.as_ref().map(ToString::to_string) != new.as_ref().map(ToString::to_string))
                .then_some((vec!["if", key], new))
        })
        .collect()
}

fn set_optional(
    doc: &mut ConfigDoc,
    path: &KeyPath,
    value: Option<Value>,
) -> Result<(), EditError> {
    match value {
        Some(value) => doc.set_value(path, value),
        None if doc.get(path).is_some() => doc.remove(path),
        None => Ok(()),
    }
}
