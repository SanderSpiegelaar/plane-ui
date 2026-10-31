//! Lossless wrapper around a `toml_edit` document. Every mutation of the config goes through
//! here so untouched content stays byte-for-byte identical.

use std::fmt;

use toml_edit::{
    Array, ArrayOfTables, DocumentMut, InlineTable, Item, Key, RawString, Table, Value,
};

/// An `aerospace.toml` document that renders back to its exact source text until edited.
#[derive(Debug, Clone)]
pub struct ConfigDoc {
    document: DocumentMut,
    line_ending: LineEnding,
}

/// `toml_edit` always renders `\n`, so the source's line ending is restored on output.
#[derive(Debug, Clone, PartialEq, Eq)]
enum LineEnding {
    Lf,
    Crlf,
    /// `toml_edit` forgets which lines had `\r`, so a mixed file cannot be edited without
    /// rewriting untouched lines. It renders its source verbatim and refuses edits.
    Mixed(String),
}

impl LineEnding {
    fn detect(source: &str) -> Self {
        let line_feeds = source.matches('\n').count();
        match source.matches("\r\n").count() {
            0 => Self::Lf,
            crlf if crlf == line_feeds => Self::Crlf,
            _ => Self::Mixed(source.to_owned()),
        }
    }
}

impl ConfigDoc {
    /// Parses config text.
    ///
    /// # Errors
    ///
    /// Returns [`ParseError`] with the 1-based line and column of the first syntax error.
    pub fn parse(source: &str) -> Result<Self, ParseError> {
        source
            .parse::<DocumentMut>()
            .map(|document| Self {
                document,
                line_ending: LineEnding::detect(source),
            })
            .map_err(|error| ParseError::from_toml(source, &error))
    }

    /// The underlying document, for read-only walks.
    #[must_use]
    pub fn document(&self) -> &DocumentMut {
        &self.document
    }

    /// Resolves `path` however the file writes it: `[a.b]` tables, dotted keys, or inline
    /// tables.
    #[must_use]
    pub fn get(&self, path: &KeyPath) -> Option<&Item> {
        path.segments()
            .iter()
            .try_fold(self.document.as_item(), |item, segment| {
                item.as_table_like()?.get(segment)
            })
    }

    /// Sets the value at `path`. An existing value keeps its key and value decor (comments,
    /// spacing); a new key goes after its last sibling in the style the siblings use.
    ///
    /// # Errors
    ///
    /// Returns [`EditError`] when a parent is not a table or `path` names a table.
    pub fn set_value(&mut self, path: &KeyPath, mut value: Value) -> Result<(), EditError> {
        self.ensure_editable()?;
        let (leaf, parents) = path.split_last()?;
        let mut indent = indentation(self.document.as_table());
        let mut container = self.document.as_item_mut();
        for (depth, segment) in parents.iter().enumerate() {
            if let Item::Table(table) = &*container
                && !table.is_dotted()
            {
                indent = indentation(table);
            }
            let exists = container
                .as_table_like()
                .ok_or_else(|| EditError::NotATable(path.prefix(depth)))?
                .contains_key(segment);
            if !exists {
                let missing = parents.get(depth..).unwrap_or_default();
                return insert_new(container, missing, leaf, value, &indent)
                    .map_err(|()| EditError::NotATable(path.prefix(depth)));
            }
            container = container
                .as_table_like_mut()
                .and_then(|table| table.get_mut(segment))
                .ok_or_else(|| EditError::NotATable(path.prefix(depth)))?;
        }
        if let Item::Table(table) = &*container
            && !table.is_dotted()
        {
            indent = indentation(table);
        }
        let table = container
            .as_table_like_mut()
            .ok_or_else(|| EditError::NotATable(path.prefix(parents.len())))?;
        match table.get_mut(leaf) {
            Some(Item::Value(existing)) => {
                *value.decor_mut() = existing.decor().clone();
                *existing = value;
                Ok(())
            }
            Some(_) => Err(EditError::NotAValue(path.clone())),
            None => insert_new(container, &[], leaf, value, &indent)
                .map_err(|()| EditError::NotATable(path.prefix(parents.len()))),
        }
    }

    /// Inserts an empty table, using an inline value for dotted or inline parents.
    ///
    /// # Errors
    ///
    /// Refuses existing paths and parents that are not tables.
    pub fn insert_empty_table(&mut self, path: &KeyPath) -> Result<(), EditError> {
        if self.get(path).is_some() {
            return Err(EditError::AlreadyExists(path.clone()));
        }
        self.set_value(path, Value::InlineTable(InlineTable::new()))?;
        let (leaf, parents) = path.split_last()?;
        if let Some(parent) = table_mut(self.document.as_table_mut(), parents)
            && !parent.is_dotted()
        {
            parent.insert(leaf, Item::Table(Table::new()));
        }
        Ok(())
    }

    /// Renames a key without moving its item or changing its surrounding decor.
    ///
    /// # Errors
    ///
    /// Returns [`EditError`] if the source is missing or the new key already exists.
    pub fn rename_key(&mut self, path: &KeyPath, new_name: &str) -> Result<(), EditError> {
        self.ensure_editable()?;
        let (leaf, parents) = path.split_last()?;
        let parent = parents
            .iter()
            .try_fold(self.document.as_item_mut(), |item, segment| {
                item.as_table_like_mut()?.get_mut(segment)
            })
            .ok_or_else(|| EditError::NotFound(path.clone()))?;
        let container = parent
            .as_table_like_mut()
            .ok_or_else(|| EditError::NotFound(path.clone()))?;
        if !container.contains_key(leaf) {
            return Err(EditError::NotFound(path.clone()));
        }
        if leaf == new_name {
            return Ok(());
        }
        if container.contains_key(new_name) {
            return Err(EditError::AlreadyExists(
                path.prefix(parents.len()).join(new_name),
            ));
        }
        let entries: Vec<_> = container
            .iter()
            .filter_map(|(name, _)| container.get_key_value(name))
            .map(|(key, item)| {
                let renamed = if key.get() == leaf {
                    let mut renamed = Key::new(new_name);
                    *renamed.leaf_decor_mut() = key.leaf_decor().clone();
                    *renamed.dotted_decor_mut() = key.dotted_decor().clone();
                    renamed
                } else {
                    key.clone()
                };
                (renamed, item.clone())
            })
            .collect();
        container.clear();
        for (key, item) in entries {
            match &mut *parent {
                Item::Table(table) => {
                    table.insert_formatted(&key, item);
                }
                Item::Value(Value::InlineTable(table)) => {
                    if let Item::Value(value) = item {
                        table.insert_formatted(&key, value);
                    }
                }
                _ => return Err(EditError::NotATable(path.prefix(parents.len()))),
            }
        }
        Ok(())
    }

