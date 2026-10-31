//! Constant and per-monitor gap reads and lossless edits.

use toml_edit::{Array, InlineTable, Item, Key, Value};

use crate::config_doc::{ConfigDoc, EditError, KeyPath};
use crate::monitors::{MonitorEditError, MonitorPattern, ensure_pattern};

/// One of AeroSpace's six gap leaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GapField {
    /// Space between columns.
    InnerHorizontal,
    /// Space between rows.
    InnerVertical,
    /// Left screen edge.
    OuterLeft,
    /// Right screen edge.
    OuterRight,
    /// Top screen edge.
    OuterTop,
    /// Bottom screen edge.
    OuterBottom,
}

impl GapField {
    /// All gap leaves, in form order.
    pub const ALL: [Self; 6] = [
        Self::InnerHorizontal,
        Self::InnerVertical,
        Self::OuterLeft,
        Self::OuterRight,
        Self::OuterTop,
        Self::OuterBottom,
    ];
    /// The leaves changed by a linked edit.
    pub const OUTER: [Self; 4] = [
        Self::OuterLeft,
        Self::OuterRight,
        Self::OuterTop,
        Self::OuterBottom,
    ];

    /// The lossless document path.
    #[must_use]
    pub fn path(self) -> KeyPath {
        let (group, leaf) = match self {
            Self::InnerHorizontal => ("inner", "horizontal"),
            Self::InnerVertical => ("inner", "vertical"),
            Self::OuterLeft => ("outer", "left"),
            Self::OuterRight => ("outer", "right"),
            Self::OuterTop => ("outer", "top"),
            Self::OuterBottom => ("outer", "bottom"),
        };
        KeyPath::from(["gaps", group, leaf])
    }
}

/// Ordered monitor-specific gaps with a trailing fallback.
///
/// Mirrors [DynamicConfigValue.swift](https://github.com/nikitabobko/AeroSpace/blob/v0.21.3-Beta/Sources/AppBundle/config/DynamicConfigValue.swift).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PerMonitor {
    /// First matching monitor rule wins.
    pub rules: Vec<(MonitorPattern, u32)>,
    /// Gap when no rule matches.
    pub default: u32,
}

impl PerMonitor {
    /// Reads only AeroSpace's exact rule-array shape; malformed values remain read-only.
    #[must_use]
    pub fn parse(value: &Value) -> Option<Self> {
        let array = value.as_array()?;
        if array.len() < 2 {
            return None;
        }
        let default = unsigned(array.iter().last()?)?;
        let rules = array
            .iter()
            .take(array.len() - 1)
            .map(|value| {
                let rule = value.as_inline_table()?;
                if rule.len() != 1 {
                    return None;
                }
                let monitor = rule.get("monitor")?.as_inline_table()?;
                if monitor.len() != 1 {
                    return None;
                }
                let (pattern, value) = monitor.iter().next()?;
                Some((MonitorPattern::parse_str(pattern).ok()?, unsigned(value)?))
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Self { rules, default })
    }
}

fn unsigned(value: &Value) -> Option<u32> {
    u32::try_from(value.as_integer()?).ok()
}

/// A gap leaf as found in the document.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum GapValue {
    /// AeroSpace uses zero.
    #[default]
    Absent,
    /// A constant gap.
    Const(u32),
    /// An editable monitor-dependent gap.
    PerMonitor(PerMonitor),
    /// An unsupported value retained read-only, including malformed rule arrays.
    Invalid {
        /// The original value text.
        raw: String,
    },
}

impl GapValue {
    /// Reads a leaf without changing its representation.
    #[must_use]
    pub fn read(doc: &ConfigDoc, field: GapField) -> Self {
        match doc.get(&field.path()) {
            None => Self::Absent,
            Some(item) if item.is_array() => {
                item.as_value().and_then(PerMonitor::parse).map_or_else(
                    || Self::Invalid {
                        raw: item.to_string().trim().to_owned(),
                    },
                    Self::PerMonitor,
                )
            }
            Some(item) => item
                .as_integer()
                .and_then(|value| u32::try_from(value).ok())
                .map_or_else(
                    || Self::Invalid {
                        raw: item.to_string().trim().to_owned(),
                    },
                    Self::Const,
                ),
        }
    }
}

