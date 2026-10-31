//! Ordered window rules and the boundary between form-editable and preserved matchers.
//!
//! Rule shapes follow AeroSpace's [0.21.3 parser](https://github.com/nikitabobko/AeroSpace/blob/v0.21.3-Beta/Sources/AppBundle/config/parseOnWindowDetected.swift).

use std::ops::Range;

use toml_edit::{Item, TableLike, Value};

use crate::commands::{ParsedScript, Script, ScriptToken, tokenize};
use crate::config_doc::{ConfigDoc, KeyPath};

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