    /// Inserts a new value with its original representation and value decor intact.
    ///
    /// # Errors
    ///
    /// Returns [`EditError`] when the key exists or its parent cannot hold values.
    pub fn insert_value_verbatim(&mut self, path: &KeyPath, value: Value) -> Result<(), EditError> {
        self.ensure_editable()?;
        if self.get(path).is_some() {
            return Err(EditError::AlreadyExists(path.clone()));
        }
        self.set_value(path, value.clone())?;
        if let Some(item) = path
            .segments()
            .iter()
            .try_fold(self.document.as_item_mut(), |item, segment| {
                item.as_table_like_mut()?.get_mut(segment)
            })
        {
            *item = Item::Value(value);
        }
        Ok(())
    }

    /// Removes the value or table at `path` with the comment lines directly above it. Comments
    /// separated from it by a blank line stay. Implicit and dotted parent tables left empty are
    /// dropped; `[table]` headers the user wrote are kept.
    ///
    /// # Errors
    ///
    /// Returns [`EditError::NotFound`] when nothing is at `path`.
    pub fn remove(&mut self, path: &KeyPath) -> Result<(), EditError> {
        self.ensure_editable()?;
        let (_, parents) = path.split_last()?;
        let visible_depth = visible_table_depth(self.document.as_table(), parents);
        let visible_path = parents.get(..visible_depth).unwrap_or_default();
        let visible = table_mut(self.document.as_table_mut(), visible_path)
            .ok_or_else(|| EditError::NotFound(path.clone()))?;
        let relative = path.segments().get(visible_depth..).unwrap_or_default();
        remove_from(visible, relative, visible_depth == 0)
            .ok_or_else(|| EditError::NotFound(path.clone()))?;
        prune_empty_tables(self.document.as_table_mut(), parents);
        Ok(())
    }

    /// Inserts `value` into the array at `path` before `index` (`index == len` appends).
    /// Arrays of tables take an inline table, written as a `[[table]]` section.
    ///
    /// # Errors
    ///
    /// Returns [`EditError`] when `path` is not an array, `index` is out of bounds, or a
    /// non-table value goes into an array of tables.
    pub fn array_insert(
        &mut self,
        path: &KeyPath,
        index: usize,
        value: Value,
    ) -> Result<(), EditError> {
        match self.array_mut(path)? {
            ArrayMut::Inline(array) => {
                check_index(path, index, array.len(), true)?;
                let old_len = array.len();
                let edges = EdgeDecor::capture(array);
                array.insert_formatted(index, value);
                let origins = (0..=old_len)
                    .map(|new| match new.cmp(&index) {
                        std::cmp::Ordering::Less => Some(new),
                        std::cmp::Ordering::Equal => None,
                        std::cmp::Ordering::Greater => Some(new - 1),
                    })
                    .collect::<Vec<_>>();
                edges.restyle(array, &origins, old_len);
            }
            ArrayMut::Tables(tables) => {
                check_index(path, index, tables.len(), true)?;
                let mut table = into_section(path, value)?;
                // Positions are unique and even (see `normalize_positions`), so the odd slot
                // just before element `index`, or just after the previous element's last
                // sub-table when appending, renders exactly there.
                let position = match tables.get(index) {
                    Some(next) => next.position().map(|position| position - 1),
                    None => index
                        .checked_sub(1)
                        .and_then(|last| tables.get(last))
                        .and_then(|previous| section_positions(previous).into_iter().max())
                        .map(|position| position + 1),
                };
                table.set_position(position);
                tables.insert(index, table);
            }
        }
        Ok(())
    }

    /// Removes the element at `index` from the array at `path`.
    ///
    /// # Errors
    ///
    /// Returns [`EditError`] when `path` is not an array or `index` is out of bounds.
    pub fn array_remove(&mut self, path: &KeyPath, index: usize) -> Result<(), EditError> {
        self.ensure_editable()?;
        match self.array_mut(path)? {
            ArrayMut::Inline(array) => {
                check_index(path, index, array.len(), false)?;
                let old_len = array.len();
                let edges = EdgeDecor::capture(array);
                array.remove(index);
                let origins = (0..old_len - 1)
                    .map(|new| Some(if new < index { new } else { new + 1 }))
                    .collect::<Vec<_>>();
                edges.restyle(array, &origins, old_len);
            }
            ArrayMut::Tables(tables) => {
                check_index(path, index, tables.len(), false)?;
                tables.remove(index);
            }
        }
        Ok(())
    }

    /// Moves the element at `from` so it ends up at index `to`, carrying its comments.
    ///
    /// # Errors
    ///
    /// Returns [`EditError`] when `path` is not an array or an index is out of bounds.
    pub fn array_move(&mut self, path: &KeyPath, from: usize, to: usize) -> Result<(), EditError> {
        self.ensure_editable()?;
        match self.array_mut(path)? {
            ArrayMut::Inline(array) => {
                check_index(path, from, array.len(), false)?;
                check_index(path, to, array.len(), false)?;
                let len = array.len();
                let edges = EdgeDecor::capture(array);
                let moved = array.remove(from);
                array.insert_formatted(to, moved);
                let mut origins: Vec<Option<usize>> = (0..len).map(Some).collect();
                let origin = origins.remove(from);
                origins.insert(to, origin);
                edges.restyle(array, &origins, len);
            }
            ArrayMut::Tables(tables) => {
                check_index(path, from, tables.len(), false)?;
                check_index(path, to, tables.len(), false)?;
                // Each element owns its header and its `[array.sub]` tables. The elements' slots
                // are dealt out again in the new order; within an element, its tables keep
                // their relative order by rank, so sub-tables move with their element.
                let mut groups: Vec<Vec<isize>> = tables.iter().map(section_positions).collect();
                let mut slots: Vec<isize> = groups.iter().flatten().copied().collect();
                slots.sort_unstable();
                let moved = tables.remove(from);
                tables.insert(to, moved);
                let moved_group = groups.remove(from);
                groups.insert(to, moved_group);
                let mut slots = slots.into_iter();
                for (table, mut group) in tables.iter_mut().zip(groups) {
                    group.sort_unstable();
                    let mapping: Vec<(isize, isize)> =
                        group.into_iter().zip(slots.by_ref()).collect();
                    remap_positions(table, &mapping);
                }
            }
        }
        Ok(())
    }