/// A refused gap edit. Failures leave the document unchanged.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GapEditError {
    /// A per-monitor value cannot be rewritten by a constant-gap operation.
    #[error("{0:?} is a per-monitor gap")]
    PerMonitor(GapField),
    /// A malformed source value must not be rewritten by a form operation.
    #[error("{0:?} has an unsupported gap value")]
    Invalid(GapField),
    /// A monitor-rule operation requires a per-monitor value.
    #[error("{0:?} is not a per-monitor gap")]
    NotPerMonitor(GapField),
    /// The requested row is outside the rule list, excluding the default.
    #[error("gap rule index {0} is out of bounds")]
    OutOfBounds(usize),
    /// AeroSpace requires at least one rule before the default.
    #[error("a per-monitor gap needs at least one monitor rule")]
    LastRule,
    /// A new selector is invalid or reserved.
    #[error(transparent)]
    Monitor(#[from] MonitorEditError),
    /// The lossless document operation failed.
    #[error(transparent)]
    Edit(#[from] EditError),
}

fn ensure_constant(doc: &ConfigDoc, field: GapField) -> Result<(), GapEditError> {
    match GapValue::read(doc, field) {
        GapValue::PerMonitor(_) => Err(GapEditError::PerMonitor(field)),
        GapValue::Invalid { .. } => Err(GapEditError::Invalid(field)),
        GapValue::Absent | GapValue::Const(_) => Ok(()),
    }
}

/// Sets one constant gap, preserving its decor.
///
/// # Errors
/// Refuses per-monitor leaves and lossless edit failures.
pub fn set_gap(doc: &mut ConfigDoc, field: GapField, value: u32) -> Result<(), GapEditError> {
    ensure_constant(doc, field)?;
    let mut edited = doc.clone();
    edited.set_value(&field.path(), i64::from(value).into())?;
    *doc = edited;
    Ok(())
}

/// Removes a constant gap so AeroSpace uses zero. Missing leaves are unchanged.
///
/// # Errors
/// Refuses per-monitor leaves and lossless edit failures.
pub fn reset_gap(doc: &mut ConfigDoc, field: GapField) -> Result<(), GapEditError> {
    ensure_constant(doc, field)?;
    if doc.get(&field.path()).is_some() {
        doc.remove(&field.path())?;
    }
    Ok(())
}

/// Sets all four outer gaps as one document change.
///
/// # Errors
/// Refuses the entire edit if any outer gap is per-monitor or cannot be edited.
pub fn set_outer_linked(doc: &mut ConfigDoc, value: u32) -> Result<(), GapEditError> {
    let mut edited = doc.clone();
    for field in GapField::OUTER {
        set_gap(&mut edited, field, value)?;
    }
    *doc = edited;
    Ok(())
}

fn per_monitor(doc: &ConfigDoc, field: GapField) -> Result<PerMonitor, GapEditError> {
    match GapValue::read(doc, field) {
        GapValue::PerMonitor(value) => Ok(value),
        GapValue::Invalid { .. } => Err(GapEditError::Invalid(field)),
        _ => Err(GapEditError::NotPerMonitor(field)),
    }
}

fn rule_value(pattern: &MonitorPattern, value: u32) -> Value {
    let mut monitor = InlineTable::new();
    monitor.set_dotted(true);
    monitor.insert(pattern.format(), i64::from(value).into());
    let mut rule = InlineTable::new();
    rule.insert("monitor", monitor.into());
    rule.fmt();
    rule.into()
}

/// Converts a constant to rules, seeding `main` and the default with the current value.
///
/// # Errors
/// Refuses malformed values and lossless edit failures. Existing rules are unchanged.
pub fn switch_to_per_monitor(doc: &mut ConfigDoc, field: GapField) -> Result<(), GapEditError> {
    let value = match GapValue::read(doc, field) {
        GapValue::Absent => 0,
        GapValue::Const(value) => value,
        GapValue::PerMonitor(_) => return Ok(()),
        GapValue::Invalid { .. } => return Err(GapEditError::Invalid(field)),
    };
    let array = Array::from_iter([
        rule_value(&MonitorPattern::Main, value),
        i64::from(value).into(),
    ]);
    let mut edited = doc.clone();
    edited.set_value(&field.path(), array.into())?;
    *doc = edited;
    Ok(())
}

/// Converts rules to a constant using the default, leaving constants unchanged.
///
/// # Errors
/// Refuses malformed values and lossless edit failures.
pub fn switch_to_constant(doc: &mut ConfigDoc, field: GapField) -> Result<(), GapEditError> {
    match GapValue::read(doc, field) {
        GapValue::PerMonitor(value) => {
            doc.set_value(&field.path(), i64::from(value.default).into())?;
        }
        GapValue::Invalid { .. } => return Err(GapEditError::Invalid(field)),
        _ => {}
    }
    Ok(())
}

/// Appends a rule before the trailing default.
///
/// # Errors
/// Refuses non-rule values, invalid selectors, and lossless edit failures.
pub fn add_gap_rule(
    doc: &mut ConfigDoc,
    field: GapField,
    pattern: &MonitorPattern,
    value: u32,
) -> Result<(), GapEditError> {
    let current = per_monitor(doc, field)?;
    ensure_pattern(pattern)?;
    doc.array_insert(
        &field.path(),
        current.rules.len(),
        rule_value(pattern, value),
    )?;
    Ok(())
}

/// Removes a rule, retaining at least one rule before the default.
///
/// # Errors
/// Refuses non-rule values, the last rule, invalid indices, and lossless edit failures.
pub fn remove_gap_rule(
    doc: &mut ConfigDoc,
    field: GapField,
    index: usize,
) -> Result<(), GapEditError> {
    let current = per_monitor(doc, field)?;
    check_rule(&current, index)?;
    if current.rules.len() == 1 {
        return Err(GapEditError::LastRule);
    }
    doc.array_remove(&field.path(), index)?;
    Ok(())
}

fn check_rule(current: &PerMonitor, index: usize) -> Result<(), GapEditError> {
    current
        .rules
        .get(index)
        .map(|_| ())
        .ok_or(GapEditError::OutOfBounds(index))
}

/// Reorders rule rows and their comments, never moving the trailing default.
///
/// # Errors
/// Refuses non-rule values, invalid indices, and lossless edit failures.
pub fn move_gap_rule(
    doc: &mut ConfigDoc,
    field: GapField,
    from: usize,
    to: usize,
) -> Result<(), GapEditError> {
    let current = per_monitor(doc, field)?;
    check_rule(&current, from)?;
    check_rule(&current, to)?;
    doc.array_move(&field.path(), from, to)?;
    Ok(())
}

/// Changes a rule's selector and value while retaining its surrounding representation.
///
/// # Errors
/// Refuses non-rule values, invalid selectors or indices, and lossless edit failures.
pub fn set_gap_rule(
    doc: &mut ConfigDoc,
    field: GapField,
    index: usize,
    pattern: &MonitorPattern,
    value: u32,
) -> Result<(), GapEditError> {
    let current = per_monitor(doc, field)?;
    let previous = current
        .rules
        .get(index)
        .ok_or(GapEditError::OutOfBounds(index))?;
    ensure_pattern(pattern)?;
    if previous == &(pattern.clone(), value) {
        return Ok(());
    }
    let mut rule = doc
        .get(&field.path())
        .and_then(Item::as_array)
        .and_then(|array| array.get(index))
        .cloned()
        .ok_or(GapEditError::Invalid(field))?;
    let monitor = rule
        .as_inline_table_mut()
        .and_then(|rule| rule.get_mut("monitor"))
        .and_then(Value::as_inline_table_mut)
        .ok_or(GapEditError::Invalid(field))?;
    let original_key = monitor
        .iter()
        .next()
        .map(|(key, _)| key.to_owned())
        .ok_or(GapEditError::Invalid(field))?;
    let (old_key, old_value) = monitor
        .remove_entry(&original_key)
        .ok_or(GapEditError::Invalid(field))?;
    let mut key = if previous.0 == *pattern {
        old_key.clone()
    } else {
        Key::new(pattern.format())
    };
    *key.leaf_decor_mut() = old_key.leaf_decor().clone();
    *key.dotted_decor_mut() = old_key.dotted_decor().clone();
    let mut new_value = if previous.1 == value {
        old_value.clone()
    } else {
        i64::from(value).into()
    };
    *new_value.decor_mut() = old_value.decor().clone();
    monitor.insert_formatted(&key, new_value);
    doc.array_set(&field.path(), index, rule)?;
    Ok(())
}

/// Changes only the trailing default value.
///
/// # Errors
/// Refuses non-rule values and lossless edit failures.
pub fn set_gap_default(
    doc: &mut ConfigDoc,
    field: GapField,
    value: u32,
) -> Result<(), GapEditError> {
    let current = per_monitor(doc, field)?;
    if current.default != value {
        doc.array_set(&field.path(), current.rules.len(), i64::from(value).into())?;
    }
    Ok(())
}
