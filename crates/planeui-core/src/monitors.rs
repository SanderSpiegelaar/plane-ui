//! Monitor patterns shared by workspace assignments and per-monitor gaps.

use std::num::NonZeroU32;

use regex::RegexBuilder;
use toml_edit::{Array, Item, Value};

use crate::aerospace::Monitor;
use crate::config_doc::{ConfigDoc, EditError, KeyPath};

/// AeroSpace's monitor selector grammar.
///
/// Mirrors [MonitorDescription.swift](https://github.com/nikitabobko/AeroSpace/blob/v0.21.3-Beta/Sources/Common/model/MonitorDescription.swift).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MonitorPattern {
    /// The macOS main display.
    Main,
    /// The non-main display, only when exactly two displays are connected.
    Secondary,
    /// One-based position in AeroSpace's monitor sequence.
    Index(NonZeroU32),
    /// Case-insensitive substring regex against a display name.
    Regex(String),
}

/// An invalid monitor selector.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PatternError {
    /// Only strings and integers are accepted.
    #[error("monitor pattern must be an integer or a string")]
    WrongType,
    /// Empty strings cannot select a display.
    #[error("monitor pattern must not be empty")]
    Empty,
    /// Sequence numbers start at one and must fit the supported index type.
    #[error("monitor sequence number must be at least 1 and fit in a 32-bit integer")]
    Index,
}

impl MonitorPattern {
    /// Parses an integer or string without rejecting regexes from Apple's different engine.
    ///
    /// # Errors
    /// Returns a typed error for an empty, nonpositive, or wrongly typed selector.
    pub fn parse(value: &Value) -> Result<Self, PatternError> {
        if let Some(index) = value.as_integer() {
            return u32::try_from(index)
                .ok()
                .and_then(NonZeroU32::new)
                .map(Self::Index)
                .ok_or(PatternError::Index);
        }
        let text = value.as_str().ok_or(PatternError::WrongType)?;
        Self::parse_str(text)
    }

    /// Parses a selector used as a per-monitor table key.
    ///
    /// # Errors
    /// Returns a typed error for an empty or nonpositive selector.
    pub fn parse_str(text: &str) -> Result<Self, PatternError> {
        match text {
            "" => Err(PatternError::Empty),
            "main" => Ok(Self::Main),
            "secondary" => Ok(Self::Secondary),
            _ => match text.parse::<i64>() {
                Ok(index) => u32::try_from(index)
                    .ok()
                    .and_then(NonZeroU32::new)
                    .map(Self::Index)
                    .ok_or(PatternError::Index),
                Err(_) => Ok(Self::Regex(text.to_owned())),
            },
        }
    }

    /// The selector text used in monitor table keys and form fields.
    #[must_use]
    pub fn format(&self) -> String {
        match self {
            Self::Main => "main".into(),
            Self::Secondary => "secondary".into(),
            Self::Index(index) => index.to_string(),
            Self::Regex(text) => text.clone(),
        }
    }

    /// Formats new selectors, writing sequence numbers as integers.
    #[must_use]
    pub fn to_value(&self) -> Value {
        match self {
            Self::Index(index) => i64::from(index.get()).into(),
            _ => self.format().into(),
        }
    }

    pub(crate) fn value_like(&self, original: &Value) -> Value {
        if matches!(self, Self::Index(_)) && original.is_str() {
            self.format().into()
        } else {
            self.to_value()
        }
    }

    /// A local regex warning, never a write-blocking error because Apple's engine differs.
    #[must_use]
    pub fn regex_warning(&self) -> Option<String> {
        match self {
            Self::Regex(text) => RegexBuilder::new(text)
                .case_insensitive(true)
                .build()
                .err()
                .map(|error| error.to_string()),
            _ => None,
        }
    }
}

/// The source representation of an assignment's selectors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatternShape {
    /// One scalar selector.
    Single,
    /// An ordered array, retained even when only one selector remains.
    List,
}