    /// Replaces the element at `index`, keeping that element's decor.
    ///
    /// # Errors
    ///
    /// Returns [`EditError`] when `path` is not an array, `index` is out of bounds, or a
    /// non-table value goes into an array of tables.
    pub fn array_set(
        &mut self,
        path: &KeyPath,
        index: usize,
        mut value: Value,
    ) -> Result<(), EditError> {
        match self.array_mut(path)? {
            ArrayMut::Inline(array) => {
                let len = array.len();
                let existing = array
                    .get_mut(index)
                    .ok_or_else(|| EditError::IndexOutOfBounds {
                        path: path.clone(),
                        index,
                        len,
                    })?;
                *value.decor_mut() = existing.decor().clone();
                *existing = value;
            }
            ArrayMut::Tables(tables) => {
                let len = tables.len();
                let mut table = into_section(path, value)?;
                let existing =
                    tables
                        .get_mut(index)
                        .ok_or_else(|| EditError::IndexOutOfBounds {
                            path: path.clone(),
                            index,
                            len,
                        })?;
                table.set_position(existing.position());
                *table.decor_mut() = existing.decor().clone();
                *existing = table;
            }
        }
        Ok(())
    }

    fn ensure_editable(&self) -> Result<(), EditError> {
        match self.line_ending {
            LineEnding::Mixed(_) => Err(EditError::MixedLineEndings),
            LineEnding::Lf | LineEnding::Crlf => Ok(()),
        }
    }

    fn array_mut(&mut self, path: &KeyPath) -> Result<ArrayMut<'_>, EditError> {
        self.ensure_editable()?;
        normalize_positions(self.document.as_table_mut());
        let item = path
            .segments()
            .iter()
            .try_fold(self.document.as_item_mut(), |item, segment| {
                item.as_table_like_mut()?.get_mut(segment)
            })
            .ok_or_else(|| EditError::NotFound(path.clone()))?;
        match item {
            Item::Value(Value::Array(array)) => Ok(ArrayMut::Inline(array)),
            Item::ArrayOfTables(tables) => Ok(ArrayMut::Tables(tables)),
            _ => Err(EditError::NotAnArray(path.clone())),
        }
    }
}

/// A key path such as `gaps.outer.left`, one segment per TOML key.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct KeyPath(Vec<String>);

impl KeyPath {
    /// Builds a path from its segments.
    pub fn new<I, S>(segments: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self(segments.into_iter().map(Into::into).collect())
    }

    /// The path's segments, outermost first.
    #[must_use]
    pub fn segments(&self) -> &[String] {
        &self.0
    }

    /// This path with `segment` appended.
    #[must_use]
    pub fn join(&self, segment: impl Into<String>) -> Self {
        let mut segments = self.0.clone();
        segments.push(segment.into());
        Self(segments)
    }

    fn prefix(&self, len: usize) -> Self {
        Self(self.0.get(..len).unwrap_or(&self.0).to_vec())
    }

    fn split_last(&self) -> Result<(&str, &[String]), EditError> {
        self.0
            .split_last()
            .map(|(leaf, parents)| (leaf.as_str(), parents))
            .ok_or(EditError::EmptyPath)
    }
}

impl<S: Into<String>, const N: usize> From<[S; N]> for KeyPath {
    fn from(segments: [S; N]) -> Self {
        Self::new(segments)
    }
}

impl fmt::Display for KeyPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, segment) in self.0.iter().enumerate() {
            if index > 0 {
                formatter.write_str(".")?;
            }
            formatter.write_str(&Key::new(segment.as_str()).display_repr())?;
        }
        Ok(())
    }
}

/// Why a document edit could not be made. The document is unchanged when one is returned.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EditError {
    /// The path has no segments.
    #[error("the key path is empty")]
    EmptyPath,
    /// The file mixes CRLF and LF line endings, which an edit cannot preserve.
    #[error("the file mixes CRLF and LF line endings; PlaneUI edits it once they are consistent")]
    MixedLineEndings,
    /// A parent on the path holds a value, not a table.
    #[error("{0} is not a table")]
    NotATable(KeyPath),
    /// The path names a table where a value was expected.
    #[error("{0} is a table, not a value")]
    NotAValue(KeyPath),
    /// Nothing exists at the path.
    #[error("{0} does not exist")]
    NotFound(KeyPath),
    /// A key already exists at the destination.
    #[error("{0} already exists")]
    AlreadyExists(KeyPath),
    /// The path holds something other than an array.
    #[error("{0} is not an array")]
    NotAnArray(KeyPath),
    /// An array index past the end.
    #[error("{path} has no element {index} (length {len})")]
    IndexOutOfBounds {
        /// The array.
        path: KeyPath,
        /// The requested index.
        index: usize,
        /// The array's length.
        len: usize,
    },
    /// A non-table value for an array of `[[tables]]`.
    #[error("{0} is an array of tables; its elements must be tables")]
    NotATableElement(KeyPath),
}

