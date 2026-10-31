//! Exact, unlimited in-session undo and redo. Configs are small, so each entry stores whole
//! documents instead of inverse edits; undoing is then a plain write of the stored text.

/// Identifies one recorded change. Texts can repeat (1 → 2 → 1), so positions in history are
/// tracked by id, never by content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EntryId(u64);

/// One committed change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Unique within one [`History`].
    pub id: EntryId,
    /// Document text before the change.
    pub before: String,
    /// Document text after the change.
    pub after: String,
    /// Short plain-language description, e.g. "Set accordion-padding to 20".
    pub label: String,
    /// The file was reloaded from disk after this change, so undoing it reaches back past an
    /// edit made outside PlaneUI.
    pub crosses_external_change: bool,
}

/// The document text an undo or redo should write, with what it reverts or reapplies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryStep {
    /// Text to write.
    pub text: String,
    /// The label of the change being undone or redone.
    pub label: String,
    /// Whether the step reaches back past an external edit.
    pub crosses_external_change: bool,
}

/// Undo and redo stacks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct History {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    next_id: u64,
}

impl History {
    /// An empty history.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records a committed change. A new change clears the redo stack.
    pub fn record(&mut self, before: String, after: String, label: impl Into<String>) {
        self.redo.clear();
        let id = EntryId(self.next_id);
        self.next_id += 1;
        self.undo.push(Entry {
            id,
            before,
            after,
            label: label.into(),
            crosses_external_change: false,
        });
    }

    /// Pops the latest change; the returned text is the document before it.
    pub fn undo(&mut self) -> Option<HistoryStep> {
        let entry = self.undo.pop()?;
        let step = HistoryStep {
            text: entry.before.clone(),
            label: entry.label.clone(),
            crosses_external_change: entry.crosses_external_change,
        };
        self.redo.push(entry);
        Some(step)
    }

    /// Re-applies the latest undone change; the returned text is the document after it.
    pub fn redo(&mut self) -> Option<HistoryStep> {
        let entry = self.redo.pop()?;
        let step = HistoryStep {
            text: entry.after.clone(),
            label: entry.label.clone(),
            crosses_external_change: entry.crosses_external_change,
        };
        self.undo.push(entry);
        Some(step)
    }

    /// Marks every recorded change as older than an external edit. History is kept; the write
    /// pipeline checks the disk before any undo lands, like any other write.
    pub fn note_external_change(&mut self) {
        for entry in self.undo.iter_mut().chain(self.redo.iter_mut()) {
            entry.crosses_external_change = true;
        }
    }

    /// The latest change the current document includes, or `None` before any change. A write
    /// carries the head it represents, so the caller knows which history state reached disk.
    #[must_use]
    pub fn head(&self) -> Option<EntryId> {
        self.undo.last().map(|entry| entry.id)
    }

    /// Moves history back to `head`, the last state that reached the disk, so undo cannot
    /// resurrect changes that were discarded before being written. A discarded undo is taken
    /// back by replaying redo entries; discarded commits are dropped.
    pub fn rewind_to(&mut self, head: Option<EntryId>) {
        if self.head() == head {
            return;
        }
        if let Some(index) = self.redo.iter().position(|entry| Some(entry.id) == head) {
            while self.redo.len() > index {
                if let Some(entry) = self.redo.pop() {
                    self.undo.push(entry);
                }
            }
            return;
        }
        while self.head().is_some() && self.head() != head {
            self.undo.pop();
        }
    }

    /// The label of the change [`Self::undo`] would revert.
    #[must_use]
    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|entry| entry.label.as_str())
    }

    /// The label of the change [`Self::redo`] would reapply.
    #[must_use]
    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|entry| entry.label.as_str())
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn new_record_clears_redo() {
        let mut history = History::new();
        history.record("a".into(), "b".into(), "first");
        history.undo();
        history.record("a".into(), "c".into(), "second");
        assert_eq!(history.redo(), None);
        assert_eq!(history.undo_label(), Some("second"));
    }

    #[test]
    fn external_change_marks_existing_entries_only() {
        let mut history = History::new();
        history.record("a".into(), "b".into(), "before reload");
        history.note_external_change();
        history.record("x".into(), "y".into(), "after reload");
        assert!(!history.undo().unwrap().crosses_external_change);
        assert!(history.undo().unwrap().crosses_external_change);
    }

    #[test]
    fn rewind_drops_commits_that_never_reached_the_disk_even_with_repeated_text() {
        let mut history = History::new();
        history.record("30".into(), "1".into(), "written");
        let on_disk = history.head();
        history.record("1".into(), "2".into(), "discarded 2");
        history.record("2".into(), "1".into(), "discarded back to 1");
        history.rewind_to(on_disk);
        assert_eq!(history.undo_label(), Some("written"));
        history.rewind_to(None);
        assert_eq!(history.undo_label(), None);
    }

    #[test]
    fn rewind_takes_back_a_discarded_undo() {
        let mut history = History::new();
        history.record("a".into(), "b".into(), "first");
        history.record("b".into(), "c".into(), "second");
        let on_disk = history.head();
        history.undo();
        history.undo();
        history.rewind_to(on_disk);
        assert_eq!(history.undo_label(), Some("second"));
        assert_eq!(history.redo(), None);
    }

    proptest! {
        #[test]
        fn undo_all_restores_the_original_and_redo_replays(
            texts in proptest::collection::vec(".{0,40}", 1..20),
        ) {
            let original = "original".to_owned();
            let mut history = History::new();
            let mut current = original.clone();
            for text in &texts {
                history.record(current.clone(), text.clone(), "edit");
                current.clone_from(text);
            }
            let latest = current.clone();
            for _ in &texts {
                current = history.undo().unwrap().text;
            }
            prop_assert_eq!(&current, &original);
            prop_assert!(history.undo().is_none());
            for _ in &texts {
                current = history.redo().unwrap().text;
            }
            prop_assert_eq!(current, latest);
            prop_assert!(history.redo().is_none());
        }
    }
}