/// Resolves the first matching selector in AeroSpace's sequence order.
///
/// Mirrors [MonitorDescriptionEx.swift](https://github.com/nikitabobko/AeroSpace/blob/v0.21.3-Beta/Sources/AppBundle/model/MonitorDescriptionEx.swift).
#[must_use]
pub fn resolve<'a>(patterns: &[MonitorPattern], monitors: &'a [Monitor]) -> Option<&'a Monitor> {
    let mut sorted: Vec<_> = monitors.iter().collect();
    sorted.sort_by_key(|monitor| monitor.id);
    patterns.iter().find_map(|pattern| match pattern {
        MonitorPattern::Main => sorted.iter().copied().find(|monitor| monitor.is_main),
        MonitorPattern::Secondary if sorted.len() == 2 => {
            sorted.iter().copied().find(|monitor| !monitor.is_main)
        }
        MonitorPattern::Secondary => None,
        MonitorPattern::Index(index) => usize::try_from(index.get() - 1)
            .ok()
            .and_then(|index| sorted.get(index).copied()),
        MonitorPattern::Regex(text) => RegexBuilder::new(text)
            .case_insensitive(true)
            .build()
            .ok()
            .and_then(|regex| {
                sorted
                    .iter()
                    .copied()
                    .find(|monitor| regex.is_match(&monitor.name))
            }),
    })
}

pub(crate) const ASSIGNMENTS: &str = "workspace-to-monitor-force-assignment";

pub(crate) fn read_patterns(
    item: &Item,
) -> Result<(Vec<MonitorPattern>, PatternShape), PatternError> {
    let value = item.as_value().ok_or(PatternError::WrongType)?;
    match value.as_array() {
        Some(array) => Ok((
            array
                .iter()
                .map(MonitorPattern::parse)
                .collect::<Result<_, _>>()?,
            PatternShape::List,
        )),
        None => Ok((vec![MonitorPattern::parse(value)?], PatternShape::Single)),
    }
}