#[derive(Debug)]
enum ArrayMut<'a> {
    Inline(&'a mut Array),
    Tables(&'a mut ArrayOfTables),
}

/// `index` may equal `len` only when `appending` (an insert at the end).
fn check_index(path: &KeyPath, index: usize, len: usize, appending: bool) -> Result<(), EditError> {
    if index < len || (appending && index == len) {
        Ok(())
    } else {
        Err(EditError::IndexOutOfBounds {
            path: path.clone(),
            index,
            len,
        })
    }
}

/// Renumbers every table header to a unique even position in the order `toml_edit` renders
/// them now, so the output is unchanged. Parsed and inserted headers can share a position,
/// and `toml_edit` breaks such ties by tree order, which position arithmetic cannot see.
fn normalize_positions(root: &mut Table) {
    fn collect(table: &Table, out: &mut Vec<Option<isize>>) {
        if !table.is_dotted() {
            out.push(table.position());
        }
        for (_, item) in table {
            match item {
                Item::Table(child) => collect(child, out),
                Item::ArrayOfTables(children) => {
                    for child in children {
                        collect(child, out);
                    }
                }
                _ => {}
            }
        }
    }
    fn assign(table: &mut Table, positions: &mut impl Iterator<Item = isize>) {
        if !table.is_dotted()
            && let Some(position) = positions.next()
        {
            table.set_position(Some(position));
        }
        for (_, item) in table.iter_mut() {
            match item {
                Item::Table(child) => assign(child, positions),
                Item::ArrayOfTables(children) => {
                    for child in children.iter_mut() {
                        assign(child, positions);
                    }
                }
                _ => {}
            }
        }
    }

    let mut visited = Vec::new();
    collect(root, &mut visited);
    // Mirrors `DocumentMut`'s Display: a header without a position inherits the last one seen
    // in tree order, the root renders first, and ties keep tree order (stable sort).
    let mut last = 0;
    let inherited: Vec<isize> = visited
        .iter()
        .map(|position| {
            if let Some(position) = position {
                last = *position;
            }
            last
        })
        .collect();
    let mut render_order: Vec<usize> = (0..inherited.len()).collect();
    render_order.sort_by_key(|&index| (index != 0, inherited.get(index).copied()));
    let mut new_positions = vec![0; inherited.len()];
    for (rank, index) in render_order.into_iter().enumerate() {
        if let Some(slot) = new_positions.get_mut(index) {
            *slot = isize::try_from(rank).unwrap_or(isize::MAX / 2) * 2 + 2;
        }
    }
    assign(root, &mut new_positions.into_iter());
}

/// Positions of `table`'s header and every header nested under it, in render-visit order.
fn section_positions(table: &Table) -> Vec<isize> {
    let mut positions: Vec<isize> = table.position().into_iter().collect();
    for (_, item) in table {
        match item {
            Item::Table(child) => positions.extend(section_positions(child)),
            Item::ArrayOfTables(children) => {
                positions.extend(children.iter().flat_map(section_positions));
            }
            _ => {}
        }
    }
    positions
}

/// Moves `table` and its nested headers from their old positions to new ones.
fn remap_positions(table: &mut Table, mapping: &[(isize, isize)]) {
    let new_position = table
        .position()
        .and_then(|old| mapping.iter().find(|(from, _)| *from == old))
        .map(|(_, to)| *to);
    if new_position.is_some() {
        table.set_position(new_position);
    }
    for (_, item) in table.iter_mut() {
        match item {
            Item::Table(child) => remap_positions(child, mapping),
            Item::ArrayOfTables(children) => {
                for child in children.iter_mut() {
                    remap_positions(child, mapping);
                }
            }
            _ => {}
        }
    }
}

fn into_section(path: &KeyPath, value: Value) -> Result<Table, EditError> {
    match value {
        Value::InlineTable(table) => Ok(table.into_table()),
        _ => Err(EditError::NotATableElement(path.clone())),
    }
}

/// Inserts `leaf = value` under `container`, first creating the `missing` tables between them.
/// New tables are dotted when the container's existing subtables mostly are, `[headers]`
/// otherwise, and nested inline tables inside an inline table. Fails when `container` is not a
/// table.
fn insert_new(
    container: &mut Item,
    missing: &[String],
    leaf: &str,
    mut value: Value,
    indent: &str,
) -> Result<(), ()> {
    match container {
        Item::Table(table) => {
            let mut key = Key::new(leaf);
            match missing.split_first() {
                None => {
                    key.leaf_decor_mut().set_prefix(indent);
                    table.insert_formatted(&key, Item::Value(value));
                }
                Some((first, rest)) if table.is_dotted() || prefers_dotted(table) => {
                    key.leaf_decor_mut().set_prefix(indent);
                    let mut chain = Table::new();
                    chain.set_dotted(true);
                    chain.insert_formatted(&key, Item::Value(value));
                    for segment in rest.iter().rev() {
                        let mut parent = Table::new();
                        parent.set_dotted(true);
                        parent.insert(segment, Item::Table(chain));
                        chain = parent;
                    }
                    table.insert(first, Item::Table(chain));
                }
                Some((first, rest)) => {
                    // Implicit still renders the header while the table has values, and lets
                    // `remove` drop a table PlaneUI created once it is empty again.
                    let mut chain = Table::new();
                    chain.set_implicit(true);
                    chain.insert_formatted(&key, Item::Value(value));
                    for segment in rest.iter().rev() {
                        let mut parent = Table::new();
                        parent.set_implicit(true);
                        parent.insert(segment, Item::Table(chain));
                        chain = parent;
                    }
                    table.insert(first, Item::Table(chain));
                }
            }
            Ok(())
        }
        Item::Value(Value::InlineTable(table)) => {
            let mut name = leaf;
            for segment in missing.iter().rev() {
                let mut nested = InlineTable::new();
                nested.insert(name, value);
                value = Value::InlineTable(nested);
                name = segment;
            }
            insert_inline(table, name, value);
            Ok(())
        }
        _ => Err(()),
    }
}

/// Appends to an inline table in the style of its last entry, moving the closing spacing from
/// the old last entry to the new one.
fn insert_inline(table: &mut InlineTable, name: &str, mut value: Value) {
    let names: Vec<String> = table.iter().map(|(key, _)| key.to_owned()).collect();
    let mut key = Key::new(name);
    if let Some(last) = names.last() {
        if let Some(last_key) = table.key(last) {
            *key.leaf_decor_mut() = last_key.leaf_decor().clone();
        }
        let interior_suffix = names
            .len()
            .checked_sub(2)
            .and_then(|index| names.get(index))
            .and_then(|name| table.get(name))
            .map_or_else(String::new, |value| raw(value.decor().suffix(), ""));
        if let Some(last_value) = table.get_mut(last) {
            let closing = raw(last_value.decor().suffix(), "");
            let opening = raw(last_value.decor().prefix(), " ");
            last_value.decor_mut().set_suffix(interior_suffix);
            value.decor_mut().set_prefix(opening);
            value.decor_mut().set_suffix(closing);
        }
    }
    table.insert_formatted(&key, value);
}

fn prefers_dotted(table: &Table) -> bool {
    let (dotted, headers) = table.iter().fold(
        (0_usize, 0_usize),
        |(dotted, headers), (_, item)| match item {
            Item::Table(child) if child.is_dotted() => (dotted + 1, headers),
            Item::Table(_) | Item::ArrayOfTables(_) => (dotted, headers + 1),
            _ => (dotted, headers),
        },
    );
    dotted > headers
}

/// The whitespace in front of the last key written in `table`'s body, so new keys line up.
fn indentation(table: &Table) -> String {
    table
        .get_values()
        .last()
        .and_then(|(keys, _)| keys.last())
        .and_then(|key| key.leaf_decor().prefix())
        .and_then(RawString::as_str)
        .map(|prefix| {
            let line = prefix.rsplit('\n').next().unwrap_or_default();
            if line.trim().is_empty() {
                line.to_owned()
            } else {
                String::new()
            }
        })
        .unwrap_or_default()
}

fn raw(text: Option<&RawString>, default: &str) -> String {
    text.and_then(RawString::as_str)
        .unwrap_or(default)
        .to_owned()
}

/// How many leading `parents` lead to the table whose body shows the removed key: dotted
/// tables and inline tables render inside their nearest `[header]` table.
fn visible_table_depth(root: &Table, parents: &[String]) -> usize {
    let mut depth = 0;
    let mut table = root;
    for (index, segment) in parents.iter().enumerate() {
        match table.get(segment) {
            Some(Item::Table(child)) => {
                if !child.is_dotted() {
                    depth = index + 1;
                }
                table = child;
            }
            _ => break,
        }
    }
    depth
}

fn table_mut<'a>(root: &'a mut Table, path: &[String]) -> Option<&'a mut Table> {
    path.iter().try_fold(root, |table, segment| {
        table.get_mut(segment).and_then(Item::as_table_mut)
    })
}

