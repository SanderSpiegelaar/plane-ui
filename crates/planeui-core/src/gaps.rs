//! Constant gap reads and lossless edits. Per-monitor arrays remain read-only.

use crate::config_doc::{ConfigDoc, EditError, KeyPath};

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

/// A per-monitor array, retained as written rather than interpreted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PerMonitorRaw(pub String);

/// A gap leaf as found in the document.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum GapValue {
    /// AeroSpace uses zero.
    #[default]
    Absent,
    /// A constant gap.
    Const(u32),
    /// A read-only monitor-dependent gap.
    PerMonitor(PerMonitorRaw),
    /// An unsupported value.
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
                Self::PerMonitor(PerMonitorRaw(item.to_string().trim().to_owned()))
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
    /// The lossless document operation failed.
    #[error(transparent)]
    Edit(#[from] EditError),
}

fn ensure_constant(doc: &ConfigDoc, field: GapField) -> Result<(), GapEditError> {
    if matches!(GapValue::read(doc, field), GapValue::PerMonitor(_)) {
        Err(GapEditError::PerMonitor(field))
    } else {
        Ok(())
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