/// A refused assignment edit. The original document is unchanged.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MonitorEditError {
    /// An existing selector cannot be safely edited.
    #[error(transparent)]
    Pattern(#[from] PatternError),
    /// The row does not have the requested selector position.
    #[error("monitor pattern index {0} is out of bounds")]
    OutOfBounds(usize),
    /// The requested selector does not retain its meaning when formatted.
    #[error("regex selector is empty or reserved for a monitor keyword or index")]
    ReservedRegex,
    /// A lossless document operation failed.
    #[error(transparent)]
    Edit(#[from] EditError),
}

pub(crate) fn ensure_pattern(pattern: &MonitorPattern) -> Result<(), MonitorEditError> {
    if MonitorPattern::parse_str(&pattern.format())? != *pattern {
        return Err(MonitorEditError::ReservedRegex);
    }
    Ok(())
}

fn path(workspace: &str) -> KeyPath {
    KeyPath::from([ASSIGNMENTS, workspace])
}

fn patterns(
    doc: &ConfigDoc,
    workspace: &str,
) -> Result<(Vec<MonitorPattern>, PatternShape), MonitorEditError> {
    let path = path(workspace);
    Ok(read_patterns(
        doc.get(&path).ok_or(EditError::NotFound(path))?,
    )?)
}

/// Adds a workspace assignment with one scalar selector.
///
/// # Errors
/// Refuses existing rows, invalid selectors, and unsafe document shapes.
pub fn add_assignment(
    doc: &mut ConfigDoc,
    workspace: &str,
    pattern: &MonitorPattern,
) -> Result<(), MonitorEditError> {
    ensure_pattern(pattern)?;
    let path = path(workspace);
    if doc.get(&path).is_some() {
        return Err(EditError::AlreadyExists(path).into());
    }
    let mut edited = doc.clone();
    edited.set_value(&path, pattern.to_value())?;
    *doc = edited;
    Ok(())
}

/// Removes an assignment, including an invalid row, without changing other rows.
///
/// # Errors
/// Returns the lossless document error when the row is absent or cannot be removed.
pub fn remove_assignment(doc: &mut ConfigDoc, workspace: &str) -> Result<(), MonitorEditError> {
    doc.remove(&path(workspace))?;
    Ok(())
}

/// Appends a selector, promoting a scalar to an array only when needed.
///
/// # Errors
/// Refuses invalid rows, selectors, and lossless document failures.
pub fn add_pattern(
    doc: &mut ConfigDoc,
    workspace: &str,
    pattern: &MonitorPattern,
) -> Result<(), MonitorEditError> {
    ensure_pattern(pattern)?;
    let (patterns, shape) = patterns(doc, workspace)?;
    let path = path(workspace);
    let mut edited = doc.clone();
    match shape {
        PatternShape::Single => {
            let mut original = doc
                .get(&path)
                .and_then(Item::as_value)
                .cloned()
                .ok_or_else(|| EditError::NotAValue(path.clone()))?;
            original.decor_mut().clear();
            edited.set_value(
                &path,
                Array::from_iter([original, pattern.to_value()]).into(),
            )?;
        }
        PatternShape::List => edited.array_insert(&path, patterns.len(), pattern.to_value())?,
    }
    *doc = edited;
    Ok(())
}

/// Removes a selector without collapsing an existing array to a scalar.
///
/// # Errors
/// Refuses invalid rows, out-of-bounds indices, and lossless edit failures.
pub fn remove_pattern(
    doc: &mut ConfigDoc,
    workspace: &str,
    index: usize,
) -> Result<(), MonitorEditError> {
    let (patterns, shape) = patterns(doc, workspace)?;
    if index >= patterns.len() {
        return Err(MonitorEditError::OutOfBounds(index));
    }
    if shape == PatternShape::Single {
        doc.set_value(&path(workspace), Array::new().into())?;
    } else {
        doc.array_remove(&path(workspace), index)?;
    }
    Ok(())
}

/// Reorders selectors with their comments.
///
/// # Errors
/// Refuses invalid rows, out-of-bounds indices, and lossless edit failures.
pub fn move_pattern(
    doc: &mut ConfigDoc,
    workspace: &str,
    from: usize,
    to: usize,
) -> Result<(), MonitorEditError> {
    let (patterns, shape) = patterns(doc, workspace)?;
    for index in [from, to] {
        if index >= patterns.len() {
            return Err(MonitorEditError::OutOfBounds(index));
        }
    }
    if shape == PatternShape::List {
        doc.array_move(&path(workspace), from, to)?;
    }
    Ok(())
}

/// Changes one selector, retaining a quoted sequence number's string representation.
///
/// # Errors
/// Refuses invalid rows, selectors, indices, and lossless edit failures.
pub fn set_pattern(
    doc: &mut ConfigDoc,
    workspace: &str,
    index: usize,
    pattern: &MonitorPattern,
) -> Result<(), MonitorEditError> {
    ensure_pattern(pattern)?;
    let (patterns, shape) = patterns(doc, workspace)?;
    let previous = patterns
        .get(index)
        .ok_or(MonitorEditError::OutOfBounds(index))?;
    if previous == pattern {
        return Ok(());
    }
    let path = path(workspace);
    let original = doc
        .get(&path)
        .and_then(Item::as_value)
        .and_then(|value| match shape {
            PatternShape::Single => Some(value),
            PatternShape::List => value.as_array()?.get(index),
        })
        .ok_or_else(|| EditError::NotAValue(path.clone()))?;
    let value = pattern.value_like(original);
    match shape {
        PatternShape::Single => doc.set_value(&path, value)?,
        PatternShape::List => doc.array_set(&path, index, value)?,
    }
    Ok(())
}