/// Removes `relative` (a path inside the visible table `visible`). Comments above the removed
/// key that are not its own move to the neighbouring line so they are not lost.
fn remove_from(visible: &mut Table, relative: &[String], at_root: bool) -> Option<()> {
    let (leaf, parents) = relative.split_last()?;
    let order: Vec<Vec<String>> = visible
        .get_values()
        .into_iter()
        .map(|(keys, _)| keys.iter().map(|key| key.get().to_owned()).collect())
        .collect();
    let parent = match container_mut(visible, parents)? {
        Container::Inline(table) => return remove_inline(table, leaf),
        Container::Table(table) => table,
    };
    let prefix = raw(parent.key(leaf)?.leaf_decor().prefix(), "");
    if !matches!(parent.remove(leaf)?, Item::Value(_)) {
        return Some(());
    }
    let kept = foreign_comments(&prefix);
    if kept.is_empty() {
        return Some(());
    }
    let index = order.iter().position(|keys| keys.as_slice() == relative)?;
    if let Some(next) = order.get(index + 1) {
        let (next_leaf, next_parents) = next.split_last()?;
        let mut container = container_mut(visible, next_parents)?;
        let mut key = container.key_mut(next_leaf)?;
        let combined = format!("{kept}{}", raw(key.leaf_decor().prefix(), ""));
        key.leaf_decor_mut().set_prefix(combined);
    } else if let Some(previous) = index.checked_sub(1).and_then(|index| order.get(index)) {
        let (previous_leaf, previous_parents) = previous.split_last()?;
        let mut container = container_mut(visible, previous_parents)?;
        let decor = container.value_mut(previous_leaf)?.decor_mut();
        let combined = format!(
            "{}\n{}",
            raw(decor.suffix(), ""),
            without_last_newline(kept)
        );
        decor.set_suffix(combined);
    } else if at_root {
        let decor = visible.decor_mut();
        let combined = format!("{}{kept}", raw(decor.prefix(), ""));
        decor.set_prefix(combined);
    } else {
        let decor = visible.decor_mut();
        let combined = format!(
            "{}\n{}",
            raw(decor.suffix(), ""),
            without_last_newline(kept)
        );
        decor.set_suffix(combined);
    }
    Some(())
}

/// A table-like reached by key path: inline tables need their own spacing rules on removal.
enum Container<'a> {
    Table(&'a mut Table),
    Inline(&'a mut InlineTable),
}

impl Container<'_> {
    fn key_mut(&mut self, name: &str) -> Option<toml_edit::KeyMut<'_>> {
        match self {
            Self::Table(table) => table.key_mut(name),
            Self::Inline(table) => table.key_mut(name),
        }
    }

    fn value_mut(&mut self, name: &str) -> Option<&mut Value> {
        match self {
            Self::Table(table) => table.get_mut(name)?.as_value_mut(),
            Self::Inline(table) => table.get_mut(name),
        }
    }
}

fn container_mut<'a>(table: &'a mut Table, path: &[String]) -> Option<Container<'a>> {
    let Some((first, rest)) = path.split_first() else {
        return Some(Container::Table(table));
    };
    let item = rest
        .iter()
        .try_fold(table.get_mut(first)?, |item, segment| {
            item.as_table_like_mut()?.get_mut(segment)
        })?;
    match item {
        Item::Table(table) => Some(Container::Table(table)),
        Item::Value(Value::InlineTable(table)) => Some(Container::Inline(table)),
        _ => None,
    }
}

/// Removes `leaf` from an inline table, handing the opening or closing spacing to the entry
/// that takes over the edge slot.
fn remove_inline(table: &mut InlineTable, leaf: &str) -> Option<()> {
    let names: Vec<String> = table.iter().map(|(key, _)| key.to_owned()).collect();
    let index = names.iter().position(|name| name == leaf)?;
    let key_prefix = raw(table.key(leaf)?.leaf_decor().prefix(), " ");
    let value_suffix = raw(table.get(leaf)?.decor().suffix(), "");
    table.remove(leaf)?;
    if index == 0 {
        if let Some(mut next) = names.get(1).and_then(|name| table.key_mut(name)) {
            next.leaf_decor_mut().set_prefix(key_prefix);
        }
    } else if index + 1 == names.len()
        && let Some(previous) = names.get(index - 1).and_then(|name| table.get_mut(name))
    {
        previous.decor_mut().set_suffix(value_suffix);
    }
    Some(())
}

/// An element prefix reduced to its line break and indentation, dropping comment lines.
fn without_comment_prefix(prefix: &str) -> String {
    match prefix.rsplit_once('\n') {
        Some((_, indent)) => format!("\n{indent}"),
        None => prefix.to_owned(),
    }
}

fn without_last_newline(text: &str) -> &str {
    text.strip_suffix('\n').unwrap_or(text)
}

/// The complete lines of a key's prefix above its own comment block: everything up to and
/// including the last line that is not a comment.
fn foreign_comments(prefix: &str) -> &str {
    let mut own_start = prefix.rfind('\n').map_or(0, |index| index + 1);
    while own_start > 0 {
        let before = prefix.get(..own_start - 1).unwrap_or_default();
        let line_start = before.rfind('\n').map_or(0, |index| index + 1);
        let line = before.get(line_start..).unwrap_or_default();
        if !line.trim_start().starts_with('#') {
            break;
        }
        own_start = line_start;
    }
    prefix.get(..own_start).unwrap_or_default()
}

/// Decor of the slots at an inline array's edges and between elements, so elements that move
/// into or out of an edge slot take that slot's spacing.
#[derive(Debug)]
struct EdgeDecor {
    first_prefix: String,
    last_suffix: String,
    interior_prefix: String,
    interior_suffix: String,
}

impl EdgeDecor {
    fn capture(array: &Array) -> Self {
        let prefix = |index: usize, default: &str| {
            array.get(index).map_or_else(
                || default.to_owned(),
                |value| raw(value.decor().prefix(), default),
            )
        };
        let suffix = |index: usize| {
            array
                .get(index)
                .map_or_else(String::new, |value| raw(value.decor().suffix(), ""))
        };
        let first_prefix = prefix(0, "");
        let interior_prefix = if array.len() >= 2 {
            without_comment_prefix(&prefix(1, " "))
        } else if first_prefix.contains('\n') {
            first_prefix.clone()
        } else {
            " ".to_owned()
        };
        let interior_suffix = if array.len() >= 2 && !suffix(0).contains('#') {
            suffix(0)
        } else {
            String::new()
        };
        Self {
            last_suffix: suffix(array.len().saturating_sub(1)),
            first_prefix,
            interior_prefix,
            interior_suffix,
        }
    }

    /// `origins[i]` is the old index of the element now at `i`, `None` for a new element.
    fn restyle(&self, array: &mut Array, origins: &[Option<usize>], old_len: usize) {
        let new_len = array.len();
        for (index, value) in array.iter_mut().enumerate() {
            let Some(origin) = origins.get(index).copied() else {
                continue;
            };
            let (prefix, suffix) = self.slot(index, new_len);
            let decor = value.decor_mut();
            match origin {
                None => {
                    decor.set_prefix(prefix);
                    decor.set_suffix(suffix);
                }
                Some(old) => {
                    let (old_prefix, old_suffix) = self.slot(old, old_len);
                    if old_prefix != prefix && raw(decor.prefix(), "") == old_prefix {
                        decor.set_prefix(prefix);
                    }
                    if old_suffix != suffix && raw(decor.suffix(), "") == old_suffix {
                        decor.set_suffix(suffix);
                    }
                }
            }
        }
    }

    fn slot(&self, index: usize, len: usize) -> (&str, &str) {
        let prefix = if index == 0 {
            &self.first_prefix
        } else {
            &self.interior_prefix
        };
        let suffix = if index + 1 == len {
            &self.last_suffix
        } else {
            &self.interior_suffix
        };
        (prefix, suffix)
    }
}

/// Drops implicit or dotted tables on `parents` that the removal left empty, deepest first.
fn prune_empty_tables(root: &mut Table, parents: &[String]) {
    for depth in (1..=parents.len()).rev() {
        let Some((leaf, path)) = parents.get(..depth).and_then(<[String]>::split_last) else {
            continue;
        };
        let Some(parent) = table_mut(root, path) else {
            continue;
        };
        let removable = matches!(
            parent.get(leaf),
            Some(Item::Table(table)) if table.is_empty() && (table.is_implicit() || table.is_dotted())
        );
        if removable {
            parent.remove(leaf);
        }
    }
}

impl fmt::Display for ConfigDoc {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.line_ending {
            LineEnding::Lf => self.document.fmt(formatter),
            LineEnding::Crlf => formatter.write_str(&to_crlf(&self.document.to_string())),
            LineEnding::Mixed(source) => formatter.write_str(source),
        }
    }
}

/// Raw string reprs keep their original `\r\n`, so only bare `\n` is converted.
fn to_crlf(text: &str) -> String {
    let mut output = String::with_capacity(text.len() + text.len() / 16);
    let mut previous = None;
    for character in text.chars() {
        if character == '\n' && previous != Some('\r') {
            output.push('\r');
        }
        output.push(character);
        previous = Some(character);
    }
    output
}

/// A TOML syntax error located in the source text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("line {line}, column {column}: {message}")]
pub struct ParseError {
    /// The parser's description of the problem.
    pub message: String,
    /// 1-based line of the error.
    pub line: usize,
    /// 1-based column of the error, counted in characters.
    pub column: usize,
}

impl ParseError {
    fn from_toml(source: &str, error: &toml_edit::TomlError) -> Self {
        let offset = error.span().map_or(0, |span| span.start);
        let (line, column) = line_and_column(source, offset);
        Self {
            message: error.message().trim_end().to_owned(),
            line,
            column,
        }
    }
}

fn line_and_column(source: &str, offset: usize) -> (usize, usize) {
    let before = source.get(..offset).unwrap_or(source);
    let line_start = before.rfind('\n').map_or(0, |newline| newline + 1);
    let line = before.matches('\n').count() + 1;
    let column = before
        .get(line_start..)
        .map_or(0, |prefix| prefix.chars().count())
        + 1;
    (line, column)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edited(source: &str, edit: impl FnOnce(&mut ConfigDoc) -> Result<(), EditError>) -> String {
        let mut document = ConfigDoc::parse(source).unwrap();
        edit(&mut document).unwrap();
        let output = document.to_string();
        ConfigDoc::parse(&output).expect("edited output re-parses");
        output
    }

    type ArrayEdit = fn(&mut ConfigDoc, &KeyPath) -> Result<(), EditError>;

    fn int(value: i64) -> Value {
        Value::from(value)
    }

    #[test]
    fn key_path_displays_as_dotted_toml() {
        let path = KeyPath::from(["workspace-to-monitor-force-assignment", "4", "a b"]);
        assert_eq!(
            path.to_string(),
            "workspace-to-monitor-force-assignment.4.\"a b\""
        );
    }

    #[test]
    fn get_resolves_every_style() {
        let path = KeyPath::from(["gaps", "outer", "left"]);
        for source in [
            "[gaps.outer]\nleft = 8\n",
            "[gaps]\nouter.left = 8\n",
            "gaps.outer.left = 8\n",
            "gaps = { outer = { left = 8 } }\n",
        ] {
            let document = ConfigDoc::parse(source).unwrap();
            let value = document.get(&path).and_then(Item::as_integer);
            assert_eq!(value, Some(8), "{source}");
        }
    }

    #[test]
    fn set_existing_key_keeps_its_style_and_comments() {
        let path = KeyPath::from(["gaps", "outer", "left"]);
        for (source, expected) in [
            (
                "[gaps.outer]\n# left gap\nleft =  8 # px\n",
                "[gaps.outer]\n# left gap\nleft =  4 # px\n",
            ),
            (
                "[gaps]\n    outer.left = 8 # px\n",
                "[gaps]\n    outer.left = 4 # px\n",
            ),
            ("gaps.outer.left = 8\n", "gaps.outer.left = 4\n"),
            (
                "gaps = { outer = { left = 8 } }\n",
                "gaps = { outer = { left = 4 } }\n",
            ),
        ] {
            assert_eq!(
                edited(source, |document| document.set_value(&path, int(4))),
                expected
            );
        }
    }

    #[test]
    fn new_root_key_goes_above_the_first_table() {
        let output = edited("a = 1\n\n[t]\nb = 2\n", |document| {
            document.set_value(&KeyPath::from(["c"]), int(3))
        });
        assert_eq!(output, "a = 1\nc = 3\n\n[t]\nb = 2\n");
        let reparsed = ConfigDoc::parse(&output).unwrap();
        assert!(reparsed.document().as_table().contains_value("c"));
    }

    #[test]
    fn new_key_follows_sibling_style() {
        let source = "[gaps]\n    inner.horizontal = 10\n    outer.left = 8\n";
        let output = edited(source, |document| {
            document.set_value(&KeyPath::from(["gaps", "inner", "vertical"]), int(10))?;
            document.set_value(&KeyPath::from(["gaps", "outer", "right"]), int(8))
        });
        assert_eq!(
            output,
            "[gaps]\n    inner.horizontal = 10\n    inner.vertical = 10\n    outer.left = 8\n    outer.right = 8\n"
        );
    }

    #[test]
    fn new_section_follows_parent_style() {
        let path = KeyPath::from(["focus-follows-mouse", "enabled"]);
        let dotted = edited("gaps.inner.vertical = 1\n", |document| {
            document.set_value(&path, Value::from(true))
        });
        assert_eq!(
            dotted,
            "gaps.inner.vertical = 1\nfocus-follows-mouse.enabled = true\n"
        );
        let tables = edited("[gaps.inner]\nvertical = 1\n", |document| {
            document.set_value(&path, Value::from(true))
        });
        assert_eq!(
            tables,
            "[gaps.inner]\nvertical = 1\n\n[focus-follows-mouse]\nenabled = true\n"
        );
        let inline = edited("gaps = { inner = { vertical = 1 } }\n", |document| {
            document.set_value(&KeyPath::from(["gaps", "outer", "left"]), int(2))
        });
        assert_eq!(
            inline,
            "gaps = { inner = { vertical = 1 }, outer = { left = 2 } }\n"
        );
    }

    #[test]
    fn remove_takes_only_its_own_comment() {
        let output = edited("a = 1\n\n# about b\nb = 2\nc = 3\n", |document| {
            document.remove(&KeyPath::from(["b"]))
        });
        assert_eq!(output, "a = 1\n\nc = 3\n");
    }

    #[test]
    fn remove_keeps_comments_separated_by_a_blank_line() {
        let output = edited("a = 1\n# section notes\n\nb = 2\n", |document| {
            document.remove(&KeyPath::from(["b"]))
        });
        assert_eq!(output, "a = 1\n# section notes\n\n");
    }

    #[test]
    fn remove_drops_empty_dotted_parents_but_keeps_headers() {
        let dotted = edited("gaps.outer.left = 8\na = 1\n", |document| {
            document.remove(&KeyPath::from(["gaps", "outer", "left"]))
        });
        assert_eq!(dotted, "a = 1\n");
        let header = edited("[key-mapping]\npreset = 'qwerty'\n", |document| {
            document.remove(&KeyPath::from(["key-mapping", "preset"]))
        });
        assert_eq!(header, "[key-mapping]\n");
    }

    #[test]
    fn remove_from_inline_table_keeps_its_spacing() {
        let output = edited("t = { a = 1, b = 2 }\n", |document| {
            document.remove(&KeyPath::from(["t", "b"]))
        });
        assert_eq!(output, "t = { a = 1 }\n");
        let output = edited("t = { a = 1, b = 2 }\n", |document| {
            document.remove(&KeyPath::from(["t", "a"]))
        });
        assert_eq!(output, "t = { b = 2 }\n");
    }

    #[test]
    fn edit_errors_name_the_key_path() {
        let mut document = ConfigDoc::parse("gaps.outer.left = 8\n").unwrap();
        let cases = [
            (
                document.set_value(&KeyPath::from(["gaps", "outer", "left", "x"]), int(1)),
                "gaps.outer.left is not a table",
            ),
            (
                document.set_value(&KeyPath::from(["gaps", "outer"]), int(1)),
                "gaps.outer is a table, not a value",
            ),
            (
                document.remove(&KeyPath::from(["gaps", "inner"])),
                "gaps.inner does not exist",
            ),
            (
                document.array_insert(&KeyPath::from(["gaps", "outer", "left"]), 0, int(1)),
                "gaps.outer.left is not an array",
            ),
        ];
        for (result, message) in cases {
            assert_eq!(result.unwrap_err().to_string(), message);
        }
        assert_eq!(document.to_string(), "gaps.outer.left = 8\n");
    }

    #[test]
    fn inline_array_edits_keep_edge_spacing() {
        let path = KeyPath::from(["a"]);
        let cases: [(&str, ArrayEdit, &str); 5] = [
            (
                "a = ['x', 'y']\n",
                |d, p| d.array_insert(p, 2, Value::from("z")),
                "a = ['x', 'y', \"z\"]\n",
            ),
            (
                "a = [ 'x', 'y' ]\n",
                |d, p| d.array_insert(p, 0, Value::from("z")),
                "a = [ \"z\", 'x', 'y' ]\n",
            ),
            (
                "a = [ 'x', 'y' ]\n",
                |d, p| d.array_remove(p, 1),
                "a = [ 'x' ]\n",
            ),
            (
                "a = ['x', 'y', 'z']\n",
                |d, p| d.array_move(p, 2, 0),
                "a = ['z', 'x', 'y']\n",
            ),
            (
                "a = [\n    'x', # first\n    'y',\n]\n",
                |d, p| d.array_insert(p, 2, Value::from("z")),
                "a = [\n    'x', # first\n    'y',\n    \"z\",\n]\n",
            ),
        ];
        for (source, edit, expected) in cases {
            assert_eq!(edited(source, |document| edit(document, &path)), expected);
        }
    }

    #[test]
    fn moving_and_appending_rules_keeps_their_sub_tables() {
        let source =
            "[[r]]\nrun = 'a'\n[r.if]\napp-id = 'a'\n\n[[r]]\nrun = 'b'\n[r.if]\napp-id = 'b'\n";
        let path = KeyPath::from(["r"]);
        let moved = edited(source, |document| document.array_move(&path, 0, 1));
        assert_eq!(
            moved,
            "\n[[r]]\nrun = 'b'\n[r.if]\napp-id = 'b'\n[[r]]\nrun = 'a'\n[r.if]\napp-id = 'a'\n"
        );
        let mut rule = InlineTable::new();
        rule.insert("run", Value::from("c"));
        let appended = edited(source, |document| {
            document.array_insert(&path, 2, Value::InlineTable(rule))
        });
        assert_eq!(appended, format!("{source}\n[[r]]\nrun = \"c\"\n"));
    }

    #[test]
    fn moving_keeps_each_elements_sub_table_order() {
        let source = "[[a]]\nid=1\n[a.x]\nv=1\n[a.y]\nv=2\n[a.x.z]\nv=3\n[[a]]\nid=2\n[a.x]\nv=4\n";
        let path = KeyPath::from(["a"]);
        assert_eq!(
            edited(source, |document| document.array_move(&path, 0, 0)),
            source
        );
        let there = edited(source, |document| document.array_move(&path, 0, 1));
        assert_eq!(
            there,
            "[[a]]\nid=2\n[a.x]\nv=4\n[[a]]\nid=1\n[a.x]\nv=1\n[a.y]\nv=2\n[a.x.z]\nv=3\n"
        );
        let back = edited(&there, |document| document.array_move(&path, 1, 0));
        assert_eq!(back, source);
    }

    #[test]
    fn normalizing_positions_never_changes_rendering() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|extension| extension != "toml") {
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap();
            let mut document = ConfigDoc::parse(&source).unwrap();
            normalize_positions(document.document.as_table_mut());
            assert_eq!(document.to_string(), source, "{}", path.display());
        }
    }

    #[test]
    fn moving_an_inserted_element_and_back_is_exact() {
        let source = "[[a]]\nid=1\n[b]\nv=9\n[a.x]\nv=1\n[[a]]\nid=2\n";
        let path = KeyPath::from(["a"]);
        let mut element = InlineTable::new();
        element.insert("id", int(3));
        let inserted = edited(source, |document| {
            document.array_insert(&path, 0, Value::InlineTable(element))
        });
        assert!(inserted.find("[b]") < inserted.find("[a.x]"), "{inserted}");
        let there_and_back = edited(&inserted, |document| {
            document.array_move(&path, 0, 1)?;
            document.array_move(&path, 1, 0)
        });
        assert_eq!(there_and_back, inserted);
        let mut document = ConfigDoc::parse(source).unwrap();
        let mut element = InlineTable::new();
        element.insert("id", int(3));
        document
            .array_insert(&path, 0, Value::InlineTable(element))
            .unwrap();
        document.array_move(&path, 0, 1).unwrap();
        document.array_move(&path, 1, 0).unwrap();
        assert_eq!(document.to_string(), inserted);
    }

    #[test]
    fn mixed_line_endings_round_trip_but_refuse_edits() {
        let source = "a = 1\r\nb = 2\n";
        let mut document = ConfigDoc::parse(source).unwrap();
        assert_eq!(document.to_string(), source);
        let error = document
            .set_value(&KeyPath::from(["b"]), int(3))
            .unwrap_err();
        assert_eq!(error, EditError::MixedLineEndings);
        assert_eq!(document.to_string(), source);
    }

    #[test]
    fn array_index_out_of_bounds_is_an_error() {
        let mut document = ConfigDoc::parse("a = [1]\n").unwrap();
        let error = document.array_remove(&KeyPath::from(["a"]), 1).unwrap_err();
        assert_eq!(error.to_string(), "a has no element 1 (length 1)");
    }

    #[test]
    fn parse_error_reports_line_and_column() {
        let error = ConfigDoc::parse("gaps = 1\nkey = = 2\n").unwrap_err();
        assert_eq!((error.line, error.column), (2, 7));
    }

    #[test]
    fn parse_error_counts_columns_in_characters() {
        let error = ConfigDoc::parse("é = 'x' ?\n").unwrap_err();
        assert_eq!((error.line, error.column), (1, 9));
    }

    #[test]
    fn display_reproduces_source() {
        let source = "# comment\na =   1 # trailing\n";
        assert_eq!(ConfigDoc::parse(source).unwrap().to_string(), source);
    }

    #[test]
    fn display_keeps_crlf_line_endings() {
        let source = "a = 1\r\n\r\n[t]\r\nb = '''x\r\ny'''\r\n";
        assert_eq!(ConfigDoc::parse(source).unwrap().to_string(), source);
    }
}
